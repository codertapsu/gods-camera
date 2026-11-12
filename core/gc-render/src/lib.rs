//! The picture that goes out: a camera frame with everything drawn on it.
//!
//! The live engine says where the swapped face belongs and how much of the
//! picture to hide. This crate draws that: the frame scaled to fill the
//! output, the blur under the face, the swapped face, a cross-fade to a
//! second picture, and a badge. It then hands the result on in the form a
//! camera driver wants, NV12.
//!
//! Everything here runs on the processor. That is a choice, measured on the
//! Windows PC: one frame at 1280 x 720 takes about a millisecond, and the
//! result is already in ordinary memory, which is where the Windows camera
//! driver needs it. A graphics card would have to draw the frame and then
//! copy it back, while it is also busy with the models. The Mac app has its
//! own Metal renderer, because there frames never leave the graphics memory.

use gc_align::Affine;
use gc_infer::image::{ImageView, PixelFormat};

/// One picture to draw, and whether to flip it left to right.
#[derive(Clone, Copy)]
pub struct Layer<'a> {
    pub image: ImageView<'a>,
    pub mirrored: bool,
}

/// A swapped face to draw onto the first picture.
#[derive(Clone, Copy)]
pub struct Patch<'a> {
    /// A square picture: red, green, blue and the mask as alpha, not premultiplied.
    pub rgba: &'a [u8],
    /// Side of the square, in pixels.
    pub size: u32,
    /// Maps patch pixels onto pixels of the first picture, before it is scaled or flipped.
    pub crop_to_frame: Affine,
    /// How strongly to draw it, 0 to 1.
    pub opacity: f32,
}

/// A small picture drawn over the output, such as the "AI" badge.
#[derive(Clone, Copy)]
pub struct Overlay<'a> {
    /// Red, green, blue, alpha; not premultiplied.
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// Where its top-left corner goes on the output, in pixels.
    pub x: u32,
    pub y: u32,
    pub opacity: f32,
}

/// Everything that makes up one output frame.
#[derive(Clone, Copy)]
pub struct Scene<'a> {
    pub primary: Layer<'a>,
    /// How much of the first picture to replace with a blurred copy, 0 to 1.
    /// The swapped face is drawn on top of the blur, sharp.
    pub blur: f32,
    pub patch: Option<Patch<'a>>,
    /// A second picture and how much of it to show, 0 to 1: the cross-fade
    /// between the live picture and a loop.
    pub secondary: Option<(Layer<'a>, f32)>,
    pub overlay: Option<Overlay<'a>>,
}

impl<'a> Scene<'a> {
    /// A scene that shows one picture and nothing else.
    pub fn of(image: ImageView<'a>) -> Self {
        Self { primary: Layer { image, mirrored: false }, blur: 0.0, patch: None, secondary: None, overlay: None }
    }
}

/// Width of the small copy the blur is made from. A face that fills a third
/// of the picture is about 30 of these pixels wide, and the blur spreads every
/// point over about 20: no feature of it survives.
const BLUR_WIDTH: usize = 96;
/// How far the small copy is blurred, in its own pixels, on each of three passes.
const BLUR_RADIUS: usize = 3;

/// Draws scenes into a picture of a fixed size. Keep one for as long as the
/// output size stays the same: it holds the memory it works in.
pub struct Compositor {
    width: usize,
    height: usize,
    /// The output, four bytes a pixel: blue, green, red, and an unused byte.
    canvas: Vec<u8>,
    /// A second canvas, for the picture a cross-fade goes towards.
    other: Vec<u8>,
    /// The blurred small copy, three numbers a pixel.
    small: Vec<f32>,
    scratch: Vec<f32>,
    /// The small copy's rows stretched to the canvas's width, four bytes a pixel like the canvas.
    wide: Vec<u8>,
    /// What scaling a picture of another size works in.
    stretch: Stretch,
}

/// Working memory for scaling a picture to the canvas's size.
#[derive(Default)]
struct Stretch {
    /// For every canvas column: where in a source row its left neighbour and
    /// its right neighbour start, and how far towards the right one it lies, in 256ths.
    columns: Vec<(u32, u32, u32)>,
    /// Two source rows stretched to the canvas's width, and which rows they are.
    rows: [Vec<u8>; 2],
    held: [Option<usize>; 2],
}

