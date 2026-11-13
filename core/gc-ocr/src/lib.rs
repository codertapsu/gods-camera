//! Reading the text in pictures of a page.
//!
//! This is the text reader for systems that have none of their own that
//! knows Vietnamese. The Mac app reads with Apple's Vision, through the small
//! Swift tool in `apps/apple/tools/gc-ocr`; the program built from this crate
//! takes the same arguments and prints the same thing, so the hub can use
//! either. Windows' built-in reader does not know Vietnamese.
//!
//! Two models work in turn, both on the processor, so that a picture reads
//! the same on every machine:
//!
//! - **PP-OCRv6's small detector** finds the lines, whatever the language
//!   (see [`lines`]).
//! - **AsterNet** reads each line. It was trained for Vietnamese and reads
//!   English as well. PP-OCR's own recognisers, version 5 "Latin" and version
//!   6, cannot write Vietnamese: their character lists lack nearly every
//!   letter with a tone mark, whatever their papers say.
//!
//! `tools/ocrkit/read_pages.py` does the same steps in Python and is the
//! reference: it is where the models were chosen and the settings measured.

pub mod lines;
pub mod picture;
pub mod reading;

use std::path::{Path, PathBuf};

use gc_doc::{OcrLine, OcrPage};
use gc_infer::runtime::{InferError, Model, Runtime};

pub use lines::Finding;
pub use picture::Picture;
use reading::Alphabet;

/// The model files, by the names they have in the model store.
pub const DETECTOR_FILE: &str = "PP-OCRv6_small_det.onnx";
pub const READER_FILE: &str = "asternet_vi.onnx";
pub const ALPHABET_FILE: &str = "asternet_vi.vocab.json";

/// The recogniser looks at lines this high,
const LINE_HEIGHT: usize = 32;
/// and no wider than this: it has 256 steps of four pixels each.
const WIDEST_LINE: usize = 1024;
/// A piece of a line narrower than this holds no character.
const NARROWEST_PIECE: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The detector sees the picture no larger than this on its longer side.
    /// A larger picture is made smaller for finding the lines; they are read
    /// from the picture at its full size.
    pub longest: usize,
    pub finding: Finding,
    /// A line read with less confidence than this is dropped: a shadow or a
    /// stain taken for text reads as nonsense, and the reader knows it.
    pub least_sure: f32,
    /// Processor threads one model run may use. The reader shares the
    /// computer with the live picture, so it does not take every core.
    pub threads: usize,
    /// What is done about a speckled scan. `None` reads every picture as it is.
    pub cleaning: Option<Cleaning>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { longest: 1920, finding: Finding::default(), least_sure: 0.5, threads: 4, cleaning: Some(Cleaning::default()) }
    }
}

/// How a speckled scan is cleaned before it is read.
///
/// A tablet's document scanner turns a page to black and white. In poor light
/// the grain of the photo then comes out as specks all over the paper, and
/// the recogniser reads them as full stops, commas and tone marks: the first
/// page a tablet sent came out at 93 % where a photo of a page reads at 99 %.
/// Such a picture is blurred a little and its brightness is stretched (see
/// [`Picture::cleaned`]); the lines are then found in the cleaned picture and
/// read from it.
///
/// Only a speckled picture is cleaned. The stretch costs a clean photo a few
/// characters in a thousand, and the blur would wash small writing out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cleaning {
    /// A picture is cleaned when at least this share of its pixels are
    /// specks, as [`Picture::speckle`] counts them. The tablet's page had
    /// 0.055; photos and pages drawn by a program have under 0.01.
    pub speckled: f32,
    /// And when its lines are usually at least this many pixels high. In
    /// smaller writing a stroke is no wider than a speck, and what takes the
    /// one away takes the other; and a sharp picture of small writing, a
    /// screenshot, has thin strokes that count as specks.
    pub smallest_line: f32,
    /// The blur, as a share of the usual line's height.
    pub blur: f32,
    /// After the blur, what is darker than this turns black,
    pub ink: f32,
    /// and what is lighter than this turns white. Both from 0 to 1.
    pub paper: f32,
}

