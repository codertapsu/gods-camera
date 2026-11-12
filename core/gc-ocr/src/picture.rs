//! Pictures in memory, and the cutting, levelling and resizing the reader needs.

use std::path::Path;

/// A picture: red, green and blue for each pixel, row after row from the top.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

/// A picture file larger than this on a side is not opened: it is no photo of
/// a page, and decoding it could take all the memory there is.
pub const LONGEST_SIDE: u32 = 12_000;

/// How one pixel is weighed against its neighbours between two pixels, by the
/// usual cubic curve (the one OpenCV uses, so that the Python reference in
/// tools/ocrkit and this code see the same line).
fn cubic(t: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let near = |x: f32| ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let far = |x: f32| ((A * x - 5.0 * A) * x + 8.0 * A) * x - 4.0 * A;
    [far(t + 1.0), near(t), near(1.0 - t), far(2.0 - t)]
}

impl Picture {
    pub fn new(width: usize, height: usize, rgb: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && rgb.len() == width * height * 3).then_some(Self { width, height, rgb })
    }

    /// Read a picture file, turned the way it was taken. JPEG, PNG, WebP,
    /// TIFF and BMP are understood.
    pub fn open(path: &Path) -> Result<Self, String> {
        use image::ImageDecoder;
        let mut reader = image::ImageReader::open(path).map_err(|error| error.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(LONGEST_SIDE);
        limits.max_image_height = Some(LONGEST_SIDE);
        reader.limits(limits);
        let mut decoder =
            reader.with_guessed_format().map_err(|error| error.to_string())?.into_decoder().map_err(|error| error.to_string())?;
        // A phone stores a photo as the sensor saw it, and says beside it which way up it was held.
        let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut picture = image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
        picture.apply_orientation(orientation);
        let rgb = picture.to_rgb8();
        Self::new(rgb.width() as usize, rgb.height() as usize, rgb.into_raw()).ok_or_else(|| "the picture is empty".to_string())
    }

    /// The pixel at a place, or the nearest one when the place is outside.
    #[inline]
    fn at(&self, x: i64, y: i64) -> &[u8] {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        &self.rgb[(y * self.width + x) * 3..][..3]
    }

    /// The colour at a place between pixels, from the sixteen around it.
    fn between(&self, x: f32, y: f32) -> [f32; 3] {
        let (left, top) = (x.floor(), y.floor());
        let (across, down) = (cubic(x - left), cubic(y - top));
        let mut colour = [0.0f32; 3];
        for (row, weight_y) in down.iter().enumerate() {
            for (column, weight_x) in across.iter().enumerate() {
                let pixel = self.at(left as i64 + column as i64 - 1, top as i64 + row as i64 - 1);
                let weight = weight_x * weight_y;
                for channel in 0..3 {
                    colour[channel] += weight * pixel[channel] as f32;
                }
            }
        }
        colour
    }

    /// The part of the picture inside `corners` (top-left, top-right,
    /// bottom-right, bottom-left), made level, at the size it has in the picture.
    pub fn straightened(&self, corners: &[[f32; 2]; 4]) -> Picture {
        let side = |p: [f32; 2], q: [f32; 2]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
        let width = side(corners[0], corners[1]).max(side(corners[3], corners[2])).round().max(1.0) as usize;
        let height = side(corners[0], corners[3]).max(side(corners[1], corners[2])).round().max(1.0) as usize;
        let mut rgb = Vec::with_capacity(width * height * 3);
        for y in 0..height {
            let down = y as f32 / height as f32;
            let start = [corners[0][0] + (corners[3][0] - corners[0][0]) * down, corners[0][1] + (corners[3][1] - corners[0][1]) * down];
            let end = [corners[1][0] + (corners[2][0] - corners[1][0]) * down, corners[1][1] + (corners[2][1] - corners[1][1]) * down];
            for x in 0..width {
                let across = x as f32 / width as f32;
                let colour = self.between(start[0] + (end[0] - start[0]) * across, start[1] + (end[1] - start[1]) * across);
                rgb.extend(colour.map(|value| value.round().clamp(0.0, 255.0) as u8));
            }
        }
        Picture { width, height, rgb }
    }

    /// The picture at another size. A side that shrinks is averaged, so that
    /// thin strokes fade instead of vanishing; a side that grows is drawn
    /// with the cubic curve.
    pub fn resized(&self, width: usize, height: usize) -> Picture {
        let (width, height) = (width.max(1), height.max(1));
        let across = taps(self.width, width);
        let down = taps(self.height, height);
        // Rows first, at the old height.
        let mut wide = vec![0.0f32; width * self.height * 3];
        for y in 0..self.height {
            let row = &self.rgb[y * self.width * 3..][..self.width * 3];
            for (x, tap) in across.iter().enumerate() {
                let out = &mut wide[(y * width + x) * 3..][..3];
                for (from, weight) in tap {
                    for channel in 0..3 {
                        out[channel] += weight * row[from * 3 + channel] as f32;
                    }
                }
            }
        }
        let mut rgb = Vec::with_capacity(width * height * 3);
        for tap in &down {
            for x in 0..width {
                let mut colour = [0.0f32; 3];
                for (from, weight) in tap {
                    let pixel = &wide[(from * width + x) * 3..][..3];
                    for channel in 0..3 {
                        colour[channel] += weight * pixel[channel];
                    }
                }
                rgb.extend(colour.map(|value| value.round().clamp(0.0, 255.0) as u8));
            }
        }
        Picture { width, height, rgb }
    }

    /// The columns from `from` up to `to`, as a picture of their own.
    pub fn columns(&self, from: usize, to: usize) -> Picture {
        let (from, to) = (from.min(self.width), to.min(self.width));
        let mut rgb = Vec::with_capacity((to - from) * self.height * 3);
        for y in 0..self.height {
            rgb.extend_from_slice(&self.rgb[(y * self.width + from) * 3..(y * self.width + to) * 3]);
        }
        Picture { width: to - from, height: self.height, rgb }
    }

    /// How bright each pixel is, 0 to 255.
    fn brightness(&self) -> Vec<u8> {
        self.rgb.as_chunks::<3>().0.iter().map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).round() as u8).collect()
    }

    /// Which columns hold no ink at all: the gaps between words, and more.
    ///
    /// Ink is what is darker than the brightness that best splits the
    /// picture's pixels into a dark and a light kind (Otsu's method).
    pub fn empty_columns(&self) -> Vec<bool> {
        let bright = self.brightness();
        let mut counts = [0u32; 256];
        for value in &bright {
            counts[*value as usize] += 1;
        }
        let total = bright.len() as f64;
        let sum: f64 = counts.iter().enumerate().map(|(value, count)| value as f64 * *count as f64).sum();
        let (mut below, mut sum_below, mut best, mut split) = (0.0f64, 0.0f64, -1.0f64, 0usize);
        for (value, count) in counts.iter().enumerate() {
            below += *count as f64;
            sum_below += value as f64 * *count as f64;
            let above = total - below;
            if below == 0.0 || above == 0.0 {
                continue;
            }
            let apart = sum_below / below - (sum - sum_below) / above;
            let spread = below * above * apart * apart;
            if spread > best {
                (best, split) = (spread, value);
            }
        }
        (0..self.width).map(|x| (0..self.height).all(|y| bright[y * self.width + x] as usize > split)).collect()
    }
}

