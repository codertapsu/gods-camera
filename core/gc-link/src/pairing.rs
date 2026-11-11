//! Pairing at the hub: the window in which a code is good, and the routes a
//! device uses to pair.
//!
//! The person opens a window in the app, which shows a six-digit code. A
//! device may then pair once. The window closes when a device has paired,
//! after too many wrong tries, when its time is up, or when the person closes
//! it. The exchange itself is in the `gc-pair` crate.

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use base64::Engine;
use gc_pair::{DeviceRecord, DeviceRegistry, PairingCode, RegistryError, Responder};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::{locked, ApiError, AppState, API_VERSION};

/// How long a code is good for unless the app asks otherwise.
pub const DEFAULT_PAIRING_TTL: Duration = Duration::from_secs(300);
/// Wrong tries after which a code is thrown away. With a million codes, this
/// leaves someone who is guessing a chance of five in a million per code.
pub const MAX_ATTEMPTS: u32 = 5;
/// How long a device may take between its two requests.
const SESSION_TTL: Duration = Duration::from_secs(60);
/// Exchanges under way at once. The oldest gives way to a new one.
const MAX_SESSIONS: usize = 8;
/// Longest base64 text taken for a message of the exchange.
const MAX_MESSAGE_TEXT: usize = 256;

/// A paired device, as the app lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    /// Seconds since 1970.
    pub paired_at: u64,
    /// Seconds since 1970.
    pub last_seen: u64,
}

impl From<&DeviceRecord> for DeviceInfo {
    fn from(record: &DeviceRecord) -> Self {
        Self { id: record.id.clone(), name: record.name.clone(), paired_at: record.paired_at, last_seen: record.last_seen }
    }
}

/// How the last pairing window ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingEnd {
    /// A device paired.
    Paired,
    /// Too many wrong codes were tried. A new code is needed.
    TooManyAttempts,
    /// Nobody paired in time.
    Expired,
    /// The app closed the window.
    Closed,
}

/// Where pairing stands, for the app to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingStatus {
    /// True while a code is good.
    pub open: bool,
    /// The code to show, while the window is open.
    pub code: Option<PairingCode>,
    pub seconds_left: u64,
    /// Wrong tries the code still survives.
    pub attempts_left: u32,
    /// How the last window ended; nothing while one is open or before the first.
    pub ended: Option<PairingEnd>,
    /// The device that paired last, since the hub started.
    pub paired: Option<DeviceInfo>,
}

/// Why the hub turned a pairing request down.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// No code is good at the moment.
    Closed,
    /// The request is not one a device would send.
    Invalid,
    /// Wrong code, changed message, or an exchange the hub does not know.
    Failed,
    /// The device proved itself, but it could not be added to the list.
    NotSaved(RegistryError),
}

struct Session {
    id: String,
    responder: Responder,
    started: Instant,
}

struct Window {
    code: PairingCode,
    expires: Instant,
    attempts_left: u32,
    sessions: Vec<Session>,
}

/// The state of pairing. Time is passed in, so that tests need not wait.
#[derive(Default)]
pub(crate) struct Desk {
    window: Option<Window>,
    ended: Option<PairingEnd>,
    paired: Option<DeviceInfo>,
}

impl Desk {
    /// Open a window with a new code. A window that was open is replaced, and
    /// its code stops working.
    pub(crate) fn open(&mut self, ttl: Duration, now: Instant) -> PairingCode {
        let code = PairingCode::generate();
        self.window = Some(Window { code: code.clone(), expires: now + ttl, attempts_left: MAX_ATTEMPTS, sessions: Vec::new() });
        self.ended = None;
        code
    }

    pub(crate) fn close(&mut self) {
        if self.window.take().is_some() {
            self.ended = Some(PairingEnd::Closed);
        }
    }

    fn expire(&mut self, now: Instant) {
        if self.window.as_ref().is_some_and(|window| now >= window.expires) {
            self.window = None;
            self.ended = Some(PairingEnd::Expired);
        }
    }

    pub(crate) fn is_open(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.window.is_some()
    }

    pub(crate) fn status(&mut self, now: Instant) -> PairingStatus {
        self.expire(now);
        let window = self.window.as_ref();
        PairingStatus {
            open: window.is_some(),
            code: window.map(|window| window.code.clone()),
            // Rounded up: "0 seconds left" would read as closed.
            seconds_left: window.map_or(0, |window| (window.expires - now).as_millis().div_ceil(1000) as u64),
            attempts_left: window.map_or(0, |window| window.attempts_left),
            ended: self.ended,
            paired: self.paired.clone(),
        }
    }

