//! What the desktop app's screens show and do, without the screens.
//!
//! The Mac app keeps this in Swift beside its views. For the Windows app it
//! is here, so that it can be tested without a window and the shell stays
//! thin. UniFFI writes the window's side of it, as it does for the core.

uniffi::setup_scaffolding!();

pub mod faces;
pub mod model;
pub mod pattern;
pub mod platform;
pub mod scanner;
pub mod secrets;
pub mod settings;
pub mod voice;

pub use model::{AppConfig, AppState, DesktopApp, Platform};
pub use scanner::{Scanner, ScannerConfig, ScannerState};
