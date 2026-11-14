//! The app as its window sees it: one object that the window asks for the
//! state of every screen, about ten times a second, and tells what the user did.
//!
//! Nothing here draws or waits for a person. Slow work, such as loading the
//! models or reading photos, runs on threads of its own, and its outcome
//! shows up in the next state.

use crate::faces::{
    self, face_from_pending, read_photos, without_photo, Consent, FaceLibrary, FindFace, KeyStore, PendingFace, SavedFace,
    MAX_PHOTOS,
};
use crate::pattern::Pattern;
use crate::settings::{LoopStyle, Quality, Settings, SourceKind};
use gc_camera::{CameraEngine, Clip, ClipLibrary, LoopMode, Mode, Output, SwapStage};
use gc_infer::image::{ImageView, PixelFormat};
use gc_live::{DetectorKind, Enroller, LiveConfig, LiveEngine, SwapModel};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where the app keeps its things.
#[derive(Debug, Clone, uniffi::Record)]
pub struct AppConfig {
    /// Settings, saved faces and saved loops go here.
    pub data_dir: String,
    /// Folders that hold the model files, searched in order.
    pub pack_dirs: Vec<String>,
    /// Folder for compiled models, so later starts are faster.
    pub cache_dir: Option<String>,
    /// A try-out on a scratch data folder. Its secrets are kept in that
    /// folder, so that nothing the user's own app has stored with the system
    /// is read, replaced or deleted.
    pub try_out: bool,
}

#[derive(Debug, uniffi::Error)]
pub enum AppError {
    Failed { message: String },
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for AppError {}

/// Whether other apps can see the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VirtualCamera {
    /// It is in every app's camera list.
    Ready,
    /// Its driver is not installed on this computer.
    NotInstalled,
    /// Windows' privacy settings do not let desktop apps use cameras.
    Refused,
    /// Another program holds the camera: another copy of this app, most likely.
    Busy,
    /// Windows could not make it, for a reason in `AppState::virtual_camera_detail`.
    Failed,
    /// This system has no virtual camera, or the app runs without one.
    None,
}

/// A camera the user can choose.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CameraChoice {
    /// What the system opens it by.
    pub id: String,
    pub name: String,
}

/// Where a platform is asked to fetch pictures from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFrom {
    /// A camera, by `CameraChoice::id`.
    Camera(String),
    /// A video file, played at its own speed, again and again.
    File(PathBuf),
}

/// Called with every picture and the moment it was taken, on the platform's clock.
pub type OnFrame = Box<dyn FnMut(&ImageView<'_>, u64) + Send>;

/// Called with each short block of the microphone's sound, one channel
/// between -1 and 1, and how many seconds it lasts.
pub type OnSound = Box<dyn FnMut(&[f32], f64) + Send>;

/// Why the microphone cannot be listened to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MicrophoneProblem {
    /// No microphone is attached, or the system has none for talking.
    NoMicrophone,
    /// The system's privacy settings do not let the app use the microphone.
    Refused,
    /// The system refused for another reason.
    Failed,
}

/// What the model needs from the system it runs on.
pub trait Platform: Send + Sync {
    /// Where the engine's pictures go: the virtual camera.
    fn output(&self) -> Option<Box<dyn Output>>;
    /// The present moment in nanoseconds, on the clock the output's pictures are stamped by.
    fn clock(&self) -> u64;
    fn virtual_camera(&self) -> (VirtualCamera, String);
    /// Something is reading the virtual camera right now.
    fn watched(&self) -> bool {
        false
    }
    /// Ask the system for the virtual camera again, if it is not there: its
    /// driver may have been installed since the app started.
    fn retry_virtual_camera(&self) {}
    fn key_store(&self) -> Box<dyn KeyStore>;
    /// The cameras attached to this computer. Asking opens none of them.
    fn cameras(&self) -> Vec<CameraChoice> {
        Vec::new()
    }
    /// Start fetching pictures. They are delivered until what is returned is dropped.
    fn capture(&self, _from: CaptureFrom, _on_frame: OnFrame) -> Result<Box<dyn Send>, SourceProblem> {
        Err(SourceProblem::NotAvailable)
    }
    /// Listen to the microphone: its sound is delivered until what is
    /// returned is dropped. Nothing else may be done with the sound.
    fn listen(&self, _on_sound: OnSound) -> Result<Box<dyn Send>, MicrophoneProblem> {
        Err(MicrophoneProblem::NoMicrophone)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EngineMode {
    Idle,
    Live,
    Looping,
}

/// Why there is no picture, when the reason is the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SourceProblem {
    /// A picture or video is wanted and no file is chosen.
    NoFileChosen,
    /// The chosen file cannot be read as a picture, or as a video.
    FileUnreadable,
    /// The user has not yet said that the app may turn the camera on.
    CameraNotAllowed,
    /// No camera is attached.
    NoCamera,
    /// The system's privacy settings do not let the app use a camera.
    CameraRefused,
    /// Another app has the camera, or it was unplugged.
    CameraUnavailable,
    /// This kind of source is not built for this system yet.
    NotAvailable,
}

/// What the face swap is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SwapState {
    Off,
    /// The models are being loaded.
    Loading,
    On,
    /// A model file is not installed; `AppState::swap_detail` names it.
    Missing,
    /// It could not start; `AppState::swap_detail` says why.
    Failed,
}

/// Something to tell the user on the Face screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FaceNotice {
    FacesCouldNotBeOpened,
    AddAFaceFirst,
    NoFaceInThatPicture,
    NoFaceInThosePictures,
    /// A face has room for twenty photos; the rest were not read.
    TooManyPhotos,
    FaceCouldNotBeSaved,
    /// A model file needed to read photos is not installed; `AppState::swap_detail` names it.
    ModelMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FaceConsent {
    Myself,
    Permission,
    Synthetic,
}

impl From<Consent> for FaceConsent {
    fn from(consent: Consent) -> Self {
        match consent {
            Consent::Myself => FaceConsent::Myself,
            Consent::Permission => FaceConsent::Permission,
            Consent::Synthetic => FaceConsent::Synthetic,
        }
    }
}

