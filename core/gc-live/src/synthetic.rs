//! A moving-face clip made from one still photo, with known keypoints on
//! every frame. It stands in for webcam recordings in tests and benchmarks:
//! real skin and features for the models, exact ground truth for the tracker.

use gc_align::{Affine, Keypoints};
use gc_infer::image::ImageView;
use gc_infer::Detection;

/// One face cut from a photo, moving over a still background.
pub struct FaceClip {
    /// The photo, packed RGB.
    photo: Vec<u8>,
    photo_size: (u32, u32),
    /// The part of the photo that moves: the face with some margin.
    region: [f32; 4],
    face_centre: [f32; 2],
    kps: Keypoints,
    /// How much larger the face is drawn than it is in the photo.
    zoom: f32,
    pub width: u32,
    pub height: u32,
}

/// How the face moves over time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Motion {
    /// Barely moving, as when listening.
    Still,
    /// Slow drift, turn and lean, as in ordinary talking.
    Talking,
    /// Quick side-to-side movement.
    Fast,
}

impl FaceClip {
    /// `eye_distance` is how far apart the eyes should be in the output, in pixels.
    pub fn new(photo: &ImageView<'_>, face: &Detection, width: u32, height: u32, eye_distance: f32) -> Self {
        let (packed, _) = photo.to_packed();
        let rgb = match photo.format.bytes() {
            3 if photo.format == gc_infer::image::PixelFormat::Rgb => packed,
            _ => {
                let mut out = Vec::with_capacity((photo.width * photo.height * 3) as usize);
                for y in 0..photo.height {
                    for x in 0..photo.width {
                        let c = photo.sample(x as f32, y as f32);
                        out.extend_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8]);
                    }
                }
                out
            }
        };
        let (bw, bh) = (face.bbox[2] - face.bbox[0], face.bbox[3] - face.bbox[1]);
        let centre = [(face.bbox[0] + face.bbox[2]) / 2.0, (face.bbox[1] + face.bbox[3]) / 2.0];
        let eyes = ((face.kps[1][0] - face.kps[0][0]).powi(2) + (face.kps[1][1] - face.kps[0][1]).powi(2)).sqrt();
        Self {
            photo: rgb,
            photo_size: (photo.width, photo.height),
            region: [centre[0] - bw * 0.85, centre[1] - bh * 0.9, centre[0] + bw * 0.85, centre[1] + bh * 0.8],
            face_centre: centre,
            kps: face.kps,
            zoom: eye_distance / eyes.max(1.0),
            width,
            height,
        }
    }

    /// The transform from photo pixels to frame pixels at `seconds`.
    pub fn pose(&self, motion: Motion, seconds: f32) -> Affine {
        let tau = std::f32::consts::TAU;
        let (dx, dy, angle, scale) = match motion {
            Motion::Still => (2.0 * (tau * seconds / 5.0).sin(), 1.5 * (tau * seconds / 7.0).sin(), 0.0, 1.0),
            Motion::Talking => (
                0.10 * self.width as f32 * (tau * seconds / 4.0).sin(),
                0.06 * self.height as f32 * (tau * seconds / 3.1).sin(),
                0.14 * (tau * seconds / 5.0).sin(),
                1.0 + 0.10 * (tau * seconds / 7.0).sin(),
            ),
            Motion::Fast => (
                0.22 * self.width as f32 * (tau * seconds / 1.1).sin(),
                0.05 * self.height as f32 * (tau * seconds / 0.9).sin(),
                0.20 * (tau * seconds / 1.3).sin(),
                1.0 + 0.08 * (tau * seconds / 2.0).sin(),
            ),
        };
        let s = self.zoom * scale;
        let (sin, cos) = angle.sin_cos();
        let target = [self.width as f32 / 2.0 + dx, self.height as f32 * 0.48 + dy];
        // Rotate and scale about the face's centre, then move that centre to the target.
        let (a, b) = (s * cos, -s * sin);
        let (c, d) = (s * sin, s * cos);
        Affine([
            [a, b, target[0] - (a * self.face_centre[0] + b * self.face_centre[1])],
            [c, d, target[1] - (c * self.face_centre[0] + d * self.face_centre[1])],
        ])
    }

    /// The true keypoints for a pose, in frame pixels.
    pub fn keypoints(&self, pose: &Affine) -> Keypoints {
        let mut kps = self.kps;
        for p in &mut kps {
            *p = pose.apply(*p);
        }
        kps
    }

    /// Draw the frame for a pose as BGRA, reusing `out`.
    pub fn render(&self, pose: &Affine, out: &mut Vec<u8>) {
        let (w, h) = (self.width as usize, self.height as usize);
        out.clear();
        out.reserve(w * h * 4);
        // A plain wall with a slow gradient, so the background has no detail to track.
        for y in 0..h {
            let shade = 70 + (y * 40 / h) as u8;
            for _ in 0..w {
                out.extend_from_slice(&[shade, shade, shade.saturating_add(6), 255]);
            }
        }
        let Some(inverse) = pose.invert() else { return };
        let photo =
            ImageView::new(&self.photo, self.photo_size.0, self.photo_size.1, self.photo_size.0 as usize * 3, gc_infer::image::PixelFormat::Rgb);
        let Some(photo) = photo else { return };
        // Only the frame area the moving region covers needs drawing.
        let corners = [
            [self.region[0], self.region[1]],
            [self.region[2], self.region[1]],
            [self.region[0], self.region[3]],
            [self.region[2], self.region[3]],
        ]
        .map(|p| pose.apply(p));
        let min_x = corners.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let min_y = corners.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let max_x = (corners.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(w.saturating_sub(1));
        let max_y = (corners.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(h.saturating_sub(1));
        let m = inverse.0;
        let (rw, rh) = ((self.region[2] - self.region[0]) / 2.0, (self.region[3] - self.region[1]) / 2.0);
        let (rx, ry) = ((self.region[0] + self.region[2]) / 2.0, (self.region[1] + self.region[3]) / 2.0);
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let sx = m[0][0] * x as f32 + m[0][1] * y as f32 + m[0][2];
                let sy = m[1][0] * x as f32 + m[1][1] * y as f32 + m[1][2];
                // An oval cut-out with a soft edge, like a head against the wall.
                let (ex, ey) = ((sx - rx) / rw, (sy - ry) / rh);
                let radius = (ex * ex + ey * ey).sqrt();
                if radius >= 1.0 {
                    continue;
                }
                let alpha = ((1.0 - radius) / 0.08).clamp(0.0, 1.0);
                let colour = photo.sample(sx, sy);
                let p = (y * w + x) * 4;
                for c in 0..3 {
                    // BGRA: blue first.
                    let old = out[p + c] as f32;
                    out[p + c] = (old + (colour[2 - c] - old) * alpha) as u8;
                }
            }
        }
    }
}
