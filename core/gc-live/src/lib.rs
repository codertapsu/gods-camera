//! The live face-swap engine and the still-image swap built on the same parts.

pub mod engine;
pub mod identity;
pub mod slot;
pub mod still;
pub mod synthetic;

pub use engine::{
    DetectorKind, Enrolment, Enroller, FrameResult, LiveConfig, LiveEngine, LiveSettings, LiveStats, Patch, SwapModel,
};
pub use identity::{combine_identities, CombinedIdentity, PhotoVerdict, SAME_PERSON};
pub use gc_infer::image::{paste_patch, ImageMut, ImageView, MaskShape, PixelFormat};
pub use gc_infer::runtime::{Accelerator, InferError};
