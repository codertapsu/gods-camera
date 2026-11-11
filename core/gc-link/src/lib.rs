//! The hub: the desktop app's server for phones and tablets.
//!
//! It listens in two places.
//!
//! - On loopback, for development: plain HTTP, one bearer token on every route
//!   except health. A device reaches it through `adb reverse`.
//! - On the local network, when [`HubConfig::lan`] is set: TLS with the hub's
//!   own certificate. A device first pairs with the six-digit code the app
//!   shows, which gives it a token of its own and the certificate to trust.
//!   The loopback token is not accepted there.
//!
//! Both reject browser origins. The live video stream comes later.
//!
//! Work that takes time runs as a job: the client starts it, asks how far it
//! is, and fetches the files it produced. Jobs are written to disk, so a
//! finished job can still be read after the hub restarts.

mod jobs;
mod pairing;
pub mod tls;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Extension, Path, Query, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::StreamExt;
use gc_pair::{DeviceRegistry, HubIdentity, PairingCode};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::io::AsyncWriteExt;
use zeroize::Zeroize;

pub use gc_pair;
pub use pairing::{
    DeviceInfo, Hello, PairFinishRequest, PairFinishResponse, PairStartRequest, PairStartResponse, PairingEnd,
    PairingStatus, DEFAULT_PAIRING_TTL, MAX_ATTEMPTS,
};

pub const API_VERSION: &str = "v1";

/// The network listener: where it listens and who it is.
#[derive(Clone)]
pub struct LanConfig {
    /// The address to listen on. `[::]:47801` takes IPv6 and IPv4 alike, and
    /// falls back to IPv4 on a system without IPv6.
    pub bind: SocketAddr,
    /// The hub's certificate and private key, as [`HubIdentity::to_pem`]
    /// writes them. Keep the same one between runs: paired devices trust
    /// that certificate only.
    pub identity_pem: String,
    /// The hub's name as a person knows it, such as the computer's name. A
    /// device shows it when it finds the hub.
    pub name: String,
}

/// The private key never appears in debug output.
impl std::fmt::Debug for LanConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanConfig").field("bind", &self.bind).field("name", &self.name).finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct HubConfig {
    /// Bearer token every client on loopback must present.
    pub token: String,
    /// Where uploads are stored.
    pub data_dir: PathBuf,
    /// Largest single file accepted, in bytes.
    pub max_file_bytes: u64,
    /// Browser origins allowed to call the hub. Empty means none.
    pub allowed_origins: Vec<String>,
    /// The program that reads text from pictures, with its leading arguments.
    /// Picture paths are added after them, and it must print what `gc-ocr`
    /// prints. Without it, document jobs are refused.
    pub ocr_command: Option<Vec<String>>,
    /// Also listen on the local network, for paired devices. Without it the
    /// hub is reachable from this computer only.
    pub lan: Option<LanConfig>,
}

