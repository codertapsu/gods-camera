//! PDFs: the photos with their text laid invisibly over them, so that a
//! photo can be searched and its text copied; and the merged document, set
//! as text on A4 pages.

use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use pdf_writer::types::TextRenderingMode;
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use super::font::{Characters, Font};
use super::photo::{self, Data};
use super::{language, nfc, title_or_first_heading};
use crate::{BlockKind, Document, OcrLine, OcrPage, PlacedLine};

/// An A4 page, in points.
const A4: [f32; 2] = [595.276, 841.89];
/// The names a page's content gives its font and its photo.
const FONT: Name<'static> = Name(b"F0");
const PHOTO: Name<'static> = Name(b"Im0");

/// Data compressed for a PDF stream.
pub(super) fn compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).expect("writing into memory does not fail");
    encoder.finish().expect("writing into memory does not fail")
}

/// Numbers for a PDF's objects, handed out in turn.
pub(super) struct Refs(i32);

impl Refs {
    pub(super) fn next(&mut self) -> Ref {
        self.0 += 1;
        Ref::new(self.0)
    }
}

/// A PDF being written. The font its pages share is written last, once every
/// character on them is known.
struct Writer {
    pdf: Pdf,
    refs: Refs,
    catalog: Ref,
    tree: Ref,
    font: Ref,
    pages: Vec<Ref>,
    characters: Characters,
}

impl Writer {
    fn new() -> Self {
        let mut refs = Refs(0);
        let [catalog, tree, font] = [(); 3].map(|()| refs.next());
        Self { pdf: Pdf::new(), refs, catalog, tree, font, pages: Vec::new(), characters: Characters::default() }
    }

    /// Add a page of the given size with its content, and with its photo if it has one.
    fn page(&mut self, size: [f32; 2], content: Content, photo: Option<Ref>) {
        let [page, contents] = [(); 2].map(|()| self.refs.next());
        let mut writer = self.pdf.page(page);
        writer.media_box(Rect::new(0.0, 0.0, size[0], size[1])).parent(self.tree).contents(contents);
        let mut resources = writer.resources();
        resources.fonts().pair(FONT, self.font);
        if let Some(photo) = photo {
            resources.x_objects().pair(PHOTO, photo);
        }
        resources.finish();
        writer.finish();
        self.pdf.stream(contents, &compress(&content.finish())).filter(Filter::FlateDecode);
        self.pages.push(page);
    }

    fn finish(mut self, title: Option<&str>, language: &str) -> Vec<u8> {
        self.characters.write(&mut self.pdf, self.font, &mut self.refs);
        self.pdf.pages(self.tree).kids(self.pages.iter().copied()).count(self.pages.len() as i32);
        let mut catalog = self.pdf.catalog(self.catalog);
        catalog.pages(self.tree).lang(TextStr(language));
        // The reader's title bar then shows the title rather than the file's name.
        if title.is_some() {
            catalog.viewer_preferences().display_doc_title(true);
        }
        catalog.finish();
        let info = self.refs.next();
        let mut properties = self.pdf.document_info(info);
        properties.producer(TextStr("God's Camera"));
        if let Some(title) = title {
            properties.title(TextStr(title));
        }
        properties.finish();
        self.pdf.finish()
    }
}

