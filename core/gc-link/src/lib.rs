//! The hub: the desktop app's local server.
//!
//! This is the development-mode hub. It listens on loopback only, requires a
//! bearer token on every route except health, and rejects browser origins.
//! Paired mode (TLS, per-device keys) and the live video stream come later.
//!
//! Work that takes time runs as a job: the client starts it, asks how far it
//! is, and fetches the files it produced. Jobs are written to disk, so a
//! finished job can still be read after the hub restarts.

mod jobs;

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;
use tokio::io::AsyncWriteExt;

pub const API_VERSION: &str = "v1";

#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Bearer token every client must present.
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
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error("development mode only listens on loopback, not {0}")]
    NotLoopback(SocketAddr),
    #[error("the token must be at least 16 characters")]
    WeakToken,
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

#[derive(Clone)]
struct AppState {
    config: Arc<HubConfig>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    /// Heavy jobs run one at a time, so they do not starve the live picture.
    heavy: Arc<tokio::sync::Semaphore>,
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
async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();

    let origin_ok = match req.headers().get(header::ORIGIN).map(|v| v.to_str()) {
        None => true,
        Some(Ok(origin)) => state.config.allowed_origins.iter().any(|o| o == origin),
        Some(Err(_)) => false,
    };
    let is_health = req.uri().path() == "/v1/health";
    let token_ok = is_health || {
        let presented = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        presented.as_bytes().ct_eq(state.config.token.as_bytes()).into()
    };

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

async fn health() -> Json<Health> {
    Json(Health { status: "ok".into(), api: API_VERSION.into(), mode: "development".into() })
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

/// Build the hub's router. Exposed so tests can serve it on an ephemeral port.
pub fn router(config: HubConfig) -> Router {
    let jobs = Arc::new(Mutex::new(jobs::load(&config)));
    let state = AppState { config: Arc::new(config), jobs, heavy: Arc::new(tokio::sync::Semaphore::new(1)) };
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/uploads", post(create_upload))
        .route("/v1/uploads/{id}", get(get_upload))
        .route("/v1/uploads/{id}/files/{name}", put(put_file))
        .route("/v1/jobs", post(create_job))
        .route("/v1/jobs/{id}", get(get_job))
        .route("/v1/jobs/{id}/files/{name}", get(get_job_file))
        // File bodies are streamed and counted by the handler itself.
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Bind the development hub. Refuses any address that is not loopback.
pub async fn bind_dev(
    addr: SocketAddr,
    config: &HubConfig,
) -> Result<tokio::net::TcpListener, HubError> {
    if !addr.ip().is_loopback() {
        return Err(HubError::NotLoopback(addr));
    }
    if config.token.len() < 16 {
        return Err(HubError::WeakToken);
    }
    std::fs::create_dir_all(config.data_dir.join("uploads"))?;
    std::fs::create_dir_all(config.data_dir.join("jobs"))?;
    Ok(tokio::net::TcpListener::bind(addr).await?)
}

pub async fn serve(listener: tokio::net::TcpListener, config: HubConfig) -> Result<(), HubError> {
    axum::serve(listener, router(config)).await?;
    Ok(())
}
