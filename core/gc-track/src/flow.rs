//! Sparse optical flow and the face tracker built on it.
//!
//! Between detector runs the tracker follows a face by tracking its five
//! keypoints and a set of textured support points from one frame to the next
//! (pyramidal Lucas-Kanade), discarding points that fail a forward-backward
//! check, and fitting one similarity transform to the rest with RANSAC.
//!
//! All coordinates are in pixels of the luma frame the tracker is given. The
//! live engine passes a half-resolution luma plane and scales the result.

use gc_align::{estimate_similarity, Affine, Keypoints};

/// An 8-bit luma image borrowed from the caller.
#[derive(Debug, Clone, Copy)]
pub struct Gray<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
    /// Bytes from the start of one row to the start of the next.
    pub stride: usize,
}

impl Gray<'_> {
    fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.stride >= self.width
            && self.data.len() >= self.stride * (self.height - 1) + self.width
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowConfig {
    /// Half-width of the matching window; the window is `2r + 1` pixels square.
    pub window_radius: usize,
    /// Pyramid levels, each half the size of the one below.
    pub levels: usize,
    pub max_iterations: usize,
    /// A point is dropped if tracking it forward and back misses by more than this.
    pub forward_backward_px: f32,
    /// A point agrees with the fitted motion if it lands within this distance.
    pub ransac_px: f32,
    pub ransac_iterations: usize,
    /// Support points to keep, on top of the five keypoints.
    pub max_support: usize,
    /// Fraction of tracked points that must agree with the motion.
    pub min_inlier_ratio: f32,
    /// Motion smaller than this is treated as none, so a still face does not drift.
    pub rest_px: f32,
    /// Margin around the tracked points when cutting the working region.
    pub roi_padding: usize,
}

impl Default for FlowConfig {
    fn default() -> Self {
        Self {
            window_radius: 7,
            levels: 3,
            max_iterations: 12,
            forward_backward_px: 1.5,
            ransac_px: 2.0,
            ransac_iterations: 60,
            max_support: 40,
            min_inlier_ratio: 0.4,
            rest_px: 0.05,
            roi_padding: 44,
        }
    }
}

/// One pyramid level: a float image.
struct Level {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Level {
    /// Whether a bilinear sample at `(x, y)` reads only pixels inside the image.
    #[inline]
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= 0.0 && y >= 0.0 && x <= (self.w - 1) as f32 && y <= (self.h - 1) as f32
    }

    /// Bilinear sample. The caller checks `contains` first.
    #[inline]
    fn sample(&self, x: f32, y: f32) -> f32 {
        let x0 = (x as usize).min(self.w - 2);
        let y0 = (y as usize).min(self.h - 2);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let i = y0 * self.w + x0;
        let top = self.px[i] + (self.px[i + 1] - self.px[i]) * fx;
        let bottom = self.px[i + self.w] + (self.px[i + self.w + 1] - self.px[i + self.w]) * fx;
        top + (bottom - top) * fy
    }

    /// Halve the image: blur with the 1-4-6-4-1 kernel, then keep every second pixel.
    fn downsample(&self) -> Level {
        const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
        let (w, h) = (self.w, self.h);
        let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
        // Horizontal pass, already subsampled in x.
        let mut tmp = vec![0f32; nw * h];
        for y in 0..h {
            let row = &self.px[y * w..(y + 1) * w];
            for nx in 0..nw {
                let cx = (nx * 2) as isize;
                let mut acc = 0.0;
                for (k, weight) in K.iter().enumerate() {
                    let sx = (cx + k as isize - 2).clamp(0, w as isize - 1) as usize;
                    acc += row[sx] * weight;
                }
                tmp[y * nw + nx] = acc;
            }
        }
        let mut px = vec![0f32; nw * nh];
        for ny in 0..nh {
            let cy = (ny * 2) as isize;
            for (k, weight) in K.iter().enumerate() {
                let sy = (cy + k as isize - 2).clamp(0, h as isize - 1) as usize;
                let src = &tmp[sy * nw..(sy + 1) * nw];
                let dst = &mut px[ny * nw..(ny + 1) * nw];
                for (d, s) in dst.iter_mut().zip(src) {
                    *d += s * weight;
                }
            }
        }
        Level { w: nw, h: nh, px }
    }
}

