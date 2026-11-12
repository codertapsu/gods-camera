//! `gc camera`: the live face swap, shown through the Windows camera.
//!
//! It does what the app will do: make the pipe, ask Windows for the camera,
//! and while something watches, run every frame through the live engine, draw
//! the outgoing picture and hand it over. The frames come from a moving-face
//! clip made from a photo, so it needs no camera of its own.

use crate::face::{percentile, Picture};
use anyhow::{bail, Context, Result};
use gc_live::synthetic::{FaceClip, Motion};
use gc_live::{Enroller, ImageView, LiveConfig, LiveEngine, LiveSettings, PixelFormat, SwapModel};
use gc_win::{Feed, FeedError, VirtualCamera, Wanted};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct CameraArgs {
    pub packs: PathBuf,
    /// A photo with at least one face. The leftmost face moves in the clip.
    pub photo: PathBuf,
    /// The face to swap in. Defaults to the rightmost face in `photo`.
    pub source: Option<PathBuf>,
    pub model: SwapModel,
    pub motion: Motion,
    /// Stop after this long. Without it, run until Ctrl+C.
    pub seconds: Option<f32>,
    /// Stop once the first app that watched has stopped watching.
    pub once: bool,
    /// Only the pipe: for tests that load the media source themselves.
    pub no_camera: bool,
    /// A pipe name of the caller's own, for a test that stands in for the media source.
    pub pipe: Option<String>,
    /// Send the clip as it is, to tell the camera's cost from the engine's.
    pub no_swap: bool,
    pub no_occlusion: bool,
    pub cache_dir: Option<PathBuf>,
    pub json: Option<PathBuf>,
    /// Set when the user asks to stop.
    pub stop: Arc<AtomicBool>,
}

/// The clip's own size, as a webcam's would be.
const CLIP: (u32, u32) = (1280, 720);

/// Times for one stage of the work, in milliseconds.
#[derive(Default)]
struct Stage(Vec<f32>);

impl Stage {
    fn add(&mut self, since: Instant) {
        self.0.push(since.elapsed().as_secs_f32() * 1000.0);
    }

    fn report(&mut self) -> serde_json::Value {
        self.0.sort_by(f32::total_cmp);
        serde_json::json!({
            "median": percentile(&self.0, 0.5), "p95": percentile(&self.0, 0.95), "max": percentile(&self.0, 1.0)
        })
    }
}

