//! Writing a document out: as plain text, Markdown, JSON, a Word file, or a
//! PDF; and the photos themselves as a PDF whose text can be searched.

mod archive;
mod docx;
mod font;
mod pdf;
mod photo;

pub use docx::to_docx;
pub use pdf::{to_pdf, to_searchable_pdf};

use unicode_normalization::UnicodeNormalization;

use crate::{BlockKind, Document};

/// Text with its letters composed: "ộ" as one character rather than "o"
/// followed by two marks. Some programs write the marks apart; composed, the
/// text is found by search and drawn by a font the same way everywhere.
fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// The title a file records: the one given, or else the document's first heading.
fn title_or_first_heading(document: &Document, title: Option<&str>) -> Option<String> {
    let first_heading = || document.blocks.iter().find(|block| block.kind == BlockKind::Heading).map(|block| block.text.as_str());
    title.map(str::trim).filter(|title| !title.is_empty()).or_else(first_heading).map(|title| nfc(title.trim()))
}

/// The language of a text, as the language tag that the files which record
/// one want: spelling checkers and screen readers go by it. Vietnamese when
/// at least one word in five has a letter that only Vietnamese uses, English
/// otherwise. Vietnamese has such a letter in a third to a half of its
/// words; an English text that names a few Vietnamese places stays well below.
fn language<'a>(texts: impl IntoIterator<Item = &'a str>) -> &'static str {
    // ơ and ư, and the letters with tone marks that Unicode has for
    // Vietnamese alone. Letters such as á or ê are not counted: French and
    // Spanish have them too.
    let only_vietnamese = |c: char| matches!(c, 'ơ' | 'Ơ' | 'ư' | 'Ư' | '\u{1EA0}'..='\u{1EF9}');
    let (mut words, mut vietnamese) = (0usize, 0usize);
    for text in texts {
        for word in text.split_whitespace().filter(|word| word.chars().any(char::is_alphabetic)) {
            words += 1;
            vietnamese += usize::from(word.nfc().any(only_vietnamese));
        }
    }
    if words > 0 && vietnamese * 5 >= words {
        "vi-VN"
    } else {
        "en-US"
    }
}

/// Plain text: one block per paragraph, a blank line between blocks.
pub fn to_text(document: &Document) -> String {
    let mut out = String::new();
    for block in &document.blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&block.text);
        out.push('\n');
    }
    out
}

/// Markdown: the first heading is the title, the others are sections.
pub fn to_markdown(document: &Document) -> String {
    let mut out = String::new();
    let mut seen_heading = false;
    for block in &document.blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        match block.kind {
            BlockKind::Heading => {
                out.push_str(if seen_heading { "## " } else { "# " });
                seen_heading = true;
            }
            BlockKind::Paragraph => {}
        }
        // Characters that would start Markdown structure at the beginning of a line are escaped.
        let text = &block.text;
        if block.kind == BlockKind::Paragraph && text.starts_with(['#', '>', '-', '*', '+']) {
            out.push('\\');
        }
        out.push_str(text);
        out.push('\n');
    }
    out
}

/// JSON with every line's place, for the review screen and for other tools.
pub fn to_json(document: &Document) -> String {
    serde_json::to_string_pretty(document).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, PlacedLine};

    fn document() -> Document {
        let line = |text: &str| PlacedLine { text: text.into(), confidence: 0.9, bbox: [0.0, 0.0, 10.0, 10.0], page: 0, title: false };
        let block = |kind, text: &str| Block { kind, text: text.into(), confidence: 0.9, lines: vec![line(text)] };
        Document {
            blocks: vec![
                block(BlockKind::Heading, "Mùa nước nổi"),
                block(BlockKind::Paragraph, "Sáng nay nước lên."),
                block(BlockKind::Heading, "Chợ nổi"),
                block(BlockKind::Paragraph, "- giá cá linh tăng"),
            ],
            pages: 2,
            groups: 1,
            duplicates_removed: 3,
        }
    }

    #[test]
    fn text_has_one_block_per_paragraph() {
        assert_eq!(to_text(&document()), "Mùa nước nổi\n\nSáng nay nước lên.\n\nChợ nổi\n\n- giá cá linh tăng\n");
    }

    #[test]
    fn markdown_marks_headings_and_escapes_list_markers() {
        assert_eq!(
            to_markdown(&document()),
            "# Mùa nước nổi\n\nSáng nay nước lên.\n\n## Chợ nổi\n\n\\- giá cá linh tăng\n"
        );
    }

    #[test]
    fn json_carries_geometry_and_counts() {
        let value: serde_json::Value = serde_json::from_str(&to_json(&document())).unwrap();
        assert_eq!(value["pages"], 2);
        assert_eq!(value["duplicates_removed"], 3);
        assert_eq!(value["blocks"][0]["kind"], "heading");
        assert_eq!(value["blocks"][0]["lines"][0]["bbox"][2], 10.0);
    }
}
