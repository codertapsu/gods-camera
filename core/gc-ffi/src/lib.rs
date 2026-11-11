//! The core as the apps see it: a small interface that UniFFI turns into
//! Swift and Kotlin.
//!
//! Frames cross as an address and a length, not as a copy. Everything else is
//! small: a handful of numbers per frame, and a 128 or 256 pixel patch when a
//! new one is ready.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gc_live::{
    DetectorKind, Enroller, ImageView, LiveConfig, LiveEngine, LiveSettings, MaskShape, PixelFormat, SwapModel,
};

uniffi::setup_scaffolding!();

#[derive(Debug, uniffi::Error)]
pub enum CoreError {
    /// A model is missing or the inference library failed. The message says which.
    Failed { message: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<gc_live::InferError> for CoreError {
    fn from(error: gc_live::InferError) -> Self {
        CoreError::Failed { message: error.to_string() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SwapModelKind {
    Inswapper128,
    Hyperswap256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DetectorChoice {
    Large,
    Small,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MaskKind {
    Oval,
    Box,
}

/// How the bytes of one pixel are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PixelLayout {
    Bgra,
    Rgba,
    Bgr,
    Rgb,
}

impl From<PixelLayout> for PixelFormat {
    fn from(layout: PixelLayout) -> Self {
        match layout {
            PixelLayout::Bgra => PixelFormat::Bgra,
            PixelLayout::Rgba => PixelFormat::Rgba,
            PixelLayout::Bgr => PixelFormat::Bgr,
            PixelLayout::Rgb => PixelFormat::Rgb,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineConfig {
    /// Folders that hold model files, searched in order.
    pub packs: Vec<String>,
    /// Folder for compiled models, so later starts are fast.
    pub cache_dir: Option<String>,
    pub detector: DetectorChoice,
    /// Side of the detector's input square: 640 or 320.
    pub detector_size: u32,
    pub swap_model: SwapModelKind,
    pub cpu_only: bool,
}

#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct EngineSettings {
    pub mask: MaskKind,
    pub colour_match: bool,
    /// 0 shows the real face, 1 the swapped one.
    pub opacity: f32,
    pub score_threshold: f32,
    /// How much crisper to make the swapped face, 0 to 1.
    pub sharpen: f32,
    /// Keep a hand or an object in front of the face in front of the swap.
    /// Has an effect only when `EngineStats::occlusion` is true.
    pub occlusion: bool,
    /// Leave the real mouth showing through the swap.
    pub real_mouth: bool,
    /// Ask for the picture to be blurred while the swap is not fully on the
    /// face. The amount comes back with every frame as `FrameOutput::blur`.
    pub blur_when_unswapped: bool,
}

/// One camera frame, described by where its bytes are.
///
/// `address` is the memory address of the frame's first byte and `length` the
/// number of bytes readable from there. The caller keeps that memory alive
/// and unchanged until the call it is passed to returns; nothing keeps the
/// address afterwards.
#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct FrameMemory {
    pub address: u64,
    pub length: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes from the start of one row to the start of the next.
    pub stride: u32,
    pub layout: PixelLayout,
}

/// What to draw for one frame.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FrameOutput {
    pub face: bool,
    /// Maps patch pixels onto the frame: `x' = a x + b y + tx`, `y' = c x + d y + ty`.
    pub a: f32,
    pub b: f32,
    pub tx: f32,
    pub c: f32,
    pub d: f32,
    pub ty: f32,
    /// How strongly to draw the patch, 0 to 1.
    pub opacity: f32,
    /// The patch to draw, or 0 for none.
    pub patch_sequence: u64,
    pub patch_age_frames: u32,
    /// Five keypoints as x, y pairs, in frame pixels.
    pub keypoints: Vec<f32>,
    /// How strongly to blur the whole picture, 0 to 1, so that the real face
    /// does not show. Always 0 unless `EngineSettings::blur_when_unswapped` is set.
    pub blur: f32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PatchData {
    pub sequence: u64,
    /// Side of the square, in pixels.
    pub size: u32,
    /// Red, green, blue, and the mask as alpha; not premultiplied.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineStats {
    pub frames: u64,
    pub detections: u64,
    pub swaps: u64,
    pub frame_ms: f32,
    pub detect_ms: f32,
    pub swap_ms: f32,
    pub swap_age_ms: f32,
    pub tracking: bool,
    /// The occlusion model is loaded.
    pub occlusion: bool,
    pub last_error: Option<String>,
}

/// One identity made from several photos. See `gc_live::combine_identities`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CombinedFace {
    /// The identity to swap in, or empty when no photo could be used.
    pub identity: Vec<f32>,
    /// For each photo, in the order given: whether it went into the identity.
    /// A photo is left out when it shows somebody other than most of the others.
    pub used: Vec<bool>,
    /// For each photo: how alike its face and the combined identity are, -1 to 1.
    pub similarity: Vec<f32>,
}

/// Combine the identities of several photos of one person into one identity.
#[uniffi::export]
pub fn combine_identities(identities: Vec<Vec<f32>>) -> CombinedFace {
    let combined = gc_live::combine_identities(&identities);
    CombinedFace {
        identity: combined.identity,
        used: combined.photos.iter().map(|photo| photo.used).collect(),
        similarity: combined.photos.iter().map(|photo| photo.similarity).collect(),
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EnrolledFace {
    /// 512 numbers that stand for the face. Give them to `FaceEngine::set_identity`.
    pub identity: Vec<f32>,
    /// How many faces the picture shows. The largest was used.
    pub faces: u32,
    /// `[x1, y1, x2, y2]` of that face.
    pub bbox: Vec<f32>,
    pub score: f32,
    pub keypoints: Vec<f32>,
}

fn paths(folders: &[String]) -> Vec<PathBuf> {
    folders.iter().map(PathBuf::from).collect()
}

/// The live face-swap engine. See `gc_live::LiveEngine`.
#[derive(uniffi::Object)]
pub struct FaceEngine {
    inner: LiveEngine,
}

#[uniffi::export]
impl FaceEngine {
    /// Loads the models, which can take seconds. Call it off the main thread.
    #[uniffi::constructor]
    pub fn new(config: EngineConfig) -> Result<Arc<Self>, CoreError> {
        let inner = LiveEngine::new(&LiveConfig {
            packs: paths(&config.packs),
            cache_dir: config.cache_dir.map(PathBuf::from),
            detector: match config.detector {
                DetectorChoice::Large => DetectorKind::Large,
                DetectorChoice::Small => DetectorKind::Small,
            },
            detector_size: config.detector_size,
            swap_model: match config.swap_model {
                SwapModelKind::Inswapper128 => SwapModel::Inswapper128,
                SwapModelKind::Hyperswap256 => SwapModel::Hyperswap256,
            },
            cpu_only: config.cpu_only,
        })?;
        Ok(Arc::new(Self { inner }))
    }

    /// Side of the swapped patch, in pixels.
    pub fn crop_size(&self) -> u32 {
        self.inner.crop_size()
    }

    /// The face to swap in, or `None` to stop swapping.
    pub fn set_identity(&self, identity: Option<Vec<f32>>) {
        self.inner.set_identity(identity);
    }

    pub fn set_settings(&self, settings: EngineSettings) {
        self.inner.set_settings(LiveSettings {
            mask: match settings.mask {
                MaskKind::Oval => MaskShape::Oval,
                MaskKind::Box => MaskShape::Box,
            },
            colour_match: settings.colour_match,
            opacity: settings.opacity,
            score_threshold: settings.score_threshold,
            sharpen: settings.sharpen,
            occlusion: settings.occlusion,
            real_mouth: settings.real_mouth,
            blur_when_unswapped: settings.blur_when_unswapped,
        });
    }

    /// Take in one camera frame, captured at `time_ns` on any steady clock.
    /// See `FrameMemory` for what the caller promises about the frame's memory.
    pub fn push_frame(&self, frame: FrameMemory, time_ns: u64) -> FrameOutput {
        let none = FrameOutput {
            face: false,
            a: 1.0,
            b: 0.0,
            tx: 0.0,
            c: 0.0,
            d: 1.0,
            ty: 0.0,
            opacity: 0.0,
            patch_sequence: 0,
            patch_age_frames: 0,
            keypoints: Vec::new(),
            blur: 0.0,
        };
        if frame.address == 0 || frame.length == 0 {
            return none;
        }
        // SAFETY: the caller promises `length` readable bytes at `address` for
        // the length of this call, as `FrameMemory` states.
        let bytes = unsafe { std::slice::from_raw_parts(frame.address as usize as *const u8, frame.length as usize) };
        let Some(frame) =
            ImageView::new(bytes, frame.width, frame.height, frame.stride as usize, frame.layout.into())
        else {
            return none;
        };
        let result = self.inner.push_frame(&frame, time_ns);
        let m = result.crop_to_frame.0;
        FrameOutput {
            face: result.face,
            a: m[0][0],
            b: m[0][1],
            tx: m[0][2],
            c: m[1][0],
            d: m[1][1],
            ty: m[1][2],
            opacity: result.opacity,
            patch_sequence: result.patch_sequence,
            patch_age_frames: result.patch_age_frames,
            keypoints: result.keypoints.iter().flatten().copied().collect(),
            blur: result.blur,
        }
    }

    /// The newest patch, if its number is above `newer_than`. Ask with the
    /// number of the patch already in hand, so pixels cross only when they changed.
    pub fn patch(&self, newer_than: u64) -> Option<PatchData> {
        let patch = self.inner.patch()?;
        (patch.sequence > newer_than)
            .then(|| PatchData { sequence: patch.sequence, size: patch.size, rgba: patch.rgba.clone() })
    }

    pub fn stats(&self) -> EngineStats {
        let stats = self.inner.stats();
        EngineStats {
            frames: stats.frames,
            detections: stats.detections,
            swaps: stats.swaps,
            frame_ms: stats.frame_ms,
            detect_ms: stats.detect_ms,
            swap_ms: stats.swap_ms,
            swap_age_ms: stats.swap_age_ms,
            tracking: stats.tracking,
            occlusion: stats.occlusion,
            last_error: stats.last_error,
        }
    }
}

/// Turns a photo of a face into an identity. See `gc_live::Enroller`.
#[derive(uniffi::Object)]
pub struct FaceEnroller {
    inner: Mutex<Enroller>,
}

#[uniffi::export]
impl FaceEnroller {
    #[uniffi::constructor]
    pub fn new(packs: Vec<String>, cache_dir: Option<String>) -> Result<Arc<Self>, CoreError> {
        let inner = Enroller::new(&paths(&packs), cache_dir.as_deref().map(std::path::Path::new))?;
        Ok(Arc::new(Self { inner: Mutex::new(inner) }))
    }

    /// The identity of the largest face in a picture, or `None` if it shows no face.
    pub fn enrol(
        &self,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        stride: u32,
        layout: PixelLayout,
    ) -> Result<Option<EnrolledFace>, CoreError> {
        let picture = ImageView::new(&pixels, width, height, stride as usize, layout.into())
            .ok_or_else(|| CoreError::Failed { message: "the picture's size does not match its data".into() })?;
        let mut enroller = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Ok(enroller.enrol(&picture)?.map(|found| EnrolledFace {
            identity: found.identity,
            faces: found.faces as u32,
            bbox: found.bbox.to_vec(),
            score: found.score,
            keypoints: found.keypoints.iter().flatten().copied().collect(),
        }))
    }
}

/// The network side of the hub: where paired phones and tablets reach it.
#[derive(Clone, uniffi::Record)]
pub struct HubLan {
    /// The port to listen on, on every network interface of this computer.
    /// 0 lets the system pick one; `HubServer::lan_port` then says which.
    pub port: u16,
    /// The hub's certificate and private key, as `generate_hub_identity`
    /// returns them. Keep the same one between runs, somewhere only this
    /// app can read: paired devices trust that certificate only.
    pub identity_pem: String,
    /// The hub's name as a person knows it, such as the computer's name. A
    /// device shows it when it finds the hub.
    pub name: String,
}

/// A phone or tablet that has paired with the hub.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    /// When it paired, in seconds since 1970.
    pub paired_at: u64,
    /// When it last made a request, in seconds since 1970.
    pub last_seen: u64,
}

impl From<gc_link::DeviceInfo> for PairedDevice {
    fn from(device: gc_link::DeviceInfo) -> Self {
        Self { id: device.id, name: device.name, paired_at: device.paired_at, last_seen: device.last_seen }
    }
}

/// How the last pairing code stopped being good.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PairingOutcome {
    /// A device paired.
    Paired,
    /// Too many wrong codes were tried. Show a new code.
    TooManyAttempts,
    /// Nobody paired in time.
    Expired,
    /// The app closed pairing.
    Closed,
}

/// Where pairing stands, for the app to show.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairingState {
    /// True while a code is good.
    pub open: bool,
    /// The six digits to show, while a code is good.
    pub code: Option<String>,
    pub seconds_left: u32,
    /// Wrong codes that may still be tried before this code is thrown away.
    pub attempts_left: u32,
    /// How the last code ended. Nothing while a code is good, or before the first.
    pub ended: Option<PairingOutcome>,
    /// The device that paired last, since the hub started.
    pub paired: Option<PairedDevice>,
}

/// The hub: the server that phones and tablets send their work to.
///
/// It always listens on this computer's own loopback address, where a device
/// reaches it through a USB cable with `adb reverse` and presents the hub's
/// token. Started with `start_with_lan`, it also listens on the local network
/// over TLS, where a device first pairs with a six-digit code and from then on
/// presents a token of its own.
#[derive(uniffi::Object)]
pub struct HubServer {
    // Dropping the runtime stops the server's threads.
    _runtime: tokio::runtime::Runtime,
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    address: String,
    lan_port: Option<u16>,
    hub: gc_link::Hub,
}

impl HubServer {
    fn launch(
        port: u16,
        token: String,
        data_dir: String,
        ocr_command: Vec<String>,
        lan: Option<HubLan>,
    ) -> Result<Arc<Self>, CoreError> {
        let failed = |message: String| CoreError::Failed { message };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("gc-hub")
            .enable_all()
            .build()
            .map_err(|e| failed(e.to_string()))?;
        let config = gc_link::HubConfig {
            token,
            data_dir: PathBuf::from(data_dir),
            max_file_bytes: 64 << 20,
            allowed_origins: Vec::new(),
            ocr_command: (!ocr_command.is_empty()).then_some(ocr_command),
            lan: lan.map(|lan| gc_link::LanConfig {
                // Every interface, IPv6 and IPv4 alike.
                bind: std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, lan.port)),
                identity_pem: lan.identity_pem,
                name: lan.name,
            }),
        };
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let bound = runtime.block_on(gc_link::Hub::bind(address, config)).map_err(|e| failed(e.to_string()))?;
        let local = bound.local_addr().map_err(|e| failed(e.to_string()))?;
        let lan_port = bound.lan_addr().map(|address| address.port());
        let hub = bound.hub();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        runtime.spawn(async move {
            let _ = bound
                .serve_until(async {
                    let _ = stopped.await;
                })
                .await;
        });
        Ok(Arc::new(Self {
            _runtime: runtime,
            stop: Mutex::new(Some(stop)),
            address: format!("http://{local}"),
            lan_port,
            hub,
        }))
    }
}

#[uniffi::export]
impl HubServer {
    /// Start the hub on `port` of the loopback address only.
    ///
    /// `token` is what clients must present, at least 16 characters. Uploads
    /// and results are kept under `data_dir`. `ocr_command` is the program
    /// that reads text from pictures, with any leading arguments; when it is
    /// empty the hub refuses document jobs.
    #[uniffi::constructor]
    pub fn start(port: u16, token: String, data_dir: String, ocr_command: Vec<String>) -> Result<Arc<Self>, CoreError> {
        Self::launch(port, token, data_dir, ocr_command, None)
    }

    /// Start the hub on loopback, as `start` does, and on the local network
    /// as well, so that devices can pair and then reach it without a cable.
    #[uniffi::constructor]
    pub fn start_with_lan(
        port: u16,
        token: String,
        data_dir: String,
        ocr_command: Vec<String>,
        lan: HubLan,
    ) -> Result<Arc<Self>, CoreError> {
        Self::launch(port, token, data_dir, ocr_command, Some(lan))
    }

    /// Where the hub listens on loopback, as a URL.
    pub fn address(&self) -> String {
        self.address.clone()
    }

    /// The port of the network listener, when the hub has one. This is what
    /// the app advertises for devices to find.
    pub fn lan_port(&self) -> Option<u16> {
        self.lan_port
    }

    /// The SHA-256 of the hub's certificate as 64 hexadecimal digits, when
    /// the hub listens on the network.
    pub fn identity_fingerprint(&self) -> Option<String> {
        self.hub.fingerprint()
    }

    /// Show a new six-digit code and return it. It is good for `seconds`
    /// (0 means five minutes), for one device, and for a few wrong tries. A
    /// code that was showing stops working.
    pub fn open_pairing(&self, seconds: u32) -> Result<String, CoreError> {
        let ttl = match seconds {
            0 => gc_link::DEFAULT_PAIRING_TTL,
            seconds => std::time::Duration::from_secs(seconds.into()),
        };
        let code = self.hub.open_pairing(ttl).map_err(|e| CoreError::Failed { message: e.to_string() })?;
        Ok(code.digits().to_string())
    }

    /// Stop taking the code that is showing, if any.
    pub fn close_pairing(&self) {
        self.hub.close_pairing();
    }

    /// Where pairing stands. Cheap enough to ask a few times a second.
    pub fn pairing_status(&self) -> PairingState {
        let status = self.hub.pairing_status();
        PairingState {
            open: status.open,
            code: status.code.map(|code| code.digits().to_string()),
            seconds_left: status.seconds_left.min(u32::MAX.into()) as u32,
            attempts_left: status.attempts_left,
            ended: status.ended.map(|ended| match ended {
                gc_link::PairingEnd::Paired => PairingOutcome::Paired,
                gc_link::PairingEnd::TooManyAttempts => PairingOutcome::TooManyAttempts,
                gc_link::PairingEnd::Expired => PairingOutcome::Expired,
                gc_link::PairingEnd::Closed => PairingOutcome::Closed,
            }),
            paired: status.paired.map(PairedDevice::from),
        }
    }

    /// The paired devices, oldest first.
    pub fn devices(&self) -> Vec<PairedDevice> {
        self.hub.devices().into_iter().map(PairedDevice::from).collect()
    }

    /// Remove a paired device. Its token stops working at once. Returns false
    /// when there is no such device.
    pub fn revoke_device(&self, id: String) -> Result<bool, CoreError> {
        self.hub.revoke_device(&id).map_err(|e| CoreError::Failed { message: e.to_string() })
    }

    /// Stop taking requests. Requests in hand are finished first.
    pub fn stop(&self) {
        if let Some(stop) = self.stop.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = stop.send(());
        }
    }
}

/// Make a new identity for the hub's network listener: a certificate and its
/// private key, in PEM form. `name` goes into the certificate; the computer's
/// name does well.
///
/// The app makes one on first use and stores it where only it can read it,
/// then passes it to `HubServer::start_with_lan` every time. Making another
/// cuts off every paired device.
#[uniffi::export]
pub fn generate_hub_identity(name: String) -> Result<String, CoreError> {
    let identity = gc_link::gc_pair::HubIdentity::generate(&name).map_err(|e| CoreError::Failed { message: e.to_string() })?;
    Ok(identity.to_pem().to_string())
}

/// The SHA-256 of an identity's certificate, as 64 hexadecimal digits. Fails
/// when the text is not an identity from `generate_hub_identity`.
#[uniffi::export]
pub fn hub_identity_fingerprint(identity_pem: String) -> Result<String, CoreError> {
    let identity =
        gc_link::gc_pair::HubIdentity::from_pem(&identity_pem).map_err(|e| CoreError::Failed { message: e.to_string() })?;
    Ok(identity.fingerprint())
}

/// The version of the core, for diagnostics.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
