//! The live face-swap engine.
//!
//! Three threads share the work so that the picture never waits for a model:
//!
//! - the caller's thread, once per camera frame: follow the face with the
//!   tracker, smooth it, and say where the newest swapped patch belongs on
//!   this frame;
//! - a detector thread, a few times a second: find the face again, which
//!   stops the tracker from drifting;
//! - a swap thread, as fast as the model allows: turn the newest aligned crop
//!   into a swapped patch.
//!
//! A patch is always drawn with the transform of the newest frame, not the
//! frame it was cut from. A slow model therefore lowers how often the face
//! texture changes, and never how well it follows the head.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use gc_align::{Affine, Keypoints};
use gc_infer::image::{self, ImageView, MaskShape, PixelFormat};
use gc_infer::runtime::{Accelerator, Detector, IdentityEncoder, InferError, Occluder, Runtime, Swapper};
use gc_infer::{Detection, SwapperContract};
use gc_track::{DetectScheduler, FlowConfig, FlowTracker, FrameSignals, Gray, KeypointSmoother, TrackOutcome};

use crate::slot::Slot;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic on another thread must not take the camera down with it.
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorKind {
    /// `det_10g`: the accurate detector.
    Large,
    /// `det_500m`: the small detector, fast enough for the CPU.
    Small,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapModel {
    Inswapper128,
    Hyperswap256,
}

/// What the engine is built with. Changing any of it means building a new engine.
#[derive(Debug, Clone)]
pub struct LiveConfig {
    /// Folders that hold the model files, searched in order: the ones that
    /// come with the app first, then the ones added later.
    pub packs: Vec<PathBuf>,
    /// Folder for compiled models, so later starts are fast.
    pub cache_dir: Option<PathBuf>,
    pub detector: DetectorKind,
    /// Side of the detector's input square: 640 or 320.
    pub detector_size: u32,
    pub swap_model: SwapModel,
    /// Run every model on the CPU. Slow; for tests and machines without acceleration.
    pub cpu_only: bool,
}

/// What can change while the engine runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveSettings {
    pub mask: MaskShape,
    /// Shift the swapped face's colours towards the real face's.
    pub colour_match: bool,
    /// 0 shows the real face, 1 the swapped one.
    pub opacity: f32,
    /// Detections below this score are ignored.
    pub score_threshold: f32,
    /// How much crisper to make the swapped face, 0 to 1. Swap models return a
    /// small picture, which looks soft once it is enlarged onto the frame.
    pub sharpen: f32,
    /// Keep whatever is in front of the face, such as a hand or a cup, in
    /// front of the swap. Needs the occlusion model; without it nothing changes.
    pub occlusion: bool,
    /// Leave the real mouth showing through the swap.
    pub real_mouth: bool,
    /// Ask for the picture to be blurred whenever the swap is not fully on the
    /// face, so the real face never shows. See `FrameResult::blur`.
    pub blur_when_unswapped: bool,
}

impl Default for LiveSettings {
    fn default() -> Self {
        Self {
            mask: MaskShape::Oval,
            colour_match: true,
            opacity: 1.0,
            score_threshold: 0.5,
            sharpen: 0.4,
            occlusion: true,
            real_mouth: false,
            blur_when_unswapped: false,
        }
    }
}

/// Where each model file is and what runs it.
struct ModelPlan {
    detector: (PathBuf, Accelerator),
    swapper: (PathBuf, SwapperContract, Option<PathBuf>, Accelerator),
    /// The occlusion model, when its file is there. The swap works without it.
    occluder: Option<(PathBuf, Accelerator)>,
}

/// The first folder that has `file`. If none has it, the path in the first
/// folder is returned, so the error names a place to put the file.
pub(crate) fn find_model(folders: &[PathBuf], file: &str) -> PathBuf {
    folders
        .iter()
        .map(|folder| folder.join(file))
        .find(|path| path.is_file())
        .unwrap_or_else(|| folders.first().map(|folder| folder.join(file)).unwrap_or_else(|| PathBuf::from(file)))
}