/// A PDF with one page for each photo: the photo fills the page, and every
/// line read from it lies over it as invisible text, in the line's place and
/// at its size. The PDF can then be searched, and its text selected and
/// copied, in any PDF reader, with Vietnamese marks intact.
///
/// Each photo comes as its file's bytes with what was read from it. A JPEG
/// goes in as it is; PNG, WebP, TIFF and BMP are decoded and kept without
/// loss. A photo is turned upright as the text readers turn it, so that the
/// lines fall on their words. A picture that cannot be read is an error that
/// says which photo it is.
pub fn to_searchable_pdf(pages: &[(Vec<u8>, OcrPage)]) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err("there are no photos to put in a PDF".into());
    }
    let mut writer = Writer::new();
    for (number, (bytes, page)) in pages.iter().enumerate() {
        let photo = photo::read(bytes).map_err(|error| format!("photo {} ({}): {error}", number + 1, page.path))?;
        // The readers turn a photo upright before reading it, and give the
        // size they read it at. A reader that did not turn it gives the size
        // as stored: the photo is then left as stored too, so that the lines
        // still fall on their words.
        let as_stored = photo.orientation >= 5 && photo.width != photo.height && (page.width, page.height) == (photo.width, photo.height);
        let orientation = if as_stored { 1 } else { photo.orientation };
        let (width, height) = if orientation >= 5 { (photo.height, photo.width) } else { (photo.width, photo.height) };
        // A photo carries no size on paper that can be trusted: phones write
        // 72 dots per inch whatever they photographed. The page gets the
        // photo's proportions, with its longer side as long as A4's.
        let scale = A4[1] / width.max(height) as f32;
        let size = [width as f32 * scale, height as f32 * scale];

        let image = writer.refs.next();
        let data: &[u8] = match &photo.data {
            Data::Jpeg { .. } => bytes,
            Data::Pixels { compressed, .. } => compressed,
        };
        let mut xobject = writer.pdf.image_xobject(image, data);
        xobject.width(photo.width as i32).height(photo.height as i32).bits_per_component(8);
        match &photo.data {
            Data::Jpeg { components, inverted } => {
                xobject.filter(Filter::DctDecode);
                match components {
                    1 => xobject.color_space().device_gray(),
                    3 => xobject.color_space().device_rgb(),
                    _ => xobject.color_space().device_cmyk(),
                }
                if *inverted {
                    xobject.decode([1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
                }
            }
            Data::Pixels { grey, .. } => {
                xobject.filter(Filter::FlateDecode);
                if *grey {
                    xobject.color_space().device_gray();
                } else {
                    xobject.color_space().device_rgb();
                }
            }
        }
        xobject.finish();

        let mut content = Content::new();
        content.save_state();
        content.transform(placement(orientation, photo.width as f32, photo.height as f32, scale));
        content.x_object(PHOTO);
        content.restore_state();
        // The lines' corners are in pixels of the photo as the reader saw it.
        let seen = if page.width > 0 && page.height > 0 { [page.width, page.height] } else { [width, height] };
        let to_page = [size[0] / seen[0] as f32, size[1] / seen[1] as f32];
        for index in reading_order(page) {
            invisible_line(&mut content, &mut writer.characters, &page.lines[index], to_page, size[1]);
        }
        writer.page(size, content, Some(image));
    }
    let language = language(pages.iter().flat_map(|(_, page)| page.lines.iter().map(|line| line.text.as_str())));
    Ok(writer.finish(None, language))
}

/// Where a photo goes on its page. PDF draws a picture into a square of
/// side 1; this matrix takes that square onto the page, turned and mirrored
/// as the photo's Exif orientation says. `width` and `height` are the size
/// as stored, in pixels, and `scale` is points per pixel.
fn placement(orientation: u8, width: f32, height: f32, scale: f32) -> [f32; 6] {
    let (w, h) = (width * scale, height * scale);
    match orientation {
        2 => [-w, 0.0, 0.0, h, w, 0.0],
        3 => [-w, 0.0, 0.0, -h, w, h],
        4 => [w, 0.0, 0.0, -h, 0.0, h],
        5 => [0.0, -w, -h, 0.0, h, w],
        6 => [0.0, -w, h, 0.0, 0.0, w],
        7 => [0.0, w, h, 0.0, 0.0, 0.0],
        8 => [0.0, w, -h, 0.0, h, 0.0],
        _ => [w, 0.0, 0.0, h, 0.0, 0.0],
    }
}

/// The order in which a photo's lines go onto its page: reading order, as
/// the layout finds it. PDF readers select and copy text in the order it is
/// written, so a selection dragged down one column then stays in it.
fn reading_order(page: &OcrPage) -> Vec<usize> {
    let lines: Vec<(usize, &OcrLine)> = page.lines.iter().enumerate().filter(|(_, line)| !line.text.trim().is_empty()).collect();
    if lines.is_empty() {
        return Vec::new();
    }
    // The lines are levelled first by the middle of their slopes, as merging
    // does: boxes from a photo taken at a slant overlap, and hide the gaps
    // between columns.
    let mut slopes: Vec<f32> = lines.iter().map(|(_, line)| (line.quad[3] - line.quad[1]).atan2(line.quad[2] - line.quad[0])).collect();
    slopes.sort_by(f32::total_cmp);
    let (sin, cos) = (-slopes[slopes.len() / 2]).sin_cos();
    let placed = lines
        .iter()
        .map(|(index, line)| {
            let q = &line.quad;
            let level = |i: usize| [q[2 * i] * cos - q[2 * i + 1] * sin, q[2 * i] * sin + q[2 * i + 1] * cos];
            let corners = [0, 1, 2, 3].map(level);
            let xs = corners.map(|p| p[0]);
            let ys = corners.map(|p| p[1]);
            PlacedLine {
                text: line.text.clone(),
                confidence: line.confidence,
                bbox: [xs.into_iter().fold(f32::MAX, f32::min), ys.into_iter().fold(f32::MAX, f32::min), xs.into_iter().fold(f32::MIN, f32::max), ys.into_iter().fold(f32::MIN, f32::max)],
                // The layout carries this through untouched, so it can hold the line's number.
                page: *index,
                title: line.title,
            }
        })
        .collect();
    crate::layout::layout(placed).into_iter().flat_map(|block| block.lines).map(|line| line.page).collect()
}

/// One line read from a photo, as invisible text over it: on the line's
/// baseline, at the size that makes the font as tall as the line, and
/// stretched to the line's length, so that a PDF reader highlights a word it
/// finds where the photo shows it. `scale` takes the photo's pixels to
/// points; the page counts upwards from its foot, `height` points high.
fn invisible_line(content: &mut Content, characters: &mut Characters, line: &OcrLine, scale: [f32; 2], height: f32) {
    let font = Font::get();
    let text = nfc(line.text.trim());
    let natural = font.width(&text) / 1000.0;
    let q = &line.quad;
    let corner = |i: usize| [q[2 * i] * scale[0], height - q[2 * i + 1] * scale[1]];
    let [top_left, top_right, bottom_right, bottom_left] = [0, 1, 2, 3].map(corner);
    let middle = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    // Along the line, from the middle of its left end to the middle of its right end.
    let (left, right) = (middle(top_left, bottom_left), middle(top_right, bottom_right));
    let length = (right[0] - left[0]).hypot(right[1] - left[1]);
    let along = [(right[0] - left[0]) / length, (right[1] - left[1]) / length];
    // Up from the line, square to it.
    let up = [-along[1], along[0]];
    let rise = |top: [f32; 2], bottom: [f32; 2]| (top[0] - bottom[0]) * up[0] + (top[1] - bottom[1]) * up[1];
    let tall = (rise(top_left, bottom_left) + rise(top_right, bottom_right)) / 2.0;
    // The size at which the font, from the foot of its descenders to the top
    // of its ascenders, is as tall as the line. PDF readers highlight found
    // and selected text over that height, so the highlight covers the line.
    let (ascent, descent) = (font.ascent() / 1000.0, font.descent() / 1000.0);
    let size = tall / (ascent - descent);
    // The baseline lies above the line's foot by the font's descent.
    let lift = -descent * size - tall / 2.0;
    let origin = [left[0] + up[0] * lift, left[1] + up[1] * lift];
    let stretch = 100.0 * length / (natural * size);
    let matrix = [along[0], along[1], up[0], up[1], origin[0], origin[1]];
    // A line without text, length or height has nothing to show.
    if !(natural > 0.0 && length > 0.0 && tall > 0.0) || !matrix.iter().chain([&size, &stretch]).all(|value| value.is_finite()) {
        return;
    }
    content.begin_text();
    content.set_text_rendering_mode(TextRenderingMode::Invisible);
    content.set_font(FONT, size);
    content.set_horizontal_scaling(stretch);
    content.set_text_matrix(matrix);
    content.show(Str(&characters.encode(&text)));
    content.end_text();
}

/// How a kind of block is set: its font size in points, its lines' spacing
/// as a multiple of that, the room above and below it in points, and its colour.
#[derive(Clone, Copy)]
struct Style {
    size: f32,
    spacing: f32,
    above: f32,
    below: f32,
    colour: [f32; 3],
    heading: bool,
}

/// The dark blue of the Word file's headings.
const HEADING_COLOUR: [f32; 3] = [0.122, 0.220, 0.392];
const TITLE: Style = Style { size: 20.0, spacing: 1.25, above: 0.0, below: 10.0, colour: HEADING_COLOUR, heading: true };
const SECTION: Style = Style { size: 14.0, spacing: 1.3, above: 14.0, below: 6.0, colour: HEADING_COLOUR, heading: true };
const BODY: Style = Style { size: 11.0, spacing: 1.45, above: 0.0, below: 8.0, colour: [0.0; 3], heading: false };
/// 2.5 cm all round, as in the Word file.
const MARGIN: f32 = 70.866;

/// Break a text into lines no wider than `width` at `size`, between words. A
/// word longer than a whole line, such as a web address, fills each line it
/// needs to the end.
fn wrap(text: &str, size: f32, width: f32) -> Vec<String> {
    let font = Font::get();
    let measure = |text: &str| font.width(text) * size / 1000.0;
    let space = measure(" ");
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let (mut line, mut used) = (String::new(), 0.0);
        for word in paragraph.split_whitespace() {
            let length = measure(word);
            if !line.is_empty() {
                if used + space + length <= width {
                    line.push(' ');
                    line.push_str(word);
                    used += space + length;
                    continue;
                }
                lines.push(std::mem::take(&mut line));
            }
            if length <= width {
                line.push_str(word);
                used = length;
                continue;
            }
            used = 0.0;
            for c in word.chars().filter(|c| !c.is_control()) {
                let advance = font.advance(c) * size / 1000.0;
                if !line.is_empty() && used + advance > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0.0;
                }
                line.push(c);
                used += advance;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

/// The merged document set as text on A4 pages, for documents put together
/// from overlapping photos, where no one photo is the page. The first
/// heading is the largest, the other headings are larger than the text, and
/// the paragraphs are wrapped to the width of the page and run on to further
/// pages as needed, all in the bundled font.
///
/// The title goes into the file's properties, and PDF readers show it in
/// their title bar; without one, the first heading serves.
pub fn to_pdf(document: &Document, title: Option<&str>) -> Vec<u8> {
    let font = Font::get();
    let (ascent, descent) = (font.ascent() / 1000.0, font.descent() / 1000.0);
    let width = A4[0] - 2.0 * MARGIN;
    let top = A4[1] - MARGIN;
    let mut seen_heading = false;
    let blocks: Vec<(Style, Vec<String>)> = document
        .blocks
        .iter()
        .filter(|block| !block.text.trim().is_empty())
        .map(|block| {
            let style = match block.kind {
                BlockKind::Heading if !seen_heading => TITLE,
                BlockKind::Heading => SECTION,
                BlockKind::Paragraph => BODY,
            };
            seen_heading |= style.heading;
            (style, wrap(&nfc(&block.text), style.size, width))
        })
        .collect();

    let mut writer = Writer::new();
    let mut content = Content::new();
    // Where the room for the next line begins, going down the page, and
    // whether anything is on the page yet.
    let (mut y, mut empty) = (top, true);
    for (index, (style, lines)) in blocks.iter().enumerate() {
        let leading = style.size * style.spacing;
        // A block starts on a new page when not even its first line fits,
        // and a heading also when the first line after it would not.
        let mut needed = style.above + leading;
        if style.heading {
            needed += leading * lines.len().saturating_sub(1) as f32;
            if let Some((next, _)) = blocks.get(index + 1) {
                needed += style.below + next.above + next.size * next.spacing;
            }
        }
        if !empty && y - needed < MARGIN {
            writer.page(A4, std::mem::replace(&mut content, Content::new()), None);
            (y, empty) = (top, true);
        }
        if !empty {
            y -= style.above;
        }
        for line in lines {
            // A line that does not fit goes to the top of the next page.
            if !empty && y - leading < MARGIN {
                writer.page(A4, std::mem::replace(&mut content, Content::new()), None);
                y = top;
            }
            // The font's height sits in the middle of the line's room.
            let baseline = y - (leading - (ascent - descent) * style.size) / 2.0 - ascent * style.size;
            content.begin_text();
            content.set_fill_rgb(style.colour[0], style.colour[1], style.colour[2]);
            content.set_font(FONT, style.size);
            content.next_line(MARGIN, baseline);
            content.show(Str(&writer.characters.encode(line)));
            content.end_text();
            y -= leading;
            empty = false;
        }
        y -= style.below;
    }
    writer.page(A4, content, None);
    let language = language(document.blocks.iter().map(|block| block.text.as_str()));
    writer.finish(title_or_first_heading(document, title).as_deref(), language)
}

#[cfg(test)]
mod tests {
    use super::super::photo::tests::{jpeg_file, turned_jpeg_file};
    use super::*;
    use crate::Block;
    use lopdf::content::Content as Operations;
    use lopdf::Object;

    /// A line as a reader gives it: a box `width` by `height` from `x`, `y` in the photo's pixels.
    fn read_line(text: &str, x: f32, y: f32, width: f32, height: f32) -> OcrLine {
        OcrLine { text: text.into(), confidence: 0.9, quad: [x, y, x + width, y, x + width, y + height, x, y + height], title: false }
    }

    fn photo_page(path: &str, width: u32, height: u32, lines: Vec<OcrLine>) -> OcrPage {
        OcrPage { path: path.into(), width, height, lines, error: None }
    }

    /// A small grey PNG, made with another library.
    fn png_file(width: u32, height: u32) -> Vec<u8> {
        let pixels: Vec<u8> = (0..width * height).map(|i| (i % 200) as u8 + 40).collect();
        let mut out = Vec::new();
        image::ImageEncoder::write_image(image::codecs::png::PngEncoder::new(&mut out), &pixels, width, height, image::ExtendedColorType::L8).unwrap();
        out
    }

    fn block(kind: BlockKind, text: &str) -> Block {
        let line = PlacedLine { text: text.into(), confidence: 0.9, bbox: [0.0, 0.0, 10.0, 10.0], page: 0, title: false };
        Block { kind, text: text.into(), confidence: 0.9, lines: vec![line] }
    }

    fn document(blocks: Vec<Block>) -> Document {
        Document { blocks, pages: 1, groups: 1, duplicates_removed: 0 }
    }

    /// The PDF read back with another library.
    fn load(pdf: &[u8]) -> lopdf::Document {
        lopdf::Document::load_mem(pdf).unwrap()
    }

    /// Each page's text, as that library extracts it: one line of text for each text object.
    fn texts(pdf: &[u8]) -> Vec<Vec<String>> {
        let document = load(pdf);
        document
            .get_pages()
            .keys()
            .map(|number| document.extract_text(&[*number]).unwrap().lines().filter(|line| !line.is_empty()).map(str::to_string).collect())
            .collect()
    }

    fn operations(document: &lopdf::Document, page: u32) -> Vec<lopdf::content::Operation> {
        Operations::decode(&document.get_page_content(document.get_pages()[&page])).unwrap().operations
    }

    fn numbers(operands: &[Object]) -> Vec<f32> {
        operands.iter().map(|operand| operand.as_float().unwrap()).collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01 * (1.0 + b.abs())
    }

    /// The page's width and height, checked to start from the corner.
    fn page_size(document: &lopdf::Document, page: u32) -> [f32; 2] {
        let media_box = numbers(document.get_dictionary(document.get_pages()[&page]).unwrap().get(b"MediaBox").unwrap().as_array().unwrap());
        assert_eq!(media_box[..2], [0.0, 0.0]);
        [media_box[2], media_box[3]]
    }

    fn assert_size(document: &lopdf::Document, page: u32, expected: [f32; 2]) {
        let size = page_size(document, page);
        assert!(close(size[0], expected[0]) && close(size[1], expected[1]), "page {page} is {size:?}, not {expected:?}");
    }

    #[test]
    fn each_photo_is_a_page_with_its_lines_in_order() {
        let jpeg = jpeg_file(400, 300);
        let first = vec![
            read_line("Mùa nước nổi", 40.0, 30.0, 200.0, 40.0),
            read_line("Sáng nay mực nước trên sông Tiền tại Tân Châu", 40.0, 100.0, 320.0, 20.0),
            read_line("đã lên thêm mười hai phân so với hôm qua.", 40.0, 130.0, 300.0, 20.0),
        ];
        let second = vec![read_line("Chợ nổi Cái Răng họp từ lúc trời chưa sáng", 20.0, 50.0, 260.0, 18.0)];
        let pdf = to_searchable_pdf(&[(jpeg.clone(), photo_page("a.jpg", 400, 300, first)), (png_file(300, 400), photo_page("b.png", 300, 400, second))]).unwrap();
        assert_eq!(texts(&pdf), [
            vec!["Mùa nước nổi", "Sáng nay mực nước trên sông Tiền tại Tân Châu", "đã lên thêm mười hai phân so với hôm qua."],
            vec!["Chợ nổi Cái Răng họp từ lúc trời chưa sáng"],
        ]);

        let document = load(&pdf);
        // Each page has its photo's proportions, with A4's height on the longer side.
        assert_size(&document, 1, [841.89, 631.42]);
        assert_size(&document, 2, [631.42, 841.89]);
        // The JPEG is in the PDF byte for byte; the PNG was decoded and is kept without loss.
        let pages = document.get_pages();
        let first_images = document.get_page_images(pages[&1]).unwrap();
        assert_eq!(first_images.len(), 1);
        assert_eq!(first_images[0].filters, Some(vec!["DCTDecode".to_string()]));
        assert_eq!(first_images[0].content, jpeg.as_slice());
        let second_images = document.get_page_images(pages[&2]).unwrap();
        assert_eq!(second_images[0].filters, Some(vec!["FlateDecode".to_string()]));
        assert_eq!(second_images[0].color_space.as_deref(), Some("DeviceGray"));
        assert_eq!((second_images[0].width, second_images[0].height), (300, 400));
        // The text is there to be found, not seen.
        assert!(operations(&document, 1).iter().any(|operation| operation.operator == "Tr" && numbers(&operation.operands) == [3.0]));
        let catalog = document.catalog().unwrap();
        assert_eq!(lopdf::decode_text_string(catalog.get(b"Lang").unwrap()).unwrap(), "vi-VN");
    }

    #[test]
    fn the_text_lies_on_its_line() {
        // A photo 1000 by 500, and a line 600 long and 50 high from (100, 200).
        let pdf = to_searchable_pdf(&[(jpeg_file(1000, 500), photo_page("a.jpg", 1000, 500, vec![read_line("Tân Châu", 100.0, 200.0, 600.0, 50.0)]))]).unwrap();
        let document = load(&pdf);
        let operations = operations(&document, 1);
        let operands = |name: &str| operations.iter().find(|operation| operation.operator == name).unwrap().operands.clone();
        let size = numbers(&operands("Tf")[1..])[0];
        let stretch = numbers(&operands("Tz"))[0];
        let matrix = numbers(&operands("Tm"));
        let (scale, height) = (A4[1] / 1000.0, 500.0 * A4[1] / 1000.0);
        let font = Font::get();
        // The text starts at the line's left end, level.
        assert!(close(matrix[0], 1.0) && close(matrix[1], 0.0) && close(matrix[2], 0.0) && close(matrix[3], 1.0), "{matrix:?}");
        assert!(close(matrix[4], 100.0 * scale), "{matrix:?}");
        // From the foot of its descenders to the top of its ascenders, it is as tall as the line.
        let baseline = matrix[5];
        assert!(close(baseline + font.ascent() / 1000.0 * size, height - 200.0 * scale), "{matrix:?} {size}");
        assert!(close(baseline + font.descent() / 1000.0 * size, height - 250.0 * scale), "{matrix:?} {size}");
        // And it is stretched to the line's length.
        assert!(close(font.width("Tân Châu") / 1000.0 * size * stretch / 100.0, 600.0 * scale), "{size} {stretch}");
    }

    #[test]
    fn a_slanting_line_is_laid_along_its_slant() {
        // A line rising to the right by 10 degrees, in a photo whose rows run
        // downwards: along it is (cos, -sin), and up from it is (-sin, -cos).
        let (sin, cos) = 10f32.to_radians().sin_cos();
        let (length, tall) = (500.0, 40.0);
        let start = [100.0, 400.0];
        let quad = [
            start[0] - tall * sin,
            start[1] - tall * cos,
            start[0] - tall * sin + length * cos,
            start[1] - tall * cos - length * sin,
            start[0] + length * cos,
            start[1] - length * sin,
            start[0],
            start[1],
        ];
        let line = OcrLine { text: "nghiêng".into(), confidence: 0.9, quad, title: false };
        let pdf = to_searchable_pdf(&[(jpeg_file(800, 600), photo_page("a.jpg", 800, 600, vec![line]))]).unwrap();
        let document = load(&pdf);
        let matrix = numbers(&operations(&document, 1).into_iter().find(|operation| operation.operator == "Tm").unwrap().operands);
        assert!(close(matrix[0], cos) && close(matrix[1], sin) && close(matrix[2], -sin) && close(matrix[3], cos), "{matrix:?}");
        assert_eq!(texts(&pdf), [vec!["nghiêng"]]);
    }

    #[test]
    fn columns_are_read_one_after_the_other() {
        // Two columns, listed row by row across both, as a reader may list them.
        let mut lines = Vec::new();
        for row in 0..4 {
            let y = 50.0 + 40.0 * row as f32;
            lines.push(read_line(&format!("trái {row}"), 50.0, y, 380.0, 30.0));
            lines.push(read_line(&format!("phải {row}"), 550.0, y, 380.0, 30.0));
        }
        let pdf = to_searchable_pdf(&[(jpeg_file(1000, 300), photo_page("a.jpg", 1000, 300, lines))]).unwrap();
        let expected: Vec<String> = ["trái", "phải"].iter().flat_map(|side| (0..4).map(move |row| format!("{side} {row}"))).collect();
        assert_eq!(texts(&pdf), [expected]);
    }

    #[test]
    fn a_photo_without_lines_is_still_a_page() {
        let blank = vec![read_line("   ", 10.0, 10.0, 50.0, 10.0)];
        let pdf = to_searchable_pdf(&[(jpeg_file(30, 40), photo_page("blank.jpg", 30, 40, Vec::new())), (png_file(30, 40), photo_page("blank.png", 30, 40, blank))]).unwrap();
        let document = load(&pdf);
        assert_eq!(document.get_pages().len(), 2);
        for number in [1, 2] {
            assert_eq!(document.get_page_images(document.get_pages()[&number]).unwrap().len(), 1);
            assert!(operations(&document, number).iter().all(|operation| operation.operator != "Tj"));
        }
    }

    #[test]
    fn a_picture_that_cannot_be_read_is_an_error() {
        let good = (jpeg_file(30, 40), photo_page("good.jpg", 30, 40, vec![read_line("chữ", 2.0, 2.0, 20.0, 8.0)]));
        let unreadable = (b"this is no picture".to_vec(), photo_page("bad.jpg", 30, 40, Vec::new()));
        let error = to_searchable_pdf(&[good.clone(), unreadable]).unwrap_err();
        assert!(error.starts_with("photo 2 (bad.jpg): "), "{error}");
        let cut_short = (jpeg_file(30, 40)[..40].to_vec(), photo_page("cut.jpg", 30, 40, Vec::new()));
        assert!(to_searchable_pdf(&[cut_short, good]).unwrap_err().starts_with("photo 1 (cut.jpg): "));
        assert!(to_searchable_pdf(&[(Vec::new(), photo_page("empty.jpg", 0, 0, Vec::new()))]).is_err());
        assert!(to_searchable_pdf(&[]).is_err());
    }

    #[test]
    fn placement_turns_each_corner_where_exif_says() {
        // Where the stored picture's top-left and top-right corners go, from
        // the Exif rules: for orientation 6, say, the stored top row is the
        // right-hand side and the stored left column is the top.
        let (width, height, scale) = (60.0, 40.0, 2.0);
        for (orientation, top_left, top_right) in [
            (1, "top-left", "top-right"),
            (2, "top-right", "top-left"),
            (3, "bottom-right", "bottom-left"),
            (4, "bottom-left", "bottom-right"),
            (5, "top-left", "bottom-left"),
            (6, "top-right", "bottom-right"),
            (7, "bottom-right", "top-right"),
            (8, "bottom-left", "top-left"),
        ] {
            let (page_width, page_height) = if orientation >= 5 { (height * scale, width * scale) } else { (width * scale, height * scale) };
            let corner = |name: &str| match name {
                "top-left" => [0.0, page_height],
                "top-right" => [page_width, page_height],
                "bottom-right" => [page_width, 0.0],
                _ => [0.0, 0.0],
            };
            let m = placement(orientation, width, height, scale);
            // In the unit square PDF draws a picture into, the stored top row is at the top.
            let apply = |u: f32, v: f32| [m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]];
            assert_eq!(apply(0.0, 1.0), corner(top_left), "orientation {orientation}");
            assert_eq!(apply(1.0, 1.0), corner(top_right), "orientation {orientation}");
        }
    }

    #[test]
    fn a_turned_photo_stands_upright() {
        // Stored 60 wide and 40 high, to be turned a quarter clockwise: the reader read it 40 wide and 60 high.
        let line = vec![read_line("dòng chữ", 5.0, 5.0, 30.0, 8.0)];
        let pdf = to_searchable_pdf(&[(turned_jpeg_file(60, 40, 6), photo_page("turned.jpg", 40, 60, line))]).unwrap();
        let document = load(&pdf);
        assert_size(&document, 1, [561.26, 841.89]);
        let turn = operations(&document, 1).into_iter().find(|operation| operation.operator == "cm").unwrap();
        let expected = placement(6, 60.0, 40.0, A4[1] / 60.0);
        assert!(numbers(&turn.operands).iter().zip(expected).all(|(got, want)| close(*got, want)), "{:?}", turn.operands);
        assert_eq!(texts(&pdf), [vec!["dòng chữ"]]);

        // A reader that read it as stored gets it as stored, so that its lines still fit.
        let pdf = to_searchable_pdf(&[(turned_jpeg_file(60, 40, 6), photo_page("as-stored.jpg", 60, 40, Vec::new()))]).unwrap();
        assert_size(&load(&pdf), 1, [841.89, 561.26]);
    }

    #[test]
    fn characters_the_font_lacks_still_copy_as_themselves() {
        let pdf = to_searchable_pdf(&[(jpeg_file(200, 100), photo_page("a.jpg", 200, 100, vec![read_line("漢字 và 😀", 10.0, 10.0, 150.0, 20.0)]))]).unwrap();
        assert_eq!(texts(&pdf), [vec!["漢字 và 😀"]]);
    }

    #[test]
    fn the_typeset_pdf_has_its_headings_and_paragraphs_in_order() {
        let pdf = to_pdf(
            &document(vec![
                block(BlockKind::Heading, "Mùa nước nổi"),
                block(BlockKind::Paragraph, "Sáng nay nước lên thêm mười hai phân."),
                block(BlockKind::Heading, "Chợ nổi Cái Răng"),
                block(BlockKind::Paragraph, "Ghe chở bông điên điển và cá linh non."),
            ]),
            None,
        );
        assert_eq!(texts(&pdf), [vec!["Mùa nước nổi", "Sáng nay nước lên thêm mười hai phân.", "Chợ nổi Cái Răng", "Ghe chở bông điên điển và cá linh non."]]);
        let document = load(&pdf);
        // The first heading is the largest, the other headings are larger than the text.
        let sizes: Vec<f32> = operations(&document, 1).iter().filter(|operation| operation.operator == "Tf").map(|operation| numbers(&operation.operands[1..])[0]).collect();
        assert_eq!(sizes, [20.0, 11.0, 14.0, 11.0]);
        assert_size(&document, 1, A4);
        // Without a title of its own, the document is called by its first heading.
        let info = document.get_dictionary(document.trailer.get(b"Info").unwrap().as_reference().unwrap()).unwrap();
        assert_eq!(lopdf::decode_text_string(info.get(b"Title").unwrap()).unwrap(), "Mùa nước nổi");
        // Only the glyphs used are in the file.
        let font = document.get_page_fonts(document.get_pages()[&1]).unwrap()[&b"F0".to_vec()];
        let name = font.get(b"BaseFont").unwrap().as_name().unwrap();
        assert!(name.ends_with(b"+NotoSans-Regular") && name.len() == 6 + "+NotoSans-Regular".len(), "{}", String::from_utf8_lossy(name));
        assert!(pdf.len() < 30_000, "{} bytes", pdf.len());
    }

    #[test]
    fn long_paragraphs_wrap_and_run_on_to_further_pages() {
        let sentence = "Mực nước trên sông Tiền tại Tân Châu đã lên thêm mười hai phân so với hôm qua.";
        let long = vec![sentence; 120].join(" ");
        let pdf = to_pdf(&document(vec![block(BlockKind::Heading, "Một đoạn rất dài"), block(BlockKind::Paragraph, &long), block(BlockKind::Paragraph, "Hết.")]), Some("Thử"));
        let pages = texts(&pdf);
        assert!(pages.len() >= 3, "{} pages", pages.len());
        let lines: Vec<&str> = pages.iter().flatten().map(String::as_str).collect();
        // Put back together, the lines give the text word for word.
        assert_eq!(lines[0], "Một đoạn rất dài");
        assert_eq!(lines[1..lines.len() - 1].join(" "), long);
        assert_eq!(lines[lines.len() - 1], "Hết.");
        // Every line fits between the margins, and every page is filled before the next begins.
        let width = A4[0] - 2.0 * MARGIN;
        assert!(lines[1..].iter().all(|line| Font::get().width(line) * 11.0 / 1000.0 <= width));
        assert!(lines[1..].iter().any(|line| Font::get().width(line) * 11.0 / 1000.0 > 0.9 * width));
        let document = load(&pdf);
        for (number, page) in pages.iter().enumerate().take(pages.len() - 1) {
            assert!(page.len() > 35, "page {} has {} lines", number + 1, page.len());
            // No line is set below the bottom margin.
            let baselines: Vec<f32> = operations(&document, number as u32 + 1).iter().filter(|operation| operation.operator == "Td").map(|operation| numbers(&operation.operands)[1]).collect();
            assert!(baselines.iter().all(|y| *y > MARGIN - 4.0), "{baselines:?}");
        }
        let info = document.get_dictionary(document.trailer.get(b"Info").unwrap().as_reference().unwrap()).unwrap();
        assert_eq!(lopdf::decode_text_string(info.get(b"Title").unwrap()).unwrap(), "Thử");
    }

    #[test]
    fn a_word_longer_than_a_line_is_broken_where_the_line_is_full() {
        let address = "https://example.com/".to_string() + &"a".repeat(300);
        let lines = wrap(&format!("Xem {address} nhé"), 11.0, 200.0);
        assert!(lines.len() > 2, "{lines:?}");
        assert_eq!(lines[0], "Xem");
        assert_eq!(lines.concat(), format!("Xem{address}nhé"));
        assert!(lines.iter().all(|line| Font::get().width(line) * 11.0 / 1000.0 <= 200.0));
        // A line break in the text is kept; spaces at the ends are not.
        assert_eq!(wrap("  một\nhai  ", 11.0, 200.0), ["một", "hai"]);
        assert!(wrap("", 11.0, 200.0).is_empty());
    }

    #[test]
    fn an_empty_document_is_one_blank_page() {
        let pdf = to_pdf(&document(Vec::new()), None);
        let document = load(&pdf);
        assert_eq!(document.get_pages().len(), 1);
        assert!(operations(&document, 1).is_empty());
        assert!(document.get_dictionary(document.trailer.get(b"Info").unwrap().as_reference().unwrap()).unwrap().get(b"Title").is_err());
    }
}
