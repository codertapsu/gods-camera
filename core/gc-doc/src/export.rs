//! Writing a document out.

use crate::{BlockKind, Document};

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