impl Default for Cleaning {
    fn default() -> Self {
        Self { speckled: 0.02, smallest_line: 32.0, blur: 0.02, ink: 0.3, paper: 0.7 }
    }
}

#[derive(Debug)]
pub enum ReadError {
    /// A model file is in none of the folders given.
    Missing(String),
    /// The character list does not belong to the recogniser.
    Alphabet(String),
    Model(InferError),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Missing(name) => write!(f, "the model file {name} was not found"),
            ReadError::Alphabet(message) => write!(f, "{message}"),
            ReadError::Model(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ReadError {}

impl From<InferError> for ReadError {
    fn from(error: InferError) -> Self {
        ReadError::Model(error)
    }
}

/// How long the two models took, for measuring.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Timing {
    pub pictures: usize,
    /// How many of the pictures were speckled scans, cleaned before they were read.
    pub cleaned: usize,
    pub lines: usize,
    pub finding: std::time::Duration,
    pub reading: std::time::Duration,
}

/// The text reader, with its models loaded.
pub struct TextReader {
    detector: Model,
    recogniser: Model,
    alphabet: Alphabet,
    settings: Settings,
    timing: Timing,
    /// Where the pieces of lines are kept as pictures, and how many there are.
    kept: Option<(PathBuf, usize)>,
}

/// How high a line is at its left end.
fn height_of(quad: &[[f32; 2]; 4]) -> f32 {
    ((quad[0][0] - quad[3][0]).powi(2) + (quad[0][1] - quad[3][1]).powi(2)).sqrt()
}

/// The lines' corners kept inside the picture, which is what is reported (a
/// line is cut out where it was found), and how high a line usually is.
fn placed(found: &[[[f32; 2]; 4]], picture: &Picture) -> (Vec<[[f32; 2]; 4]>, f32) {
    let (right, bottom) = (picture.width as f32 - 1.0, picture.height as f32 - 1.0);
    let inside: Vec<[[f32; 2]; 4]> = found.iter().map(|corners| corners.map(|c| [c[0].clamp(0.0, right), c[1].clamp(0.0, bottom)])).collect();
    let mut heights: Vec<f32> = inside.iter().map(height_of).collect();
    heights.sort_by(f32::total_cmp);
    let usual = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    (inside, usual)
}

/// The first of `folders` that holds `name`.
fn locate(folders: &[PathBuf], name: &str) -> Result<PathBuf, ReadError> {
    folders.iter().map(|folder| folder.join(name)).find(|file| file.is_file()).ok_or_else(|| ReadError::Missing(name.to_string()))
}

impl TextReader {
    /// The model file that is in none of `folders`, if any: what to tell the
    /// user is missing, without loading anything.
    pub fn missing_file(folders: &[PathBuf]) -> Option<String> {
        [DETECTOR_FILE, READER_FILE, ALPHABET_FILE].into_iter().find(|name| locate(folders, name).is_err()).map(str::to_string)
    }

    /// Load the models from the folders of the model store.
    pub fn load(folders: &[PathBuf], settings: Settings) -> Result<Self, ReadError> {
        let alphabet_file = locate(folders, ALPHABET_FILE)?;
        let alphabet = std::fs::read_to_string(&alphabet_file)
            .map_err(|error| ReadError::Alphabet(format!("{}: {error}", alphabet_file.display())))
            .and_then(|text| Alphabet::from_json(&text).map_err(ReadError::Alphabet))?;
        let runtime = Runtime::shared(None)?;
        let detector = Model::load(runtime, &locate(folders, DETECTOR_FILE)?, settings.threads)?;
        let recogniser = Model::load(runtime, &locate(folders, READER_FILE)?, settings.threads)?;
        Ok(Self { detector, recogniser, alphabet, settings, timing: Timing::default(), kept: None })
    }

