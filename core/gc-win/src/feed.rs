//! The pipe the camera's media source fetches pictures from.
//!
//! The app makes one [`Feed`] at start-up. While nothing reads the camera it
//! costs nothing. [`Feed::watched`] says when an app has started reading and
//! which size it asked for; from then on the app hands over its pictures with
//! [`Feed::send`]. Sending never waits: the picture is copied, and a thread of
//! the feed's own writes it. If the next picture comes before the last one is
//! written, the older one is dropped.
//!
//! This is the same server as `apps/windows/vcam-client/client.cpp`, which the
//! camera's own test tools use. Keep the two alike: who may open the pipe, and
//! who owns it, are what the media source checks before it believes a pipe.

use crate::protocol::{self, Wanted, FRAME_HEADER_BYTES, REQUEST_BYTES};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_ACCESS_DENIED, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL,
    WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, LookupAccountNameW, TokenUser, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SID_NAME_USE,
    TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcessToken, ResetEvent, SetEvent, WaitForMultipleObjects, INFINITE,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// What the camera service may do with the pipe: read, write and look at it.
/// It may not create a further instance of the pipe, which is the one right
/// that ordinary write access would add.
const SOURCE_RIGHTS: &str = "0x12019b";
/// Room in the pipe for one 1080p picture, so a write does not wait for the reader.
const OUT_BUFFER_BYTES: u32 = 4 * 1024 * 1024;
/// A write that takes longer than this means the source is stuck.
const WRITE_TIMEOUT_MS: u32 = 1000;

/// Why the pipe could not be made.
#[derive(Debug)]
pub enum FeedError {
    /// Another program already holds the pipe's name: another copy of the app,
    /// or one of the camera's test tools.
    Taken,
    /// Windows refused for another reason. `step` is the call that failed.
    System { step: &'static str, code: i32 },
}

impl fmt::Display for FeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeedError::Taken => write!(f, "another program already holds the camera's pipe"),
            FeedError::System { step, code } => write!(f, "the camera's pipe could not be made: {step} failed with {code:#010x}"),
        }
    }
}

impl std::error::Error for FeedError {}

fn failed(step: &'static str) -> impl Fn(windows::core::Error) -> FeedError {
    move |error| FeedError::System { step, code: error.code().0 }
}

/// Counters since the feed was made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeedStats {
    /// Pictures written to the source.
    pub sent: u64,
    /// Pictures replaced by a newer one before they were written.
    pub dropped: u64,
    /// Times a source connected.
    pub connects: u64,
    /// Writes that failed or took too long; the source is then cut off.
    pub write_failures: u64,
}

/// A Windows handle that is closed with its owner.
struct Owned(HANDLE);

// A kernel handle may be used from any thread.
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // Nothing can be done about a handle that will not close.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

fn event(manual_reset: bool) -> Result<Owned, FeedError> {
    unsafe { CreateEventW(None, manual_reset, false, PCWSTR::null()) }.map(Owned).map_err(failed("CreateEventW"))
}

/// Text that Windows allocated, as a string; the allocation is given back.
unsafe fn take_text(text: PWSTR) -> String {
    let string = unsafe { text.to_string() }.unwrap_or_default();
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    string
}

/// A SID as text, such as `S-1-5-21-...`.
unsafe fn sid_text(sid: PSID) -> Option<String> {
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.ok()?;
    Some(unsafe { take_text(text) })
}

/// The SID of the user this process runs as, as text.
fn current_user_sid() -> Result<String, FeedError> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(failed("OpenProcessToken"))?;
        let token = Owned(token);
        let mut bytes = 0u32;
        // The first call only says how much room is needed.
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut bytes);
        // Eight-byte units, so the structure Windows writes is aligned.
        let mut room = vec![0u64; (bytes as usize).div_ceil(8).max(1)];
        GetTokenInformation(token.0, TokenUser, Some(room.as_mut_ptr().cast()), bytes, &mut bytes)
            .map_err(failed("GetTokenInformation"))?;
        let user = &*room.as_ptr().cast::<TOKEN_USER>();
        sid_text(user.User.Sid).ok_or(FeedError::System { step: "ConvertSidToStringSidW", code: 0 })
    }
}

