//! The typeface the PDFs carry, and the part of it each PDF needs.
//!
//! The font is bundled so that a PDF shows and copies the same on every
//! machine, whatever fonts that machine has. Each PDF gets only the glyphs
//! its text uses.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, UnicodeCmap};
use pdf_writer::{Filter, Finish, Name, Pdf, Rect, Ref, Str};
use subsetter::GlyphRemapper;
use ttf_parser::GlyphId;

use super::pdf::{compress, Refs};

/// Noto Sans Regular, version 2.015 without hinting, from the Noto project's
/// release NotoSans-v2.015 at github.com/notofonts/latin-greek-cyrillic. It
/// has every Vietnamese letter. Its licence, the SIL Open Font License 1.1 in
/// fonts/OFL.txt, allows it to be bundled and embedded.
const NOTO_SANS: &[u8] = include_bytes!("../../fonts/NotoSans-Regular.ttf");
const POSTSCRIPT_NAME: &str = "NotoSans-Regular";

pub(super) struct Font {
    face: ttf_parser::Face<'static>,
    /// From the font's own units to thousandths of the font size, the unit
    /// PDF measures glyphs in.
    scale: f32,
}

impl Font {
    /// The bundled font, read once.
    pub(super) fn get() -> &'static Font {
        static FONT: OnceLock<Font> = OnceLock::new();
        FONT.get_or_init(|| {
            let face = ttf_parser::Face::parse(NOTO_SANS, 0).expect("the bundled font is a font");
            Font { scale: 1000.0 / f32::from(face.units_per_em()), face }
        })
    }

    fn glyph(&self, c: char) -> u16 {
        self.face.glyph_index(c).map_or(0, |glyph| glyph.0)
    }

    fn units(&self, value: impl Into<f32>) -> f32 {
        value.into() * self.scale
    }

    /// How far a character moves the pen, in thousandths of the font size. A
    /// character the font has no glyph for takes the width of the empty box
    /// drawn in its place.
    pub(super) fn advance(&self, c: char) -> f32 {
        self.units(self.face.glyph_hor_advance(GlyphId(self.glyph(c))).unwrap_or(0))
    }

    /// The width of a text, in thousandths of the font size. Control
    /// characters are not written, so they take no room.
    pub(super) fn width(&self, text: &str) -> f32 {
        text.chars().filter(|c| !c.is_control()).map(|c| self.advance(c)).sum()
    }

    /// How far the font reaches above the baseline, in thousandths of the font size.
    pub(super) fn ascent(&self) -> f32 {
        self.units(self.face.ascender())
    }

    /// How far it reaches below the baseline, as a negative number.
    pub(super) fn descent(&self) -> f32 {
        self.units(self.face.descender())
    }
}

/// The characters a PDF's text uses, each with a number of its own, its CID,
/// in which the pages' text is written.
///
/// Every character gets a number of its own, even one the font has no glyph
/// for and draws as an empty box: the PDF's map from numbers back to text
/// then still gives the character itself when the text is searched or copied.
#[derive(Default)]
pub(super) struct Characters {
    numbers: HashMap<char, u16>,
    /// The characters in the order they were numbered, from 1. Number 0 is
    /// the font's empty glyph and stands for no character.
    list: Vec<char>,
}

