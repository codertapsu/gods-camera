//! Finding the lines of text in the detector's answer.
//!
//! The detector gives one number per pixel: how likely the pixel is to lie
//! inside a line of text. It marks only the middle of each line, so that
//! neighbouring lines stay apart. Every island of likely pixels is one line:
//! the smallest rectangle around it, at whatever angle, widened to take in the
//! whole line. This is the method of "Real-time Scene Text Detection with
//! Differentiable Binarization" (Liao and others, 2020), which PP-OCR's
//! detectors are trained for.

/// How sure the detector must be, and how far a found line is widened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Finding {
    /// A pixel this likely counts as text.
    pub inside: f32,
    /// A line must be this likely on average.
    pub sure: f32,
    /// The marked middle of a line is widened on every side by this much of
    /// its area divided by its outline.
    pub grow: f32,
}

/// The values PP-OCRv6's small detector is published with.
impl Default for Finding {
    fn default() -> Self {
        Self { inside: 0.2, sure: 0.45, grow: 1.4 }
    }
}

/// A line of text in the detector's map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Found {
    /// Top-left, top-right, bottom-right, bottom-left, for a line that is roughly level.
    pub corners: [[f32; 2]; 4],
    /// How likely the pixels of its marked middle are, on average.
    pub score: f32,
}

/// No picture has more lines than this; a map of noise could have.
const MOST: usize = 3000;
/// A marked middle thinner than this is a speck.
const THINNEST_MARK: f32 = 3.0;
/// A widened line thinner than this is not text anyone could read.
const THINNEST_LINE: f32 = 5.0;

/// Every line in a map of `width` by `height` likelihoods, in no particular order.
pub fn find(likely: &[f32], width: usize, height: usize, finding: &Finding) -> Vec<Found> {
    assert_eq!(likely.len(), width * height, "the map is not as large as its size says");
    let mut seen = vec![false; likely.len()];
    let mut waiting = Vec::new();
    let mut rim: Vec<[i32; 2]> = Vec::new();
    let mut found = Vec::new();
    let mut islands = 0;
    for start in 0..likely.len() {
        if seen[start] || likely[start] <= finding.inside {
            continue;
        }
        islands += 1;
        if islands > MOST {
            break;
        }
        // Walk the island, keeping the pixels on its rim: they alone decide its outline.
        rim.clear();
        seen[start] = true;
        waiting.push(start);
        while let Some(at) = waiting.pop() {
            let (x, y) = ((at % width) as i64, (at / width) as i64);
            let mut on_rim = false;
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                let outside = nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64;
                let next = if outside { 0 } else { ny as usize * width + nx as usize };
                if outside || likely[next] <= finding.inside {
                    on_rim |= dx == 0 || dy == 0;
                } else if !seen[next] {
                    seen[next] = true;
                    waiting.push(next);
                }
            }
            if on_rim {
                rim.push([x as i32, y as i32]);
            }
        }
        found.extend(line_around(&rim, likely, width, height, finding));
    }
    found
}

/// A rectangle at an angle: `along` and `across` are unit vectors, and the
/// rectangle covers the points whose distances along them lie in the two ranges.
#[derive(Debug, Clone, Copy)]
struct Tilted {
    along: [f32; 2],
    across: [f32; 2],
    a: [f32; 2],
    b: [f32; 2],
}

impl Tilted {
    fn sides(&self) -> (f32, f32) {
        (self.a[1] - self.a[0], self.b[1] - self.b[0])
    }

    fn corners(&self) -> [[f32; 2]; 4] {
        let at = |a: f32, b: f32| [self.along[0] * a + self.across[0] * b, self.along[1] * a + self.across[1] * b];
        [at(self.a[0], self.b[0]), at(self.a[1], self.b[0]), at(self.a[1], self.b[1]), at(self.a[0], self.b[1])]
    }
}

fn dot(p: [f32; 2], q: [f32; 2]) -> f32 {
    p[0] * q[0] + p[1] * q[1]
}

/// The line an island marks, if it is one.
fn line_around(rim: &[[i32; 2]], likely: &[f32], width: usize, height: usize, finding: &Finding) -> Option<Found> {
    let mut tilted = smallest_rectangle(&outline(rim))?;
    let (long, short) = tilted.sides();
    if long.min(short) < THINNEST_MARK {
        return None;
    }
    let score = mean_inside(&tilted, likely, width, height)?;
    if score < finding.sure {
        return None;
    }
    let margin = long * short * finding.grow / (2.0 * (long + short));
    tilted.a = [tilted.a[0] - margin, tilted.a[1] + margin];
    tilted.b = [tilted.b[0] - margin, tilted.b[1] + margin];
    let (long, short) = tilted.sides();
    if long.min(short) < THINNEST_LINE {
        return None;
    }
    Some(Found { corners: clockwise(tilted.corners()), score })
}

