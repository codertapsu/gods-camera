//! Turning text read from many overlapping photos into one document.
//!
//! The photos are placed in one frame by the text itself: a line that two
//! photos both show is a landmark, and a few of those fix how one photo sits
//! against the other. No picture processing is needed, so the same code
//! serves whichever engine read the text.
//!
//! The steps: `register` places the photos, `merge` drops what was read twice,
//! `layout` finds columns and reading order, and the `export` module writes
//! the result.

pub mod export;
pub mod layout;
pub mod register;
pub mod text;

use serde::{Deserialize, Serialize};

/// One line of text as an engine read it from one photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrLine {
    pub text: String,
    /// 0 to 1.
    pub confidence: f32,
    /// Corners in the photo's pixels from its top-left: top-left, top-right,
    /// bottom-right, bottom-left, each as x then y.
    pub quad: [f32; 8],
    /// The engine judged the line to be a heading.
    #[serde(default)]
    pub title: bool,
}

/// Everything read from one photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrPage {
    /// The photo's file name, kept for the record.
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub lines: Vec<OcrLine>,
    #[serde(default)]
    pub error: Option<String>,
}

/// A line placed in the document's frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedLine {
    pub text: String,
    pub confidence: f32,
    /// `[left, top, right, bottom]` in the document's frame.
    pub bbox: [f32; 4],
    /// Which photo it was read from.
    pub page: usize,
    pub title: bool,
}

impl PlacedLine {
    pub fn height(&self) -> f32 {
        self.bbox[3] - self.bbox[1]
    }

    pub fn centre(&self) -> [f32; 2] {
        [(self.bbox[0] + self.bbox[2]) / 2.0, (self.bbox[1] + self.bbox[3]) / 2.0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockKind {
    Heading,
    Paragraph,
}

/// A heading or a paragraph, in reading order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    /// The lowest confidence among the block's lines: where to look first when checking.
    pub confidence: f32,
    pub lines: Vec<PlacedLine>,
}

/// The finished document. `export::to_json` writes it, and it reads back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// How many photos went in.
    pub pages: usize,
    /// Photos that shared no text with any other were laid out on their own;
    /// this counts the separate groups. 1 means every photo joined up.
    pub groups: usize,
    /// Lines that were read from more than one photo and kept once.
    pub duplicates_removed: usize,
}

/// Build one document from the text of many photos.
pub fn build(pages: &[OcrPage]) -> Document {
    let placement = register::register(pages);
    let mut merged: Vec<Vec<PlacedLine>> = Vec::new();
    let mut duplicates_removed = 0;
    for group in &placement.groups {
        let (lines, removed) = register::merge(pages, &placement, group);
        duplicates_removed += removed;
        merged.push(lines);
    }
    // A photo that shows too little text to be placed still repeats lines the
    // others have. Starting from the largest group, a later group keeps only
    // the lines not seen before. The groups keep their order in the document.
    let mut by_size: Vec<usize> = (0..merged.len()).collect();
    by_size.sort_by_key(|group| std::cmp::Reverse(merged[*group].len()));
    let mut seen = std::collections::HashSet::new();
    for group in by_size {
        let before = merged[group].len();
        merged[group].retain(|line| {
            let key = text::normalise(&line.text);
            key.chars().count() < 8 || !seen.contains(&key)
        });
        duplicates_removed += before - merged[group].len();
        for line in &merged[group] {
            seen.insert(text::normalise(&line.text));
        }
    }
    let groups = merged.iter().filter(|lines| !lines.is_empty()).count();
    let blocks = merged.into_iter().flat_map(layout::layout).collect();
    Document { blocks, pages: pages.len(), groups, duplicates_removed }
}