    /// Keep every piece of a line exactly as the recogniser is shown it, as
    /// `line-001.png` and so on in `folder`: for looking at when a reading is
    /// wrong, and for comparing two machines.
    pub fn keep_lines(&mut self, folder: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        self.kept = Some((folder.to_path_buf(), 0));
        Ok(())
    }

    pub fn timing(&self) -> Timing {
        self.timing
    }

    /// Read a picture file. A file that cannot be read gives a page with its
    /// `error` set and no lines, as the Mac's reader does.
    pub fn read_file(&mut self, path: &Path) -> OcrPage {
        let mut page = OcrPage { path: path.to_string_lossy().into_owned(), width: 0, height: 0, lines: Vec::new(), error: None };
        let picture = match Picture::open(path) {
            Ok(picture) => picture,
            Err(_) => {
                page.error = Some("not a readable picture".into());
                return page;
            }
        };
        (page.width, page.height) = (picture.width as u32, picture.height as u32);
        match self.read(&picture) {
            Ok(lines) => page.lines = lines,
            Err(error) => page.error = Some(error.to_string()),
        }
        page
    }

    /// Every line of text in a picture, from the top down.
    pub fn read(&mut self, picture: &Picture) -> Result<Vec<OcrLine>, ReadError> {
        let started = std::time::Instant::now();
        let mut found = self.find_lines(picture)?;
        let (mut inside, mut usual) = placed(&found, picture);
        // A speckled scan is cleaned, and its lines are looked for again:
        // they too are found better without the specks.
        let cleaned;
        let mut picture = picture;
        if let Some(cleaning) = self.settings.cleaning {
            if usual >= cleaning.smallest_line && picture.speckle() >= cleaning.speckled {
                // To the twentieth of a pixel, so that a line height that
                // differs in its last digits gives the same picture.
                let blur = (cleaning.blur * usual * 20.0).round() / 20.0;
                cleaned = picture.cleaned(blur, cleaning.ink, cleaning.paper);
                picture = &cleaned;
                found = self.find_lines(picture)?;
                (inside, usual) = placed(&found, picture);
                self.timing.cleaned += 1;
            }
        }
        self.timing.pictures += 1;
        self.timing.finding += started.elapsed();
        let started = std::time::Instant::now();

        let mut lines = Vec::new();
        for (corners, quad) in found.iter().zip(&inside) {
            let tall = height_of(quad);
            // The sliver of a line that the picture's top or bottom edge left over reads as nonsense.
            let at_the_edge = quad.iter().any(|c| c[1] < 0.012 * picture.height as f32 || c[1] > 0.988 * picture.height as f32);
            if at_the_edge && tall < 0.7 * usual {
                continue;
            }
            let line = picture.straightened(corners);
            // Text that runs down the page is not read.
            if line.height as f32 > 1.5 * line.width as f32 {
                continue;
            }
            let (text, confidence) = self.read_line(&line)?;
            if text.is_empty() || confidence < self.settings.least_sure {
                continue;
            }
            lines.push(OcrLine {
                text,
                confidence,
                quad: [quad[0][0], quad[0][1], quad[1][0], quad[1][1], quad[2][0], quad[2][1], quad[3][0], quad[3][1]],
                title: tall > 1.25 * usual,
            });
        }
        self.timing.lines += lines.len();
        self.timing.reading += started.elapsed();
        Ok(lines)
    }

