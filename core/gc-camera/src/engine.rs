//! The live pipeline: takes pictures from a source, shapes them for the
//! output, and can record a clip and play it back as a loop.
//!
//! The same behaviour as the Mac app's `CameraEngine`. All work happens on one
//! thread of the engine's own, which is what makes it safe to call from any
//! thread. Two helpers run beside it: one compresses a recording, one decodes
//! the next frame of a loop ahead of time.

use crate::badge::Badge;
use crate::clip::{frame_rate_label, Clip, ClipDecoder, ClipRecorder};
use crate::looping::LoopSequencer;
use crate::thumbnail::Thumbnail;
use gc_align::Affine;
use gc_infer::image::{ImageView, PixelFormat};
use gc_render::{Compositor, Layer, Overlay, Patch, Scene};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Frames of a recording that may wait to be compressed. More than this and
/// the newest is left out, so the live picture never waits for the recording.
const RECORDING_QUEUE: usize = 8;

/// Where finished frames go: the virtual camera.
pub trait Output: Send {
    /// The picture size the reading side asked for, while something is
    /// reading. `None` when nothing is: no picture is made for it then.
    fn wanted(&self) -> Option<(u32, u32)>;
    /// One NV12 picture. `host_ns` is when the frame it shows was taken.
    fn send(&mut self, nv12: &[u8], width: u32, height: u32, host_ns: u64);
}

/// A swapped face to draw on one frame.
#[derive(Clone)]
pub struct SwappedFace {
    pub patch: Arc<gc_live::Patch>,
    /// Maps patch pixels onto the frame.
    pub crop_to_frame: Affine,
    /// How strongly to draw it, 0 to 1.
    pub opacity: f32,
}

/// What the face swap says about one frame.
#[derive(Clone, Default)]
pub struct Swapped {
    pub face: Option<SwappedFace>,
    /// How strongly to blur the whole picture, 0 to 1.
    pub blur: f32,
}

/// How the face swap is doing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SwapReport {
    /// A face is being followed.
    pub face_found: bool,
    /// Smoothed time for one swap, and from a frame's arrival to its swapped face, in milliseconds.
    pub swap_ms: f32,
    pub swap_age_ms: f32,
    /// What stopped the swap, if anything did.
    pub error: Option<String>,
}

/// A face swap as the engine sees it: a frame in, what to draw out.
pub trait SwapStage: Send + Sync {
    /// Take in one frame and say what to draw on it. Called on the engine's thread.
    fn process(&self, frame: &ImageView<'_>, host_ns: u64) -> Swapped;
    fn report(&self) -> SwapReport;
}

impl SwapStage for gc_live::LiveEngine {
    fn process(&self, frame: &ImageView<'_>, host_ns: u64) -> Swapped {
        let result = self.push_frame(frame, host_ns);
        let face = (result.patch_sequence > 0 && result.opacity > 0.0)
            .then(|| self.patch())
            .flatten()
            .map(|patch| SwappedFace { patch, crop_to_frame: result.crop_to_frame, opacity: result.opacity });
        Swapped { face, blur: result.blur }
    }

