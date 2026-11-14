//! The little of the ZIP format a Word file needs: a few parts, each
//! compressed by itself, and the directory at the end that lists them.

use std::io::Write;

use flate2::write::DeflateEncoder;
use flate2::Compression;

/// ZIP 2.0, the version that brought Deflate compression.
const VERSION: u16 = 20;
/// 1 January 1980, the earliest date a ZIP file can hold. The time of writing
/// is not kept, so that the same document always gives the same bytes.
const DATE: u16 = (1 << 5) | 1;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;

/// A ZIP archive being written in memory.
pub(crate) struct Archive {
    out: Vec<u8>,
    directory: Vec<u8>,
    entries: u16,
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

impl Archive {
    pub(crate) fn new() -> Self {
        Self { out: Vec::new(), directory: Vec::new(), entries: 0 }
    }

    /// Add a file. The parts of a document are far below the 4 GB where the
    /// format needs its 64-bit extensions.
    pub(crate) fn add(&mut self, name: &str, data: &[u8]) {
        let crc = crc32fast::hash(data);
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("writing into memory does not fail");
        let compressed = encoder.finish().expect("writing into memory does not fail");
        // A part too small to gain from compression is stored as it is.
        let (method, body) = if compressed.len() < data.len() { (DEFLATED, compressed.as_slice()) } else { (STORED, data) };
        let offset = self.out.len() as u32;
        let name = name.as_bytes();

        // The entry: a header, then the data.
        let out = &mut self.out;
        put32(out, 0x0403_4b50);
        put16(out, VERSION);
        put16(out, 0);
        put16(out, method);
        put16(out, 0);
        put16(out, DATE);
        put32(out, crc);
        put32(out, body.len() as u32);
        put32(out, data.len() as u32);
        put16(out, name.len() as u16);
        put16(out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(body);

        // Its line in the directory, which says where the entry starts.
        let line = &mut self.directory;
        put32(line, 0x0201_4b50);
        put16(line, VERSION);
        put16(line, VERSION);
        put16(line, 0);
        put16(line, method);
        put16(line, 0);
        put16(line, DATE);
        put32(line, crc);
        put32(line, body.len() as u32);
        put32(line, data.len() as u32);
        put16(line, name.len() as u16);
        // No extra fields, no comment, the first disk, no attributes.
        put16(line, 0);
        put16(line, 0);
        put16(line, 0);
        put16(line, 0);
        put32(line, 0);
        put32(line, offset);
        line.extend_from_slice(name);
        self.entries += 1;
    }

    /// The finished archive: the entries, the directory, and the record that
    /// says where the directory is.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        let start = self.out.len() as u32;
        let size = self.directory.len() as u32;
        self.out.append(&mut self.directory);
        let out = &mut self.out;
        put32(out, 0x0605_4b50);
        put16(out, 0);
        put16(out, 0);
        put16(out, self.entries);
        put16(out, self.entries);
        put32(out, size);
        put32(out, start);
        put16(out, 0);
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn another_library_reads_the_parts_back() {
        let text = "Mùa nước nổi năm nay về sớm. ".repeat(40);
        let mut archive = Archive::new();
        archive.add("word/document.xml", text.as_bytes());
        archive.add("tiny", b"x");
        archive.add("empty", b"");
        let bytes = archive.finish();

        // The reader checks every part's checksum as it reads it.
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(zip.len(), 3);
        let mut read = |name: &str| {
            let mut file = zip.by_name(name).unwrap();
            let method = file.compression();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            (method, data)
        };
        assert_eq!(read("word/document.xml"), (zip::CompressionMethod::Deflated, text.into_bytes()));
        assert_eq!(read("tiny"), (zip::CompressionMethod::Stored, b"x".to_vec()));
        assert_eq!(read("empty"), (zip::CompressionMethod::Stored, Vec::new()));
    }
}
