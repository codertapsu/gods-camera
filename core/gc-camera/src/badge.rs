//! The small labels drawn on the outgoing picture: "AI" while a swapped face
//! is shown, "LOOP" while a loop plays.
//!
//! The letters are drawn here from strokes, so a badge is sharp at any size
//! and the app carries no font for them.

/// A label: white letters on a rounded plate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    /// Red, green, blue, alpha; not premultiplied.
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Stroke width of the letters, in units of their height.
const STROKE: f32 = 0.2;
/// Space between letters, in the same units, measured between the strokes' edges.
const GAP: f32 = 0.24;
/// Letter height as a part of the plate's height.
const LETTERS: f32 = 0.46;
/// The plate's corner radius as a part of its height.
const CORNER: f32 = 0.3;

/// The strokes of one letter, in a box one unit tall, and the box's width.
/// The top of a letter is at 0 and its foot at 1.
fn letter(character: char) -> Option<(Vec<Vec<[f32; 2]>>, f32)> {
    let arc = |centre: [f32; 2], radii: [f32; 2], from: f32, to: f32| -> Vec<[f32; 2]> {
        (0..=24)
            .map(|step| {
                let angle = (from + (to - from) * step as f32 / 24.0).to_radians();
                [centre[0] + radii[0] * angle.cos(), centre[1] + radii[1] * angle.sin()]
            })
            .collect()
    };
    Some(match character {
        'A' => (vec![vec![[0.0, 1.0], [0.43, 0.0], [0.86, 1.0]], vec![[0.17, 0.66], [0.69, 0.66]]], 0.86),
        'I' => (vec![vec![[0.0, 0.0], [0.0, 1.0]]], 0.0),
        'L' => (vec![vec![[0.0, 0.0], [0.0, 1.0], [0.56, 1.0]]], 0.56),
        'O' => (vec![arc([0.43, 0.5], [0.43, 0.5], 0.0, 360.0)], 0.86),
        'P' => {
            let mut bowl = vec![[0.0, 0.0]];
            bowl.extend(arc([0.3, 0.29], [0.3, 0.29], -90.0, 90.0));
            bowl.push([0.0, 0.58]);
            (vec![vec![[0.0, 1.0], [0.0, 0.0]], bowl], 0.6)
        }
        _ => return None,
    })
}

