//! The microphone, as far as the app needs it: the sound, block by block, to
//! hear when the user starts to speak.
//!
//! Windows' audio service (WASAPI) is asked for the default microphone for
//! talking, in shared mode, so other apps keep using it. The samples are
//! handed on as they come and nothing is kept. Windows shows its microphone
//! sign in the taskbar while this listens, as it should.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::HRESULT;
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};

/// Why the microphone cannot be listened to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicrophoneError {
    /// No microphone is attached, or none is set up for talking.
    NoMicrophone,
    /// Windows' privacy settings do not let desktop apps use the microphone.
    Refused,
    /// Windows refused for another reason; its words, for the log.
    Failed(String),
}

impl std::fmt::Display for MicrophoneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MicrophoneError::NoMicrophone => write!(f, "no microphone is attached"),
            MicrophoneError::Refused => write!(f, "Windows does not let desktop apps use the microphone"),
            MicrophoneError::Failed(why) => write!(f, "the microphone could not be opened: {why}"),
        }
    }
}

impl std::error::Error for MicrophoneError {}

/// "Element not found": what Windows says when no microphone is set up for talking.
const E_NOTFOUND: HRESULT = HRESULT(0x8007_0490_u32 as i32);

fn failed(error: windows::core::Error) -> MicrophoneError {
    match error.code() {
        code if code == E_ACCESSDENIED => MicrophoneError::Refused,
        code if code == E_NOTFOUND => MicrophoneError::NoMicrophone,
        _ => MicrophoneError::Failed(error.message().to_string()),
    }
}

/// Listens until dropped.
pub struct Listener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How the microphone's samples are written down.
#[derive(Debug, Clone, Copy)]
struct Format {
    channels: usize,
    rate: u32,
    /// Thirty-two-bit floating point, as Windows mixes sound; otherwise sixteen-bit whole numbers.
    float: bool,
}

/// Start listening: `on_sound` is called on a thread of its own with each
/// block of samples, one channel, between -1 and 1, and how many seconds it
/// lasts. It stops when the returned [`Listener`] is dropped.
pub fn listen(mut on_sound: impl FnMut(&[f32], f64) + Send + 'static) -> Result<Listener, MicrophoneError> {
    let stop = Arc::new(AtomicBool::new(false));
    let (opened, result) = mpsc::channel::<Result<(), MicrophoneError>>();
    let stopping = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("microphone".into())
        .spawn(move || {
            // COM is set up for this thread alone, and taken down at its end.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let started = unsafe { open() };
            let (client, capture, format) = match started {
                Ok(parts) => {
                    let _ = opened.send(Ok(()));
                    parts
                }
                Err(error) => {
                    let _ = opened.send(Err(error));
                    unsafe { CoUninitialize() };
                    return;
                }
            };
            let mut mono = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                // Windows gathers about ten milliseconds a packet; looking twice as often is plenty.
                std::thread::sleep(Duration::from_millis(20));
                loop {
                    let waiting = unsafe { capture.GetNextPacketSize() }.unwrap_or(0);
                    if waiting == 0 {
                        break;
                    }
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                    if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err() {
                        break;
                    }
                    mono.clear();
                    // A packet Windows marks as silent may hold anything; it is silence.
                    const SILENT: u32 = 2;
                    if flags & SILENT != 0 || data.is_null() {
                        mono.resize(frames as usize, 0.0);
                    } else {
                        unsafe { to_mono(data, frames as usize, format, &mut mono) };
                    }
                    let _ = unsafe { capture.ReleaseBuffer(frames) };
                    on_sound(&mono, frames as f64 / format.rate.max(1) as f64);
                }
            }
            let _ = unsafe { client.Stop() };
            drop(capture);
            drop(client);
            unsafe { CoUninitialize() };
        })
        .map_err(|error| MicrophoneError::Failed(error.to_string()))?;
    match result.recv() {
        Ok(Ok(())) => Ok(Listener { stop, thread: Some(thread) }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err(MicrophoneError::Failed("the microphone's thread ended at once".into()))
        }
    }
}

/// Open the default microphone for talking and start it.
unsafe fn open() -> Result<(IAudioClient, IAudioCaptureClient, Format), MicrophoneError> {
    let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(failed)?;
    let device = devices.GetDefaultAudioEndpoint(eCapture, eCommunications).map_err(failed)?;
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(failed)?;
    let mix = client.GetMixFormat().map_err(failed)?;
    // The structure is packed; it is copied out before its fields are read.
    let header: WAVEFORMATEX = std::ptr::read_unaligned(mix);
    let format = Format {
        channels: header.nChannels.max(1) as usize,
        rate: header.nSamplesPerSec,
        float: header.wBitsPerSample == 32,
    };
    let bits = header.wBitsPerSample;
    // Half a second of buffer: the sound is looked at every 20 ms, and nothing is lost in between.
    let started = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 5_000_000, 0, mix, None);
    CoTaskMemFree(Some(mix as *const _));
    started.map_err(failed)?;
    if bits != 32 && bits != 16 {
        return Err(MicrophoneError::Failed(format!("the microphone gives {bits}-bit samples")));
    }
    let capture: IAudioCaptureClient = client.GetService().map_err(failed)?;
    client.Start().map_err(failed)?;
    Ok((client, capture, format))
}

/// The packet's samples as one channel, the mean of its channels.
unsafe fn to_mono(data: *const u8, frames: usize, format: Format, into: &mut Vec<f32>) {
    let channels = format.channels;
    into.reserve(frames);
    if format.float {
        let samples = std::slice::from_raw_parts(data as *const f32, frames * channels);
        into.extend(samples.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
    } else {
        let samples = std::slice::from_raw_parts(data as *const i16, frames * channels);
        into.extend(samples.chunks_exact(channels).map(|frame| frame.iter().map(|&value| value as f32 / 32768.0).sum::<f32>() / channels as f32));
    }
}
