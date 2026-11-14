//! The scanner as the window sees it: the hub's switch, what a phone or
//! tablet needs to connect, and the documents received.
//!
//! The hub itself is `gc-link`. This is what the Mac app keeps in Swift
//! (`ScanHub` and `ScanModel`): starting and stopping the hub, its token and
//! certificate, the code a device connects with, and the list of documents.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, UNIX_EPOCH};

use gc_link::gc_pair::HubIdentity;
use gc_link::{Hub, HubConfig, LanConfig, PairingEnd};

use crate::secrets::Secrets;

/// Where devices attached by cable reach the hub, and where devices on the
/// local network do. The same ports as the Mac's hub.
pub const LOCAL_PORT: u16 = 47800;
pub const NETWORK_PORT: u16 = 47801;

/// What the secrets are kept under.
const TOKEN: &str = "HubToken";
const IDENTITY: &str = "HubIdentity";

#[derive(Debug, Clone, uniffi::Record)]
pub struct ScannerConfig {
    /// The app's data folder. What the hub receives is kept in `Hub` inside it.
    pub data_dir: String,
    /// The program that reads text from pictures, with its leading arguments.
    /// Empty when the app has none: the hub then refuses documents.
    pub reader: Vec<String>,
    /// The folders the reader finds its model files in.
    pub pack_dirs: Vec<String>,
    /// This program's own file. Windows Firewall's rules are made for it.
    pub program: String,
    /// A try-out on a scratch data folder. Its hub takes whatever ports are
    /// free and stays on this computer, and its secrets are kept in the data
    /// folder, so that nothing of the user's own app is touched or shared.
    pub try_out: bool,
}

/// Why the scanner is not running although it is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScannerProblem {
    /// The hub could not start. Another program may be using its port.
    CouldNotStart,
    /// The hub's token could not be kept or read.
    NoToken,
}

/// Whether phones and tablets on the local network can reach the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NetworkState {
    /// The scanner is off.
    Off,
    /// They can.
    Ready,
    /// The system's firewall has no rule for this app yet. The user allows it once.
    FirewallNeeded,
    /// This computer is only on networks that the system treats as public,
    /// where the hub does not listen.
    PublicNetwork,
    /// The hub could not listen on the network: its port is taken, or its
    /// certificate could not be made or kept.
    Failed,
}

/// What the system's firewall says about devices calling the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAccess {
    Allowed,
    /// A rule is missing. The user adds it with administrator rights.
    RuleNeeded,
    /// Only public networks are connected.
    PublicOnly,
}

/// An announcement of the hub on the local network. It lasts until dropped.
pub trait Announce: Send {
    /// Devices can find the hub; `None` while the system has not answered yet.
    fn up(&self) -> Option<bool>;
}

/// What the scanner needs from the system it runs on.
pub trait ScanPlatform: Send + Sync {
    fn secrets(&self) -> Arc<dyn Secrets>;
    /// This computer's name, as its user knows it. Devices show it.
    fn computer_name(&self) -> String;
    /// Whether the firewall lets devices on the network reach `program` on `port`.
    fn network_access(&self, _program: &Path, _port: u16) -> NetworkAccess {
        NetworkAccess::Allowed
    }
    /// Tell devices on the network where the hub is.
    fn announce(&self, _name: &str, _port: u16, _fingerprint: &str) -> Option<Box<dyn Announce>> {
        None
    }
    /// This computer's addresses on the local network, for a device that
    /// does not find the hub by itself and has to be told where it is.
    fn addresses(&self) -> Vec<String> {
        Vec::new()
    }
    /// Where the hub listens: the port for devices attached by cable, and
    /// the address and port for devices on the network.
    fn listening(&self) -> (u16, SocketAddr) {
        (LOCAL_PORT, SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), NETWORK_PORT))
    }
}

/// A phone or tablet that was connected with a code.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    /// When it was connected, in seconds since 1970.
    pub paired_at: u64,
    /// When it last sent something, if it has.
    pub last_seen: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DocumentStatus {
    Queued,
    Reading,
    Done,
    Failed,
}