impl ModelPlan {
    /// The choices measured in spike S3 on Apple silicon: the swap model gets
    /// the unit it is fastest on, and the detector stays off that unit.
    fn new(config: &LiveConfig) -> Self {
        let find = |file: &str| find_model(&config.packs, file);
        let on = |accelerator| if config.cpu_only { Accelerator::Cpu } else { accelerator };
        let swapper = match config.swap_model {
            SwapModel::Inswapper128 if cfg!(windows) => {
                // Measured on the RTX 3050 through DirectML: the file as
                // downloaded takes 58 ms. A wholly half-precision copy takes
                // 18 ms and gives a wrong picture, 14 levels off on average.
                // The mixed copy keeps the normalisation arithmetic in full
                // precision: 22 ms, and 0.3 levels off.
                let emap = Some(find("inswapper_128.emap.bin"));
                let mixed = find("inswapper_128.mixed.onnx");
                let file = if !config.cpu_only && mixed.is_file() { mixed } else { find("inswapper_128.onnx") };
                (file, SwapperContract::INSWAPPER_128, emap, on(Accelerator::Gpu))
            }
            SwapModel::Inswapper128 => {
                let emap = Some(find("inswapper_128.emap.bin"));
                let fast = find("inswapper_128.reflectfree.fp16.onnx");
                let full = find("inswapper_128.onnx");
                // The half-precision file runs on the Neural Engine. The CPU needs the full one.
                if fast.is_file() && (!config.cpu_only || !full.is_file()) {
                    (fast, SwapperContract::INSWAPPER_128, emap, on(Accelerator::NeuralEngine))
                } else {
                    (full, SwapperContract::INSWAPPER_128, emap, on(Accelerator::Gpu))
                }
            }
            SwapModel::Hyperswap256 => {
                // The file as downloaded stores half precision. Through DirectML
                // its normalisation then comes out wrong, 7 levels off on
                // average, so on a graphics card the mixed copy is used: 21 ms,
                // 0.05 levels off. If that copy is missing the load fails and
                // names it, which is better than a wrong face.
                let on_a_card = cfg!(windows) && !config.cpu_only;
                let file = if on_a_card { "hyperswap_1b_256.mixed.onnx" } else { "hyperswap_1b_256.onnx" };
                (find(file), SwapperContract::HYPERSWAP_256, None, on(Accelerator::Gpu))
            }
        };
        // On the Mac both detectors run on the CPU. Measured on the M4 Pro at
        // 640 pixels: about 11 ms for the large one and 4 ms for the small one,
        // which is faster than Core ML ran them, and it leaves the GPU and the
        // Neural Engine to the swap model.
        //
        // On Windows the large detector goes to the graphics card. Measured on
        // the Core i5-13400F and RTX 3050: 48 ms on the processor, and far more
        // while the other models run, against 10 ms on the card. The small one
        // takes 1 ms on the processor, and DirectML cannot run it at all.
        let large_detector_on = if cfg!(windows) { on(Accelerator::Gpu) } else { Accelerator::Cpu };
        let detector = match config.detector {
            DetectorKind::Small => (find("buffalo_s/det_500m.onnx"), Accelerator::Cpu),
            DetectorKind::Large => (find("buffalo_l/det_10g.onnx"), large_detector_on),
        };
        // The occlusion model runs beside the swap model, so it gets the unit
        // the swap model leaves free. Measured on the M4 Pro for one mask:
        // 15 ms on the GPU, 30 ms on the Neural Engine, 36 ms on the CPU.
        let beside = if swapper.3 == Accelerator::Gpu { Accelerator::NeuralEngine } else { Accelerator::Gpu };
        let occluder = Some(find("xseg_3.onnx")).filter(|path| path.is_file()).map(|path| (path, on(beside)));
        Self { detector, swapper, occluder }
    }
}

/// A swapped face, ready to be drawn.
#[derive(Debug)]
pub struct Patch {
    /// Goes up by one with every new patch.
    pub sequence: u64,
    /// Side of the square, in pixels.
    pub size: u32,
    /// Red, green, blue and the mask as alpha; not premultiplied.
    pub rgba: Vec<u8>,
    /// The frame the crop was cut from.
    pub frame_index: u64,
    pub frame_time_ns: u64,
}

/// What the caller needs to draw one output frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameResult {
    /// A face is being followed or briefly held.
    pub face: bool,
    /// Maps patch pixels onto this frame.
    pub crop_to_frame: Affine,
    /// How strongly to draw the patch, 0 to 1, with every fade applied.
    pub opacity: f32,
    /// The patch to draw, or 0 for none. Fetch it with `LiveEngine::patch`.
    pub patch_sequence: u64,
    /// How many frames old the patch's picture is.
    pub patch_age_frames: u32,
    /// The face's five keypoints on this frame.
    pub keypoints: Keypoints,
    /// How strongly to blur the whole picture, 0 to 1. Above 0 only with
    /// `LiveSettings::blur_when_unswapped`, while a face is set and the swap
    /// is not fully on it: no face found, the head turned to profile, the
    /// face lost, or the first swapped picture still on its way.
    pub blur: f32,
}

impl FrameResult {
    const NONE: Self = Self {
        face: false,
        crop_to_frame: Affine::IDENTITY,
        opacity: 0.0,
        patch_sequence: 0,
        patch_age_frames: 0,
        keypoints: [[0.0; 2]; 5],
        blur: 0.0,
    };
}

#[derive(Debug, Clone, Default)]
pub struct LiveStats {
    pub frames: u64,
    pub detections: u64,
    pub swaps: u64,
    /// Smoothed times, in milliseconds.
    pub frame_ms: f32,
    pub detect_ms: f32,
    pub swap_ms: f32,
    /// From a frame's arrival to its swapped patch being ready.
    pub swap_age_ms: f32,
    pub tracking: bool,
    /// The occlusion model is loaded, so `LiveSettings::occlusion` has an effect.
    pub occlusion: bool,
    pub last_error: Option<String>,
}

fn smooth(old: f32, new: f32) -> f32 {
    if old == 0.0 {
        new
    } else {
        old * 0.9 + new * 0.1
    }
}

