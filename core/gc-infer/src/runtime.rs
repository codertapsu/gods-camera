//! Model sessions on ONNX Runtime: the detector, the identity encoder and the
//! swap models.
//!
//! Everything here works on small CPU images. Each app keeps full frames on
//! the GPU and hands over only the detector's input and the aligned crop.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gc_align::Keypoints;
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;

use crate::image::{self, ImageView};
use crate::scrfd::{self, Detection, ScrfdOutputs};
use crate::swap::{self, IdentityInput, SwapperContract};

#[derive(Debug)]
pub enum InferError {
    /// The inference library reported an error.
    Runtime(String),
    /// A model file is missing or is not the model that was expected.
    Model(String),
}

impl std::fmt::Display for InferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InferError::Runtime(message) => write!(f, "inference failed: {message}"),
            InferError::Model(message) => write!(f, "model problem: {message}"),
        }
    }
}

impl std::error::Error for InferError {}

fn runtime_error<E: std::fmt::Display>(error: E) -> InferError {
    InferError::Runtime(error.to_string())
}

/// Which part of the machine runs a model.
///
/// On Apple devices these select Core ML compute units. Elsewhere only `Cpu`
/// has an effect so far; the other platforms get their own providers when
/// their phases begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accelerator {
    Cpu,
    Gpu,
    NeuralEngine,
    /// Let the system choose.
    Any,
}

/// The inference library, set up once per process.
pub struct Runtime {
    environment: Environment,
    /// Where compiled models are kept, so later starts are fast. Only Core ML uses it so far.
    #[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
    cache_dir: Option<PathBuf>,
}

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

impl Runtime {
    /// The process-wide runtime. The library allows only one per process, so
    /// the first call decides `cache_dir` and later calls get the same runtime.
    pub fn shared(cache_dir: Option<&Path>) -> Result<&'static Runtime, InferError> {
        RUNTIME
            .get_or_init(|| {
                if let Some(dir) = cache_dir {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                let environment = ort::init().with_name("gods-camera").build().map_err(|e| e.to_string())?;
                Ok(Runtime { environment, cache_dir: cache_dir.map(Path::to_path_buf) })
            })
            .as_ref()
            .map_err(|message| InferError::Runtime(message.clone()))
    }

    /// Open a model. `pinned` fixes named free dimensions, which Core ML needs
    /// for models whose input size is left open.
    fn session(
        &self,
        model: &Path,
        accelerator: Accelerator,
        pinned: &[(&str, i64)],
    ) -> Result<Session, InferError> {
        if !model.is_file() {
            return Err(InferError::Model(format!("{} is missing", model.display())));
        }
        let mut builder = Session::builder(&self.environment).map_err(runtime_error)?;
        for (name, size) in pinned {
            builder = builder.with_dimension_override(name, *size).map_err(runtime_error)?;
        }
        #[cfg(target_vendor = "apple")]
        if accelerator != Accelerator::Cpu {
            use ort::ep::coreml::{ComputeUnits, CoreML, ModelFormat};
            let units = match accelerator {
                Accelerator::Gpu => ComputeUnits::CPUAndGPU,
                Accelerator::NeuralEngine => ComputeUnits::CPUAndNeuralEngine,
                _ => ComputeUnits::All,
            };
            let mut provider =
                CoreML::default().with_model_format(ModelFormat::MLProgram).with_compute_units(units);
            if let Some(dir) = &self.cache_dir {
                provider = provider.with_model_cache_dir(dir.display());
            }
            builder = builder.with_execution_providers([provider.build()]).map_err(runtime_error)?;
        }
        #[cfg(not(target_vendor = "apple"))]
        let _ = accelerator;
        builder.commit_from_file(model).map_err(runtime_error)
    }
}

/// The SCRFD face detector.
pub struct Detector {
    session: Session,
    canvas: u32,
}

impl Detector {
    /// `canvas` is the side of the square the frame is fitted into: 640 for
    /// accuracy, 320 for speed. It must be a multiple of 32.
    pub fn load(
        runtime: &Runtime,
        model: &Path,
        canvas: u32,
        accelerator: Accelerator,
    ) -> Result<Self, InferError> {
        if canvas == 0 || !canvas.is_multiple_of(32) {
            return Err(InferError::Model(format!("detector size {canvas} is not a multiple of 32")));
        }
        // InsightFace's detectors name both open dimensions "?".
        let session = runtime.session(model, accelerator, &[("?", canvas as i64)])?;
        if session.outputs().len() != 9 {
            return Err(InferError::Model(format!(
                "{} has {} outputs; a detector with keypoints has 9",
                model.display(),
                session.outputs().len()
            )));
        }
        Ok(Self { session, canvas })
    }