/// The corners of the smallest shape without dents that holds every point,
/// in order around it.
fn outline(points: &[[i32; 2]]) -> Vec<[i32; 2]> {
    let mut sorted = points.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() < 3 {
        return sorted;
    }
    let turn = |o: [i32; 2], p: [i32; 2], q: [i32; 2]| {
        (p[0] - o[0]) as i64 * (q[1] - o[1]) as i64 - (p[1] - o[1]) as i64 * (q[0] - o[0]) as i64
    };
    let mut hull: Vec<[i32; 2]> = Vec::with_capacity(sorted.len() + 1);
    // Once along the bottom and once back along the top. A corner already
    // settled by the first pass is not taken away by the second.
    for pass in 0..2 {
        let floor = if pass == 0 { 1 } else { hull.len() };
        let order: Box<dyn Iterator<Item = &[i32; 2]>> =
            if pass == 0 { Box::new(sorted.iter()) } else { Box::new(sorted.iter().rev().skip(1)) };
        for &point in order {
            while hull.len() > floor && turn(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0 {
                hull.pop();
            }
            hull.push(point);
        }
    }
    hull.pop();
    hull
}

/// The rectangle of least area around an outline. One of its sides always
/// lies along one of the outline's edges, so every edge is tried.
fn smallest_rectangle(outline: &[[i32; 2]]) -> Option<Tilted> {
    if outline.len() < 3 {
        return None;
    }
    let mut best: Option<(f32, Tilted)> = None;
    for (index, from) in outline.iter().enumerate() {
        let to = outline[(index + 1) % outline.len()];
        let edge = [(to[0] - from[0]) as f32, (to[1] - from[1]) as f32];
        let length = dot(edge, edge).sqrt();
        if length == 0.0 {
            continue;
        }
        let along = [edge[0] / length, edge[1] / length];
        let across = [-along[1], along[0]];
        let (mut a, mut b) = ([f32::MAX, f32::MIN], [f32::MAX, f32::MIN]);
        for point in outline {
            let point = [point[0] as f32, point[1] as f32];
            let (along_it, across_it) = (dot(point, along), dot(point, across));
            a = [a[0].min(along_it), a[1].max(along_it)];
            b = [b[0].min(across_it), b[1].max(across_it)];
        }
        let area = (a[1] - a[0]) * (b[1] - b[0]);
        if best.as_ref().is_none_or(|(least, _)| area < *least) {
            best = Some((area, Tilted { along, across, a, b }));
        }
    }
    best.map(|(_, tilted)| tilted)
}

/// The mean likelihood of the pixels inside a rectangle.
fn mean_inside(tilted: &Tilted, likely: &[f32], width: usize, height: usize) -> Option<f32> {
    let corners = tilted.corners();
    let reach = |axis: usize, limit: usize| {
        let low = corners.iter().map(|c| c[axis]).fold(f32::MAX, f32::min).floor().clamp(0.0, limit as f32 - 1.0);
        let high = corners.iter().map(|c| c[axis]).fold(f32::MIN, f32::max).ceil().clamp(0.0, limit as f32 - 1.0);
        (low as usize, high as usize)
    };
    let ((x0, x1), (y0, y1)) = (reach(0, width), reach(1, height));
    // A pixel on the outline counts as inside.
    let within = |value: f32, range: [f32; 2]| value >= range[0] - 0.5 && value <= range[1] + 0.5;
    let (mut sum, mut count) = (0.0f64, 0u32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let point = [x as f32, y as f32];
            if within(dot(point, tilted.along), tilted.a) && within(dot(point, tilted.across), tilted.b) {
                sum += likely[y * width + x] as f64;
                count += 1;
            }
        }
    }
    (count > 0).then(|| (sum / count as f64) as f32)
}

