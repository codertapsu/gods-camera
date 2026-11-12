//! Pictures from a camera or from a video file, through Media Foundation.
//!
//! Both come the same way: Windows decodes whatever the camera or the file
//! holds and hands over one picture at a time, four bytes a pixel. A camera
//! delivers at its own pace; a file is paced here by the times its pictures
//! carry, and starts again when it ends.

use crate::feed::now_100ns;
use crate::protocol;
use gc_infer::image::{ImageView, PixelFormat};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{GUID, HSTRING, PWSTR};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFSourceReader, MFCreateAttributes, MFCreateDeviceSource, MFCreateMediaType,
    MFCreateSourceReaderFromMediaSource, MFCreateSourceReaderFromURL, MFEnumDeviceSources, MFMediaType_Video, MFShutdown, MFStartup,
    MFVideoFormat_RGB32, MFSTARTUP_FULL, MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
    MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_SOURCE_READERF_NATIVEMEDIATYPECHANGED,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VT_I8;

/// The first video stream of whatever is being read.
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// Why pictures could not be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureError {
    /// The call that failed.
    pub step: &'static str,
    /// Windows' own code for the failure.
    pub code: i32,
}

impl CaptureError {
    /// Windows' privacy settings do not let this app use a camera.
    pub fn refused(&self) -> bool {
        self.code as u32 == 0x8007_0005
    }

    /// Another app has the camera, or it was unplugged.
    pub fn unavailable(&self) -> bool {
        matches!(self.code as u32, 0xC00D_3704 | 0xC00D_3EA2 | 0xC00D_3EA3 | 0x8007_001F | 0x8007_0020)
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed with {:#010x}", self.step, self.code)
    }
}

impl std::error::Error for CaptureError {}

fn failed(step: &'static str) -> impl Fn(windows::core::Error) -> CaptureError {
    move |error| CaptureError { step, code: error.code().0 }
}

/// Media Foundation, started on this thread for as long as this is held.
struct MediaFoundation;

impl MediaFoundation {
    fn start() -> Result<Self, CaptureError> {
        // The thread may already belong to COM in another way; that is fine.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(failed("MFStartup"))?;
        Ok(Self)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

fn attributes(room: u32) -> Result<IMFAttributes, CaptureError> {
    let mut made = None;
    unsafe { MFCreateAttributes(&mut made, room) }.map_err(failed("MFCreateAttributes"))?;
    made.ok_or(CaptureError { step: "MFCreateAttributes", code: 0 })
}

/// A camera attached to this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    /// The name people know it by.
    pub name: String,
    /// What Windows opens it by. It stays the same while the camera stays in the same socket.
    pub id: String,
}

/// A text a device says about itself.
unsafe fn text(device: &IMFActivate, key: &GUID) -> Option<String> {
    let mut value = PWSTR::null();
    let mut length = 0u32;
    unsafe { device.GetAllocatedString(key, &mut value, &mut length) }.ok()?;
    let text = unsafe { value.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    text
}

/// The cameras attached to this computer, without the app's own virtual camera.
pub fn cameras() -> Vec<CameraDevice> {
    let Ok(_media_foundation) = MediaFoundation::start() else { return Vec::new() };
    let mut found = Vec::new();
    unsafe {
        let Ok(video_capture) = attributes(1) else { return found };
        if video_capture.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID).is_err() {
            return found;
        }
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        if MFEnumDeviceSources(&video_capture, &mut list, &mut count).is_err() || list.is_null() {
            return found;
        }
        for device in std::slice::from_raw_parts_mut(list, count as usize) {
            // Taking each one out lets go of it when this turn of the loop ends.
            let Some(device) = device.take() else { continue };
            let name = text(&device, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME);
            let id = text(&device, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK);
            if let (Some(name), Some(id)) = (name, id) {
                // Showing the app its own output would be a hall of mirrors.
                if !name.starts_with(protocol::FRIENDLY_NAME) {
                    found.push(CameraDevice { name, id });
                }
            }
        }
        CoTaskMemFree(Some(list.cast()));
    }
    found
}

/// Where pictures come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSource {
    /// A camera, by `CameraDevice::id`.
    Camera(String),
    /// A video file. It plays at its own speed and starts again when it ends.
    File(PathBuf),
}

/// Called with every picture and the moment it arrived, in nanoseconds on the camera's clock.
pub type OnFrame = Box<dyn FnMut(&ImageView<'_>, u64) + Send>;

/// Pictures from a camera or a file, delivered on a thread of their own until this is dropped.
pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Open the source and start delivering. Returns once the source is open,
    /// or says why it could not be opened.
    pub fn start(source: CaptureSource, on_frame: OnFrame) -> Result<Self, CaptureError> {
        let stop = Arc::new(AtomicBool::new(false));
        let (answer, opened) = channel();
        let stopped = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                let media_foundation = MediaFoundation::start();
                let reader = media_foundation.as_ref().map_err(Clone::clone).and_then(|_| open(&source));
                match reader {
                    Ok(reader) => {
                        let _ = answer.send(Ok(()));
                        deliver(&reader, matches!(source, CaptureSource::File(_)), &stopped, on_frame);
                    }
                    Err(error) => {
                        let _ = answer.send(Err(error));
                    }
                }
            })
            .map_err(|_| CaptureError { step: "starting the capture thread", code: 0 })?;
        match opened.recv() {
            Ok(Ok(())) => Ok(Self { stop, thread: Some(thread) }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(CaptureError { step: "the capture thread", code: 0 }),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Open a source and ask for its pictures as four bytes a pixel.
fn open(source: &CaptureSource) -> Result<IMFSourceReader, CaptureError> {
    unsafe {
        // Windows converts whatever the source delivers, be it MJPEG, H.264 or NV12.
        let converting = attributes(1)?;
        converting.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(failed("IMFAttributes::SetUINT32"))?;
        let reader = match source {
            CaptureSource::File(path) => MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &converting)
                .map_err(failed("MFCreateSourceReaderFromURL"))?,
            CaptureSource::Camera(id) => {
                let which = attributes(2)?;
                which
                    .SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)
                    .and_then(|()| which.SetString(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK, &HSTRING::from(id.as_str())))
                    .map_err(failed("IMFAttributes::SetString"))?;
                let device = MFCreateDeviceSource(&which).map_err(failed("MFCreateDeviceSource"))?;
                let reader = MFCreateSourceReaderFromMediaSource(&device, &converting).map_err(failed("MFCreateSourceReaderFromMediaSource"))?;
                choose_camera_mode(&reader);
                reader
            }
        };
        let wanted = MFCreateMediaType().map_err(failed("MFCreateMediaType"))?;
        wanted
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .and_then(|()| wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32))
            .map_err(failed("IMFMediaType::SetGUID"))?;
        reader.SetCurrentMediaType(VIDEO, None, &wanted).map_err(failed("IMFSourceReader::SetCurrentMediaType"))?;
        Ok(reader)
    }
}

