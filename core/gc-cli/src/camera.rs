//! `gc camera`: the live face swap, shown through the Windows camera.
//!
//! It does what the app does: make the pipe, ask Windows for the camera, and
//! while something watches, run the camera engine on every frame. The frames
//! come from a moving-face clip made from a photo, so it needs no camera of
//! its own.

use crate::face::{percentile, Picture};
use anyhow::{bail, Context, Result};
use gc_camera::{CameraEngine, Mode, SwapStage};
use gc_live::synthetic::{FaceClip, Motion};
use gc_live::{Enroller, ImageView, LiveConfig, LiveEngine, LiveSettings, PixelFormat, SwapModel};
use gc_win::{CameraOutput, Feed, FeedError, VirtualCamera, Wanted};
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
    /// After this many seconds of live picture, record a loop and play it.
    pub loop_after: Option<f32>,
    pub cache_dir: Option<PathBuf>,
    pub json: Option<PathBuf>,
    /// Set when the user asks to stop.
    pub stop: Arc<AtomicBool>,
}

/// The clip's own size, as a webcam's would be.
const CLIP: (u32, u32) = (1280, 720);
/// How long the loop recorded by `--loop-after` is.
const LOOP_SECONDS: f32 = 5.0;

/// Where `--loop-after` has got to.
enum LoopDemo {
    Off,
    Waiting { since: Instant },
    Recording { since: Instant },
    AwaitingClip,
    Playing,
}

fn median_and_tail(times: &mut [f32]) -> serde_json::Value {
    times.sort_by(f32::total_cmp);
    serde_json::json!({ "median": percentile(times, 0.5), "p95": percentile(times, 0.95), "max": percentile(times, 1.0) })
}

