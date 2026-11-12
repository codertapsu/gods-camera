//! Short recordings of the outgoing picture, for playing back as a loop.
//!
//! A clip is held in memory as one JPEG per frame. Every frame stands on its
//! own, so any frame can be decoded without the ones before it. That is what
//! makes backwards playback and cross-fades cheap. The Mac app keeps its clips
//! the same way, with a video codec's key frames in place of JPEG.

use crate::looping::{self, LoopMode, LoopPoints};
use crate::thumbnail::{Thumbnail, CELLS};
use gc_infer::image::{ImageView, PixelFormat};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// How good the stored pictures are, out of 100. At this setting a frame of
/// the loop cannot be told from a live one once it has been through a meeting
/// app's own compression.
const QUALITY: u8 = 90;
/// Decoded frames kept ready, so a cross-fade does not decode a frame twice.
const CACHE_FRAMES: usize = 12;

#[derive(Debug)]
pub enum ClipError {
    /// The recording has no frames.
    Empty,
    /// The picture is larger than a clip can hold.
    Size(u32, u32),
    /// A clip's files are missing or do not fit together.
    Unreadable(PathBuf),
    Io(std::io::Error),
}

impl fmt::Display for ClipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClipError::Empty => write!(f, "the recording has no frames"),
            ClipError::Size(width, height) => write!(f, "a clip cannot hold pictures of {width} x {height}"),
            ClipError::Unreadable(path) => write!(f, "the clip at {} cannot be read", path.display()),
            ClipError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ClipError {}

impl From<std::io::Error> for ClipError {
    fn from(error: std::io::Error) -> Self {
        ClipError::Io(error)
    }
}

/// The rate to call a stream that was measured at `measured` pictures a
/// second, or `None` when the measurement is not believable. A rate close to a
/// common one is rounded to it, so that playback ticks evenly.
pub fn frame_rate_label(measured: f64) -> Option<f64> {
    if !(4.0..=130.0).contains(&measured) {
        return None;
    }
    const COMMON: [f64; 10] = [10.0, 12.0, 15.0, 20.0, 24.0, 25.0, 30.0, 50.0, 60.0, 120.0];
    Some(COMMON.into_iter().find(|rate| (rate - measured).abs() / rate < 0.05).unwrap_or((measured * 10.0).round() / 10.0))
}

/// A name no other clip has: 32 hexadecimal digits.
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    // The standard library seeds every one of these from the system's random
    // numbers. An id only has to be different from the others in a folder.
    let word = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", word(), word())
}

fn now_unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// A short recording.
pub struct Clip {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// The rate the frames arrived at, which is the rate the loop plays at.
    pub frame_rate: f64,
    /// When it was recorded, in seconds since 1970.
    pub created: u64,
    pub points: LoopPoints,
    pub mode: LoopMode,
    /// One small greyscale picture per frame.
    thumbnails: Vec<Thumbnail>,
    /// One JPEG per frame.
    frames: Vec<Box<[u8]>>,
}

impl fmt::Debug for Clip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Clip")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("size", &(self.width, self.height))
            .field("frames", &self.frames.len())
            .field("frame_rate", &self.frame_rate)
            .field("points", &self.points)
            .field("mode", &self.mode)
            .finish()
    }
}

impl Clip {
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Seconds from the first frame to the end of the last.
    pub fn duration(&self) -> f64 {
        self.frames.len() as f64 / self.frame_rate
    }

    pub fn thumbnails(&self) -> &[Thumbnail] {
        &self.thumbnails
    }

    /// Bytes the compressed frames take.
    pub fn stored_bytes(&self) -> usize {
        self.frames.iter().map(|frame| frame.len()).sum()
    }

    /// How unlike each other the two ends of the loop are, in 8-bit levels.
    /// Near zero when the picture at the end matches the picture at the start.
    pub fn seam_mismatch(&self) -> f32 {
        looping::seam_mismatch(&self.thumbnails, &self.points)
    }

    /// The way of repeating that hides the seam best.
    pub fn suggested_mode(&self) -> LoopMode {
        looping::suggested_mode(&self.thumbnails, &self.points)
    }