    fn report(&self) -> SwapReport {
        let stats = self.stats();
        SwapReport { face_found: stats.tracking, swap_ms: stats.swap_ms, swap_age_ms: stats.swap_age_ms, error: stats.last_error }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// The outgoing picture's size while nothing reading the camera asks for another.
    pub output: (u32, u32),
    /// Flip the live picture left to right.
    pub mirror: bool,
    /// Draw a "LOOP" badge on the output while a loop is playing.
    pub show_loop_badge: bool,
    /// Frames over which live and loop pictures fade into each other.
    pub transition_frames: u32,
    /// Longest recording, in seconds.
    pub max_clip_seconds: f64,
    /// The rate a source is expected to deliver at, until it has been measured.
    pub frame_rate: f64,
    /// Draw an "AI" badge on the output while a swapped face is shown.
    pub show_ai_badge: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output: (1280, 720),
            mirror: false,
            show_loop_badge: false,
            transition_frames: 8,
            max_clip_seconds: 30.0,
            frame_rate: 30.0,
            show_ai_badge: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// No source: nothing goes out.
    #[default]
    Idle,
    Live,
    Looping,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub mode: Mode,
    pub is_recording: bool,
    pub recorded_seconds: f64,
    /// Pictures that went out since the engine started.
    pub frames_out: u64,
    /// Source pictures that were replaced by a newer one before they could be drawn.
    pub frames_skipped: u64,
    /// Smoothed time to draw one frame, in milliseconds.
    pub render_ms: f64,
    /// Average brightness of the latest live picture, 0 to 255.
    pub live_luma: f32,
    /// Pictures per second arriving from the source, measured.
    pub source_fps: f64,
    /// A swapped face is being drawn on the live picture.
    pub swapping: bool,
    /// The swap is on and a face is being followed.
    pub face_found: bool,
    /// Smoothed time for one swap, and from a frame's arrival to its swapped face, in milliseconds.
    pub swap_ms: f32,
    pub swap_age_ms: f32,
    /// What stopped the swap, if anything did.
    pub swap_error: Option<String>,
    /// The live picture is blurred because the swap is not on the face.
    pub picture_hidden: bool,
    /// The outgoing picture's size.
    pub output: (u32, u32),
    /// Something is reading the virtual camera.
    pub watched: bool,
}

/// What `CameraEngine::preview` copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewInfo {
    pub width: u32,
    pub height: u32,
    /// Goes up with every picture, so a caller can tell a new one from the last.
    pub sequence: u64,
}

type Change = Box<dyn FnOnce(&mut Settings) + Send>;

enum Message {
    Update(Change),
    Swap(Option<Arc<dyn SwapStage>>),
    SourceStarted,
    SourceStopped,
    /// A frame waits in the slot.
    Frame,
    StartRecording,
    StopRecording,
    StartLoop(Arc<Clip>),
    StopLoop,
    /// A frame of the loop is due. The number says which loop the tick belongs to.
    Tick(u64),
    /// Answer when everything before this has been done.
    Sync(Sender<()>),
    Quit,
}

/// The newest source frame that has not been drawn yet.
#[derive(Default)]
struct Slot {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    format: Option<PixelFormat>,
    host_ns: u64,
    full: bool,
    skipped: u64,
}

#[derive(Default)]
struct Preview {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
    sequence: u64,
}

struct Shared {
    slot: Mutex<Slot>,
    status: Mutex<Status>,
    preview: Mutex<Preview>,
    /// How many pictures had gone out when the preview was last looked at.
    /// Pictures are copied for the preview only while someone looks.
    preview_asked: AtomicU64,
    frames_out: AtomicU64,
    /// Finished recordings waiting to be collected.
    clips: Mutex<Vec<Arc<Clip>>>,
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Nothing kept under these locks can be left half-changed in a way that
    // matters: each is overwritten whole.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The live pipeline. See the module's description.
pub struct CameraEngine {
    shared: Arc<Shared>,
    messages: Sender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl CameraEngine {
    /// `output` is the virtual camera, or `None` for an engine that only
    /// feeds a preview. Times are counted from now.
    pub fn new(output: Option<Box<dyn Output>>) -> Self {
        let started = Instant::now();
        Self::with_clock(output, move || started.elapsed().as_nanos() as u64)
    }

    /// As `new`, with the clock that source frames are stamped by: `clock`
    /// says the present moment in the nanoseconds `push_frame` is given.
    pub fn with_clock(output: Option<Box<dyn Output>>, clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            slot: Mutex::default(),
            status: Mutex::default(),
            preview: Mutex::default(),
            preview_asked: AtomicU64::new(0),
            frames_out: AtomicU64::new(0),
            clips: Mutex::default(),
            clock: Box::new(clock),
        });
        let (messages, inbox) = channel();
        let (for_thread, own_inbox) = (Arc::clone(&shared), messages.clone());
        let thread = std::thread::Builder::new()
            .name("camera engine".into())
            .spawn(move || Worker::new(for_thread, output, own_inbox).run(inbox))
            .expect("the engine's thread");
        Self { shared, messages, thread: Some(thread) }
    }

    fn tell(&self, message: Message) {
        // The thread only ends when the engine is dropped.
        let _ = self.messages.send(message);
    }

