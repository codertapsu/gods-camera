//! Small-image work on the CPU: the crops and inputs the models need, the
//! soft mask, and the paste back into a frame.
//!
//! Full frames are composited on the GPU by each app. The functions here are
//! the reference for that work, and they serve still images and tests.

use crate::scrfd::{self, Letterbox};
use crate::swap::SwapperContract;
use gc_align::Affine;

/// How the bytes of one pixel are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra,
    Rgba,
    Bgr,
    Rgb,
}

impl PixelFormat {
    pub fn bytes(self) -> usize {
        match self {
            PixelFormat::Bgra | PixelFormat::Rgba => 4,
            PixelFormat::Bgr | PixelFormat::Rgb => 3,
        }
    }

    /// Byte offsets of red, green and blue within a pixel.
    fn rgb_offsets(self) -> [usize; 3] {
        match self {
            PixelFormat::Bgra | PixelFormat::Bgr => [2, 1, 0],
            PixelFormat::Rgba | PixelFormat::Rgb => [0, 1, 2],
        }
    }
}

/// A borrowed picture.
#[derive(Debug, Clone, Copy)]
pub struct ImageView<'a> {
    data: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// Bytes from the start of one row to the start of the next.
    pub stride: usize,
    pub format: PixelFormat,
}

impl<'a> ImageView<'a> {
    /// Returns `None` if `data` is too short for the stated size.
    pub fn new(data: &'a [u8], width: u32, height: u32, stride: usize, format: PixelFormat) -> Option<Self> {
        let row = width as usize * format.bytes();
        if width == 0 || height == 0 || stride < row {
            return None;
        }
        let needed = stride * (height as usize - 1) + row;
        (data.len() >= needed).then_some(Self { data, width, height, stride, format })
    }

    /// The bytes of row `y`, without the padding after its last pixel.
    #[inline]
    pub fn row(&self, y: usize) -> &'a [u8] {
        let start = y * self.stride;
        &self.data[start..start + self.width as usize * self.format.bytes()]
    }

    /// A copy of the picture with its rows packed together, and the new stride.
    pub fn to_packed(&self) -> (Vec<u8>, usize) {
        let row_bytes = self.width as usize * self.format.bytes();
        let mut out = Vec::with_capacity(row_bytes * self.height as usize);
        for y in 0..self.height as usize {
            out.extend_from_slice(self.row(y));
        }
        (out, row_bytes)
    }

    #[inline]
    fn rgb(&self, x: usize, y: usize) -> [f32; 3] {
        let o = self.format.rgb_offsets();
        let p = y * self.stride + x * self.format.bytes();
        [self.data[p + o[0]] as f32, self.data[p + o[1]] as f32, self.data[p + o[2]] as f32]
    }

    /// The colour at `(x, y)`, blended from the four pixels around it. Pixel
    /// centres are on whole numbers. Anything outside the picture is black.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let (w, h) = (self.width as i64, self.height as i64);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        if x0 < -1 || y0 < -1 || x0 >= w || y0 >= h {
            return [0.0; 3];
        }
        let at = |xi: i64, yi: i64| -> [f32; 3] {
            if xi < 0 || yi < 0 || xi >= w || yi >= h {
                [0.0; 3]
            } else {
                self.rgb(xi as usize, yi as usize)
            }
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
        let mut out = [0.0; 3];
        for i in 0..3 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bottom - top) * fy;
        }
        out
    }

    /// Like `sample`, but points outside take the colour of the nearest edge.
    #[inline]
    fn sample_clamped(&self, x: f32, y: f32) -> [f32; 3] {
        let x = x.clamp(0.0, (self.width - 1) as f32);
        let y = y.clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width as usize - 1), (y0 + 1).min(self.height as usize - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let (a, b, c, d) = (self.rgb(x0, y0), self.rgb(x1, y0), self.rgb(x0, y1), self.rgb(x1, y1));
        let mut out = [0.0; 3];
        for i in 0..3 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bottom - top) * fy;
        }
        out
    }
}

