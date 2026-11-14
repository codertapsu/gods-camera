//! A Word document, in the Office Open XML format that Word, LibreOffice,
//! Pages and Google Docs all open.
//!
//! A .docx file is a ZIP archive of XML parts. Only the parts a plain text
//! needs are written: the text, its styles, a few settings and the title.

use std::fmt::Write;

use super::archive::Archive;
use super::{language, nfc, title_or_first_heading};
use crate::{BlockKind, Document};

const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// What each part is, by its name in the archive.
const CONTENT_TYPES: &str = concat!(
    "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">",
    "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>",
    "<Default Extension=\"xml\" ContentType=\"application/xml\"/>",
    "<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>",
    "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>",
    "<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>",
    "<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    "</Types>"
);

/// Where the text and the document's properties are.
const PACKAGE_RELATIONSHIPS: &str = concat!(
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>",
    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>",
    "</Relationships>"
);

/// Where the text's styles and settings are.
const DOCUMENT_RELATIONSHIPS: &str = concat!(
    "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
    "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>",
    "</Relationships>"
);

/// Without the compatibility mode of a current Word, Word opens the file as
/// one from an older version and says so in its title bar.
const SETTINGS: &str = concat!(
    "<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
    "<w:defaultTabStop w:val=\"720\"/>",
    "<w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat>",
    "</w:settings>"
);

/// An A4 page with margins of 2.5 cm, in twentieths of a point.
const SECTION: &str = concat!(
    "<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/>",
    "<w:pgMar w:top=\"1418\" w:right=\"1418\" w:bottom=\"1418\" w:left=\"1418\" w:header=\"709\" w:footer=\"709\" w:gutter=\"0\"/>",
    "</w:sectPr>"
);

/// A Word document: the first heading as Heading 1 and the others as
/// Heading 2, as in the Markdown, and every paragraph as a paragraph, in
/// reading order.
///
/// The title goes into the file's properties, where Word shows it; without
/// one, the first heading serves. The text itself is not changed. The
/// document's language is Vietnamese or English, from its text, so that the
/// spelling checker uses the right dictionary.
pub fn to_docx(document: &Document, title: Option<&str>) -> Vec<u8> {
    let language = language(document.blocks.iter().map(|block| block.text.as_str()));
    let title = title_or_first_heading(document, title);
    let mut archive = Archive::new();
    // The content types come first: some tools recognise the format by them.
    archive.add("[Content_Types].xml", [DECLARATION, CONTENT_TYPES].concat().as_bytes());
    archive.add("_rels/.rels", [DECLARATION, PACKAGE_RELATIONSHIPS].concat().as_bytes());
    archive.add("docProps/core.xml", properties(title.as_deref(), language).as_bytes());
    archive.add("word/_rels/document.xml.rels", [DECLARATION, DOCUMENT_RELATIONSHIPS].concat().as_bytes());
    archive.add("word/document.xml", body(document).as_bytes());
    archive.add("word/styles.xml", styles(language).as_bytes());
    archive.add("word/settings.xml", [DECLARATION, SETTINGS].concat().as_bytes());
    archive.finish()
}

/// Text made safe for XML. The characters XML gives a meaning to are written
/// as references, and control characters are left out: XML 1.0 does not
/// allow them even as references, and Word refuses a file that has one.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\u{FFFE}' | '\u{FFFF}' => {}
            c if c < ' ' => {}
            c => out.push(c),
        }
    }
}

/// One paragraph. A line break or a tab inside the text becomes Word's own
/// mark for it: in Word's text element either would show as a space.
fn paragraph(out: &mut String, style: Option<&str>, text: &str) {
    out.push_str("<w:p>");
    if let Some(style) = style {
        let _ = write!(out, "<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>");
    }
    out.push_str("<w:r>");
    for (row, line) in text.split('\n').enumerate() {
        if row > 0 {
            out.push_str("<w:br/>");
        }
        for (column, piece) in line.trim_end_matches('\r').split('\t').enumerate() {
            if column > 0 {
                out.push_str("<w:tab/>");
            }
            if !piece.is_empty() {
                // Spaces at the ends of a piece are kept, which Word does only when told.
                out.push_str("<w:t xml:space=\"preserve\">");
                escape(piece, out);
                out.push_str("</w:t>");
            }
        }
    }
    out.push_str("</w:r></w:p>");
}