    pub fn update(&self, change: impl FnOnce(&mut Settings) + Send + 'static) {
        self.tell(Message::Update(Box::new(change)));
    }

    /// Turn the face swap on by giving it a stage, or off with `None`.
    pub fn set_swap(&self, stage: Option<Arc<dyn SwapStage>>) {
        self.tell(Message::Swap(stage));
    }

    /// A source is about to deliver frames.
    pub fn source_started(&self) {
        self.tell(Message::SourceStarted);
    }

    /// The source has stopped. Nothing goes out until another starts, unless a loop is playing.
    pub fn source_stopped(&self) {
        self.tell(Message::SourceStopped);
    }

    /// Hand over one picture from the source, taken at `host_ns`. Never
    /// waits: the picture is copied, and replaces one the engine has not got to.
    pub fn push_frame(&self, frame: &ImageView<'_>, host_ns: u64) {
        let was_full = {
            let mut slot = lock(&self.shared.slot);
            slot.bytes.clear();
            for y in 0..frame.height as usize {
                slot.bytes.extend_from_slice(frame.row(y));
            }
            (slot.width, slot.height, slot.format, slot.host_ns) = (frame.width, frame.height, Some(frame.format), host_ns);
            if slot.full {
                slot.skipped += 1;
            }
            std::mem::replace(&mut slot.full, true)
        };
        if !was_full {
            self.tell(Message::Frame);
        }
    }

    /// Start recording the outgoing picture. Does nothing unless the picture is live.
    pub fn start_recording(&self) {
        self.tell(Message::StartRecording);
    }

    /// End the recording. The clip arrives through `take_clips` a moment later.
    pub fn stop_recording(&self) {
        self.tell(Message::StopRecording);
    }

    /// Play `clip` on the output in place of the live picture.
    pub fn start_loop(&self, clip: Arc<Clip>) {
        self.tell(Message::StartLoop(clip));
    }

    /// Fade back to the live picture.
    pub fn stop_loop(&self) {
        self.tell(Message::StopLoop);
    }

    pub fn status(&self) -> Status {
        lock(&self.shared.status).clone()
    }

    /// The recordings that have finished since the last call, oldest first.
    pub fn take_clips(&self) -> Vec<Arc<Clip>> {
        std::mem::take(&mut *lock(&self.shared.clips))
    }

    /// Copy the outgoing picture into `into`, four bytes a pixel: blue, green,
    /// red and an unused byte. `None` before the first picture. The engine
    /// keeps pictures ready for this only while it is being called.
    pub fn preview(&self, into: &mut Vec<u8>) -> Option<PreviewInfo> {
        self.shared.preview_asked.store(self.shared.frames_out.load(Ordering::Relaxed).max(1), Ordering::Relaxed);
        let preview = lock(&self.shared.preview);
        if preview.sequence == 0 {
            return None;
        }
        into.clear();
        into.extend_from_slice(&preview.bgra);
        Some(PreviewInfo { width: preview.width, height: preview.height, sequence: preview.sequence })
    }

