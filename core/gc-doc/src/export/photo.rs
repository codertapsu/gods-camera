//! Photos as a PDF takes them: a JPEG as it is, any other picture decoded.

use std::io::Cursor;

use super::pdf::compress;

/// A picture file larger than this on a side is not opened, as the text
/// reader does not open it either: it is no photo of a page, and decoding it
/// could take all the memory there is.
const LONGEST_SIDE: u32 = 12_000;

/// A photo ready to go into a PDF.
pub(super) struct Photo {
    /// The size as stored, before the photo is turned upright.
    pub width: u32,
    pub height: u32,
    /// How to turn the photo to stand the right way up, as the camera wrote
    /// it in the photo's Exif data: 1 to 8, where 1 is already upright.
    pub orientation: u8,
    pub data: Data,
}

pub(super) enum Data {
    /// A JPEG file's own bytes, which PDF readers decode themselves.
    Jpeg { components: u8, inverted: bool },
    /// Pixels, compressed: one byte a pixel for grey, three for colour.
    Pixels { grey: bool, compressed: Vec<u8> },
}

/// Read a photo file.
pub(super) fn read(bytes: &[u8]) -> Result<Photo, String> {
    if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg(bytes)
    } else {
        decode(bytes)
    }
}

/// What a PDF needs to know about a JPEG, read from the markers before its
/// picture data. The picture data itself is left to the PDF reader.
fn jpeg(bytes: &[u8]) -> Result<Photo, String> {
    let byte = |at: usize| bytes.get(at).copied().ok_or_else(|| "the JPEG file is cut short".to_string());
    let mut place = 2;
    let mut orientation = 1;
    let mut adobe = false;
    let mut frame = None;
    loop {
        if byte(place)? != 0xFF {
            return Err("the JPEG file is damaged".into());
        }
        // A marker may follow any number of padding bytes.
        let mut marker = byte(place + 1)?;
        while marker == 0xFF {
            place += 1;
            marker = byte(place + 1)?;
        }
        place += 2;
        // Markers that stand alone, without a length.
        if matches!(marker, 0x01 | 0xD0..=0xD8) {
            continue;
        }
        if marker == 0xD9 {
            return Err("the JPEG file holds no picture".into());
        }
        let length = usize::from(u16::from_be_bytes([byte(place)?, byte(place + 1)?]));
        let segment = bytes.get(place + 2..place + length.max(2)).ok_or("the JPEG file is cut short")?;
        match marker {
            0xE1 if segment.starts_with(b"Exif\0\0") => orientation = exif_orientation(&segment[6..]).unwrap_or(1),
            // Photoshop writes CMYK inverted, and says so with this marker.
            0xEE if segment.starts_with(b"Adobe") => adobe = true,
            // The frame header of the kinds of JPEG every PDF reader decodes.
            0xC0..=0xC2 => {
                let [precision, height_high, height_low, width_high, width_low, components, ..] = *segment else {
                    return Err("the JPEG file is damaged".into());
                };
                if precision != 8 {
                    return Err(format!("a JPEG with {precision}-bit samples cannot go into a PDF"));
                }
                let height = u32::from(u16::from_be_bytes([height_high, height_low]));
                let width = u32::from(u16::from_be_bytes([width_high, width_low]));
                frame = Some((width, height, components));
            }
            // The other kinds: lossless, hierarchical and arithmetic-coded JPEG.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err("this kind of JPEG (lossless, hierarchical or arithmetic-coded) cannot go into a PDF".into());
            }
            // The picture data starts here; everything needed has been read.
            0xDA => break,
            _ => {}
        }
        place += length;
    }
    let (width, height, components) = frame.ok_or("the JPEG file has no frame header")?;
    if width == 0 || height == 0 {
        return Err("the JPEG file does not give its size".into());
    }
    if !matches!(components, 1 | 3 | 4) {
        return Err(format!("a JPEG with {components} colour channels cannot go into a PDF"));
    }
    Ok(Photo { width, height, orientation, data: Data::Jpeg { components, inverted: adobe && components == 4 } })
}

/// The orientation in a photo's Exif data: which way up the camera was held.
fn exif_orientation(tiff: &[u8]) -> Option<u8> {
    let big_endian = match tiff.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let read16 = |at: usize| -> Option<u16> {
        let bytes = [*tiff.get(at)?, *tiff.get(at.checked_add(1)?)?];
        Some(if big_endian { u16::from_be_bytes(bytes) } else { u16::from_le_bytes(bytes) })
    };
    let read32 = |at: usize| -> Option<u32> {
        let bytes: [u8; 4] = tiff.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if big_endian { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) })
    };
    if read16(2)? != 42 {
        return None;
    }
    // The first directory of tags, each twelve bytes: tag, type, count, value.
    let directory = usize::try_from(read32(4)?).ok()?;
    let count = usize::from(read16(directory)?);
    for entry in 0..count {
        let at = directory.checked_add(2 + 12 * entry)?;
        if read16(at)? == 0x0112 {
            let value = read16(at.checked_add(8)?)?;
            return u8::try_from(value).ok().filter(|value| (1..=8).contains(value));
        }
    }
    None
}