impl From<FaceConsent> for Consent {
    fn from(consent: FaceConsent) -> Self {
        match consent {
            FaceConsent::Myself => Consent::Myself,
            FaceConsent::Permission => Consent::Permission,
            FaceConsent::Synthetic => Consent::Synthetic,
        }
    }
}

/// A saved face, as the list shows it. Its pictures are fetched separately.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FaceInfo {
    pub id: String,
    pub name: String,
    pub consent: FaceConsent,
    /// The ids of its photos, in order.
    pub photos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PendingPhotoInfo {
    pub id: String,
    pub file_name: String,
    pub faces_in_photo: u32,
    /// The photo shows the same person as the others, so it goes into the face.
    pub used: bool,
}

/// Photos that were read and wait for the user to save or discard them.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PendingInfo {
    pub photos: Vec<PendingPhotoInfo>,
    /// Files in which no face was found, or that could not be read.
    pub skipped: Vec<String>,
    /// The saved face the photos are for, or `None` for a new face.
    pub target: Option<String>,
    pub target_name: Option<String>,
}

/// The loop in hand.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ClipInfo {
    pub id: String,
    pub name: String,
    /// Seconds recorded, and seconds that repeat.
    pub duration: f64,
    pub loop_duration: f64,
    /// It ends as it began, so it plays start to end when the choice is left to the app.
    pub ends_as_it_began: bool,
    pub saved: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SavedClipInfo {
    pub id: String,
    pub name: String,
    pub duration: f64,
}

/// The options on the Face tab, set all at once.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FaceOptions {
    pub sharpen: f32,
    pub colour_match: bool,
    pub keep_objects_in_front: bool,
    pub real_mouth: bool,
    pub real_eyes: bool,
    pub blur_when_unswapped: bool,
    pub show_ai_badge: bool,
}

/// Everything the screens show.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AppState {
    // The picture.
    pub mode: EngineMode,
    pub frames_out: u64,
    pub source_fps: f64,
    pub is_recording: bool,
    pub recorded_seconds: f64,
    pub swapping: bool,
    pub face_found: bool,
    pub swap_ms: f32,
    pub swap_age_ms: f32,
    pub swap_error: Option<String>,
    pub picture_hidden: bool,
    /// Something is reading the virtual camera, and the size it asked for.
    pub watched: bool,
    pub output_width: u32,
    pub output_height: u32,

    // Camera.
    pub source: SourceKind,
    /// The name of the picture or video file chosen for the source in use.
    pub file_name: Option<String>,
    pub source_problem: Option<SourceProblem>,
    pub cameras: Vec<CameraChoice>,
    /// The camera in use, or the one that would be.
    pub camera: Option<String>,
    pub mirror: bool,
    pub virtual_camera: VirtualCamera,
    pub virtual_camera_detail: String,
    /// No picture is being made because the window is hidden and no app reads
    /// the camera. A camera is switched off meanwhile.
    pub resting: bool,
    /// Closing the window leaves the app running.
    pub keep_running: bool,

    // Loop.
    pub clip: Option<ClipInfo>,
    pub saved_clips: Vec<SavedClipInfo>,
    pub loop_style: LoopStyle,
    pub show_loop_badge: bool,
    pub can_record: bool,
    pub can_loop: bool,
    /// The room looks brighter or darker than in the loop in hand.
    pub lighting_differs: bool,

    // Face.
    pub swap_wanted: bool,
    pub swap_state: SwapState,
    pub swap_detail: String,
    pub policy_accepted: bool,
    pub faces: Vec<FaceInfo>,
    pub selected_face: Option<String>,
    pub busy_adding: bool,
    pub pending: Option<PendingInfo>,
    pub face_notice: Option<FaceNotice>,
    pub quality: Quality,
    pub qualities: Vec<Quality>,
    pub sharpen: f32,
    pub colour_match: bool,
    pub can_keep_objects_in_front: bool,
    pub keep_objects_in_front: bool,
    pub real_mouth: bool,
    pub real_eyes: bool,
    pub blur_when_unswapped: bool,
    pub show_ai_badge: bool,

    // Going back to live when the user speaks.
    pub back_to_live_on_voice: bool,
    /// The microphone is open: a loop plays and the option is on.
    pub listening: bool,
    /// Why the microphone could not be opened, the last time it was tried.
    pub microphone_problem: Option<MicrophoneProblem>,
}

/// The size and number of the picture `DesktopApp::preview` copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
}

/// Draws a source's picture for a moment in seconds since the source started,
/// and says its size.
type Draw = Box<dyn FnMut(f32, &mut Vec<u8>) -> (u32, u32) + Send>;

/// A source of the model's own that delivers pictures until it is dropped.
struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// A source of the platform's, with the engine told when it ends.
struct Captured {
    /// Dropped first: no picture comes after the engine was told.
    capture: Option<Box<dyn Send>>,
    core: Arc<Core>,
}