    /// Wait until everything asked of the engine so far has been done.
    pub fn sync(&self) {
        let (done, wait) = channel();
        self.tell(Message::Sync(done));
        let _ = wait.recv();
    }
}

impl Drop for CameraEngine {
    fn drop(&mut self) {
        self.tell(Message::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A recording in progress: frames go to a helper thread that compresses them.
struct Recording {
    frames: SyncSender<(Vec<u8>, u64)>,
    /// Buffers the helper has finished with, to use again.
    spare: Receiver<Vec<u8>>,
    progress: Arc<RecordingProgress>,
    /// How many frames the helper was handed.
    handed: u64,
}

#[derive(Default)]
struct RecordingProgress {
    /// Seconds recorded so far, as the bits of an `f64`.
    seconds: AtomicU64,
    full: AtomicBool,
    /// How many frames the helper has dealt with.
    handled: AtomicU64,
}

/// A loop being played.
struct Playing {
    decoder: Arc<ClipDecoder>,
    sequencer: LoopSequencer,
    /// Which loop this is, counted from the engine's start. Its ticker's ticks carry the number.
    number: u64,
    /// Ticks that have been asked for and not yet drawn. The ticker reads it
    /// and leaves a tick out while the engine is behind; when the loop ends
    /// it is set to `u32::MAX`, which tells the ticker to stop.
    ticks_waiting: Arc<AtomicU32>,
    /// Frames left in a fade between live and loop pictures.
    transition_left: u32,
    leaving: bool,
    /// Frames to decode ahead of time.
    ahead: Sender<usize>,
}

impl Drop for Playing {
    fn drop(&mut self) {
        self.ticks_waiting.store(u32::MAX, Ordering::Release);
    }
}

/// Ask the engine for a frame of the loop every `period`, on a schedule that
/// does not drift, until the loop ends.
///
/// A thread of its own because it sleeps: a sleep is exact to a fraction of a
/// millisecond on every system, and waiting on the engine's inbox with a time
/// limit is not. On Windows such a wait can be 15 ms late, which is half a frame.
fn tick(messages: Sender<Message>, number: u64, period: Duration, ticks_waiting: Arc<AtomicU32>) {
    let mut due = Instant::now();
    loop {
        match ticks_waiting.load(Ordering::Acquire) {
            u32::MAX => return,
            // The engine has not drawn the last tick yet: asking again would only queue pictures up.
            waiting if waiting > 0 => {}
            _ => {
                // Unless the loop ended in this instant, one tick is now waiting.
                if ticks_waiting.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok()
                    && messages.send(Message::Tick(number)).is_err()
                {
                    return;
                }
            }
        }
        due += period;
        let now = Instant::now();
        match due.checked_duration_since(now) {
            Some(wait) => std::thread::sleep(wait),
            // Very late, as after the computer slept: carry on from now.
            None if now.duration_since(due) > period => due = now,
            None => {}
        }
    }
}

enum Drawn {
    Live,
    Loop,
}

struct Worker {
    shared: Arc<Shared>,
    /// The engine's own inbox, for the helpers that report back to it.
    messages: Sender<Message>,
    output: Option<Box<dyn Output>>,
    settings: Settings,
    status: Status,
    swap: Option<Arc<dyn SwapStage>>,
    source_running: bool,

    /// Draws the live picture, swapped face and badge included. Fades between
    /// live and loop use what it drew last, so the real face never shows through them.
    live: Compositor,
    /// Whether `live` holds a picture drawn since the source started.
    have_live: bool,
    /// Draws the frames of a loop.
    looped: Compositor,
    frame: Vec<u8>,
    nv12: Vec<u8>,
    ai_badge: Badge,
    loop_badge: Badge,

    frames_in: u64,
    rate_window: Option<(Instant, u32)>,
    recording: Option<Recording>,
    clips_recorded: u32,
    loops_played: u64,
    playing: Option<Playing>,
}

/// A decoded frame of `clip` as a picture.
fn clip_view<'a>(bgra: &'a [u8], clip: &Clip) -> Option<ImageView<'a>> {
    ImageView::new(bgra, clip.width, clip.height, clip.width as usize * 4, PixelFormat::Bgra)
}

fn canvas_view(compositor: &Compositor) -> Option<ImageView<'_>> {
    ImageView::new(compositor.canvas(), compositor.width(), compositor.height(), compositor.width() as usize * 4, PixelFormat::Bgra)
}

impl Worker {
    fn new(shared: Arc<Shared>, output: Option<Box<dyn Output>>, messages: Sender<Message>) -> Self {
        let settings = Settings::default();
        let (width, height) = settings.output;
        Self {
            shared,
            messages,
            output,
            settings,
            status: Status { output: settings.output, ..Status::default() },
            swap: None,
            source_running: false,
            live: Compositor::new(width, height),
            have_live: false,
            looped: Compositor::new(width, height),
            frame: Vec::new(),
            nv12: Vec::new(),
            ai_badge: Badge::ai(height),
            loop_badge: Badge::looping(height),
            frames_in: 0,
            rate_window: None,
            recording: None,
            clips_recorded: 0,
            loops_played: 0,
            playing: None,
        }
    }

