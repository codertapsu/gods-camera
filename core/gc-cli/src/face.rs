//! `gc swap` and `gc replay`: the face pipeline on stills and on a moving-face clip.

use anyhow::{bail, Context, Result};
use gc_live::still::{StillSwapper, Targets};
use gc_live::synthetic::{FaceClip, Motion};
use gc_live::{
    DetectorKind, Enroller, ImageMut, ImageView, LiveConfig, LiveEngine, LiveSettings, MaskShape, PixelFormat,
    SwapModel,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct Picture {
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Picture {
    pub fn load(path: &Path) -> Result<Self> {
        let picture = image::open(path).with_context(|| format!("cannot read {}", path.display()))?.to_rgb8();
        let (width, height) = picture.dimensions();
        Ok(Self { rgb: picture.into_raw(), width, height })
    }

    pub fn view(&self) -> ImageView<'_> {
        ImageView::new(&self.rgb, self.width, self.height, self.width as usize * 3, PixelFormat::Rgb)
            .expect("a loaded picture is consistent")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        image::save_buffer(path, &self.rgb, self.width, self.height, image::ColorType::Rgb8)
            .with_context(|| format!("cannot write {}", path.display()))
    }
}

pub fn parse_model(name: &str) -> Result<SwapModel> {
    match name {
        "inswapper" => Ok(SwapModel::Inswapper128),
        "hyperswap" => Ok(SwapModel::Hyperswap256),
        other => bail!("unknown swap model '{other}'; use inswapper or hyperswap"),
    }
}

pub fn parse_mask(name: &str) -> Result<MaskShape> {
    match name {
        "oval" => Ok(MaskShape::Oval),
        "box" => Ok(MaskShape::Box),
        other => bail!("unknown mask '{other}'; use oval or box"),
    }
}

pub struct SwapArgs {
    pub packs: PathBuf,
    pub source: PathBuf,
    pub target: PathBuf,
    pub out: PathBuf,
    pub model: SwapModel,
    pub mask: MaskShape,
    pub all: bool,
    pub colour_match: bool,
}

/// Put the face from one picture onto the faces in another.
pub fn swap(args: SwapArgs) -> Result<()> {
    let source = Picture::load(&args.source)?;
    let mut target = Picture::load(&args.target)?;
    let mut swapper = StillSwapper::new(std::slice::from_ref(&args.packs), args.model, gc_infer_cpu())?;
    let enrolled = swapper
        .enroller()
        .enrol(&source.view())?
        .with_context(|| format!("no face found in {}", args.source.display()))?;
    let started = Instant::now();
    let mut picture = ImageMut {
        data: &mut target.rgb,
        width: target.width,
        height: target.height,
        stride: target.width as usize * 3,
        format: PixelFormat::Rgb,
    };
    let targets = if args.all { Targets::All } else { Targets::Largest };
    let count = swapper.swap(&mut picture, &enrolled.identity, targets, args.mask, args.colour_match)?;
    target.save(&args.out)?;
    println!(
        "source: {} face(s), used the largest. target: swapped {count} face(s) in {:.0} ms. wrote {}",
        enrolled.faces,
        started.elapsed().as_secs_f64() * 1000.0,
        args.out.display()
    );
    Ok(())
}

fn gc_infer_cpu() -> gc_live::Accelerator {
    gc_live::Accelerator::Cpu
}

pub struct ReplayArgs {
    pub packs: PathBuf,
    /// A photo with at least one face. The leftmost face moves in the clip.
    pub photo: PathBuf,
    /// The face to swap in. Defaults to the rightmost face in `photo`.
    pub source: Option<PathBuf>,
    pub model: SwapModel,
    pub detector: DetectorKind,
    pub detector_size: u32,
    pub motion: Motion,
    pub seconds: f32,
    pub width: u32,
    pub height: u32,
    /// Feed frames as fast as they can be taken, instead of 30 a second.
    pub unpaced: bool,
    pub cpu_only: bool,
    pub cache_dir: Option<PathBuf>,
    /// Folder for every fifteenth output frame, as PNG.
    pub dump: Option<PathBuf>,
    pub json: Option<PathBuf>,
}

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f32 * fraction).round() as usize]
}

