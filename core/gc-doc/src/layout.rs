//! From lines scattered over a page to headings and paragraphs in reading order.
//!
//! The page is cut along its empty strips, the way a reader's eye does: first
//! into bands that run across (a headline above the body), then each band
//! into columns, and so on down until a piece has no strip left to cut along.
//! Pieces are read top to bottom and left to right.

use crate::{Block, BlockKind, PlacedLine};

/// An empty strip across a region must be this many line heights tall to
/// separate two bands. Ordinary paragraph gaps are smaller.
const BAND_GAP: f32 = 1.4;
/// An empty strip down a region must be this many line heights wide to
/// separate two columns.
const COLUMN_GAP: f32 = 0.6;
/// A line this many times taller than the typical one is a heading.
const HEADING_SCALE: f32 = 1.35;
/// A line the reader calls a title is taken as a heading only if it is at
/// least this many times the usual line height. The reader's own judgement is
/// not enough: it marks body lines as titles now and then, and a heading that
/// was not there would cut a paragraph in two.
const TITLE_SCALE: f32 = 1.15;

fn median(mut values: Vec<f32>) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
}

/// Split `members` wherever an empty strip of at least `gap` runs between
/// them along one axis (`axis` 0 cuts into columns, 1 into bands).
fn split(lines: &[PlacedLine], members: &[usize], axis: usize, gap: f32) -> Vec<Vec<usize>> {
    let mut sorted = members.to_vec();
    sorted.sort_by(|a, b| lines[*a].bbox[axis].total_cmp(&lines[*b].bbox[axis]));
    let mut parts: Vec<Vec<usize>> = Vec::new();
    let mut reach = f32::MIN;
    for index in sorted {
        let (start, end) = (lines[index].bbox[axis], lines[index].bbox[axis + 2]);
        match parts.last_mut() {
            Some(part) if start - reach < gap => part.push(index),
            _ => parts.push(vec![index]),
        }
        reach = reach.max(end);
    }
    parts
}

/// Cut a region into pieces in reading order.
fn cut(lines: &[PlacedLine], members: Vec<usize>, line_height: f32, out: &mut Vec<Vec<usize>>, depth: usize) {
    if members.len() <= 1 || depth > 12 {
        out.push(members);
        return;
    }
    let bands = split(lines, &members, 1, BAND_GAP * line_height);
    if bands.len() > 1 {
        for band in bands {
            cut(lines, band, line_height, out, depth + 1);
        }
        return;
    }
    let columns = split(lines, &members, 0, COLUMN_GAP * line_height);
    if columns.len() > 1 {
        for column in columns {
            cut(lines, column, line_height, out, depth + 1);
        }
        return;
    }
    out.push(members);
}

/// Join a paragraph's lines into running text. A word broken across two lines
/// with a hyphen is put back together.
fn join(lines: &[&PlacedLine]) -> String {
    let mut text = String::new();
    for line in lines {
        let next = line.text.trim();
        if text.is_empty() {
            text.push_str(next);
            continue;
        }
        let broken_word = text.ends_with('-')
            && text.chars().rev().nth(1).is_some_and(char::is_alphabetic)
            && next.chars().next().is_some_and(char::is_lowercase);
        if broken_word {
            text.pop();
        } else {
            text.push(' ');
        }
        text.push_str(next);
    }
    text
}

fn block(kind: BlockKind, lines: Vec<&PlacedLine>) -> Block {
    Block {
        kind,
        text: join(&lines),
        confidence: lines.iter().map(|line| line.confidence).fold(1.0, f32::min),
        lines: lines.into_iter().cloned().collect(),
    }
}