/// A rectangle of the frame, in whole pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Roi {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

/// An image pyramid over one region of a frame.
struct Pyramid {
    levels: Vec<Level>,
    /// Top-left corner of the region, in frame pixels.
    origin: [f32; 2],
}

impl Pyramid {
    fn build(frame: &Gray<'_>, roi: Roi, levels: usize) -> Pyramid {
        let mut px = Vec::with_capacity(roi.w * roi.h);
        for y in roi.y..roi.y + roi.h {
            let row = &frame.data[y * frame.stride + roi.x..y * frame.stride + roi.x + roi.w];
            px.extend(row.iter().map(|&v| v as f32));
        }
        let mut out = vec![Level { w: roi.w, h: roi.h, px }];
        while out.len() < levels {
            let last = out.last().unwrap();
            if last.w < 16 || last.h < 16 {
                break;
            }
            out.push(last.downsample());
        }
        Pyramid { levels: out, origin: [roi.x as f32, roi.y as f32] }
    }
}

/// Reusable buffers for one tracked window.
#[derive(Default)]
struct Scratch {
    template: Vec<f32>,
    ix: Vec<f32>,
    iy: Vec<f32>,
}

/// Track one point from `prev` to `cur`. Returns its new position in frame pixels.
fn track_point(
    prev: &Pyramid,
    cur: &Pyramid,
    point: [f32; 2],
    cfg: &FlowConfig,
    scratch: &mut Scratch,
) -> Option<[f32; 2]> {
    let r = cfg.window_radius as isize;
    let n = (2 * r + 1) as usize;
    let border = n + 2;
    scratch.template.resize(border * border, 0.0);
    scratch.ix.resize(n * n, 0.0);
    scratch.iy.resize(n * n, 0.0);

    let top = prev.levels.len().min(cur.levels.len()) - 1;
    let mut d = [0f32; 2];
    for level in (0..=top).rev() {
        if level != top {
            d = [d[0] * 2.0, d[1] * 2.0];
        }
        let (lp, lc) = (&prev.levels[level], &cur.levels[level]);
        let scale = 1.0 / (1u32 << level) as f32;
        let x = (point[0] - prev.origin[0]) * scale;
        let y = (point[1] - prev.origin[1]) * scale;
        let reach = (r + 1) as f32;
        if !lp.contains(x - reach, y - reach) || !lp.contains(x + reach, y + reach) {
            // The window does not fit at this level. Coarse levels may be skipped;
            // the finest one may not.
            if level == 0 {
                return None;
            }
            continue;
        }

        // Template with a one-pixel border, then gradients by central differences.
        for j in 0..border {
            for i in 0..border {
                let sx = x + (i as isize - r - 1) as f32;
                let sy = y + (j as isize - r - 1) as f32;
                scratch.template[j * border + i] = lp.sample(sx, sy);
            }
        }
        let (mut gxx, mut gxy, mut gyy) = (0f32, 0f32, 0f32);
        for j in 0..n {
            for i in 0..n {
                let c = (j + 1) * border + i + 1;
                let gx = (scratch.template[c + 1] - scratch.template[c - 1]) * 0.5;
                let gy = (scratch.template[c + border] - scratch.template[c - border]) * 0.5;
                scratch.ix[j * n + i] = gx;
                scratch.iy[j * n + i] = gy;
                gxx += gx * gx;
                gxy += gx * gy;
                gyy += gy * gy;
            }
        }
        let det = gxx * gyy - gxy * gxy;
        let min_eigen = 0.5 * ((gxx + gyy) - ((gxx - gyy).powi(2) + 4.0 * gxy * gxy).sqrt());
        if min_eigen / ((n * n) as f32) < 1.0 || det.abs() < 1e-6 {
            // Too little texture to say where the window moved.
            if level == 0 {
                return None;
            }
            continue;
        }

        for _ in 0..cfg.max_iterations {
            let (cx, cy) = (x + d[0], y + d[1]);
            if !lc.contains(cx - r as f32, cy - r as f32) || !lc.contains(cx + r as f32, cy + r as f32) {
                if level == 0 {
                    return None;
                }
                break;
            }
            let (mut bx, mut by) = (0f32, 0f32);
            for j in 0..n {
                let sy = cy + (j as isize - r) as f32;
                for i in 0..n {
                    let sx = cx + (i as isize - r) as f32;
                    let diff = scratch.template[(j + 1) * border + i + 1] - lc.sample(sx, sy);
                    bx += diff * scratch.ix[j * n + i];
                    by += diff * scratch.iy[j * n + i];
                }
            }
            let dx = (gyy * bx - gxy * by) / det;
            let dy = (gxx * by - gxy * bx) / det;
            d[0] += dx;
            d[1] += dy;
            if dx * dx + dy * dy < 1e-4 {
                break;
            }
        }
    }
    let moved = [point[0] + d[0], point[1] + d[1]];
    (moved[0].is_finite() && moved[1].is_finite()).then_some(moved)
}