/// The token never appears in debug output.
impl std::fmt::Debug for HubConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubConfig")
            .field("data_dir", &self.data_dir)
            .field("max_file_bytes", &self.max_file_bytes)
            .field("allowed_origins", &self.allowed_origins)
            .field("ocr_command", &self.ocr_command)
            .field("lan", &self.lan)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error("development mode only listens on loopback, not {0}")]
    NotLoopback(SocketAddr),
    #[error("the token must be at least 16 characters")]
    WeakToken,
    #[error(transparent)]
    Identity(#[from] gc_pair::IdentityError),
    #[error("the hub's TLS could not be set up: {0}")]
    Tls(String),
    #[error("this hub does not listen on the network, so nothing can pair with it")]
    NoNetworkListener,
    #[error(transparent)]
    Devices(#[from] gc_pair::RegistryError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub status: String,
    pub api: String,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadInfo {
    pub id: String,
    pub files: Vec<FileInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileInfo {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobRequest {
    pub kind: String,
    pub upload_id: String,
}

/// A piece of work the hub does for a client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    /// `inventory` or `document`.
    pub kind: String,
    pub upload_id: String,
    /// `queued`, `running`, `done` or `failed`.
    pub status: String,
    /// How far a running job is, 0 to 1.
    #[serde(default)]
    pub progress: f32,
    /// A summary once the job is done.
    pub result: Option<serde_json::Value>,
    /// Files the job produced, fetched from `/v1/jobs/{id}/files/{name}`.
    #[serde(default)]
    pub files: Vec<String>,
    /// Why a failed job failed, in words fit for the user.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub request_id: String,
}

/// What the network listener tells devices about the hub.
struct LanInfo {
    name: String,
    /// SHA-256 of the hub's certificate, which the pairing exchange covers.
    fingerprint: String,
}

#[derive(Clone)]
struct AppState {
    config: Arc<HubConfig>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    /// Heavy jobs run one at a time, so they do not starve the live picture.
    heavy: Arc<tokio::sync::Semaphore>,
    /// The paired devices. When both are needed, the desk is locked first.
    registry: Arc<Mutex<DeviceRegistry>>,
    desk: Arc<Mutex<pairing::Desk>>,
    lan: Option<Arc<LanInfo>>,
}

/// A lock that a panic elsewhere does not take away: the data behind these
/// locks is whole after every statement that changes it.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Which listener a request came in on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// Loopback: this computer, or a device through `adb reverse`.
    Local,
    /// The local network, over TLS.
    Lan,
}

/// A generic error for the client; details stay in the server log.
struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Adds a request id, enforces the origin rule and the token, and makes every
/// error body the same shape.
///
/// On loopback the token is the hub's own. On the network it is a paired
/// device's, and the hub's own token is refused: it must not be worth
/// anything to someone who is not at this computer.
async fn guard(State((state, via)): State<(AppState, Via)>, mut req: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();

    let origin_ok = match req.headers().get(header::ORIGIN).map(|v| v.to_str()) {
        None => true,
        Some(Ok(origin)) => state.config.allowed_origins.iter().any(|o| o == origin),
        Some(Err(_)) => false,
    };
    let path = req.uri().path();
    let open_to_all = match via {
        Via::Local => path == "/v1/health",
        // What a device needs before it holds a token: to see the hub and to pair.
        Via::Lan => matches!(path, "/v1/hello" | "/v1/pair/start" | "/v1/pair/finish"),
    };
    let token_ok = open_to_all || {
        let presented = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        match via {
            Via::Local => presented.as_bytes().ct_eq(state.config.token.as_bytes()).into(),
            Via::Lan => {
                let mut registry = locked(&state.registry);
                match registry.authenticate(presented).map(|device| device.id.clone()) {
                    Some(id) => {
                        registry.touch(&id);
                        true
                    }
                    None => false,
                }
            }
        }
    };
    req.extensions_mut().insert(via);

    let response = if !origin_ok {
        ApiError(StatusCode::FORBIDDEN, "origin not allowed").into_response()
    } else if !token_ok {
        ApiError(StatusCode::UNAUTHORIZED, "missing or invalid token").into_response()
    } else {
        next.run(req).await
    };
    finish(response, request_id).await
}

async fn finish(response: Response, request_id: String) -> Response {
    let status = response.status();
    let mut out = if status.is_client_error() || status.is_server_error() {
        let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        let error = if text.is_empty() || text.starts_with('{') {
            status.canonical_reason().unwrap_or("error").to_lowercase()
        } else {
            text.into_owned()
        };
        (status, Json(ErrorBody { error, request_id: request_id.clone() })).into_response()
    } else {
        response
    };
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        out.headers_mut().insert("x-request-id", value);
    }
    out
}

/// On loopback anyone may ask. On the network only a paired device gets an
/// answer, which makes this the way for a device to check that it still is one.
async fn health(Extension(via): Extension<Via>) -> Json<Health> {
    let mode = match via {
        Via::Local => "development",
        Via::Lan => "paired",
    };
    Json(Health { status: "ok".into(), api: API_VERSION.into(), mode: mode.into() })
}

fn upload_dir(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    if !valid_id(id) {
        return Err(ApiError(StatusCode::NOT_FOUND, "no such upload"));
    }
    let dir = state.config.data_dir.join("uploads").join(id);
    if !dir.is_dir() {
        return Err(ApiError(StatusCode::NOT_FOUND, "no such upload"));
    }
    Ok(dir)
}

fn list_files(dir: &std::path::Path) -> std::io::Result<Vec<FileInfo>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(FileInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes: entry.metadata()?.len(),
            });
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