/// The SID of a Windows service, as text, or `None` if this machine has no such service.
fn service_sid(account: &str) -> Option<String> {
    let account = HSTRING::from(account);
    unsafe {
        let (mut sid_bytes, mut domain_chars) = (0u32, 0u32);
        let mut kind = SID_NAME_USE::default();
        let _ = LookupAccountNameW(PCWSTR::null(), &account, None, &mut sid_bytes, None, &mut domain_chars, &mut kind);
        if sid_bytes == 0 {
            return None;
        }
        let mut sid = vec![0u64; (sid_bytes as usize).div_ceil(8)];
        let mut domain = vec![0u16; domain_chars as usize + 1];
        LookupAccountNameW(
            PCWSTR::null(),
            &account,
            Some(PSID(sid.as_mut_ptr().cast())),
            &mut sid_bytes,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_chars,
            &mut kind,
        )
        .ok()?;
        sid_text(PSID(sid.as_mut_ptr().cast()))
    }
}

/// The time both sides stamp and compare: the performance counter in units of
/// 100 nanoseconds, which is also the clock Media Foundation's samples use.
pub fn now_100ns() -> i64 {
    let (mut counter, mut frequency) = (0i64, 0i64);
    // Neither call can fail on any Windows this app runs on.
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter);
        let _ = QueryPerformanceFrequency(&mut frequency);
    }
    if frequency <= 0 {
        return 0;
    }
    // Whole seconds and the rest are scaled apart, so nothing overflows.
    (counter / frequency) * 10_000_000 + (counter % frequency) * 10_000_000 / frequency
}

struct Pending {
    /// A header and its picture, ready to write.
    bytes: Vec<u8>,
    waiting: bool,
    wanted: Wanted,
}

struct Shared {
    pipe: Owned,
    /// Set once, when the feed is being dropped.
    stop: Owned,
    /// Set whenever a picture is waiting to be written.
    frame_ready: Owned,
    pending: Mutex<Pending>,
    watched: AtomicBool,
    sequence: AtomicU64,
    sent: AtomicU64,
    dropped: AtomicU64,
    connects: AtomicU64,
    write_failures: AtomicU64,
}

impl Shared {
    fn pending(&self) -> MutexGuard<'_, Pending> {
        // A panic while the lock was held cannot leave the buffer half-written
        // in a way that matters: the next send overwrites it.
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The app's end of the camera: the pipe, and the thread that writes to it.
pub struct Feed {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    pipe_name: String,
}

impl Feed {
    /// Make the pipe for the user this process runs as.
    pub fn open() -> Result<Self, FeedError> {
        let user = current_user_sid()?;
        Self::open_named(&format!("{}{user}", protocol::PIPE_PREFIX))
    }

    /// Make the pipe under a name of the caller's choosing. The media source
    /// only opens the name [`Feed::open`] uses; this is for tests, which must
    /// not take that name from a running app.
    #[doc(hidden)]
    pub fn open_named(pipe_name: &str) -> Result<Self, FeedError> {
        let user = current_user_sid()?;
        let (stop, frame_ready) = (event(true)?, event(false)?);

        // Whose the pipe is: this user's, also when the app runs as
        // administrator (Windows would otherwise write the Administrators
        // group as the owner). The media source checks the owner before it
        // believes the pipe.
        //
        // Who may open it: this user, fully; and the camera service, to read
        // and write. The service runs as LOCAL SERVICE; its own identity and
        // the system's are named as well, in case a version of Windows runs it
        // more narrowly or does part of its work as the system. Nobody else,
        // and nobody over the network.
        let mut sddl = format!("O:{user}D:P(A;;GA;;;{user})(A;;{SOURCE_RIGHTS};;;LS)(A;;{SOURCE_RIGHTS};;;SY)");
        if let Some(frame_server) = service_sid(r"NT SERVICE\FrameServer") {
            sddl.push_str(&format!("(A;;{SOURCE_RIGHTS};;;{frame_server})"));
        }
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(&HSTRING::from(sddl), SDDL_REVISION_1, &mut descriptor, None)
        }
        .map_err(failed("ConvertStringSecurityDescriptorToSecurityDescriptorW"))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        // FILE_FLAG_FIRST_PIPE_INSTANCE: if another program already made a
        // pipe of this name, fail instead of quietly sharing the name with it.
        let pipe = unsafe {
            CreateNamedPipeW(
                &HSTRING::from(pipe_name),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                OUT_BUFFER_BYTES,
                4096,
                0,
                Some(&attributes),
            )
        };
        // The error has to be read before anything else is asked of Windows.
        let error = pipe.is_invalid().then(windows::core::Error::from_thread);
        unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
        if let Some(error) = error {
            let taken = error.code() == ERROR_ACCESS_DENIED.to_hresult() || error.code() == ERROR_PIPE_BUSY.to_hresult();
            return Err(if taken { FeedError::Taken } else { failed("CreateNamedPipeW")(error) });
        }