struct DetectJob {
    frame_index: u64,
    buffer: Vec<u8>,
    width: u32,
    height: u32,
    stride: usize,
    format: PixelFormat,
    threshold: f32,
}

struct DetectResult {
    frame_index: u64,
    faces: Vec<Detection>,
    failed: bool,
    /// The frame copy, handed back for the next job.
    buffer: Vec<u8>,
}

struct SwapJob {
    crop: Vec<u8>,
    frame_index: u64,
    frame_time_ns: u64,
    arrived: Instant,
}

#[derive(Default)]
struct Shared {
    detect_jobs: Slot<DetectJob>,
    detect_results: Slot<DetectResult>,
    swap_jobs: Slot<SwapJob>,
    /// The newest aligned crop for the occlusion model, and when its frame arrived.
    occlusion_jobs: Slot<(Vec<u8>, Instant)>,
    /// The newest occlusion mask, and when the frame it was made from arrived.
    uncovered: Mutex<Option<(Arc<Vec<u8>>, Instant)>>,
    patch: Mutex<Option<Arc<Patch>>>,
    /// The source face's identity vector, as the encoder returned it.
    identity: Mutex<Option<Arc<Vec<f32>>>>,
    settings: Mutex<Option<LiveSettings>>,
    stats: Mutex<LiveStats>,
}

impl Shared {
    fn settings(&self) -> LiveSettings {
        lock(&self.settings).unwrap_or_default()
    }

    fn fail(&self, error: &InferError) {
        lock(&self.stats).last_error = Some(error.to_string());
    }
}

/// Everything the frame thread keeps between frames.
struct FrameState {
    tracker: FlowTracker,
    smoother: KeypointSmoother,
    scheduler: DetectScheduler,
    gray: Vec<u8>,
    gray_size: (usize, usize),
    frame_index: u64,
    last_time_ns: u64,
    /// How each recent frame moved relative to the one before, in tracker pixels.
    motions: VecDeque<(u64, Affine)>,
    detect_in_flight: bool,
    /// Time passed since the scheduler was last asked.
    unscheduled_s: f32,
    spare_frame: Vec<u8>,
    detect_gap_px: f32,
    speed_px: f32,
    empty_detections: u32,
    settings_changed: bool,
    /// The face was taken up from a detection that could not be carried
    /// forward to the frame it was applied on: the first sight of a face, or
    /// its return. The place may be off by however far the face moved while
    /// the detector worked, so the next detection is taken whole.
    unconfirmed: bool,
    /// The face's keypoints on the newest frame, smoothed, in tracker pixels.
    smoothed: Option<Keypoints>,
    /// Seconds since the face was last followed.
    unseen_s: f32,
    /// 0 to 1: rises while a face and a patch are present, falls when not.
    presence: f32,
    /// 0 to 1: rises only while the face is being followed and a patch is on
    /// it. Unlike `presence` it starts to fall the moment the face is lost,
    /// not after the hold, so the blur covers the picture before the patch goes.
    certainty: f32,
}

/// Seconds a lost face is still drawn at its last place before it fades.
const HOLD_S: f32 = 0.25;
/// Seconds a fade in or out takes.
const FADE_S: f32 = 0.15;
/// Keypoint smoothing, in tracker pixels. The tracker is already steady, so the
/// filter only softens the small corrections from the detector: a low cut-off
/// at rest that opens up quickly with speed, so a moving face is not followed late.
const SMOOTH_MIN_CUTOFF_HZ: f32 = 1.5;
const SMOOTH_SPEED_GAIN: f32 = 1.0;
const SMOOTH_DEADBAND_PX: f32 = 0.15;
/// How far a detection pulls the tracked keypoints towards itself when the two
/// nearly agree, 0 to 1.
const DETECT_PULL: f32 = 0.1;
/// Disagreement up to this share of the eye distance is treated as detector noise.
const NOISE_FRACTION: f32 = 0.05;
/// Disagreement from this share of the eye distance replaces the tracked keypoints outright.
const SNAP_FRACTION: f32 = 0.16;
/// Frames of motion history kept for moving a detection to the newest frame.
const MOTION_HISTORY: usize = 90;
/// Faces whose eyes are closer than this, in frame pixels, are too small to swap.
const MIN_EYE_DISTANCE: f32 = 14.0;
/// An occlusion mask is used on a patch only if its picture is at most this
/// much older than the patch's. An older one would cut holes where a hand used to be.
const MASK_FRESH: std::time::Duration = std::time::Duration::from_millis(250);

