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
        self.rgb.as_chunks::<3>().0.iter().map(bright).collect()
    }

    /// How speckled the picture is: the share of its pixels, 0 to 1, that
    /// stand apart from the eight around them.
    ///
    /// A pixel stands apart when it is much darker or much lighter than the
    /// middle one of the nine, taken by brightness. A stroke three pixels
    /// wide has none of these, and neither has a photo; a scan whose grain a
    /// filter has turned into specks has many. Every third pixel of every
    /// third row is looked at, which is enough to tell.
    pub fn speckle(&self) -> f32 {
        /// How far apart, of 255.
        const APART: i32 = 80;
        if self.width < 3 || self.height < 3 {
            return 0.0;
        }
        let (mut looked, mut apart) = (0usize, 0usize);
        for y in (1..self.height - 1).step_by(3) {
            for x in (1..self.width - 1).step_by(3) {
                let mut nine = [0u8; 9];
                for (place, value) in nine.iter_mut().enumerate() {
                    *value = bright(self.rgb[((y + place / 3 - 1) * self.width + x + place % 3 - 1) * 3..].first_chunk().expect("inside the picture"));
                }
                let own = nine[4] as i32;
                let middle = *nine.select_nth_unstable(4).1 as i32;
                looked += 1;
                apart += usize::from((own - middle).abs() > APART);
            }
        }
        apart as f32 / looked.max(1) as f32
    }

    /// The picture in grey with its specks washed out: blurred by `blur`
    /// pixels (the width of the bell curve, its standard deviation), then
    /// stretched so that what is darker than `ink` turns black and what is
    /// lighter than `paper` turns white. Both are brightnesses from 0 to 1.
    ///
    /// The blur spreads a speck thin over the paper around it, while a
    /// stroke, being wider, stays dark down its middle. The stretch then
    /// takes the faint grey off the paper and gives the strokes their edges
    /// back.
    pub fn cleaned(&self, blur: f32, ink: f32, paper: f32) -> Picture {
        let (width, height) = (self.width, self.height);
        let blur = blur.max(0.05);
        let reach = ((3.0 * blur).ceil() as usize).max(1);
        let mut weights: Vec<f32> = (0..=2 * reach).map(|tap| bell((tap as f64 - reach as f64) / blur as f64) as f32).collect();
        let whole: f32 = weights.iter().sum();
        weights.iter_mut().for_each(|weight| *weight /= whole);

        // Along the rows first. Past either end, the end pixel is repeated.
        let mut along = vec![0.0f32; width * height];
        let mut row = vec![0.0f32; width + 2 * reach];
        for (pixels, blurred) in self.rgb.chunks_exact(width * 3).zip(along.chunks_exact_mut(width)) {
            for (value, pixel) in row[reach..].iter_mut().zip(pixels.as_chunks::<3>().0) {
                *value = bright(pixel) as f32;
            }
            let (first, last) = (row[reach], row[reach + width - 1]);
            row[..reach].fill(first);
            row[reach + width..].fill(last);
            for (x, value) in blurred.iter_mut().enumerate() {
                *value = weights.iter().zip(&row[x..]).map(|(weight, pixel)| weight * pixel).sum();
            }
        }

        // Then down the columns, a whole row at a time, and the stretch.
        let stretch = if paper > ink { 1.0 / (paper - ink) } else { 1.0 };
        let dark = if paper > ink { ink } else { 0.0 };
        let mut rgb = Vec::with_capacity(width * height * 3);
        let mut down = vec![0.0f32; width];
        for y in 0..height {
            down.fill(0.0);
            for (tap, weight) in weights.iter().enumerate() {
                let from = (y + tap).saturating_sub(reach).min(height - 1);
                for (value, pixel) in down.iter_mut().zip(&along[from * width..][..width]) {
                    *value += weight * pixel;
                }
            }
            for value in &down {
                let grey = (((value / 255.0 - dark) * stretch).clamp(0.0, 1.0) * 255.0).round() as u8;
                rgb.extend_from_slice(&[grey, grey, grey]);
            }
        }
        Picture { width, height, rgb }
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

/// How bright a pixel is, 0 to 255.
#[inline]
fn bright(pixel: &[u8; 3]) -> u8 {
    (0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32).round() as u8
}

/// The bell curve, `x` widths from its middle: e to the power of minus half
/// of x squared.
///
/// Worked out with sums and products only, which give the same digits on
/// every machine. The system's own `exp` may differ in the last digit from
/// one system to the next, and a cleaned picture is to be the same picture
/// everywhere.
fn bell(x: f64) -> f64 {
    // e to a small power by the first terms of its series, then squared ten
    // times over, which makes the power 1024 times as large.
    let power = -x * x / 2.0 / 1024.0;
    let mut value = 1.0 + power * (1.0 + power / 2.0 * (1.0 + power / 3.0 * (1.0 + power / 4.0)));
    for _ in 0..10 {
        value *= value;
    }
    value
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

    /// White paper with a grey speck on one pixel in 23, none of them touching.
    fn speckled_paper(width: usize, height: usize) -> Picture {
        let mut picture = flat(width, height, [255, 255, 255]);
        for y in 0..height {
            for x in (0..width).filter(|x| (x * 7 + y * 13) % 23 == 0) {
                paint(&mut picture, x..x + 1, y..y + 1, [110, 110, 110]);
            }
        }
        picture
    }

    #[test]
    fn specks_are_counted_and_strokes_are_not() {
        assert_eq!(flat(60, 40, [240, 238, 230]).speckle(), 0.0);
        // Strokes three pixels wide, across and down, as close together as letters have them.
        let mut strokes = flat(90, 60, [255, 255, 255]);
        for x in (6..84).step_by(9) {
            paint(&mut strokes, x..x + 3, 5..55, [0, 0, 0]);
        }
        paint(&mut strokes, 3..87, 28..31, [0, 0, 0]);
        assert_eq!(strokes.speckle(), 0.0);
        // Light that falls off towards a corner, as in a photo.
        let mut photo = flat(90, 60, [0, 0, 0]);
        for y in 0..60 {
            for x in 0..90 {
                let level = (250 - x - y) as u8;
                paint(&mut photo, x..x + 1, y..y + 1, [level, level, level]);
            }
        }
        assert_eq!(photo.speckle(), 0.0);
        // One pixel in 23 is a speck.
        let share = speckled_paper(230, 115).speckle();
        assert!((share - 1.0 / 23.0).abs() < 0.004, "{share}");
        // Too small a picture to have surroundings.
        assert_eq!(flat(2, 9, [0, 0, 0]).speckle(), 0.0);
    }

    #[test]
    fn cleaning_takes_the_specks_and_leaves_the_strokes() {
        let mut picture = speckled_paper(120, 60);
        paint(&mut picture, 40..46, 5..55, [20, 20, 20]);
        assert!(picture.speckle() > 0.03);
        let cleaned = picture.cleaned(1.6, 0.3, 0.7);
        assert_eq!((cleaned.width, cleaned.height), (120, 60));
        // The paper is white again, to its corners,
        for (x, y) in [(0, 0), (10, 10), (80, 30), (110, 50), (119, 59), (30, 30), (56, 30)] {
            assert_eq!(pixel(&cleaned, x, y), [255, 255, 255], "at {x}, {y}");
        }
        // the stroke is black down its middle, with an edge on either side,
        for y in [10, 30, 50] {
            assert_eq!([pixel(&cleaned, 42, y)[0], pixel(&cleaned, 43, y)[0]], [0, 0], "row {y}");
            assert!(pixel(&cleaned, 39, y)[0] > pixel(&cleaned, 40, y)[0], "row {y}");
            assert!(pixel(&cleaned, 46, y)[0] > pixel(&cleaned, 45, y)[0], "row {y}");
        }
        // and nothing stands alone any more.
        assert_eq!(cleaned.speckle(), 0.0);
        assert!(cleaned.rgb.as_chunks::<3>().0.iter().all(|p| p[0] == p[1] && p[1] == p[2]));
    }

    #[test]
    fn cleaning_keeps_flat_paper_flat_and_colour_out() {
        // Without a stretch, a flat picture stays what it was, in grey.
        let cleaned = flat(20, 10, [200, 200, 200]).cleaned(1.5, 0.0, 1.0);
        assert!(cleaned.rgb.iter().all(|v| *v == 200), "{:?}", &cleaned.rgb[..6]);
        let cleaned = flat(7, 5, [255, 0, 0]).cleaned(0.8, 0.0, 1.0);
        assert!(cleaned.rgb.iter().all(|v| *v == 76));
        // A picture narrower than the blur reaches.
        assert_eq!(flat(1, 1, [9, 9, 9]).cleaned(3.0, 0.0, 1.0).rgb, [9, 9, 9]);
    }

    #[test]
    fn the_bell_curve_is_the_bell_curve() {
        assert_eq!(bell(0.0), 1.0);
        for (x, value) in [(1.0, 0.606_530_659_712_633_4), (2.0, 0.135_335_283_236_612_7), (3.0, 0.011_108_996_538_242_306)] {
            assert!((bell(x) - value).abs() < 1e-10 && (bell(-x) - value).abs() < 1e-10, "{x}: {}", bell(x));
        }
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