/// Of the modes a camera offers, take the one nearest to 1280 x 720 at 30
/// pictures a second: what a meeting app asks for, and what the face swap is
/// measured at. A camera's first mode is often a small one.
unsafe fn choose_camera_mode(reader: &IMFSourceReader) {
    let mut best: Option<(f64, windows::Win32::Media::MediaFoundation::IMFMediaType)> = None;
    for index in 0..500 {
        let Ok(mode) = (unsafe { reader.GetNativeMediaType(VIDEO, index) }) else { break };
        let (Ok(size), Ok(rate)) = (unsafe { mode.GetUINT64(&MF_MT_FRAME_SIZE) }, unsafe { mode.GetUINT64(&MF_MT_FRAME_RATE) }) else {
            continue;
        };
        let (width, height) = ((size >> 32) as f64, (size & 0xFFFF_FFFF) as f64);
        let per_second = (rate >> 32) as f64 / ((rate & 0xFFFF_FFFF) as f64).max(1.0);
        // Too small or too slow costs most; larger or faster than wanted costs a little.
        let size_cost = ((height - 720.0) / 720.0).abs() * if height < 720.0 { 3.0 } else { 1.0 }
            + ((width / height.max(1.0)) - 16.0 / 9.0).abs();
        let rate_cost = ((per_second - 30.0) / 30.0).abs() * if per_second < 29.0 { 4.0 } else { 0.5 };
        let cost = size_cost + rate_cost;
        if best.as_ref().is_none_or(|(least, _)| cost < *least) {
            best = Some((cost, mode));
        }
    }
    if let Some((_, mode)) = best {
        // If the camera refuses, its own choice stands.
        let _ = unsafe { reader.SetCurrentMediaType(VIDEO, None, &mode) };
    }
}