impl Compositor {
    pub fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width.max(2) as usize & !1, height.max(2) as usize & !1);
        Self {
            width,
            height,
            canvas: vec![0; width * height * 4],
            other: Vec::new(),
            small: Vec::new(),
            scratch: Vec::new(),
            wide: Vec::new(),
            stretch: Stretch::default(),
        }
    }

    pub fn width(&self) -> u32 {
        self.width as u32
    }

    pub fn height(&self) -> u32 {
        self.height as u32
    }

    /// The picture drawn last, in the form `draw` returns it. Black before the first scene.
    pub fn canvas(&self) -> &[u8] {
        &self.canvas
    }

    /// Draw a scene. Returns the picture, four bytes a pixel in the order
    /// blue, green, red, unused, row after row without gaps: a window can show it as it is.
    pub fn draw(&mut self, scene: &Scene<'_>) -> &[u8] {
        let (width, height) = (self.width, self.height);
        let to_output = fill(&scene.primary, &mut self.canvas, width, height, &mut self.stretch);

        let blur = scene.blur.clamp(0.0, 1.0);
        if blur > 0.0 {
            self.blur_canvas(blur);
        }

        if let Some(patch) = &scene.patch {
            if patch.opacity > 0.0 {
                // The patch is placed on the first picture; carry it along to where that picture went.
                let placed = to_output.compose(&patch.crop_to_frame);
                paste(&mut self.canvas, width, height, patch, &placed);
            }
        }

        if let Some((layer, mix)) = &scene.secondary {
            let mix = mix.clamp(0.0, 1.0);
            if mix > 0.0 {
                self.other.resize(width * height * 4, 0);
                fill(layer, &mut self.other, width, height, &mut self.stretch);
                blend(&mut self.canvas, &self.other, (mix * 256.0 + 0.5) as u16);
            }
        }

        if let Some(overlay) = &scene.overlay {
            self.draw_overlay(overlay);
        }
        &self.canvas
    }

    /// The picture drawn last, as NV12: one byte of brightness for every
    /// pixel, then one pair of colour bytes for every two-by-two block. The
    /// colours follow BT.709 in video range, the usual form for HD video.
    pub fn nv12(&self, out: &mut Vec<u8>) {
        let (width, height) = (self.width, self.height);
        out.resize(width * height * 3 / 2, 0);
        let (luma, chroma) = out.split_at_mut(width * height);
        for (y, row) in luma.chunks_exact_mut(width).enumerate() {
            let pixels = &self.canvas[y * width * 4..(y + 1) * width * 4];
            for (value, pixel) in row.iter_mut().zip(pixels.as_chunks::<4>().0) {
                let (b, g, r) = (pixel[0] as i32, pixel[1] as i32, pixel[2] as i32);
                *value = ((47 * r + 157 * g + 16 * b + 128) >> 8) as u8 + 16;
            }
        }
        for (y, row) in chroma.chunks_exact_mut(width).enumerate() {
            let upper = &self.canvas[y * 2 * width * 4..(y * 2 + 1) * width * 4];
            let lower = &self.canvas[(y * 2 + 1) * width * 4..(y * 2 + 2) * width * 4];
            for (x, pair) in row.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                // The colour of a block is the colour of its four pixels' average.
                let mut sum = [0i32; 3];
                for pixels in [upper, lower] {
                    let block = &pixels[x * 8..x * 8 + 8];
                    for c in 0..3 {
                        sum[c] += block[c] as i32 + block[4 + c] as i32;
                    }
                }
                let (b, g, r) = (sum[0], sum[1], sum[2]);
                pair[0] = ((-26 * r - 87 * g + 112 * b + 512) >> 10).clamp(-128, 127) as u8 ^ 0x80;
                pair[1] = ((112 * r - 102 * g - 10 * b + 512) >> 10).clamp(-128, 127) as u8 ^ 0x80;
            }
        }
    }

    /// Replace `amount` of the canvas with a copy blurred beyond recognition.
    fn blur_canvas(&mut self, amount: f32) {
        let (width, height) = (self.width, self.height);
        let small_width = BLUR_WIDTH.min(width);
        let small_height = (small_width * height / width).max(2);
        self.small.clear();
        self.small.resize(small_width * small_height * 3, 0.0);
        // Shrink: every small pixel is the average of the block of the canvas
        // it stands for. Every second pixel of every second row is plenty for
        // an average that is about to be blurred.
        for sy in 0..small_height {
            let (y0, y1) = (sy * height / small_height, ((sy + 1) * height / small_height).max(sy * height / small_height + 1));
            for sx in 0..small_width {
                let (x0, x1) = (sx * width / small_width, ((sx + 1) * width / small_width).max(sx * width / small_width + 1));
                let (mut sum, mut count) = ([0u32; 3], 0u32);
                for y in (y0..y1).step_by(2) {
                    let row = &self.canvas[(y * width + x0) * 4..(y * width + x1) * 4];
                    for pixel in row.as_chunks::<4>().0.iter().step_by(2) {
                        sum[0] += pixel[0] as u32;
                        sum[1] += pixel[1] as u32;
                        sum[2] += pixel[2] as u32;
                        count += 1;
                    }
                }
                let at = (sy * small_width + sx) * 3;
                for (value, total) in self.small[at..at + 3].iter_mut().zip(sum) {
                    *value = total as f32 / count.max(1) as f32;
                }
            }
        }
        // Three box blurs come close to a Gaussian one.
        self.scratch.resize(self.small.len(), 0.0);
        for _ in 0..3 {
            box_blur(&self.small, &mut self.scratch, small_width, small_height, BLUR_RADIUS, true);
            box_blur(&self.scratch, &mut self.small, small_width, small_height, BLUR_RADIUS, false);
        }
        // Enlarge in two steps. First every small row is stretched to the
        // canvas's width, sampling between its pixels: few rows, so the slow
        // per-pixel work stays small.
        self.wide.resize(small_height * width * 4, 255);
        let scale = small_width as f32 / width as f32;
        for (sy, row) in self.wide.chunks_exact_mut(width * 4).enumerate() {
            let small_row = &self.small[sy * small_width * 3..(sy + 1) * small_width * 3];
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let fx = ((x as f32 + 0.5) * scale - 0.5).clamp(0.0, small_width as f32 - 1.0);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(small_width - 1);
                for c in 0..3 {
                    let (a, b) = (small_row[x0 * 3 + c], small_row[x1 * 3 + c]);
                    pixel[c] = (a + (b - a) * tx + 0.5) as u8;
                }
            }
        }
        // Then every canvas row is a mix of the two stretched rows around it,
        // mixed in turn with the canvas: whole rows of plain arithmetic.
        let amount = (amount * 256.0 + 0.5) as u16;
        let row_scale = small_height as f32 / height as f32;
        for (y, row) in self.canvas.chunks_exact_mut(width * 4).enumerate() {
            let fy = ((y as f32 + 0.5) * row_scale - 0.5).clamp(0.0, small_height as f32 - 1.0);
            let (y0, ty) = (fy as usize, (fy.fract() * 256.0) as u16);
            let y1 = (y0 + 1).min(small_height - 1);
            let upper = &self.wide[y0 * width * 4..(y0 + 1) * width * 4];
            let lower = &self.wide[y1 * width * 4..(y1 + 1) * width * 4];
            for ((value, a), b) in row.iter_mut().zip(upper).zip(lower) {
                let soft = (*a as u16 * (256 - ty) + *b as u16 * ty) >> 8;
                *value = ((*value as u16 * (256 - amount) + soft * amount + 128) >> 8) as u8;
            }
        }
    }

    fn draw_overlay(&mut self, overlay: &Overlay<'_>) {
        let (ow, oh) = (overlay.width as usize, overlay.height as usize);
        if overlay.rgba.len() < ow * oh * 4 || overlay.opacity <= 0.0 {
            return;
        }
        for oy in 0..oh {
            let y = overlay.y as usize + oy;
            if y >= self.height {
                break;
            }
            for ox in 0..ow {
                let x = overlay.x as usize + ox;
                if x >= self.width {
                    break;
                }
                let texel = &overlay.rgba[(oy * ow + ox) * 4..(oy * ow + ox) * 4 + 4];
                let alpha = texel[3] as f32 / 255.0 * overlay.opacity.min(1.0);
                let pixel = &mut self.canvas[(y * self.width + x) * 4..(y * self.width + x) * 4 + 3];
                // The canvas is blue, green, red; the overlay red, green, blue.
                for c in 0..3 {
                    let colour = texel[2 - c] as f32;
                    pixel[c] = (pixel[c] as f32 + (colour - pixel[c] as f32) * alpha + 0.5) as u8;
                }
            }
        }
    }
}