/// A half-size brightness copy of `frame`, for the tracker: each output pixel is
/// the average brightness of a 2 x 2 block. `out` is reused between frames.
/// Returns the width and height of the result.
pub fn half_luma(frame: &ImageView<'_>, out: &mut Vec<u8>) -> (usize, usize) {
    let (width, height) = (frame.width as usize / 2, frame.height as usize / 2);
    out.clear();
    out.resize(width * height, 0);
    let [r, g, b] = frame.format.rgb_offsets();
    let bytes = frame.format.bytes();
    for y in 0..height {
        let top = frame.row(y * 2);
        let bottom = frame.row(y * 2 + 1);
        let line = &mut out[y * width..(y + 1) * width];
        for (x, value) in line.iter_mut().enumerate() {
            let p = x * 2 * bytes;
            let q = p + bytes;
            // Integer weights 77, 150, 29 out of 256, summed over four pixels.
            let red = top[p + r] as u32 + top[q + r] as u32 + bottom[p + r] as u32 + bottom[q + r] as u32;
            let green = top[p + g] as u32 + top[q + g] as u32 + bottom[p + g] as u32 + bottom[q + g] as u32;
            let blue = top[p + b] as u32 + top[q + b] as u32 + bottom[p + b] as u32 + bottom[q + b] as u32;
            *value = ((77 * red + 150 * green + 29 * blue) >> 10) as u8;
        }
    }
    (width, height)
}

/// The square crop of side `size` that `frame_to_crop` maps the frame onto, as
/// packed RGB. Matches OpenCV's `warpAffine` with linear sampling and a black border.
pub fn warp_to_crop(frame: &ImageView<'_>, frame_to_crop: &Affine, size: u32) -> Vec<u8> {
    let size = size as usize;
    let mut out = vec![0u8; size * size * 3];
    let Some(inverse) = frame_to_crop.invert() else { return out };
    let m = inverse.0;
    for y in 0..size {
        // The source position moves by a fixed step for each pixel along a row.
        let mut sx = m[0][1] * y as f32 + m[0][2];
        let mut sy = m[1][1] * y as f32 + m[1][2];
        let row = &mut out[y * size * 3..(y + 1) * size * 3];
        for pixel in row.as_chunks_mut::<3>().0.iter_mut() {
            let c = frame.sample(sx, sy);
            pixel[0] = (c[0] + 0.5) as u8;
            pixel[1] = (c[1] + 0.5) as u8;
            pixel[2] = (c[2] + 0.5) as u8;
            sx += m[0][0];
            sy += m[1][0];
        }
    }
    out
}

/// The detector's input: the frame resized to fit a `canvas` x `canvas` square,
/// placed top-left on black, as planar RGB floats in the detector's range.
pub fn detector_input(frame: &ImageView<'_>, canvas: u32) -> (Vec<f32>, Letterbox) {
    let geometry = scrfd::letterbox_geometry(frame.width, frame.height, canvas);
    let side = canvas as usize;
    let plane = side * side;
    // Black after normalisation.
    let black = (0.0 - scrfd::INPUT_MEAN) * scrfd::INPUT_SCALE;
    let mut out = vec![black; plane * 3];
    let scale_x = frame.width as f32 / geometry.resized_width as f32;
    let scale_y = frame.height as f32 / geometry.resized_height as f32;
    for y in 0..geometry.resized_height as usize {
        let sy = (y as f32 + 0.5) * scale_y - 0.5;
        for x in 0..geometry.resized_width as usize {
            let sx = (x as f32 + 0.5) * scale_x - 0.5;
            let c = frame.sample_clamped(sx, sy);
            let i = y * side + x;
            out[i] = (c[0] - scrfd::INPUT_MEAN) * scrfd::INPUT_SCALE;
            out[plane + i] = (c[1] - scrfd::INPUT_MEAN) * scrfd::INPUT_SCALE;
            out[2 * plane + i] = (c[2] - scrfd::INPUT_MEAN) * scrfd::INPUT_SCALE;
        }
    }
    (out, geometry)
}

/// Planar float input for a model from a packed RGB crop: `(value / 255 - mean) / std`.
pub fn crop_to_planar(crop_rgb: &[u8], mean: f32, std: f32) -> Vec<f32> {
    let plane = crop_rgb.len() / 3;
    let mut out = vec![0f32; plane * 3];
    for (i, pixel) in crop_rgb.as_chunks::<3>().0.iter().enumerate() {
        for c in 0..3 {
            out[c * plane + i] = (pixel[c] as f32 / 255.0 - mean) / std;
        }
    }
    out
}