    /// A device's first request. Returns the exchange's id and the hub's message.
    pub(crate) fn start(
        &mut self,
        device_name: &str,
        device_hello: &[u8],
        fingerprint: &str,
        now: Instant,
    ) -> Result<(String, Vec<u8>), Refusal> {
        self.expire(now);
        let window = self.window.as_mut().ok_or(Refusal::Closed)?;
        // What can go wrong here does not depend on the code, so saying that
        // the request is malformed gives nothing away.
        let (responder, hub_hello) =
            Responder::start(&window.code, device_name, device_hello, fingerprint).map_err(|_| Refusal::Invalid)?;
        window.sessions.retain(|session| now.duration_since(session.started) < SESSION_TTL);
        if window.sessions.len() >= MAX_SESSIONS {
            window.sessions.remove(0);
        }
        let id = uuid::Uuid::new_v4().to_string();
        window.sessions.push(Session { id: id.clone(), responder, started: now });
        Ok((id, hub_hello))
    }

    /// A device's second request: its confirmation. On success the device is
    /// on the list, the window is closed, and the hub's confirmation and the
    /// sealed grant are returned.
    pub(crate) fn finish(
        &mut self,
        session: &str,
        confirmation: &[u8],
        registry: &mut DeviceRegistry,
        now: Instant,
    ) -> Result<(Vec<u8>, Vec<u8>), Refusal> {
        self.expire(now);
        let window = self.window.as_mut().ok_or(Refusal::Closed)?;
        // An exchange is good for one try, whatever comes of it.
        let index = window.sessions.iter().position(|known| known.id == session).ok_or(Refusal::Failed)?;
        let session = window.sessions.remove(index);
        if now.duration_since(session.started) >= SESSION_TTL {
            return Err(Refusal::Failed);
        }
        let Ok(verified) = session.responder.verify(confirmation) else {
            // This was a guess at the code, or a meddled-with exchange. Either
            // way it uses up one of the code's tries.
            window.attempts_left -= 1;
            if window.attempts_left == 0 {
                self.window = None;
                self.ended = Some(PairingEnd::TooManyAttempts);
            }
            return Err(Refusal::Failed);
        };
        let grant = registry.add(verified.device_name()).map_err(Refusal::NotSaved)?;
        self.paired = registry.list().iter().find(|device| device.id == grant.device_id).map(DeviceInfo::from);
        self.window = None;
        self.ended = Some(PairingEnd::Paired);
        Ok(verified.grant(&grant))
    }
}

/// What `GET /v1/hello` answers: enough for a device to show which hub it found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The hub's name, as the person knows it.
    pub name: String,
    /// The version of the hub's core.
    pub version: String,
    pub api: String,
    /// True while the hub shows a code.
    pub pairing_open: bool,
}

/// The body of `POST /v1/pair/start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairStartRequest {
    /// The name the hub lists the device under, in its tidy form.
    pub device_name: String,
    /// The device's first message, in base64.
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairStartResponse {
    /// Names this exchange in the second request.
    pub session: String,
    /// The hub's message, in base64.
    pub message: String,
}

/// The body of `POST /v1/pair/finish`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairFinishRequest {
    pub session: String,
    /// The device's confirmation, in base64.
    pub confirmation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairFinishResponse {
    /// The hub's confirmation, in base64.
    pub confirmation: String,
    /// The sealed grant, in base64. Only the device can open it.
    pub grant: String,
}

/// Standard base64. Padding may be left off when sending to the hub.
const BASE64: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    base64::engine::GeneralPurposeConfig::new()
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
);

fn decode(text: &str) -> Result<Vec<u8>, ApiError> {
    if text.len() > MAX_MESSAGE_TEXT {
        return Err(INVALID);
    }
    BASE64.decode(text).map_err(|_| INVALID)
}

const INVALID: ApiError = ApiError(StatusCode::BAD_REQUEST, "invalid pairing request");
const TOO_LARGE: ApiError = ApiError(StatusCode::PAYLOAD_TOO_LARGE, "pairing request too large");
const CLOSED: ApiError = ApiError(StatusCode::FORBIDDEN, "pairing is closed");
const FAILED: ApiError = ApiError(StatusCode::FORBIDDEN, "pairing failed");

/// A body that is not the JSON a device would send. The details of what was
/// wrong with it are of no use to a device, and are not sent.
fn rejected(rejection: JsonRejection) -> ApiError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        TOO_LARGE
    } else {
        INVALID
    }
}

