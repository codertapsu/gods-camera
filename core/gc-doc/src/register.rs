//! Placing overlapping photos in one frame by the text they share, and
//! keeping each line once.

use std::collections::HashMap;

use gc_align::Affine;

use crate::text::{edit_distance, is_fragment_of, normalise, similarity};
use crate::{OcrLine, OcrPage, PlacedLine};

/// Where every photo sits.
#[derive(Debug, Clone)]
pub struct Placement {
    /// For each photo, the transform from its pixels into its group's frame.
    pub transforms: Vec<Affine>,
    /// Photos that join up, as lists of photo numbers. A photo that shares no
    /// text with any other is a group of its own.
    pub groups: Vec<Vec<usize>>,
}

/// A line used as a landmark.
struct Landmark {
    text: String,
    centre: [f32; 2],
    /// The middles of the line's left and right ends.
    left: [f32; 2],
    right: [f32; 2],
    height: f32,
    /// The photo's edge runs through that end of the line.
    cut_left: bool,
    cut_right: bool,
    /// No other line in the photo reads the same.
    unique: bool,
}

fn centre(quad: &[f32; 8]) -> [f32; 2] {
    [(quad[0] + quad[2] + quad[4] + quad[6]) / 4.0, (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0]
}

fn height(quad: &[f32; 8]) -> f32 {
    let left = ((quad[6] - quad[0]).powi(2) + (quad[7] - quad[1]).powi(2)).sqrt();
    let right = ((quad[4] - quad[2]).powi(2) + (quad[5] - quad[3]).powi(2)).sqrt();
    (left + right) / 2.0
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Which ends of a line a photo's edges run through: left, right, and top or bottom.
fn cut_ends(quad: &[f32; 8], width: f32, height: f32) -> [bool; 3] {
    let (edge_x, edge_y) = (width * 0.012, height * 0.012);
    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];
    [
        xs.iter().any(|x| *x < edge_x),
        xs.iter().any(|x| *x > width - edge_x),
        ys.iter().any(|y| *y < edge_y || *y > height - edge_y),
    ]
}

/// The lines of a photo that can serve as landmarks.
fn landmarks(page: &OcrPage) -> Vec<Landmark> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut all: Vec<Landmark> = page
        .lines
        .iter()
        .filter_map(|line| {
            let q = &line.quad;
            let cut = cut_ends(q, page.width as f32, page.height as f32);
            // Letters sliced by the top or bottom edge are not to be trusted at all.
            (!cut[2]).then(|| Landmark {
                text: normalise(&line.text),
                centre: centre(q),
                left: [(q[0] + q[6]) / 2.0, (q[1] + q[7]) / 2.0],
                right: [(q[2] + q[4]) / 2.0, (q[3] + q[5]) / 2.0],
                height: height(q),
                cut_left: cut[0],
                cut_right: cut[1],
                unique: true,
            })
        })
        .filter(|mark| mark.text.chars().count() >= 6)
        .collect();
    for mark in &all {
        *seen.entry(mark.text.clone()).or_default() += 1;
    }
    for mark in &mut all {
        mark.unique = seen[&mark.text] == 1;
    }
    all
}