/// Arrange the lines of one page into headings and paragraphs in reading order.
pub fn layout(lines: Vec<PlacedLine>) -> Vec<Block> {
    if lines.is_empty() {
        return Vec::new();
    }
    let line_height = median(lines.iter().map(PlacedLine::height).collect()).max(1.0);
    let mut regions = Vec::new();
    cut(&lines, (0..lines.len()).collect(), line_height, &mut regions, 0);

    let mut blocks = Vec::new();
    for mut region in regions {
        region.sort_by(|a, b| lines[*a].bbox[1].total_cmp(&lines[*b].bbox[1]));
        let left = region.iter().map(|i| lines[*i].bbox[0]).fold(f32::MAX, f32::min);
        let right = region.iter().map(|i| lines[*i].bbox[2]).fold(f32::MIN, f32::max);
        // The usual distance between one line's bottom and the next line's top.
        let gaps: Vec<f32> = region.windows(2).map(|pair| lines[pair[1]].bbox[1] - lines[pair[0]].bbox[3]).collect();
        let usual_gap = median(gaps);

        let mut current: Vec<&PlacedLine> = Vec::new();
        let mut current_is_heading = false;
        for (position, &index) in region.iter().enumerate() {
            let line = &lines[index];
            let is_heading = (line.title && line.height() > TITLE_SCALE * line_height)
                || line.height() > HEADING_SCALE * line_height;
            let starts_new = match position.checked_sub(1).map(|p| &lines[region[p]]) {
                None => false,
                Some(previous) => {
                    let gap = line.bbox[1] - previous.bbox[3];
                    let wide_gap = gap > usual_gap + 0.35 * line_height;
                    let indented = line.bbox[0] > left + 0.8 * line_height && previous.bbox[0] <= left + 0.4 * line_height;
                    // A short line that ends a sentence closes its paragraph, gap or not.
                    let previous_ended = previous.bbox[2] < right - 0.25 * (right - left)
                        && previous.text.trim_end().ends_with(['.', '!', '?', '…', ':']);
                    is_heading != current_is_heading || wide_gap || indented || previous_ended
                }
            };
            if starts_new && !current.is_empty() {
                let kind = if current_is_heading { BlockKind::Heading } else { BlockKind::Paragraph };
                blocks.push(block(kind, std::mem::take(&mut current)));
            }
            current_is_heading = is_heading;
            current.push(line);
        }
        if !current.is_empty() {
            let kind = if current_is_heading { BlockKind::Heading } else { BlockKind::Paragraph };
            blocks.push(block(kind, current));
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x: f32, y: f32, width: f32, height: f32) -> PlacedLine {
        PlacedLine { text: text.into(), confidence: 0.9, bbox: [x, y, x + width, y + height], page: 0, title: false }
    }

    #[test]
    fn headline_then_columns_left_to_right() {
        let mut lines = vec![line("Tin trong ngày", 100.0, 50.0, 900.0, 90.0)];
        // Two columns of four lines each, listed in a scrambled order.
        for row in (0..4).rev() {
            lines.push(line(&format!("phải {row}"), 650.0, 220.0 + 60.0 * row as f32, 400.0, 44.0));
            lines.push(line(&format!("trái {row}"), 100.0, 220.0 + 60.0 * row as f32, 400.0, 44.0));
        }
        let blocks = layout(lines);
        assert_eq!(blocks[0].kind, BlockKind::Heading);
        assert_eq!(blocks[0].text, "Tin trong ngày");
        let body: Vec<&str> = blocks[1..].iter().map(|b| b.text.as_str()).collect();
        assert_eq!(body, ["trái 0 trái 1 trái 2 trái 3", "phải 0 phải 1 phải 2 phải 3"]);
    }

    #[test]
    fn a_wider_gap_starts_a_new_paragraph() {
        let lines = vec![
            line("Đoạn một, dòng một", 100.0, 100.0, 500.0, 44.0),
            line("đoạn một, dòng hai.", 100.0, 160.0, 300.0, 44.0),
            line("Đoạn hai, dòng một", 100.0, 250.0, 500.0, 44.0),
            line("đoạn hai, dòng hai.", 100.0, 310.0, 280.0, 44.0),
        ];
        let blocks = layout(lines);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "Đoạn một, dòng một đoạn một, dòng hai.");
        assert_eq!(blocks[1].text, "Đoạn hai, dòng một đoạn hai, dòng hai.");
    }

    #[test]
    fn a_body_line_called_a_title_does_not_cut_its_paragraph() {
        let mut lines = vec![
            line("Ở chợ nổi Cái Răng, ghe chở bông điên điển, rau nhút", 100.0, 100.0, 500.0, 44.0),
            line("và cá linh non đã xuất hiện nhiều hơn. Giá cá linh đầu", 100.0, 160.0, 500.0, 44.0),
            line("mùa dao động quanh một trăm nghìn đồng một ký.", 100.0, 220.0, 420.0, 44.0),
        ];
        // The reader's mistake: the middle line marked as a title, at the size of the others.
        lines[1].title = true;
        let blocks = layout(lines);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
        assert!(blocks[0].text.starts_with("Ở chợ nổi") && blocks[0].text.ends_with("một ký."));
    }

    #[test]
    fn a_larger_line_called_a_title_is_a_heading() {
        let mut lines = vec![
            line("Giá cá đầu mùa", 100.0, 100.0, 300.0, 54.0),
            line("Ở chợ nổi Cái Răng, ghe chở bông điên điển, rau nhút", 100.0, 180.0, 500.0, 44.0),
            line("và cá linh non đã xuất hiện nhiều hơn.", 100.0, 240.0, 380.0, 44.0),
            line("Giá dao động quanh một trăm nghìn đồng một ký, tuỳ", 100.0, 300.0, 500.0, 44.0),
            line("từng buổi chợ.", 100.0, 360.0, 150.0, 44.0),
        ];
        // 54 against 44 is under the size that makes a heading by itself, so the mark decides.
        lines[0].title = true;
        let blocks = layout(lines);
        assert_eq!(blocks[0].kind, BlockKind::Heading);
        assert_eq!(blocks[0].text, "Giá cá đầu mùa");
        assert_eq!(blocks[1].kind, BlockKind::Paragraph);
    }

    #[test]
    fn a_short_closing_line_ends_a_paragraph_without_a_gap() {
        let lines = vec![
            line("The service will run every twenty", 100.0, 100.0, 500.0, 44.0),
            line("minutes.", 100.0, 160.0, 120.0, 44.0),
            line("City engineers spent eighteen", 100.0, 220.0, 500.0, 44.0),
            line("months on it.", 100.0, 280.0, 200.0, 44.0),
        ];
        let blocks = layout(lines);
        assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), [
            "The service will run every twenty minutes.",
            "City engineers spent eighteen months on it."
        ]);
    }

    #[test]
    fn a_hyphenated_word_is_rejoined() {
        let lines = vec![
            line("The landing stages were re-", 100.0, 100.0, 500.0, 44.0),
            line("built in steel", 100.0, 160.0, 500.0, 44.0),
        ];
        assert_eq!(layout(lines)[0].text, "The landing stages were rebuilt in steel");
    }

    #[test]
    fn nothing_in_gives_nothing_out() {
        assert!(layout(Vec::new()).is_empty());
    }
}