    fn run(mut self, inbox: Receiver<Message>) {
        self.publish();
        // The worker holds a sender to its own inbox, so the inbox never
        // closes by itself: the engine ends when it is told to.
        while let Ok(message) = inbox.recv() {
            match message {
                Message::Update(change) => self.update(change),
                Message::Swap(stage) => self.set_swap(stage),
                Message::SourceStarted => self.source_started(),
                Message::SourceStopped => self.source_stopped(),
                Message::Frame => self.receive(),
                Message::StartRecording => self.start_recording(),
                Message::StopRecording => self.finish_recording(),
                Message::StartLoop(clip) => self.start_loop(clip),
                Message::StopLoop => self.stop_loop(),
                Message::Tick(number) => self.loop_tick(number),
                Message::Sync(done) => {
                    if let Some(recording) = &self.recording {
                        // The recording's helper finishes what it was handed
                        // first, so that whoever asked sees the recording as it stands.
                        let patience = Instant::now() + Duration::from_secs(2);
                        while recording.progress.handled.load(Ordering::Relaxed) < recording.handed && Instant::now() < patience {
                            std::thread::sleep(Duration::from_micros(200));
                        }
                        self.status.recorded_seconds = f64::from_bits(recording.progress.seconds.load(Ordering::Relaxed));
                        if recording.progress.full.load(Ordering::Relaxed) {
                            self.finish_recording();
                        }
                    }
                    self.publish();
                    let _ = done.send(());
                }
                Message::Quit => break,
            }
        }
        self.finish_recording();
    }

    // Control

    fn update(&mut self, change: Change) {
        let before = self.settings.output;
        change(&mut self.settings);
        if self.settings.output != before {
            self.resize();
        }
    }

    /// Make everything that depends on the outgoing picture's size anew.
    fn resize(&mut self) {
        // A clip has one size, so a recording in progress ends here.
        self.finish_recording();
        let (width, height) = self.settings.output;
        self.live = Compositor::new(width, height);
        self.looped = Compositor::new(width, height);
        // What the compositor made of the size: at least two pixels, and even.
        self.settings.output = (self.live.width(), self.live.height());
        self.have_live = false;
        self.ai_badge = Badge::ai(self.live.height());
        self.loop_badge = Badge::looping(self.live.height());
        self.status.output = self.settings.output;
        self.publish();
    }

    /// Follow what the app reading the camera asked for.
    fn follow_output(&mut self) {
        let wanted = self.output.as_ref().and_then(|output| output.wanted());
        self.status.watched = wanted.is_some();
        if let Some(wanted) = wanted {
            if wanted != self.settings.output {
                self.settings.output = wanted;
                self.resize();
            }
        }
    }

    fn set_swap(&mut self, stage: Option<Arc<dyn SwapStage>>) {
        if stage.is_none() {
            self.status.swapping = false;
            self.status.face_found = false;
            self.status.swap_error = None;
            self.status.picture_hidden = false;
        }
        self.swap = stage;
        self.publish();
    }

    fn source_started(&mut self) {
        self.follow_output();
        self.source_running = true;
        self.have_live = false;
        self.rate_window = None;
        self.status.source_fps = 0.0;
        if self.status.mode == Mode::Idle {
            self.status.mode = Mode::Live;
        }
        self.publish();
    }

    fn source_stopped(&mut self) {
        self.source_running = false;
        self.have_live = false;
        self.rate_window = None;
        self.status.source_fps = 0.0;
        self.finish_recording();
        // A frame that was still waiting is not shown after the source stopped.
        lock(&self.shared.slot).full = false;
        if self.status.mode == Mode::Live {
            self.status.mode = Mode::Idle;
        }
        self.publish();
    }

    // Frames

    fn receive(&mut self) {
        let (width, height, format, host_ns) = {
            let mut slot = lock(&self.shared.slot);
            if !slot.full {
                return;
            }
            slot.full = false;
            std::mem::swap(&mut self.frame, &mut slot.bytes);
            self.status.frames_skipped = slot.skipped;
            (slot.width, slot.height, slot.format, slot.host_ns)
        };
        if !self.source_running {
            return;
        }
        self.follow_output();
        // The frame is lent out of `self` while the rest of `self` works on it.
        let frame = std::mem::take(&mut self.frame);
        if let Some(view) = format.and_then(|format| ImageView::new(&frame, width, height, width as usize * format.bytes(), format)) {
            self.draw_live(&view, host_ns);
        }
        self.frame = frame;
    }

