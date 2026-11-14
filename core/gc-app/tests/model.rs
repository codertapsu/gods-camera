//! The app's model, with stand-ins for the virtual camera, the key store and
//! the face models. No model files and no Windows are needed.

use gc_app::faces::{FaceError, FindFace, FoundFace, KeyStore};
use gc_app::model::{
    AppConfig, AppState, CameraChoice, CaptureFrom, DesktopApp, EngineMode, FaceConsent, FaceNotice, MicrophoneProblem, OnFrame, OnSound,
    Platform, SourceProblem, SwapState, VirtualCamera,
};
use gc_app::settings::{LoopStyle, Quality, SourceKind};
use gc_camera::Output;
use gc_infer::image::{ImageView, PixelFormat};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PATIENCE: Duration = Duration::from_secs(20);

/// Counts the pictures sent to the "virtual camera" and remembers the last one's size.
#[derive(Clone, Default)]
struct Camera {
    sent: Arc<AtomicU64>,
    last_size: Arc<Mutex<(u32, u32)>>,
    /// No app reads the camera.
    unwatched: Arc<AtomicBool>,
}

impl Output for Camera {
    fn wanted(&self) -> Option<(u32, u32)> {
        (!self.unwatched.load(Ordering::Relaxed)).then_some((640, 360))
    }

    fn send(&mut self, nv12: &[u8], width: u32, height: u32, _: u64) {
        assert_eq!(nv12.len(), (width * height * 3 / 2) as usize);
        *self.last_size.lock().unwrap() = (width, height);
        self.sent.fetch_add(1, Ordering::Relaxed);
    }
}

/// A key store in memory: the key lives as long as the test's platform.
#[derive(Clone, Default)]
struct Keys(Arc<Mutex<Option<[u8; 32]>>>);

impl KeyStore for Keys {
    fn load_or_create(&self) -> Result<[u8; 32], FaceError> {
        let mut key = self.0.lock().unwrap();
        Ok(*key.get_or_insert_with(gc_app::faces::new_key))
    }

    fn delete(&self) {
        *self.0.lock().unwrap() = None;
    }
}

/// The cameras a test has "attached", and what was opened.
#[derive(Clone, Default)]
struct Attached {
    cameras: Arc<Mutex<Vec<CameraChoice>>>,
    /// Every source the app asked to be opened, in order.
    opened: Arc<Mutex<Vec<CaptureFrom>>>,
}

/// A stand-in for a camera or a video file: grey pictures until it is dropped.
struct Delivering {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Delivering {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

/// A stand-in for the microphone: quiet, or "speech" while `speaking` is set.
#[derive(Clone, Default)]
struct Ears {
    speaking: Arc<AtomicBool>,
    /// The microphone is refused, as Windows' privacy settings would.
    refuse: Arc<AtomicBool>,
    /// How often the app asked to listen, and how many listeners are open now.
    asked: Arc<AtomicU64>,
    open: Arc<AtomicU64>,
}

/// Counts itself out of `Ears::open` when the app drops it.
struct Listening {
    _delivering: Delivering,
    open: Arc<AtomicU64>,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::Relaxed);
    }
}

struct Stand {
    camera: Camera,
    keys: Keys,
    attached: Attached,
    ears: Ears,
    started: Instant,
}

impl Platform for Stand {
    fn output(&self) -> Option<Box<dyn Output>> {
        Some(Box::new(self.camera.clone()))
    }

    fn clock(&self) -> u64 {
        self.started.elapsed().as_nanos() as u64
    }

    fn virtual_camera(&self) -> (VirtualCamera, String) {
        (VirtualCamera::Ready, String::new())
    }

    fn watched(&self) -> bool {
        !self.camera.unwatched.load(Ordering::Relaxed)
    }

    fn key_store(&self) -> Box<dyn KeyStore> {
        Box::new(self.keys.clone())
    }

    fn cameras(&self) -> Vec<CameraChoice> {
        self.attached.cameras.lock().unwrap().clone()
    }