    /// Average brightness, used to warn when the room's lighting has changed.
    pub fn mean_luma(&self) -> f32 {
        if self.thumbnails.is_empty() {
            return 0.0;
        }
        self.thumbnails.iter().map(Thumbnail::mean).sum::<f32>() / self.thumbnails.len() as f32
    }
}

/// Compresses frames into a [`Clip`] as they arrive.
pub struct ClipRecorder {
    width: u32,
    height: u32,
    frame_rate: f64,
    max_seconds: f64,
    max_frames: usize,
    frames: Vec<Box<[u8]>>,
    thumbnails: Vec<Thumbnail>,
    /// When the first and the latest frame arrived, in nanoseconds on any steady clock.
    first_ns: u64,
    last_ns: u64,
    packed: Vec<u8>,
}

impl ClipRecorder {
    /// `frame_rate` is the rate the frames are expected to arrive at; the
    /// recorder measures the real one. `max_seconds` is the longest recording.
    pub fn new(width: u32, height: u32, frame_rate: f64, max_seconds: f64) -> Result<Self, ClipError> {
        if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
            return Err(ClipError::Size(width, height));
        }
        Ok(Self {
            width,
            height,
            frame_rate,
            max_seconds,
            // A cap on memory as well as on time, for sources faster than expected.
            max_frames: ((max_seconds * frame_rate.max(60.0)) as usize).max(1),
            frames: Vec::new(),
            thumbnails: Vec::new(),
            first_ns: 0,
            last_ns: 0,
            packed: Vec::new(),
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// How long the recording has run, by the clock.
    pub fn elapsed_seconds(&self) -> f64 {
        match self.frames.len() < 2 {
            true => 0.0,
            false => (self.last_ns - self.first_ns) as f64 / 1e9,
        }
    }

    pub fn is_full(&self) -> bool {
        self.frames.len() >= self.max_frames || self.elapsed_seconds() >= self.max_seconds
    }

    /// The rate the frames really arrived at, or `None` while too few have
    /// arrived to tell. A camera in dim light may deliver 15 pictures a second
    /// where 30 were expected; played back at 30, the loop would run at double speed.
    pub fn measured_frame_rate(&self) -> Option<f64> {
        let elapsed = self.elapsed_seconds();
        (self.frames.len() >= 8 && elapsed > 0.2).then(|| frame_rate_label((self.frames.len() - 1) as f64 / elapsed)).flatten()
    }

    /// Add a frame that arrived at `host_ns`. Returns false once the clip has
    /// reached its longest, and for a picture of another size than the clip's.
    pub fn append(&mut self, picture: &ImageView<'_>, host_ns: u64) -> bool {
        if self.is_full() || (picture.width, picture.height) != (self.width, self.height) {
            return false;
        }
        let colours = match picture.format {
            PixelFormat::Bgra => jpeg_encoder::ColorType::Bgra,
            PixelFormat::Rgba => jpeg_encoder::ColorType::Rgba,
            PixelFormat::Bgr => jpeg_encoder::ColorType::Bgr,
            PixelFormat::Rgb => jpeg_encoder::ColorType::Rgb,
        };
        // The encoder wants the rows packed together.
        let row_bytes = picture.width as usize * picture.format.bytes();
        self.packed.clear();
        for y in 0..picture.height as usize {
            self.packed.extend_from_slice(picture.row(y));
        }
        debug_assert_eq!(self.packed.len(), row_bytes * picture.height as usize);
        let mut jpeg = Vec::with_capacity(self.frames.last().map_or(256 * 1024, |last| last.len() + last.len() / 8));
        let mut encoder = jpeg_encoder::Encoder::new(&mut jpeg, QUALITY);
        // Colour at half resolution, as in the picture that goes to the camera.
        encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
        if encoder.encode(&self.packed, self.width as u16, self.height as u16, colours).is_err() {
            return false;
        }
        self.last_ns = self.last_ns.max(host_ns);
        if self.frames.is_empty() {
            self.first_ns = self.last_ns;
        }
        self.thumbnails.push(Thumbnail::of(picture));
        self.frames.push(jpeg.into_boxed_slice());
        !self.is_full()
    }

    /// The clip, with loop points chosen for it. It plays back at the rate its
    /// frames really arrived at. Without a `crossfade`, the seam fades over
    /// about a third of a second.
    pub fn finish(self, name: &str, crossfade: Option<usize>) -> Result<Clip, ClipError> {
        if self.frames.is_empty() {
            return Err(ClipError::Empty);
        }
        let rate = self.measured_frame_rate().unwrap_or(self.frame_rate);
        let crossfade = crossfade.unwrap_or(((0.35 * rate).round() as usize).max(4));
        let points = looping::find(&self.thumbnails, self.frames.len().min(rate as usize), crossfade);
        Ok(Clip {
            id: new_id(),
            name: name.to_string(),
            width: self.width,
            height: self.height,
            frame_rate: rate,
            created: now_unix_seconds(),
            points,
            mode: LoopMode::Forward,
            thumbnails: self.thumbnails,
            frames: self.frames,
        })
    }
}

/// Turns a clip's compressed frames back into pictures, in any order.
///
/// It may be shared between threads: one can decode the frame that is needed
/// next while another draws the frame that is needed now.
pub struct ClipDecoder {
    clip: Arc<Clip>,
    cache: Mutex<VecDeque<(usize, Arc<Vec<u8>>)>>,
}

impl ClipDecoder {
    pub fn new(clip: Arc<Clip>) -> Self {
        Self { clip, cache: Mutex::new(VecDeque::with_capacity(CACHE_FRAMES + 1)) }
    }

    pub fn clip(&self) -> &Arc<Clip> {
        &self.clip
    }

    fn cached(&self, index: usize) -> Option<Arc<Vec<u8>>> {
        let cache = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.iter().find(|(cached, _)| *cached == index).map(|(_, hit)| Arc::clone(hit))
    }

    /// The picture at `index` as packed BGRA, or `None` if it cannot be decoded.
    pub fn frame(&self, index: usize) -> Option<Arc<Vec<u8>>> {
        if let Some(hit) = self.cached(index) {
            return Some(hit);
        }
        // Decoded outside the lock, so two threads can decode at once.
        let pixels = Arc::new(self.decode(index)?);
        let mut cache = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // The other thread may have decoded the same frame meanwhile.
        if let Some((_, hit)) = cache.iter().find(|(cached, _)| *cached == index) {
            return Some(Arc::clone(hit));
        }
        cache.push_back((index, Arc::clone(&pixels)));
        if cache.len() > CACHE_FRAMES {
            cache.pop_front();
        }
        Some(pixels)
    }

    fn decode(&self, index: usize) -> Option<Vec<u8>> {
        let jpeg = self.clip.frames.get(index)?;
        let options = DecoderOptions::new_fast().jpeg_set_out_colorspace(ColorSpace::BGRA);
        let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(&jpeg[..]), options);
        decoder.decode_headers().ok()?;
        let info = decoder.info()?;
        if (info.width as u32, info.height as u32) != (self.clip.width, self.clip.height) {
            return None;
        }
        let mut pixels = vec![0u8; decoder.output_buffer_size()?];
        decoder.decode_into(&mut pixels).ok()?;
        (pixels.len() == self.clip.width as usize * self.clip.height as usize * 4).then_some(pixels)
    }
}

/// What the library stores about each clip, next to the file holding its frames.
#[derive(Serialize, Deserialize)]
struct ClipRecord {
    id: String,
    name: String,
    width: u32,
    height: u32,
    frame_rate: f64,
    created: u64,
    #[serde(rename = "loop")]
    points: LoopPoints,
    mode: LoopMode,
    mean_luma: f32,
    /// Compressed size of each frame, in order. Their sum is the length of the frame data.
    frame_sizes: Vec<usize>,
}

/// A saved clip, as listed in the library without loading its frames.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipSummary {
    pub id: String,
    pub name: String,
    pub duration: f64,
    pub created: u64,
    pub mean_luma: f32,
}

