//! Temporal pieces of the live engine: the optical-flow face tracker,
//! smoothing, detection scheduling and late reprojection.

pub mod flow;
pub mod synthetic;

pub use flow::{FlowConfig, FlowTracker, Gray, TrackOutcome};

use gc_align::{Affine, Keypoints};

/// One Euro filter for a single value (Casiez et al., 2012).
#[derive(Debug, Clone)]
pub struct OneEuro {
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    prev: Option<(f32, f32)>, // (value, derivative)
}

fn alpha(cutoff: f32, dt: f32) -> f32 {
    let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
    1.0 / (1.0 + tau / dt)
}

impl OneEuro {
    pub fn new(min_cutoff: f32, beta: f32) -> Self {
        Self { min_cutoff, beta, d_cutoff: 1.0, prev: None }
    }

    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// Filter `x`, observed `dt` seconds after the previous sample.
    pub fn filter(&mut self, x: f32, dt: f32) -> f32 {
        let Some((px, pdx)) = self.prev else {
            self.prev = Some((x, 0.0));
            return x;
        };
        if dt <= 0.0 {
            return px;
        }
        let dx = (x - px) / dt;
        let dx_hat = pdx + alpha(self.d_cutoff, dt) * (dx - pdx);
        let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
        let x_hat = px + alpha(cutoff, dt) * (x - px);
        self.prev = Some((x_hat, dx_hat));
        x_hat
    }
}

/// Smooths five keypoints, with a deadband that holds them still at rest.
#[derive(Debug, Clone)]
pub struct KeypointSmoother {
    filters: Vec<OneEuro>,
    deadband_px: f32,
    last: Option<Keypoints>,
}

impl KeypointSmoother {
    pub fn new(min_cutoff: f32, beta: f32, deadband_px: f32) -> Self {
        Self { filters: vec![OneEuro::new(min_cutoff, beta); 10], deadband_px, last: None }
    }

    pub fn reset(&mut self) {
        self.filters.iter_mut().for_each(OneEuro::reset);
        self.last = None;
    }

    pub fn update(&mut self, kps: &Keypoints, dt: f32) -> Keypoints {
        let mut out = *kps;
        for (i, p) in out.iter_mut().enumerate() {
            p[0] = self.filters[2 * i].filter(p[0], dt);
            p[1] = self.filters[2 * i + 1].filter(p[1], dt);
        }
        if let Some(last) = self.last {
            let max_move = out
                .iter()
                .zip(&last)
                .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
                .fold(0.0f32, f32::max);
            if max_move < self.deadband_px {
                return last;
            }
        }
        self.last = Some(out);
        out
    }
}

/// What the tracker observed on a frame, used to decide whether to run the detector.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameSignals {
    /// Seconds since the previous frame.
    pub dt: f32,
    /// Whether a face is currently being tracked.
    pub tracking: bool,
    /// Tracker speed in pixels per frame.
    pub speed_px: f32,
    /// Distance between the tracked position and the last detection, in pixels.
    pub flow_detect_gap_px: f32,
    /// Camera frames skipped since the previous processed frame.
    pub dropped_frames: u32,
    /// A setting changed, so cached state may be stale.
    pub settings_changed: bool,
}

/// Decides on which frames the detector runs.
///
/// The detector runs on a slow wall-clock cadence while tracking and a faster one
/// without it, and is forced on fast motion, tracker disagreement, dropped frames
/// or a settings change.
#[derive(Debug, Clone)]
pub struct DetectScheduler {
    pub interval_tracking_s: f32,
    pub interval_searching_s: f32,
    pub unsettled_px: f32,
    pub max_dropped: u32,
    since_detect_s: f32,
    force_frames: u32,
}

impl Default for DetectScheduler {
    fn default() -> Self {
        Self {
            interval_tracking_s: 0.25,
            interval_searching_s: 0.08,
            unsettled_px: 8.0,
            max_dropped: 3,
            since_detect_s: f32::INFINITY,
            force_frames: 0,
        }
    }
}

impl DetectScheduler {
    /// Returns true if the detector should run on this frame.
    pub fn should_detect(&mut self, s: &FrameSignals) -> bool {
        self.since_detect_s += s.dt.max(0.0);
        if s.settings_changed {
            self.force_frames = 3;
        }
        let interval = if s.tracking { self.interval_tracking_s } else { self.interval_searching_s };
        let forced = self.force_frames > 0
            || s.speed_px > self.unsettled_px
            || s.flow_detect_gap_px > self.unsettled_px
            || s.dropped_frames >= self.max_dropped;
        if forced || self.since_detect_s >= interval {
            self.since_detect_s = 0.0;
            self.force_frames = self.force_frames.saturating_sub(1);
            return true;
        }
        false
    }
}

/// Late reprojection: where to draw a swapped patch on the newest frame.
///
/// `crop_at_swap` mapped frame pixels into the aligned crop on the frame that was
/// swapped. `motion_since` maps that frame's pixels onto the newest frame, as
/// measured by the tracker. The result maps crop pixels onto the newest frame.
pub fn reproject(crop_at_swap: &Affine, motion_since: &Affine) -> Option<Affine> {
    Some(motion_since.compose(&crop_at_swap.invert()?))
}