impl Characters {
    /// A text as a page's content writes it: two bytes for each character.
    /// Control characters are left out.
    pub(super) fn encode(&mut self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 * text.len());
        for c in text.chars().filter(|c| !c.is_control()) {
            let number = match self.numbers.get(&c) {
                Some(number) => *number,
                // There are numbers for 65 535 characters, far more than any document has.
                None => match u16::try_from(self.list.len() + 1) {
                    Ok(number) => {
                        self.list.push(c);
                        self.numbers.insert(c, number);
                        number
                    }
                    Err(_) => 0,
                },
            };
            out.extend_from_slice(&number.to_be_bytes());
        }
        out
    }

    /// Write the font as the object `id`, with the objects it needs: the
    /// glyphs of these characters, their widths, and the map from each
    /// character's number back to the character.
    pub(super) fn write(&self, pdf: &mut Pdf, id: Ref, refs: &mut Refs) {
        let font = Font::get();
        let [descendant, descriptor, program, to_unicode, glyph_map] = [(); 5].map(|()| refs.next());

        // Only the glyphs used go in: the whole font is 431 KB, a page of text needs a few.
        let glyphs: Vec<u16> = self.list.iter().map(|c| font.glyph(*c)).collect();
        let mut remapper = GlyphRemapper::new();
        for glyph in &glyphs {
            remapper.remap(*glyph);
        }
        let (data, numbers, name) = match subsetter::subset(NOTO_SANS, 0, &remapper) {
            Ok(subset) => {
                let numbers = glyphs.iter().map(|glyph| remapper.get(*glyph).unwrap_or(0)).collect();
                // A font cut down to some of its glyphs is named with six capitals of its own in front.
                (Cow::Owned(subset), numbers, format!("{}+{POSTSCRIPT_NAME}", tag(&self.list)))
            }
            // Should cutting the font down fail, the whole of it goes in, and its glyphs keep their numbers.
            Err(_) => (Cow::Borrowed(NOTO_SANS), glyphs, POSTSCRIPT_NAME.to_string()),
        };
        let name = Name(name.as_bytes());

        pdf.type0_font(id)
            .base_font(name)
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(descendant)
            .to_unicode(to_unicode);

        let mut cid_font = pdf.cid_font(descendant);
        cid_font
            .subtype(CidFontType::Type2)
            .base_font(name)
            .system_info(SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 })
            .font_descriptor(descriptor)
            .default_width(font.units(font.face.glyph_hor_advance(GlyphId(0)).unwrap_or(0)))
            .cid_to_gid_map_stream(glyph_map);
        if !self.list.is_empty() {
            cid_font.widths().consecutive(1, self.list.iter().map(|c| font.advance(*c)));
        }
        cid_font.finish();

        let bbox = font.face.global_bounding_box();
        pdf.font_descriptor(descriptor)
            .name(name)
            // Symbolic: the font has glyphs beyond the standard Latin set.
            .flags(FontFlags::SYMBOLIC)
            .bbox(Rect::new(font.units(bbox.x_min), font.units(bbox.y_min), font.units(bbox.x_max), font.units(bbox.y_max)))
            .italic_angle(font.face.italic_angle())
            .ascent(font.ascent())
            .descent(font.descent())
            .cap_height(font.face.capital_height().map_or(0.7 * font.ascent(), |height| font.units(height)))
            // The usual estimate of the stems' width from the font's weight.
            .stem_v(10.0 + 0.244 * (f32::from(font.face.weight().to_number()) - 50.0))
            .font_file2(program);

        pdf.stream(program, &compress(&data)).filter(Filter::FlateDecode).pair(Name(b"Length1"), data.len() as i32);

        // Each character's number to its glyph's number, two bytes each, from number 0.
        let map: Vec<u8> = std::iter::once(0).chain(numbers).flat_map(u16::to_be_bytes).collect();
        pdf.stream(glyph_map, &compress(&map)).filter(Filter::FlateDecode);

        // Each character's number to the character, which is what search and copying go by.
        let mut cmap = UnicodeCmap::new(Name(b"Custom"), SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"UCS"), supplement: 0 });
        for (index, c) in self.list.iter().enumerate() {
            cmap.pair(index as u16 + 1, *c);
        }
        pdf.stream(to_unicode, &compress(&cmap.finish())).filter(Filter::FlateDecode);
    }
}

/// Six capital letters made from the characters, for the name of a font cut
/// down to them.
fn tag(characters: &[char]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for c in characters {
        hash = (hash ^ u64::from(*c)).wrapping_mul(0x0100_0000_01b3);
    }
    (0..6).map(|place| char::from(b'A' + ((hash >> (8 * place)) % 26) as u8)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_font_has_every_vietnamese_letter() {
        let font = Font::get();
        let lower = "aàáảãạăằắẳẵặâầấẩẫậbcdđeèéẻẽẹêềếểễệghiìíỉĩịklmnoòóỏõọôồốổỗộơờớởỡợpqrstuùúủũụưừứửữựvxyỳýỷỹỵ";
        let missing: String = lower.chars().chain(lower.chars().flat_map(char::to_uppercase)).filter(|c| font.face.glyph_index(*c).is_none()).collect();
        assert_eq!(missing, "");
    }

    #[test]
    fn each_character_gets_one_number() {
        let mut characters = Characters::default();
        assert_eq!(characters.encode("aba"), [0, 1, 0, 2, 0, 1]);
        // A control character is not written; a character the font lacks still gets a number.
        assert_eq!(characters.encode("\u{7}漢b"), [0, 3, 0, 2]);
        assert_eq!(characters.list, ['a', 'b', '漢']);
    }

    #[test]
    fn widths_come_from_the_font() {
        let font = Font::get();
        // In Noto Sans a space is 260 thousandths of the size wide, and "i" is narrower than "m".
        assert_eq!(font.advance(' '), 260.0);
        assert!(font.advance('i') < font.advance('m'));
        assert_eq!(font.width("mi\u{7}"), font.advance('m') + font.advance('i'));
        assert!(font.ascent() > 0.0 && font.descent() < 0.0);
    }
}