impl Drop for Captured {
    fn drop(&mut self) {
        self.capture = None;
        self.core.engine.source_stopped();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The loaded swap engine and the model it was loaded for.
struct Stage {
    quality: Quality,
    engine: Arc<LiveEngine>,
}

struct Inner {
    settings: Settings,
    /// Whatever delivers pictures now. Dropping it stops them.
    source: Option<Box<dyn Send>>,
    source_problem: Option<SourceProblem>,
    /// The cameras attached when the app last looked.
    cameras: Vec<CameraChoice>,
    /// The window shows the picture. While it does not, and nothing reads the
    /// virtual camera, no picture is made and a camera is switched off.
    preview_wanted: bool,
    /// The source was stopped because nobody looks at its picture.
    resting: bool,

    clip: Option<Arc<Clip>>,
    clip_saved: bool,
    clips: Option<ClipLibrary>,

    faces: Option<FaceLibrary>,
    saved: Vec<SavedFace>,
    swap: SwapState,
    swap_detail: String,
    stage: Option<Stage>,
    /// Counts attempts to load the models, so that a slow one that was overtaken is ignored.
    loading: u64,
    pending: Option<PendingFace>,
    busy_adding: bool,
    notice: Option<FaceNotice>,

    /// The microphone, open while a loop plays and the user asked to go back to live on speaking.
    listener: Option<Box<dyn Send>>,
    /// The microphone could not be opened for the loop that plays: not tried again until the next one.
    microphone_problem: Option<MicrophoneProblem>,
}

struct Core {
    data_dir: PathBuf,
    pack_dirs: Vec<PathBuf>,
    cache_dir: Option<PathBuf>,
    platform: Arc<dyn Platform>,
    engine: CameraEngine,
    inner: Mutex<Inner>,
    /// Reads photos. Made on first use, because it loads two models.
    finder: Mutex<Option<Box<dyn FindFace + Send>>>,
}

fn now_unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

impl Core {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // Every change under this lock leaves the state whole, so a panic
        // elsewhere while it was held does no harm here.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    /// Open the microphone while a loop plays and the user wants to go back
    /// to live on speaking, and close it otherwise. When the user speaks, the
    /// loop ends from the microphone's own thread, without waiting for the
    /// window to ask how things stand.
    fn listen_or_not(&self, inner: &mut Inner, looping: bool) {
        if !looping || !inner.settings.back_to_live_on_voice {
            inner.listener = None;
            if !looping {
                inner.microphone_problem = None;
            }
            return;
        }
        if inner.listener.is_some() || inner.microphone_problem.is_some() {
            return;
        }
        let stopper = self.engine.loop_stopper();
        let mut voice = crate::voice::VoiceActivity::default();
        let on_sound: OnSound = Box::new(move |samples, seconds| {
            if voice.feed(crate::voice::level_db(samples), seconds) {
                voice.reset();
                stopper.stop();
            }
        });
        match self.platform.listen(on_sound) {
            Ok(listener) => inner.listener = Some(listener),
            Err(problem) => inner.microphone_problem = Some(problem),
        }
    }

    fn save_settings(&self, inner: &Inner) {
        // The settings are a convenience; a disk that refuses them is not a reason to stop.
        let _ = inner.settings.save(&self.settings_path());
    }

    fn live_config(&self, quality: Quality) -> LiveConfig {
        LiveConfig {
            packs: self.pack_dirs.clone(),
            cache_dir: self.cache_dir.clone(),
            detector: DetectorKind::Large,
            detector_size: 640,
            swap_model: match quality {
                Quality::Standard => SwapModel::Inswapper128,
                Quality::Sharper => SwapModel::Hyperswap256,
            },
            cpu_only: false,
        }
    }

    fn selected<'a>(&self, inner: &'a Inner) -> Option<&'a SavedFace> {
        let id = inner.settings.selected_face.as_ref()?;
        inner.saved.iter().find(|face| &face.id == id)
    }

    // The source

    /// Whether anybody looks at the picture: the window, an app reading the
    /// virtual camera, or a recording or a loop that is under way.
    fn picture_needed(&self, inner: &Inner) -> bool {
        let status = self.engine.status();
        inner.preview_wanted || self.platform.watched() || status.is_recording || status.mode == Mode::Looping
    }

    /// Switch the source off while nobody looks, and on again when somebody does.
    fn rest_or_wake(self: &Arc<Self>, inner: &mut Inner) {
        let needed = self.picture_needed(inner);
        if needed && inner.resting {
            self.restart_source(inner);
        } else if !needed && !inner.resting && inner.source.is_some() {
            inner.source = None;
            inner.resting = true;
        }
    }

