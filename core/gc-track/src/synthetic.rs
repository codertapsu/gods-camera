//! Synthetic scenes with exactly known motion, for tests and benchmarks.
//!
//! A scene is a fixed texture. A frame shows that texture after a known
//! transform, so the true position of every point is known on every frame.
//! Real footage tests robustness; these scenes test accuracy.

use crate::flow::Gray;
use gc_align::{Affine, Keypoints, ARCFACE_112};

/// An owned 8-bit luma frame.
pub struct Frame {
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

impl Frame {
    pub fn gray(&self) -> Gray<'_> {
        Gray { data: &self.data, width: self.width, height: self.height, stride: self.width }
    }
}

pub struct Scene {
    pub width: usize,
    pub height: usize,
    seed: u32,
}

fn hash(seed: u32, x: i32, y: i32) -> f32 {
    let mut h = seed ^ (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Smooth random values on a lattice with the given cell size, in 0..1.
fn value_noise(seed: u32, x: f32, y: f32, cell: f32) -> f32 {
    let (u, v) = (x / cell, y / cell);
    let (x0, y0) = (u.floor(), v.floor());
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (fx, fy) = (smooth(u - x0), smooth(v - y0));
    let (xi, yi) = (x0 as i32, y0 as i32);
    let top = hash(seed, xi, yi) + (hash(seed, xi + 1, yi) - hash(seed, xi, yi)) * fx;
    let bottom = hash(seed, xi, yi + 1) + (hash(seed, xi + 1, yi + 1) - hash(seed, xi, yi + 1)) * fx;
    top + (bottom - top) * fy
}

impl Scene {
    pub fn new(width: usize, height: usize, seed: u32) -> Self {
        Self { width, height, seed }
    }

    /// The texture at a scene position, in 8-bit levels.
    fn texture(&self, x: f32, y: f32) -> f32 {
        let v = 0.5 * value_noise(self.seed, x, y, 24.0)
            + 0.3 * value_noise(self.seed ^ 0xA5A5, x, y, 9.0)
            + 0.2 * value_noise(self.seed ^ 0x5A5A, x, y, 4.0);
        20.0 + 215.0 * v
    }

    fn face_centre(&self) -> [f32; 2] {
        [self.width as f32 * 0.5, self.height as f32 * 0.5]
    }

    /// Distance between the eyes of the scene's face, in pixels.
    pub fn eye_distance(&self) -> f32 {
        self.width as f32 * 0.14
    }

    /// Five face keypoints in the middle of the scene, laid out like a real face.
    pub fn keypoints(&self) -> Keypoints {
        let scale = self.eye_distance() / (ARCFACE_112[1][0] - ARCFACE_112[0][0]);
        let centre = self.face_centre();
        let mut kps = ARCFACE_112;
        for k in &mut kps {
            k[0] = (k[0] - 56.0) * scale + centre[0];
            k[1] = (k[1] - 72.0) * scale + centre[1];
        }
        kps
    }

    /// A rotation and uniform scale about the face centre, followed by a shift.
    pub fn motion_about_face(&self, angle: f32, scale: f32, shift: [f32; 2]) -> Affine {
        let c = self.face_centre();
        let (a, b) = (scale * angle.cos(), scale * angle.sin());
        Affine([
            [a, -b, c[0] - (a * c[0] - b * c[1]) + shift[0]],
            [b, a, c[1] - (b * c[0] + a * c[1]) + shift[1]],
        ])
    }

    /// The scene after `motion`, with uniform noise of up to `noise` levels.
    pub fn render(&self, motion: &Affine, noise: f32, frame: u32) -> Frame {
        self.render_inner(motion, None, noise, frame)
    }

    /// Like `render`, but the lower part of the face follows `occluder` instead,
    /// as if something moving on its own covered it.
    pub fn render_with_occluder(&self, motion: &Affine, occluder: &Affine, noise: f32, frame: u32) -> Frame {
        self.render_inner(motion, Some(occluder), noise, frame)
    }

    fn render_inner(&self, motion: &Affine, occluder: Option<&Affine>, noise: f32, frame: u32) -> Frame {
        let inverse = motion.invert().expect("motion must be invertible");
        let occluder_inverse = occluder.map(|o| o.invert().expect("occluder must be invertible"));
        let centre = self.face_centre();
        let iod = self.eye_distance();
        let mut data = Vec::with_capacity(self.width * self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let p = [x as f32, y as f32];
                let covered = (p[0] - centre[0]).abs() < 1.4 * iod
                    && p[1] > centre[1] + 0.55 * iod
                    && p[1] < centre[1] + 2.0 * iod;
                let source = match (&occluder_inverse, covered) {
                    (Some(inv), true) => inv.apply(p),
                    _ => inverse.apply(p),
                };
                let jitter = (hash(frame.wrapping_mul(0x9E37_79B9), x as i32, y as i32) * 2.0 - 1.0) * noise;
                data.push((self.texture(source[0], source[1]) + jitter).clamp(0.0, 255.0).round() as u8);
            }
        }
        Frame { data, width: self.width, height: self.height }
    }
}
