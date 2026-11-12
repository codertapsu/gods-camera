//! A very small greyscale copy of a picture, enough to tell frames apart.
//!
//! The loop finder compares these instead of whole pictures. Same grid and
//! same arithmetic as the Mac app's `Thumbnail`.

use gc_infer::image::ImageView;

pub const COLUMNS: usize = 32;
pub const ROWS: usize = 18;
pub const CELLS: usize = COLUMNS * ROWS;

/// Average brightness of each cell of a 32 x 18 grid over a picture.
#[derive(Clone, PartialEq, Eq)]
pub struct Thumbnail(Box<[u8; CELLS]>);

impl std::fmt::Debug for Thumbnail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Thumbnail(mean {:.1})", self.mean())
    }
}

impl Thumbnail {
    pub fn from_cells(cells: [u8; CELLS]) -> Self {
        Self(Box::new(cells))
    }

    /// `None` unless `bytes` is exactly one thumbnail.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self(Box::new(bytes.try_into().ok()?)))
    }

    pub fn of(picture: &ImageView<'_>) -> Self {
        let (width, height) = (picture.width as usize, picture.height as usize);
        let [r, g, b] = picture.format.rgb_offsets();
        let bytes = picture.format.bytes();
        let mut cells = [0u8; CELLS];
        if width == 0 || height == 0 {
            return Self(Box::new(cells));
        }
        // Four samples per cell are enough to tell frames apart.
        const OFFSETS: [(f64, f64); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
        for row in 0..ROWS {
            for column in 0..COLUMNS {
                let mut sum = 0usize;
                for (dx, dy) in OFFSETS {
                    let x = (((column as f64 + dx) * width as f64 / COLUMNS as f64) as usize).min(width - 1);
                    let y = (((row as f64 + dy) * height as f64 / ROWS as f64) as usize).min(height - 1);
                    let pixel = &picture.row(y)[x * bytes..];
                    // Integer luma.
                    sum += (pixel[b] as usize * 29 + pixel[g] as usize * 150 + pixel[r] as usize * 77) >> 8;
                }
                cells[row * COLUMNS + column] = (sum / OFFSETS.len()) as u8;
            }
        }
        Self(Box::new(cells))
    }

    pub fn cells(&self) -> &[u8; CELLS] {
        &self.0
    }

    /// Average brightness, 0 to 255.
    pub fn mean(&self) -> f32 {
        self.0.iter().map(|&cell| cell as u32).sum::<u32>() as f32 / CELLS as f32
    }

    /// Mean absolute difference from another thumbnail, in 8-bit levels.
    pub fn difference(&self, other: &Thumbnail) -> f32 {
        self.0.iter().zip(other.0.iter()).map(|(&a, &b)| a.abs_diff(b) as u32).sum::<u32>() as f32 / CELLS as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gc_infer::image::PixelFormat;

    #[test]
    fn brightness_is_measured_per_cell_whatever_the_byte_order() {
        // Left half white, right half a mid red; 64 x 36, so every cell covers 2 x 2 pixels.
        let (width, height) = (64usize, 36usize);
        let colour = |x: usize| if x < width / 2 { [255u8, 255, 255] } else { [200u8, 0, 0] };
        let mut bgra = Vec::new();
        let mut rgb = Vec::new();
        for _ in 0..height {
            for x in 0..width {
                let [r, g, b] = colour(x);
                bgra.extend_from_slice(&[b, g, r, 255]);
                rgb.extend_from_slice(&[r, g, b]);
            }
        }
        let from_bgra =
            Thumbnail::of(&ImageView::new(&bgra, width as u32, height as u32, width * 4, PixelFormat::Bgra).unwrap());
        let from_rgb =
            Thumbnail::of(&ImageView::new(&rgb, width as u32, height as u32, width * 3, PixelFormat::Rgb).unwrap());
        assert_eq!(from_bgra, from_rgb);
        assert_eq!(from_bgra.cells()[0], 255);
        // (200 * 77) >> 8
        assert_eq!(from_bgra.cells()[COLUMNS - 1], 60);
        assert!((from_bgra.mean() - (255.0 + 60.0) / 2.0).abs() < 0.01);
        assert_eq!(from_bgra.difference(&from_bgra), 0.0);
        assert_eq!(from_bgra.difference(&Thumbnail::from_cells([255; CELLS])), (255.0 - 60.0) / 2.0);
    }

    #[test]
    fn a_thumbnail_survives_being_written_out() {
        let mut cells = [0u8; CELLS];
        for (index, cell) in cells.iter_mut().enumerate() {
            *cell = (index % 251) as u8;
        }
        let thumbnail = Thumbnail::from_cells(cells);
        assert_eq!(Thumbnail::from_bytes(thumbnail.cells()), Some(thumbnail));
        assert_eq!(Thumbnail::from_bytes(&[0u8; 5]), None);
    }
}