    /// Stop the source that runs, and start the one the settings name.
    fn restart_source(self: &Arc<Self>, inner: &mut Inner) {
        inner.source = None;
        inner.source_problem = None;
        inner.resting = false;
        let frames: Draw = match inner.settings.source {
            SourceKind::Pattern => {
                let pattern = Pattern::new(1280, 720);
                Box::new(move |seconds, out| {
                    pattern.render(seconds, out);
                    (pattern.width(), pattern.height())
                })
            }
            SourceKind::Image => {
                let Some(file) = inner.settings.file.clone() else {
                    inner.source_problem = Some(SourceProblem::NoFileChosen);
                    return;
                };
                let Some((bgra, width, height)) = still_picture(&PathBuf::from(file)) else {
                    inner.source_problem = Some(SourceProblem::FileUnreadable);
                    return;
                };
                Box::new(move |_, out| {
                    out.clear();
                    out.extend_from_slice(&bgra);
                    (width, height)
                })
            }
            SourceKind::Camera => {
                inner.cameras = self.platform.cameras();
                if !inner.settings.camera_allowed {
                    inner.source_problem = Some(SourceProblem::CameraNotAllowed);
                    return;
                }
                // The camera chosen last time if it is still there, otherwise the first.
                let chosen = inner.settings.camera.as_ref().filter(|id| inner.cameras.iter().any(|camera| &&camera.id == id)).cloned();
                let Some(id) = chosen.or_else(|| inner.cameras.first().map(|camera| camera.id.clone())) else {
                    inner.source_problem = Some(SourceProblem::NoCamera);
                    return;
                };
                self.capture(inner, CaptureFrom::Camera(id));
                return;
            }
            SourceKind::Video => {
                match inner.settings.video.clone() {
                    Some(file) => self.capture(inner, CaptureFrom::File(PathBuf::from(file))),
                    None => inner.source_problem = Some(SourceProblem::NoFileChosen),
                }
                return;
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (core, stopped) = (Arc::clone(self), Arc::clone(&stop));
        let thread = std::thread::Builder::new().name("picture source".into()).spawn(move || deliver(&core, &stopped, frames));
        inner.source = thread.ok().map(|thread| Box::new(Running { stop, thread: Some(thread) }) as Box<dyn Send>);
    }

    /// Have the platform fetch pictures and hand them to the engine.
    fn capture(self: &Arc<Self>, inner: &mut Inner, from: CaptureFrom) {
        let core = Arc::clone(self);
        let started = self.platform.capture(from, Box::new(move |picture, taken| core.engine.push_frame(picture, taken)));
        match started {
            Ok(capture) => {
                // Told only now, so that a source that will not open never
                // looks live for a moment. A picture that came before this is
                // passed over; the next one is a thirtieth of a second away.
                self.engine.source_started();
                inner.source = Some(Box::new(Captured { capture: Some(capture), core: Arc::clone(self) }));
            }
            Err(problem) => inner.source_problem = Some(problem),
        }
    }

    // The face swap

    fn turn_off(&self, inner: &mut Inner) {
        self.engine.set_swap(None);
        inner.swap = SwapState::Off;
        inner.swap_detail.clear();
    }

    fn turn_on(self: &Arc<Self>, inner: &mut Inner) {
        let Some(identity) = self.selected(inner).map(|face| face.identity.clone()) else {
            inner.notice = Some(FaceNotice::AddAFaceFirst);
            inner.settings.swap_wanted = false;
            self.save_settings(inner);
            self.turn_off(inner);
            return;
        };
        let quality = inner.settings.quality;
        let config = self.live_config(quality);
        if let Some(missing) = config.missing_file() {
            self.engine.set_swap(None);
            inner.swap = SwapState::Missing;
            inner.swap_detail = missing;
            return;
        }
        if let Some(stage) = inner.stage.as_ref().filter(|stage| stage.quality == quality) {
            stage.engine.set_identity(Some(identity));
            stage.engine.set_settings(inner.settings.live());
            self.engine.set_swap(Some(Arc::clone(&stage.engine) as Arc<dyn SwapStage>));
            inner.swap = SwapState::On;
            inner.swap_detail.clear();
            return;
        }
        // Loading compiles the models the first time, which takes several seconds.
        inner.stage = None;
        inner.swap = SwapState::Loading;
        inner.swap_detail.clear();
        inner.loading += 1;
        let (core, attempt) = (Arc::clone(self), inner.loading);
        let spawned = std::thread::Builder::new().name("swap models".into()).spawn(move || {
            let loaded = LiveEngine::new(&config);
            let mut inner = core.lock();
            if inner.loading != attempt {
                // The user changed their mind while this loaded; a newer attempt is on its way.
                return;
            }
            match loaded {
                Ok(engine) => {
                    inner.stage = Some(Stage { quality, engine: Arc::new(engine) });
                    match inner.settings.swap_wanted {
                        true => core.turn_on(&mut inner),
                        false => core.turn_off(&mut inner),
                    }
                }
                Err(error) => {
                    inner.swap = SwapState::Failed;
                    inner.swap_detail = error.to_string();
                }
            }
        });
        if spawned.is_err() {
            inner.swap = SwapState::Failed;
            inner.swap_detail = "no thread could be started".into();
        }
    }

    /// Hand the face swap whatever the settings and the chosen face now say.
    fn refresh_swap(self: &Arc<Self>, inner: &mut Inner) {
        self.engine.update({
            let (badge, loop_badge, mirror) = (inner.settings.show_ai_badge, inner.settings.show_loop_badge, inner.settings.mirror);
            move |settings| {
                settings.show_ai_badge = badge;
                settings.show_loop_badge = loop_badge;
                settings.mirror = mirror;
            }
        });
        if inner.settings.swap_wanted {
            self.turn_on(inner);
        } else if inner.swap != SwapState::Off {
            self.turn_off(inner);
        }
    }

    fn reload_faces(&self, inner: &mut Inner) {
        inner.saved = inner.faces.as_ref().map_or_else(Vec::new, FaceLibrary::list);
        if self.selected(inner).is_none() {
            inner.settings.selected_face = inner.saved.first().map(|face| face.id.clone());
        }
    }

    fn open_faces(&self, inner: &mut Inner) {
        let opened = self.platform.key_store().load_or_create().and_then(|key| FaceLibrary::new(self.data_dir.join("Faces"), key));
        match opened {
            Ok(library) => inner.faces = Some(library),
            Err(_) => {
                inner.faces = None;
                inner.notice = Some(FaceNotice::FacesCouldNotBeOpened);
            }
        }
        self.reload_faces(inner);
    }
}

/// A picture file as packed blue, green, red, unused, no larger than full HD.
fn still_picture(path: &std::path::Path) -> Option<(Vec<u8>, u32, u32)> {
    let photo = faces::Photo::open(path).ok()?;
    let picture = image::RgbImage::from_raw(photo.width, photo.height, photo.rgb)?;
    let picture = match picture.width() > 1920 || picture.height() > 1080 {
        true => image::imageops::resize(&picture, 1920, 1920 * picture.height() / picture.width().max(1), image::imageops::FilterType::Triangle),
        false => picture,
    };
    // An even size, which the camera's picture format needs.
    let (width, height) = (picture.width() & !1, picture.height() & !1);
    if width < 16 || height < 16 {
        return None;
    }
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = picture.get_pixel(x, y).0;
            bgra.extend_from_slice(&[b, g, r, 255]);
        }
    }
    Some((bgra, width, height))
}

/// A source's thread: thirty pictures a second into the engine until told to stop.
fn deliver(core: &Core, stop: &AtomicBool, mut frames: Draw) {
    const PERIOD: Duration = Duration::from_nanos(33_333_333);
    core.engine.source_started();
    let started = Instant::now();
    let mut due = started;
    let mut picture = Vec::new();
    while !stop.load(Ordering::Acquire) {
        let (width, height) = frames(started.elapsed().as_secs_f32(), &mut picture);
        if let Some(view) = ImageView::new(&picture, width, height, width as usize * 4, PixelFormat::Bgra) {
            core.engine.push_frame(&view, core.platform.clock());
        }
        // On a schedule that does not drift.
        due += PERIOD;
        let now = Instant::now();
        match due.checked_duration_since(now) {
            Some(wait) => std::thread::sleep(wait),
            None if now.duration_since(due) > PERIOD => due = now,
            None => {}
        }
    }
    core.engine.source_stopped();
}

/// The app, without its window.
#[derive(uniffi::Object)]
pub struct DesktopApp {
    core: Arc<Core>,
}