/// The text: one Word paragraph per block.
fn body(document: &Document) -> String {
    let mut out = String::from(DECLARATION);
    let _ = write!(out, "<w:document xmlns:w=\"{WORD}\"><w:body>");
    let mut seen_heading = false;
    for block in &document.blocks {
        let style = match block.kind {
            BlockKind::Heading => {
                let style = if seen_heading { "Heading2" } else { "Heading1" };
                seen_heading = true;
                Some(style)
            }
            BlockKind::Paragraph => None,
        };
        paragraph(&mut out, style, &nfc(&block.text));
    }
    // Word wants a paragraph in the body even when there is no text.
    if document.blocks.is_empty() {
        out.push_str("<w:p/>");
    }
    out.push_str(SECTION);
    out.push_str("</w:body></w:document>");
    out
}

/// The styles: Calibri, which Word and LibreOffice both have or stand in for,
/// and two levels of heading that Word's navigation pane lists. The names
/// "heading 1" and "heading 2" are Word's own, so Word shows them in its
/// language and treats them as its headings.
fn styles(language: &str) -> String {
    let mut out = String::from(DECLARATION);
    let _ = write!(
        out,
        concat!(
            "<w:styles xmlns:w=\"{word}\">",
            "<w:docDefaults>",
            "<w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:eastAsia=\"Calibri\" w:cs=\"Calibri\"/>",
            "<w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/><w:lang w:val=\"{language}\"/></w:rPr></w:rPrDefault>",
            "<w:pPrDefault><w:pPr><w:spacing w:after=\"160\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault>",
            "</w:docDefaults>",
            "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>",
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/>",
            "<w:next w:val=\"Normal\"/><w:uiPriority w:val=\"9\"/><w:qFormat/>",
            "<w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before=\"240\" w:after=\"160\"/><w:outlineLvl w:val=\"0\"/></w:pPr>",
            "<w:rPr><w:b/><w:bCs/><w:color w:val=\"1F3864\"/><w:sz w:val=\"36\"/><w:szCs w:val=\"36\"/></w:rPr></w:style>",
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/>",
            "<w:next w:val=\"Normal\"/><w:uiPriority w:val=\"9\"/><w:unhideWhenUsed/><w:qFormat/>",
            "<w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before=\"280\" w:after=\"80\"/><w:outlineLvl w:val=\"1\"/></w:pPr>",
            "<w:rPr><w:b/><w:bCs/><w:color w:val=\"1F3864\"/><w:sz w:val=\"28\"/><w:szCs w:val=\"28\"/></w:rPr></w:style>",
            "</w:styles>"
        ),
        word = WORD,
        language = language
    );
    out
}