/// The first `count` characters of a text, and the last.
fn head(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

fn tail(text: &str, count: usize) -> String {
    let length = text.chars().count();
    text.chars().skip(length.saturating_sub(count)).collect()
}

/// Points that two photos both show, as (point in `a`, point in `b`, line height in `a`).
///
/// A line both photos show whole gives its centre. A line that one photo
/// cuts gives the end both can see: a piece cut off on its right still starts
/// where the whole line starts.
fn shared_points(a: &[Landmark], b: &[Landmark]) -> Vec<([f32; 2], [f32; 2], f32)> {
    let mut points = Vec::new();
    let whole = |mark: &Landmark| !mark.cut_left && !mark.cut_right;
    let by_text: HashMap<&str, &Landmark> =
        a.iter().filter(|mark| mark.unique && mark.text.chars().count() >= 8).map(|mark| (mark.text.as_str(), mark)).collect();
    for mark in b.iter().filter(|mark| mark.unique) {
        if let Some(same) = by_text.get(mark.text.as_str()) {
            if whole(mark) == whole(same) {
                points.push((same.centre, mark.centre, same.height));
            }
        }
    }
    // Pieces against whole lines, in both directions. The last letters before
    // a cut are often misread, so two are left out of the comparison.
    let mut pieces = |cut: &[Landmark], full: &[Landmark], cut_is_b: bool| {
        for piece in cut.iter().filter(|mark| mark.cut_left != mark.cut_right) {
            let length = piece.text.chars().count();
            if length < 8 {
                continue;
            }
            let mut matches = full.iter().filter(|line| {
                !whole(piece) && line.text.chars().count() > length && {
                    if piece.cut_right {
                        !line.cut_left && head(&line.text, length - 2) == head(&piece.text, length - 2)
                    } else {
                        !line.cut_right && tail(&line.text, length - 2) == tail(&piece.text, length - 2)
                    }
                }
            });
            if let (Some(line), None) = (matches.next(), matches.next()) {
                let (in_full, in_cut) = if piece.cut_right { (line.left, piece.left) } else { (line.right, piece.right) };
                points.push(if cut_is_b { (in_full, in_cut, line.height) } else { (in_cut, in_full, piece.height) });
            }
        }
    };
    pieces(b, a, true);
    pieces(a, b, false);
    points
}

/// The transform that takes photo `b`'s pixels onto photo `a`'s, from the
/// lines both show, with the number of lines that agree with it.
fn pair_transform(a: &[Landmark], b: &[Landmark]) -> Option<(Affine, usize)> {
    let mut points = shared_points(a, b);
    // Near matches are a fallback for when two readings of a line differ by a
    // letter. They are risky where lines resemble each other, as in a numbered
    // list, so they are used only when exact matches are too few, only when
    // the digits agree, and only when exactly one line is near.
    if points.len() < 3 {
        let digits = |text: &str| text.chars().filter(char::is_ascii_digit).collect::<String>();
        let whole = |mark: &&Landmark| !mark.cut_left && !mark.cut_right && mark.unique;
        for mark in b.iter().filter(whole) {
            let length = mark.text.chars().count();
            if length < 12 {
                continue;
            }
            let limit = length / 8;
            let mut near = a.iter().filter(whole).filter(|other| {
                other.text != mark.text
                    && digits(&other.text) == digits(&mark.text)
                    && edit_distance(&other.text, &mark.text, limit) <= limit
            });
            if let (Some(other), None) = (near.next(), near.next()) {
                points.push((other.centre, mark.centre, other.height));
            }
        }
    }
    if points.len() < 2 {
        return None;
    }

    // Two points fix a transform; the best one is the one most other points agree with.
    let agrees = |t: &Affine, point: &([f32; 2], [f32; 2], f32)| distance(t.apply(point.1), point.0) < 0.7 * point.2;
    let mut best: Option<(Affine, usize)> = None;
    for first in 0..points.len() {
        for second in first + 1..points.len() {
            let (p, q) = (&points[first], &points[second]);
            // Points too close together give a poor estimate.
            if distance(p.1, q.1) < 2.0 * p.2 {
                continue;
            }
            let Ok(t) = gc_align::estimate_similarity(&[p.1, q.1], &[p.0, q.0]) else { continue };
            if !(0.5..=2.0).contains(&t.scale()) {
                continue;
            }
            let count = points.iter().filter(|point| agrees(&t, point)).count();
            if best.as_ref().is_none_or(|(_, most)| count > *most) {
                best = Some((t, count));
            }
        }
    }
    let (rough, count) = best?;
    // Two agreeing points could be chance; three or more are not.
    if count < 3 && points.len() > 2 {
        return None;
    }
    let inliers: Vec<&([f32; 2], [f32; 2], f32)> = points.iter().filter(|point| agrees(&rough, point)).collect();
    let from: Vec<[f32; 2]> = inliers.iter().map(|point| point.1).collect();
    let to: Vec<[f32; 2]> = inliers.iter().map(|point| point.0).collect();
    let refined = gc_align::estimate_similarity(&from, &to).unwrap_or(rough);
    Some((refined, inliers.len()))
}

/// Work out how the photos sit against each other.
pub fn register(pages: &[OcrPage]) -> Placement {
    let marks: Vec<Vec<Landmark>> = pages.iter().map(landmarks).collect();
    let count = pages.len();
    // links[a] holds (b, transform taking b's pixels onto a's, lines that agree).
    let mut links: Vec<Vec<(usize, Affine, usize)>> = vec![Vec::new(); count];
    for a in 0..count {
        for b in a + 1..count {
            if let Some((b_to_a, agreeing)) = pair_transform(&marks[a], &marks[b]) {
                if let Some(a_to_b) = b_to_a.invert() {
                    links[a].push((b, b_to_a, agreeing));
                    links[b].push((a, a_to_b, agreeing));
                }
            }
        }
    }

    let mut transforms = vec![Affine::IDENTITY; count];
    let mut placed = vec![false; count];
    let mut groups = Vec::new();
    for start in 0..count {
        if placed[start] {
            continue;
        }
        // Grow the group from `start`, always adding the photo with the
        // strongest link to one already placed, so weak links are used last.
        placed[start] = true;
        let mut group = vec![start];
        loop {
            let mut next: Option<(usize, Affine, usize)> = None;
            for &member in &group {
                for (other, other_to_member, agreeing) in &links[member] {
                    if !placed[*other] && next.as_ref().is_none_or(|(_, _, most)| agreeing > most) {
                        next = Some((*other, transforms[member].compose(other_to_member), *agreeing));
                    }
                }
            }
            let Some((page, transform, _)) = next else { break };
            transforms[page] = transform;
            placed[page] = true;
            group.push(page);
        }
        group.sort_unstable();
        groups.push(group);
    }
    Placement { transforms, groups }
}

/// A line in the group's frame, with what is needed to choose between two readings of it.
struct Candidate {
    line: PlacedLine,
    /// The photo's left or right edge runs through the line, so that end of its text is missing.
    cut_left: bool,
    cut_right: bool,
    /// The photo's top or bottom edge runs through the line, so its letters are sliced.
    clipped: bool,
}

impl Candidate {
    fn is_whole(&self) -> bool {
        !(self.cut_left || self.cut_right || self.clipped)
    }

    fn quality(&self) -> f32 {
        // A whole line beats a cut one whatever the confidence, and a sliced one comes last.
        let state = if self.is_whole() { 2.0 } else if self.clipped { -1.0 } else { 0.0 };
        self.line.confidence + state + (self.line.text.chars().count() as f32).min(200.0) / 2000.0
    }
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// Join two cut readings of one line: the end of `left` and the start of
/// `right` show the same words. Returns the whole line, or `None` if the two
/// do not overlap in text.
fn stitch(left: &str, right: &str) -> Option<String> {
    let l: Vec<char> = left.chars().collect();
    let r: Vec<char> = right.chars().collect();
    // An exact overlap is looked for first. Only then are small differences
    // allowed, because a near match can be one letter out of step and would
    // join the two pieces in the wrong place.
    for exact in [true, false] {
        // A cut can leave half a letter, misread, at the very edge; allow dropping a few.
        for trim_left in 0..=3usize.min(l.len()) {
            for trim_right in 0..=3usize.min(r.len()) {
                let l = &l[..l.len() - trim_left];
                let r = &r[trim_right..];
                let longest = l.len().min(r.len());
                for size in (4..=longest).rev() {
                    let tail: String = l[l.len() - size..].iter().collect::<String>().to_lowercase();
                    let head: String = r[..size].iter().collect::<String>().to_lowercase();
                    let matches = if exact {
                        tail == head
                    } else {
                        // The ends must line up, which rules out a match that is out of step.
                        let slack = size / 10;
                        slack > 0
                            && tail.chars().next() == head.chars().next()
                            && tail.chars().last() == head.chars().last()
                            && edit_distance(&tail, &head, slack) <= slack
                    };
                    if matches {
                        let mut whole: String = l.iter().collect();
                        whole.extend(&r[size..]);
                        return Some(whole);
                    }
                }
            }
        }
    }
    None
}

/// A line's corners in the group's frame, the photo it came from, the line
/// itself, and which of its ends that photo cuts.
type Moved<'a> = ([[f32; 2]; 4], usize, &'a OcrLine, [bool; 3]);

/// Put every line of a group into the group's frame and keep each line once.
/// Returns the lines and how many repeated readings were dropped.
pub fn merge(pages: &[OcrPage], placement: &Placement, group: &[usize]) -> (Vec<PlacedLine>, usize) {
    // Corners of every line in the group's frame.
    let mut corners: Vec<Moved<'_>> = Vec::new();
    for &page in group {
        let transform = &placement.transforms[page];
        let (w, h) = (pages[page].width as f32, pages[page].height as f32);
        for line in &pages[page].lines {
            if line.text.trim().is_empty() {
                continue;
            }
            let q = &line.quad;
            let cut = cut_ends(q, w, h);
            let moved = [0, 1, 2, 3].map(|i| transform.apply([q[i * 2], q[i * 2 + 1]]));
            corners.push((moved, page, line, cut));
        }
    }
    if corners.is_empty() {
        return (Vec::new(), 0);
    }

    // Turn the frame so that text runs level: the middle value of the lines' slopes.
    let mut angles: Vec<f32> = corners.iter().map(|(q, ..)| (q[1][1] - q[0][1]).atan2(q[1][0] - q[0][0])).collect();
    angles.sort_by(f32::total_cmp);
    let (sin, cos) = (-angles[angles.len() / 2]).sin_cos();
    let level = |p: [f32; 2]| [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos];

    let mut candidates: Vec<Candidate> = corners
        .into_iter()
        .map(|(quad, page, line, cut)| {
            let q = quad.map(level);
            let xs = q.map(|p| p[0]);
            let ys = q.map(|p| p[1]);
            // The line's own height, not the box's, which a slight tilt would inflate.
            let tall = (distance(q[0], q[3]) + distance(q[1], q[2])) / 2.0;
            let middle = ys.iter().sum::<f32>() / 4.0;
            let bbox = [
                xs.iter().copied().fold(f32::MAX, f32::min),
                middle - tall / 2.0,
                xs.iter().copied().fold(f32::MIN, f32::max),
                middle + tall / 2.0,
            ];
            Candidate {
                line: PlacedLine { text: line.text.trim().to_string(), confidence: line.confidence, bbox, page, title: line.title },
                cut_left: cut[0],
                cut_right: cut[1],
                clipped: cut[2],
            }
        })
        .collect();
    candidates.sort_by(|a, b| b.quality().total_cmp(&a.quality()));

    let mut kept: Vec<Candidate> = Vec::new();
    let mut removed = 0;
    'candidates: for candidate in candidates {
        for held in kept.iter_mut() {
            if held.line.page == candidate.line.page {
                continue;
            }
            let (a, b) = (&held.line.bbox, &candidate.line.bbox);
            let rows = overlap(a[1], a[3], b[1], b[3]) / (a[3] - a[1]).min(b[3] - b[1]).max(1.0);
            let across = overlap(a[0], a[2], b[0], b[2]) / (a[2] - a[0]).min(b[2] - b[0]).max(1.0);
            if rows < 0.5 || across < 0.2 {
                continue;
            }
            // The same row of the page, read from two photos.
            let (held_text, new_text) = (normalise(&held.line.text), normalise(&candidate.line.text));
            let related = similarity(&held_text, &new_text) >= 0.5
                || is_fragment_of(&new_text, &held_text)
                || is_fragment_of(&held_text, &new_text);
            if held.is_whole() {
                if !related && candidate.is_whole() {
                    // Two whole lines with different text: neighbours that the placement
                    // put a little too close. Both stay.
                    continue;
                }
                // The kept reading is complete; this one adds nothing.
            } else if candidate.is_whole() || held.clipped {
                // A complete reading replaces a cut one, and anything replaces a sliced one.
                if related || held.clipped {
                    *held = candidate;
                }
            } else {
                // Both are cut. Each can supply the end the other lacks, joined
                // where their text overlaps.
                if held.cut_right && !candidate.cut_right {
                    if let Some(whole) = stitch(&held.line.text, &candidate.line.text) {
                        held.line.text = whole;
                        held.line.bbox[2] = held.line.bbox[2].max(candidate.line.bbox[2]);
                        held.cut_right = false;
                    }
                }
                if held.cut_left && !candidate.cut_left {
                    if let Some(whole) = stitch(&candidate.line.text, &held.line.text) {
                        held.line.text = whole;
                        held.line.bbox[0] = held.line.bbox[0].min(candidate.line.bbox[0]);
                        held.cut_left = false;
                    }
                }
                held.line.confidence = held.line.confidence.min(candidate.line.confidence);
            }
            removed += 1;
            continue 'candidates;
        }
        kept.push(candidate);
    }
    (kept.into_iter().map(|candidate| candidate.line).collect(), removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn line(text: &str, x: f32, y: f32, width: f32) -> OcrLine {
        OcrLine { text: text.into(), confidence: 0.95, quad: [x, y, x + width, y, x + width, y + 40.0, x, y + 40.0], title: false }
    }

    fn page(name: &str, lines: Vec<OcrLine>) -> OcrPage {
        OcrPage { path: name.into(), width: 1000, height: 1000, lines, error: None }
    }

    #[test]
    fn stitching_joins_two_cut_readings() {
        assert_eq!(
            stitch("Mực nước trên sông Tiền tại Tân", "sông Tiền tại Tân Châu đã lên").as_deref(),
            Some("Mực nước trên sông Tiền tại Tân Châu đã lên")
        );
        // A half letter misread at the cut is dropped.
        assert_eq!(
            stitch("the first passenger ferry tl", "senger ferry to cross").as_deref(),
            Some("the first passenger ferry to cross")
        );
        assert_eq!(stitch("nothing in common here", "với dòng chữ này"), None);
    }

    #[test]
    fn two_photos_of_one_column_join_up() {
        // Photo B starts 300 pixels further down the page than photo A.
        let rows: Vec<String> = (0..12).map(|i| format!("dòng chữ số {i} của trang báo")).collect();
        let a = page("a", (0..8).map(|i| line(&rows[i], 50.0, 100.0 + 60.0 * i as f32, 600.0)).collect());
        let b = page("b", (5..12).map(|i| line(&rows[i], 80.0, 100.0 + 60.0 * (i - 5) as f32, 600.0)).collect());
        let placement = register(&[a.clone(), b.clone()]);
        assert_eq!(placement.groups, vec![vec![0, 1]]);
        // B's top-left lands 30 left and 300 below A's.
        let origin = placement.transforms[1].apply([0.0, 0.0]);
        assert!((origin[0] + 30.0).abs() < 1.0 && (origin[1] - 300.0).abs() < 1.0, "{origin:?}");

        let (lines, removed) = merge(&[a, b], &placement, &placement.groups[0]);
        assert_eq!(removed, 3);
        let mut texts: Vec<(f32, String)> = lines.iter().map(|l| (l.bbox[1], l.text.clone())).collect();
        texts.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(texts.into_iter().map(|t| t.1).collect::<Vec<_>>(), rows);
    }

    #[test]
    fn photos_with_nothing_in_common_stay_apart() {
        let a = page("a", (0..5).map(|i| line(&format!("trang thứ nhất dòng {i}"), 50.0, 100.0 + 60.0 * i as f32, 500.0)).collect());
        let b = page("b", (0..5).map(|i| line(&format!("another page line {i}"), 50.0, 100.0 + 60.0 * i as f32, 500.0)).collect());
        let placement = register(&[a, b]);
        assert_eq!(placement.groups, vec![vec![0], vec![1]]);
    }

    #[test]
    fn a_line_cut_by_both_photos_is_made_whole() {
        // The right photo sits 700 pixels to the right of the left one. Three
        // short lines in the strip both photos show place them against each other.
        let shared: Vec<String> = (0..3).map(|i| format!("cột chung dòng {i}")).collect();
        let mut left: Vec<OcrLine> = shared.iter().enumerate().map(|(i, t)| line(t, 750.0, 100.0 + 60.0 * i as f32, 200.0)).collect();
        // A long line runs off the right edge of the left photo...
        left.push(line("Mực nước trên sông Tiền tại Tân", 400.0, 400.0, 598.0));
        let mut right: Vec<OcrLine> = shared.iter().enumerate().map(|(i, t)| line(t, 50.0, 100.0 + 60.0 * i as f32, 200.0)).collect();
        // ...and begins before the left edge of the right photo.
        right.push(line("sông Tiền tại Tân Châu đã lên", 2.0, 400.0, 600.0));
        let pages = [page("left", left), page("right", right)];
        let placement = register(&pages);
        assert_eq!(placement.groups.len(), 1);
        let (lines, _) = merge(&pages, &placement, &placement.groups[0]);
        assert!(lines.iter().any(|l| l.text == "Mực nước trên sông Tiền tại Tân Châu đã lên"), "{:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
        assert_eq!(lines.len(), 4);
    }
}