/// A document made from pictures a device sent.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DocumentInfo {
    pub id: String,
    /// In seconds since 1970.
    pub received_at: u64,
    pub status: DocumentStatus,
    /// 0 to 1 while the text is being read.
    pub progress: f32,
    pub pictures: u32,
    /// The document's first line, or nothing until it is done.
    pub title: String,
    /// The folder with `document.md`, `document.txt` and `document.json`.
    pub folder: String,
    /// Why a failed document failed, in words fit for the user.
    pub error: Option<String>,
    /// What was read from each photo was kept, so a searchable PDF of the photos can be made.
    pub searchable: bool,
}

/// What a document can be saved as, besides its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DocumentExport {
    /// A Word document with the headings and paragraphs.
    Word,
    /// The text set as a PDF.
    Pdf,
    /// The photos as a PDF, with their text laid over them so it can be searched and copied.
    SearchablePdf,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ScannerState {
    /// The user has turned the scanner on. Kept between runs.
    pub wanted: bool,
    pub running: bool,
    pub problem: Option<ScannerProblem>,
    /// The name devices show for this computer.
    pub name: String,
    pub network: NetworkState,
    /// Devices find this computer by themselves. When false while the network
    /// is ready, the system has not announced the hub (yet).
    pub announced: bool,
    pub devices: Vec<DeviceInfo>,
    /// The six digits a device must enter, while a code is on offer.
    pub pairing_code: Option<String>,
    pub pairing_seconds_left: u32,
    /// The code on offer ran out or was guessed wrongly too often. A new one is needed.
    pub pairing_ended: bool,
    /// The device that connected with the code on offer.
    pub just_paired: Option<String>,
    /// Newest first.
    pub documents: Vec<DocumentInfo>,
    /// The reader's model file that is missing, if one is: documents cannot be read without it.
    pub reader_missing: Option<String>,
    /// Where a device attached by cable reaches the hub, for development.
    pub local_address: String,
    /// This computer's addresses on the local network, while the hub listens
    /// there: what to enter on a device that does not find it by itself.
    pub addresses: Vec<String>,
}

/// The hub while it runs.
struct Running {
    hub: Hub,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    local: SocketAddr,
    network_port: Option<u16>,
    announcement: Option<Box<dyn Announce>>,
    // Dropped last: it ends the hub's threads.
    _runtime: tokio::runtime::Runtime,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.announcement = None;
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

struct Inner {
    wanted: bool,
    running: Option<Running>,
    problem: Option<ScannerProblem>,
    network: NetworkState,
    /// A code was shown and has not been dealt with yet.
    offering: bool,
    pairing_ended: bool,
    just_paired: Option<String>,
    documents: Vec<DocumentInfo>,
    documents_read: Option<Instant>,
}

/// The scanner's model. The window asks for its state about once a second.
#[derive(uniffi::Object)]
pub struct Scanner {
    config: ScannerConfig,
    platform: Arc<dyn ScanPlatform>,
    inner: Mutex<Inner>,
    /// The app's camera, for paired devices to work from the Remote tab.
    camera: Mutex<Option<Arc<dyn gc_link::CameraRemote>>>,
}

/// What is kept between runs.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Kept {
    wanted: bool,
}

fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