    /// The corners of every line in a picture, in the picture's pixels, from
    /// the top down. A line at the picture's edge can reach past it.
    fn find_lines(&mut self, picture: &Picture) -> Result<Vec<[[f32; 2]; 4]>, ReadError> {
        let scale = (self.settings.longest as f32 / picture.width.max(picture.height) as f32).min(1.0);
        // The detector halves the picture five times: both sides must divide by 32.
        let fitted = |length: usize| (((length as f32 * scale / 32.0).round() as usize) * 32).max(32);
        let (width, height) = (fitted(picture.width), fitted(picture.height));
        let resized;
        let small = if (width, height) == (picture.width, picture.height) {
            picture
        } else {
            resized = picture.resized(width, height);
            &resized
        };
        // Blue, green and red planes. PP-OCR takes its usual means and
        // deviations in that order, as they stand: the first pair goes to blue.
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const DEVIATION: [f32; 3] = [0.229, 0.224, 0.225];
        let plane = width * height;
        let mut input = vec![0.0f32; 3 * plane];
        for (index, pixel) in small.rgb.as_chunks::<3>().0.iter().enumerate() {
            for (channel, colour) in [pixel[2], pixel[1], pixel[0]].into_iter().enumerate() {
                input[channel * plane + index] = (colour as f32 / 255.0 - MEAN[channel]) / DEVIATION[channel];
            }
        }
        let (_, likely) = self.detector.run([1, 3, height, width], input)?;
        if likely.len() != plane {
            return Err(ReadError::Model(InferError::Model(format!("the line detector returned {} numbers for {plane} pixels", likely.len()))));
        }
        let (across, down) = (picture.width as f32 / width as f32, picture.height as f32 / height as f32);
        let mut found: Vec<[[f32; 2]; 4]> = lines::find(&likely, width, height, &self.settings.finding)
            .into_iter()
            .map(|line| line.corners.map(|corner| [corner[0] * across, corner[1] * down]))
            .collect();
        // Rows of ten pixels from the top, and from the left within a row.
        let place = |corners: &[[f32; 2]; 4]| {
            let middle = corners.iter().map(|c| c[1]).sum::<f32>() / 4.0;
            let left = corners.iter().map(|c| c[0]).fold(f32::MAX, f32::min);
            ((middle / 10.0).round() as i64, left)
        };
        found.sort_by(|p, q| {
            let (p, q) = (place(p), place(q));
            p.0.cmp(&q.0).then(p.1.total_cmp(&q.1))
        });
        Ok(found)
    }

    /// The text of one levelled line, and how sure the recogniser is of it, 0 to 1.
    fn read_line(&mut self, line: &Picture) -> Result<(String, f32), ReadError> {
        let wide = ((line.width as f32 * LINE_HEIGHT as f32 / line.height as f32).round() as usize).max(LINE_HEIGHT);
        let small = line.resized(wide, LINE_HEIGHT);
        let (mut texts, mut sure) = (Vec::new(), Vec::new());
        for edges in reading::cuts(&small, WIDEST_LINE).windows(2) {
            if edges[1] - edges[0] < NARROWEST_PIECE {
                continue;
            }
            let mut piece = small.columns(edges[0], edges[1]);
            if piece.width > WIDEST_LINE {
                piece = piece.resized(WIDEST_LINE, LINE_HEIGHT);
            }
            if let Some((folder, count)) = &mut self.kept {
                *count += 1;
                let file = folder.join(format!("line-{count:03}.png"));
                let _ = image::save_buffer(file, &piece.rgb, piece.width as u32, LINE_HEIGHT as u32, image::ExtendedColorType::Rgb8);
            }
            // Red, green and blue planes, 0 to 1.
            let plane = piece.width * LINE_HEIGHT;
            let mut input = vec![0.0f32; 3 * plane];
            for (index, pixel) in piece.rgb.as_chunks::<3>().0.iter().enumerate() {
                for channel in 0..3 {
                    input[channel * plane + index] = pixel[channel] as f32 / 255.0;
                }
            }
            let (shape, scores) = self.recogniser.run([1, 3, LINE_HEIGHT, piece.width], input)?;
            if shape.last() != Some(&self.alphabet.len()) {
                return Err(ReadError::Alphabet(format!(
                    "the recogniser knows {} characters and its list has {}",
                    shape.last().copied().unwrap_or(0),
                    self.alphabet.len()
                )));
            }
            let (text, each) = reading::decode(&scores, &self.alphabet);
            let text = text.trim();
            if !text.is_empty() {
                texts.push(text.to_string());
                sure.extend(each);
            }
        }
        let confidence = if sure.is_empty() { 0.0 } else { sure.iter().sum::<f32>() / sure.len() as f32 };
        Ok((texts.join(" "), confidence))
    }
}