fn internal(context: &'static str) -> impl Fn(std::io::Error) -> ApiError {
    move |e| {
        eprintln!("hub: {context}: {e}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    }
}

async fn create_upload(State(state): State<AppState>) -> Result<Json<UploadInfo>, ApiError> {
    let id = uuid::Uuid::new_v4().to_string();
    let dir = state.config.data_dir.join("uploads").join(&id);
    tokio::fs::create_dir_all(&dir).await.map_err(internal("create upload"))?;
    Ok(Json(UploadInfo { id, files: vec![] }))
}

async fn get_upload(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<UploadInfo>, ApiError> {
    let dir = upload_dir(&state, &id)?;
    let files = list_files(&dir).map_err(internal("list upload"))?;
    Ok(Json(UploadInfo { id, files }))
}

#[derive(Deserialize)]
struct PutQuery {
    /// Byte offset this chunk starts at. It must equal the bytes already stored.
    #[serde(default)]
    offset: u64,
}

/// Append a chunk to a file. A client resumes by asking for the stored size
/// and sending the rest from that offset.
async fn put_file(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
    Query(q): Query<PutQuery>,
    body: Body,
) -> Result<Json<FileInfo>, ApiError> {
    let dir = upload_dir(&state, &id)?;
    if !valid_name(&name) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid file name"));
    }
    let path = dir.join(&name);
    let stored = match tokio::fs::metadata(&path).await {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(internal("stat file")(e)),
    };
    if q.offset != stored {
        return Err(ApiError(StatusCode::CONFLICT, "offset does not match stored size"));
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(internal("open file"))?;
    let mut total = stored;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "upload interrupted"))?;
        total += chunk.len() as u64;
        if total > state.config.max_file_bytes {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "file too large"));
        }
        file.write_all(&chunk).await.map_err(internal("write file"))?;
    }
    file.flush().await.map_err(internal("flush file"))?;
    Ok(Json(FileInfo { name, bytes: total }))
}

async fn create_job(
    State(state): State<AppState>,
    Json(req): Json<JobRequest>,
) -> Result<Json<Job>, ApiError> {
    let dir = upload_dir(&state, &req.upload_id)?;
    let files = list_files(&dir).map_err(internal("list upload"))?;
    let mut job = Job {
        id: uuid::Uuid::new_v4().to_string(),
        kind: req.kind,
        upload_id: req.upload_id,
        status: "queued".into(),
        progress: 0.0,
        result: None,
        files: Vec::new(),
        error: None,
    };
    match job.kind.as_str() {
        // A summary of an upload: quick, so it is done before the answer goes out.
        "inventory" => {
            let bytes: u64 = files.iter().map(|f| f.bytes).sum();
            job.status = "done".into();
            job.progress = 1.0;
            job.result = Some(serde_json::json!({ "files": files.len(), "bytes": bytes }));
            jobs::store(&state, &job);
        }
        // Text from photos, merged into one document. Runs in the background.
        "document" => {
            if state.config.ocr_command.is_none() {
                return Err(ApiError(StatusCode::NOT_IMPLEMENTED, "this hub has no text reader set up"));
            }
            let pictures: Vec<PathBuf> =
                files.iter().filter(|f| jobs::is_picture(&f.name)).map(|f| dir.join(&f.name)).collect();
            if pictures.is_empty() {
                return Err(ApiError(StatusCode::BAD_REQUEST, "the upload holds no pictures"));
            }
            jobs::store(&state, &job);
            tokio::spawn(jobs::run_document(state.clone(), job.id.clone(), pictures));
        }
        _ => return Err(ApiError(StatusCode::BAD_REQUEST, "unknown job kind")),
    }
    Ok(Json(job))
}

/// A file a job produced.
async fn get_job_file(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    if !valid_id(&id) || !valid_name(&name) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid name"));
    }
    let listed = state.jobs.lock().unwrap().get(&id).is_some_and(|job| job.files.contains(&name));
    if !listed {
        return Err(ApiError(StatusCode::NOT_FOUND, "no such file"));
    }
    let bytes = tokio::fs::read(jobs::job_dir(&state.config, &id).join(&name)).await.map_err(internal("read job file"))?;
    let kind = match name.rsplit('.').next() {
        Some("json") => "application/json",
        Some("md") => "text/markdown; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    };
    Ok(([(header::CONTENT_TYPE, kind)], bytes).into_response())
}

async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Job>, ApiError> {
    state
        .jobs
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(ApiError(StatusCode::NOT_FOUND, "no such job"))
}