        let shared = Arc::new(Shared {
            pipe: Owned(pipe),
            stop,
            frame_ready,
            pending: Mutex::new(Pending { bytes: Vec::new(), waiting: false, wanted: Wanted::default() }),
            watched: AtomicBool::new(false),
            sequence: AtomicU64::new(0),
            sent: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            connects: AtomicU64::new(0),
            write_failures: AtomicU64::new(0),
        });
        let for_worker = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("camera feed".into())
            .spawn(move || run(&for_worker))
            .map_err(|error| FeedError::System { step: "starting the feed's thread", code: error.raw_os_error().unwrap_or(0) })?;
        Ok(Self { shared, worker: Some(worker), pipe_name: pipe_name.to_string() })
    }

    /// The pipe's name. The app hands it to the virtual camera before starting it.
    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    /// What the reading side asked for, while a source is connected and has
    /// said so. `None` otherwise: nothing is reading the camera.
    pub fn watched(&self) -> Option<Wanted> {
        self.shared.watched.load(Ordering::Acquire).then(|| self.shared.pending().wanted)
    }

    /// Hand over one NV12 picture: `width` bytes a row of luma, `height` rows,
    /// then `height / 2` rows of interleaved chroma, with no padding.
    /// `time_100ns` is when it was made, from [`now_100ns`].
    ///
    /// Returns whether it was taken: `false` if nothing is watching, or the
    /// size is not sensible, or `nv12` is not exactly one picture of that size.
    pub fn send(&self, nv12: &[u8], width: u32, height: u32, time_100ns: i64) -> bool {
        if !self.shared.watched.load(Ordering::Acquire)
            || !protocol::size_is_sensible(width, height)
            || nv12.len() != protocol::nv12_bytes(width, height)
        {
            return false;
        }
        let sequence = self.shared.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let header = protocol::frame_header(width, height, sequence, time_100ns);
        {
            let mut pending = self.shared.pending();
            if pending.waiting {
                // The newer picture replaces the one not yet written.
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            }
            pending.bytes.clear();
            pending.bytes.extend_from_slice(&header);
            pending.bytes.extend_from_slice(nv12);
            pending.waiting = true;
        }
        // Cannot fail for an event this feed owns.
        let _ = unsafe { SetEvent(self.shared.frame_ready.0) };
        true
    }

