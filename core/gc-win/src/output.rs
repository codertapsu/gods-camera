//! The camera's pipe as the place the camera engine sends its pictures.

use crate::feed::{now_100ns, Feed};
use gc_camera::Output;
use std::sync::Arc;

/// Hands the engine's pictures to the camera's media source.
pub struct CameraOutput {
    feed: Arc<Feed>,
}

impl CameraOutput {
    pub fn new(feed: Arc<Feed>) -> Self {
        Self { feed }
    }

    /// The clock to make the engine with: the present moment in nanoseconds,
    /// on the clock the camera's pictures are stamped by.
    pub fn clock() -> u64 {
        now_100ns() as u64 * 100
    }
}

impl Output for CameraOutput {
    fn wanted(&self) -> Option<(u32, u32)> {
        self.feed.watched().map(|wanted| (wanted.width, wanted.height))
    }

    fn send(&mut self, nv12: &[u8], width: u32, height: u32, host_ns: u64) {
        self.feed.send(nv12, width, height, (host_ns / 100) as i64);
    }
}