    pub fn canvas(&self) -> u32 {
        self.canvas
    }

    /// Faces in `frame`, best first, in frame coordinates.
    pub fn detect(&mut self, frame: &ImageView<'_>, score_threshold: f32) -> Result<Vec<Detection>, InferError> {
        let (input, geometry) = image::detector_input(frame, self.canvas);
        let side = self.canvas as usize;
        let tensor = Tensor::from_array(([1usize, 3, side, side], input.into_boxed_slice())).map_err(runtime_error)?;
        let outputs = self.session.run(ort::inputs![tensor]).map_err(runtime_error)?;

        // Sort the nine outputs by what they hold (1, 4 or 10 values an anchor)
        // and by how many anchors they cover (most first: stride 8, 16, 32).
        let mut scores: Vec<&[f32]> = Vec::new();
        let mut boxes: Vec<&[f32]> = Vec::new();
        let mut points: Vec<&[f32]> = Vec::new();
        for index in 0..outputs.len() {
            let (shape, data) = outputs[index].try_extract_tensor::<f32>().map_err(runtime_error)?;
            match shape.last().copied() {
                Some(1) => scores.push(data),
                Some(4) => boxes.push(data),
                Some(10) => points.push(data),
                other => return Err(InferError::Model(format!("unexpected detector output of width {other:?}"))),
            }
        }
        for group in [&mut scores, &mut boxes, &mut points] {
            group.sort_by_key(|data| std::cmp::Reverse(data.len()));
        }
        let (Ok(scores), Ok(bboxes), Ok(kps)) = (
            <[&[f32]; 3]>::try_from(scores),
            <[&[f32]; 3]>::try_from(boxes),
            <[&[f32]; 3]>::try_from(points),
        ) else {
            return Err(InferError::Model("detector outputs are not three of each kind".into()));
        };
        scrfd::decode(
            &ScrfdOutputs { scores, bboxes, kps },
            self.canvas,
            geometry.det_scale,
            score_threshold,
            scrfd::DEFAULT_NMS_THRESHOLD,
        )
        .map_err(|e| InferError::Model(e.to_string()))
    }
}

/// The ArcFace identity encoder: a face in, a 512-number identity vector out.
pub struct IdentityEncoder {
    session: Session,
}

impl IdentityEncoder {
    pub const CROP: u32 = 112;
    pub const LENGTH: usize = 512;

    pub fn load(runtime: &Runtime, model: &Path, accelerator: Accelerator) -> Result<Self, InferError> {
        // The batch dimension is left open under the name "None".
        Ok(Self { session: runtime.session(model, accelerator, &[("None", 1)])? })
    }

    /// The identity vector of the face with these keypoints. It is not
    /// normalised; each swap model prepares it in its own way.
    pub fn embed(&mut self, frame: &ImageView<'_>, kps: &Keypoints) -> Result<Vec<f32>, InferError> {
        let matrix = gc_align::estimate_norm(kps, Self::CROP).map_err(|e| InferError::Model(e.to_string()))?;
        let crop = image::warp_to_crop(frame, &matrix, Self::CROP);
        self.embed_crop(&crop)
    }

    /// The identity vector of an already aligned 112 x 112 RGB crop.
    pub fn embed_crop(&mut self, crop_rgb: &[u8]) -> Result<Vec<f32>, InferError> {
        let side = Self::CROP as usize;
        if crop_rgb.len() != side * side * 3 {
            return Err(InferError::Model("the identity encoder needs a 112 x 112 RGB crop".into()));
        }
        // (value - 127.5) / 127.5, in RGB order.
        let input = image::crop_to_planar(crop_rgb, 0.5, 0.5);
        let tensor = Tensor::from_array(([1usize, 3, side, side], input.into_boxed_slice())).map_err(runtime_error)?;
        let outputs = self.session.run(ort::inputs![tensor]).map_err(runtime_error)?;
        let (_, data) = outputs[0].try_extract_tensor::<f32>().map_err(runtime_error)?;
        if data.len() != Self::LENGTH {
            return Err(InferError::Model(format!("identity vector has {} numbers, not 512", data.len())));
        }
        Ok(data.to_vec())
    }
}