impl From<Refusal> for ApiError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Closed => CLOSED,
            Refusal::Invalid => INVALID,
            Refusal::Failed => FAILED,
            Refusal::NotSaved(RegistryError::Full) => ApiError(StatusCode::CONFLICT, "too many devices are paired"),
            Refusal::NotSaved(error) => {
                eprintln!("hub: a device could not be added: {error}");
                ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        }
    }
}

pub(crate) async fn hello(State(state): State<AppState>) -> Result<Json<Hello>, ApiError> {
    let lan = state.lan.as_ref().ok_or(ApiError(StatusCode::NOT_FOUND, "not found"))?;
    let pairing_open = locked(&state.desk).is_open(Instant::now());
    Ok(Json(Hello {
        name: lan.name.clone(),
        version: env!("CARGO_PKG_VERSION").into(),
        api: API_VERSION.into(),
        pairing_open,
    }))
}

pub(crate) async fn pair_start(
    State(state): State<AppState>,
    body: Result<Json<PairStartRequest>, JsonRejection>,
) -> Result<Json<PairStartResponse>, ApiError> {
    let lan = state.lan.as_ref().ok_or(CLOSED)?;
    let Json(request) = body.map_err(rejected)?;
    let device_hello = decode(&request.message)?;
    let (session, hub_hello) = locked(&state.desk).start(
        &request.device_name,
        &device_hello,
        &lan.fingerprint,
        Instant::now(),
    )?;
    Ok(Json(PairStartResponse { session, message: BASE64.encode(hub_hello) }))
}