    fn draw_live(&mut self, view: &ImageView<'_>, host_ns: u64) {
        self.frames_in += 1;
        self.measure_rate();
        if self.frames_in % 15 == 1 {
            self.status.live_luma = Thumbnail::of(view).mean();
        }
        // The swap follows every frame, also while a loop plays, so that it
        // is already on the face when the picture returns to live.
        let swapped = self.swap.as_ref().map(|stage| stage.process(view, host_ns)).unwrap_or_default();
        let drawing_face = swapped.face.is_some();
        if let Some(stage) = &self.swap {
            if self.frames_in % 15 == 1 || self.status.swapping != drawing_face {
                let report = stage.report();
                self.status.face_found = report.face_found;
                self.status.swap_ms = report.swap_ms;
                self.status.swap_age_ms = report.swap_age_ms;
                self.status.swap_error = report.error;
            }
        }
        self.status.swapping = drawing_face;
        self.status.picture_hidden = swapped.blur > 0.5;

        let started = Instant::now();
        let (out_width, out_height) = (self.live.width(), self.live.height());
        let mut scene = Scene::of(*view);
        scene.primary.mirrored = self.settings.mirror;
        scene.blur = swapped.blur;
        if let Some(face) = &swapped.face {
            scene.patch = Some(Patch {
                rgba: &face.patch.rgba,
                size: face.patch.size,
                crop_to_frame: face.crop_to_frame,
                opacity: face.opacity,
            });
            if self.settings.show_ai_badge && face.opacity > 0.05 {
                // Top right, a little in from the corner.
                let margin = (out_width as f32 * 0.02).round() as u32;
                scene.overlay = Some(Overlay {
                    rgba: &self.ai_badge.rgba,
                    width: self.ai_badge.width,
                    height: self.ai_badge.height,
                    x: out_width.saturating_sub(margin + self.ai_badge.width),
                    y: (out_height as f32 * 0.03).round() as u32,
                    opacity: face.opacity,
                });
            }
        }
        self.live.draw(&scene);
        self.have_live = true;
        if self.status.mode != Mode::Live {
            return;
        }
        self.note_render(started);
        self.record(host_ns);
        self.emit(Drawn::Live, host_ns);
    }

    fn loop_tick(&mut self, number: u64) {
        // A tick from a loop that has ended since is not this loop's.
        if self.playing.as_ref().is_none_or(|playing| playing.number != number) {
            return;
        }
        let Some(mut playing) = self.playing.take() else { return };
        playing.ticks_waiting.store(0, Ordering::Release);

        let step = playing.sequencer.step();
        // Have the frames of the next tick decoded while this one is drawn.
        let next = playing.sequencer.clone().step();
        let _ = playing.ahead.send(next.primary);
        if let Some(index) = next.secondary {
            let _ = playing.ahead.send(index);
        }

        self.follow_output();
        let clip = Arc::clone(playing.decoder.clip());
        let primary = playing.decoder.frame(step.primary);
        let secondary = step.secondary.and_then(|index| playing.decoder.frame(index));
        let Some(primary_view) = primary.as_deref().and_then(|bytes| clip_view(bytes, &clip)) else {
            self.playing = Some(playing);
            return;
        };
        let started = Instant::now();
        let mut first = Layer { image: primary_view, mirrored: false };
        let mut second =
            secondary.as_deref().and_then(|bytes| clip_view(bytes, &clip)).map(|image| Layer { image, mirrored: false });
        let mut mix = step.mix;
        // While fading in or out, the loop picture is blended with the live
        // one instead of with its own cross-fade partner.
        if playing.transition_left > 0 && self.have_live {
            let total = self.settings.transition_frames.max(1) as f32;
            // Rises to 1.
            let progress = 1.0 - (playing.transition_left - 1) as f32 / total;
            // Already mirrored and swapped when it was drawn.
            if let Some(image) = canvas_view(&self.live) {
                let live = Layer { image, mirrored: false };
                if playing.leaving {
                    second = Some(live);
                } else {
                    second = Some(first);
                    first = live;
                }
                mix = progress;
            }
            playing.transition_left -= 1;
        } else {
            playing.transition_left = 0;
        }

        let (out_width, out_height) = (self.looped.width(), self.looped.height());
        let overlay = self.settings.show_loop_badge.then(|| Overlay {
            rgba: &self.loop_badge.rgba,
            width: self.loop_badge.width,
            height: self.loop_badge.height,
            x: (out_width as f32 * 0.02).round() as u32,
            y: (out_height as f32 * 0.03).round() as u32,
            opacity: 1.0,
        });
        self.looped.draw(&Scene { primary: first, blur: 0.0, patch: None, secondary: second.map(|layer| (layer, mix)), overlay });
        self.note_render(started);
        self.emit(Drawn::Loop, (self.shared.clock)());

        match playing.leaving && playing.transition_left == 0 {
            true => self.end_loop(),
            false => self.playing = Some(playing),
        }
    }