impl DesktopApp {
    /// Make the app on `platform`. The window's constructor chooses the
    /// platform; tests bring their own.
    pub fn with_platform(config: AppConfig, platform: Arc<dyn Platform>) -> Arc<Self> {
        let data_dir = PathBuf::from(&config.data_dir);
        let _ = std::fs::create_dir_all(&data_dir);
        let settings = Settings::load(&data_dir.join("settings.json"));
        let clock = Arc::clone(&platform);
        let engine = CameraEngine::with_clock(platform.output(), move || clock.clock());
        let core = Arc::new(Core {
            pack_dirs: config.pack_dirs.iter().map(PathBuf::from).collect(),
            cache_dir: config.cache_dir.map(PathBuf::from),
            platform,
            engine,
            inner: Mutex::new(Inner {
                settings,
                source: None,
                source_problem: None,
                cameras: Vec::new(),
                preview_wanted: true,
                resting: false,
                clip: None,
                clip_saved: false,
                clips: ClipLibrary::new(data_dir.join("Loops")).ok(),
                faces: None,
                saved: Vec::new(),
                swap: SwapState::Off,
                swap_detail: String::new(),
                stage: None,
                loading: 0,
                pending: None,
                busy_adding: false,
                notice: None,
                listener: None,
                microphone_problem: None,
            }),
            finder: Mutex::new(None),
            data_dir,
        });
        {
            let mut inner = core.lock();
            core.open_faces(&mut inner);
            core.refresh_swap(&mut inner);
            core.restart_source(&mut inner);
        }
        // The newest app is the one whose picture `gc_app_preview` copies.
        *CURRENT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::downgrade(&core);
        Arc::new(Self { core })
    }

    /// Copy the outgoing picture into `into`: four bytes a pixel, blue, green,
    /// red, unused. `None` before the first picture.
    pub fn preview(&self, into: &mut Vec<u8>) -> Option<PreviewFrame> {
        self.core.engine.preview(into).map(|info| PreviewFrame { width: info.width, height: info.height, sequence: info.sequence })
    }

    /// Use `finder` to read photos instead of the models. For tests.
    #[doc(hidden)]
    pub fn set_face_finder(&self, finder: Box<dyn FindFace + Send>) {
        *self.core.finder.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(finder);
    }

    /// Wait until the engine has done everything asked of it so far. For tests.
    #[doc(hidden)]
    pub fn settle(&self) {
        self.core.engine.sync();
    }
}

#[uniffi::export]
impl DesktopApp {
    /// Start the app: open its data, ask the system for the virtual camera,
    /// and start the picture source chosen last time.
    #[uniffi::constructor]
    pub fn new(config: AppConfig) -> Result<Arc<Self>, AppError> {
        let platform = crate::platform::native(&config).map_err(|message| AppError::Failed { message })?;
        Ok(Self::with_platform(config, platform))
    }

    /// Everything the screens show. Cheap: ask about ten times a second.
    pub fn state(&self) -> AppState {
        let core = &self.core;
        let status = core.engine.status();
        let mut inner = core.lock();
        // A recording that has finished becomes the loop in hand.
        if let Some(clip) = core.engine.take_clips().pop() {
            inner.clip = Some(clip);
            inner.clip_saved = false;
        }
        core.rest_or_wake(&mut inner);
        core.listen_or_not(&mut inner, status.mode == Mode::Looping);
        let (virtual_camera, virtual_camera_detail) = core.platform.virtual_camera();
        let clip = inner.clip.as_ref().map(|clip| ClipInfo {
            id: clip.id.clone(),
            name: clip.name.clone(),
            duration: clip.duration(),
            loop_duration: clip.points.length() as f64 / clip.frame_rate,
            ends_as_it_began: clip.suggested_mode() == LoopMode::Forward,
            saved: inner.clip_saved,
        });
        let lighting_differs = inner.clip.as_ref().is_some_and(|clip| {
            status.mode == Mode::Live && status.live_luma > 0.0 && (clip.mean_luma() - status.live_luma).abs() > 25.0
        });
        let qualities: Vec<Quality> =
            [Quality::Standard, Quality::Sharper].into_iter().filter(|quality| core.live_config(*quality).missing_file().is_none()).collect();
        let settings = &inner.settings;
        AppState {
            mode: match status.mode {
                Mode::Idle => EngineMode::Idle,
                Mode::Live => EngineMode::Live,
                Mode::Looping => EngineMode::Looping,
            },
            frames_out: status.frames_out,
            source_fps: status.source_fps,
            is_recording: status.is_recording,
            recorded_seconds: status.recorded_seconds,
            swapping: status.swapping,
            face_found: status.face_found,
            swap_ms: status.swap_ms,
            swap_age_ms: status.swap_age_ms,
            swap_error: status.swap_error.clone(),
            picture_hidden: status.picture_hidden,
            watched: status.watched,
            output_width: status.output.0,
            output_height: status.output.1,

            source: settings.source,
            file_name: match settings.source {
                SourceKind::Video => settings.video.as_ref(),
                _ => settings.file.as_ref(),
            }
            .and_then(|file| PathBuf::from(file).file_name().map(|name| name.to_string_lossy().into_owned())),
            source_problem: inner.source_problem,
            cameras: inner.cameras.clone(),
            camera: settings.camera.clone().filter(|id| inner.cameras.iter().any(|camera| &camera.id == id)).or_else(|| inner.cameras.first().map(|camera| camera.id.clone())),
            mirror: settings.mirror,
            virtual_camera,
            virtual_camera_detail,
            resting: inner.resting,
            keep_running: settings.keep_running,

            can_record: status.mode == Mode::Live,
            can_loop: clip.is_some() && status.mode != Mode::Idle || status.mode == Mode::Looping,
            clip,
            saved_clips: inner.clips.as_ref().map_or_else(Vec::new, |library| {
                library.list().into_iter().map(|clip| SavedClipInfo { id: clip.id, name: clip.name, duration: clip.duration }).collect()
            }),
            loop_style: settings.loop_style,
            show_loop_badge: settings.show_loop_badge,
            lighting_differs,

            swap_wanted: settings.swap_wanted,
            swap_state: inner.swap,
            swap_detail: inner.swap_detail.clone(),
            policy_accepted: settings.policy_accepted.is_some(),
            faces: inner
                .saved
                .iter()
                .map(|face| FaceInfo {
                    id: face.id.clone(),
                    name: face.name.clone(),
                    consent: face.consent.into(),
                    photos: face.photos.iter().map(|photo| photo.id.clone()).collect(),
                })
                .collect(),
            selected_face: settings.selected_face.clone(),
            busy_adding: inner.busy_adding,
            pending: inner.pending.as_ref().map(|pending| PendingInfo {
                photos: pending
                    .photos
                    .iter()
                    .map(|photo| PendingPhotoInfo {
                        id: photo.id.clone(),
                        file_name: photo.file_name.clone(),
                        faces_in_photo: photo.faces_in_photo as u32,
                        used: photo.used,
                    })
                    .collect(),
                skipped: pending.skipped.clone(),
                target: pending.target.clone(),
                target_name: pending.target.as_ref().and_then(|id| inner.saved.iter().find(|face| &face.id == id)).map(|face| face.name.clone()),
            }),
            face_notice: inner.notice,
            quality: settings.quality,
            qualities,
            sharpen: settings.sharpen,
            colour_match: settings.colour_match,
            can_keep_objects_in_front: core.live_config(settings.quality).has_occlusion_model(),
            keep_objects_in_front: settings.keep_objects_in_front,
            real_mouth: settings.real_mouth,
            real_eyes: settings.real_eyes,
            blur_when_unswapped: settings.blur_when_unswapped,
            show_ai_badge: settings.show_ai_badge,
            back_to_live_on_voice: settings.back_to_live_on_voice,
            listening: inner.listener.is_some(),
            microphone_problem: inner.microphone_problem,
        }
    }

