//! Pairing and paired use of the hub, over real TLS sockets on this computer.

mod common;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use gc_link::gc_pair::{Grant, HubIdentity, Initiator, PairingCode};
use gc_link::{
    ErrorBody, FileInfo, Health, Hello, Hub, HubConfig, HubError, Job, LanConfig, PairFinishRequest,
    PairFinishResponse, PairStartRequest, PairStartResponse, PairingEnd, UploadInfo, DEFAULT_PAIRING_TTL,
    MAX_ATTEMPTS,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TOKEN: &str = "test-token-0123456789";

/// A hub with both listeners, on ports the system picks.
struct Running {
    hub: Hub,
    /// The network listener, over TLS.
    lan: String,
    lan_addr: std::net::SocketAddr,
    /// The loopback listener, plain.
    local: String,
    dir: PathBuf,
    identity: HubIdentity,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    served: tokio::task::JoinHandle<Result<(), HubError>>,
}

impl Running {
    /// Stop the hub and wait until it has let go of its ports.
    async fn stop(mut self) -> (PathBuf, HubIdentity) {
        self.stop.take().unwrap().send(()).unwrap();
        self.served.await.unwrap().unwrap();
        (self.dir, self.identity)
    }
}

fn config(dir: &Path, identity: Option<&HubIdentity>, ocr_command: Option<Vec<String>>) -> HubConfig {
    HubConfig {
        token: TOKEN.into(),
        data_dir: dir.to_path_buf(),
        max_file_bytes: 1 << 20,
        allowed_origins: vec![],
        ocr_command,
        lan: identity.map(|identity| LanConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            identity_pem: identity.to_pem().to_string(),
            name: "Máy Mac thử nghiệm".into(),
        }),
    }
}