/// A small deterministic generator for RANSAC sampling.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Fit a similarity to point pairs, ignoring pairs that disagree with the majority.
///
/// Returns the transform and, for each pair, whether it agrees with it.
fn fit_similarity_ransac(
    src: &[[f32; 2]],
    dst: &[[f32; 2]],
    threshold: f32,
    iterations: usize,
    seed: u64,
) -> Option<(Affine, Vec<bool>)> {
    let n = src.len();
    if n < 3 {
        return None;
    }
    let inliers_of = |m: &Affine| -> Vec<bool> {
        src.iter().zip(dst).map(|(s, d)| distance(m.apply(*s), *d) <= threshold).collect()
    };
    let mut rng = XorShift(seed | 1);
    let mut best: Option<(usize, Vec<bool>)> = None;
    for _ in 0..iterations {
        let (a, b) = (rng.below(n), rng.below(n));
        if a == b {
            continue;
        }
        let Ok(model) = estimate_similarity(&[src[a], src[b]], &[dst[a], dst[b]]) else {
            continue;
        };
        let mask = inliers_of(&model);
        let count = mask.iter().filter(|&&v| v).count();
        if best.as_ref().is_none_or(|(c, _)| count > *c) {
            best = Some((count, mask));
        }
    }
    let (count, mask) = best?;
    if count < 3 {
        return None;
    }
    // Refit on every agreeing pair, then re-mark with the refined transform.
    let pick = |pts: &[[f32; 2]]| -> Vec<[f32; 2]> {
        pts.iter().zip(&mask).filter(|(_, &keep)| keep).map(|(p, _)| *p).collect()
    };
    let refined = estimate_similarity(&pick(src), &pick(dst)).ok()?;
    let mask = inliers_of(&refined);
    Some((refined, mask))
}

/// The result of advancing the tracker by one frame.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackOutcome {
    Tracked {
        /// Maps the previous frame's pixels onto this frame.
        motion: Affine,
        /// The face keypoints on this frame.
        kps: Keypoints,
        /// Points that agreed with the motion, out of those tracked.
        inliers: usize,
        tracked: usize,
    },
    /// The face could not be followed. The caller should run the detector.
    Lost,
}

/// Follows one face between detector runs.
pub struct FlowTracker {
    cfg: FlowConfig,
    prev: Vec<u8>,
    prev_size: (usize, usize),
    kps: Option<Keypoints>,
    support: Vec<[f32; 2]>,
    /// Translation of the last motion, used to widen the working region.
    velocity: [f32; 2],
    frame_index: u64,
    scratch: Scratch,
}

impl FlowTracker {
    pub fn new(cfg: FlowConfig) -> Self {
        Self {
            cfg,
            prev: Vec::new(),
            prev_size: (0, 0),
            kps: None,
            support: Vec::new(),
            velocity: [0.0; 2],
            frame_index: 0,
            scratch: Scratch::default(),
        }
    }

    pub fn reset(&mut self) {
        self.kps = None;
        self.support.clear();
        self.velocity = [0.0; 2];
    }

    pub fn keypoints(&self) -> Option<&Keypoints> {
        self.kps.as_ref()
    }

    pub fn support_points(&self) -> &[[f32; 2]] {
        &self.support
    }

    fn store(&mut self, frame: &Gray<'_>) {
        self.prev.clear();
        for y in 0..frame.height {
            self.prev.extend_from_slice(&frame.data[y * frame.stride..y * frame.stride + frame.width]);
        }
        self.prev_size = (frame.width, frame.height);
    }