impl AppState {
    /// Everything the hub keeps while it runs. `lan` is what the network
    /// listener says about the hub; without it nothing can pair.
    fn new(config: HubConfig, lan: Option<LanInfo>) -> Self {
        let jobs = Arc::new(Mutex::new(jobs::load(&config)));
        let registry = DeviceRegistry::open(&config.data_dir);
        if registry.was_damaged() {
            eprintln!("hub: the list of paired devices could not be read and was set aside; devices must pair again");
        }
        Self {
            config: Arc::new(config),
            jobs,
            heavy: Arc::new(tokio::sync::Semaphore::new(1)),
            registry: Arc::new(Mutex::new(registry)),
            desk: Arc::new(Mutex::new(pairing::Desk::default())),
            lan: lan.map(Arc::new),
        }
    }

    /// The routes as one listener serves them.
    fn router(&self, via: Via) -> Router {
        let work = Router::new()
            .route("/v1/health", get(health))
            .route("/v1/uploads", post(create_upload))
            .route("/v1/uploads/{id}", get(get_upload))
            .route("/v1/uploads/{id}/files/{name}", put(put_file))
            .route("/v1/jobs", post(create_job))
            .route("/v1/jobs/{id}", get(get_job))
            .route("/v1/jobs/{id}/files/{name}", get(get_job_file))
            // File bodies are streamed and counted by the handler itself.
            .layer(DefaultBodyLimit::disable());
        let routes = match via {
            Via::Local => work,
            Via::Lan => work.merge(
                Router::new()
                    .route("/v1/hello", get(pairing::hello))
                    .route("/v1/pair/start", post(pairing::pair_start))
                    .route("/v1/pair/finish", post(pairing::pair_finish))
                    // Anyone on the network can call these, so they take little.
                    .layer(DefaultBodyLimit::max(4096)),
            ),
        };
        routes.layer(middleware::from_fn_with_state((self.clone(), via), guard)).with_state(self.clone())
    }
}

fn check(local: SocketAddr, config: &HubConfig) -> Result<(), HubError> {
    if !local.ip().is_loopback() {
        return Err(HubError::NotLoopback(local));
    }
    if config.token.len() < 16 {
        return Err(HubError::WeakToken);
    }
    std::fs::create_dir_all(config.data_dir.join("uploads"))?;
    std::fs::create_dir_all(config.data_dir.join("jobs"))?;
    Ok(())
}

/// Listen on the network. The IPv6 "any" address `[::]` also takes IPv4, so it
/// serves both; on a system without IPv6 it falls back to IPv4 alone.
fn bind_lan(address: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    use std::io::ErrorKind::{AddrInUse, PermissionDenied};
    match bind_one(address) {
        Err(error) if address.is_ipv6() && address.ip().is_unspecified() && !matches!(error.kind(), AddrInUse | PermissionDenied) => {
            bind_one((std::net::Ipv4Addr::UNSPECIFIED, address.port()).into())
        }
        bound => bound,
    }
}

fn bind_one(address: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
    if address.is_ipv6() {
        let _ = socket.set_only_v6(false);
    }
    // So that the hub can come back at once after a restart. On Windows this
    // option means something else, and is left alone.
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(128)?;
    socket.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(socket.into())
}

/// A handle on a hub: what the app uses to pair devices and to list them.
/// Handles are cheap to copy and all speak to the same hub.
#[derive(Clone)]
pub struct Hub {
    state: AppState,
}

impl Hub {
    /// Set a hub up and bind its listeners: the loopback one on `local`, and
    /// the network one when [`HubConfig::lan`] is set. Nothing is served until
    /// [`BoundHub::serve`] or [`BoundHub::serve_until`] runs.
    pub async fn bind(local: SocketAddr, mut config: HubConfig) -> Result<BoundHub, HubError> {
        check(local, &config)?;
        let network = match &mut config.lan {
            Some(lan) => {
                let identity = HubIdentity::from_pem(&lan.identity_pem)?;
                // The TLS settings hold the key from here on; the text is not kept as well.
                lan.identity_pem.zeroize();
                let info = LanInfo { name: lan.name.clone(), fingerprint: identity.fingerprint() };
                Some((bind_lan(lan.bind)?, tls::server_config(&identity)?, info))
            }
            None => None,
        };
        let local = tokio::net::TcpListener::bind(local).await?;
        let (lan, info) = match network {
            Some((listener, tls, info)) => (Some((listener, tls)), Some(info)),
            None => (None, None),
        };
        Ok(BoundHub { hub: Hub { state: AppState::new(config, info) }, local, lan })
    }

