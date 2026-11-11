//! Model pre- and post-processing that is independent of the inference runtime.
//!
//! Ported from InsightFace (MIT), `python-package/insightface/model_zoo/scrfd.py`
//! and `inswapper.py`. Running the networks is the caller's job; this crate turns
//! frames into the numbers the networks expect and their outputs into faces.

pub mod scrfd;
pub mod swap;

pub use scrfd::{decode, letterbox_geometry, nms, Detection, Letterbox, ScrfdOutputs};
pub use swap::{latent_from_embedding, SwapperContract};