/// What a swap model returns for one crop.
pub struct SwapResult {
    /// The swapped crop, packed RGB, the same size as the input crop.
    pub rgb: Vec<u8>,
    /// The model's own face mask, when it produces one: one byte per pixel.
    pub mask: Option<Vec<u8>>,
}

/// A face-swap model behind the swapper contract.
pub struct Swapper {
    session: Session,
    contract: SwapperContract,
    emap: Option<Vec<f32>>,
}

impl Swapper {
    /// `emap` is the identity projection matrix file, needed by models whose
    /// contract says `EmapProjected`: 512 x 512 little-endian floats.
    pub fn load(
        runtime: &Runtime,
        model: &Path,
        contract: SwapperContract,
        emap: Option<&Path>,
        accelerator: Accelerator,
    ) -> Result<Self, InferError> {
        let emap = match (contract.identity, emap) {
            (IdentityInput::EmapProjected, Some(path)) => {
                let bytes = std::fs::read(path).map_err(|e| InferError::Model(format!("{}: {e}", path.display())))?;
                let side = IdentityEncoder::LENGTH;
                if bytes.len() != side * side * 4 {
                    return Err(InferError::Model(format!("{} is not a 512 x 512 matrix", path.display())));
                }
                Some(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
            }
            (IdentityInput::EmapProjected, None) => {
                return Err(InferError::Model(format!("{} needs its projection matrix file", contract.name)));
            }
            _ => None,
        };
        let session = runtime.session(model, accelerator, &[])?;
        Ok(Self { session, contract, emap })
    }

    pub fn contract(&self) -> &SwapperContract {
        &self.contract
    }

    /// Prepare an identity vector from the encoder for this model.
    pub fn identity(&self, embedding: &[f32]) -> Result<Vec<f32>, InferError> {
        if embedding.len() != IdentityEncoder::LENGTH {
            return Err(InferError::Model("an identity vector has 512 numbers".into()));
        }
        match self.contract.identity {
            IdentityInput::EmapProjected => {
                let emap = self.emap.as_deref().unwrap_or(&[]);
                swap::latent_from_embedding(embedding, emap)
                    .ok_or_else(|| InferError::Model("projection matrix has the wrong size".into()))
            }
            IdentityInput::Normalised => {
                let norm = embedding.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>().sqrt() as f32;
                if norm <= 0.0 {
                    return Err(InferError::Model("the identity vector is all zeros".into()));
                }
                Ok(embedding.iter().map(|v| v / norm).collect())
            }
            IdentityInput::Raw => Ok(embedding.to_vec()),
        }
    }

    /// Swap the face in an aligned crop. `identity` comes from `identity()`.
    pub fn swap(&mut self, crop_rgb: &[u8], identity: &[f32]) -> Result<SwapResult, InferError> {
        let side = self.contract.crop_size as usize;
        if crop_rgb.len() != side * side * 3 {
            return Err(InferError::Model(format!("{} needs a {side} x {side} RGB crop", self.contract.name)));
        }
        let target = image::crop_to_planar(crop_rgb, self.contract.mean, self.contract.std);
        let target = Tensor::from_array(([1usize, 3, side, side], target.into_boxed_slice())).map_err(runtime_error)?;
        let source = Tensor::from_array(([1usize, identity.len()], identity.to_vec().into_boxed_slice()))
            .map_err(runtime_error)?;
        let outputs =
            self.session.run(ort::inputs!["target" => target, "source" => source]).map_err(runtime_error)?;
        let (_, picture) = outputs[0].try_extract_tensor::<f32>().map_err(runtime_error)?;
        if picture.len() != side * side * 3 {
            return Err(InferError::Model(format!("{} returned {} values", self.contract.name, picture.len())));
        }
        let rgb = image::planar_to_crop(picture, &self.contract);
        let mut mask = None;
        if outputs.len() > 1 {
            let (_, values) = outputs[1].try_extract_tensor::<f32>().map_err(runtime_error)?;
            if values.len() == side * side {
                mask = Some(values.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect());
            }
        }
        Ok(SwapResult { rgb, mask })
    }
}
