//! The camera pipeline: pictures come from a source, the face swap is drawn on
//! them, and the outgoing picture goes to the virtual camera and the preview.
//! A clip of the output can be recorded and played back as a loop.
//!
//! The Mac app does this work in Swift (`CameraEngine`, `Clips`, `Looping`),
//! on the graphics chip. This crate is the same behaviour for desktops whose
//! app shell is not Swift, drawn on the processor by `gc-render`. Nothing in
//! it belongs to one system: where frames come from and where they go are
//! traits, and the Windows ends of both are in `gc-win`.

pub mod badge;
pub mod clip;
pub mod engine;
pub mod looping;
pub mod thumbnail;

pub use badge::Badge;
pub use clip::{Clip, ClipDecoder, ClipError, ClipLibrary, ClipRecorder, ClipSummary};
pub use engine::{CameraEngine, LoopStopper, Mode, Output, PreviewInfo, Settings, Status, SwapReport, SwapStage, Swapped, SwappedFace};
pub use looping::{LoopMode, LoopPoints, LoopSequencer, LoopStep};
pub use thumbnail::Thumbnail;