/// The packed RGB crop a swap model's planar output stands for.
pub fn planar_to_crop(planar: &[f32], contract: &SwapperContract) -> Vec<u8> {
    let plane = planar.len() / 3;
    let mut out = vec![0u8; plane * 3];
    for i in 0..plane {
        for c in 0..3 {
            out[i * 3 + c] = contract.denormalise(planar[c * plane + i]);
        }
    }
    out
}

/// The shape of the soft mask that limits where a swapped crop replaces the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskShape {
    /// The whole crop with softened edges, as InsightFace's own paste does.
    Box,
    /// An oval over the face, which leaves hair, ears and background alone.
    Oval,
}

/// A soft mask in crop space, one byte per pixel, 255 where the swap shows fully.
pub fn crop_mask(size: u32, shape: MaskShape) -> Vec<u8> {
    let n = size as usize;
    let s = size as f32;
    let mut out = vec![0u8; n * n];
    // Smooth step from 0 at `edge0` to 1 at `edge1`.
    let ramp = |edge0: f32, edge1: f32, v: f32| {
        let t = ((v - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let value = match shape {
                MaskShape::Box => {
                    // InsightFace erodes the crop's outline by a tenth of its size
                    // and blurs it by a twentieth. In crop space that is a border
                    // of about 5 % that fades over the next 5 %.
                    let edge = px.min(py).min(s - px).min(s - py);
                    ramp(s * 0.025, s * 0.085, edge)
                }
                MaskShape::Oval => {
                    // Centred a little below the middle, where the aligned face sits.
                    let dx = (px - s * 0.5) / (s * 0.40);
                    let dy = (py - s * 0.52) / (s * 0.47);
                    let radius = (dx * dx + dy * dy).sqrt();
                    1.0 - ramp(0.80, 1.0, radius)
                }
            };
            out[y * n + x] = (value * 255.0 + 0.5) as u8;
        }
    }
    out
}

/// Sample a square single-channel or RGB crop at `(x, y)`; outside is zero.
#[inline]
fn sample_crop<const C: usize>(crop: &[u8], size: usize, x: f32, y: f32) -> [f32; C] {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let n = size as i64;
    let at = |xi: i64, yi: i64, c: usize| -> f32 {
        if xi < 0 || yi < 0 || xi >= n || yi >= n {
            0.0
        } else {
            crop[(yi as usize * size + xi as usize) * C + c] as f32
        }
    };
    let mut out = [0.0; C];
    for (c, value) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) + (at(x0 + 1, y0, c) - at(x0, y0, c)) * fx;
        let bottom = at(x0, y0 + 1, c) + (at(x0 + 1, y0 + 1, c) - at(x0, y0 + 1, c)) * fx;
        *value = top + (bottom - top) * fy;
    }
    out
}

/// A picture that can be written into.
pub struct ImageMut<'a> {
    pub data: &'a mut [u8],
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
}

/// Paste a swapped crop back into the frame it was cut from.
///
/// `crop_rgb` and `mask` are in crop space with side `size`; `frame_to_crop` is
/// the transform the crop was cut with. `opacity` scales the mask, 0 to 1.
pub fn paste_crop(
    frame: &mut ImageMut<'_>,
    crop_rgb: &[u8],
    mask: &[u8],
    size: u32,
    frame_to_crop: &Affine,
    opacity: f32,
) {
    let n = size as usize;
    if crop_rgb.len() < n * n * 3 || mask.len() < n * n {
        return;
    }
    let Some(crop_to_frame) = frame_to_crop.invert() else { return };
    // Only the part of the frame the crop covers needs visiting.
    let s = size as f32;
    let corners = [[0.0, 0.0], [s, 0.0], [0.0, s], [s, s]].map(|p| crop_to_frame.apply(p));
    let min_x = corners.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let min_y = corners.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let max_x = (corners.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as i64).min(frame.width as i64 - 1);
    let max_y = (corners.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as i64).min(frame.height as i64 - 1);
    if max_x < min_x as i64 || max_y < min_y as i64 {
        return;
    }
    let m = frame_to_crop.0;
    let offsets = frame.format.rgb_offsets();
    let bytes = frame.format.bytes();
    for y in min_y..=max_y as usize {
        let mut cx = m[0][0] * min_x as f32 + m[0][1] * y as f32 + m[0][2];
        let mut cy = m[1][0] * min_x as f32 + m[1][1] * y as f32 + m[1][2];
        for x in min_x..=max_x as usize {
            let [alpha] = sample_crop::<1>(mask, n, cx, cy);
            let alpha = alpha / 255.0 * opacity;
            if alpha > 0.002 {
                let colour = sample_crop::<3>(crop_rgb, n, cx, cy);
                let p = y * frame.stride + x * bytes;
                for c in 0..3 {
                    let old = frame.data[p + offsets[c]] as f32;
                    frame.data[p + offsets[c]] = (old + (colour[c] - old) * alpha + 0.5) as u8;
                }
            }
            cx += m[0][0];
            cy += m[1][0];
        }
    }
}