/// Any other picture the text reader takes, decoded and turned upright the
/// way the reader turns it. Its pixels are kept without loss.
fn decode(bytes: &[u8]) -> Result<Photo, String> {
    use image::ImageDecoder;
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(LONGEST_SIDE);
    limits.max_image_height = Some(LONGEST_SIDE);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    picture.apply_orientation(orientation);
    let (width, height) = (picture.width(), picture.height());
    if width == 0 || height == 0 {
        return Err("the picture is empty".into());
    }
    // Transparent parts are laid on white, as on paper.
    let on_white = |value: u8, alpha: u8| ((u32::from(value) * u32::from(alpha) + 255 * (255 - u32::from(alpha)) + 127) / 255) as u8;
    let grey = !picture.color().has_color();
    let pixels = match (grey, picture.color().has_alpha()) {
        (true, false) => picture.to_luma8().into_raw(),
        (false, false) => picture.to_rgb8().into_raw(),
        (true, true) => picture.to_luma_alpha8().as_chunks::<2>().0.iter().map(|[value, alpha]| on_white(*value, *alpha)).collect(),
        (false, true) => picture
            .to_rgba8()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|[red, green, blue, alpha]| [red, green, blue].map(|value| on_white(*value, *alpha)))
            .collect(),
    };
    Ok(Photo { width, height, orientation: 1, data: Data::Pixels { grey, compressed: compress(&pixels) } })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A small JPEG, made with another library.
    pub(crate) fn jpeg_file(width: u32, height: u32) -> Vec<u8> {
        let pixels: Vec<u8> = (0..width * height).flat_map(|i| [(i % 251) as u8, 128, (i % 13 * 19) as u8]).collect();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
            .encode(&pixels, width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    /// A JPEG with an Exif block that says how to turn it.
    pub(crate) fn turned_jpeg_file(width: u32, height: u32, orientation: u16) -> Vec<u8> {
        let plain = jpeg_file(width, height);
        // Big-endian TIFF header, then one directory with the one tag.
        let mut tiff = b"MM\x00\x2a\x00\x00\x00\x08\x00\x01".to_vec();
        tiff.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01]);
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut segment = b"Exif\0\0".to_vec();
        segment.extend_from_slice(&tiff);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend_from_slice(&(segment.len() as u16 + 2).to_be_bytes());
        out.extend_from_slice(&segment);
        out.extend_from_slice(&plain[2..]);
        out
    }

    #[test]
    fn a_jpeg_is_read_from_its_header() {
        let photo = read(&jpeg_file(40, 30)).unwrap();
        assert_eq!((photo.width, photo.height, photo.orientation), (40, 30, 1));
        assert!(matches!(photo.data, Data::Jpeg { components: 3, inverted: false }));
        let turned = read(&turned_jpeg_file(40, 30, 6)).unwrap();
        assert_eq!((turned.width, turned.height, turned.orientation), (40, 30, 6));
    }

    #[test]
    fn exif_in_either_byte_order_gives_the_orientation() {
        let little = b"II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x08\x00\x00\x00";
        assert_eq!(exif_orientation(little), Some(8));
        let big = b"MM\x00\x2a\x00\x00\x00\x08\x00\x01\x01\x12\x00\x03\x00\x00\x00\x01\x00\x03\x00\x00";
        assert_eq!(exif_orientation(big), Some(3));
        // A directory that claims more tags than it has, or a value out of range.
        assert_eq!(exif_orientation(b"MM\x00\x2a\x00\x00\x00\x08\x00\x09\x01"), None);
        assert_eq!(exif_orientation(b"II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x09\x00\x00\x00"), None);
    }

    #[test]
    fn a_png_with_transparency_is_laid_on_white() {
        // Two pixels: opaque black, and black with no opacity at all.
        let mut file = Vec::new();
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new(&mut file),
            &[0, 0, 0, 255, 0, 0, 0, 0],
            2,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        let photo = read(&file).unwrap();
        let Data::Pixels { grey: false, compressed } = photo.data else { panic!("expected colour pixels") };
        let mut pixels = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(compressed.as_slice()), &mut pixels).unwrap();
        assert_eq!(pixels, [0, 0, 0, 255, 255, 255]);
    }

    #[test]
    fn damaged_files_are_errors() {
        let whole = jpeg_file(16, 16);
        for bytes in [&b""[..], b"not a picture at all", &whole[..20], &[0xFF, 0xD8, 0xFF, 0xD9], &[0xFF, 0xD8, 0x00]] {
            assert!(read(bytes).is_err(), "{bytes:?}");
        }
        // A segment whose length runs past the end of the file.
        assert!(read(&[0xFF, 0xD8, 0xFF, 0xE0, 0xFF, 0xF0, 0x00]).is_err());
        // A 12-bit JPEG: the frame header says so before any picture data.
        let mut twelve = whole.clone();
        let frame = twelve.windows(2).position(|pair| pair == [0xFF, 0xC0]).unwrap();
        twelve[frame + 4] = 12;
        assert_eq!(read(&twelve).err().unwrap(), "a JPEG with 12-bit samples cannot go into a PDF");
    }
}