    fn emit(&mut self, drawn: Drawn, host_ns: u64) {
        let compositor = match drawn {
            Drawn::Live => &self.live,
            Drawn::Loop => &self.looped,
        };
        self.status.frames_out += 1;
        if let Some(output) = &mut self.output {
            let wanted = output.wanted();
            self.status.watched = wanted.is_some();
            // A picture of another size than the one asked for would only be thrown away.
            if wanted == Some((compositor.width(), compositor.height())) {
                compositor.nv12(&mut self.nv12);
                output.send(&self.nv12, compositor.width(), compositor.height(), host_ns);
            }
        }
        // The preview is copied while someone has looked at it within the last
        // second's worth of pictures, and once at the start so that the first
        // look is not empty.
        let asked = self.shared.preview_asked.load(Ordering::Relaxed);
        let mut preview = lock(&self.shared.preview);
        if preview.sequence == 0 || (asked > 0 && self.status.frames_out - asked.min(self.status.frames_out) <= 30) {
            preview.bgra.clear();
            preview.bgra.extend_from_slice(compositor.canvas());
            (preview.width, preview.height) = (compositor.width(), compositor.height());
            preview.sequence += 1;
        }
        drop(preview);
        self.shared.frames_out.store(self.status.frames_out, Ordering::Relaxed);
        // The numbers change every frame; whoever watches them looks ten times a second.
        if self.status.frames_out.is_multiple_of(3) {
            self.publish();
        }
    }

    /// Count the source's pictures over one-second windows.
    fn measure_rate(&mut self) {
        let now = Instant::now();
        match &mut self.rate_window {
            None => self.rate_window = Some((now, 0)),
            Some((started, frames)) => {
                *frames += 1;
                let elapsed = now.duration_since(*started).as_secs_f64();
                if elapsed >= 1.0 {
                    self.status.source_fps = *frames as f64 / elapsed;
                    self.rate_window = Some((now, 0));
                }
            }
        }
    }

    fn note_render(&mut self, started: Instant) {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        self.status.render_ms = if self.status.render_ms == 0.0 { ms } else { self.status.render_ms * 0.9 + ms * 0.1 };
    }

    fn publish(&self) {
        *lock(&self.shared.status) = self.status.clone();
    }

    // Recording and looping