/// Draw a patch whose mask is its alpha channel onto a frame. This is what
/// each app's GPU compositor does; here it serves stills, tests and benchmarks.
///
/// `patch_rgba` is square with side `size`, not premultiplied. `crop_to_frame`
/// maps patch pixels onto the frame.
pub fn paste_patch(frame: &mut ImageMut<'_>, patch_rgba: &[u8], size: u32, crop_to_frame: &Affine, opacity: f32) {
    let n = size as usize;
    let Some(frame_to_crop) = crop_to_frame.invert() else { return };
    if patch_rgba.len() < n * n * 4 || opacity <= 0.0 {
        return;
    }
    let s = size as f32;
    let corners = [[0.0, 0.0], [s, 0.0], [0.0, s], [s, s]].map(|p| crop_to_frame.apply(p));
    let min_x = corners.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let min_y = corners.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let max_x = (corners.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as i64).min(frame.width as i64 - 1);
    let max_y = (corners.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as i64).min(frame.height as i64 - 1);
    if max_x < min_x as i64 || max_y < min_y as i64 {
        return;
    }
    let m = frame_to_crop.0;
    let offsets = frame.format.rgb_offsets();
    let bytes = frame.format.bytes();
    for y in min_y..=max_y as usize {
        let mut cx = m[0][0] * min_x as f32 + m[0][1] * y as f32 + m[0][2];
        let mut cy = m[1][0] * min_x as f32 + m[1][1] * y as f32 + m[1][2];
        for x in min_x..=max_x as usize {
            let texel = sample_crop::<4>(patch_rgba, n, cx, cy);
            let alpha = texel[3] / 255.0 * opacity;
            if alpha > 0.002 {
                let p = y * frame.stride + x * bytes;
                for c in 0..3 {
                    let old = frame.data[p + offsets[c]] as f32;
                    // Sampling blends colour with the zero outside the patch, so divide the edge back out.
                    let colour = if texel[3] > 0.0 { texel[c] } else { old };
                    frame.data[p + offsets[c]] = (old + (colour - old) * alpha + 0.5) as u8;
                }
            }
            cx += m[0][0];
            cy += m[1][0];
        }
    }
}

/// Shift the crop's colours so its masked area has the same mean and spread as
/// the original crop's. This hides lighting differences between the two faces.
pub fn match_colour(swapped_rgb: &mut [u8], original_rgb: &[u8], mask: &[u8]) {
    let pixels = mask.len().min(swapped_rgb.len() / 3).min(original_rgb.len() / 3);
    let mut weight = 0f64;
    let (mut sum_s, mut sum_o) = ([0f64; 3], [0f64; 3]);
    let (mut sq_s, mut sq_o) = ([0f64; 3], [0f64; 3]);
    for i in 0..pixels {
        let w = mask[i] as f64 / 255.0;
        if w == 0.0 {
            continue;
        }
        weight += w;
        for c in 0..3 {
            let (s, o) = (swapped_rgb[i * 3 + c] as f64, original_rgb[i * 3 + c] as f64);
            sum_s[c] += w * s;
            sum_o[c] += w * o;
            sq_s[c] += w * s * s;
            sq_o[c] += w * o * o;
        }
    }
    if weight < 1.0 {
        return;
    }
    for c in 0..3 {
        let (mean_s, mean_o) = (sum_s[c] / weight, sum_o[c] / weight);
        let std_s = (sq_s[c] / weight - mean_s * mean_s).max(1.0).sqrt();
        let std_o = (sq_o[c] / weight - mean_o * mean_o).max(1.0).sqrt();
        // A gentle correction: extreme gains make noise visible.
        let gain = (std_o / std_s).clamp(0.6, 1.6);
        for i in 0..pixels {
            let v = (swapped_rgb[i * 3 + c] as f64 - mean_s) * gain + mean_o;
            swapped_rgb[i * 3 + c] = v.clamp(0.0, 255.0) as u8;
        }
    }
}