    fn capture(&self, from: CaptureFrom, mut on_frame: OnFrame) -> Result<Box<dyn Send>, SourceProblem> {
        self.attached.opened.lock().unwrap().push(from.clone());
        // What is shown says where it came from: each camera has a brightness of its own.
        let level = match &from {
            CaptureFrom::Camera(id) if id == "broken" => return Err(SourceProblem::CameraUnavailable),
            CaptureFrom::Camera(id) => 40 + 60 * id.len() as u8,
            CaptureFrom::File(path) if !path.exists() => return Err(SourceProblem::FileUnreadable),
            CaptureFrom::File(_) => 230,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (stopped, started) = (Arc::clone(&stop), self.started);
        let thread = std::thread::spawn(move || {
            let picture = [level, level, level, 255].repeat(320 * 180);
            while !stopped.load(Ordering::Acquire) {
                let view = ImageView::new(&picture, 320, 180, 320 * 4, PixelFormat::Bgra).unwrap();
                on_frame(&view, started.elapsed().as_nanos() as u64);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(Box::new(Delivering { stop, thread: Some(thread) }))
    }

    fn listen(&self, mut on_sound: OnSound) -> Result<Box<dyn Send>, MicrophoneProblem> {
        self.ears.asked.fetch_add(1, Ordering::Relaxed);
        if self.ears.refuse.load(Ordering::Relaxed) {
            return Err(MicrophoneProblem::Refused);
        }
        self.ears.open.fetch_add(1, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let (stopped, speaking) = (Arc::clone(&stop), Arc::clone(&self.ears.speaking));
        // Blocks of 20 ms, as a microphone delivers them, as fast as the test can take them.
        let thread = std::thread::spawn(move || {
            let mut block = vec![0.0f32; 960];
            while !stopped.load(Ordering::Acquire) {
                let loudness = if speaking.load(Ordering::Relaxed) { 0.3 } else { 0.002 };
                for (n, sample) in block.iter_mut().enumerate() {
                    *sample = loudness * (n as f32 * 0.07).sin();
                }
                on_sound(&block, 0.02);
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Ok(Box::new(Listening { _delivering: Delivering { stop, thread: Some(thread) }, open: Arc::clone(&self.ears.open) }))
    }
}

/// Finds a "face" by the colour of a picture's first pixel: red is one
/// person, green another, and a dark picture has no face.
struct ByColour;

impl FindFace for ByColour {
    fn find(&mut self, picture: &ImageView<'_>) -> Result<Option<FoundFace>, String> {
        let pixel = picture.row(0);
        let person = match (pixel[0], pixel[1]) {
            (red, _) if red > 200 => 0,
            (_, green) if green > 200 => 1,
            _ => return Ok(None),
        };
        let mut identity = vec![0.01f32; 512];
        identity[person] = 1.0;
        identity[100] = pixel[2] as f32 / 255.0;
        let (width, height) = (picture.width as f32, picture.height as f32);
        Ok(Some(FoundFace { identity, faces: 1, bbox: [width * 0.25, height * 0.25, width * 0.75, height * 0.75] }))
    }
}

struct Fixture {
    app: Arc<DesktopApp>,
    camera: Camera,
    keys: Keys,
    attached: Attached,
    ears: Ears,
    folder: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let folder = std::env::temp_dir().join(format!("gc-app-model-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        Self::open(folder, Camera::default(), Keys::default(), Attached::default(), Ears::default())
    }

    /// Start the app on a data folder, as after closing and opening it again.
    fn open(folder: PathBuf, camera: Camera, keys: Keys, attached: Attached, ears: Ears) -> Self {
        let platform = Arc::new(Stand {
            camera: camera.clone(),
            keys: keys.clone(),
            attached: attached.clone(),
            ears: ears.clone(),
            started: Instant::now(),
        });
        let config = AppConfig {
            data_dir: folder.join("data").to_string_lossy().into_owned(),
            // No model files anywhere near here.
            pack_dirs: vec![folder.join("packs").to_string_lossy().into_owned()],
            cache_dir: Some(folder.join("cache").to_string_lossy().into_owned()),
            try_out: true,
        };
        let app = DesktopApp::with_platform(config, platform);
        app.set_face_finder(Box::new(ByColour));
        Self { app, camera, keys, attached, ears, folder }
    }

    fn reopen(self) -> Self {
        let (folder, camera, keys, attached, ears) =
            (self.folder.clone(), self.camera.clone(), self.keys.clone(), self.attached.clone(), self.ears.clone());
        drop(self);
        Self::open(folder, camera, keys, attached, ears)
    }

    /// The brightness of the picture the window would show.
    fn shown(&self) -> u8 {
        let mut preview = Vec::new();
        self.app.settle();
        self.app.preview(&mut preview).map_or(0, |_| preview[0])
    }

    fn until(&self, what: &str, mut done: impl FnMut(&AppState) -> bool) -> AppState {
        let started = Instant::now();
        loop {
            let state = self.app.state();
            if done(&state) {
                return state;
            }
            assert!(started.elapsed() < PATIENCE, "waited too long for {what}: {state:#?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn photo(&self, name: &str, colour: [u8; 3]) -> String {
        std::fs::create_dir_all(&self.folder).unwrap();
        let path = self.folder.join(name);
        image::RgbImage::from_pixel(64, 48, image::Rgb(colour)).save(&path).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _: &Path = &self.folder;
    }
}

#[test]
fn the_test_picture_goes_to_the_camera_in_the_size_asked_for() {
    let fixture = Fixture::new("pattern");
    // Nothing was chosen before: the camera is the source, and it is not on until the user allows it.
    let state = fixture.app.state();
    assert_eq!((state.source, state.source_problem, state.mode), (SourceKind::Camera, Some(SourceProblem::CameraNotAllowed), EngineMode::Idle));
    assert_eq!(state.virtual_camera, VirtualCamera::Ready);

    fixture.app.set_source(SourceKind::Pattern, None);
    let state = fixture.until("pictures from the test pattern", |state| state.frames_out >= 20);
    assert_eq!((state.mode, state.source_problem, state.watched), (EngineMode::Live, None, true));
    assert_eq!((state.output_width, state.output_height), (640, 360));
    assert_eq!(*fixture.camera.last_size.lock().unwrap(), (640, 360));
    assert!(fixture.camera.sent.load(Ordering::Relaxed) >= 20);
    assert!(state.can_record && !state.can_loop);

    // The window's preview is the same picture: light grey at the left, nearly black at the right.
    let mut preview = Vec::new();
    let frame = fixture.app.preview(&mut preview).expect("a preview");
    assert_eq!((frame.width, frame.height), (640, 360));
    let pixel = |x: usize, y: usize| preview[(y * 640 + x) * 4..][..3].to_vec();
    assert_eq!(pixel(5, 5), [191, 191, 191]);
    assert_eq!(pixel(634, 5), [16, 16, 16]);
    // Mirrored, the bars swap sides.
    fixture.app.set_mirror(true);
    fixture.until("the mirrored picture", |state| state.mirror && {
        fixture.app.settle();
        fixture.app.preview(&mut preview);
        preview[(5 * 640 + 5) * 4] == 16
    });

    // The pattern arrives thirty times a second.
    let state = fixture.until("the source's rate to be measured", |state| state.source_fps > 0.0);
    assert!((state.source_fps - 30.0).abs() < 3.0, "{} pictures a second", state.source_fps);
}

#[test]
fn a_picture_file_is_a_source_and_what_was_chosen_is_remembered() {
    let fixture = Fixture::new("image");
    fixture.app.set_source(SourceKind::Image, None);
    assert_eq!(fixture.app.state().source_problem, Some(SourceProblem::NoFileChosen));
    std::fs::create_dir_all(&fixture.folder).unwrap();
    let not_a_picture = fixture.folder.join("notes.txt");
    std::fs::write(&not_a_picture, b"words").unwrap();
    fixture.app.set_source(SourceKind::Image, Some(not_a_picture.to_string_lossy().into_owned()));
    assert_eq!(fixture.app.state().source_problem, Some(SourceProblem::FileUnreadable));

    let picture = fixture.photo("desk.png", [10, 200, 90]);
    fixture.app.set_source(SourceKind::Image, Some(picture));
    let state = fixture.until("pictures from the file", |state| state.frames_out >= 5);
    assert_eq!((state.mode, state.file_name.as_deref(), state.source_problem), (EngineMode::Live, Some("desk.png"), None));
    let mut preview = Vec::new();
    fixture.app.preview(&mut preview).unwrap();
    // Blue, green, red of the file's one colour, wherever it lands in the picture.
    assert_eq!(&preview[(180 * 640 + 320) * 4..][..3], [90, 200, 10]);

    fixture.app.set_mirror(true);
    let fixture = fixture.reopen();
    let state = fixture.until("the picture again after a restart", |state| state.frames_out >= 3);
    assert_eq!((state.source, state.file_name.as_deref(), state.mirror), (SourceKind::Image, Some("desk.png"), true));
}

#[test]
fn a_loop_is_recorded_played_saved_and_found_again() {
    let fixture = Fixture::new("loop");
    fixture.app.set_source(SourceKind::Pattern, None);
    fixture.until("the live picture", |state| state.can_record && state.frames_out > 3);

    fixture.app.toggle_recording();
    fixture.until("the recording to run for half a second", |state| state.is_recording && state.recorded_seconds >= 0.5);
    fixture.app.toggle_recording();
    let state = fixture.until("the clip", |state| state.clip.is_some());
    let clip = state.clip.unwrap();
    assert!(!state.is_recording && state.can_loop);
    assert!(!clip.saved && clip.name == "Loop 1" && clip.duration >= 0.5, "{clip:?}");
    assert!(clip.loop_duration > 0.0 && clip.loop_duration <= clip.duration);

    fixture.app.set_loop_style(LoopStyle::PingPong);
    fixture.app.set_show_loop_badge(true);
    fixture.app.toggle_loop();
    let state = fixture.until("the loop to play", |state| state.mode == EngineMode::Looping);
    assert!(state.can_loop && !state.can_record && state.show_loop_badge);
    assert_eq!(state.loop_style, LoopStyle::PingPong);
    let before = fixture.camera.sent.load(Ordering::Relaxed);
    fixture.until("the loop's pictures to reach the camera", |_| fixture.camera.sent.load(Ordering::Relaxed) > before + 10);
    fixture.app.toggle_loop();
    fixture.until("the live picture again", |state| state.mode == EngineMode::Live);

    assert!(fixture.app.save_clip("  At my desk ".into()));
    let state = fixture.app.state();
    assert_eq!(state.clip.as_ref().map(|clip| (clip.name.as_str(), clip.saved)), Some(("At my desk", true)));
    assert_eq!(state.saved_clips.iter().map(|clip| clip.name.as_str()).collect::<Vec<_>>(), ["At my desk"]);
    let saved_id = state.saved_clips[0].id.clone();

    // After a restart there is no loop in hand, and the saved one can be taken up.
    let fixture = fixture.reopen();
    let state = fixture.app.state();
    assert!(state.clip.is_none() && state.saved_clips.len() == 1);
    assert_eq!((state.loop_style, state.show_loop_badge), (LoopStyle::PingPong, true));
    assert!(fixture.app.select_saved_clip(saved_id.clone()));
    assert!(!fixture.app.select_saved_clip("no such loop".into()));
    let state = fixture.app.state();
    assert_eq!(state.clip.as_ref().map(|clip| (clip.name.as_str(), clip.saved)), Some(("At my desk", true)));

    // Deleting the saved copy leaves the loop in hand, unsaved.
    fixture.app.delete_saved_clip(saved_id);
    let state = fixture.app.state();
    assert!(state.saved_clips.is_empty());
    assert_eq!(state.clip.map(|clip| clip.saved), Some(false));
}

#[test]
fn speaking_while_a_loop_plays_goes_back_to_live() {
    let fixture = Fixture::new("voice");
    let ears = fixture.ears.clone();
    fixture.app.set_source(SourceKind::Pattern, None);
    fixture.until("the live picture", |state| state.can_record && state.frames_out > 3);
    fixture.app.toggle_recording();
    fixture.until("half a second recorded", |state| state.is_recording && state.recorded_seconds >= 0.5);
    fixture.app.toggle_recording();
    fixture.until("the clip", |state| state.clip.is_some());

    // With the option off, a loop plays and the microphone stays closed.
    fixture.app.toggle_loop();
    let state = fixture.until("the loop", |state| state.mode == EngineMode::Looping);
    assert!(!state.back_to_live_on_voice && !state.listening);
    assert_eq!(ears.asked.load(Ordering::Relaxed), 0);

    // Turned on while the loop plays, the microphone opens; quiet keeps the loop.
    fixture.app.set_back_to_live_on_voice(true);
    let state = fixture.until("the microphone to open", |state| state.listening);
    assert!(state.back_to_live_on_voice && state.microphone_problem.is_none());
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(fixture.app.state().mode, EngineMode::Looping);

    // Speech ends the loop by itself, and the microphone is closed again.
    ears.speaking.store(true, Ordering::Relaxed);
    fixture.until("the live picture again", |state| state.mode == EngineMode::Live && !state.listening);
    fixture.until("the microphone to close", |_| ears.open.load(Ordering::Relaxed) == 0);
    ears.speaking.store(false, Ordering::Relaxed);
    assert_eq!(ears.asked.load(Ordering::Relaxed), 1);

    // A refused microphone is said, and not asked for again while that loop plays.
    ears.refuse.store(true, Ordering::Relaxed);
    fixture.app.toggle_loop();
    fixture.until("the loop, without a microphone", |state| {
        state.mode == EngineMode::Looping && state.microphone_problem == Some(MicrophoneProblem::Refused)
    });
    for _ in 0..30 {
        assert!(!fixture.app.state().listening);
    }
    assert_eq!(ears.asked.load(Ordering::Relaxed), 2);
    assert_eq!(fixture.app.state().mode, EngineMode::Looping);
    fixture.app.toggle_loop();
    fixture.until("live once more", |state| state.mode == EngineMode::Live && state.microphone_problem.is_none());

    // The option is remembered.
    let fixture = fixture.reopen();
    assert!(fixture.app.state().back_to_live_on_voice);
}

#[test]
fn a_face_is_added_from_photos_kept_encrypted_and_chosen() {
    let fixture = Fixture::new("faces");
    let state = fixture.app.state();
    assert!(state.faces.is_empty() && state.selected_face.is_none() && !state.policy_accepted);
    assert_eq!(fixture.app.max_photos(), 20);

    // A photo without a face.
    fixture.app.add_photos(vec![fixture.photo("wall.png", [0, 0, 0])], None);
    let state = fixture.until("the photo to be read", |state| !state.busy_adding && state.face_notice.is_some());
    assert_eq!((state.face_notice, state.pending.is_some()), (Some(FaceNotice::NoFaceInThatPicture), false));
    fixture.app.dismiss_face_notice();

    // Three photos of one person and one of another.
    let photos = vec![
        fixture.photo("a.png", [255, 0, 10]),
        fixture.photo("b.png", [255, 0, 80]),
        fixture.photo("other.png", [0, 255, 0]),
        fixture.photo("c.png", [255, 0, 160]),
        fixture.photo("wall.png", [0, 0, 0]),
    ];
    fixture.app.add_photos(photos, None);
    let state = fixture.until("the photos to be read", |state| state.pending.is_some());
    let pending = state.pending.unwrap();
    assert_eq!(pending.photos.iter().map(|photo| (photo.file_name.as_str(), photo.used)).collect::<Vec<_>>(), [
        ("a.png", true),
        ("b.png", true),
        ("other.png", false),
        ("c.png", true)
    ]);
    assert_eq!((pending.skipped, pending.target), (vec!["wall.png".to_string()], None));
    let picture = fixture.app.pending_picture(pending.photos[0].id.clone());
    assert_eq!(image::load_from_memory(&picture).map(|small| (small.width(), small.height())).unwrap(), (160, 160));

    assert!(fixture.app.save_pending("Mai".into(), FaceConsent::Permission));
    let state = fixture.app.state();
    assert!(state.pending.is_none());
    let face = &state.faces[0];
    assert_eq!((face.name.as_str(), face.consent, face.photos.len()), ("Mai", FaceConsent::Permission, 3));
    assert_eq!(state.selected_face.as_deref(), Some(face.id.as_str()));
    assert!(!fixture.app.face_picture(face.id.clone(), None).is_empty());
    assert!(!fixture.app.face_picture(face.id.clone(), Some(face.photos[1].clone())).is_empty());
    assert!(fixture.app.face_picture("nobody".into(), None).is_empty());

    // A second face; then more photos for the first; rename, remove a photo, choose, delete.
    fixture.app.add_photos(vec![fixture.photo("an.png", [0, 255, 30])], None);
    fixture.until("the second face's photo", |state| state.pending.is_some());
    assert!(fixture.app.save_pending(String::new(), FaceConsent::Myself));
    let state = fixture.app.state();
    assert_eq!(state.faces.iter().map(|face| face.name.as_str()).collect::<Vec<_>>(), ["Mai", "Face 2"]);
    let (mai, second) = (state.faces[0].id.clone(), state.faces[1].id.clone());
    assert_eq!(state.selected_face.as_deref(), Some(second.as_str()), "the face just added is the chosen one");

    fixture.app.add_photos(vec![fixture.photo("d.png", [255, 0, 240])], Some(mai.clone()));
    let state = fixture.until("the extra photo", |state| state.pending.is_some());
    assert_eq!(state.pending.as_ref().and_then(|pending| pending.target_name.clone()).as_deref(), Some("Mai"));
    fixture.app.cancel_pending();
    assert!(fixture.app.state().pending.is_none());
    fixture.app.add_photos(vec![fixture.photo("d.png", [255, 0, 240])], Some(mai.clone()));
    fixture.until("the extra photo again", |state| state.pending.is_some());
    assert!(fixture.app.save_pending("ignored".into(), FaceConsent::Synthetic));
    let state = fixture.app.state();
    assert_eq!((state.faces[0].name.as_str(), state.faces[0].consent, state.faces[0].photos.len()), ("Mai", FaceConsent::Permission, 4));

    fixture.app.rename_face(mai.clone(), "  Mai Anh ".into());
    fixture.app.rename_face(mai.clone(), "   ".into());
    fixture.app.remove_photo(mai.clone(), state.faces[0].photos[0].clone());
    fixture.app.select_face(mai.clone());
    fixture.app.select_face("nobody".into());
    let state = fixture.app.state();
    assert_eq!((state.faces[0].name.as_str(), state.faces[0].photos.len()), ("Mai Anh", 3));
    assert_eq!(state.selected_face.as_deref(), Some(mai.as_str()));

    // On disk nothing of a face can be read without the key, and with it everything comes back.
    let faces_folder = fixture.folder.join("data").join("Faces");
    for file in std::fs::read_dir(&faces_folder).unwrap() {
        let bytes = std::fs::read(file.unwrap().path()).unwrap();
        assert!(!bytes.windows(3).any(|window| window == b"Mai"));
    }
    let fixture = fixture.reopen();
    let state = fixture.app.state();
    assert_eq!(state.faces.iter().map(|face| face.name.as_str()).collect::<Vec<_>>(), ["Mai Anh", "Face 2"]);
    assert_eq!(state.selected_face.as_deref(), Some(mai.as_str()));

    fixture.app.delete_face(mai);
    let state = fixture.app.state();
    assert_eq!(state.faces.len(), 1);
    assert_eq!(state.selected_face.as_deref(), Some(second.as_str()), "another face takes the deleted one's place");
}

#[test]
fn the_swap_asks_for_the_rules_first_and_says_what_it_lacks() {
    let fixture = Fixture::new("swap");
    assert_eq!(fixture.app.state().swap_state, SwapState::Off);
    // Not before the rules are accepted.
    assert!(!fixture.app.set_swap_wanted(true));
    assert!(!fixture.app.state().swap_wanted);

    // Accepted, but there is no face to swap in.
    fixture.app.accept_policy();
    let state = fixture.app.state();
    assert!(state.policy_accepted && !state.swap_wanted);
    assert_eq!((state.swap_state, state.face_notice), (SwapState::Off, Some(FaceNotice::AddAFaceFirst)));

    // With a face, the swap wants its models, and none are installed here.
    fixture.app.add_photos(vec![fixture.photo("a.png", [255, 0, 10])], None);
    fixture.until("the photo", |state| state.pending.is_some());
    assert!(fixture.app.save_pending("Mai".into(), FaceConsent::Myself));
    assert!(fixture.app.set_swap_wanted(true));
    let state = fixture.app.state();
    assert!(state.swap_wanted && state.face_notice.is_none());
    assert_eq!(state.swap_state, SwapState::Missing);
    assert!(state.swap_detail.ends_with(".onnx"), "the missing file is named: {}", state.swap_detail);
    assert!(state.qualities.is_empty() && !state.can_keep_objects_in_front);

    // The options are kept, also across a restart, and so is the wish to swap.
    fixture.app.set_quality(Quality::Sharper);
    fixture.app.set_face_options(0.8, false, false, true, true, false);
    let fixture = fixture.reopen();
    let state = fixture.app.state();
    assert_eq!((state.quality, state.sharpen, state.colour_match), (Quality::Sharper, 0.8, false));
    assert_eq!((state.keep_objects_in_front, state.real_mouth, state.blur_when_unswapped, state.show_ai_badge), (false, true, true, false));
    assert!(state.swap_wanted && state.policy_accepted);
    assert_eq!(state.swap_state, SwapState::Missing);

    assert!(fixture.app.set_swap_wanted(false));
    assert_eq!(fixture.app.state().swap_state, SwapState::Off);
}

#[test]
fn deleting_all_data_leaves_nothing_readable() {
    let fixture = Fixture::new("delete");
    fixture.app.set_source(SourceKind::Pattern, None);
    fixture.app.accept_policy();
    fixture.app.add_photos(vec![fixture.photo("a.png", [255, 0, 10])], None);
    fixture.until("the photo", |state| state.pending.is_some());
    assert!(fixture.app.save_pending("Mai".into(), FaceConsent::Myself));
    fixture.until("the live picture", |state| state.can_record && state.frames_out > 3);
    fixture.app.toggle_recording();
    fixture.until("some recording", |state| state.recorded_seconds >= 0.3);
    fixture.app.toggle_recording();
    fixture.until("the clip", |state| state.clip.is_some());
    assert!(fixture.app.save_clip("Kept".into()));
    let old_key = fixture.keys.load_or_create().unwrap();
    let face_files = || std::fs::read_dir(fixture.folder.join("data").join("Faces")).unwrap().count();
    assert_eq!(face_files(), 1);

    fixture.app.delete_all_data();
    let state = fixture.app.state();
    assert!(state.faces.is_empty() && state.saved_clips.is_empty() && state.clip.is_none());
    assert!(!state.policy_accepted && !state.swap_wanted && state.selected_face.is_none());
    assert_eq!(state.source, SourceKind::Pattern, "where the picture comes from is not personal data");
    assert_eq!(face_files(), 0);
    assert_ne!(fixture.keys.load_or_create().unwrap(), old_key, "the old key is gone for good");
}

#[test]
fn the_camera_stays_off_until_the_user_allows_it() {
    let fixture = Fixture::new("camera");
    let attach = |cameras: &[(&str, &str)]| {
        *fixture.attached.cameras.lock().unwrap() =
            cameras.iter().map(|(id, name)| CameraChoice { id: id.to_string(), name: name.to_string() }).collect();
        fixture.app.refresh_cameras();
    };
    attach(&[("a", "Desk camera"), ("bb", "Other camera")]);

    // The camera is the source from the first start, and nothing has opened it.
    let state = fixture.app.state();
    assert_eq!((state.source, state.source_problem, state.mode), (SourceKind::Camera, Some(SourceProblem::CameraNotAllowed), EngineMode::Idle));
    assert_eq!(state.cameras.iter().map(|camera| camera.name.as_str()).collect::<Vec<_>>(), ["Desk camera", "Other camera"]);
    assert_eq!(state.camera.as_deref(), Some("a"), "the first camera is the one that would be used");
    fixture.app.set_camera("bb".into());
    fixture.app.set_source(SourceKind::Pattern, None);
    fixture.app.set_source(SourceKind::Camera, None);
    assert!(fixture.attached.opened.lock().unwrap().is_empty(), "the camera was opened without being allowed");

    // Allowed: the chosen camera comes on.
    fixture.app.allow_camera();
    let state = fixture.until("the camera's picture", |state| state.mode == EngineMode::Live && fixture.shown() == 160);
    assert_eq!((state.source_problem, state.camera.as_deref()), (None, Some("bb")));
    assert_eq!(*fixture.attached.opened.lock().unwrap(), [CaptureFrom::Camera("bb".into())]);

    // Another camera is chosen: the first is let go, the other comes on.
    fixture.app.set_camera("a".into());
    fixture.until("the other camera's picture", |_| fixture.shown() == 100);
    assert_eq!(fixture.attached.opened.lock().unwrap().len(), 2);

    // After a restart the allowance and the choice hold, and the camera comes on by itself.
    let fixture = fixture.reopen();
    fixture.until("the camera after a restart", |state| state.mode == EngineMode::Live && state.frames_out > 2);
    assert_eq!(fixture.attached.opened.lock().unwrap().last(), Some(&CaptureFrom::Camera("a".into())));

    // The chosen camera is pulled out: the next start takes the one that is left.
    *fixture.attached.cameras.lock().unwrap() = vec![CameraChoice { id: "bb".into(), name: "Other camera".into() }];
    fixture.app.set_source(SourceKind::Camera, None);
    fixture.until("the remaining camera", |_| fixture.shown() == 160);

    // No camera at all, and one that will not open.
    *fixture.attached.cameras.lock().unwrap() = Vec::new();
    fixture.app.set_source(SourceKind::Camera, None);
    let state = fixture.until("the picture to stop", |state| state.mode == EngineMode::Idle);
    assert_eq!(state.source_problem, Some(SourceProblem::NoCamera));
    *fixture.attached.cameras.lock().unwrap() = vec![CameraChoice { id: "broken".into(), name: "Busy camera".into() }];
    fixture.app.set_source(SourceKind::Camera, None);
    assert_eq!(fixture.app.state().source_problem, Some(SourceProblem::CameraUnavailable));

    // A camera that is plugged in while one is waited for is used at once.
    *fixture.attached.cameras.lock().unwrap() = Vec::new();
    fixture.app.set_source(SourceKind::Camera, None);
    assert_eq!(fixture.app.state().source_problem, Some(SourceProblem::NoCamera));
    *fixture.attached.cameras.lock().unwrap() = vec![CameraChoice { id: "a".into(), name: "Desk camera".into() }];
    fixture.app.refresh_cameras();
    fixture.until("the camera that was plugged in", |state| state.mode == EngineMode::Live);
}

#[test]
fn a_video_file_is_a_source_of_its_own() {
    let fixture = Fixture::new("video");
    fixture.app.set_source(SourceKind::Video, None);
    assert_eq!(fixture.app.state().source_problem, Some(SourceProblem::NoFileChosen));
    fixture.app.set_source(SourceKind::Video, Some(fixture.folder.join("missing.mp4").to_string_lossy().into_owned()));
    let state = fixture.app.state();
    assert_eq!((state.source_problem, state.mode), (Some(SourceProblem::FileUnreadable), EngineMode::Idle));

    std::fs::create_dir_all(&fixture.folder).unwrap();
    let film = fixture.folder.join("holiday.mp4");
    std::fs::write(&film, b"stands in for a film").unwrap();
    fixture.app.set_source(SourceKind::Video, Some(film.to_string_lossy().into_owned()));
    let state = fixture.until("the film's pictures", |state| state.mode == EngineMode::Live && fixture.shown() == 230);
    assert_eq!((state.file_name.as_deref(), state.source_problem), (Some("holiday.mp4"), None));

    // The picture file chosen earlier and the film are remembered apart.
    let picture = fixture.photo("desk.png", [10, 200, 90]);
    fixture.app.set_source(SourceKind::Image, Some(picture));
    assert_eq!(fixture.app.state().file_name.as_deref(), Some("desk.png"));
    fixture.app.set_source(SourceKind::Video, None);
    let state = fixture.until("the film again", |_| fixture.shown() == 230);
    assert_eq!(state.file_name.as_deref(), Some("holiday.mp4"));
}

#[test]
fn the_camera_is_switched_off_while_nobody_looks() {
    let fixture = Fixture::new("resting");
    *fixture.attached.cameras.lock().unwrap() = vec![CameraChoice { id: "a".into(), name: "Desk camera".into() }];
    fixture.app.allow_camera();
    fixture.until("the camera's picture", |state| state.mode == EngineMode::Live && fixture.shown() == 100);
    let opened = || fixture.attached.opened.lock().unwrap().len();
    assert_eq!(opened(), 1);

    // The window is hidden, and an app still reads the camera: it stays on.
    fixture.app.set_preview_wanted(false);
    let state = fixture.app.state();
    assert!(!state.resting && state.mode == EngineMode::Live);

    // That app stops reading: nobody looks, so the camera goes off.
    fixture.camera.unwatched.store(true, Ordering::Relaxed);
    let state = fixture.until("the camera to be switched off", |state| state.resting && state.mode == EngineMode::Idle);
    assert_eq!(state.source_problem, None);
    let sent = fixture.camera.sent.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(fixture.camera.sent.load(Ordering::Relaxed), sent);
    assert_eq!(opened(), 1, "nothing opens the camera while it rests");

    // An app starts reading: the camera comes back on by itself.
    fixture.camera.unwatched.store(false, Ordering::Relaxed);
    let state = fixture.until("the camera to come back", |state| !state.resting && state.mode == EngineMode::Live);
    assert!(state.watched);
    assert_eq!(opened(), 2);

    // Nobody reads, and the window is shown again: on as well.
    fixture.camera.unwatched.store(true, Ordering::Relaxed);
    fixture.until("the camera to rest again", |state| state.resting);
    fixture.app.set_preview_wanted(true);
    fixture.until("the camera for the window", |state| !state.resting && state.mode == EngineMode::Live);

    // A loop that plays keeps going with the window hidden, and so does a recording.
    fixture.app.toggle_recording();
    fixture.until("some recording", |state| state.recorded_seconds >= 0.3);
    fixture.app.set_preview_wanted(false);
    assert!(!fixture.app.state().resting, "a recording under way is somebody looking");
    fixture.app.toggle_recording();
    fixture.until("the clip, and then rest", |state| state.clip.is_some() && state.resting);

    // The choice to keep running is remembered, and is on unless turned off.
    assert!(fixture.app.state().keep_running);
    fixture.app.set_keep_running(false);
    let fixture = fixture.reopen();
    assert!(!fixture.app.state().keep_running);
}