fn distance_to_segment(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (ab, ap) = ([b[0] - a[0], b[1] - a[1]], [point[0] - a[0], point[1] - a[1]]);
    let length = ab[0] * ab[0] + ab[1] * ab[1];
    let along = if length > 0.0 { ((ap[0] * ab[0] + ap[1] * ab[1]) / length).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (ap[0] - ab[0] * along, ap[1] - ab[1] * along);
    (dx * dx + dy * dy).sqrt()
}

impl Badge {
    /// A badge `height` pixels tall. `text` may use the letters A, I, L, O and
    /// P; others are left out. `plate` is the plate's colour: red, green, blue, alpha.
    pub fn new(text: &str, height: u32, plate: [u8; 4]) -> Self {
        let height = height.max(8);
        let unit = height as f32 * LETTERS;
        // Lay the letters out in units, starting half a stroke in so the
        // first stroke's round edge is inside the plate's padding.
        let mut strokes: Vec<[[f32; 2]; 2]> = Vec::new();
        let mut cursor = STROKE / 2.0;
        for (strokes_of, width) in text.chars().filter_map(letter) {
            for line in strokes_of {
                for pair in line.windows(2) {
                    strokes.push([[pair[0][0] + cursor, pair[0][1]], [pair[1][0] + cursor, pair[1][1]]]);
                }
            }
            cursor += width + STROKE + GAP;
        }
        let letters_width = (cursor - GAP - STROKE / 2.0).max(0.0);
        let padding = height as f32 * 0.34;
        let width = (letters_width * unit + 2.0 * padding).ceil().max(height as f32) as u32;
        let origin = [(width as f32 - letters_width * unit) / 2.0, (height as f32 - unit) / 2.0];
        let radius = STROKE / 2.0 * unit;
        let corner = CORNER * height as f32;

        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let point = [x as f32 + 0.5, y as f32 + 0.5];
                // How much of the pixel the plate covers: a rounded rectangle.
                let half = [width as f32 / 2.0, height as f32 / 2.0];
                let inner = [(point[0] - half[0]).abs() - (half[0] - corner), (point[1] - half[1]).abs() - (half[1] - corner)];
                let outside = (inner[0].max(0.0).powi(2) + inner[1].max(0.0).powi(2)).sqrt() + inner[0].max(inner[1]).min(0.0);
                let plate_cover = (0.5 - (outside - corner)).clamp(0.0, 1.0);
                // And how much the letters do.
                let in_units = [(point[0] - origin[0]) / unit, (point[1] - origin[1]) / unit];
                let nearest =
                    strokes.iter().map(|stroke| distance_to_segment(in_units, stroke[0], stroke[1])).fold(f32::MAX, f32::min);
                let letter_cover = (0.5 - (nearest * unit - radius)).clamp(0.0, 1.0) * plate_cover;

                let plate_alpha = plate[3] as f32 / 255.0 * plate_cover;
                let alpha = letter_cover + plate_alpha * (1.0 - letter_cover);
                let pixel = &mut rgba[((y * width + x) * 4) as usize..][..4];
                if alpha > 0.0 {
                    for channel in 0..3 {
                        let colour = (255.0 * letter_cover + plate[channel] as f32 * plate_alpha * (1.0 - letter_cover)) / alpha;
                        pixel[channel] = colour.round().clamp(0.0, 255.0) as u8;
                    }
                }
                pixel[3] = (alpha * 255.0).round() as u8;
            }
        }
        Self { rgba, width, height }
    }

    /// The "AI" badge: shown whenever a swapped face is on the picture.
    pub fn ai(output_height: u32) -> Self {
        Self::new("AI", (output_height as f32 * 0.05).round() as u32, [26, 26, 26, 191])
    }

    /// The "LOOP" badge.
    pub fn looping(output_height: u32) -> Self {
        Self::new("LOOP", (output_height as f32 * 0.06).round() as u32, [217, 51, 51, 230])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(badge: &Badge, x: u32, y: u32) -> u8 {
        badge.rgba[((y * badge.width + x) * 4 + 3) as usize]
    }

    fn colour(badge: &Badge, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * badge.width + x) * 4) as usize;
        [badge.rgba[at], badge.rgba[at + 1], badge.rgba[at + 2]]
    }

    #[test]
    fn a_badge_is_a_rounded_plate_with_white_letters() {
        let badge = Badge::ai(720);
        assert_eq!(badge.height, 36);
        assert!(badge.width > badge.height && badge.width < 3 * badge.height, "{} wide", badge.width);
        assert_eq!(badge.rgba.len(), (badge.width * badge.height * 4) as usize);
        // The corners are cut away; the middle of an edge is not.
        assert_eq!(alpha(&badge, 0, 0), 0);
        assert_eq!(alpha(&badge, badge.width - 1, badge.height - 1), 0);
        assert_eq!(alpha(&badge, badge.width / 2, 0), 191);
        assert_eq!(colour(&badge, badge.width / 2, 1), [26, 26, 26]);
        // Somewhere there are fully white, fully solid letter pixels, and they
        // are a modest part of the plate.
        let white = (0..badge.width * badge.height)
            .filter(|&i| badge.rgba[(i * 4) as usize..][..4] == [255, 255, 255, 255])
            .count() as f32;
        let part = white / (badge.width * badge.height) as f32;
        assert!((0.04..0.35).contains(&part), "letters cover {part} of the plate");
        // The letters stay inside the plate's padding.
        for y in 0..badge.height {
            assert_ne!(colour(&badge, 2, y), [255, 255, 255]);
            assert_ne!(colour(&badge, badge.width - 3, y), [255, 255, 255]);
        }
    }

    #[test]
    fn a_longer_word_makes_a_wider_badge_and_size_follows_the_picture() {
        let (ai, looping) = (Badge::ai(720), Badge::looping(720));
        assert_eq!(looping.height, 43);
        assert!(looping.width > ai.width + 40);
        assert_eq!(colour(&looping, looping.width / 2, 1), [217, 51, 51]);
        assert_eq!(Badge::ai(1080).height, 54);
        // Letters this module cannot draw are left out instead of drawn wrong.
        assert_eq!(Badge::new("A?I", 36, [0, 0, 0, 255]), Badge::new("AI", 36, [0, 0, 0, 255]));
        // Nothing to write still gives a plate.
        let empty = Badge::new("", 36, [0, 0, 0, 255]);
        assert_eq!((empty.width, empty.height), (36, 36));
    }

    /// Writes the badges where a person can look at them: target/tmp/badges.png.
    #[test]
    fn badges_can_be_looked_at() {
        let badges = [Badge::ai(720), Badge::looping(720), Badge::ai(1080), Badge::looping(1080), Badge::ai(360)];
        // On a mid-grey ground and on a light one, enlarged three times.
        let scale = 3usize;
        let width = badges.iter().map(|badge| badge.width as usize * scale + 30).sum::<usize>() + 30;
        let row = badges.iter().map(|badge| badge.height as usize * scale).max().unwrap() + 60;
        let mut picture = vec![0u8; width * row * 2 * 3];
        for (band, ground) in [[96u8, 104, 112], [228, 226, 220]].into_iter().enumerate() {
            for pixel in picture[band * width * row * 3..][..width * row * 3].as_chunks_mut::<3>().0 {
                *pixel = ground;
            }
            let mut left = 30;
            for badge in &badges {
                for y in 0..badge.height as usize * scale {
                    for x in 0..badge.width as usize * scale {
                        let at = ((y / scale) * badge.width as usize + x / scale) * 4;
                        let alpha = badge.rgba[at + 3] as f32 / 255.0;
                        let pixel = &mut picture[((band * row + 30 + y) * width + left + x) * 3..][..3];
                        for (value, &colour) in pixel.iter_mut().zip(&badge.rgba[at..at + 3]) {
                            *value = (*value as f32 + (colour as f32 - *value as f32) * alpha) as u8;
                        }
                    }
                }
                left += badge.width as usize * scale + 30;
            }
        }
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/tmp");
        std::fs::create_dir_all(&out).unwrap();
        image::save_buffer(out.join("badges.png"), &picture, width as u32, (row * 2) as u32, image::ColorType::Rgb8).unwrap();
    }
}