    // Camera

    /// Choose where the picture comes from. `file` is the picture or video to
    /// show, for the sources that show one; without it, the one chosen last time.
    pub fn set_source(&self, source: SourceKind, file: Option<String>) {
        let mut inner = self.core.lock();
        inner.settings.source = source;
        match (source, file) {
            (SourceKind::Video, Some(file)) => inner.settings.video = Some(file),
            (_, Some(file)) => inner.settings.file = Some(file),
            (_, None) => {}
        }
        self.core.save_settings(&inner);
        self.core.restart_source(&mut inner);
    }

    /// The user says the app may turn the camera on. Until this is called the
    /// camera stays off, whatever else is chosen.
    pub fn allow_camera(&self) {
        let mut inner = self.core.lock();
        inner.settings.camera_allowed = true;
        self.core.save_settings(&inner);
        if inner.settings.source == SourceKind::Camera {
            self.core.restart_source(&mut inner);
        }
    }

    /// Use this camera, one of `AppState::cameras`.
    pub fn set_camera(&self, id: String) {
        let mut inner = self.core.lock();
        inner.settings.camera = Some(id);
        self.core.save_settings(&inner);
        if inner.settings.source == SourceKind::Camera {
            self.core.restart_source(&mut inner);
        }
    }

    /// Look again for cameras: one may have been plugged in or pulled out.
    pub fn refresh_cameras(&self) {
        let cameras = self.core.platform.cameras();
        let mut inner = self.core.lock();
        let changed = inner.cameras != cameras;
        inner.cameras = cameras;
        // A camera that has just arrived is used at once if one was waited for.
        if changed && inner.settings.source == SourceKind::Camera && inner.source.is_none() && inner.settings.camera_allowed {
            self.core.restart_source(&mut inner);
        }
    }

    /// Ask the system for the virtual camera again. Call it after the camera's
    /// driver was installed or removed; the next state says how it went.
    pub fn retry_virtual_camera(&self) {
        self.core.platform.retry_virtual_camera();
    }

    /// Say whether the window shows the picture. While it does not, and no
    /// app reads the virtual camera, the source is switched off: a camera's
    /// light goes out. `state` has to be asked now and then for this to follow
    /// what reads the camera.
    pub fn set_preview_wanted(&self, wanted: bool) {
        let mut inner = self.core.lock();
        inner.preview_wanted = wanted;
        self.core.rest_or_wake(&mut inner);
    }

    /// Whether closing the window leaves the app running.
    pub fn set_keep_running(&self, keep: bool) {
        let mut inner = self.core.lock();
        inner.settings.keep_running = keep;
        self.core.save_settings(&inner);
    }

    /// Flip the live picture left to right.
    pub fn set_mirror(&self, mirror: bool) {
        let mut inner = self.core.lock();
        inner.settings.mirror = mirror;
        self.core.save_settings(&inner);
        self.core.engine.update(move |settings| settings.mirror = mirror);
    }

    // Loop

    /// Start a recording, or end the one that runs.
    pub fn toggle_recording(&self) {
        match self.core.engine.status().is_recording {
            true => self.core.engine.stop_recording(),
            false => self.core.engine.start_recording(),
        }
    }

    /// Play the loop in hand, or go back to the live picture.
    pub fn toggle_loop(&self) {
        if self.core.engine.status().mode == Mode::Looping {
            self.core.engine.stop_loop();
            return;
        }
        let inner = self.core.lock();
        if let Some(clip) = &inner.clip {
            let mut playing = Clip::clone(clip);
            playing.mode = match inner.settings.loop_style {
                LoopStyle::Automatic => clip.suggested_mode(),
                LoopStyle::Forward => LoopMode::Forward,
                LoopStyle::PingPong => LoopMode::PingPong,
            };
            self.core.engine.start_loop(Arc::new(playing));
        }
    }

    /// While a loop plays, listen to the microphone and go back to the live
    /// picture as soon as the user speaks. The microphone is open only while
    /// a loop plays.
    pub fn set_back_to_live_on_voice(&self, on: bool) {
        let mut inner = self.core.lock();
        inner.settings.back_to_live_on_voice = on;
        // Asked again, the microphone is tried again.
        inner.microphone_problem = None;
        self.core.save_settings(&inner);
        let looping = self.core.engine.status().mode == Mode::Looping;
        self.core.listen_or_not(&mut inner, looping);
    }

    /// How loops repeat. It takes effect the next time a loop starts.
    pub fn set_loop_style(&self, style: LoopStyle) {
        let mut inner = self.core.lock();
        inner.settings.loop_style = style;
        self.core.save_settings(&inner);
    }

    pub fn set_show_loop_badge(&self, show: bool) {
        let mut inner = self.core.lock();
        inner.settings.show_loop_badge = show;
        self.core.save_settings(&inner);
        self.core.engine.update(move |settings| settings.show_loop_badge = show);
    }