/// Guard for the latest-wins swap queue: reject a result if the face has jumped.
///
/// Mirrors the reference thresholds: more than two inter-ocular distances of
/// movement, or a scale change beyond 1.5x.
pub fn face_jumped(at_swap: &Keypoints, now: &Keypoints) -> bool {
    let iod = |k: &Keypoints| ((k[1][0] - k[0][0]).powi(2) + (k[1][1] - k[0][1]).powi(2)).sqrt();
    let centre = |k: &Keypoints| {
        let (mut x, mut y) = (0.0, 0.0);
        for p in k {
            x += p[0];
            y += p[1];
        }
        [x / 5.0, y / 5.0]
    };
    let (a, b) = (iod(at_swap), iod(now));
    if a < 1e-3 || b < 1e-3 {
        return true;
    }
    let (ca, cb) = (centre(at_swap), centre(now));
    let moved = ((ca[0] - cb[0]).powi(2) + (ca[1] - cb[1]).powi(2)).sqrt();
    let ratio = (a / b).max(b / a);
    moved > 2.0 * a || ratio > 1.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use gc_align::ARCFACE_112;

    #[test]
    fn one_euro_passes_constant_and_reduces_jitter() {
        let mut f = OneEuro::new(1.0, 0.0);
        assert_eq!(f.filter(5.0, 1.0 / 30.0), 5.0);
        let mut max_dev = 0.0f32;
        for i in 0..200 {
            let noisy = 5.0 + if i % 2 == 0 { 0.5 } else { -0.5 };
            max_dev = max_dev.max((f.filter(noisy, 1.0 / 30.0) - 5.0).abs());
        }
        assert!(max_dev < 0.5, "jitter not reduced: {max_dev}");
    }

    #[test]
    fn one_euro_follows_fast_motion_with_beta() {
        let mut slow = OneEuro::new(1.0, 0.0);
        let mut fast = OneEuro::new(1.0, 0.5);
        let (mut a, mut b) = (0.0, 0.0);
        for i in 0..30 {
            let x = i as f32 * 10.0;
            a = slow.filter(x, 1.0 / 30.0);
            b = fast.filter(x, 1.0 / 30.0);
        }
        assert!((290.0 - b).abs() < (290.0 - a).abs());
    }

    #[test]
    fn deadband_holds_still() {
        let mut s = KeypointSmoother::new(1.0, 0.05, 0.6);
        let first = s.update(&ARCFACE_112, 1.0 / 30.0);
        let mut nudged = ARCFACE_112;
        nudged[0][0] += 0.2;
        assert_eq!(s.update(&nudged, 1.0 / 30.0), first);
    }

    #[test]
    fn scheduler_cadence_and_forcing() {
        let mut d = DetectScheduler::default();
        let frame = FrameSignals { dt: 1.0 / 30.0, tracking: true, ..Default::default() };
        assert!(d.should_detect(&frame), "first frame detects");
        let runs = (0..30).filter(|_| d.should_detect(&frame)).count();
        assert!((3..=5).contains(&runs), "about 4 per second while tracking, got {runs}");

        let fast = FrameSignals { speed_px: 20.0, ..frame };
        assert!(d.should_detect(&fast));
        let dropped = FrameSignals { dropped_frames: 3, ..frame };
        assert!(d.should_detect(&dropped));
        let changed = FrameSignals { settings_changed: true, ..frame };
        assert!(d.should_detect(&changed));
        assert!(d.should_detect(&frame) && d.should_detect(&frame), "three forced frames");
        assert!(!d.should_detect(&frame));
    }

    #[test]
    fn reprojection_follows_motion() {
        let crop = gc_align::estimate_norm(&ARCFACE_112, 128).unwrap();
        let shift = Affine([[1.0, 0.0, 15.0], [0.0, 1.0, -4.0]]);
        let m = reproject(&crop, &shift).unwrap();
        let template = gc_align::template(128).unwrap();
        for (t, k) in template.iter().zip(&ARCFACE_112) {
            let p = m.apply(*t);
            assert!((p[0] - (k[0] + 15.0)).abs() < 1e-2 && (p[1] - (k[1] - 4.0)).abs() < 1e-2);
        }
    }

    #[test]
    fn jump_guard() {
        let mut moved = ARCFACE_112;
        for p in &mut moved {
            p[0] += 10.0;
        }
        assert!(!face_jumped(&ARCFACE_112, &moved));
        for p in &mut moved {
            p[0] += 100.0;
        }
        assert!(face_jumped(&ARCFACE_112, &moved));
        let mut bigger = ARCFACE_112;
        for p in &mut bigger {
            p[0] *= 2.0;
            p[1] *= 2.0;
        }
        assert!(face_jumped(&ARCFACE_112, &bigger));
    }
}