    pub fn stats(&self) -> FeedStats {
        FeedStats {
            sent: self.shared.sent.load(Ordering::Relaxed),
            dropped: self.shared.dropped.load(Ordering::Relaxed),
            connects: self.shared.connects.load(Ordering::Relaxed),
            write_failures: self.shared.write_failures.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        let _ = unsafe { SetEvent(self.shared.stop.0) };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Which of `handles` was signalled first, or `None` if the time ran out.
fn wait(handles: &[HANDLE], milliseconds: u32) -> Option<usize> {
    let which = unsafe { WaitForMultipleObjects(handles, false, milliseconds) }.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
    (which < handles.len()).then_some(which)
}

fn is(error: &windows::core::Error, code: windows::Win32::Foundation::WIN32_ERROR) -> bool {
    error.code() == code.to_hresult()
}

/// One operation on the pipe that may finish later, with the event that says when.
struct Operation {
    /// Boxed: Windows keeps the address until the operation has finished.
    overlapped: Box<OVERLAPPED>,
    _event: Owned,
}

impl Operation {
    fn new() -> Option<Self> {
        let event = event(true).ok()?;
        Some(Self { overlapped: Box::new(OVERLAPPED { hEvent: event.0, ..Default::default() }), _event: event })
    }

    fn event(&self) -> HANDLE {
        self.overlapped.hEvent
    }

    fn reset(&mut self) {
        let _ = unsafe { ResetEvent(self.overlapped.hEvent) };
    }

    /// The number of bytes a finished operation moved, or `None` if it failed.
    fn result(&self, pipe: HANDLE) -> Option<u32> {
        let mut bytes = 0u32;
        unsafe { GetOverlappedResult(pipe, &*self.overlapped, &mut bytes, false) }.ok()?;
        Some(bytes)
    }

    /// Wait until a cancelled or finished operation has let go of its buffer.
    fn settle(&self, pipe: HANDLE) {
        let mut ignored = 0u32;
        unsafe {
            let _ = CancelIoEx(pipe, Some(&*self.overlapped));
            let _ = GetOverlappedResult(pipe, &*self.overlapped, &mut ignored, true);
        }
    }
}

/// Write a whole buffer, giving up when the feed stops or the source does not take it.
fn write_all(shared: &Shared, write: &mut Operation, bytes: &[u8]) -> bool {
    let pipe = shared.pipe.0;
    write.reset();
    let mut written = 0u32;
    match unsafe { WriteFile(pipe, Some(bytes), Some(&mut written), Some(&mut *write.overlapped)) } {
        Ok(()) => {}
        Err(error) if is(&error, ERROR_IO_PENDING) => {
            if wait(&[shared.stop.0, write.event()], WRITE_TIMEOUT_MS) != Some(1) {
                write.settle(pipe);
                return false;
            }
            match write.result(pipe) {
                Some(bytes) => written = bytes,
                None => return false,
            }
        }
        Err(_) => return false,
    }
    written as usize == bytes.len()
}

/// Talk to one connected source until it leaves. Returns true if the feed is stopping.
fn serve(shared: &Shared, read: &mut Operation, write: &mut Operation) -> bool {
    let pipe = shared.pipe.0;
    // Boxed for the same reason as the operations: a read that has not
    // finished still points here.
    let mut request = Box::new([0u8; REQUEST_BYTES]);
    let mut have = 0usize;
    let mut reading = false;
    let mut outgoing = Vec::new();
    loop {
        if have == REQUEST_BYTES {
            have = 0;
            match protocol::parse_request(&request) {
                Some(wanted) => {
                    shared.pending().wanted = wanted;
                    shared.watched.store(true, Ordering::Release);
                }
                // Not a source this feed understands.
                None => return false,
            }
        }
        if !reading {
            // Always keep one read going: it brings the next request, and it
            // is how the source's leaving is noticed.
            read.reset();
            let mut got = 0u32;
            match unsafe { ReadFile(pipe, Some(&mut request[have..]), Some(&mut got), Some(&mut *read.overlapped)) } {
                Ok(()) => {
                    if got == 0 {
                        return false;
                    }
                    have += got as usize;
                    continue;
                }
                Err(error) if is(&error, ERROR_IO_PENDING) => reading = true,
                // The source has gone.
                Err(_) => return false,
            }
        }

        match wait(&[shared.stop.0, read.event(), shared.frame_ready.0], INFINITE) {
            Some(0) => {
                read.settle(pipe);
                return true;
            }
            Some(1) => {
                reading = false;
                match read.result(pipe) {
                    Some(got) if got > 0 => have += got as usize,
                    _ => return false,
                }
            }
            Some(2) => {
                // A picture is waiting: take it and write it.
                {
                    let mut pending = shared.pending();
                    if !pending.waiting {
                        continue;
                    }
                    std::mem::swap(&mut outgoing, &mut pending.bytes);
                    pending.waiting = false;
                }
                if !write_all(shared, write, &outgoing) {
                    read.settle(pipe);
                    if wait(&[shared.stop.0], 0) == Some(0) {
                        return true;
                    }
                    shared.write_failures.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                shared.sent.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                read.settle(pipe);
                return false;
            }
        }
    }
}

/// The feed's thread: wait for a source, serve it, and wait for the next.
fn run(shared: &Shared) {
    let pipe = shared.pipe.0;
    let (Some(mut connect), Some(mut read), Some(mut write)) = (Operation::new(), Operation::new(), Operation::new()) else {
        return;
    };
    loop {
        connect.reset();
        let connected = match unsafe { ConnectNamedPipe(pipe, Some(&mut *connect.overlapped)) } {
            Ok(()) => true,
            // The source connected before this call.
            Err(error) if is(&error, ERROR_PIPE_CONNECTED) => true,
            Err(error) if is(&error, ERROR_IO_PENDING) => {
                if wait(&[shared.stop.0, connect.event()], INFINITE) != Some(1) {
                    connect.settle(pipe);
                    return;
                }
                connect.result(pipe).is_some()
            }
            Err(_) => false,
        };
        if connected {
            shared.connects.fetch_add(1, Ordering::Relaxed);
            let stopping = serve(shared, &mut read, &mut write);
            shared.watched.store(false, Ordering::Release);
            shared.pending().waiting = false;
            if stopping {
                return;
            }
        } else if wait(&[shared.stop.0], 200) == Some(0) {
            return;
        }
        let _ = unsafe { DisconnectNamedPipe(pipe) };
    }
}

// The header and the request are plain bytes; nothing here depends on their size
// beyond these two facts, which the protocol's header states as well.
const _: () = assert!(FRAME_HEADER_BYTES == 40 && REQUEST_BYTES == 28);