    /// Keep the loop in hand for later, under `name` if one is given.
    pub fn save_clip(&self, name: String) -> bool {
        let mut inner = self.core.lock();
        let (Some(clip), Some(library)) = (inner.clip.clone(), inner.clips.as_ref()) else { return false };
        let mut named = Clip::clone(&clip);
        if !name.trim().is_empty() {
            named.name = name.trim().to_string();
        }
        if library.save(&named).is_err() {
            return false;
        }
        inner.clip = Some(Arc::new(named));
        inner.clip_saved = true;
        true
    }

    /// Take a saved loop in hand.
    pub fn select_saved_clip(&self, id: String) -> bool {
        let mut inner = self.core.lock();
        let Some(Ok(clip)) = inner.clips.as_ref().map(|library| library.load(&id)) else { return false };
        inner.clip = Some(Arc::new(clip));
        inner.clip_saved = true;
        true
    }

    pub fn delete_saved_clip(&self, id: String) {
        let mut inner = self.core.lock();
        if let Some(library) = &inner.clips {
            library.delete(&id);
        }
        // The loop in hand stays in hand; it is only no longer saved.
        if inner.clip.as_ref().is_some_and(|clip| clip.id == id) {
            inner.clip_saved = false;
        }
    }

    // Face

    /// Turn the face swap on or off. Returns false, and changes nothing, if it
    /// is to be turned on and the user has not yet accepted the rules for it:
    /// show them, then call `accept_policy`.
    pub fn set_swap_wanted(&self, wanted: bool) -> bool {
        let mut inner = self.core.lock();
        if wanted && inner.settings.policy_accepted.is_none() {
            return false;
        }
        inner.settings.swap_wanted = wanted;
        inner.notice = None;
        self.core.save_settings(&inner);
        match wanted {
            true => self.core.turn_on(&mut inner),
            false => self.core.turn_off(&mut inner),
        }
        true
    }

    /// The user has read and accepted the rules for swapping a face. Turns the swap on.
    pub fn accept_policy(&self) {
        {
            let mut inner = self.core.lock();
            inner.settings.policy_accepted = Some(now_unix_seconds());
            self.core.save_settings(&inner);
        }
        self.set_swap_wanted(true);
    }

    pub fn select_face(&self, id: String) {
        let mut inner = self.core.lock();
        if !inner.saved.iter().any(|face| face.id == id) {
            return;
        }
        inner.settings.selected_face = Some(id);
        self.core.save_settings(&inner);
        if inner.settings.swap_wanted {
            self.core.turn_on(&mut inner);
        }
    }

    /// Read photos and put what was found in them into the state as
    /// `pending`: for a new face, or for more photos of the saved face `target`.
    pub fn add_photos(&self, paths: Vec<String>, target: Option<String>) {
        let core = Arc::clone(&self.core);
        let target_face = {
            let mut inner = core.lock();
            if inner.busy_adding || paths.is_empty() {
                return;
            }
            inner.notice = None;
            inner.busy_adding = true;
            target.and_then(|id| inner.saved.iter().find(|face| face.id == id).cloned())
        };
        let spawned = std::thread::Builder::new().name("photo reader".into()).spawn({
            let core = Arc::clone(&core);
            move || {
                let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                let mut finder = core.finder.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if finder.is_none() {
                    if let Some(missing) = Enroller::missing_file(&core.pack_dirs) {
                        let mut inner = core.lock();
                        inner.busy_adding = false;
                        inner.notice = Some(FaceNotice::ModelMissing);
                        inner.swap_detail = missing;
                        return;
                    }
                    *finder = Enroller::new(&core.pack_dirs, core.cache_dir.as_deref()).ok().map(|enroller| Box::new(enroller) as Box<dyn FindFace + Send>);
                }
                let Some(reader) = finder.as_mut() else {
                    let mut inner = core.lock();
                    inner.busy_adding = false;
                    inner.notice = Some(FaceNotice::FacesCouldNotBeOpened);
                    return;
                };
                let pending = read_photos(&paths, target_face.as_ref(), reader.as_mut());
                drop(finder);
                let mut inner = core.lock();
                inner.busy_adding = false;
                if pending.photos.is_empty() {
                    inner.notice = Some(match paths.len() {
                        1 => FaceNotice::NoFaceInThatPicture,
                        _ => FaceNotice::NoFaceInThosePictures,
                    });
                } else {
                    if pending.some_not_read {
                        inner.notice = Some(FaceNotice::TooManyPhotos);
                    }
                    inner.pending = Some(pending);
                }
            }
        });
        if spawned.is_err() {
            core.lock().busy_adding = false;
        }
    }

    /// Save the photos in hand: as a new face with `name` and `consent`, or
    /// into the saved face they were read for.
    pub fn save_pending(&self, name: String, consent: FaceConsent) -> bool {
        let mut inner = self.core.lock();
        let Some(pending) = inner.pending.clone() else { return false };
        let target = pending.target.as_ref().and_then(|id| inner.saved.iter().find(|face| &face.id == id)).cloned();
        let Some(mut face) = face_from_pending(&pending, target.as_ref(), &name, consent.into(), inner.saved.len() + 1) else {
            return false;
        };
        // The list is in the order of adding. Two faces added in the same
        // instant, or after the clock was set back, still keep that order.
        if target.is_none() {
            let newest = inner.saved.iter().map(|face| face.added).max().unwrap_or(0);
            face.added = face.added.max(newest + 1);
        }
        if inner.faces.as_ref().is_none_or(|library| library.save(&face).is_err()) {
            inner.notice = Some(FaceNotice::FaceCouldNotBeSaved);
            return false;
        }
        inner.pending = None;
        inner.settings.selected_face = Some(face.id.clone());
        self.core.save_settings(&inner);
        self.core.reload_faces(&mut inner);
        if inner.settings.swap_wanted {
            self.core.turn_on(&mut inner);
        }
        true
    }

    /// Discard the photos in hand.
    pub fn cancel_pending(&self) {
        self.core.lock().pending = None;
    }

