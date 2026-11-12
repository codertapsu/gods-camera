//! Colour bars with a moving block: a picture source that needs no camera,
//! for checking the virtual camera in other apps.

/// Draws the test picture.
pub struct Pattern {
    width: usize,
    height: usize,
    /// One row of the bars, four bytes a pixel: blue, green, red, unused.
    row: Vec<u8>,
}

/// The eight bars from left to right, at three quarters of full strength, as red, green, blue.
const BARS: [[u8; 3]; 8] =
    [[191, 191, 191], [191, 191, 0], [0, 191, 191], [0, 191, 0], [191, 0, 191], [191, 0, 0], [0, 0, 191], [16, 16, 16]];

impl Pattern {
    pub fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width.max(16) as usize, height.max(16) as usize);
        let mut row = Vec::with_capacity(width * 4);
        for x in 0..width {
            let [r, g, b] = BARS[x * BARS.len() / width];
            row.extend_from_slice(&[b, g, r, 255]);
        }
        Self { width, height, row }
    }

    pub fn width(&self) -> u32 {
        self.width as u32
    }

    pub fn height(&self) -> u32 {
        self.height as u32
    }

    /// The picture `seconds` after the start, into `out`: four bytes a pixel, row after row.
    pub fn render(&self, seconds: f32, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(self.width * self.height * 4);
        for _ in 0..self.height {
            out.extend_from_slice(&self.row);
        }
        // A white block that crosses the lower part of the picture and comes
        // back, once every four seconds: movement shows that the picture is live.
        let side = self.height / 6;
        let travel = (self.width - side) as f32;
        let phase = (seconds / 4.0).fract();
        let left = (travel * if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 }) as usize;
        let top = self.height * 2 / 3;
        for y in top..(top + side).min(self.height) {
            let start = (y * self.width + left) * 4;
            out[start..start + side * 4].fill(255);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bars_stand_still_and_the_block_moves() {
        let pattern = Pattern::new(1280, 720);
        let (mut early, mut later, mut back) = (Vec::new(), Vec::new(), Vec::new());
        pattern.render(0.0, &mut early);
        pattern.render(1.0, &mut later);
        pattern.render(4.0, &mut back);
        assert_eq!(early.len(), 1280 * 720 * 4);
        let pixel = |picture: &[u8], x: usize, y: usize| picture[(y * 1280 + x) * 4..][..3].to_vec();
        // The first bar is light grey, the last nearly black, in the upper part of the picture.
        assert_eq!(pixel(&early, 10, 10), [191, 191, 191]);
        assert_eq!(pixel(&early, 1270, 10), [16, 16, 16]);
        // Blue, green, red: the second bar is yellow.
        assert_eq!(pixel(&early, 200, 10), [0, 191, 191]);
        // The block starts at the left, is half-way after a second, and is back after four.
        assert_eq!(pixel(&early, 20, 540), [255, 255, 255]);
        assert_ne!(pixel(&later, 20, 540), [255, 255, 255]);
        assert_eq!(pixel(&later, 640, 540), [255, 255, 255]);
        assert_eq!(early, back);
        // Above the block's path nothing changes.
        assert_eq!(early[..1280 * 400 * 4], later[..1280 * 400 * 4]);
    }

    #[test]
    fn any_size_can_be_drawn() {
        for (width, height) in [(16, 16), (641, 359), (1920, 1080)] {
            let pattern = Pattern::new(width, height);
            let mut picture = Vec::new();
            for step in 0..9 {
                pattern.render(step as f32 * 0.5, &mut picture);
                assert_eq!(picture.len(), (width * height * 4) as usize);
            }
        }
    }
}
