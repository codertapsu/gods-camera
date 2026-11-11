//! The hub as the Mac app drives it: started, paired with and managed through
//! the same interface UniFFI turns into Swift.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use gc_ffi::{generate_hub_identity, hub_identity_fingerprint, HubLan, HubServer, PairingOutcome};
use gc_link::gc_pair::{Grant, Initiator, PairingCode};
use gc_link::{PairFinishRequest, PairFinishResponse, PairStartRequest, PairStartResponse};

const TOKEN: &str = "test-token-0123456789";

fn folder(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("gc-ffi-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

/// What a device does to pair. Returns the grant and the fingerprint it saw,
/// or the status the hub refused it with.
async fn pair(port: u16, code: &str, name: &str) -> Result<(Grant, String), u16> {
    let (tls, seen) = gc_link::tls::pairing_client_config();
    let http = reqwest::Client::builder().tls_backend_preconfigured(tls).build().unwrap();
    let base = format!("https://127.0.0.1:{port}");
    let device = Initiator::new(&PairingCode::parse(code).unwrap(), name).unwrap();
    let begun = http
        .post(format!("{base}/v1/pair/start"))
        .json(&PairStartRequest { device_name: device.device_name().into(), message: BASE64.encode(device.hello()) })
        .send()
        .await
        .unwrap();
    if !begun.status().is_success() {
        return Err(begun.status().as_u16());
    }
    let begun: PairStartResponse = begun.json().await.unwrap();
    let fingerprint = seen.fingerprint().unwrap();
    let (device, confirmation) = device.finish(&BASE64.decode(&begun.message).unwrap(), &fingerprint).unwrap();
    let ended = http
        .post(format!("{base}/v1/pair/finish"))
        .json(&PairFinishRequest { session: begun.session, confirmation: BASE64.encode(confirmation) })
        .send()
        .await
        .unwrap();
    if !ended.status().is_success() {
        return Err(ended.status().as_u16());
    }
    let ended: PairFinishResponse = ended.json().await.unwrap();
    let grant = device.accept(&BASE64.decode(&ended.confirmation).unwrap(), &BASE64.decode(&ended.grant).unwrap()).unwrap();
    Ok((grant, fingerprint))
}

async fn health(port: u16, fingerprint: &str, token: &str) -> u16 {
    let tls = gc_link::tls::pinned_client_config(fingerprint).unwrap();
    let http = reqwest::Client::builder().tls_backend_preconfigured(tls).build().unwrap();
    http.get(format!("https://127.0.0.1:{port}/v1/health")).bearer_auth(token).send().await.unwrap().status().as_u16()
}

#[test]
fn the_app_pairs_lists_and_removes_a_device() {
    let identity = generate_hub_identity("Máy Mac".into()).unwrap();
    let fingerprint = hub_identity_fingerprint(identity.clone()).unwrap();
    assert_eq!(fingerprint.len(), 64);
    assert!(hub_identity_fingerprint("not an identity".into()).is_err());

    let dir = folder("lan");
    let lan = HubLan { port: 0, identity_pem: identity.clone(), name: "Máy Mac".into() };
    let server = HubServer::start_with_lan(0, TOKEN.into(), dir.clone(), vec![], lan).unwrap();
    assert!(server.address().starts_with("http://127.0.0.1:"));
    let port = server.lan_port().expect("the hub listens on the network");
    assert_eq!(server.identity_fingerprint(), Some(fingerprint.clone()));
    assert!(server.devices().is_empty());

    let before = server.pairing_status();
    assert!(!before.open && before.code.is_none() && before.ended.is_none() && before.paired.is_none());

    // The app shows a code; zero seconds means the usual five minutes.
    let code = server.open_pairing(0).unwrap();
    assert!(code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()));
    let showing = server.pairing_status();
    assert!(showing.open);
    assert_eq!(showing.code.as_deref(), Some(code.as_str()));
    assert!((295..=300).contains(&showing.seconds_left), "{}", showing.seconds_left);
    assert_eq!(showing.attempts_left, 5);

    // The device runs on its own runtime, as another process would.
    let device = tokio::runtime::Runtime::new().unwrap();
    let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
    assert_eq!(device.block_on(pair(port, &wrong, "Galaxy Tab S9+")).err(), Some(403));
    assert_eq!(server.pairing_status().attempts_left, 4);

    let (grant, seen) = device.block_on(pair(port, &code, "Galaxy Tab S9+")).unwrap();
    assert_eq!(seen, fingerprint);
    let after = server.pairing_status();
    assert!(!after.open && after.code.is_none());
    assert_eq!(after.ended, Some(PairingOutcome::Paired));
    assert_eq!(after.paired.as_ref().map(|device| device.name.as_str()), Some("Galaxy Tab S9+"));
    let devices = server.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, grant.device_id);
    assert!(devices[0].paired_at > 1_700_000_000 && devices[0].last_seen >= devices[0].paired_at);
    assert_eq!(device.block_on(health(port, &fingerprint, &grant.token)), 200);

    // A short code that nobody uses runs out.
    server.open_pairing(1).unwrap();
    assert!(server.pairing_status().open);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(server.pairing_status().ended, Some(PairingOutcome::Expired));
    server.open_pairing(60).unwrap();
    server.close_pairing();
    assert_eq!(server.pairing_status().ended, Some(PairingOutcome::Closed));
    assert_eq!(device.block_on(pair(port, &code, "phone")).err(), Some(403));

    // Removing the device cuts it off at once.
    assert!(server.revoke_device(grant.device_id.clone()).unwrap());
    assert!(!server.revoke_device(grant.device_id.clone()).unwrap());
    assert!(server.devices().is_empty());
    assert_eq!(device.block_on(health(port, &fingerprint, &grant.token)), 401);

    // Stopped and started again on the same folder with the same identity:
    // the same certificate, and devices paired earlier are still listed.
    let (again, _) = {
        let code = server.open_pairing(0).unwrap();
        device.block_on(pair(port, &code, "phone")).unwrap()
    };
    server.stop();
    drop(server);
    let lan = HubLan { port: 0, identity_pem: identity, name: "Máy Mac".into() };
    let server = HubServer::start_with_lan(0, TOKEN.into(), dir.clone(), vec![], lan).unwrap();
    assert_eq!(server.identity_fingerprint(), Some(fingerprint.clone()));
    assert_eq!(server.devices().iter().map(|device| device.name.as_str()).collect::<Vec<_>>(), ["phone"]);
    assert_eq!(device.block_on(health(server.lan_port().unwrap(), &fingerprint, &again.token)), 200);
    server.stop();
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_hub_started_for_this_computer_only_cannot_pair() {
    let dir = folder("local");
    let server = HubServer::start(0, TOKEN.into(), dir.clone(), vec![]).unwrap();
    assert_eq!(server.lan_port(), None);
    assert_eq!(server.identity_fingerprint(), None);
    assert!(server.open_pairing(0).is_err());
    assert!(!server.pairing_status().open);
    assert!(server.devices().is_empty());
    server.close_pairing();
    server.stop();

    // A broken identity is reported, not ignored.
    let lan = HubLan { port: 0, identity_pem: "nonsense".into(), name: "Mac".into() };
    assert!(HubServer::start_with_lan(0, TOKEN.into(), dir.clone(), vec![], lan).is_err());
    std::fs::remove_dir_all(dir).ok();
}
