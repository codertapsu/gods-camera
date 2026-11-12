//! The app's end of the Windows camera.
//!
//! Windows loads the camera's media source (`apps/windows/vcam`) into its
//! camera service, which runs as another account. Pictures reach it through a
//! named pipe that the app owns: [`Feed`]. [`VirtualCamera`] asks Windows to
//! show the camera in every app's camera list for as long as it is held.
//!
//! ```text
//!   engine -> Feed (pipe server) ----> media source in the camera service -> Zoom, Teams, Chrome...
//!             VirtualCamera: "Windows, there is a camera called God's Camera"
//! ```
//!
//! The scanner's hub needs three more things from Windows, in `network`:
//! the computer's name, an announcement of the hub on the local network, and
//! what Windows Firewall will do when a phone calls.
//!
//! On every other system this crate is empty.
#![cfg(windows)]

mod camera;
mod feed;
mod media;
mod network;
mod output;
pub mod protocol;
mod vault;

pub use camera::{CameraError, CameraKeeper, VirtualCamera};
pub use feed::{now_100ns, Feed, FeedError, FeedStats};
pub use media::{cameras, CameraDevice, Capture, CaptureError, CaptureSource, OnFrame};
pub use network::{addresses, computer_name, firewall, Announced, Announcement, FirewallAnswer, RULE_PRIVATE, RULE_PUBLIC, SERVICE_TYPE};
pub use output::CameraOutput;
pub use protocol::Wanted;
pub use vault::Credential;