pub(crate) async fn pair_finish(
    State(state): State<AppState>,
    body: Result<Json<PairFinishRequest>, JsonRejection>,
) -> Result<Json<PairFinishResponse>, ApiError> {
    let Json(request) = body.map_err(rejected)?;
    let confirmation = decode(&request.confirmation)?;
    // The desk before the list of devices, here and everywhere.
    let mut desk = locked(&state.desk);
    let mut registry = locked(&state.registry);
    let (hub_confirmation, grant) = desk.finish(&request.session, &confirmation, &mut registry, Instant::now())?;
    Ok(Json(PairFinishResponse { confirmation: BASE64.encode(hub_confirmation), grant: BASE64.encode(grant) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gc_pair::{certificate_fingerprint, Grant, Initiator};

    fn registry() -> (DeviceRegistry, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("gc-desk-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (DeviceRegistry::open(&dir), dir)
    }

    fn fingerprint() -> String {
        certificate_fingerprint(b"hub")
    }

    /// A device pairs with `code`. Returns what it was granted.
    fn pair(desk: &mut Desk, registry: &mut DeviceRegistry, code: &PairingCode, now: Instant) -> Result<Grant, Refusal> {
        let device = Initiator::new(code, "tablet").unwrap();
        let (session, hub_hello) = desk.start(device.device_name(), device.hello(), &fingerprint(), now)?;
        let (device, confirmation) = device.finish(&hub_hello, &fingerprint()).unwrap();
        let (hub_confirmation, grant) = desk.finish(&session, &confirmation, registry, now)?;
        Ok(device.accept(&hub_confirmation, &grant).expect("the hub's answer opens"))
    }

    fn wrong(code: &PairingCode) -> PairingCode {
        let other = (code.digits().parse::<u32>().unwrap() + 1) % 1_000_000;
        PairingCode::parse(&format!("{other:06}")).unwrap()
    }

    #[test]
    fn a_window_takes_one_device_and_then_closes() {
        let (mut registry, dir) = registry();
        let mut desk = Desk::default();
        let now = Instant::now();
        assert!(!desk.is_open(now));
        assert!(matches!(pair(&mut desk, &mut registry, &PairingCode::generate(), now), Err(Refusal::Closed)));
        let before = desk.status(now);
        assert_eq!((before.open, before.ended, before.attempts_left), (false, None, 0));

        let code = desk.open(DEFAULT_PAIRING_TTL, now);
        let status = desk.status(now);
        assert!(status.open);
        assert_eq!(status.code.as_ref(), Some(&code));
        assert_eq!(status.seconds_left, 300);
        assert_eq!(status.attempts_left, MAX_ATTEMPTS);
        assert_eq!(desk.status(now + Duration::from_millis(1500)).seconds_left, 299);

        let grant = pair(&mut desk, &mut registry, &code, now).unwrap();
        assert_eq!(registry.authenticate(&grant.token).unwrap().id, grant.device_id);
        let status = desk.status(now);
        assert!(!status.open);
        assert_eq!(status.code, None);
        assert_eq!(status.ended, Some(PairingEnd::Paired));
        assert_eq!(status.paired.as_ref().map(|device| device.name.as_str()), Some("tablet"));
        assert_eq!(status.paired.unwrap().id, grant.device_id);

        // The code is spent: a second device needs a new one.
        assert!(matches!(pair(&mut desk, &mut registry, &code, now), Err(Refusal::Closed)));
        assert_eq!(registry.list().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn five_wrong_codes_close_the_window() {
        let (mut registry, dir) = registry();
        let mut desk = Desk::default();
        let now = Instant::now();
        let code = desk.open(DEFAULT_PAIRING_TTL, now);
        let guess = wrong(&code);
        for tries_left in (0..MAX_ATTEMPTS).rev() {
            assert!(matches!(pair(&mut desk, &mut registry, &guess, now), Err(Refusal::Failed)));
            let status = desk.status(now);
            assert_eq!(status.attempts_left, tries_left);
            assert_eq!(status.open, tries_left > 0);
        }
        assert_eq!(desk.status(now).ended, Some(PairingEnd::TooManyAttempts));
        // Even the right code is refused now.
        assert!(matches!(pair(&mut desk, &mut registry, &code, now), Err(Refusal::Closed)));
        assert!(registry.list().is_empty());

        // A new code starts with a full count, and the old one stays dead.
        let fresh = desk.open(DEFAULT_PAIRING_TTL, now);
        assert_eq!(desk.status(now).attempts_left, MAX_ATTEMPTS);
        assert_eq!(desk.status(now).ended, None);
        assert!(pair(&mut desk, &mut registry, &fresh, now).is_ok());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn four_wrong_codes_leave_room_for_the_right_one() {
        let (mut registry, dir) = registry();
        let mut desk = Desk::default();
        let now = Instant::now();
        let code = desk.open(DEFAULT_PAIRING_TTL, now);
        for _ in 0..MAX_ATTEMPTS - 1 {
            assert!(matches!(pair(&mut desk, &mut registry, &wrong(&code), now), Err(Refusal::Failed)));
        }
        assert!(pair(&mut desk, &mut registry, &code, now).is_ok());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_code_runs_out_and_can_be_closed_or_replaced() {
        let (mut registry, dir) = registry();
        let mut desk = Desk::default();
        let now = Instant::now();
        let code = desk.open(Duration::from_secs(30), now);
        assert!(desk.is_open(now + Duration::from_secs(29)));
        let late = now + Duration::from_secs(30);
        assert!(matches!(pair(&mut desk, &mut registry, &code, late), Err(Refusal::Closed)));
        let status = desk.status(late);
        assert_eq!((status.open, status.ended, status.seconds_left), (false, Some(PairingEnd::Expired), 0));

        // A device that began in time but finishes after the code ran out is refused.
        let code = desk.open(Duration::from_secs(30), now);
        let device = Initiator::new(&code, "tablet").unwrap();
        let (session, hub_hello) = desk.start("tablet", device.hello(), &fingerprint(), now).unwrap();
        let (_, confirmation) = device.finish(&hub_hello, &fingerprint()).unwrap();
        assert!(matches!(desk.finish(&session, &confirmation, &mut registry, late), Err(Refusal::Closed)));

        let code = desk.open(DEFAULT_PAIRING_TTL, now);
        desk.close();
        assert_eq!(desk.status(now).ended, Some(PairingEnd::Closed));
        assert!(matches!(pair(&mut desk, &mut registry, &code, now), Err(Refusal::Closed)));
        desk.close();
        assert_eq!(desk.status(now).ended, Some(PairingEnd::Closed));

        // Opening again replaces the code that was showing.
        let first = desk.open(DEFAULT_PAIRING_TTL, now);
        let second = desk.open(DEFAULT_PAIRING_TTL, now);
        if first != second {
            assert!(matches!(pair(&mut desk, &mut registry, &first, now), Err(Refusal::Failed)));
        }
        assert!(pair(&mut desk, &mut registry, &second, now).is_ok());
        assert!(registry.list().len() == 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_exchange_is_good_for_one_try_within_a_minute() {
        let (mut registry, dir) = registry();
        let mut desk = Desk::default();
        let now = Instant::now();
        let code = desk.open(DEFAULT_PAIRING_TTL, now);

        // Too slow between the two requests.
        let device = Initiator::new(&code, "tablet").unwrap();
        let (session, hub_hello) = desk.start("tablet", device.hello(), &fingerprint(), now).unwrap();
        let (_, confirmation) = device.finish(&hub_hello, &fingerprint()).unwrap();
        assert!(matches!(desk.finish(&session, &confirmation, &mut registry, now + SESSION_TTL), Err(Refusal::Failed)));
        // Slowness is not a guess at the code.
        assert_eq!(desk.status(now).attempts_left, MAX_ATTEMPTS);
        // The exchange is gone: the same confirmation, in time, is refused too.
        assert!(matches!(desk.finish(&session, &confirmation, &mut registry, now), Err(Refusal::Failed)));

        // An exchange nobody started.
        assert!(matches!(desk.finish("no-such-exchange", &confirmation, &mut registry, now), Err(Refusal::Failed)));
        assert_eq!(desk.status(now).attempts_left, MAX_ATTEMPTS);

        // A wrong confirmation uses the exchange up; the right one afterwards is refused.
        let device = Initiator::new(&code, "tablet").unwrap();
        let (session, hub_hello) = desk.start("tablet", device.hello(), &fingerprint(), now).unwrap();
        let (_, confirmation) = device.finish(&hub_hello, &fingerprint()).unwrap();
        assert!(matches!(desk.finish(&session, &[0u8; 32], &mut registry, now), Err(Refusal::Failed)));
        assert_eq!(desk.status(now).attempts_left, MAX_ATTEMPTS - 1);
        assert!(matches!(desk.finish(&session, &confirmation, &mut registry, now), Err(Refusal::Failed)));
        assert_eq!(desk.status(now).attempts_left, MAX_ATTEMPTS - 1, "an unknown exchange is not a guess");
        assert!(registry.list().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn requests_no_device_would_send_are_called_invalid() {
        let mut desk = Desk::default();
        let now = Instant::now();
        let code = desk.open(DEFAULT_PAIRING_TTL, now);
        let device = Initiator::new(&code, "tablet").unwrap();
        for (name, hello) in [(" untidy ", device.hello()), ("", device.hello()), ("tablet", &b"short"[..]), ("tablet", &[0x42; 33][..])] {
            assert!(matches!(desk.start(name, hello, &fingerprint(), now), Err(Refusal::Invalid)), "{name:?}");
        }
        // None of that counts against the code.
        assert_eq!(desk.status(now).attempts_left, MAX_ATTEMPTS);
    }

    #[test]
    fn many_unfinished_exchanges_push_out_the_oldest_only() {
        let (mut registry, dir) = registry();
        let mut desk = Desk::default();
        let now = Instant::now();
        let code = desk.open(DEFAULT_PAIRING_TTL, now);
        let begin = |desk: &mut Desk| {
            let device = Initiator::new(&code, "tablet").unwrap();
            let (session, hub_hello) = desk.start("tablet", device.hello(), &fingerprint(), now).unwrap();
            let (device, confirmation) = device.finish(&hub_hello, &fingerprint()).unwrap();
            (session, confirmation, device)
        };
        let first = begin(&mut desk);
        let others: Vec<_> = (0..MAX_SESSIONS).map(|_| begin(&mut desk)).collect();
        // The first one was pushed out; the newest is still good.
        assert!(matches!(desk.finish(&first.0, &first.1, &mut registry, now), Err(Refusal::Failed)));
        let (session, confirmation, device) = others.into_iter().next_back().unwrap();
        let (hub_confirmation, grant) = desk.finish(&session, &confirmation, &mut registry, now).unwrap();
        assert!(device.accept(&hub_confirmation, &grant).is_ok());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_full_list_refuses_the_device_but_keeps_the_code() {
        let (mut registry, dir) = registry();
        for index in 0..gc_pair::MAX_DEVICES {
            registry.add(&format!("device {index}")).unwrap();
        }
        let mut desk = Desk::default();
        let now = Instant::now();
        let code = desk.open(DEFAULT_PAIRING_TTL, now);
        assert!(matches!(pair(&mut desk, &mut registry, &code, now), Err(Refusal::NotSaved(RegistryError::Full))));
        let status = desk.status(now);
        assert!(status.open, "the person can remove a device and try again with the same code");
        assert_eq!(status.attempts_left, MAX_ATTEMPTS);
        let oldest = registry.list()[0].id.clone();
        registry.revoke(&oldest).unwrap();
        assert!(pair(&mut desk, &mut registry, &code, now).is_ok());
        std::fs::remove_dir_all(dir).ok();
    }
}