/// Top-left, top-right, bottom-right, bottom-left, for a line that is roughly
/// level: the two corners further left are its start.
pub(crate) fn clockwise(mut corners: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    corners.sort_by(|p, q| p[0].total_cmp(&q[0]));
    let (mut left, mut right) = ([corners[0], corners[1]], [corners[2], corners[3]]);
    left.sort_by(|p, q| p[1].total_cmp(&q[1]));
    right.sort_by(|p, q| p[1].total_cmp(&q[1]));
    [left[0], right[0], right[1], left[1]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map that is sure of text inside a rectangle turned by `degrees`
    /// about its centre, and sure of none elsewhere.
    fn map_with(width: usize, height: usize, marks: &[([f32; 2], [f32; 2], f32, f32)]) -> Vec<f32> {
        let mut likely = vec![0.02; width * height];
        for (centre, size, degrees, value) in marks {
            let (sin, cos) = degrees.to_radians().sin_cos();
            for y in 0..height {
                for x in 0..width {
                    let (dx, dy) = (x as f32 - centre[0], y as f32 - centre[1]);
                    let (along, across) = (dx * cos + dy * sin, -dx * sin + dy * cos);
                    if along.abs() <= size[0] / 2.0 && across.abs() <= size[1] / 2.0 {
                        likely[y * width + x] = *value;
                    }
                }
            }
        }
        likely
    }

    fn side(p: [f32; 2], q: [f32; 2]) -> f32 {
        ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
    }

    #[test]
    fn a_level_line_is_found_and_widened() {
        let likely = map_with(200, 100, &[([100.0, 50.0], [120.0, 20.0], 0.0, 0.9)]);
        let found = find(&likely, 200, 100, &Finding::default());
        assert_eq!(found.len(), 1);
        let line = found[0];
        assert!((line.score - 0.9).abs() < 0.01, "{}", line.score);
        // 120 by 20 widened by 1.4 * area / outline = 12 pixels on every side.
        assert!((side(line.corners[0], line.corners[1]) - 144.0).abs() < 2.5, "{:?}", line.corners);
        assert!((side(line.corners[0], line.corners[3]) - 44.0).abs() < 2.5, "{:?}", line.corners);
        // Clockwise from the top-left.
        assert!(line.corners[0][0] < line.corners[1][0] && line.corners[0][1] < line.corners[3][1]);
        assert!((line.corners[0][0] - 28.0).abs() < 2.0 && (line.corners[0][1] - 28.0).abs() < 2.0, "{:?}", line.corners);
    }

    #[test]
    fn a_turned_line_keeps_its_angle_and_size() {
        let likely = map_with(300, 200, &[([150.0, 100.0], [160.0, 24.0], 8.0, 0.8)]);
        let found = find(&likely, 300, 200, &Finding::default());
        assert_eq!(found.len(), 1);
        let [top_left, top_right, _, bottom_left] = found[0].corners;
        let angle = (top_right[1] - top_left[1]).atan2(top_right[0] - top_left[0]).to_degrees();
        assert!((angle - 8.0).abs() < 1.0, "angle {angle}");
        let margin = 160.0 * 24.0 * 1.4 / (2.0 * 184.0);
        assert!((side(top_left, top_right) - (160.0 + 2.0 * margin)).abs() < 3.0);
        assert!((side(top_left, bottom_left) - (24.0 + 2.0 * margin)).abs() < 3.0);
    }

    #[test]
    fn lines_close_together_stay_apart() {
        let likely = map_with(
            200,
            120,
            &[([100.0, 30.0], [150.0, 14.0], 0.0, 0.9), ([100.0, 52.0], [150.0, 14.0], 0.0, 0.9), ([100.0, 74.0], [90.0, 14.0], 0.0, 0.9)],
        );
        let mut found = find(&likely, 200, 120, &Finding::default());
        found.sort_by(|p, q| p.corners[0][1].total_cmp(&q.corners[0][1]));
        assert_eq!(found.len(), 3);
        assert!(side(found[2].corners[0], found[2].corners[1]) < side(found[0].corners[0], found[0].corners[1]));
    }

    #[test]
    fn specks_and_faint_marks_are_not_lines() {
        let likely = map_with(
            200,
            100,
            &[
                // A hair, one pixel high.
                ([40.0, 20.0], [30.0, 1.0], 0.0, 0.9),
                // Likely enough to count as inside, not enough to be believed.
                ([120.0, 60.0], [100.0, 20.0], 0.0, 0.3),
            ],
        );
        assert_eq!(find(&likely, 200, 100, &Finding::default()), Vec::new());
    }

    #[test]
    fn a_line_at_the_edge_of_the_map_is_found() {
        let likely = map_with(100, 60, &[([90.0, 30.0], [40.0, 16.0], 0.0, 0.9)]);
        let found = find(&likely, 100, 60, &Finding::default());
        assert_eq!(found.len(), 1);
        // The widened line reaches past the map; the caller keeps it inside the picture.
        assert!(found[0].corners[1][0] > 99.0);
    }

    #[test]
    fn corners_are_put_in_order() {
        let mixed = [[90.0, 12.0], [10.0, 30.0], [92.0, 34.0], [8.0, 10.0]];
        assert_eq!(clockwise(mixed), [[8.0, 10.0], [90.0, 12.0], [92.0, 34.0], [10.0, 30.0]]);
    }

    #[test]
    fn an_outline_has_no_dents_and_no_repeats() {
        let points = [[0, 0], [4, 0], [4, 4], [0, 4], [2, 2], [2, 0], [4, 0]];
        let hull = outline(&points);
        assert_eq!(hull.len(), 4, "{hull:?}");
        for corner in [[0, 0], [4, 0], [4, 4], [0, 4]] {
            assert!(hull.contains(&corner));
        }
    }
}