pub fn run(args: CameraArgs) -> Result<()> {
    let feed = match &args.pipe {
        Some(name) => Feed::open_named(name),
        None => Feed::open(),
    };
    let feed = Arc::new(feed.map_err(|error| match error {
        FeedError::Taken => anyhow::anyhow!(
            "another program already holds the camera's pipe. Is the app, another gc camera or gc-vcam-host running?"
        ),
        other => anyhow::anyhow!(other),
    })?);
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

    let swap = match args.no_swap {
        true => None,
        false => {
            let identity = match &args.source {
                Some(path) => {
                    let source = Picture::load(path)?;
                    enroller.enrol(&source.view())?.with_context(|| format!("no face found in {}", path.display()))?.identity
                }
                None => enroller.identity(&photo.view(), &faces[faces.len() - 1].kps)?,
            };
            let swap = LiveEngine::new(&LiveConfig {
                packs: vec![args.packs.clone()],
                cache_dir: args.cache_dir.clone(),
                detector: gc_live::DetectorKind::Large,
                detector_size: 640,
                swap_model: args.model,
                cpu_only: false,
            })?;
            swap.set_settings(LiveSettings { occlusion: !args.no_occlusion, ..LiveSettings::default() });
            swap.set_identity(Some(identity));
            // The first runs of a model are slow. Get them over with before anything watches.
            let warm_started = Instant::now();
            clip.render(&clip.pose(args.motion, 0.0), &mut frame);
            let mut clock_ns = 1u64;
            while swap.patch().is_none() && warm_started.elapsed() < Duration::from_secs(120) {
                let view = ImageView::new(&frame, CLIP.0, CLIP.1, stride, PixelFormat::Bgra).context("frame")?;
                swap.push_frame(&view, clock_ns);
                clock_ns += 33_333_333;
                std::thread::sleep(Duration::from_millis(33));
            }
            if swap.patch().is_none() {
                bail!("no swapped patch after two minutes: {:?}", swap.stats().last_error);
            }
            Some(Arc::new(swap))
        }
    };
    drop(enroller);

    let engine = CameraEngine::with_clock(Some(Box::new(CameraOutput::new(Arc::clone(&feed)))), CameraOutput::clock);
    engine.set_swap(swap.clone().map(|swap| swap as Arc<dyn SwapStage>));
    match &camera {
        Some(_) => eprintln!("The camera \"God's Camera\" is in every app's camera list for as long as this runs."),
        None => eprintln!("The pipe is open. No camera was asked of Windows."),
    }

    let started = Instant::now();
    let mut watching: Option<(Wanted, Instant)> = None;
    let (mut made, mut with_face, mut from_loop, mut watched_s) = (0u64, 0u64, 0u64, 0f32);
    let (mut clip_ms, mut hand_over_ms) = (Vec::new(), Vec::new());
    let mut sizes: Vec<String> = Vec::new();
    let mut due = Instant::now();
    let swap_before = swap.as_ref().map(|swap| swap.stats());
    let mut demo = LoopDemo::Off;
    let mut loop_report = serde_json::Value::Null;

    while !args.stop.load(Ordering::Relaxed) && args.seconds.is_none_or(|limit| started.elapsed().as_secs_f32() < limit) {
        let wanted = feed.watched();
        if wanted != watching.map(|(wanted, _)| wanted) {
            if let Some((_, since)) = watching {
                watched_s += since.elapsed().as_secs_f32();
            }
            match (wanted, watching) {
                (Some(wanted), was) => {
                    eprintln!("Something is watching: it asked for {} x {}.", wanted.width, wanted.height);
                    let size = format!("{}x{}", wanted.width, wanted.height);
                    if !sizes.contains(&size) {
                        sizes.push(size);
                    }
                    if was.is_none() {
                        engine.source_started();
                        due = Instant::now();
                        if args.loop_after.is_some() {
                            demo = LoopDemo::Waiting { since: Instant::now() };
                        }
                    }
                }
                (None, _) => {
                    eprintln!("Nothing is watching.");
                    engine.stop_loop();
                    engine.source_stopped();
                    demo = LoopDemo::Off;
                    if args.once {
                        watching = None;
                        break;
                    }
                }
            }
            watching = wanted.map(|wanted| (wanted, Instant::now()));
        }
        let Some((wanted, _)) = watching else {
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

        // This is the camera: a frame is taken and handed to the engine.
        let stage = Instant::now();
        let taken = CameraOutput::clock();
        clip.render(&clip.pose(args.motion, started.elapsed().as_secs_f32()), &mut frame);
        clip_ms.push(stage.elapsed().as_secs_f32() * 1000.0);
        let view = ImageView::new(&frame, CLIP.0, CLIP.1, stride, PixelFormat::Bgra).context("frame")?;
        let stage = Instant::now();
        engine.push_frame(&view, taken);
        hand_over_ms.push(stage.elapsed().as_secs_f32() * 1000.0);
        made += 1;
        let status = engine.status();
        with_face += status.swapping as u64;
        from_loop += (status.mode == Mode::Looping) as u64;

        // `--loop-after`: live for a while, then record a loop and play it.
        demo = match demo {
            LoopDemo::Waiting { since } if since.elapsed().as_secs_f32() >= args.loop_after.unwrap_or(f32::MAX) => {
                eprintln!("Recording a loop of {LOOP_SECONDS} seconds.");
                engine.start_recording();
                LoopDemo::Recording { since: Instant::now() }
            }
            LoopDemo::Recording { since } if since.elapsed().as_secs_f32() >= LOOP_SECONDS => {
                engine.stop_recording();
                LoopDemo::AwaitingClip
            }
            LoopDemo::AwaitingClip => match engine.take_clips().pop() {
                Some(clip) => {
                    // The way of repeating that hides the seam best, as the app chooses it.
                    let mode = clip.suggested_mode();
                    loop_report = serde_json::json!({
                        "frames": clip.frame_count(),
                        "frame_rate": clip.frame_rate,
                        "megabytes": clip.stored_bytes() as f64 / 1e6,
                        "loop": [clip.points.start, clip.points.end],
                        "seam_mismatch": clip.seam_mismatch(),
                        "mode": format!("{mode:?}"),
                    });
                    eprintln!("Playing the loop: {} frames, {mode:?}.", clip.points.length());
                    let mut clip = Arc::try_unwrap(clip).map_err(|_| anyhow::anyhow!("the clip is in use"))?;
                    clip.mode = mode;
                    engine.start_loop(Arc::new(clip));
                    LoopDemo::Playing
                }
                None => LoopDemo::AwaitingClip,
            },
            other => other,
        };
    }
    if let Some((_, since)) = watching {
        watched_s += since.elapsed().as_secs_f32();
    }

    // Give the last picture a moment to leave before the pipe closes.
    engine.sync();
    std::thread::sleep(Duration::from_millis(50));
    let status = engine.status();
    let stats = feed.stats();
    let swap_report = match (&swap, swap_before) {
        (Some(swap), Some(before)) => {
            let after = swap.stats();
            serde_json::json!({
                "model": format!("{:?}", args.model),
                "occlusion_mask": after.occlusion,
                "swaps_per_second": (after.swaps - before.swaps) as f32 / watched_s.max(0.001),
                "swap_ms": after.swap_ms,
                "swap_age_ms": after.swap_age_ms,
                "detector_ms": after.detect_ms,
                "frame_thread_ms": after.frame_ms,
                "last_error": after.last_error,
            })
        }
        _ => serde_json::Value::Null,
    };
    drop(engine);
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
        "frames_with_the_swapped_face": with_face,
        "frame_periods_in_the_loop": from_loop,
        "frames_out": status.frames_out,
        "frames_passed_over": status.frames_skipped,
        "frames_sent": stats.sent,
        "frames_dropped": stats.dropped,
        "connects": stats.connects,
        "write_failures": stats.write_failures,
        "ended_in": match status.mode { Mode::Idle => "idle", Mode::Live => "live", Mode::Looping => "loop" },
        "clip_ms": median_and_tail(&mut clip_ms),
        "hand_over_ms": median_and_tail(&mut hand_over_ms),
        "draw_ms": status.render_ms,
        "swap": swap_report,
        "loop": loop_report,
    });
    println!("{}", serde_json::to_string(&report)?);
    if let Some(path) = &args.json {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    Ok(())
}