/// Make a crop crisper. Each pixel moves away from the average of its
/// neighbours by `amount` times the difference; 0 changes nothing.
///
/// A swap model's output is small, 128 or 256 pixels across, and is enlarged
/// onto the frame. A little of this restores the look of fine detail.
pub fn sharpen(rgb: &mut [u8], size: u32, amount: f32) {
    let n = size as usize;
    if amount <= 0.0 || n < 3 || rgb.len() != n * n * 3 {
        return;
    }
    // A 3 x 3 blur with weights 1 2 1, first along rows, then along columns.
    let mut rows = vec![0u16; rgb.len()];
    for y in 0..n {
        for x in 0..n {
            let (left, right) = (x.saturating_sub(1), (x + 1).min(n - 1));
            for c in 0..3 {
                let at = |xi: usize| rgb[(y * n + xi) * 3 + c] as u16;
                rows[(y * n + x) * 3 + c] = at(left) + 2 * at(x) + at(right);
            }
        }
    }
    let original = rgb.to_vec();
    for y in 0..n {
        let (up, down) = (y.saturating_sub(1), (y + 1).min(n - 1));
        for x in 0..n {
            for c in 0..3 {
                let at = |yi: usize| rows[(yi * n + x) * 3 + c] as f32;
                let blurred = (at(up) + 2.0 * at(y) + at(down)) / 16.0;
                let value = original[(y * n + x) * 3 + c] as f32;
                rgb[(y * n + x) * 3 + c] = (value + amount * (value - blurred)).clamp(0.0, 255.0).round() as u8;
            }
        }
    }
}

/// Open an oval hole in a crop mask, so the frame shows through there.
/// The hole is fully open out to `radii` around `centre`, and closes over a
/// further `feather` share of the radii.
pub fn cut_oval(mask: &mut [u8], size: u32, centre: [f32; 2], radii: [f32; 2], feather: f32) {
    let n = size as usize;
    if mask.len() != n * n || radii[0] <= 0.0 || radii[1] <= 0.0 {
        return;
    }
    let feather = feather.max(1e-3);
    for y in 0..n {
        for x in 0..n {
            let dx = (x as f32 + 0.5 - centre[0]) / radii[0];
            let dy = (y as f32 + 0.5 - centre[1]) / radii[1];
            let t = (((dx * dx + dy * dy).sqrt() - 1.0) / feather).clamp(0.0, 1.0);
            let keep = t * t * (3.0 - 2.0 * t);
            let value = &mut mask[y * n + x];
            *value = (*value as f32 * keep + 0.5) as u8;
        }
    }
}

/// Where the mouth is in an aligned crop of side `size`: the centre and the
/// radii of an oval that covers the lips, closed or open, with a small margin.
pub fn mouth_oval(size: u32) -> Option<([f32; 2], [f32; 2])> {
    let template = gc_align::template(size).ok()?;
    let (left, right) = (template[3], template[4]);
    let width = ((right[0] - left[0]).powi(2) + (right[1] - left[1]).powi(2)).sqrt();
    // An open mouth grows downwards from the line between its corners.
    let centre = [(left[0] + right[0]) / 2.0, (left[1] + right[1]) / 2.0 + 0.08 * width];
    Some((centre, [0.75 * width, 0.5 * width]))
}

/// Resize a square packed RGB crop from side `from` to side `to`, sampling
/// between the nearest four pixels.
pub fn resize_crop(crop_rgb: &[u8], from: u32, to: u32) -> Vec<u8> {
    let (n, m) = (from as usize, to as usize);
    let mut out = Vec::with_capacity(m * m * 3);
    let scale = n as f32 / m as f32;
    for y in 0..m {
        let sy = ((y as f32 + 0.5) * scale - 0.5).clamp(0.0, n as f32 - 1.0);
        for x in 0..m {
            let sx = ((x as f32 + 0.5) * scale - 0.5).clamp(0.0, n as f32 - 1.0);
            // Positions are kept inside the crop, so the zero beyond its edge never has weight.
            let pixel: [f32; 3] = sample_crop(crop_rgb, n, sx, sy);
            out.extend(pixel.iter().map(|v| v.round().clamp(0.0, 255.0) as u8));
        }
    }
    out
}