    fn start_recording(&mut self) {
        if self.recording.is_some() || self.status.mode != Mode::Live {
            return;
        }
        // Take the size the reading app asks for first: a recording made at
        // another size would end with the next picture.
        self.follow_output();
        // The rate the recording is expected to arrive at: the source's
        // measured rate, once known. The recorder then measures the clip's own
        // rate, which is the one the loop is played back at.
        let rate = frame_rate_label(self.status.source_fps).unwrap_or(self.settings.frame_rate);
        let (width, height) = (self.live.width(), self.live.height());
        let Ok(mut recorder) = ClipRecorder::new(width, height, rate, self.settings.max_clip_seconds) else { return };
        self.clips_recorded += 1;
        let name = format!("Loop {}", self.clips_recorded);
        let (frames, waiting) = sync_channel::<(Vec<u8>, u64)>(RECORDING_QUEUE);
        let (give_back, spare) = channel();
        let progress = Arc::new(RecordingProgress::default());
        let (shared, for_helper) = (Arc::clone(&self.shared), Arc::clone(&progress));
        let helper = std::thread::Builder::new().name("clip recorder".into()).spawn(move || {
            for (bgra, host_ns) in waiting {
                if let Some(view) = ImageView::new(&bgra, width, height, width as usize * 4, PixelFormat::Bgra) {
                    let room = recorder.append(&view, host_ns);
                    for_helper.seconds.store(recorder.elapsed_seconds().to_bits(), Ordering::Relaxed);
                    for_helper.full.store(!room, Ordering::Relaxed);
                }
                for_helper.handled.fetch_add(1, Ordering::Relaxed);
                let _ = give_back.send(bgra);
            }
            // The engine has let go of the recording: make the clip of it.
            if let Ok(clip) = recorder.finish(&name, None) {
                lock(&shared.clips).push(Arc::new(clip));
            }
        });
        if helper.is_err() {
            return;
        }
        self.recording = Some(Recording { frames, spare, progress, handed: 0 });
        self.status.is_recording = true;
        self.status.recorded_seconds = 0.0;
        self.publish();
    }

    /// Add the live picture just drawn to the recording, if there is one.
    fn record(&mut self, host_ns: u64) {
        let Some(recording) = &mut self.recording else { return };
        let mut bgra = recording.spare.try_recv().unwrap_or_default();
        bgra.clear();
        bgra.extend_from_slice(self.live.canvas());
        // A full queue means the helper is behind: this frame is left out.
        let sent = recording.frames.try_send((bgra, host_ns));
        recording.handed += u64::from(sent.is_ok());
        let gone = matches!(sent, Err(TrySendError::Disconnected(_)));
        let full = recording.progress.full.load(Ordering::Relaxed);
        self.status.recorded_seconds = f64::from_bits(recording.progress.seconds.load(Ordering::Relaxed));
        if gone || full {
            self.finish_recording();
        }
    }

    fn finish_recording(&mut self) {
        // Letting go of the recording is what tells its helper to finish.
        if self.recording.take().is_some() {
            self.status.is_recording = false;
            self.publish();
        }
    }

    fn start_loop(&mut self, clip: Arc<Clip>) {
        self.finish_recording();
        if clip.points.end > clip.frame_count() || clip.frame_rate <= 0.0 {
            return;
        }
        let Some(sequencer) = LoopSequencer::new(clip.points, clip.mode) else { return };
        let decoder = Arc::new(ClipDecoder::new(Arc::clone(&clip)));
        let (ahead, wanted) = channel::<usize>();
        let for_helper = Arc::clone(&decoder);
        let helper = std::thread::Builder::new().name("loop decoder".into()).spawn(move || {
            for index in wanted {
                for_helper.frame(index);
            }
        });
        if helper.is_err() {
            return;
        }
        // The loop before this one, if any, ends here, and its ticker with it.
        self.playing = None;
        self.loops_played += 1;
        let (number, ticks_waiting) = (self.loops_played, Arc::new(AtomicU32::new(0)));
        let (messages, period, for_ticker) =
            (self.messages.clone(), Duration::from_secs_f64(1.0 / clip.frame_rate), Arc::clone(&ticks_waiting));
        let ticker =
            std::thread::Builder::new().name("loop ticker".into()).spawn(move || tick(messages, number, period, for_ticker));
        if ticker.is_err() {
            return;
        }
        self.playing = Some(Playing {
            decoder,
            sequencer,
            number,
            ticks_waiting,
            transition_left: if self.have_live { self.settings.transition_frames } else { 0 },
            leaving: false,
            ahead,
        });
        self.status.mode = Mode::Looping;
        self.publish();
    }

    fn stop_loop(&mut self) {
        let Some(playing) = &mut self.playing else { return };
        if playing.leaving {
            return;
        }
        if !self.have_live || self.settings.transition_frames == 0 {
            self.end_loop();
        } else {
            playing.leaving = true;
            playing.transition_left = self.settings.transition_frames;
        }
    }

    fn end_loop(&mut self) {
        self.playing = None;
        self.status.mode = if self.source_running { Mode::Live } else { Mode::Idle };
        self.publish();
    }
}