/// Mix `other` into `canvas`: `weight` of 256 parts. Sixteen-bit arithmetic
/// on plain slices, which the compiler turns into wide instructions.
fn blend(canvas: &mut [u8], other: &[u8], weight: u16) {
    let weight = weight.min(256);
    for (a, b) in canvas.iter_mut().zip(other) {
        *a = ((*a as u16 * (256 - weight) + *b as u16 * weight + 128) >> 8) as u8;
    }
}

/// Draw a swapped face onto the canvas. `placed` maps patch pixels to canvas
/// pixels. The patch is sampled between its pixels, and its alpha is the mask.
fn paste(canvas: &mut [u8], width: usize, height: usize, patch: &Patch<'_>, placed: &Affine) {
    let n = patch.size as usize;
    let Some(to_patch) = placed.invert() else { return };
    if n == 0 || patch.rgba.len() < n * n * 4 {
        return;
    }
    // Only the part of the canvas the patch covers needs visiting.
    let s = patch.size as f32;
    let corners = [[0.0, 0.0], [s, 0.0], [0.0, s], [s, s]].map(|p| placed.apply(p));
    let min_x = corners.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let min_y = corners.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let max_x = (corners.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as i64).min(width as i64 - 1);
    let max_y = (corners.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as i64).min(height as i64 - 1);
    if max_x < min_x as i64 || max_y < min_y as i64 {
        return;
    }
    let opacity = (patch.opacity.clamp(0.0, 1.0) * 256.0 + 0.5) as u32;
    let m = to_patch.0;
    let last = n as i32 - 1;
    // A patch pixel, or nothing where the patch ends.
    let texel = |x: i32, y: i32| -> [u32; 4] {
        if x < 0 || y < 0 || x > last || y > last {
            return [0; 4];
        }
        let p = &patch.rgba[(y as usize * n + x as usize) * 4..];
        [p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32]
    };
    for y in min_y..=max_y as usize {
        let mut px = m[0][0] * min_x as f32 + m[0][1] * y as f32 + m[0][2];
        let mut py = m[1][0] * min_x as f32 + m[1][1] * y as f32 + m[1][2];
        let row = &mut canvas[(y * width + min_x) * 4..(y * width + max_x as usize + 1) * 4];
        for pixel in row.as_chunks_mut::<4>().0 {
            let (x0, y0) = (px.floor() as i32, py.floor() as i32);
            if x0 >= -1 && y0 >= -1 && x0 <= last && y0 <= last {
                let quad = [texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1), texel(x0 + 1, y0 + 1)];
                // Most of the square around a face is outside its mask.
                if quad[0][3] | quad[1][3] | quad[2][3] | quad[3][3] != 0 {
                    let (tx, ty) = (((px - x0 as f32) * 256.0) as u32, ((py - y0 as f32) * 256.0) as u32);
                    let weights = [(256 - tx) * (256 - ty), tx * (256 - ty), (256 - tx) * ty, tx * ty];
                    // Each pixel counts by its weight and by its own alpha, so the
                    // nothing beyond the patch's edge adds no black to the colour.
                    let shares = [quad[0][3] * weights[0], quad[1][3] * weights[1], quad[2][3] * weights[2], quad[3][3] * weights[3]];
                    let covered = shares[0] as u64 + shares[1] as u64 + shares[2] as u64 + shares[3] as u64;
                    let alpha = ((covered >> 16) as u32 * opacity / 255).min(256);
                    if alpha > 0 {
                        for c in 0..3 {
                            let colour = quad[0][c] as u64 * shares[0] as u64
                                + quad[1][c] as u64 * shares[1] as u64
                                + quad[2][c] as u64 * shares[2] as u64
                                + quad[3][c] as u64 * shares[3] as u64;
                            let colour = (colour / covered) as u32;
                            // The canvas is blue, green, red; the patch red, green, blue.
                            let old = pixel[2 - c] as u32;
                            pixel[2 - c] = ((old * (256 - alpha) + colour * alpha + 128) >> 8) as u8;
                        }
                    }
                }
            }
            px += m[0][0];
            py += m[1][0];
        }
    }
}

