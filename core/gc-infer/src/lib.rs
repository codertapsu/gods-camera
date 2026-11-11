//! Model pre- and post-processing, and, with the `runtime` feature, the model
//! sessions themselves.
//!
//! Ported from InsightFace (MIT), `python-package/insightface/model_zoo/scrfd.py`,
//! `arcface_onnx.py` and `inswapper.py`. Without the `runtime` feature this crate
//! only turns frames into the numbers the networks expect and their outputs
//! into faces; with it, it also runs the networks through ONNX Runtime.

pub mod image;
#[cfg(feature = "runtime")]
pub mod runtime;
pub mod scrfd;
pub mod swap;

pub use scrfd::{decode, letterbox_geometry, nms, Detection, Letterbox, ScrfdOutputs};
pub use swap::{latent_from_embedding, SwapperContract};