pub struct LiveEngine {
    shared: Arc<Shared>,
    frame: Mutex<FrameState>,
    crop_size: u32,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl LiveEngine {
    /// Load the models and start the worker threads. This can take several
    /// seconds the first time, while the system compiles the models.
    pub fn new(config: &LiveConfig) -> Result<Self, InferError> {
        let runtime = Runtime::shared(config.cache_dir.as_deref())?;
        let plan = ModelPlan::new(config);
        let detector = Detector::load(runtime, &plan.detector.0, config.detector_size, plan.detector.1)?;
        let (model, contract, emap, accelerator) = plan.swapper;
        let swapper = Swapper::load(runtime, &model, contract, emap.as_deref(), accelerator)?;
        let crop_size = contract.crop_size;

        let shared = Arc::new(Shared::default());
        // The swap is still worth having if the occlusion model cannot be used.
        let occluder = plan.occluder.and_then(|(model, accelerator)| {
            Occluder::load(runtime, &model, accelerator).map_err(|error| shared.fail(&error)).ok()
        });
        lock(&shared.stats).occlusion = occluder.is_some();
        let mut workers = vec![
            {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name("gc-detect".into())
                    .spawn(move || detect_worker(&shared, detector))
                    .map_err(|e| InferError::Runtime(e.to_string()))?
            },
            {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name("gc-swap".into())
                    .spawn(move || swap_worker(&shared, swapper))
                    .map_err(|e| InferError::Runtime(e.to_string()))?
            },
        ];
        if let Some(occluder) = occluder {
            let shared = shared.clone();
            workers.push(
                std::thread::Builder::new()
                    .name("gc-occlusion".into())
                    .spawn(move || occlusion_worker(&shared, occluder, crop_size))
                    .map_err(|e| InferError::Runtime(e.to_string()))?,
            );
        }
        let frame = FrameState {
            tracker: FlowTracker::new(FlowConfig::default()),
            smoother: KeypointSmoother::new(
                SMOOTH_MIN_CUTOFF_HZ, SMOOTH_SPEED_GAIN, SMOOTH_DEADBAND_PX,
            ),
            scheduler: DetectScheduler::default(),
            gray: Vec::new(),
            gray_size: (0, 0),
            frame_index: 0,
            last_time_ns: 0,
            motions: VecDeque::with_capacity(MOTION_HISTORY),
            detect_in_flight: false,
            unscheduled_s: 0.0,
            spare_frame: Vec::new(),
            detect_gap_px: 0.0,
            speed_px: 0.0,
            empty_detections: 0,
            settings_changed: false,
            unconfirmed: false,
            smoothed: None,
            unseen_s: f32::INFINITY,
            presence: 0.0,
            certainty: 0.0,
        };
        Ok(Self { shared, frame: Mutex::new(frame), crop_size, workers: Mutex::new(workers) })
    }

    /// Side of the swapped patch, in pixels.
    pub fn crop_size(&self) -> u32 {
        self.crop_size
    }

    /// Set the face to swap in, as an identity vector from `Enroller`, or
    /// `None` to stop swapping.
    pub fn set_identity(&self, identity: Option<Vec<f32>>) {
        *lock(&self.shared.identity) = identity.map(Arc::new);
        // The old face's patch must not be shown as the new face.
        *lock(&self.shared.patch) = None;
    }

    pub fn set_settings(&self, settings: LiveSettings) {
        let changed = self.shared.settings() != settings;
        *lock(&self.shared.settings) = Some(settings);
        if changed {
            lock(&self.frame).settings_changed = true;
        }
    }

    pub fn stats(&self) -> LiveStats {
        lock(&self.shared.stats).clone()
    }

    /// The newest swapped patch, if there is one.
    pub fn patch(&self) -> Option<Arc<Patch>> {
        lock(&self.shared.patch).clone()
    }

    /// Take in one camera frame and say how to draw the output for it.
    ///
    /// `time_ns` is when the frame was captured, on any steady clock.
    pub fn push_frame(&self, frame: &ImageView<'_>, time_ns: u64) -> FrameResult {
        let started = Instant::now();
        let settings = self.shared.settings();
        let has_identity = lock(&self.shared.identity).is_some();
        let mut guard = lock(&self.frame);
        let st = &mut *guard;

        let dt = if st.last_time_ns == 0 || time_ns <= st.last_time_ns {
            1.0 / 30.0
        } else {
            ((time_ns - st.last_time_ns) as f32 / 1e9).min(0.5)
        };
        st.last_time_ns = time_ns;
        st.frame_index += 1;
        let index = st.frame_index;

        // The tracker works on a half-size brightness copy of the frame. The
        // buffer leaves the state for the length of this call and returns at the end.
        let mut gray_data = std::mem::take(&mut st.gray);
        let size = image::half_luma(frame, &mut gray_data);
        if size != st.gray_size {
            st.tracker.reset();
            st.smoother.reset();
            st.motions.clear();
            st.smoothed = None;
            st.gray_size = size;
        }
        // With the blur option, the picture is hidden whenever a face is set and no swap is on it.
        let hide = settings.blur_when_unswapped && has_identity && settings.opacity > 0.0;
        if size.0 < 32 || size.1 < 32 {
            st.gray = gray_data;
            return FrameResult { blur: if hide { 1.0 } else { 0.0 }, ..FrameResult::NONE };
        }
        let gray = Gray { data: &gray_data, width: size.0, height: size.1, stride: size.0 };
        // Frame pixels per tracker pixel.
        let scale = [frame.width as f32 / size.0 as f32, frame.height as f32 / size.1 as f32];

        // 1. Follow the face from the previous frame to this one.
        let before = st.tracker.keypoints().copied();
        match st.tracker.track(&gray) {
            TrackOutcome::Tracked { motion, kps, .. } => {
                st.speed_px = before.map(|b| distance(b[2], kps[2])).unwrap_or(0.0);
                if st.motions.len() == MOTION_HISTORY {
                    st.motions.pop_front();
                }
                st.motions.push_back((index, motion));
            }
            TrackOutcome::Lost => {
                st.speed_px = 0.0;
                st.motions.clear();
            }
        }

        // 2. Take in a detection that finished since the last frame.
        if let Some(result) = self.shared.detect_results.try_take() {
            st.detect_in_flight = false;
            st.spare_frame = result.buffer;
            if !result.failed {
                apply_detection(st, &self.shared, &gray, scale, result.frame_index, &result.faces, index);
            }
        }

        // 3. Run the detector on this frame if it is due and free.
        st.unscheduled_s += dt;
        if !st.detect_in_flight {
            // The scheduler's thresholds are in pixels for a face whose eyes are
            // 60 pixels apart, so speeds and gaps are scaled to that size.
            let to_reference = st
                .tracker
                .keypoints()
                .map(|k| 60.0 / distance(k[0], k[1]).max(1.0))
                .unwrap_or(1.0);
            let signals = FrameSignals {
                dt: std::mem::take(&mut st.unscheduled_s),
                tracking: st.tracker.keypoints().is_some(),
                speed_px: st.speed_px * to_reference,
                flow_detect_gap_px: std::mem::take(&mut st.detect_gap_px) * to_reference,
                dropped_frames: 0,
                // An unconfirmed face is looked at again at once, like after a changed setting.
                settings_changed: std::mem::take(&mut st.settings_changed) || st.unconfirmed,
            };
            if st.scheduler.should_detect(&signals) {
                let mut buffer = std::mem::take(&mut st.spare_frame);
                buffer.clear();
                for y in 0..frame.height as usize {
                    buffer.extend_from_slice(frame.row(y));
                }
                self.shared.detect_jobs.put(DetectJob {
                    frame_index: index,
                    buffer,
                    width: frame.width,
                    height: frame.height,
                    stride: frame.width as usize * frame.format.bytes(),
                    format: frame.format,
                    threshold: settings.score_threshold,
                });
                st.detect_in_flight = true;
            }
        }

        // 4. Where the face is now, smoothed.
        let tracking = match st.tracker.keypoints() {
            Some(kps) => {
                st.smoothed = Some(st.smoother.update(kps, dt));
                st.unseen_s = 0.0;
                true
            }
            None => {
                st.unseen_s += dt;
                false
            }
        };

        // 5. Cut the newest aligned crop for the swap model, and place the newest patch.
        let mut result = FrameResult::NONE;
        let patch = lock(&self.shared.patch).clone();
        let mut visible = false;
        // A face whose place is not confirmed yet is followed but not shown:
        // for a frame or two it would be drawn where the face was, not where it is.
        if let Some(smoothed) = st.smoothed.filter(|_| st.unseen_s <= HOLD_S + FADE_S && !st.unconfirmed) {
            let mut kps = smoothed;
            for p in &mut kps {
                p[0] *= scale[0];
                p[1] *= scale[1];
            }
            let big_enough = distance(kps[0], kps[1]) >= MIN_EYE_DISTANCE;
            if let (true, Ok(frame_to_crop)) = (big_enough, gc_align::estimate_norm(&kps, self.crop_size)) {
                if tracking && has_identity && settings.opacity > 0.0 {
                    let crop = image::warp_to_crop(frame, &frame_to_crop, self.crop_size);
                    self.shared.swap_jobs.put(SwapJob { crop, frame_index: index, frame_time_ns: time_ns, arrived: started });
                }
                if let Some(crop_to_frame) = frame_to_crop.invert() {
                    visible = patch.is_some() && st.unseen_s <= HOLD_S;
                    result = FrameResult {
                        face: true,
                        crop_to_frame,
                        opacity: 0.0,
                        patch_sequence: patch.as_ref().map(|p| p.sequence).unwrap_or(0),
                        patch_age_frames: patch.as_ref().map(|p| index.saturating_sub(p.frame_index) as u32).unwrap_or(0),
                        keypoints: kps,
                        blur: 0.0,
                    };
                    result.opacity = turn_fade(&kps);
                }
            }
        } else {
            st.smoothed = None;
            st.smoother.reset();
        }

        // Fades: the patch eases in when it appears and out when the face goes.
        let step = (dt / FADE_S).min(1.0);
        st.presence = if visible { (st.presence + step).min(1.0) } else { (st.presence - step).max(0.0) };
        st.certainty = if visible && tracking { (st.certainty + step).min(1.0) } else { (st.certainty - step).max(0.0) };
        // How much of the face the swap covers, before the user's own opacity:
        // the turn towards profile, and how sure the place of the face is.
        let covered = if result.patch_sequence == 0 { 0.0 } else { result.opacity * st.certainty };
        result.opacity *= st.presence * settings.opacity.clamp(0.0, 1.0);
        if result.patch_sequence == 0 {
            result.opacity = 0.0;
        }
        // The blur leads: it is complete by the time the swap is half gone.
        result.blur = if hide { ((1.0 - covered) * 2.0).clamp(0.0, 1.0) } else { 0.0 };

        st.gray = gray_data;
        let mut stats = lock(&self.shared.stats);
        stats.frames += 1;
        stats.tracking = tracking;
        stats.frame_ms = smooth(stats.frame_ms, started.elapsed().as_secs_f32() * 1000.0);
        result
    }
}

impl Drop for LiveEngine {
    fn drop(&mut self) {
        self.shared.detect_jobs.close();
        self.shared.swap_jobs.close();
        self.shared.occlusion_jobs.close();
        for worker in lock(&self.workers).drain(..) {
            let _ = worker.join();
        }
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// 1 while the head faces the camera, falling to 0 as it turns to profile.
///
/// Judged from where the nose sits between the eyes. The swap models are
/// trained on faces that look towards the camera and fail on profiles, so the
/// real face shows through instead.
fn turn_fade(kps: &Keypoints) -> f32 {
    let (left, right, nose) = (kps[0], kps[1], kps[2]);
    let eye = [right[0] - left[0], right[1] - left[1]];
    let length_squared = eye[0] * eye[0] + eye[1] * eye[1];
    if length_squared < 1.0 {
        return 0.0;
    }
    // 0.5 when the nose is midway between the eyes; 0 or 1 when it is under one of them.
    let along = ((nose[0] - left[0]) * eye[0] + (nose[1] - left[1]) * eye[1]) / length_squared;
    let turn = (along - 0.5).abs() * 2.0;
    let t = ((turn - 0.75) / 0.25).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Use a finished detection: move it to the newest frame and re-anchor the tracker on it.
fn apply_detection(
    st: &mut FrameState,
    shared: &Shared,
    gray: &Gray<'_>,
    scale: [f32; 2],
    detected_on: u64,
    faces: &[Detection],
    now: u64,
) {
    if faces.is_empty() {
        st.empty_detections += 1;
        // One miss can be a blink of the detector; two in a row means the face has gone.
        if st.empty_detections >= 2 {
            st.tracker.reset();
            st.unconfirmed = false;
        }
        return;
    }
    st.empty_detections = 0;

    // The detector saw an older frame. The tracker measured how every frame
    // since then moved, so the detection can be carried forward to now.
    let mut since: Option<Affine> = Some(Affine::IDENTITY);
    for frame in detected_on + 1..=now {
        since = match (since, st.motions.iter().find(|(index, _)| *index == frame)) {
            (Some(total), Some((_, motion))) => Some(motion.compose(&total)),
            _ => None,
        };
    }
    let carried = |face: &Detection| -> Keypoints {
        let mut kps = face.kps;
        for p in &mut kps {
            *p = [p[0] / scale[0], p[1] / scale[1]];
            if let Some(motion) = &since {
                *p = motion.apply(*p);
            }
        }
        kps
    };

    let tracked = st.tracker.keypoints().copied();
    let area = |face: &Detection| (face.bbox[2] - face.bbox[0]) * (face.bbox[3] - face.bbox[1]);
    let largest = faces.iter().max_by(|a, b| area(a).total_cmp(&area(b)));
    // Stay with the face being followed if the detector still sees it; otherwise take the largest.
    let chosen = match tracked {
        Some(t) => {
            let nearest =
                faces.iter().min_by(|a, b| distance(carried(a)[2], t[2]).total_cmp(&distance(carried(b)[2], t[2])));
            nearest.filter(|face| !gc_track::face_jumped(&t, &carried(face))).or(largest)
        }
        None => largest,
    };
    let Some(chosen) = chosen else { return };
    let mut kps = carried(chosen);

    match tracked {
        Some(t) if !gc_track::face_jumped(&t, &kps) => {
            let gap = t.iter().zip(&kps).map(|(a, b)| distance(*a, *b)).sum::<f32>() / 5.0;
            st.detect_gap_px = gap;
            // The tracker is steady from frame to frame but drifts slowly; the
            // detector does not drift but is a pixel or two off each time. So
            // the detection pulls the tracked points part of the way. Only a
            // large disagreement, which means the tracker went wrong, is taken whole.
            // The pull grows with the disagreement: a little while they differ
            // by no more than the detector's own noise, everything once they
            // differ by a clear margin.
            let relative = gap / distance(kps[0], kps[1]).max(1.0);
            let t01 = ((relative - NOISE_FRACTION) / (SNAP_FRACTION - NOISE_FRACTION)).clamp(0.0, 1.0);
            let mut pull = DETECT_PULL + (1.0 - DETECT_PULL) * t01 * t01 * (3.0 - 2.0 * t01);
            // The face was taken up from a stale detection. This one was
            // carried forward properly, so it is the first true fix: take it whole.
            if st.unconfirmed && since.is_some() {
                pull = 1.0;
                st.unconfirmed = false;
            }
            for (point, held) in kps.iter_mut().zip(&t) {
                *point = [held[0] + (point[0] - held[0]) * pull, held[1] + (point[1] - held[1]) * pull];
            }
        }
        _ => {
            // A different face, or the first one: nothing carries over from before.
            st.smoother.reset();
            st.smoothed = None;
            st.presence = 0.0;
            *lock(&shared.patch) = None;
            // With no face followed there was no motion to carry the detection
            // forward by, unless it is from this very frame.
            st.unconfirmed = detected_on != now;
        }
    }
    st.tracker.observe(gray, &kps);
}

fn detect_worker(shared: &Shared, mut detector: Detector) {
    while let Some(job) = shared.detect_jobs.take() {
        let started = Instant::now();
        let outcome = ImageView::new(&job.buffer, job.width, job.height, job.stride, job.format)
            .ok_or_else(|| InferError::Model("frame copy has the wrong size".into()))
            .and_then(|view| detector.detect(&view, job.threshold));
        let (faces, failed) = match outcome {
            Ok(faces) => (faces, false),
            Err(error) => {
                shared.fail(&error);
                (Vec::new(), true)
            }
        };
        {
            let mut stats = lock(&shared.stats);
            stats.detections += 1;
            stats.detect_ms = smooth(stats.detect_ms, started.elapsed().as_secs_f32() * 1000.0);
        }
        shared.detect_results.put(DetectResult { frame_index: job.frame_index, faces, failed, buffer: job.buffer });
    }
}

/// Keeps the occlusion mask up to date with the newest crop.
///
/// It runs beside the swap model, not in step with it: waiting for both on
/// every patch cost a third of the swaps a second. The swap thread takes
/// whichever mask is newest, which is nearly always the one for its own crop,
/// because this model is the faster of the two.
fn occlusion_worker(shared: &Shared, mut occluder: Occluder, size: u32) {
    while let Some((crop, arrived)) = shared.occlusion_jobs.take() {
        match occluder.mask(&crop, size) {
            Ok(mask) => *lock(&shared.uncovered) = Some((Arc::new(mask), arrived)),
            Err(error) => shared.fail(&error),
        }
    }
}

fn swap_worker(shared: &Shared, mut swapper: Swapper) {
    let size = swapper.contract().crop_size;
    let occlusion_model = lock(&shared.stats).occlusion;
    let box_mask = image::crop_mask(size, MaskShape::Box);
    let oval_mask = image::crop_mask(size, MaskShape::Oval);
    // 255 everywhere except over the mouth, for showing the real mouth.
    let mut mouth_hole = vec![255u8; (size * size) as usize];
    if let Some((centre, radii)) = image::mouth_oval(size) {
        image::cut_oval(&mut mouth_hole, size, centre, radii, 0.35);
    }
    // The identity as the encoder gave it, and as this model wants it.
    let mut prepared: Option<(Arc<Vec<f32>>, Vec<f32>)> = None;
    let mut sequence = 0u64;
    while let Some(job) = shared.swap_jobs.take() {
        let Some(identity) = lock(&shared.identity).clone() else { continue };
        if !prepared.as_ref().is_some_and(|(raw, _)| Arc::ptr_eq(raw, &identity)) {
            match swapper.identity(&identity) {
                Ok(input) => prepared = Some((identity.clone(), input)),
                Err(error) => {
                    shared.fail(&error);
                    continue;
                }
            }
        }
        let Some((_, input)) = &prepared else { continue };
        let settings = shared.settings();
        let started = Instant::now();
        // The occlusion model looks at the same crop while the swap model works on it.
        let occlusion = settings.occlusion && occlusion_model;
        if occlusion {
            shared.occlusion_jobs.put((job.crop.clone(), job.arrived));
        }
        let swapped = match swapper.swap(&job.crop, input) {
            Ok(swapped) => swapped,
            Err(error) => {
                shared.fail(&error);
                continue;
            }
        };
        let uncovered = lock(&shared.uncovered)
            .clone()
            .filter(|(_, cut)| occlusion && job.arrived.saturating_duration_since(*cut) <= MASK_FRESH);
        let shape = if settings.mask == MaskShape::Box { &box_mask } else { &oval_mask };
        let mut mask = shape.clone();
        // Every further mask narrows the shape; none widens it.
        let mut narrow = |other: &[u8]| {
            for (value, limit) in mask.iter_mut().zip(other) {
                *value = ((*value as u16 * *limit as u16) / 255) as u8;
            }
        };
        if let Some(own) = &swapped.mask {
            narrow(own);
        }
        // Without a mask the swap covers what is in front of the face, as it did before.
        if let Some((uncovered, _)) = &uncovered {
            narrow(uncovered);
        }
        if settings.real_mouth {
            narrow(&mouth_hole);
        }
        let mut rgb = swapped.rgb;
        if settings.colour_match {
            image::match_colour(&mut rgb, &job.crop, &mask);
        }
        image::sharpen(&mut rgb, size, settings.sharpen.clamp(0.0, 1.0) * 1.2);
        let mut rgba = Vec::with_capacity(mask.len() * 4);
        for (pixel, alpha) in rgb.as_chunks::<3>().0.iter().zip(&mask) {
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], *alpha]);
        }
        // The identity may have changed while the model ran; that patch would show the old face.
        if !lock(&shared.identity).as_ref().is_some_and(|current| Arc::ptr_eq(current, &identity)) {
            continue;
        }
        sequence += 1;
        *lock(&shared.patch) = Some(Arc::new(Patch {
            sequence,
            size,
            rgba,
            frame_index: job.frame_index,
            frame_time_ns: job.frame_time_ns,
        }));
        let mut stats = lock(&shared.stats);
        stats.swaps += 1;
        stats.swap_ms = smooth(stats.swap_ms, started.elapsed().as_secs_f32() * 1000.0);
        stats.swap_age_ms = smooth(stats.swap_age_ms, job.arrived.elapsed().as_secs_f32() * 1000.0);
    }
}

/// What enrolling a face from a photo found.
#[derive(Debug, Clone)]
pub struct Enrolment {
    /// The identity vector to give to `LiveEngine::set_identity`.
    pub identity: Vec<f32>,
    /// How many faces the photo shows. The largest one was used.
    pub faces: usize,
    /// That face's box, `[x1, y1, x2, y2]`, and detector score.
    pub bbox: [f32; 4],
    pub score: f32,
    pub keypoints: Keypoints,
}

/// Turns a photo of a face into an identity vector. Used when a face is added
/// to the library, not while the camera runs, so it stays on the CPU.
pub struct Enroller {
    detector: Detector,
    encoder: IdentityEncoder,
}

impl Enroller {
    /// `packs` are the folders that hold the model files, searched in order.
    pub fn new(packs: &[PathBuf], cache_dir: Option<&std::path::Path>) -> Result<Self, InferError> {
        let runtime = Runtime::shared(cache_dir)?;
        Ok(Self {
            detector: Detector::load(runtime, &find_model(packs, "buffalo_l/det_10g.onnx"), 640, Accelerator::Cpu)?,
            encoder: IdentityEncoder::load(runtime, &find_model(packs, "buffalo_l/w600k_r50.onnx"), Accelerator::Cpu)?,
        })
    }