/// Resize a square map of numbers from side `from` to side `to`. Shrinking by
/// a whole factor averages each block; anything else samples between pixels.
pub fn resize_map(map: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to || map.len() != from * from || to == 0 {
        return map.to_vec();
    }
    let mut out = Vec::with_capacity(to * to);
    if from.is_multiple_of(to) {
        let k = from / to;
        for y in 0..to {
            for x in 0..to {
                let mut sum = 0.0;
                for by in 0..k {
                    for bx in 0..k {
                        sum += map[(y * k + by) * from + x * k + bx];
                    }
                }
                out.push(sum / (k * k) as f32);
            }
        }
        return out;
    }
    let scale = from as f32 / to as f32;
    let last = from - 1;
    for y in 0..to {
        let sy = ((y as f32 + 0.5) * scale - 0.5).clamp(0.0, last as f32);
        let (y0, fy) = (sy.floor() as usize, sy.fract());
        let y1 = (y0 + 1).min(last);
        for x in 0..to {
            let sx = ((x as f32 + 0.5) * scale - 0.5).clamp(0.0, last as f32);
            let (x0, fx) = (sx.floor() as usize, sx.fract());
            let x1 = (x0 + 1).min(last);
            let top = map[y0 * from + x0] + (map[y0 * from + x1] - map[y0 * from + x0]) * fx;
            let bottom = map[y1 * from + x0] + (map[y1 * from + x1] - map[y1 * from + x0]) * fx;
            out.push(top + (bottom - top) * fy);
        }
    }
    out
}

/// One pass of a box blur along rows or columns of a square map, with the
/// edge value repeated beyond the edge.
fn box_blur_pass(source: &[f32], out: &mut [f32], n: usize, radius: usize, columns: bool) {
    let width = (2 * radius + 1) as f32;
    for line in 0..n {
        let at = |i: isize| -> f32 {
            let i = i.clamp(0, n as isize - 1) as usize;
            if columns {
                source[i * n + line]
            } else {
                source[line * n + i]
            }
        };
        let mut sum: f32 = (-(radius as isize)..=radius as isize).map(at).sum();
        for i in 0..n {
            let index = if columns { i * n + line } else { line * n + i };
            out[index] = sum / width;
            sum += at(i as isize + radius as isize + 1) - at(i as isize - radius as isize);
        }
    }
}