/// One pass of a box blur over a picture of three numbers a pixel, along its rows or its columns.
fn box_blur(source: &[f32], out: &mut [f32], width: usize, height: usize, radius: usize, along_rows: bool) {
    let (lines, length) = if along_rows { (height, width) } else { (width, height) };
    let span = (2 * radius + 1) as f32;
    for line in 0..lines {
        let index = |i: isize| -> usize {
            let i = i.clamp(0, length as isize - 1) as usize;
            if along_rows {
                (line * width + i) * 3
            } else {
                (i * width + line) * 3
            }
        };
        for c in 0..3 {
            let mut sum: f32 = (-(radius as isize)..=radius as isize).map(|i| source[index(i) + c]).sum();
            for i in 0..length {
                out[index(i as isize) + c] = sum / span;
                sum += source[index(i as isize + radius as isize + 1) + c] - source[index(i as isize - radius as isize) + c];
            }
        }
    }
}

/// Draw `layer` so that it fills a canvas of `width` by `height`, cutting off
/// whichever dimension overflows. Returns the transform from the layer's own
/// pixels to canvas pixels, so that things placed on the layer can follow it.
fn fill(layer: &Layer<'_>, canvas: &mut [u8], width: usize, height: usize, stretch: &mut Stretch) -> Affine {
    let image = &layer.image;
    let (source_width, source_height) = (image.width as usize, image.height as usize);
    let offsets = image.format.rgb_offsets();
    let bytes = image.format.bytes();

    if source_width == width && source_height == height {
        // The common case: the camera already delivers the output size.
        for (y, row) in canvas.chunks_exact_mut(width * 4).enumerate() {
            let source = image.row(y);
            if image.format == PixelFormat::Bgra && !layer.mirrored {
                // And in the canvas's own layout: a straight copy.
                row.copy_from_slice(&source[..width * 4]);
                continue;
            }
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let from = if layer.mirrored { width - 1 - x } else { x } * bytes;
                *pixel = [source[from + offsets[2]], source[from + offsets[1]], source[from + offsets[0]], 255];
            }
        }
        return match layer.mirrored {
            true => Affine([[-1.0, 0.0, width as f32 - 1.0], [0.0, 1.0, 0.0]]),
            false => Affine::IDENTITY,
        };
    }

    // Scale so the picture covers the canvas, and centre what overflows.
    //
    // Each source row that is needed is stretched to the canvas's width once,
    // and every canvas row is a mix of two such rows. Whole numbers
    // throughout: a camera of one size feeding a meeting app that asked for
    // another is an everyday case, and has to cost a few milliseconds, not thirty.
    let scale = (width as f32 / source_width as f32).max(height as f32 / source_height as f32);
    let left = (source_width as f32 - width as f32 / scale) / 2.0;
    let top = (source_height as f32 - height as f32 / scale) / 2.0;
    stretch.columns.clear();
    stretch.columns.extend((0..width).map(|x| {
        let across = if layer.mirrored { width - 1 - x } else { x };
        let sx = (left + (across as f32 + 0.5) / scale - 0.5).clamp(0.0, source_width as f32 - 1.0);
        let x0 = sx as usize;
        let x1 = (x0 + 1).min(source_width - 1);
        ((x0 * bytes) as u32, (x1 * bytes) as u32, (sx.fract() * 256.0) as u32)
    }));
    stretch.held = [None; 2];
    for row in &mut stretch.rows {
        row.resize(width * 4, 255);
    }
    let Stretch { columns, rows, held } = stretch;
    let mut stretched = |source_row: usize, rows: &mut [Vec<u8>; 2]| {
        // Neighbouring rows go to different places, so both stay at hand.
        let place = source_row & 1;
        if held[place] != Some(source_row) {
            held[place] = Some(source_row);
            let source = image.row(source_row);
            for (pixel, &(a, b, towards)) in rows[place].as_chunks_mut::<4>().0.iter_mut().zip(columns.iter()) {
                let (a, b) = (a as usize, b as usize);
                for (c, value) in pixel[..3].iter_mut().enumerate() {
                    let channel = offsets[2 - c];
                    *value = ((source[a + channel] as u32 * (256 - towards) + source[b + channel] as u32 * towards + 128) >> 8) as u8;
                }
            }
        }
        place
    };
    for (y, row) in canvas.chunks_exact_mut(width * 4).enumerate() {
        let sy = (top + (y as f32 + 0.5) / scale - 0.5).clamp(0.0, source_height as f32 - 1.0);
        let (y0, towards) = (sy as usize, (sy.fract() * 256.0) as u16);
        let upper = stretched(y0, rows);
        if towards == 0 {
            row.copy_from_slice(&rows[upper]);
            continue;
        }
        let lower = stretched((y0 + 1).min(source_height - 1), rows);
        let (upper, lower) = (&rows[upper], &rows[lower]);
        for ((value, &a), &b) in row.iter_mut().zip(upper.iter()).zip(lower.iter()) {
            *value = ((a as u16 * (256 - towards) + b as u16 * towards + 128) >> 8) as u8;
        }
    }
    // A point at x on the layer lands at (x + 0.5 - left) * scale - 0.5 on the canvas.
    let shift_x = (0.5 - left) * scale - 0.5;
    let shift_y = (0.5 - top) * scale - 0.5;
    match layer.mirrored {
        true => Affine([[-scale, 0.0, width as f32 - 1.0 - shift_x], [0.0, scale, shift_y]]),
        false => Affine([[scale, 0.0, shift_x], [0.0, scale, shift_y]]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture where every pixel's red is its column and green its row.
    fn ramp(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[x as u8, y as u8, 40]);
            }
        }
        data
    }

    fn rgb(data: &[u8], width: u32, height: u32) -> ImageView<'_> {
        ImageView::new(data, width, height, width as usize * 3, PixelFormat::Rgb).unwrap()
    }

    /// Red, green, blue of a canvas pixel.
    fn at(canvas: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
        let p = &canvas[(y * width + x) * 4..];
        [p[2], p[1], p[0]]
    }

    fn flat(colour: [u8; 3], width: u32, height: u32) -> Vec<u8> {
        colour.repeat((width * height) as usize)
    }

    #[test]
    fn a_picture_of_the_output_size_comes_out_as_it_is() {
        let data = ramp(64, 36);
        let mut compositor = Compositor::new(64, 36);
        let canvas = compositor.draw(&Scene::of(rgb(&data, 64, 36)));
        assert_eq!(at(canvas, 64, 0, 0), [0, 0, 40]);
        assert_eq!(at(canvas, 64, 63, 35), [63, 35, 40]);
        assert_eq!(at(canvas, 64, 10, 20), [10, 20, 40]);
    }

    #[test]
    fn mirroring_flips_left_and_right() {
        let data = ramp(64, 36);
        let mut compositor = Compositor::new(64, 36);
        let mut scene = Scene::of(rgb(&data, 64, 36));
        scene.primary.mirrored = true;
        let canvas = compositor.draw(&scene);
        assert_eq!(at(canvas, 64, 0, 5), [63, 5, 40]);
        assert_eq!(at(canvas, 64, 63, 5), [0, 5, 40]);
    }

    #[test]
    fn a_picture_of_another_shape_fills_the_output_and_is_cut_not_stretched() {
        // A 4:3 picture into a 16:9 output: the full width shows, rows are cut off top and bottom.
        let data = ramp(80, 60);
        let mut compositor = Compositor::new(160, 90);
        let canvas = compositor.draw(&Scene::of(rgb(&data, 80, 60)));
        // Twice as large: the middle of the output shows the middle of the picture.
        let middle = at(canvas, 160, 80, 45);
        assert!((middle[0] as i32 - 40).abs() <= 1 && (middle[1] as i32 - 30).abs() <= 1, "{middle:?}");
        // The top output row shows picture row 7 or 8, not row 0: 15 rows are cut in all.
        let top = at(canvas, 160, 80, 0);
        assert!((6..=8).contains(&top[1]), "{top:?}");
        // Left and right edges of the picture are both there.
        assert!(at(canvas, 160, 0, 45)[0] <= 1);
        assert!(at(canvas, 160, 159, 45)[0] >= 78);
    }

    #[test]
    fn the_swapped_face_lands_where_the_engine_says_also_after_scaling_and_flipping() {
        let grey = flat([100, 100, 100], 80, 45);
        // A red patch of 8 pixels, fully opaque, placed with its corner at (20, 10) on the frame.
        let patch_rgba = [255u8, 0, 0, 255].repeat(8 * 8);
        let patch = Patch { rgba: &patch_rgba, size: 8, crop_to_frame: Affine([[1.0, 0.0, 20.0], [0.0, 1.0, 10.0]]), opacity: 1.0 };

        let mut same = Compositor::new(80, 45);
        let mut scene = Scene::of(rgb(&grey, 80, 45));
        scene.patch = Some(patch);
        let canvas = same.draw(&scene);
        assert_eq!(at(canvas, 80, 23, 13), [255, 0, 0]);
        assert_eq!(at(canvas, 80, 15, 13), [100, 100, 100]);
        assert_eq!(at(canvas, 80, 31, 13), [100, 100, 100]);

        // Twice the size: the patch covers twice the pixels, at twice the place.
        let mut doubled = Compositor::new(160, 90);
        let canvas = doubled.draw(&scene);
        assert_eq!(at(canvas, 160, 48, 28), [255, 0, 0]);
        assert_eq!(at(canvas, 160, 30, 28), [100, 100, 100]);

        // Flipped: the patch is on the other side, at the mirror image of its place.
        scene.primary.mirrored = true;
        let canvas = same.draw(&scene);
        assert_eq!(at(canvas, 80, 79 - 23, 13), [255, 0, 0]);
        assert_eq!(at(canvas, 80, 23, 13), [100, 100, 100]);
    }

    /// How much a canvas varies from place to place: the spread of its green.
    fn detail(canvas: &[u8]) -> f32 {
        let values: Vec<f32> = canvas.as_chunks::<4>().0.iter().step_by(7).map(|p| p[1] as f32).collect();
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len() as f32).sqrt()
    }

    #[test]
    fn the_blur_hides_the_picture_and_the_swapped_face_stays_sharp_on_it() {
        // Light and dark squares 40 pixels wide: detail at the scale of a face's features.
        let (width, height) = (640u32, 360u32);
        let mut board = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let v = if (x / 40 + y / 40) % 2 == 0 { 40 } else { 220 };
                board.extend_from_slice(&[v, v, v]);
            }
        }
        let mut compositor = Compositor::new(width, height);
        let mut scene = Scene::of(rgb(&board, width, height));
        let sharp = detail(compositor.draw(&scene));
        assert!(sharp > 80.0);

        scene.blur = 1.0;
        let canvas = compositor.draw(&scene);
        assert!(detail(canvas) < 8.0, "squares still show: {}", detail(canvas));
        let middle = at(canvas, width as usize, 320, 180);
        assert!((middle[0] as i32 - 130).abs() < 14, "the picture keeps its overall brightness: {middle:?}");

        scene.blur = 0.5;
        let half = detail(compositor.draw(&scene));
        assert!(half > sharp * 0.35 && half < sharp * 0.65, "{half} of {sharp}");

        // The patch goes on top of the blur and is not blurred itself.
        let patch_rgba = [255u8, 0, 0, 255].repeat(64 * 64);
        scene.blur = 1.0;
        scene.patch = Some(Patch { rgba: &patch_rgba, size: 64, crop_to_frame: Affine([[1.0, 0.0, 300.0], [0.0, 1.0, 150.0]]), opacity: 1.0 });
        let canvas = compositor.draw(&scene);
        assert_eq!(at(canvas, width as usize, 332, 182), [255, 0, 0]);
    }

    #[test]
    fn a_cross_fade_mixes_two_pictures_and_a_badge_goes_on_top() {
        let red = flat([200, 0, 0], 32, 18);
        let blue = flat([0, 0, 200], 32, 18);
        let mut compositor = Compositor::new(32, 18);
        let mut scene = Scene::of(rgb(&red, 32, 18));
        scene.secondary = Some((Layer { image: rgb(&blue, 32, 18), mirrored: false }, 0.25));
        let mixed = at(compositor.draw(&scene), 32, 5, 5);
        assert!((mixed[0] as i32 - 150).abs() <= 2 && (mixed[2] as i32 - 50).abs() <= 2, "{mixed:?}");

        // A white badge, half see-through, two pixels square at (4, 3).
        let badge = [255u8, 255, 255, 128].repeat(4);
        scene.secondary = None;
        scene.overlay = Some(Overlay { rgba: &badge, width: 2, height: 2, x: 4, y: 3, opacity: 1.0 });
        let canvas = compositor.draw(&scene);
        let under = at(canvas, 32, 4, 3);
        assert!((under[0] as i32 - 228).abs() <= 2 && (under[1] as i32 - 128).abs() <= 2, "{under:?}");
        assert_eq!(at(canvas, 32, 6, 3), [200, 0, 0]);
    }

    #[test]
    fn nv12_carries_the_colours_in_video_range() {
        let mut out = Vec::new();
        for (colour, expected) in [
            ([255u8, 255, 255], [235u8, 128, 128]),
            ([0, 0, 0], [16, 128, 128]),
            ([255, 0, 0], [63, 102, 240]),
            ([0, 255, 0], [173, 42, 26]),
            ([0, 0, 255], [32, 240, 118]),
        ] {
            let data = flat(colour, 16, 8);
            let mut compositor = Compositor::new(16, 8);
            compositor.draw(&Scene::of(rgb(&data, 16, 8)));
            compositor.nv12(&mut out);
            assert_eq!(out.len(), 16 * 8 * 3 / 2);
            let got = [out[0], out[16 * 8], out[16 * 8 + 1]];
            for (g, e) in got.iter().zip(&expected) {
                assert!((*g as i32 - *e as i32).abs() <= 1, "{colour:?}: {got:?}, expected {expected:?}");
            }
        }
    }

    /// Not a check: prints how long a frame takes on this machine.
    /// `cargo test -p gc-render --release -- --ignored --nocapture speed`
    #[test]
    #[ignore]
    fn speed() {
        for (width, height) in [(1280u32, 720u32), (1920, 1080)] {
            let frame: Vec<u8> = (0..width * height * 4).map(|i| (i * 31 % 251) as u8).collect();
            let view = ImageView::new(&frame, width, height, width as usize * 4, PixelFormat::Bgra).unwrap();
            let patch_rgba = [200u8, 150, 120, 255].repeat(256 * 256);
            let patch = Patch { rgba: &patch_rgba, size: 256, crop_to_frame: Affine([[1.4, 0.0, 400.0], [0.0, 1.4, 150.0]]), opacity: 1.0 };
            let mut compositor = Compositor::new(width, height);
            let mut out = Vec::new();
            let mut time = |label: &str, scene: Scene<'_>| {
                let mut times = Vec::new();
                for _ in 0..60 {
                    let started = std::time::Instant::now();
                    compositor.draw(&scene);
                    let drawn = started.elapsed();
                    compositor.nv12(&mut out);
                    times.push((drawn.as_secs_f64() * 1000.0, started.elapsed().as_secs_f64() * 1000.0));
                }
                times.sort_by(|a, b| a.1.total_cmp(&b.1));
                let (draw, total) = times[times.len() / 2];
                println!("{width} x {height} {label}: draw {draw:.2} ms, with NV12 {total:.2} ms (worst {:.2})", times[times.len() - 1].1);
            };
            let mut scene = Scene::of(view);
            time("plain", scene);
            scene.patch = Some(patch);
            time("with the swapped face", scene);
            scene.blur = 1.0;
            time("blurred, with the face", scene);
            scene.blur = 0.0;
            scene.secondary = Some((Layer { image: view, mirrored: true }, 0.5));
            time("in a cross-fade", scene);
            // A camera of the other size: 1080p shown as 720p, and the reverse.
            let (other_width, other_height) = if width == 1280 { (1920u32, 1080u32) } else { (1280, 720) };
            let other: Vec<u8> = (0..other_width * other_height * 4).map(|i| (i * 31 % 251) as u8).collect();
            let other = ImageView::new(&other, other_width, other_height, other_width as usize * 4, PixelFormat::Bgra).unwrap();
            let mut scene = Scene::of(other);
            scene.patch = Some(patch);
            time(&format!("from {other_width} x {other_height}, with the face"), scene);
        }
    }
}