    /// Faces in a picture, best first.
    pub fn detect(&mut self, picture: &ImageView<'_>) -> Result<Vec<Detection>, InferError> {
        self.detector.detect(picture, 0.5)
    }

    pub fn identity(&mut self, picture: &ImageView<'_>, kps: &Keypoints) -> Result<Vec<f32>, InferError> {
        self.encoder.embed(picture, kps)
    }

    /// The identity of the largest face in a photo, or `None` if it shows no face.
    pub fn enrol(&mut self, picture: &ImageView<'_>) -> Result<Option<Enrolment>, InferError> {
        let faces = self.detect(picture)?;
        let area = |face: &Detection| (face.bbox[2] - face.bbox[0]) * (face.bbox[3] - face.bbox[1]);
        let Some(face) = faces.iter().max_by(|a, b| area(a).total_cmp(&area(b))) else { return Ok(None) };
        let identity = self.identity(picture, &face.kps)?;
        Ok(Some(Enrolment { identity, faces: faces.len(), bbox: face.bbox, score: face.score, keypoints: face.kps }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_the_camera_shows_the_swap_and_profile_hides_it() {
        // Eyes level, nose midway.
        let frontal: Keypoints = [[40.0, 50.0], [80.0, 50.0], [60.0, 70.0], [45.0, 90.0], [75.0, 90.0]];
        assert_eq!(turn_fade(&frontal), 1.0);
        // Three-quarter view: still swapped.
        let mut turned = frontal;
        turned[2][0] = 72.0;
        assert_eq!(turn_fade(&turned), 1.0);
        // Nose under the right eye: profile.
        let mut profile = frontal;
        profile[2][0] = 80.0;
        assert_eq!(turn_fade(&profile), 0.0);
        // Eyes on top of each other: no usable face.
        let mut collapsed = frontal;
        collapsed[1] = collapsed[0];
        assert_eq!(turn_fade(&collapsed), 0.0);
    }

    #[test]
    fn model_plan_follows_the_measurements() {
        let config = LiveConfig {
            packs: vec![PathBuf::from("/nonexistent")],
            cache_dir: None,
            detector: DetectorKind::Large,
            detector_size: 640,
            swap_model: SwapModel::Hyperswap256,
            cpu_only: false,
        };
        let plan = ModelPlan::new(&config);
        assert_eq!(plan.swapper.3, Accelerator::Gpu);
        // On the Mac the detector stays off the units the swap model can use.
        // A PC's processor is too slow for it, so there it shares the graphics card.
        assert_eq!(plan.detector.1, if cfg!(windows) { Accelerator::Gpu } else { Accelerator::Cpu });

        let cpu = ModelPlan::new(&LiveConfig { cpu_only: true, swap_model: SwapModel::Inswapper128, ..config.clone() });
        assert_eq!(cpu.swapper.3, Accelerator::Cpu);
        assert_eq!(cpu.detector.1, Accelerator::Cpu);
        assert!(cpu.swapper.0.ends_with("inswapper_128.onnx"));

        let small = ModelPlan::new(&LiveConfig { detector: DetectorKind::Small, ..config });
        assert_eq!(small.detector.1, Accelerator::Cpu);
    }
}