/// Turn a hard face map from a segmentation model, 0 to 1 per pixel, into a
/// mask with a soft rim that lies wholly inside the face.
///
/// The map is blurred by about `sigma` pixels and its upper half is stretched
/// to the full range: the rim then starts at the old outline and reaches full
/// strength a little inside it, so nothing outside the face is ever replaced.
pub fn soften_mask(map: &mut [f32], size: usize, sigma: f32) {
    if map.len() != size * size || size == 0 {
        return;
    }
    // Three box blurs come close to a Gaussian blur.
    let radius = (((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0).round() as usize;
    if radius > 0 {
        let mut scratch = vec![0f32; map.len()];
        for _ in 0..3 {
            box_blur_pass(map, &mut scratch, size, radius, false);
            box_blur_pass(&scratch, map, size, radius, true);
        }
    }
    for value in map.iter_mut() {
        *value = ((value.clamp(0.5, 1.0)) - 0.5) * 2.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[(x * 2) as u8, (y * 3) as u8, 50]);
            }
        }
        data
    }

    #[test]
    fn view_rejects_short_data() {
        let data = vec![0u8; 10];
        assert!(ImageView::new(&data, 4, 4, 12, PixelFormat::Rgb).is_none());
        assert!(ImageView::new(&data, 0, 4, 12, PixelFormat::Rgb).is_none());
    }

    #[test]
    fn half_luma_averages_blocks() {
        // A 4 x 2 picture: left block white, right block pure green.
        let mut data = Vec::new();
        for _ in 0..2 {
            data.extend_from_slice(&[255, 255, 255, 255, 255, 255, 0, 255, 0, 0, 255, 0]);
        }
        let view = ImageView::new(&data, 4, 2, 12, PixelFormat::Rgb).unwrap();
        let mut out = Vec::new();
        assert_eq!(half_luma(&view, &mut out), (2, 1));
        assert_eq!(out[0], 255);
        assert_eq!(out[1], 149); // 150 / 256 of full green
    }

    #[test]
    fn identity_warp_copies_the_corner() {
        let data = gradient(64, 48);
        let view = ImageView::new(&data, 64, 48, 64 * 3, PixelFormat::Rgb).unwrap();
        let crop = warp_to_crop(&view, &Affine::IDENTITY, 16);
        assert_eq!(&crop[..3], &[0, 0, 50]);
        // Pixel (5, 7) of the crop is pixel (5, 7) of the frame.
        assert_eq!(&crop[(7 * 16 + 5) * 3..(7 * 16 + 5) * 3 + 3], &[10, 21, 50]);
    }

    #[test]
    fn warp_outside_the_frame_is_black() {
        let data = gradient(8, 8);
        let view = ImageView::new(&data, 8, 8, 24, PixelFormat::Rgb).unwrap();
        // Shift the frame 100 pixels right in crop space: the crop sees nothing.
        let shift = Affine([[1.0, 0.0, 100.0], [0.0, 1.0, 0.0]]);
        assert!(warp_to_crop(&view, &shift, 8).iter().all(|&v| v == 0));
    }

    #[test]
    fn bgra_and_rgb_views_agree() {
        let rgb = gradient(8, 8);
        let bgra: Vec<u8> = rgb.as_chunks::<3>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        let a = ImageView::new(&rgb, 8, 8, 24, PixelFormat::Rgb).unwrap();
        let b = ImageView::new(&bgra, 8, 8, 32, PixelFormat::Bgra).unwrap();
        assert_eq!(a.sample(3.25, 4.5), b.sample(3.25, 4.5));
    }

    #[test]
    fn detector_input_places_the_frame_top_left() {
        let data = vec![255u8; 32 * 16 * 3];
        let view = ImageView::new(&data, 32, 16, 96, PixelFormat::Rgb).unwrap();
        let (input, geometry) = detector_input(&view, 64);
        assert_eq!((geometry.resized_width, geometry.resized_height), (64, 32));
        let white = (255.0 - scrfd::INPUT_MEAN) * scrfd::INPUT_SCALE;
        let black = -scrfd::INPUT_MEAN * scrfd::INPUT_SCALE;
        assert!((input[0] - white).abs() < 1e-6);
        assert!((input[31 * 64 + 63] - white).abs() < 1e-6);
        // Below the resized frame the canvas stays black.
        assert!((input[32 * 64] - black).abs() < 1e-6);
        assert_eq!(input.len(), 64 * 64 * 3);
    }

    #[test]
    fn planar_round_trip() {
        let crop: Vec<u8> = (0..4 * 4 * 3).map(|v| (v * 5) as u8).collect();
        for contract in [SwapperContract::INSWAPPER_128, SwapperContract::HYPERSWAP_256] {
            let planar = crop_to_planar(&crop, contract.mean, contract.std);
            let back = planar_to_crop(&planar, &contract);
            for (a, b) in crop.iter().zip(&back) {
                assert!((*a as i32 - *b as i32).abs() <= 1);
            }
        }
    }

    #[test]
    fn masks_are_full_inside_and_empty_at_the_corners() {
        for shape in [MaskShape::Box, MaskShape::Oval] {
            let mask = crop_mask(128, shape);
            assert_eq!(mask[64 * 128 + 64], 255, "{shape:?} centre");
            assert_eq!(mask[0], 0, "{shape:?} corner");
        }
        // The oval leaves the crop's edges alone; the box does not reach them either.
        let oval = crop_mask(128, MaskShape::Oval);
        assert_eq!(oval[64 * 128 + 2], 0);
    }

    #[test]
    fn paste_replaces_only_the_masked_area() {
        let mut data = vec![10u8; 64 * 64 * 4];
        let crop = vec![200u8; 16 * 16 * 3];
        let mask = crop_mask(16, MaskShape::Box);
        // The crop covers frame pixels 20..36 in both directions.
        let frame_to_crop = Affine([[1.0, 0.0, -20.0], [0.0, 1.0, -20.0]]);
        let mut frame =
            ImageMut { data: &mut data, width: 64, height: 64, stride: 256, format: PixelFormat::Bgra };
        paste_crop(&mut frame, &crop, &mask, 16, &frame_to_crop, 1.0);
        let pixel = |x: usize, y: usize| data[y * 256 + x * 4];
        assert_eq!(pixel(28, 28), 200);
        assert_eq!(pixel(5, 5), 10);
        assert_eq!(pixel(50, 28), 10);
        // Alpha is untouched.
        assert_eq!(data[28 * 256 + 28 * 4 + 3], 10);
    }

    #[test]
    fn colour_match_moves_the_mean() {
        let original = vec![100u8; 8 * 8 * 3];
        let mut swapped = vec![160u8; 8 * 8 * 3];
        let mask = vec![255u8; 8 * 8];
        match_colour(&mut swapped, &original, &mask);
        assert!(swapped.iter().all(|&v| (v as i32 - 100).abs() <= 1));
    }

    #[test]
    fn sharpening_steepens_an_edge_and_leaves_flat_areas_alone() {
        // Left half dark, right half light.
        let n = 16usize;
        let mut crop = Vec::new();
        for _ in 0..n {
            for x in 0..n {
                let v = if x < n / 2 { 80 } else { 160 };
                crop.extend_from_slice(&[v, v, v]);
            }
        }
        let before = crop.clone();
        sharpen(&mut crop, n as u32, 0.0);
        assert_eq!(crop, before, "an amount of 0 changes nothing");
        sharpen(&mut crop, n as u32, 1.0);
        let at = |x: usize| crop[(8 * n + x) * 3];
        assert_eq!((at(2), at(13)), (80, 160), "flat areas keep their value");
        assert!(at(7) < 80 && at(8) > 160, "the two sides of the edge move apart: {} and {}", at(7), at(8));
    }

    #[test]
    fn an_oval_hole_opens_the_mask_where_the_mouth_is() {
        let size = 128u32;
        let (centre, radii) = mouth_oval(size).expect("a template for 128");
        // Between the mouth corners of the aligned face, a little below their line.
        assert!((centre[0] - 64.0).abs() < 1.5 && (88.0..100.0).contains(&centre[1]), "{centre:?}");
        let mut mask = crop_mask(size, MaskShape::Oval);
        let n = size as usize;
        let eye = mask[52 * n + 46];
        cut_oval(&mut mask, size, centre, radii, 0.35);
        assert_eq!(mask[centre[1] as usize * n + centre[0] as usize], 0, "the mouth shows the frame");
        assert_eq!(mask[52 * n + 46], eye, "the eyes are untouched");
        assert_eq!(mask[60 * n + 64], 255, "so is the nose");
    }

    #[test]
    fn maps_resize_by_averaging_blocks_or_sampling_between_pixels() {
        let map: Vec<f32> = (0..16).map(|i| i as f32).collect();
        // 4 x 4 to 2 x 2: each value is the mean of a 2 x 2 block.
        assert_eq!(resize_map(&map, 4, 2), vec![2.5, 4.5, 10.5, 12.5]);
        // Enlarging a flat map keeps it flat.
        assert!(resize_map(&[0.25; 9], 3, 7).iter().all(|v| (*v - 0.25).abs() < 1e-6));
        // A crop of one colour stays that colour at any size.
        let flat = [10u8, 200, 30].repeat(8 * 8);
        let bigger = resize_crop(&flat, 8, 20);
        assert_eq!(bigger.len(), 20 * 20 * 3);
        assert!(bigger.as_chunks::<3>().0.iter().all(|p| *p == [10, 200, 30]));
    }

    #[test]
    fn a_softened_mask_stays_inside_the_face() {
        // A hard square face map in the middle of a 64 x 64 crop.
        let n = 64usize;
        let mut map = vec![0f32; n * n];
        for y in 16..48 {
            for x in 16..48 {
                map[y * n + x] = 1.0;
            }
        }
        soften_mask(&mut map, n, 2.0);
        assert_eq!(map[32 * n + 10], 0.0, "nothing outside the outline");
        assert!(map[32 * n + 16] < 0.2, "the rim starts at the outline: {}", map[32 * n + 16]);
        assert!(map[32 * n + 32] > 0.99, "full strength inside");
        let rim = map[32 * n + 19];
        assert!(rim > 0.2 && rim < 1.0, "a soft rim in between: {rim}");
    }
}