/// For each pixel of a side's new length, the old pixels it is made of and
/// how much each counts.
fn taps(old: usize, new: usize) -> Vec<Vec<(usize, f32)>> {
    let step = old as f32 / new as f32;
    (0..new)
        .map(|index| {
            if new < old {
                // The stretch of old pixels this new pixel covers, with the
                // first and the last counted for the part that is covered.
                let (from, to) = (index as f32 * step, (index + 1) as f32 * step);
                let (first, last) = (from.floor() as usize, (to.ceil() as usize).min(old));
                (first..last)
                    .map(|pixel| (pixel, ((pixel + 1) as f32).min(to) - (pixel as f32).max(from)))
                    .filter(|(_, covered)| *covered > 0.0)
                    .map(|(pixel, covered)| (pixel, covered / step))
                    .collect()
            } else {
                let place = (index as f32 + 0.5) * step - 0.5;
                let left = place.floor();
                cubic(place - left)
                    .iter()
                    .enumerate()
                    .map(|(tap, weight)| ((left as i64 + tap as i64 - 1).clamp(0, old as i64 - 1) as usize, *weight))
                    .collect()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: usize, height: usize, colour: [u8; 3]) -> Picture {
        Picture { width, height, rgb: colour.repeat(width * height) }
    }

    fn paint(picture: &mut Picture, x: std::ops::Range<usize>, y: std::ops::Range<usize>, colour: [u8; 3]) {
        for row in y {
            for column in x.clone() {
                picture.rgb[(row * picture.width + column) * 3..][..3].copy_from_slice(&colour);
            }
        }
    }

    fn pixel(picture: &Picture, x: usize, y: usize) -> [u8; 3] {
        picture.rgb[(y * picture.width + x) * 3..][..3].try_into().unwrap()
    }

    #[test]
    fn a_level_part_is_cut_out_as_it_is() {
        let mut picture = flat(40, 30, [250, 250, 250]);
        paint(&mut picture, 10..20, 8..12, [10, 20, 30]);
        let cut = picture.straightened(&[[10.0, 8.0], [20.0, 8.0], [20.0, 12.0], [10.0, 12.0]]);
        assert_eq!((cut.width, cut.height), (10, 4));
        assert!(cut.rgb.as_chunks::<3>().0.iter().all(|p| *p == [10, 20, 30]));
    }

    #[test]
    fn a_turned_part_comes_out_level() {
        // A dark bar turned by about 14 degrees on white paper.
        let mut picture = flat(120, 80, [255, 255, 255]);
        let (sin, cos) = 14.0f32.to_radians().sin_cos();
        for y in 0..80 {
            for x in 0..120 {
                let (dx, dy) = (x as f32 - 60.0, y as f32 - 40.0);
                let (along, across) = (dx * cos + dy * sin, -dx * sin + dy * cos);
                if along.abs() <= 40.0 && across.abs() <= 6.0 {
                    picture.rgb[(y * 120 + x) * 3..][..3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        let corner = |along: f32, across: f32| [60.0 + along * cos - across * sin, 40.0 + along * sin + across * cos];
        let cut = picture.straightened(&[corner(-40.0, -12.0), corner(40.0, -12.0), corner(40.0, 12.0), corner(-40.0, 12.0)]);
        assert_eq!((cut.width, cut.height), (80, 24));
        // White above and below, dark through the middle, from end to end.
        for x in [3, 40, 76] {
            assert!(pixel(&cut, x, 2)[0] > 230, "top at {x}");
            assert!(pixel(&cut, x, 12)[0] < 25, "middle at {x}");
            assert!(pixel(&cut, x, 21)[0] > 230, "bottom at {x}");
        }
    }

    #[test]
    fn a_part_past_the_edge_repeats_the_edge() {
        let picture = flat(20, 20, [7, 8, 9]);
        let cut = picture.straightened(&[[15.0, 5.0], [30.0, 5.0], [30.0, 10.0], [15.0, 10.0]]);
        assert!(cut.rgb.as_chunks::<3>().0.iter().all(|p| *p == [7, 8, 9]));
    }

    #[test]
    fn shrinking_averages() {
        // Black and white columns, two pixels each, halved: every pixel stays pure; quartered: all grey.
        let mut picture = flat(8, 4, [255, 255, 255]);
        paint(&mut picture, 0..2, 0..4, [0, 0, 0]);
        paint(&mut picture, 4..6, 0..4, [0, 0, 0]);
        let half = picture.resized(4, 2);
        assert_eq!([pixel(&half, 0, 0)[0], pixel(&half, 1, 0)[0], pixel(&half, 2, 1)[0], pixel(&half, 3, 1)[0]], [0, 255, 0, 255]);
        let quarter = picture.resized(2, 1);
        assert_eq!(pixel(&quarter, 0, 0), [128, 128, 128]);
        // A size that does not divide evenly keeps the picture's brightness.
        let odd = picture.resized(3, 4);
        let mean = odd.rgb.iter().map(|v| *v as f32).sum::<f32>() / odd.rgb.len() as f32;
        assert!((mean - 127.5).abs() < 1.5, "{mean}");
    }

    #[test]
    fn growing_keeps_flat_colour_flat() {
        let picture = flat(5, 3, [200, 100, 50]);
        let larger = picture.resized(13, 9);
        assert_eq!((larger.width, larger.height), (13, 9));
        assert!(larger.rgb.as_chunks::<3>().0.iter().all(|p| *p == [200, 100, 50]));
    }

    #[test]
    fn columns_without_ink_are_found_under_uneven_light() {
        // Paper that gets darker to the right, with two words on it.
        let mut picture = flat(100, 20, [0, 0, 0]);
        for x in 0..100 {
            let paper = 240 - x as u8 / 2;
            paint(&mut picture, x..x + 1, 0..20, [paper, paper, paper]);
        }
        paint(&mut picture, 10..40, 5..15, [30, 30, 30]);
        paint(&mut picture, 60..90, 5..15, [30, 30, 30]);
        let empty = picture.empty_columns();
        assert!(empty[..10].iter().all(|e| *e) && empty[40..60].iter().all(|e| *e) && empty[90..].iter().all(|e| *e));
        assert!(empty[10..40].iter().all(|e| !*e) && empty[60..90].iter().all(|e| !*e));
    }

    #[test]
    fn columns_are_cut_out() {
        let mut picture = flat(10, 2, [1, 1, 1]);
        paint(&mut picture, 4..6, 0..2, [9, 9, 9]);
        let part = picture.columns(4, 6);
        assert_eq!((part.width, part.height), (2, 2));
        assert!(part.rgb.iter().all(|v| *v == 9));
    }
}
