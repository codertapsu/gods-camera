//! The core as the apps see it: a small interface that UniFFI turns into
//! Swift and Kotlin.
//!
//! Frames cross as an address and a length, not as a copy. Everything else is
//! small: a handful of numbers per frame, and a 128 or 256 pixel patch when a
//! new one is ready.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gc_live::{
    DetectorKind, Enroller, ImageView, LiveConfig, LiveEngine, LiveSettings, MaskShape, PixelFormat, SwapModel,
};

uniffi::setup_scaffolding!();

#[derive(Debug, uniffi::Error)]
pub enum CoreError {
    /// A model is missing or the inference library failed. The message says which.
    Failed { message: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<gc_live::InferError> for CoreError {
    fn from(error: gc_live::InferError) -> Self {
        CoreError::Failed { message: error.to_string() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SwapModelKind {
    Inswapper128,
    Hyperswap256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DetectorChoice {
    Large,
    Small,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MaskKind {
    Oval,
    Box,
}

/// How the bytes of one pixel are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PixelLayout {
    Bgra,
    Rgba,
    Bgr,
    Rgb,
}

impl From<PixelLayout> for PixelFormat {
    fn from(layout: PixelLayout) -> Self {
        match layout {
            PixelLayout::Bgra => PixelFormat::Bgra,
            PixelLayout::Rgba => PixelFormat::Rgba,
            PixelLayout::Bgr => PixelFormat::Bgr,
            PixelLayout::Rgb => PixelFormat::Rgb,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineConfig {
    /// Folders that hold model files, searched in order.
    pub packs: Vec<String>,
    /// Folder for compiled models, so later starts are fast.
    pub cache_dir: Option<String>,
    pub detector: DetectorChoice,
    /// Side of the detector's input square: 640 or 320.
    pub detector_size: u32,
    pub swap_model: SwapModelKind,
    pub cpu_only: bool,
}

#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct EngineSettings {
    pub mask: MaskKind,
    pub colour_match: bool,
    /// 0 shows the real face, 1 the swapped one.
    pub opacity: f32,
    pub score_threshold: f32,
}

/// What to draw for one frame.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FrameOutput {
    pub face: bool,
    /// Maps patch pixels onto the frame: `x' = a x + b y + tx`, `y' = c x + d y + ty`.
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub c: f32,
    pub d: f32,
    pub ty: f32,
    /// How strongly to draw the patch, 0 to 1.
    pub opacity: f32,
    /// The patch to draw, or 0 for none.
    pub patch_sequence: u64,
    pub patch_age_frames: u32,
    /// Five keypoints as x, y pairs, in frame pixels.
    pub keypoints: Vec<f32>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PatchData {
    pub sequence: u64,
    /// Side of the square, in pixels.
    pub size: u32,
    /// Red, green, blue, and the mask as alpha; not premultiplied.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineStats {
    pub frames: u64,
    pub detections: u64,
    pub swaps: u64,
    pub frame_ms: f32,
    pub detect_ms: f32,
    pub swap_ms: f32,
    pub swap_age_ms: f32,
    pub tracking: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EnrolledFace {
    /// 512 numbers that stand for the face. Give them to `FaceEngine::set_identity`.
    pub identity: Vec<f32>,
    /// How many faces the picture shows. The largest was used.
    pub faces: u32,
    /// `[x1, y1, x2, y2]` of that face.
    pub bbox: Vec<f32>,
    pub score: f32,
    pub keypoints: Vec<f32>,
}

fn paths(folders: &[String]) -> Vec<PathBuf> {
    folders.iter().map(PathBuf::from).collect()
}

/// The live face-swap engine. See `gc_live::LiveEngine`.
#[derive(uniffi::Object)]
pub struct FaceEngine {
    inner: LiveEngine,
}

#[uniffi::export]
impl FaceEngine {
    /// Loads the models, which can take seconds. Call it off the main thread.
    #[uniffi::constructor]
    pub fn new(config: EngineConfig) -> Result<Arc<Self>, CoreError> {
        let inner = LiveEngine::new(&LiveConfig {
            packs: paths(&config.packs),
            cache_dir: config.cache_dir.map(PathBuf::from),
            detector: match config.detector {
                DetectorChoice::Large => DetectorKind::Large,
                DetectorChoice::Small => DetectorKind::Small,
            },
            detector_size: config.detector_size,
            swap_model: match config.swap_model {
                SwapModelKind::Inswapper128 => SwapModel::Inswapper128,
                SwapModelKind::Hyperswap256 => SwapModel::Hyperswap256,
            },
            cpu_only: config.cpu_only,
        })?;
        Ok(Arc::new(Self { inner }))
    }

    /// Side of the swapped patch, in pixels.
    pub fn crop_size(&self) -> u32 {
        self.inner.crop_size()
    }

    /// The face to swap in, or `None` to stop swapping.
    pub fn set_identity(&self, identity: Option<Vec<f32>>) {
        self.inner.set_identity(identity);
    }

    pub fn set_settings(&self, settings: EngineSettings) {
        self.inner.set_settings(LiveSettings {
            mask: match settings.mask {
                MaskKind::Oval => MaskShape::Oval,
                MaskKind::Box => MaskShape::Box,
            },
            colour_match: settings.colour_match,
            opacity: settings.opacity,
            score_threshold: settings.score_threshold,
        });
    }

    /// Take in one camera frame.
    ///
    /// `address` is the memory address of the frame's first byte and `length`
    /// the number of bytes readable from there. The caller keeps that memory
    /// alive and unchanged until this call returns; nothing keeps the address
    /// afterwards.
    pub fn push_frame(
        &self,
        address: u64,
        length: u64,
        width: u32,
        height: u32,
        stride: u32,
        layout: PixelLayout,
        time_ns: u64,
    ) -> FrameOutput {
        let none = FrameOutput {
            face: false,
            a: 1.0,
            b: 0.0,
            tx: 0.0,
            c: 0.0,
            d: 1.0,
            ty: 0.0,
            opacity: 0.0,
            patch_sequence: 0,
            patch_age_frames: 0,
            keypoints: Vec::new(),
        };
        if address == 0 || length == 0 {
            return none;
        }
        // SAFETY: the caller promises `length` readable bytes at `address` for
        // the length of this call, as the method's contract states.
        let bytes = unsafe { std::slice::from_raw_parts(address as usize as *const u8, length as usize) };
        let Some(frame) = ImageView::new(bytes, width, height, stride as usize, layout.into()) else {
            return none;
        };
        let result = self.inner.push_frame(&frame, time_ns);
        let m = result.crop_to_frame.0;
        FrameOutput {
            face: result.face,
            a: m[0][0],
            b: m[0][1],
            tx: m[0][2],
            c: m[1][0],
            d: m[1][1],
            ty: m[1][2],
            opacity: result.opacity,
            patch_sequence: result.patch_sequence,
            patch_age_frames: result.patch_age_frames,
            keypoints: result.keypoints.iter().flatten().copied().collect(),
        }
    }

    /// The newest patch, if its number is above `newer_than`. Ask with the
    /// number of the patch already in hand, so pixels cross only when they changed.
    pub fn patch(&self, newer_than: u64) -> Option<PatchData> {
        let patch = self.inner.patch()?;
        (patch.sequence > newer_than)
            .then(|| PatchData { sequence: patch.sequence, size: patch.size, rgba: patch.rgba.clone() })
    }

    pub fn stats(&self) -> EngineStats {
        let stats = self.inner.stats();
        EngineStats {
            frames: stats.frames,
            detections: stats.detections,
            swaps: stats.swaps,
            frame_ms: stats.frame_ms,
            detect_ms: stats.detect_ms,
            swap_ms: stats.swap_ms,
            swap_age_ms: stats.swap_age_ms,
            tracking: stats.tracking,
            last_error: stats.last_error,
        }
    }
}

/// Turns a photo of a face into an identity. See `gc_live::Enroller`.
#[derive(uniffi::Object)]
pub struct FaceEnroller {
    inner: Mutex<Enroller>,
}

#[uniffi::export]
impl FaceEnroller {
    #[uniffi::constructor]
    pub fn new(packs: Vec<String>, cache_dir: Option<String>) -> Result<Arc<Self>, CoreError> {
        let inner = Enroller::new(&paths(&packs), cache_dir.as_deref().map(std::path::Path::new))?;
        Ok(Arc::new(Self { inner: Mutex::new(inner) }))
    }

    /// The identity of the largest face in a picture, or `None` if it shows no face.
    pub fn enrol(
        &self,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        stride: u32,
        layout: PixelLayout,
    ) -> Result<Option<EnrolledFace>, CoreError> {
        let picture = ImageView::new(&pixels, width, height, stride as usize, layout.into())
            .ok_or_else(|| CoreError::Failed { message: "the picture's size does not match its data".into() })?;
        let mut enroller = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Ok(enroller.enrol(&picture)?.map(|found| EnrolledFace {
            identity: found.identity,
            faces: found.faces as u32,
            bbox: found.bbox.to_vec(),
            score: found.score,
            keypoints: found.keypoints.iter().flatten().copied().collect(),
        }))
    }
}

/// The version of the core, for diagnostics.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