impl Scanner {
    /// The model on a platform of the caller's choosing: tests bring stand-ins.
    pub fn with_platform(config: ScannerConfig, platform: Arc<dyn ScanPlatform>) -> Arc<Self> {
        let kept: Kept = std::fs::read(Path::new(&config.data_dir).join("scanner.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Arc::new(Self {
            config,
            platform,
            camera: Mutex::new(None),
            inner: Mutex::new(Inner {
                wanted: kept.wanted,
                running: None,
                problem: None,
                network: NetworkState::Off,
                offering: false,
                pairing_ended: false,
                just_paired: None,
                documents: Vec::new(),
                documents_read: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn hub_folder(&self) -> PathBuf {
        Path::new(&self.config.data_dir).join("Hub")
    }

    fn keep(&self, inner: &Inner) {
        let _ = std::fs::create_dir_all(&self.config.data_dir);
        let kept = serde_json::to_vec_pretty(&Kept { wanted: inner.wanted }).unwrap_or_default();
        let _ = std::fs::write(Path::new(&self.config.data_dir).join("scanner.json"), kept);
    }

    /// The hub's token, made on first use.
    fn token(&self) -> Result<String, String> {
        let secrets = self.platform.secrets();
        if let Some(token) = secrets.read(TOKEN)?.and_then(|bytes| String::from_utf8(bytes).ok()).filter(|token| token.len() >= 16) {
            return Ok(token);
        }
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
        let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        secrets.write(TOKEN, token.as_bytes(), "What a device attached by cable presents to God's Camera's scanner.")?;
        Ok(token)
    }

    /// The hub's certificate and private key, made on first use. Devices that
    /// were connected with a code trust this certificate and no other, so it
    /// must stay the same.
    fn identity(&self, name: &str) -> Result<String, String> {
        let secrets = self.platform.secrets();
        if let Some(identity) = secrets.read(IDENTITY)?.and_then(|bytes| String::from_utf8(bytes).ok()) {
            if HubIdentity::from_pem(&identity).is_ok() {
                return Ok(identity);
            }
        }
        let identity = HubIdentity::generate(name).map_err(|error| error.to_string())?.to_pem().to_string();
        secrets.write(IDENTITY, identity.as_bytes(), "The certificate phones and tablets know God's Camera's scanner by.")?;
        Ok(identity)
    }

    fn turn_on(&self, inner: &mut Inner) {
        inner.problem = None;
        inner.running = None;
        let token = match self.token() {
            Ok(token) => token,
            Err(_) => {
                inner.problem = Some(ScannerProblem::NoToken);
                inner.network = NetworkState::Off;
                return;
            }
        };
        let name = self.platform.computer_name();
        // A try-out takes whatever ports are free and stays on this computer.
        let (local_port, network_at) = match self.config.try_out {
            true => (0, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
            false => self.platform.listening(),
        };
        // The network side starts only when the firewall is ready for it:
        // listening without a rule would make Windows put a question on the screen.
        let access = match self.config.try_out {
            true => NetworkAccess::Allowed,
            false => self.platform.network_access(Path::new(&self.config.program), network_at.port()),
        };
        let mut network = match access {
            NetworkAccess::Allowed => NetworkState::Ready,
            NetworkAccess::RuleNeeded => NetworkState::FirewallNeeded,
            NetworkAccess::PublicOnly => NetworkState::PublicNetwork,
        };
        let mut lan = None;
        if network == NetworkState::Ready {
            match self.identity(&name) {
                Ok(identity_pem) => lan = Some(LanConfig { bind: network_at, identity_pem, name: name.clone() }),
                Err(_) => network = NetworkState::Failed,
            }
        }
        // With the network side first; without it if that cannot start, so
        // that a device attached by cable is still served.
        let mut started = None;
        if lan.is_some() {
            started = self.launch(local_port, &token, lan);
            if started.is_none() {
                network = NetworkState::Failed;
            }
        }
        let Some(mut running) = started.or_else(|| self.launch(local_port, &token, None)) else {
            inner.problem = Some(ScannerProblem::CouldNotStart);
            inner.network = NetworkState::Off;
            return;
        };
        if let (false, Some(port), Some(fingerprint)) = (self.config.try_out, running.network_port, running.hub.fingerprint()) {
            running.announcement = self.platform.announce(&name, port, &fingerprint);
        }
        inner.network = network;
        inner.running = Some(running);
    }

    /// Start a hub with its threads. `None` when a port is taken.
    fn launch(&self, local_port: u16, token: &str, lan: Option<LanConfig>) -> Option<Running> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("gc-hub").enable_all().build().ok()?;
        let config = HubConfig {
            token: token.to_string(),
            data_dir: self.hub_folder(),
            max_file_bytes: 64 << 20,
            allowed_origins: Vec::new(),
            ocr_command: (!self.config.reader.is_empty()).then(|| self.config.reader.clone()),
            lan,
        };
        let bound = runtime.block_on(Hub::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, local_port)), config)).ok()?;
        let local = bound.local_addr().ok()?;
        let network_port = bound.lan_addr().map(|address| address.port());
        let hub = bound.hub();
        hub.set_camera_remote(self.camera.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone());
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        runtime.spawn(async move {
            let _ = bound
                .serve_until(async {
                    let _ = stopped.await;
                })
                .await;
        });
        Some(Running { hub, stop: Some(stop), local, network_port, announcement: None, _runtime: runtime })
    }

    fn turn_off(&self, inner: &mut Inner) {
        if let Some(running) = &inner.running {
            running.hub.close_pairing();
        }
        inner.running = None;
        inner.network = NetworkState::Off;
        inner.offering = false;
        inner.pairing_ended = false;
        inner.just_paired = None;
    }

    /// The documents the hub has made or is making, newest first.
    /// The photos of a document with their text laid over them, as one PDF.
    fn searchable_pdf(&self, folder: &Path) -> Result<Vec<u8>, String> {
        let job: gc_link::Job = std::fs::read(folder.join("job.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or("the document cannot be read")?;
        if !valid_id(&job.upload_id) {
            return Err("the document's photos are not known".into());
        }
        let pages: Vec<gc_doc::OcrPage> = std::fs::read(folder.join("pages.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or("this document was read before its photos' text was kept")?;
        let photos = self.hub_folder().join("uploads").join(&job.upload_id);
        let mut with_pictures = Vec::with_capacity(pages.len());
        for page in pages {
            // The hub keeps file names only; one that leads anywhere else is not a photo of this document.
            let name = Path::new(&page.path).file_name().map(|name| name.to_os_string()).unwrap_or_default();
            if name.is_empty() || Path::new(&name) != Path::new(&page.path) {
                return Err("the document's photos are not known".into());
            }
            let picture = std::fs::read(photos.join(&name)).map_err(|_| format!("the photo {} is missing", page.path))?;
            with_pictures.push((picture, page));
        }
        gc_doc::export::to_searchable_pdf(&with_pictures)
    }

    fn read_documents(&self) -> Vec<DocumentInfo> {
        let Ok(folders) = std::fs::read_dir(self.hub_folder().join("jobs")) else { return Vec::new() };
        let mut documents: Vec<DocumentInfo> = folders
            .flatten()
            .filter_map(|entry| {
                let folder = entry.path();
                let job: gc_link::Job = serde_json::from_slice(&std::fs::read(folder.join("job.json")).ok()?).ok()?;
                if job.kind != "document" {
                    return None;
                }
                let status = match job.status.as_str() {
                    "queued" => DocumentStatus::Queued,
                    "running" => DocumentStatus::Reading,
                    "done" => DocumentStatus::Done,
                    _ => DocumentStatus::Failed,
                };
                let title = match status {
                    DocumentStatus::Done => std::fs::read_to_string(folder.join("document.txt"))
                        .ok()
                        .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()).map(|line| line.chars().take(120).collect()))
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                let received_at = entry
                    .metadata()
                    .and_then(|data| data.created().or_else(|_| data.modified()))
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_secs());
                let pictures = job.result.as_ref().and_then(|result| result.get("pictures")).and_then(|count| count.as_u64()).unwrap_or(0) as u32;
                Some(DocumentInfo {
                    id: job.id,
                    received_at,
                    status,
                    progress: job.progress,
                    pictures,
                    title,
                    searchable: folder.join("pages.json").is_file(),
                    folder: folder.to_string_lossy().into_owned(),
                    error: job.error,
                })
            })
            .collect();
        documents.sort_by(|a, b| b.received_at.cmp(&a.received_at).then_with(|| a.id.cmp(&b.id)));
        documents
    }

    fn remove_document(&self, id: &str) {
        if !valid_id(id) {
            return;
        }
        let folder = self.hub_folder().join("jobs").join(id);
        // The pictures the document came from go with it.
        let upload = std::fs::read(folder.join("job.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<gc_link::Job>(&bytes).ok())
            .map(|job| job.upload_id)
            .filter(|upload| valid_id(upload));
        if let Some(upload) = upload {
            let _ = std::fs::remove_dir_all(self.hub_folder().join("uploads").join(upload));
        }
        let _ = std::fs::remove_dir_all(folder);
    }
}

#[uniffi::export]
impl Scanner {
    /// The scanner on this system.
    #[uniffi::constructor]
    pub fn new(config: ScannerConfig) -> Arc<Self> {
        let platform = crate::platform::scan_platform(&config);
        Self::with_platform(config, platform)
    }

    /// Let paired phones and tablets work `app`'s camera: the swap, the face,
    /// recording and the loop, as the controls under its picture do.
    pub fn set_camera(&self, app: Arc<crate::model::DesktopApp>) {
        let camera = crate::remote::camera(&app);
        *self.camera.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&camera));
        if let Some(running) = &self.lock().running {
            running.hub.set_camera_remote(Some(camera));
        }
    }

    /// Carry on from the last session. The hub stays off until the user has turned it on.
    pub fn start(&self) {
        let mut inner = self.lock();
        if inner.wanted && inner.running.is_none() {
            self.turn_on(&mut inner);
        }
    }

    pub fn set_wanted(&self, wanted: bool) {
        let mut inner = self.lock();
        inner.wanted = wanted;
        self.keep(&inner);
        match wanted {
            true => self.turn_on(&mut inner),
            false => self.turn_off(&mut inner),
        }
    }

    /// Look at the firewall again, after the user was asked to allow the
    /// app: when devices may now come in, the hub starts listening for them.
    pub fn refresh_network(&self) {
        let mut inner = self.lock();
        if inner.running.is_some() && inner.network != NetworkState::Ready {
            self.turn_on(&mut inner);
        }
    }

    /// Offer a new six-digit code, good for five minutes, for one device and
    /// a few wrong tries. Returns false when the hub does not listen on the network.
    pub fn begin_pairing(&self) -> bool {
        let mut inner = self.lock();
        let Some(running) = &inner.running else { return false };
        if running.hub.open_pairing(gc_link::DEFAULT_PAIRING_TTL).is_err() {
            return false;
        }
        inner.offering = true;
        inner.pairing_ended = false;
        inner.just_paired = None;
        true
    }

    /// Withdraw the code on offer, and forget how the last one ended.
    pub fn end_pairing(&self) {
        let mut inner = self.lock();
        if let Some(running) = &inner.running {
            running.hub.close_pairing();
        }
        inner.offering = false;
        inner.pairing_ended = false;
        inner.just_paired = None;
    }

    /// Take a device's right to send scans away. It takes effect at once.
    pub fn remove_device(&self, id: String) {
        if let Some(running) = &self.lock().running {
            let _ = running.hub.revoke_device(&id);
        }
    }

    pub fn state(&self) -> ScannerState {
        let mut inner = self.lock();
        // The folder is read at most once a second, however often the window asks.
        if inner.documents_read.is_none_or(|read| read.elapsed() >= Duration::from_secs(1)) {
            inner.documents = self.read_documents();
            inner.documents_read = Some(Instant::now());
        }
        let (mut pairing_code, mut pairing_seconds_left) = (None, 0);
        if inner.offering {
            match inner.running.as_ref().map(|running| running.hub.pairing_status()) {
                Some(status) if status.open => {
                    pairing_code = status.code.map(|code| code.digits().to_string());
                    pairing_seconds_left = status.seconds_left.min(u32::MAX.into()) as u32;
                }
                Some(status) => {
                    inner.offering = false;
                    match status.ended {
                        // The hub remembers the device that paired last; it counts for the code it used.
                        Some(PairingEnd::Paired) => inner.just_paired = status.paired.map(|device| device.name),
                        Some(PairingEnd::Closed) | None => {}
                        Some(PairingEnd::Expired | PairingEnd::TooManyAttempts) => inner.pairing_ended = true,
                    }
                }
                None => inner.offering = false,
            }
        }
        let running = inner.running.as_ref();
        let mut devices: Vec<DeviceInfo> = running
            .map(|running| running.hub.devices())
            .unwrap_or_default()
            .into_iter()
            .map(|device| DeviceInfo {
                last_seen: (device.last_seen > device.paired_at).then_some(device.last_seen),
                id: device.id,
                name: device.name,
                paired_at: device.paired_at,
            })
            .collect();
        devices.sort_by_key(|device| device.paired_at);
        let folders: Vec<PathBuf> = self.config.pack_dirs.iter().map(PathBuf::from).collect();
        ScannerState {
            wanted: inner.wanted,
            running: running.is_some(),
            problem: inner.problem,
            name: self.platform.computer_name(),
            network: inner.network,
            announced: running.and_then(|running| running.announcement.as_ref()).and_then(|announcement| announcement.up()).unwrap_or(false),
            devices,
            pairing_code,
            pairing_seconds_left,
            pairing_ended: inner.pairing_ended,
            just_paired: inner.just_paired.clone(),
            documents: inner.documents.clone(),
            reader_missing: match self.config.reader.is_empty() {
                true => Some(String::new()),
                false => gc_ocr::TextReader::missing_file(&folders),
            },
            local_address: running.map(|running| format!("http://{}", running.local)).unwrap_or_default(),
            addresses: match running.is_some_and(|running| running.network_port.is_some()) && !self.config.try_out {
                true => self.platform.addresses(),
                false => Vec::new(),
            },
        }
    }

    /// The port devices on the network reach the hub at, while it listens there.
    pub fn network_port(&self) -> Option<u16> {
        self.lock().running.as_ref().and_then(|running| running.network_port)
    }

    /// The text of a finished document.
    pub fn document_text(&self, id: String) -> String {
        match valid_id(&id) {
            true => std::fs::read_to_string(self.hub_folder().join("jobs").join(id).join("document.txt")).unwrap_or_default(),
            false => String::new(),
        }
    }

    /// Write a document to the file `to`. `None` when it is written;
    /// otherwise what went wrong, for the log.
    pub fn export_document(&self, id: String, kind: DocumentExport, to: String) -> Option<String> {
        if !valid_id(&id) {
            return Some("no such document".into());
        }
        let folder = self.hub_folder().join("jobs").join(&id);
        let bytes = match kind {
            DocumentExport::Word | DocumentExport::Pdf => {
                let text = match std::fs::read(folder.join("document.json")) {
                    Ok(text) => text,
                    Err(error) => return Some(format!("the document cannot be read: {error}")),
                };
                let document: gc_doc::Document = match serde_json::from_slice(&text) {
                    Ok(document) => document,
                    Err(error) => return Some(format!("the document cannot be read: {error}")),
                };
                match kind {
                    DocumentExport::Word => gc_doc::export::to_docx(&document, None),
                    _ => gc_doc::export::to_pdf(&document, None),
                }
            }
            DocumentExport::SearchablePdf => match self.searchable_pdf(&folder) {
                Ok(bytes) => bytes,
                Err(why) => return Some(why),
            },
        };
        std::fs::write(&to, bytes).err().map(|error| format!("{to} could not be written: {error}"))
    }

    /// The photos a document was read from, as files, in the order they were sent.
    pub fn document_photos(&self, id: String) -> Vec<String> {
        if !valid_id(&id) {
            return Vec::new();
        }
        let job: Option<gc_link::Job> = std::fs::read(self.hub_folder().join("jobs").join(&id).join("job.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let Some(upload) = job.map(|job| job.upload_id).filter(|upload| valid_id(upload)) else { return Vec::new() };
        let Ok(files) = std::fs::read_dir(self.hub_folder().join("uploads").join(upload)) else { return Vec::new() };
        let mut photos: Vec<String> = files
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png")))
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        photos.sort();
        photos
    }

    /// Remove a document and the pictures it came from.
    pub fn delete_document(&self, id: String) {
        self.remove_document(&id);
        self.lock().documents_read = None;
    }

    /// Remove every document, every connected device, and the hub's
    /// certificate: nothing a removed device holds is of any use afterwards.
    pub fn delete_everything(&self) {
        let mut inner = self.lock();
        let was_running = inner.running.is_some();
        self.turn_off(&mut inner);
        let _ = std::fs::remove_dir_all(self.hub_folder());
        let secrets = self.platform.secrets();
        secrets.delete(IDENTITY);
        secrets.delete(TOKEN);
        inner.documents.clear();
        inner.documents_read = None;
        if was_running {
            self.turn_on(&mut inner);
        }
    }
}