    /// The region that holds the given points plus a margin, clipped to the frame.
    fn roi_for(&self, points: &[[f32; 2]], frame: (usize, usize), lead: f32) -> Option<Roi> {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in points {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let pad = self.cfg.roi_padding as f32 + lead;
        let x0 = (x0 - pad).floor().max(0.0) as usize;
        let y0 = (y0 - pad).floor().max(0.0) as usize;
        let x1 = ((x1 + pad).ceil() as usize + 1).min(frame.0);
        let y1 = ((y1 + pad).ceil() as usize + 1).min(frame.1);
        (x1 > x0 + 16 && y1 > y0 + 16).then(|| Roi { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    /// Anchor the tracker on detector keypoints for `frame`.
    ///
    /// Call this on every frame the detector ran on. It replaces the tracked
    /// keypoints, so tracking error does not build up between detections.
    pub fn observe(&mut self, frame: &Gray<'_>, kps: &Keypoints) {
        if !frame.is_valid() {
            self.reset();
            return;
        }
        self.kps = Some(*kps);
        self.support.clear();
        self.velocity = [0.0; 2];
        if let Some(roi) = self.roi_for(kps, (frame.width, frame.height), 0.0) {
            let level = Pyramid::build(frame, roi, 1);
            self.top_up_support(&level.levels[0], level.origin, kps);
        }
        self.store(frame);
    }

    /// Advance to `frame` and report how the face moved since the previous one.
    pub fn track(&mut self, frame: &Gray<'_>) -> TrackOutcome {
        let Some(kps) = self.kps else { return TrackOutcome::Lost };
        if !frame.is_valid() || self.prev_size != (frame.width, frame.height) {
            self.reset();
            return TrackOutcome::Lost;
        }
        self.frame_index += 1;

        let mut points: Vec<[f32; 2]> = kps.to_vec();
        points.extend_from_slice(&self.support);
        let lead = 2.0 * self.velocity[0].abs().max(self.velocity[1].abs());
        let Some(roi) = self.roi_for(&points, self.prev_size, lead) else {
            self.reset();
            return TrackOutcome::Lost;
        };
        let prev_frame =
            Gray { data: &self.prev, width: self.prev_size.0, height: self.prev_size.1, stride: self.prev_size.0 };
        let prev = Pyramid::build(&prev_frame, roi, self.cfg.levels);
        let cur = Pyramid::build(frame, roi, self.cfg.levels);

        // Track every point forward, then back, and keep those that return home.
        let (mut from, mut to, mut index) = (Vec::new(), Vec::new(), Vec::new());
        for (i, p) in points.iter().enumerate() {
            let Some(forward) = track_point(&prev, &cur, *p, &self.cfg, &mut self.scratch) else { continue };
            let Some(back) = track_point(&cur, &prev, forward, &self.cfg, &mut self.scratch) else { continue };
            if distance(back, *p) <= self.cfg.forward_backward_px {
                from.push(*p);
                to.push(forward);
                index.push(i);
            }
        }
        let tracked = from.len();
        let fit = fit_similarity_ransac(
            &from,
            &to,
            self.cfg.ransac_px,
            self.cfg.ransac_iterations,
            self.frame_index.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );
        let Some((mut motion, mask)) = fit else {
            self.reset();
            return TrackOutcome::Lost;
        };
        let inliers = mask.iter().filter(|&&v| v).count();
        if inliers < 4 || (inliers as f32) < self.cfg.min_inlier_ratio * tracked as f32 {
            self.reset();
            return TrackOutcome::Lost;
        }

        // A face at rest should stay exactly where it is.
        let at_rest = kps.iter().all(|k| distance(motion.apply(*k), *k) < self.cfg.rest_px);
        if at_rest {
            motion = Affine::IDENTITY;
        }

        let mut new_kps = kps;
        for k in &mut new_kps {
            *k = motion.apply(*k);
        }
        // Support points keep their own tracked positions, which stay locked to
        // the texture; points that disagreed with the motion are dropped.
        let mut support = Vec::with_capacity(self.support.len());
        for ((i, moved), keep) in index.iter().zip(&to).zip(&mask) {
            if *keep && *i >= 5 {
                support.push(if at_rest { points[*i] } else { *moved });
            }
        }
        self.support = support;
        self.velocity = [motion.0[0][2], motion.0[1][2]];
        self.kps = Some(new_kps);
        if self.support.len() < self.cfg.max_support / 2 {
            self.top_up_support(&cur.levels[0], cur.origin, &new_kps);
        }
        // At rest the earlier frame stays the reference. A drift too slow to
        // show between two frames then adds up against it until it is large
        // enough to follow, instead of being lost one sliver at a time.
        if !at_rest {
            self.store(frame);
        }
        TrackOutcome::Tracked { motion, kps: new_kps, inliers, tracked }
    }

    /// Add well-textured points inside the face region until there are enough.
    fn top_up_support(&mut self, level: &Level, origin: [f32; 2], kps: &Keypoints) {
        const WINDOW: isize = 2; // 5x5 structure-tensor window
        const STEP: usize = 3;
        const MIN_SEPARATION: f32 = 6.0;

        let centre = [kps.iter().map(|k| k[0]).sum::<f32>() / 5.0, kps.iter().map(|k| k[1]).sum::<f32>() / 5.0];
        let iod = distance(kps[0], kps[1]).max(4.0);
        let (rx, ry) = (1.3 * iod, 1.7 * iod);

        let margin = (WINDOW + 1) as usize;
        if level.w <= 2 * margin || level.h <= 2 * margin {
            return;
        }
        let mut candidates: Vec<(f32, [f32; 2])> = Vec::new();
        let mut y = margin;
        while y < level.h - margin {
            let mut x = margin;
            while x < level.w - margin {
                let p = [x as f32 + origin[0], y as f32 + origin[1]];
                let (ex, ey) = ((p[0] - centre[0]) / rx, (p[1] - centre[1]) / ry);
                if ex * ex + ey * ey <= 1.0 {
                    let (mut gxx, mut gxy, mut gyy) = (0f32, 0f32, 0f32);
                    for j in -WINDOW..=WINDOW {
                        for i in -WINDOW..=WINDOW {
                            let c = ((y as isize + j) as usize) * level.w + (x as isize + i) as usize;
                            let gx = (level.px[c + 1] - level.px[c - 1]) * 0.5;
                            let gy = (level.px[c + level.w] - level.px[c - level.w]) * 0.5;
                            gxx += gx * gx;
                            gxy += gx * gy;
                            gyy += gy * gy;
                        }
                    }
                    let min_eigen = 0.5 * ((gxx + gyy) - ((gxx - gyy).powi(2) + 4.0 * gxy * gxy).sqrt());
                    if min_eigen > 25.0 {
                        candidates.push((min_eigen, p));
                    }
                }
                x += STEP;
            }
            y += STEP;
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, p) in candidates {
            if self.support.len() >= self.cfg.max_support {
                break;
            }
            if self.support.iter().all(|s| distance(*s, p) >= MIN_SEPARATION) {
                self.support.push(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::Scene;

    fn max_error(a: &Keypoints, b: &Keypoints) -> f32 {
        a.iter().zip(b).map(|(p, q)| distance(*p, *q)).fold(0.0, f32::max)
    }

    /// Track one step of `motion` on a fresh scene and return the keypoint error.
    fn one_step(motion: Affine, noise: f32) -> (TrackOutcome, f32) {
        let scene = Scene::new(320, 240, 7);
        let kps = scene.keypoints();
        let first = scene.render(&Affine::IDENTITY, 0.0, 1);
        let second = scene.render(&motion, noise, 2);
        let mut tracker = FlowTracker::new(FlowConfig::default());
        tracker.observe(&first.gray(), &kps);
        assert!(tracker.support_points().len() >= 20, "support points: {}", tracker.support_points().len());
        let outcome = tracker.track(&second.gray());
        let mut expected = kps;
        for k in &mut expected {
            *k = motion.apply(*k);
        }
        let error = match &outcome {
            TrackOutcome::Tracked { kps, .. } => max_error(kps, &expected),
            TrackOutcome::Lost => f32::INFINITY,
        };
        (outcome, error)
    }

    #[test]
    fn follows_a_small_shift() {
        let (_, error) = one_step(Affine([[1.0, 0.0, 3.3], [0.0, 1.0, -2.1]]), 0.0);
        assert!(error < 0.05, "error {error}");
    }

    #[test]
    fn follows_a_large_shift_through_the_pyramid() {
        let (_, error) = one_step(Affine([[1.0, 0.0, 18.0], [0.0, 1.0, -11.0]]), 0.0);
        assert!(error < 0.2, "error {error}");
    }

    #[test]
    fn follows_rotation_and_scale() {
        let scene = Scene::new(320, 240, 7);
        let motion = scene.motion_about_face(3.0f32.to_radians(), 1.03, [4.0, -2.0]);
        let (_, error) = one_step(motion, 1.0);
        assert!(error < 0.2, "error {error}");
    }

    #[test]
    fn a_still_face_does_not_drift_under_noise() {
        let scene = Scene::new(320, 240, 11);
        let kps = scene.keypoints();
        let mut tracker = FlowTracker::new(FlowConfig::default());
        tracker.observe(&scene.render(&Affine::IDENTITY, 2.0, 1).gray(), &kps);
        let mut worst = 0.0f32;
        for frame in 2..32 {
            match tracker.track(&scene.render(&Affine::IDENTITY, 2.0, frame).gray()) {
                TrackOutcome::Tracked { kps: now, .. } => worst = worst.max(max_error(&now, &kps)),
                TrackOutcome::Lost => panic!("lost on frame {frame}"),
            }
        }
        assert!(worst < 0.3, "drift {worst}");
    }

    #[test]
    fn ignores_a_region_that_moves_differently() {
        let scene = Scene::new(320, 240, 5);
        let kps = scene.keypoints();
        let motion = Affine([[1.0, 0.0, 5.0], [0.0, 1.0, 2.0]]);
        let first = scene.render(&Affine::IDENTITY, 0.0, 1);
        // The lower part of the face is covered by something moving the other way.
        let second = scene.render_with_occluder(&motion, &Affine([[1.0, 0.0, -9.0], [0.0, 1.0, -6.0]]), 0.0, 2);
        let mut tracker = FlowTracker::new(FlowConfig::default());
        tracker.observe(&first.gray(), &kps);
        let TrackOutcome::Tracked { kps: now, inliers, tracked, .. } = tracker.track(&second.gray()) else {
            panic!("lost");
        };
        let mut expected = kps;
        for k in &mut expected {
            *k = motion.apply(*k);
        }
        assert!(inliers < tracked, "some points should have been rejected");
        assert!(max_error(&now, &expected) < 0.3, "error {}", max_error(&now, &expected));
    }

    #[test]
    fn reports_lost_on_a_blank_frame_and_before_any_detection() {
        let scene = Scene::new(320, 240, 3);
        let mut tracker = FlowTracker::new(FlowConfig::default());
        let first = scene.render(&Affine::IDENTITY, 0.0, 1);
        assert_eq!(tracker.track(&first.gray()), TrackOutcome::Lost);
        tracker.observe(&first.gray(), &scene.keypoints());
        let blank = vec![128u8; 320 * 240];
        let gray = Gray { data: &blank, width: 320, height: 240, stride: 320 };
        assert_eq!(tracker.track(&gray), TrackOutcome::Lost);
        assert!(tracker.keypoints().is_none());
    }

    #[test]
    fn ransac_recovers_the_majority_motion() {
        let motion = Affine([[0.98, -0.05, 6.0], [0.05, 0.98, -3.0]]);
        let mut src = Vec::new();
        let mut dst = Vec::new();
        for i in 0..30 {
            let p = [(i * 37 % 200) as f32, (i * 53 % 160) as f32];
            src.push(p);
            // Every fourth pair is wrong.
            dst.push(if i % 4 == 0 { [p[0] + 40.0, p[1] - 25.0] } else { motion.apply(p) });
        }
        let (fit, mask) = fit_similarity_ransac(&src, &dst, 1.0, 60, 42).unwrap();
        assert_eq!(mask.iter().filter(|&&v| v).count(), 22);
        for i in 0..2 {
            for j in 0..3 {
                assert!((fit.0[i][j] - motion.0[i][j]).abs() < 1e-3);
            }
        }
    }
}