pub fn run(args: CameraArgs) -> Result<()> {
    let feed = match &args.pipe {
        Some(name) => Feed::open_named(name),
        None => Feed::open(),
    };
    let feed = feed.map_err(|error| match error {
        FeedError::Taken => anyhow::anyhow!(
            "another program already holds the camera's pipe. Is the app, another gc camera or gc-vcam-host running?"
        ),
        other => anyhow::anyhow!(other),
    })?;
    let camera = match args.no_camera {
        true => None,
        false => Some(VirtualCamera::start(feed.pipe_name()).map_err(|error| {
            let advice = match error.not_installed() {
                true => " Build it with tools\\build-windows.ps1, then run tools\\register-windows-camera.ps1 in a PowerShell window opened as administrator.",
                false => "",
            };
            anyhow::anyhow!("{error}.{advice}")
        })?),
    };

    let photo = Picture::load(&args.photo)?;
    let mut enroller = Enroller::new(std::slice::from_ref(&args.packs), args.cache_dir.as_deref())?;
    let mut faces = enroller.detect(&photo.view())?;
    faces.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
    let actor = faces.first().cloned().with_context(|| format!("no face found in {}", args.photo.display()))?;
    let clip = FaceClip::new(&photo.view(), &actor, CLIP.0, CLIP.1, CLIP.0 as f32 * 0.1);
    let stride = CLIP.0 as usize * 4;
    let mut frame = Vec::new();

    let engine = match args.no_swap {
        true => None,
        false => {
            let identity = match &args.source {
                Some(path) => {
                    let source = Picture::load(path)?;
                    enroller.enrol(&source.view())?.with_context(|| format!("no face found in {}", path.display()))?.identity
                }
                None => enroller.identity(&photo.view(), &faces[faces.len() - 1].kps)?,
            };
            let engine = LiveEngine::new(&LiveConfig {
                packs: vec![args.packs.clone()],
                cache_dir: args.cache_dir.clone(),
                detector: gc_live::DetectorKind::Large,
                detector_size: 640,
                swap_model: args.model,
                cpu_only: false,
            })?;
            engine.set_settings(LiveSettings { occlusion: !args.no_occlusion, ..LiveSettings::default() });
            engine.set_identity(Some(identity));
            // The first runs of a model are slow. Get them over with before anything watches.
            let warm_started = Instant::now();
            clip.render(&clip.pose(args.motion, 0.0), &mut frame);
            let mut clock_ns = 1u64;
            while engine.patch().is_none() && warm_started.elapsed() < Duration::from_secs(120) {
                let view = ImageView::new(&frame, CLIP.0, CLIP.1, stride, PixelFormat::Bgra).context("frame")?;
                engine.push_frame(&view, clock_ns);
                clock_ns += 33_333_333;
                std::thread::sleep(Duration::from_millis(33));
            }
            if engine.patch().is_none() {
                bail!("no swapped patch after two minutes: {:?}", engine.stats().last_error);
            }
            Some(engine)
        }
    };
    drop(enroller);

    match &camera {
        Some(_) => eprintln!("The camera \"God's Camera\" is in every app's camera list for as long as this runs."),
        None => eprintln!("The pipe is open. No camera was asked of Windows."),
    }

    let started = Instant::now();
    let mut watching: Option<(Wanted, Instant)> = None;
    let mut compositor: Option<gc_render::Compositor> = None;
    let mut outgoing = Vec::new();
    let (mut made, mut refused, mut swapped, mut watched_s) = (0u64, 0u64, 0u64, 0f32);
    let (mut clip_ms, mut engine_ms, mut compose_ms, mut send_ms) =
        (Stage::default(), Stage::default(), Stage::default(), Stage::default());
    let mut sizes: Vec<String> = Vec::new();
    let mut due = Instant::now();
    let engine_before = engine.as_ref().map(|engine| engine.stats());

    while !args.stop.load(Ordering::Relaxed) && args.seconds.is_none_or(|limit| started.elapsed().as_secs_f32() < limit) {
        let wanted = feed.watched();
        if wanted != watching.map(|(wanted, _)| wanted) {
            if let Some((_, since)) = watching {
                watched_s += since.elapsed().as_secs_f32();
            }
            match wanted {
                Some(wanted) => {
                    eprintln!("Something is watching: it asked for {} x {}.", wanted.width, wanted.height);
                    let size = format!("{}x{}", wanted.width, wanted.height);
                    if !sizes.contains(&size) {
                        sizes.push(size);
                    }
                    compositor = Some(gc_render::Compositor::new(wanted.width, wanted.height));
                    due = Instant::now();
                }
                None => {
                    eprintln!("Nothing is watching.");
                    if args.once {
                        watching = None;
                        break;
                    }
                }
            }
            watching = wanted.map(|wanted| (wanted, Instant::now()));
        }
        let (Some((wanted, _)), Some(compositor)) = (watching, compositor.as_mut()) else {
            // Nothing reads the camera, so nothing is made.
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };

        // One picture a frame period, on a schedule that does not drift.
        let period = Duration::from_secs_f64(wanted.frame_rate.1 as f64 / wanted.frame_rate.0 as f64);
        let now = Instant::now();
        match due.checked_duration_since(now) {
            Some(wait) => std::thread::sleep(wait),
            // Fell behind by more than a frame: skip the pictures that are already late.
            None if now.duration_since(due) > period => due = now,
            None => {}
        }
        due += period;

        let stage = Instant::now();
        let taken = gc_win::now_100ns();
        let seconds = started.elapsed().as_secs_f32();
        clip.render(&clip.pose(args.motion, seconds), &mut frame);
        clip_ms.add(stage);
        let view = ImageView::new(&frame, CLIP.0, CLIP.1, stride, PixelFormat::Bgra).context("frame")?;
        let mut scene = gc_render::Scene::of(view);

        let stage = Instant::now();
        let result = engine.as_ref().map(|engine| engine.push_frame(&view, (seconds as f64 * 1e9) as u64 + 5_000_000_000));
        let patch = match (&engine, &result) {
            (Some(engine), Some(result)) => engine.patch().filter(|_| result.patch_sequence > 0 && result.opacity > 0.0),
            _ => None,
        };
        engine_ms.add(stage);

        let stage = Instant::now();
        if let Some(result) = &result {
            scene.blur = result.blur;
            scene.patch = patch.as_ref().map(|patch| gc_render::Patch {
                rgba: &patch.rgba,
                size: patch.size,
                crop_to_frame: result.crop_to_frame,
                opacity: result.opacity,
            });
        }
        swapped += scene.patch.as_ref().is_some_and(|patch| patch.opacity > 0.9) as u64;
        compositor.draw(&scene);
        compositor.nv12(&mut outgoing);
        compose_ms.add(stage);

        let stage = Instant::now();
        // The time is when the frame was taken, so a reader measures the whole way through.
        match feed.send(&outgoing, wanted.width, wanted.height, taken) {
            true => made += 1,
            false => refused += 1,
        }
        send_ms.add(stage);
    }
    if let Some((_, since)) = watching {
        watched_s += since.elapsed().as_secs_f32();
    }

    // Give the last picture a moment to leave before the pipe closes.
    std::thread::sleep(Duration::from_millis(50));
    let stats = feed.stats();
    let engine_report = match (&engine, engine_before) {
        (Some(engine), Some(before)) => {
            let after = engine.stats();
            serde_json::json!({
                "model": format!("{:?}", args.model),
                "occlusion_mask": after.occlusion,
                "swaps_per_second": (after.swaps - before.swaps) as f32 / watched_s.max(0.001),
                "swap_ms": after.swap_ms,
                "swap_age_ms": after.swap_age_ms,
                "detector_ms": after.detect_ms,
                "last_error": after.last_error,
            })
        }
        _ => serde_json::Value::Null,
    };
    drop(camera);
    drop(feed);

    let report = serde_json::json!({
        "tool": "gc camera",
        "camera": if args.no_camera { "not asked for" } else { "started" },
        "seconds": started.elapsed().as_secs_f32(),
        "watched_seconds": watched_s,
        "sizes_asked_for": sizes,
        "frames_made": made,
        "frames_made_per_watched_second": made as f32 / watched_s.max(0.001),
        "frames_not_taken": refused,
        "frames_with_the_swapped_face": swapped,
        "frames_sent": stats.sent,
        "frames_dropped": stats.dropped,
        "connects": stats.connects,
        "write_failures": stats.write_failures,
        "clip_ms": clip_ms.report(),
        "engine_ms": engine_ms.report(),
        "compose_ms": compose_ms.report(),
        "send_ms": send_ms.report(),
        "engine": engine_report,
    });
    println!("{}", serde_json::to_string(&report)?);
    if let Some(path) = &args.json {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    Ok(())
}