/// Run the live engine over a moving-face clip and report how it kept up.
pub fn replay(args: ReplayArgs) -> Result<()> {
    const FPS: f32 = 30.0;
    let photo = Picture::load(&args.photo)?;
    let mut enroller = Enroller::new(std::slice::from_ref(&args.packs), args.cache_dir.as_deref())?;
    let mut faces = enroller.detect(&photo.view())?;
    faces.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
    let actor = faces.first().cloned().with_context(|| format!("no face found in {}", args.photo.display()))?;
    let identity = match &args.source {
        Some(path) => {
            let source = Picture::load(path)?;
            enroller.enrol(&source.view())?.with_context(|| format!("no face found in {}", path.display()))?.identity
        }
        None => enroller.identity(&photo.view(), &faces[faces.len() - 1].kps)?,
    };

    let load_started = Instant::now();
    let engine = LiveEngine::new(&LiveConfig {
        packs: vec![args.packs.clone()],
        cache_dir: args.cache_dir.clone(),
        detector: args.detector,
        detector_size: args.detector_size,
        swap_model: args.model,
        cpu_only: args.cpu_only,
    })?;
    engine.set_settings(LiveSettings::default());
    engine.set_identity(Some(identity));
    let load_s = load_started.elapsed().as_secs_f32();

    // A face about as large as in a video call: eyes a tenth of the frame's width apart.
    let eye_distance = args.width as f32 * 0.1;
    let clip = FaceClip::new(&photo.view(), &actor, args.width, args.height, eye_distance);
    let zoom = eye_distance
        / ((actor.kps[1][0] - actor.kps[0][0]).powi(2) + (actor.kps[1][1] - actor.kps[0][1]).powi(2)).sqrt();
    let stride = args.width as usize * 4;
    let mut frame = Vec::new();

    // Warm-up: the first runs of a model are slow, and are not what a call feels like.
    let warm_started = Instant::now();
    let pose = clip.pose(args.motion, 0.0);
    clip.render(&pose, &mut frame);
    let mut clock_ns = 1u64;
    while engine.patch().is_none() && warm_started.elapsed() < Duration::from_secs(120) {
        let view = ImageView::new(&frame, args.width, args.height, stride, PixelFormat::Bgra).context("frame")?;
        engine.push_frame(&view, clock_ns);
        clock_ns += (1e9 / FPS) as u64;
        std::thread::sleep(Duration::from_millis(33));
    }
    if engine.patch().is_none() {
        bail!("no swapped patch after two minutes: {:?}", engine.stats().last_error);
    }
    let warm_s = warm_started.elapsed().as_secs_f32();
    let before = engine.stats();

    if let Some(folder) = &args.dump {
        std::fs::create_dir_all(folder)?;
    }
    let frames = (args.seconds * FPS) as usize;
    let mut frame_ms = Vec::with_capacity(frames);
    let mut patch_ages = Vec::with_capacity(frames);
    let mut on_photo: Vec<[[f32; 2]; 5]> = Vec::with_capacity(frames);
    let (mut followed, mut visible, mut largest_step) = (0usize, 0usize, 0f32);
    let run_started = Instant::now();
    for i in 0..frames {
        let seconds = i as f32 / FPS;
        let pose = clip.pose(args.motion, seconds);
        clip.render(&pose, &mut frame);
        let view = ImageView::new(&frame, args.width, args.height, stride, PixelFormat::Bgra).context("frame")?;
        let pushed = Instant::now();
        let result = engine.push_frame(&view, clock_ns + (seconds * 1e9) as u64);
        frame_ms.push(pushed.elapsed().as_secs_f32() * 1000.0);
        if result.face {
            followed += 1;
            let back = pose.invert().context("pose")?;
            let now = result.keypoints.map(|p| back.apply(p));
            if let Some(previous) = on_photo.last() {
                let step = now
                    .iter()
                    .zip(previous)
                    .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
                    .fold(0.0, f32::max);
                largest_step = largest_step.max(step * zoom);
            }
            on_photo.push(now);
        }
        if result.patch_sequence > 0 && result.opacity > 0.9 {
            visible += 1;
            patch_ages.push(result.patch_age_frames as f32);
        }
        if let (Some(folder), Some(patch), true) = (&args.dump, engine.patch(), i % 15 == 0) {
            let mut out = frame.clone();
            let mut picture = ImageMut {
                data: &mut out,
                width: args.width,
                height: args.height,
                stride,
                format: PixelFormat::Bgra,
            };
            gc_live::paste_patch(&mut picture, &patch.rgba, patch.size, &result.crop_to_frame, result.opacity);
            let rgb: Vec<u8> = out.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0]]).collect();
            image::save_buffer(folder.join(format!("frame-{i:04}.png")), &rgb, args.width, args.height, image::ColorType::Rgb8)?;
        }
        if !args.unpaced {
            let due = run_started + Duration::from_secs_f32((i + 1) as f32 / FPS);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    }
    let run_s = run_started.elapsed().as_secs_f32();
    let stats = engine.stats();

    // How much the keypoints move about on a face that, seen from the photo, stands still.
    let count = on_photo.len().max(1) as f32;
    let mut wobble = 0.0;
    for k in 0..5 {
        let mean = on_photo.iter().fold([0.0; 2], |m, f| [m[0] + f[k][0] / count, m[1] + f[k][1] / count]);
        wobble +=
            on_photo.iter().map(|f| (f[k][0] - mean[0]).powi(2) + (f[k][1] - mean[1]).powi(2)).sum::<f32>() / count / 5.0;
    }
    let wobble = wobble.sqrt() * zoom;
    frame_ms.sort_by(f32::total_cmp);
    patch_ages.sort_by(f32::total_cmp);
    let swaps = stats.swaps - before.swaps;
    let detections = stats.detections - before.detections;

    let report = serde_json::json!({
        "model": format!("{:?}", args.model),
        "detector": format!("{:?} at {}", args.detector, args.detector_size),
        "motion": format!("{:?}", args.motion),
        "frame": format!("{}x{}", args.width, args.height),
        "paced": !args.unpaced,
        "cpu_only": args.cpu_only,
        "load_s": load_s,
        "warm_up_s": warm_s,
        "frames": frames,
        "run_s": run_s,
        "frames_per_second": frames as f32 / run_s,
        "frame_thread_ms": { "median": percentile(&frame_ms, 0.5), "p95": percentile(&frame_ms, 0.95), "max": percentile(&frame_ms, 1.0) },
        "detector_runs_per_second": detections as f32 / run_s,
        "detector_ms": stats.detect_ms,
        "swaps_per_second": swaps as f32 / run_s,
        "swap_ms": stats.swap_ms,
        "swap_age_ms": stats.swap_age_ms,
        "patch_age_frames": { "median": percentile(&patch_ages, 0.5), "p95": percentile(&patch_ages, 0.95) },
        "face_followed_percent": 100.0 * followed as f32 / frames.max(1) as f32,
        "swap_visible_percent": 100.0 * visible as f32 / frames.max(1) as f32,
        "eye_distance_px": eye_distance,
        "keypoint_wobble_px": wobble,
        "keypoint_wobble_percent_of_eye_distance": 100.0 * wobble / eye_distance,
        "largest_keypoint_step_px": largest_step,
        "last_error": stats.last_error,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    if let Some(path) = &args.json {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    Ok(())
}