    pub fn rename_face(&self, id: String, name: String) {
        let mut inner = self.core.lock();
        let name = name.trim();
        let Some(mut face) = inner.saved.iter().find(|face| face.id == id).cloned() else { return };
        if name.is_empty() {
            return;
        }
        face.name = name.to_string();
        if let Some(library) = &inner.faces {
            let _ = library.save(&face);
        }
        self.core.reload_faces(&mut inner);
    }

    /// Take one photo out of a face. The last photo stays; delete the face instead.
    pub fn remove_photo(&self, face_id: String, photo_id: String) {
        let mut inner = self.core.lock();
        let Some(changed) = inner.saved.iter().find(|face| face.id == face_id).and_then(|face| without_photo(face, &photo_id)) else {
            return;
        };
        if let Some(library) = &inner.faces {
            let _ = library.save(&changed);
        }
        self.core.reload_faces(&mut inner);
        if inner.settings.swap_wanted && inner.settings.selected_face.as_deref() == Some(face_id.as_str()) {
            self.core.turn_on(&mut inner);
        }
    }

    pub fn delete_face(&self, id: String) {
        let mut inner = self.core.lock();
        if let Some(library) = &inner.faces {
            library.delete(&id);
        }
        self.core.reload_faces(&mut inner);
        self.core.save_settings(&inner);
        // Without a face there is nothing to swap in.
        if inner.settings.swap_wanted {
            self.core.turn_on(&mut inner);
        }
    }

    pub fn set_quality(&self, quality: Quality) {
        let mut inner = self.core.lock();
        inner.settings.quality = quality;
        self.core.save_settings(&inner);
        // A different model needs a new stage.
        self.core.refresh_swap(&mut inner);
    }

    /// The options on the Face tab, all at once.
    pub fn set_face_options(&self, options: FaceOptions) {
        let mut inner = self.core.lock();
        let settings = &mut inner.settings;
        settings.sharpen = options.sharpen.clamp(0.0, 1.0);
        settings.colour_match = options.colour_match;
        settings.keep_objects_in_front = options.keep_objects_in_front;
        settings.real_mouth = options.real_mouth;
        settings.real_eyes = options.real_eyes;
        settings.blur_when_unswapped = options.blur_when_unswapped;
        let show_ai_badge = options.show_ai_badge;
        settings.show_ai_badge = show_ai_badge;
        self.core.save_settings(&inner);
        self.core.engine.update(move |settings| settings.show_ai_badge = show_ai_badge);
        if let Some(stage) = &inner.stage {
            stage.engine.set_settings(inner.settings.live());
        }
    }

    /// The small picture of a saved face, of one of its photos, or of a photo
    /// that waits to be saved, as PNG. Empty if there is none.
    pub fn face_picture(&self, face_id: String, photo_id: Option<String>) -> Vec<u8> {
        let inner = self.core.lock();
        let Some(face) = inner.saved.iter().find(|face| face.id == face_id) else { return Vec::new() };
        match photo_id {
            None => face.thumbnail().to_vec(),
            Some(photo_id) => face.photos.iter().find(|photo| photo.id == photo_id).map_or_else(Vec::new, |photo| photo.thumbnail.clone()),
        }
    }

    pub fn pending_picture(&self, photo_id: String) -> Vec<u8> {
        let inner = self.core.lock();
        inner
            .pending
            .as_ref()
            .and_then(|pending| pending.photos.iter().find(|photo| photo.id == photo_id))
            .map_or_else(Vec::new, |photo| photo.thumbnail.clone())
    }

    /// The most photos one face is made from.
    pub fn max_photos(&self) -> u32 {
        MAX_PHOTOS as u32
    }

    /// The user has seen the notice on the Face screen.
    pub fn dismiss_face_notice(&self) {
        self.core.lock().notice = None;
    }

    // Settings

    /// Remove every saved face and the key that protected them, every saved
    /// loop, and what the app remembered. This cannot be undone.
    pub fn delete_all_data(&self) {
        let mut inner = self.core.lock();
        self.core.engine.stop_loop();
        self.core.turn_off(&mut inner);
        inner.stage = None;
        inner.loading += 1;
        if let Some(library) = &inner.faces {
            library.delete_all();
        }
        self.core.platform.key_store().delete();
        if let Some(library) = &inner.clips {
            for clip in library.list() {
                library.delete(&clip.id);
            }
        }
        inner.clip = None;
        inner.clip_saved = false;
        inner.pending = None;
        inner.notice = None;
        inner.settings = Settings {
            source: inner.settings.source,
            camera: inner.settings.camera.clone(),
            camera_allowed: inner.settings.camera_allowed,
            keep_running: inner.settings.keep_running,
            ..Settings::default()
        };
        self.core.save_settings(&inner);
        if let Some(cache) = &self.core.cache_dir {
            let _ = std::fs::remove_dir_all(cache);
        }
        // A fresh key and an empty library, ready for new faces.
        self.core.open_faces(&mut inner);
        self.core.refresh_swap(&mut inner);
    }
}

impl Drop for DesktopApp {
    fn drop(&mut self) {
        // The source's thread holds the core; stopping it lets everything go.
        let source = self.core.lock().source.take();
        drop(source);
        self.core.engine.set_swap(None);
    }
}

/// The app whose picture `gc_app_preview` copies: the one made last.
static CURRENT: Mutex<Weak<Core>> = Mutex::new(Weak::new());

/// Copy the outgoing picture of the app made last into `into`: four bytes a
/// pixel, blue, green, red, unused. `None` if there is no app, no picture
/// yet, or not enough room.
///
/// The window calls this through a plain function in `gc-ffi`, beside the
/// generated interface: a picture thirty times a second should be copied
/// once, straight into the window's own memory.
pub fn copy_current_preview(into: &mut [u8]) -> Option<PreviewFrame> {
    let core = CURRENT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).upgrade()?;
    PREVIEW.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let info = core.engine.preview(&mut scratch)?;
        into.get_mut(..scratch.len())?.copy_from_slice(&scratch);
        Some(PreviewFrame { width: info.width, height: info.height, sequence: info.sequence })
    })
}

thread_local! {
    static PREVIEW: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}