async fn start_in(dir: PathBuf, identity: HubIdentity, ocr_command: Option<Vec<String>>) -> Running {
    gc_link::tls::install_crypto_provider();
    let bound = Hub::bind("127.0.0.1:0".parse().unwrap(), config(&dir, Some(&identity), ocr_command)).await.unwrap();
    let hub = bound.hub();
    let lan_addr = bound.lan_addr().unwrap();
    let lan = format!("https://{lan_addr}");
    let local = format!("http://{}", bound.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(bound.serve_until(async {
        let _ = stopped.await;
    }));
    Running { hub, lan, lan_addr, local, dir, identity, stop: Some(stop), served }
}

async fn start() -> Running {
    let dir = std::env::temp_dir().join(format!("gc-pairing-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    start_in(dir, HubIdentity::generate("test hub").unwrap(), None).await
}

/// A client that knows the hub by its certificate, as a paired device does.
fn pinned(fingerprint: &str) -> reqwest::Client {
    let tls = gc_link::tls::pinned_client_config(fingerprint).unwrap();
    reqwest::Client::builder().tls_backend_preconfigured(tls).build().unwrap()
}

/// What went wrong for a device that tried to pair.
#[derive(Debug, PartialEq)]
enum PairFailure {
    /// The hub answered with this status and error text.
    Refused(u16, String),
    /// The hub's answer did not hold up: it does not know the code.
    HubNotTrusted,
}

async fn refusal(response: reqwest::Response) -> PairFailure {
    let status = response.status().as_u16();
    let body: ErrorBody = response.json().await.unwrap();
    assert!(!body.request_id.is_empty());
    PairFailure::Refused(status, body.error)
}

/// Pair as a device would: over TLS to a hub it does not know yet, taking the
/// certificate as it comes and letting the exchange decide whether to trust
/// it. Returns the grant and the fingerprint to pin.
async fn pair(lan: &str, code: &PairingCode, name: &str) -> Result<(Grant, String), PairFailure> {
    let (tls, seen) = gc_link::tls::pairing_client_config();
    let http = reqwest::Client::builder().tls_backend_preconfigured(tls).build().unwrap();
    let device = Initiator::new(code, name).unwrap();
    let begun = http
        .post(format!("{lan}/v1/pair/start"))
        .json(&PairStartRequest { device_name: device.device_name().into(), message: BASE64.encode(device.hello()) })
        .send()
        .await
        .unwrap();
    if !begun.status().is_success() {
        return Err(refusal(begun).await);
    }
    let begun: PairStartResponse = begun.json().await.unwrap();
    let fingerprint = seen.fingerprint().expect("a connection was made");
    let (device, confirmation) = device.finish(&BASE64.decode(&begun.message).unwrap(), &fingerprint).unwrap();
    let ended = http
        .post(format!("{lan}/v1/pair/finish"))
        .json(&PairFinishRequest { session: begun.session, confirmation: BASE64.encode(confirmation) })
        .send()
        .await
        .unwrap();
    if !ended.status().is_success() {
        return Err(refusal(ended).await);
    }
    let ended: PairFinishResponse = ended.json().await.unwrap();
    let grant = device
        .accept(&BASE64.decode(&ended.confirmation).unwrap(), &BASE64.decode(&ended.grant).unwrap())
        .map_err(|_| PairFailure::HubNotTrusted)?;
    Ok((grant, fingerprint))
}

fn other_than(code: &PairingCode) -> PairingCode {
    let other = (code.digits().parse::<u32>().unwrap() + 1) % 1_000_000;
    PairingCode::parse(&format!("{other:06}")).unwrap()
}

#[tokio::test]
async fn a_device_pairs_and_then_sends_a_document_over_tls() {
    let dir = std::env::temp_dir().join(format!("gc-pairing-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let reader = common::fake_reader(&dir, false);
    let running = start_in(dir, HubIdentity::generate("test hub").unwrap(), Some(reader)).await;
    let (hub, lan) = (running.hub.clone(), running.lan.clone());
    assert_eq!(hub.fingerprint(), Some(running.identity.fingerprint()));

    // Before pairing, a device can see the hub and nothing more.
    let (tls, seen) = gc_link::tls::pairing_client_config();
    let stranger = reqwest::Client::builder().tls_backend_preconfigured(tls).build().unwrap();
    let hello: Hello = stranger.get(format!("{lan}/v1/hello")).send().await.unwrap().json().await.unwrap();
    assert_eq!(hello, Hello { name: "Máy Mac thử nghiệm".into(), version: env!("CARGO_PKG_VERSION").into(), api: "v1".into(), pairing_open: false });
    assert_eq!(seen.fingerprint(), Some(running.identity.fingerprint()));
    for path in ["/v1/health", "/v1/uploads/00000000-0000-0000-0000-000000000000", "/v1/jobs/x"] {
        assert_eq!(stranger.get(format!("{lan}{path}")).send().await.unwrap().status(), 401, "{path}");
    }
    assert_eq!(stranger.post(format!("{lan}/v1/uploads")).send().await.unwrap().status(), 401);

    let code = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    let status = hub.pairing_status();
    assert!(status.open);
    assert_eq!(status.code.as_ref(), Some(&code));
    assert_eq!(status.attempts_left, MAX_ATTEMPTS);
    assert!((295..=300).contains(&status.seconds_left), "{}", status.seconds_left);
    let hello: Hello = stranger.get(format!("{lan}/v1/hello")).send().await.unwrap().json().await.unwrap();
    assert!(hello.pairing_open);

    let (grant, fingerprint) = pair(&lan, &code, "Galaxy Tab S9+").await.unwrap();
    assert_eq!(fingerprint, running.identity.fingerprint());
    let status = hub.pairing_status();
    assert!(!status.open);
    assert_eq!(status.ended, Some(PairingEnd::Paired));
    assert_eq!(status.paired.as_ref().map(|device| device.id.as_str()), Some(grant.device_id.as_str()));
    let devices = hub.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Galaxy Tab S9+");
    assert_eq!(devices[0].id, grant.device_id);
    // The token is the device's alone: the hub keeps a digest of it.
    let saved = std::fs::read_to_string(running.dir.join("devices.json")).unwrap();
    assert!(!saved.contains(&grant.token));

    // Paired: the device knows the hub by its certificate, and the hub knows
    // the device by its token.
    let http = pinned(&fingerprint);
    let health: Health =
        http.get(format!("{lan}/v1/health")).bearer_auth(&grant.token).send().await.unwrap().json().await.unwrap();
    assert_eq!(health, Health { status: "ok".into(), api: "v1".into(), mode: "paired".into() });

    let upload: UploadInfo =
        http.post(format!("{lan}/v1/uploads")).bearer_auth(&grant.token).send().await.unwrap().json().await.unwrap();
    for name in ["page-1.jpg", "page-2.jpg"] {
        let url = format!("{lan}/v1/uploads/{}/files/{name}", upload.id);
        let first: FileInfo =
            http.put(&url).bearer_auth(&grant.token).body(vec![7u8; 5000]).send().await.unwrap().json().await.unwrap();
        assert_eq!(first.bytes, 5000);
        // Resuming works over the network as it does over the cable.
        let rest: FileInfo = http
            .put(format!("{url}?offset=5000"))
            .bearer_auth(&grant.token)
            .body(vec![8u8; 3000])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(rest.bytes, 8000);
    }
    let mut job: Job = http
        .post(format!("{lan}/v1/jobs"))
        .bearer_auth(&grant.token)
        .json(&serde_json::json!({ "kind": "document", "upload_id": upload.id }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for _ in 0..200 {
        if job.status == "done" || job.status == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        job = http.get(format!("{lan}/v1/jobs/{}", job.id)).bearer_auth(&grant.token).send().await.unwrap().json().await.unwrap();
    }
    assert_eq!(job.status, "done", "{:?}", job.error);
    assert_eq!(job.result.as_ref().unwrap()["pictures"], 2);
    let text = http
        .get(format!("{lan}/v1/jobs/{}/files/document.txt", job.id))
        .bearer_auth(&grant.token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(text, "Dòng chữ thứ nhất của trang dòng chữ thứ hai của trang.\n");
    assert!(hub.devices()[0].last_seen >= hub.devices()[0].paired_at);

    // A client that expects another certificate does not even connect, and
    // neither does one that speaks without TLS.
    let elsewhere = pinned(&HubIdentity::generate("another hub").unwrap().fingerprint());
    let refused = elsewhere.get(format!("{lan}/v1/hello")).send().await.unwrap_err();
    assert!(format!("{refused:?}").contains("ApplicationVerificationFailure"), "{refused:?}");
    let plain = reqwest::Client::new();
    let plain_url = lan.replace("https://", "http://");
    assert!(plain.get(format!("{plain_url}/v1/hello")).send().await.is_err());
    // And the hub still answers afterwards.
    assert!(http.get(format!("{lan}/v1/hello")).send().await.unwrap().status().is_success());

    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn five_wrong_codes_close_pairing() {
    let running = start().await;
    let (hub, lan) = (running.hub.clone(), running.lan.clone());
    let code = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    let guess = other_than(&code);
    for left in (0..MAX_ATTEMPTS).rev() {
        assert_eq!(pair(&lan, &guess, "guesser").await, Err(PairFailure::Refused(403, "pairing failed".into())));
        assert_eq!(hub.pairing_status().attempts_left, left);
    }
    let status = hub.pairing_status();
    assert!(!status.open);
    assert_eq!(status.ended, Some(PairingEnd::TooManyAttempts));
    assert_eq!(status.code, None);
    // The right code comes too late, and nobody was let in.
    assert_eq!(pair(&lan, &code, "tablet").await, Err(PairFailure::Refused(403, "pairing is closed".into())));
    assert!(hub.devices().is_empty());

    // A new code works, and the old one does not.
    let fresh = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    assert_eq!(hub.pairing_status().attempts_left, MAX_ATTEMPTS);
    assert!(pair(&lan, &fresh, "tablet").await.is_ok());
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn a_revoked_device_is_refused_at_once() {
    let running = start().await;
    let (hub, lan) = (running.hub.clone(), running.lan.clone());
    let (tablet, fingerprint) = pair(&lan, &hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap(), "tablet").await.unwrap();
    let (phone, _) = pair(&lan, &hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap(), "phone").await.unwrap();
    assert_ne!(tablet.token, phone.token);
    let http = pinned(&fingerprint);
    let health = |token: String| {
        let (http, lan) = (http.clone(), lan.clone());
        async move { http.get(format!("{lan}/v1/health")).bearer_auth(token).send().await.unwrap().status().as_u16() }
    };
    assert_eq!(health(tablet.token.clone()).await, 200);
    assert_eq!(health(phone.token.clone()).await, 200);

    assert!(hub.revoke_device(&tablet.device_id).unwrap());
    // On the connection that was already open, too.
    assert_eq!(health(tablet.token.clone()).await, 401);
    assert_eq!(health(phone.token.clone()).await, 200);
    assert!(!hub.revoke_device(&tablet.device_id).unwrap());
    assert_eq!(hub.devices().iter().map(|device| device.name.as_str()).collect::<Vec<_>>(), ["phone"]);

    // The device can pair again, and gets a new token; the old one stays dead.
    let (again, _) = pair(&lan, &hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap(), "tablet").await.unwrap();
    assert_ne!(again.token, tablet.token);
    assert_eq!(health(again.token).await, 200);
    assert_eq!(health(tablet.token).await, 401);
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn each_listener_takes_only_its_own_kind_of_token() {
    let running = start().await;
    let (hub, lan, local) = (running.hub.clone(), running.lan.clone(), running.local.clone());
    let (device, fingerprint) = pair(&lan, &hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap(), "tablet").await.unwrap();
    let http = pinned(&fingerprint);

    // The hub's own token is worth nothing on the network.
    for path in ["/v1/health", "/v1/uploads/00000000-0000-0000-0000-000000000000"] {
        let refused = http.get(format!("{lan}{path}")).bearer_auth(TOKEN).send().await.unwrap();
        assert_eq!(refused.status(), 401, "{path}");
        let body: ErrorBody = refused.json().await.unwrap();
        assert_eq!(body.error, "missing or invalid token");
    }
    assert_eq!(http.post(format!("{lan}/v1/uploads")).bearer_auth(TOKEN).send().await.unwrap().status(), 401);
    assert_eq!(http.post(format!("{lan}/v1/uploads")).bearer_auth(&device.token).send().await.unwrap().status(), 200);

    // A device's token is worth nothing on loopback, where the hub's own still works as before.
    let plain = reqwest::Client::new();
    assert_eq!(plain.post(format!("{local}/v1/uploads")).bearer_auth(&device.token).send().await.unwrap().status(), 401);
    assert_eq!(plain.post(format!("{local}/v1/uploads")).bearer_auth(TOKEN).send().await.unwrap().status(), 200);
    let health: Health = plain.get(format!("{local}/v1/health")).send().await.unwrap().json().await.unwrap();
    assert_eq!(health.mode, "development");

    // Pairing is a matter for the network listener only.
    let code = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    let begin = PairStartRequest {
        device_name: "tablet".into(),
        message: BASE64.encode(Initiator::new(&code, "tablet").unwrap().hello()),
    };
    assert_eq!(plain.post(format!("{local}/v1/pair/start")).json(&begin).send().await.unwrap().status(), 401);
    assert_eq!(plain.post(format!("{local}/v1/pair/start")).bearer_auth(TOKEN).json(&begin).send().await.unwrap().status(), 404);
    assert_eq!(plain.get(format!("{local}/v1/hello")).bearer_auth(TOKEN).send().await.unwrap().status(), 404);
    assert!(hub.pairing_status().open, "none of that touched the code");
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn pairing_is_refused_while_no_code_is_showing() {
    let running = start().await;
    let (hub, lan) = (running.hub.clone(), running.lan.clone());
    let closed = Err(PairFailure::Refused(403, "pairing is closed".into()));
    let any = PairingCode::parse("123456").unwrap();

    // Before any code was shown.
    assert_eq!(pair(&lan, &any, "tablet").await, closed);
    assert_eq!(hub.pairing_status().ended, None);

    // After the app closed the window.
    let code = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    hub.close_pairing();
    assert_eq!(pair(&lan, &code, "tablet").await, closed);
    assert_eq!(hub.pairing_status().ended, Some(PairingEnd::Closed));

    // After a device has paired: the code is spent.
    let code = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    assert!(pair(&lan, &code, "tablet").await.is_ok());
    assert_eq!(pair(&lan, &code, "phone").await, closed);
    assert_eq!(hub.devices().len(), 1);

    // An exchange that nobody began is refused as a failure, without a hint.
    hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    let (tls, _) = gc_link::tls::pairing_client_config();
    let http = reqwest::Client::builder().tls_backend_preconfigured(tls).build().unwrap();
    let unknown = http
        .post(format!("{lan}/v1/pair/finish"))
        .json(&PairFinishRequest { session: uuid::Uuid::new_v4().to_string(), confirmation: BASE64.encode([0u8; 32]) })
        .send()
        .await
        .unwrap();
    assert_eq!(refusal(unknown).await, PairFailure::Refused(403, "pairing failed".into()));
    assert_eq!(hub.pairing_status().attempts_left, MAX_ATTEMPTS);
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn an_expired_code_is_refused() {
    let running = start().await;
    let (hub, lan) = (running.hub.clone(), running.lan.clone());
    let code = hub.open_pairing(Duration::from_millis(60)).unwrap();
    assert!(hub.pairing_status().open);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(pair(&lan, &code, "tablet").await, Err(PairFailure::Refused(403, "pairing is closed".into())));
    let status = hub.pairing_status();
    assert_eq!((status.open, status.ended, status.seconds_left), (false, Some(PairingEnd::Expired), 0));
    assert!(hub.devices().is_empty());
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn requests_no_device_would_send_are_refused() {
    let running = start().await;
    let (hub, lan) = (running.hub.clone(), running.lan.clone());
    let code = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    let (tls, _) = gc_link::tls::pairing_client_config();
    let http = reqwest::Client::builder().tls_backend_preconfigured(tls).build().unwrap();
    let hello = BASE64.encode(Initiator::new(&code, "tablet").unwrap().hello());
    let invalid = PairFailure::Refused(400, "invalid pairing request".into());

    let start_url = format!("{lan}/v1/pair/start");
    let bodies = [
        serde_json::json!({ "device_name": "tablet" }),
        serde_json::json!({ "device_name": "tablet", "message": "not base64 !!" }),
        serde_json::json!({ "device_name": "tablet", "message": BASE64.encode(b"too short") }),
        serde_json::json!({ "device_name": " untidy ", "message": hello }),
        serde_json::json!({ "device_name": "", "message": hello }),
        serde_json::json!({ "device_name": "x".repeat(65), "message": hello }),
        serde_json::json!({ "device_name": "tablet", "message": "A".repeat(400) }),
        serde_json::json!(["not", "an", "object"]),
    ];
    for body in bodies {
        let response = http.post(&start_url).json(&body).send().await.unwrap();
        assert_eq!(refusal(response).await, invalid, "{body}");
    }
    let not_json = http.post(&start_url).header("content-type", "application/json").body("{").send().await.unwrap();
    assert_eq!(refusal(not_json).await, invalid);
    let no_type = http.post(&start_url).body(r#"{"device_name":"tablet"}"#).send().await.unwrap();
    assert_eq!(refusal(no_type).await, invalid);
    let finish = http
        .post(format!("{lan}/v1/pair/finish"))
        .json(&serde_json::json!({ "session": "s", "confirmation": "***" }))
        .send()
        .await
        .unwrap();
    assert_eq!(refusal(finish).await, invalid);

    // Too much to be a pairing request.
    let huge = serde_json::json!({ "device_name": "tablet", "message": hello, "padding": "x".repeat(8000) });
    let response = http.post(&start_url).json(&huge).send().await.unwrap();
    assert_eq!(response.status(), 413);

    // A web page cannot pair: browsers say where a request comes from.
    let from_a_page = http
        .post(&start_url)
        .header("Origin", "https://evil.example")
        .json(&serde_json::json!({ "device_name": "tablet", "message": hello }))
        .send()
        .await
        .unwrap();
    assert_eq!(refusal(from_a_page).await, PairFailure::Refused(403, "origin not allowed".into()));
    let hello_from_a_page = http.get(format!("{lan}/v1/hello")).header("Origin", "https://evil.example").send().await.unwrap();
    assert_eq!(hello_from_a_page.status(), 403);

    // None of this used up a try, and the code still works.
    assert_eq!(hub.pairing_status().attempts_left, MAX_ATTEMPTS);
    assert!(pair(&lan, &code, "tablet").await.is_ok());
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn a_hub_that_guesses_the_code_is_not_trusted_by_the_device() {
    // The device talks to a hub that shows another code than the person typed:
    // the hub refuses, and had it answered, the device would have refused.
    let running = start().await;
    let (hub, lan) = (running.hub.clone(), running.lan.clone());
    let shown = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    let typed = other_than(&shown);
    assert_eq!(pair(&lan, &typed, "tablet").await, Err(PairFailure::Refused(403, "pairing failed".into())));
    assert_eq!(hub.pairing_status().attempts_left, MAX_ATTEMPTS - 1);
    assert!(pair(&lan, &shown, "tablet").await.is_ok());
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

/// Someone between the device and the hub: takes the device's connection with
/// a certificate of their own, and passes every byte on to the real hub and
/// back. Returns where they listen and who they claim to be.
async fn in_the_middle(hub: std::net::SocketAddr) -> (String, HubIdentity) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
    let identity = HubIdentity::generate("somebody else").unwrap();
    let server = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(identity.certificate_der().to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.private_key_der().to_vec())),
        )
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(server));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("https://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut device) = acceptor.accept(stream).await else { return };
                // Towards the hub they are an ordinary stranger.
                let (client, _) = gc_link::tls::pairing_client_config();
                let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(client));
                let stream = tokio::net::TcpStream::connect(hub).await.unwrap();
                let mut hub = connector.connect(ServerName::try_from("hub.invalid").unwrap(), stream).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut device, &mut hub).await;
            });
        }
    });
    (address, identity)
}

#[tokio::test]
async fn someone_in_the_middle_cannot_pair_even_with_the_right_code() {
    let running = start().await;
    let hub = running.hub.clone();
    let (middle, theirs) = in_the_middle(running.lan_addr).await;
    assert_ne!(theirs.fingerprint(), running.identity.fingerprint());

    // The relay works: through it, a device sees the real hub's answers, but
    // the relay's certificate.
    let (tls, seen) = gc_link::tls::pairing_client_config();
    let http = reqwest::Client::builder().tls_backend_preconfigured(tls).build().unwrap();
    let hello: Hello = http.get(format!("{middle}/v1/hello")).send().await.unwrap().json().await.unwrap();
    assert_eq!(hello.name, "Máy Mac thử nghiệm");
    assert_eq!(seen.fingerprint(), Some(theirs.fingerprint()));

    // The person types the right code. The device and the hub still disagree
    // about the certificate, so the hub refuses and nobody is paired.
    let code = hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap();
    assert_eq!(pair(&middle, &code, "tablet").await, Err(PairFailure::Refused(403, "pairing failed".into())));
    assert!(hub.devices().is_empty());
    assert_eq!(hub.pairing_status().attempts_left, MAX_ATTEMPTS - 1);

    // Straight to the hub, the same code still pairs.
    let (grant, fingerprint) = pair(&running.lan, &code, "tablet").await.unwrap();
    assert_eq!(fingerprint, running.identity.fingerprint());

    // A paired device does not talk through the relay at all: the certificate is not the hub's.
    let paired = pinned(&fingerprint);
    assert!(paired.get(format!("{middle}/v1/health")).bearer_auth(&grant.token).send().await.is_err());
    assert_eq!(paired.get(format!("{}/v1/health", running.lan)).bearer_auth(&grant.token).send().await.unwrap().status(), 200);
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn devices_stay_paired_across_a_restart_with_the_same_identity() {
    let running = start().await;
    let (grant, fingerprint) =
        pair(&running.lan, &running.hub.open_pairing(DEFAULT_PAIRING_TTL).unwrap(), "tablet").await.unwrap();
    let (dir, identity) = running.stop().await;

    // The same folder and the same identity: the device is still known.
    let running = start_in(dir, identity, None).await;
    let http = pinned(&fingerprint);
    let again = http.get(format!("{}/v1/health", running.lan)).bearer_auth(&grant.token).send().await.unwrap();
    assert_eq!(again.status(), 200);
    assert_eq!(running.hub.devices().len(), 1);
    assert!(!running.hub.pairing_status().open, "a restart does not reopen pairing");
    let (dir, _) = running.stop().await;

    // A new identity: the device no longer recognises the hub, and says nothing to it.
    let running = start_in(dir, HubIdentity::generate("test hub").unwrap(), None).await;
    assert!(http.get(format!("{}/v1/health", running.lan)).bearer_auth(&grant.token).send().await.is_err());
    let (dir, _) = running.stop().await;
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn a_hub_without_a_network_listener_cannot_pair() {
    gc_link::tls::install_crypto_provider();
    let dir = std::env::temp_dir().join(format!("gc-pairing-test-{}", uuid::Uuid::new_v4()));
    let bound = Hub::bind("127.0.0.1:0".parse().unwrap(), config(&dir, None, None)).await.unwrap();
    assert_eq!(bound.lan_addr(), None);
    let hub = bound.hub();
    assert_eq!(hub.fingerprint(), None);
    assert!(matches!(hub.open_pairing(DEFAULT_PAIRING_TTL), Err(HubError::NoNetworkListener)));
    assert!(!hub.pairing_status().open);
    assert!(hub.devices().is_empty());
    hub.close_pairing();

    // The loopback listener serves as before.
    let local = format!("http://{}", bound.local_addr().unwrap());
    tokio::spawn(bound.serve());
    let health: Health = reqwest::Client::new().get(format!("{local}/v1/health")).send().await.unwrap().json().await.unwrap();
    assert_eq!(health.status, "ok");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn a_hub_refuses_an_identity_it_cannot_read_and_an_address_off_loopback() {
    let dir = std::env::temp_dir().join(format!("gc-pairing-test-{}", uuid::Uuid::new_v4()));
    let identity = HubIdentity::generate("test hub").unwrap();
    let mut broken = config(&dir, Some(&identity), None);
    broken.lan.as_mut().unwrap().identity_pem = "not an identity".into();
    assert!(matches!(Hub::bind("127.0.0.1:0".parse().unwrap(), broken).await, Err(HubError::Identity(_))));

    // The loopback listener stays on loopback, with or without the network one.
    let public = Hub::bind("0.0.0.0:0".parse().unwrap(), config(&dir, Some(&identity), None)).await;
    assert!(matches!(public, Err(HubError::NotLoopback(_))));
    let mut weak = config(&dir, Some(&identity), None);
    weak.token = "short".into();
    assert!(matches!(Hub::bind("127.0.0.1:0".parse().unwrap(), weak).await, Err(HubError::WeakToken)));

    // Neither the token nor the private key shows up when a configuration is printed.
    let printed = format!("{:?}", config(&dir, Some(&identity), None));
    assert!(!printed.contains(TOKEN));
    assert!(!printed.contains("PRIVATE KEY"));
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn the_any_address_takes_ipv4_and_ipv6() {
    gc_link::tls::install_crypto_provider();
    let dir = std::env::temp_dir().join(format!("gc-pairing-test-{}", uuid::Uuid::new_v4()));
    let identity = HubIdentity::generate("test hub").unwrap();
    let mut both = config(&dir, Some(&identity), None);
    both.lan.as_mut().unwrap().bind = "[::]:0".parse().unwrap();
    let bound = Hub::bind("127.0.0.1:0".parse().unwrap(), both).await.unwrap();
    let port = bound.lan_addr().unwrap().port();
    tokio::spawn(bound.serve());

    let http = pinned(&identity.fingerprint());
    let over_v4 = http.get(format!("https://127.0.0.1:{port}/v1/hello")).send().await.unwrap();
    assert_eq!(over_v4.status(), 200);
    let over_v6 = http.get(format!("https://[::1]:{port}/v1/hello")).send().await.unwrap();
    assert_eq!(over_v6.status(), 200);
    std::fs::remove_dir_all(dir).ok();
}