/// Named loops saved on disk.
///
/// Each clip is two files: a small JSON description, and a data file holding
/// the compressed frames followed by the thumbnails.
pub struct ClipLibrary {
    directory: PathBuf,
}

/// Replace a file in one step, so a crash never leaves half of it.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// Ids are made by this library; anything else is not a file name of ours.
fn is_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl ClipLibrary {
    pub fn new(directory: impl Into<PathBuf>) -> Result<Self, ClipError> {
        let directory = directory.into();
        std::fs::create_dir_all(&directory)?;
        Ok(Self { directory })
    }

    fn data_path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.frames"))
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    fn record(&self, id: &str) -> Option<ClipRecord> {
        if !is_id(id) {
            return None;
        }
        let record: ClipRecord = serde_json::from_slice(&std::fs::read(self.record_path(id)).ok()?).ok()?;
        (record.id == id).then_some(record)
    }

    fn write_record(&self, record: &ClipRecord) -> Result<(), ClipError> {
        let json = serde_json::to_vec_pretty(record).map_err(|error| ClipError::Io(error.into()))?;
        Ok(write_whole(&self.record_path(&record.id), &json)?)
    }

    pub fn save(&self, clip: &Clip) -> Result<(), ClipError> {
        if clip.frames.is_empty() || !is_id(&clip.id) {
            return Err(ClipError::Empty);
        }
        let mut payload = Vec::with_capacity(clip.stored_bytes() + clip.thumbnails.len() * CELLS);
        for frame in &clip.frames {
            payload.extend_from_slice(frame);
        }
        for thumbnail in &clip.thumbnails {
            payload.extend_from_slice(thumbnail.cells());
        }
        write_whole(&self.data_path(&clip.id), &payload)?;
        self.write_record(&ClipRecord {
            id: clip.id.clone(),
            name: clip.name.clone(),
            width: clip.width,
            height: clip.height,
            frame_rate: clip.frame_rate,
            created: clip.created,
            points: clip.points,
            mode: clip.mode,
            mean_luma: clip.mean_luma(),
            frame_sizes: clip.frames.iter().map(|frame| frame.len()).collect(),
        })
    }

    /// Every saved clip, newest first.
    pub fn list(&self) -> Vec<ClipSummary> {
        let Ok(entries) = std::fs::read_dir(&self.directory) else { return Vec::new() };
        let mut records: Vec<ClipRecord> = entries
            .filter_map(|entry| entry.ok()?.path().file_name()?.to_str()?.strip_suffix(".json").map(str::to_string))
            .filter_map(|id| self.record(&id))
            .filter(|record| record.frame_rate > 0.0)
            .collect();
        records.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.id.cmp(&b.id)));
        records
            .into_iter()
            .map(|record| ClipSummary {
                duration: record.frame_sizes.len() as f64 / record.frame_rate,
                id: record.id,
                name: record.name,
                created: record.created,
                mean_luma: record.mean_luma,
            })
            .collect()
    }

    pub fn load(&self, id: &str) -> Result<Clip, ClipError> {
        let unreadable = || ClipError::Unreadable(self.data_path(id));
        let record = self.record(id).ok_or_else(unreadable)?;
        let payload = std::fs::read(self.data_path(id)).map_err(|_| unreadable())?;
        let count = record.frame_sizes.len();
        let frame_bytes = record.frame_sizes.iter().try_fold(0usize, |sum, &size| sum.checked_add(size)).ok_or_else(unreadable)?;
        let sensible = count > 0
            && record.frame_rate > 0.0
            && record.width > 0
            && record.height > 0
            && payload.len() == frame_bytes + count * CELLS
            && record.points.end <= count
            && record.points.start < record.points.end;
        if !sensible {
            return Err(unreadable());
        }
        let mut frames = Vec::with_capacity(count);
        let mut offset = 0;
        for &size in &record.frame_sizes {
            frames.push(payload[offset..offset + size].to_vec().into_boxed_slice());
            offset += size;
        }
        let thumbnails = payload[frame_bytes..].as_chunks::<CELLS>().0.iter().map(|cells| Thumbnail::from_cells(*cells)).collect();
        Ok(Clip {
            id: record.id,
            name: record.name,
            width: record.width,
            height: record.height,
            frame_rate: record.frame_rate,
            created: record.created,
            points: record.points,
            mode: record.mode,
            thumbnails,
            frames,
        })
    }

    /// Store a clip's changed name, loop points or mode without rewriting its frames.
    pub fn update(&self, clip: &Clip) -> Result<(), ClipError> {
        let Some(mut record) = self.record(&clip.id) else { return Ok(()) };
        record.name = clip.name.clone();
        record.points = clip.points;
        record.mode = clip.mode;
        self.write_record(&record)
    }

    /// Give a saved clip another name.
    pub fn rename(&self, id: &str, name: &str) -> Result<(), ClipError> {
        let mut record = self.record(id).ok_or_else(|| ClipError::Unreadable(self.record_path(id)))?;
        record.name = name.to_string();
        self.write_record(&record)
    }

    pub fn delete(&self, id: &str) {
        if is_id(id) {
            let _ = std::fs::remove_file(self.data_path(id));
            let _ = std::fs::remove_file(self.record_path(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (320, 180);

    /// A soft picture with a bright bar whose place says which frame it is.
    fn picture(frame: usize) -> Vec<u8> {
        let (width, height) = (SIZE.0 as usize, SIZE.1 as usize);
        let mut bgra = Vec::with_capacity(width * height * 4);
        let bar = 20 + frame * 4;
        for y in 0..height {
            for x in 0..width {
                let lit = if x >= bar && x < bar + 30 { 90 } else { 0 };
                let (b, g, r) = (60 + x / 4, 80 + y / 3, 100 + lit);
                bgra.extend_from_slice(&[b as u8, g as u8, r as u8, 255]);
            }
        }
        bgra
    }

    fn view(bytes: &[u8]) -> ImageView<'_> {
        ImageView::new(bytes, SIZE.0, SIZE.1, SIZE.0 as usize * 4, PixelFormat::Bgra).unwrap()
    }

    fn record(frames: usize, every_ms: u64) -> Clip {
        let mut recorder = ClipRecorder::new(SIZE.0, SIZE.1, 30.0, 30.0).unwrap();
        for frame in 0..frames {
            assert!(recorder.append(&view(&picture(frame)), 1_000 + frame as u64 * every_ms * 1_000_000));
        }
        recorder.finish("Loop 1", None).unwrap()
    }

    fn mean_difference(a: &[u8], b: &[u8]) -> f32 {
        a.iter().zip(b).map(|(&x, &y)| x.abs_diff(y) as u32).sum::<u32>() as f32 / a.len() as f32
    }

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("gc-camera-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn any_frame_comes_back_as_it_went_in() {
        let clip = Arc::new(record(40, 33));
        assert_eq!((clip.frame_count(), clip.width, clip.height), (40, SIZE.0, SIZE.1));
        assert_eq!(clip.frame_rate, 30.0);
        assert!((clip.duration() - 40.0 / 30.0).abs() < 1e-9);
        assert_eq!(clip.thumbnails().len(), 40);
        let decoder = ClipDecoder::new(Arc::clone(&clip));
        // Out of order, as a loop that plays backwards asks.
        for frame in [39, 0, 17, 16, 15] {
            let decoded = decoder.frame(frame).expect("a frame");
            let original = picture(frame);
            assert_eq!(decoded.len(), original.len());
            let difference = mean_difference(&decoded, &original);
            assert!(difference < 1.5, "frame {frame} came back {difference} levels off");
            // Not another frame: the bar is somewhere else there.
            assert!(mean_difference(&decoded, &picture(frame + 10)) > 3.0);
        }
        assert!(decoder.frame(40).is_none());
        // A frame asked for again is the same picture, not decoded anew.
        let (first, again) = (decoder.frame(17).unwrap(), decoder.frame(17).unwrap());
        assert!(Arc::ptr_eq(&first, &again));
    }

    #[test]
    fn the_clip_plays_at_the_rate_its_frames_arrived_at() {
        // A camera in dim light: 15 pictures a second where 30 were expected.
        let clip = record(30, 66);
        assert_eq!(clip.frame_rate, 15.0);
        assert!((clip.duration() - 2.0).abs() < 1e-9);
        // Too few frames to tell: the expected rate stands.
        assert_eq!(record(5, 66).frame_rate, 30.0);
        assert_eq!(frame_rate_label(29.2), Some(30.0));
        assert_eq!(frame_rate_label(27.26), Some(27.3));
        assert_eq!(frame_rate_label(2.0), None);
        assert_eq!(frame_rate_label(500.0), None);
    }

    #[test]
    fn a_recording_ends_at_its_longest() {
        let mut recorder = ClipRecorder::new(SIZE.0, SIZE.1, 30.0, 0.5).unwrap();
        let bytes = picture(0);
        let mut taken = 0u64;
        while recorder.append(&view(&bytes), taken * 50_000_000) {
            taken += 1;
            assert!(taken < 100, "the recording never filled");
        }
        assert!(recorder.is_full());
        // Half a second by the clock, at twenty a second: the first frame and ten more.
        assert_eq!(recorder.frame_count(), 11);
        assert_eq!(recorder.elapsed_seconds(), 0.5);
        assert!(!recorder.append(&view(&bytes), 99 * 50_000_000), "a full recording takes nothing more");
        assert_eq!(recorder.frame_count(), 11);

        // A source far faster than expected is stopped by the count instead.
        let mut fast = ClipRecorder::new(SIZE.0, SIZE.1, 30.0, 0.5).unwrap();
        let mut frames = 0u64;
        while fast.append(&view(&bytes), frames * 1_000_000) {
            frames += 1;
        }
        assert_eq!(fast.frame_count(), 30);
    }

    #[test]
    fn pictures_that_do_not_fit_are_refused() {
        assert!(matches!(ClipRecorder::new(0, 180, 30.0, 5.0), Err(ClipError::Size(0, 180))));
        assert!(matches!(ClipRecorder::new(70_000, 180, 30.0, 5.0), Err(ClipError::Size(..))));
        let mut recorder = ClipRecorder::new(SIZE.0, SIZE.1, 30.0, 5.0).unwrap();
        let other = vec![0u8; 64 * 64 * 4];
        let other = ImageView::new(&other, 64, 64, 256, PixelFormat::Bgra).unwrap();
        assert!(!recorder.append(&other, 1));
        assert_eq!(recorder.frame_count(), 0);
        assert!(matches!(recorder.finish("nothing", None), Err(ClipError::Empty)));
    }

    #[test]
    fn a_picture_with_padded_rows_or_other_byte_order_is_stored_alike() {
        let bgra = picture(3);
        let (width, height) = (SIZE.0 as usize, SIZE.1 as usize);
        // The same picture as RGB with eight spare bytes after every row.
        let stride = width * 3 + 8;
        let mut padded = vec![0xEEu8; stride * height];
        for y in 0..height {
            for x in 0..width {
                let p = &bgra[(y * width + x) * 4..];
                padded[y * stride + x * 3..y * stride + x * 3 + 3].copy_from_slice(&[p[2], p[1], p[0]]);
            }
        }
        let mut recorder = ClipRecorder::new(SIZE.0, SIZE.1, 30.0, 5.0).unwrap();
        assert!(recorder.append(&ImageView::new(&padded, SIZE.0, SIZE.1, stride, PixelFormat::Rgb).unwrap(), 1));
        let decoder = ClipDecoder::new(Arc::new(recorder.finish("padded", None).unwrap()));
        assert!(mean_difference(&decoder.frame(0).unwrap(), &bgra) < 1.5);
    }

    #[test]
    fn a_saved_clip_is_listed_loaded_changed_and_deleted() {
        let folder = scratch("library");
        let library = ClipLibrary::new(&folder).unwrap();
        assert!(library.list().is_empty());

        let mut clip = record(40, 33);
        library.save(&clip).unwrap();
        let mut older = record(12, 33);
        older.created = clip.created - 100;
        older.name = "Loop 0".into();
        library.save(&older).unwrap();

        let listed = library.list();
        assert_eq!(listed.iter().map(|summary| summary.name.as_str()).collect::<Vec<_>>(), ["Loop 1", "Loop 0"]);
        assert_eq!(listed[0].id, clip.id);
        assert!((listed[0].duration - clip.duration()).abs() < 1e-9);
        assert!((listed[0].mean_luma - clip.mean_luma()).abs() < 1e-3);

        let loaded = library.load(&clip.id).unwrap();
        assert_eq!((loaded.frame_count(), loaded.points, loaded.mode), (40, clip.points, clip.mode));
        assert_eq!(loaded.thumbnails(), clip.thumbnails());
        let (before, after) = (ClipDecoder::new(Arc::new(record(40, 33))), ClipDecoder::new(Arc::new(loaded)));
        assert_eq!(before.frame(21), after.frame(21), "the stored frames are the recorded ones, byte for byte");

        // A new name, loop and way of repeating are stored without the frames being rewritten.
        let frames_written = std::fs::metadata(folder.join(format!("{}.frames", clip.id))).unwrap().modified().unwrap();
        clip.name = "Listening".into();
        clip.points = LoopPoints { start: 5, end: 30, crossfade: 5 };
        clip.mode = LoopMode::PingPong;
        library.update(&clip).unwrap();
        let changed = library.load(&clip.id).unwrap();
        assert_eq!((changed.name.as_str(), changed.points, changed.mode), ("Listening", clip.points, LoopMode::PingPong));
        assert_eq!(std::fs::metadata(folder.join(format!("{}.frames", clip.id))).unwrap().modified().unwrap(), frames_written);
        library.rename(&older.id, "Thinking").unwrap();
        assert_eq!(library.list()[1].name, "Thinking");

        library.delete(&clip.id);
        assert_eq!(library.list().len(), 1);
        assert!(matches!(library.load(&clip.id), Err(ClipError::Unreadable(_))));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn files_that_do_not_fit_together_are_not_loaded() {
        let folder = scratch("damaged");
        let library = ClipLibrary::new(&folder).unwrap();
        let clip = record(10, 33);
        library.save(&clip).unwrap();
        let data = folder.join(format!("{}.frames", clip.id));

        // The frame data lost its end.
        let whole = std::fs::read(&data).unwrap();
        std::fs::write(&data, &whole[..whole.len() - 10]).unwrap();
        assert!(matches!(library.load(&clip.id), Err(ClipError::Unreadable(_))));
        std::fs::write(&data, &whole).unwrap();
        assert!(library.load(&clip.id).is_ok());

        // A description that is not ours, and one that points outside its clip.
        std::fs::write(folder.join("notes.json"), b"{\"hello\": 1}").unwrap();
        std::fs::write(folder.join(format!("{}.json", "0".repeat(32))), b"not json").unwrap();
        assert_eq!(library.list().len(), 1);
        let record_path = folder.join(format!("{}.json", clip.id));
        let text = std::fs::read_to_string(&record_path).unwrap();
        std::fs::write(&record_path, text.replace(&format!("\"end\": {}", clip.points.end), "\"end\": 9999")).unwrap();
        assert!(matches!(library.load(&clip.id), Err(ClipError::Unreadable(_))));

        // A name that is not an id never becomes a path.
        assert!(matches!(library.load("../../somewhere"), Err(ClipError::Unreadable(_))));
        library.delete("../../somewhere");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn every_clip_gets_a_name_of_its_own() {
        let ids: std::collections::HashSet<String> = (0..200).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 200);
        assert!(ids.iter().all(|id| is_id(id)));
    }

    /// Prints how long a frame takes to store and to bring back. Run by hand:
    /// `cargo test -p gc-camera --release speed -- --ignored --nocapture`
    #[test]
    #[ignore = "prints timings; run by hand"]
    fn speed() {
        for (width, height) in [(1280u32, 720u32), (1920, 1080)] {
            // A picture with detail in it, as a camera's has.
            let mut bgra = vec![0u8; (width * height * 4) as usize];
            let mut seed = 1u32;
            for (index, pixel) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let (x, y) = (index as u32 % width, index as u32 / width);
                let noise = (seed >> 28) as u8;
                pixel.copy_from_slice(&[(x / 8) as u8 + noise, (y / 4) as u8 + noise, ((x + y) / 10) as u8 + noise, 255]);
            }
            let picture = ImageView::new(&bgra, width, height, width as usize * 4, PixelFormat::Bgra).unwrap();
            let mut recorder = ClipRecorder::new(width, height, 30.0, 30.0).unwrap();
            let started = std::time::Instant::now();
            for frame in 0..60u64 {
                recorder.append(&picture, frame * 33_333_333);
            }
            let store_ms = started.elapsed().as_secs_f64() * 1000.0 / 60.0;
            let clip = Arc::new(recorder.finish("speed", None).unwrap());
            let decoder = ClipDecoder::new(Arc::clone(&clip));
            let started = std::time::Instant::now();
            for frame in 0..60 {
                decoder.frame(frame).unwrap();
            }
            let back_ms = started.elapsed().as_secs_f64() * 1000.0 / 60.0;
            println!(
                "{width}x{height}: store {store_ms:.2} ms, bring back {back_ms:.2} ms, {} KB a frame",
                clip.stored_bytes() / 60 / 1024
            );
        }
    }
}