    /// Show a new code, good for `ttl`, or until one device has paired or too
    /// many wrong codes were tried. A code that was showing stops working.
    pub fn open_pairing(&self, ttl: Duration) -> Result<PairingCode, HubError> {
        if self.state.lan.is_none() {
            return Err(HubError::NoNetworkListener);
        }
        Ok(locked(&self.state.desk).open(ttl, Instant::now()))
    }

    /// Stop taking the code that is showing, if any.
    pub fn close_pairing(&self) {
        locked(&self.state.desk).close();
    }

    pub fn pairing_status(&self) -> PairingStatus {
        locked(&self.state.desk).status(Instant::now())
    }

    /// The paired devices, oldest first.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        locked(&self.state.registry).list().iter().map(DeviceInfo::from).collect()
    }

    /// Remove a paired device. Its token stops working at once. Returns false
    /// when there is no such device.
    pub fn revoke_device(&self, id: &str) -> Result<bool, HubError> {
        Ok(locked(&self.state.registry).revoke(id)?)
    }

    /// The SHA-256 of the hub's certificate, when it listens on the network.
    pub fn fingerprint(&self) -> Option<String> {
        self.state.lan.as_ref().map(|lan| lan.fingerprint.clone())
    }
}

/// A hub whose listeners are bound but not yet served.
pub struct BoundHub {
    hub: Hub,
    local: tokio::net::TcpListener,
    lan: Option<(tokio::net::TcpListener, Arc<rustls::ServerConfig>)>,
}

impl BoundHub {
    pub fn hub(&self) -> Hub {
        self.hub.clone()
    }

    /// Where the loopback listener is.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.local.local_addr()
    }

    /// Where the network listener is, when there is one.
    pub fn lan_addr(&self) -> Option<SocketAddr> {
        self.lan.as_ref().and_then(|(listener, _)| listener.local_addr().ok())
    }

    pub async fn serve(self) -> Result<(), HubError> {
        self.serve_until(std::future::pending()).await
    }

    /// Serve both listeners until `stop` completes, then finish the requests
    /// in hand and return.
    pub async fn serve_until(self, stop: impl std::future::Future<Output = ()> + Send + 'static) -> Result<(), HubError> {
        let (stopping, _) = tokio::sync::watch::channel(false);
        let stopped = |mut told: tokio::sync::watch::Receiver<bool>| async move {
            let _ = told.wait_for(|stop| *stop).await;
        };
        let local = axum::serve(self.local, self.hub.state.router(Via::Local))
            .with_graceful_shutdown(stopped(stopping.subscribe()));
        let lan = match self.lan {
            Some((listener, tls)) => Some(
                axum::serve(tls::TlsListener::new(listener, tls, tls::MAX_CONNECTIONS, tls::IDLE_TIMEOUT)?, self.hub.state.router(Via::Lan))
                    .with_graceful_shutdown(stopped(stopping.subscribe())),
            ),
            None => None,
        };
        let stop = async {
            stop.await;
            let _ = stopping.send(true);
        };
        let lan = async {
            match lan {
                Some(lan) => lan.await,
                None => Ok(()),
            }
        };
        let (local, lan, ()) = tokio::join!(async { local.await }, lan, stop);
        local?;
        lan?;
        Ok(())
    }
}

/// Build the loopback listener's router. Exposed so tests can serve it on an
/// ephemeral port.
pub fn router(config: HubConfig) -> Router {
    AppState::new(config, None).router(Via::Local)
}

/// Bind the development hub. Refuses any address that is not loopback.
///
/// This and [`serve`] run the loopback listener alone, whatever
/// [`HubConfig::lan`] says. [`Hub::bind`] runs both.
pub async fn bind_dev(
    addr: SocketAddr,
    config: &HubConfig,
) -> Result<tokio::net::TcpListener, HubError> {
    check(addr, config)?;
    Ok(tokio::net::TcpListener::bind(addr).await?)
}

pub async fn serve(listener: tokio::net::TcpListener, config: HubConfig) -> Result<(), HubError> {
    axum::serve(listener, router(config)).await?;
    Ok(())
}

/// Serve until `stop` completes, then finish the requests in hand and return.
pub async fn serve_until(
    listener: tokio::net::TcpListener,
    config: HubConfig,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), HubError> {
    axum::serve(listener, router(config)).with_graceful_shutdown(stop).await?;
    Ok(())
}