/// The document's properties: its title and language, nothing about who made it.
fn properties(title: Option<&str>, language: &str) -> String {
    let mut out = String::from(DECLARATION);
    out.push_str(concat!(
        "<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" ",
        "xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" ",
        "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">"
    ));
    if let Some(title) = title {
        out.push_str("<dc:title>");
        escape(&nfc(title), &mut out);
        out.push_str("</dc:title>");
    }
    let _ = write!(out, "<dc:language>{language}</dc:language></cp:coreProperties>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, PlacedLine};
    use std::io::Read;

    fn block(kind: BlockKind, text: &str) -> Block {
        let line = PlacedLine { text: text.into(), confidence: 0.9, bbox: [0.0, 0.0, 10.0, 10.0], page: 0, title: false };
        Block { kind, text: text.into(), confidence: 0.9, lines: vec![line] }
    }

    fn document(blocks: Vec<Block>) -> Document {
        Document { blocks, pages: 1, groups: 1, duplicates_removed: 0 }
    }

    /// Every part of the file in the order it was written, read back with the
    /// ZIP and XML libraries of the tests.
    fn unpack(bytes: Vec<u8>) -> Vec<(String, String)> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..zip.len())
            .map(|index| {
                let mut file = zip.by_index(index).unwrap();
                let name = file.name().unwrap().to_string();
                let mut text = String::new();
                file.read_to_string(&mut text).unwrap();
                // Each part must be well-formed XML.
                roxmltree::Document::parse(&text).unwrap_or_else(|error| panic!("{name}: {error}"));
                (name, text)
            })
            .collect()
    }

    fn part<'a>(parts: &'a [(String, String)], name: &str) -> &'a str {
        &parts.iter().find(|(part, _)| part == name).unwrap_or_else(|| panic!("no {name}")).1
    }

    /// The paragraphs of document.xml: each one's style, if it has one, and its text.
    fn paragraphs(xml: &str) -> Vec<(Option<String>, String)> {
        let tree = roxmltree::Document::parse(xml).unwrap();
        tree.descendants()
            .filter(|node| node.has_tag_name((WORD, "p")))
            .map(|p| {
                let style = p
                    .descendants()
                    .find(|node| node.has_tag_name((WORD, "pStyle")))
                    .and_then(|node| node.attribute((WORD, "val")))
                    .map(str::to_string);
                let mut text = String::new();
                for node in p.descendants() {
                    if node.has_tag_name((WORD, "t")) {
                        text.push_str(node.text().unwrap_or_default());
                    } else if node.has_tag_name((WORD, "tab")) {
                        text.push('\t');
                    } else if node.has_tag_name((WORD, "br")) {
                        text.push('\n');
                    }
                }
                (style, text)
            })
            .collect()
    }

    #[test]
    fn headings_and_paragraphs_come_out_in_order_with_their_marks() {
        let bytes = to_docx(
            &document(vec![
                block(BlockKind::Heading, "Mùa nước nổi"),
                block(BlockKind::Paragraph, "Sáng nay mực nước trên sông Tiền tại Tân Châu đã lên thêm mười hai phân."),
                block(BlockKind::Heading, "Chợ nổi Cái Răng"),
                block(BlockKind::Paragraph, "Ghe chở bông điên điển, rau nhút và cá linh non."),
                block(BlockKind::Paragraph, "The ferry runs every twenty minutes."),
            ]),
            None,
        );
        let parts = unpack(bytes);
        let names: Vec<&str> = parts.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/core.xml",
            "word/_rels/document.xml.rels",
            "word/document.xml",
            "word/styles.xml",
            "word/settings.xml"
        ]);
        let heading = |text: &str, level: &str| (Some(level.to_string()), text.to_string());
        let plain = |text: &str| (None, text.to_string());
        assert_eq!(paragraphs(part(&parts, "word/document.xml")), [
            heading("Mùa nước nổi", "Heading1"),
            plain("Sáng nay mực nước trên sông Tiền tại Tân Châu đã lên thêm mười hai phân."),
            heading("Chợ nổi Cái Răng", "Heading2"),
            plain("Ghe chở bông điên điển, rau nhút và cá linh non."),
            plain("The ferry runs every twenty minutes."),
        ]);
        // The headings the text names are defined, and the text is marked as Vietnamese.
        let styles = part(&parts, "word/styles.xml");
        assert!(styles.contains("w:styleId=\"Heading1\"") && styles.contains("w:styleId=\"Heading2\""));
        assert!(styles.contains("<w:lang w:val=\"vi-VN\"/>"));
        // Without a title of its own, the document is called by its first heading.
        let core = roxmltree::Document::parse(part(&parts, "docProps/core.xml")).unwrap();
        let title = core.descendants().find(|node| node.has_tag_name(("http://purl.org/dc/elements/1.1/", "title"))).unwrap();
        assert_eq!(title.text(), Some("Mùa nước nổi"));
    }

    #[test]
    fn characters_xml_gives_a_meaning_to_survive_and_controls_are_dropped() {
        let text = "a < b && c > \"d\" 'e' \u{1}\u{1f}x\ty\nz";
        let parts = unpack(to_docx(&document(vec![block(BlockKind::Paragraph, text)]), Some("Tom & Jerry <1>")));
        assert_eq!(paragraphs(part(&parts, "word/document.xml")), [(None, "a < b && c > \"d\" 'e' x\ty\nz".to_string())]);
        assert!(part(&parts, "docProps/core.xml").contains("<dc:title>Tom &amp; Jerry &lt;1&gt;</dc:title>"));
    }

    #[test]
    fn decomposed_letters_are_composed() {
        // "Hà Nội" with its marks as characters of their own, as some programs write it.
        let decomposed = "Ha\u{300} No\u{323}\u{302}i";
        let parts = unpack(to_docx(&document(vec![block(BlockKind::Paragraph, decomposed)]), None));
        let text = &paragraphs(part(&parts, "word/document.xml"))[0].1;
        assert_eq!(text, "Hà Nội");
        assert_eq!(text.chars().count(), 6, "one character for each letter");
    }

    #[test]
    fn english_text_is_marked_as_english() {
        // A few Vietnamese names do not make a text Vietnamese.
        let text = "The ferry to Cần Thơ runs every twenty minutes from the old landing stage.";
        let parts = unpack(to_docx(&document(vec![block(BlockKind::Paragraph, text)]), None));
        assert!(part(&parts, "word/styles.xml").contains("<w:lang w:val=\"en-US\"/>"));
    }

    #[test]
    fn an_empty_document_is_still_a_document() {
        let parts = unpack(to_docx(&document(Vec::new()), None));
        assert_eq!(paragraphs(part(&parts, "word/document.xml")), [(None, String::new())]);
        assert!(!part(&parts, "docProps/core.xml").contains("<dc:title>"));
    }
}