/// How the pictures being delivered are laid out: width, height, and bytes
/// from one row to the next. A negative step means the last row comes first.
unsafe fn layout(reader: &IMFSourceReader) -> Option<(u32, u32, i32)> {
    let current = unsafe { reader.GetCurrentMediaType(VIDEO) }.ok()?;
    let size = unsafe { current.GetUINT64(&MF_MT_FRAME_SIZE) }.ok()?;
    let (width, height) = ((size >> 32) as u32, size as u32);
    let step = unsafe { current.GetUINT32(&MF_MT_DEFAULT_STRIDE) }.map_or(width as i32 * 4, |step| step as i32);
    (width > 0 && height > 0).then_some((width, height, step))
}

/// Read pictures and hand them on until told to stop or the source ends for good.
fn deliver(reader: &IMFSourceReader, from_file: bool, stop: &AtomicBool, mut on_frame: OnFrame) {
    let Some(mut laid_out) = (unsafe { layout(reader) }) else { return };
    let mut upright = Vec::new();
    // A file's pictures say when they are due, counted from the file's start.
    let mut pass_started = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let (mut flags, mut time, mut sample) = (0u32, 0i64, None);
        if unsafe { reader.ReadSample(VIDEO, 0, None, Some(&mut flags), Some(&mut time), Some(&mut sample)) }.is_err() {
            return;
        }
        let said = |flag: windows::Win32::Media::MediaFoundation::MF_SOURCE_READER_FLAG| flags & flag.0 as u32 != 0;
        if said(MF_SOURCE_READERF_ERROR) {
            return;
        }
        if said(MF_SOURCE_READERF_ENDOFSTREAM) {
            if !from_file || !rewind(reader) {
                return;
            }
            pass_started = Instant::now();
            continue;
        }
        if said(MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED) || said(MF_SOURCE_READERF_NATIVEMEDIATYPECHANGED) {
            match unsafe { layout(reader) } {
                Some(new) => laid_out = new,
                None => return,
            }
        }
        let Some(sample) = sample else { continue };
        if from_file {
            // Times are in units of 100 nanoseconds.
            let due = pass_started + Duration::from_nanos(time.max(0) as u64 * 100);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait.min(Duration::from_secs(1)));
            }
        }
        let (width, height, step) = laid_out;
        let row = width as usize * 4;
        unsafe {
            let Ok(buffer) = sample.ConvertToContiguousBuffer() else { continue };
            let mut bytes = std::ptr::null_mut();
            let mut length = 0u32;
            if buffer.Lock(&mut bytes, None, Some(&mut length)).is_err() {
                continue;
            }
            let picture = std::slice::from_raw_parts(bytes, length as usize);
            let stride = step.unsigned_abs() as usize;
            if stride >= row && picture.len() >= stride * (height as usize - 1) + row {
                let view = if step < 0 {
                    // Bottom row first: turn it the right way up.
                    upright.clear();
                    for y in (0..height as usize).rev() {
                        upright.extend_from_slice(&picture[y * stride..y * stride + row]);
                    }
                    ImageView::new(&upright, width, height, row, PixelFormat::Bgra)
                } else {
                    ImageView::new(picture, width, height, stride, PixelFormat::Bgra)
                };
                if let Some(view) = view {
                    on_frame(&view, now_100ns() as u64 * 100);
                }
            }
            let _ = buffer.Unlock();
        }
    }
}

/// Go back to the start of a file. False if the file cannot do that.
fn rewind(reader: &IMFSourceReader) -> bool {
    let mut start = PROPVARIANT::default();
    unsafe {
        (*start.Anonymous.Anonymous).vt = VT_I8;
        (*start.Anonymous.Anonymous).Anonymous.hVal = 0;
        reader.SetCurrentPosition(&GUID::zeroed(), &start).is_ok()
    }
}
