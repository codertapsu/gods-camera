//! The scanner's model with a stand-in for the system: no Credential
//! Manager, no firewall, no announcement on the network. The hub is real, on
//! ports the system picks, on this computer only.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gc_app::scanner::{
    Announce, DocumentStatus, NetworkAccess, NetworkState, ScanPlatform, Scanner, ScannerConfig, ScannerProblem,
};
use gc_app::secrets::{FolderSecrets, Secrets};

struct StandIn {
    secrets: Arc<FolderSecrets>,
    access: Mutex<NetworkAccess>,
    announcements: Arc<AtomicUsize>,
}

struct Told;

impl Announce for Told {
    fn up(&self) -> Option<bool> {
        Some(true)
    }
}

impl ScanPlatform for StandIn {
    fn secrets(&self) -> Arc<dyn Secrets> {
        self.secrets.clone()
    }

    fn computer_name(&self) -> String {
        "Test PC".into()
    }

    fn network_access(&self, _program: &Path, _port: u16) -> NetworkAccess {
        *self.access.lock().unwrap()
    }

    fn announce(&self, name: &str, port: u16, fingerprint: &str) -> Option<Box<dyn Announce>> {
        assert_eq!(name, "Test PC");
        assert!(port != 0 && fingerprint.len() == 64);
        self.announcements.fetch_add(1, Ordering::SeqCst);
        Some(Box::new(Told))
    }

    fn listening(&self) -> (u16, SocketAddr) {
        // Free ports, and this computer only: a test must not listen on the network.
        (0, SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
    }
}

struct Fixture {
    folder: PathBuf,
    platform: Arc<StandIn>,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let folder = std::env::temp_dir().join(format!("gc-scanner-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let platform = Arc::new(StandIn {
            secrets: Arc::new(FolderSecrets::new(folder.join("stand-in-secrets"))),
            access: Mutex::new(NetworkAccess::Allowed),
            announcements: Arc::new(AtomicUsize::new(0)),
        });
        Self { folder, platform }
    }

    fn config(&self, try_out: bool) -> ScannerConfig {
        ScannerConfig {
            data_dir: self.folder.join("data").to_string_lossy().into_owned(),
            // No reader and no models: these tests are about the hub's switch, not about reading.
            reader: Vec::new(),
            pack_dirs: vec![self.folder.join("packs").to_string_lossy().into_owned()],
            program: self.folder.join("GodsCamera.exe").to_string_lossy().into_owned(),
            try_out,
        }
    }

    /// The scanner as the user's own app has it, on the stand-in system.
    fn scanner(&self) -> Arc<Scanner> {
        Scanner::with_platform(self.config(false), self.platform.clone())
    }

    fn hub(&self) -> PathBuf {
        self.folder.join("data").join("Hub")
    }

    fn secret(&self, name: &str) -> Option<Vec<u8>> {
        self.platform.secrets.read(name).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// Ask the hub how it is, over plain HTTP on this computer.
fn health(address: &str) -> String {
    let at = address.strip_prefix("http://").expect("a plain address on this computer");
    let mut stream = TcpStream::connect(at).expect("the hub answers");
    write!(stream, "GET /v1/health HTTP/1.1\r\nHost: {at}\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer
}

#[test]
fn the_scanner_stays_off_until_it_is_wanted_and_remembers_that() {
    let fixture = Fixture::new("wanted");
    let scanner = fixture.scanner();
    scanner.start();
    let state = scanner.state();
    assert!(!state.wanted && !state.running);
    assert_eq!((state.network, state.problem), (NetworkState::Off, None));
    assert_eq!(state.name, "Test PC");
    assert!(state.local_address.is_empty() && scanner.network_port().is_none());

    scanner.set_wanted(true);
    let state = scanner.state();
    assert!(state.wanted && state.running);
    assert_eq!((state.network, state.problem), (NetworkState::Ready, None));
    assert!(state.announced);
    assert_eq!(fixture.platform.announcements.load(Ordering::SeqCst), 1);
    assert!(scanner.network_port().is_some_and(|port| port != 0));
    // The hub is there and answers.
    assert!(state.local_address.starts_with("http://127.0.0.1:"), "{}", state.local_address);
    let answer = health(&state.local_address);
    assert!(answer.starts_with("HTTP/1.1 200") && answer.contains("\"status\":\"ok\""), "{answer}");
    drop(scanner);

    // The next run carries on where this one stopped.
    let scanner = fixture.scanner();
    assert!(scanner.state().wanted && !scanner.state().running);
    scanner.start();
    assert!(scanner.state().running);
    scanner.set_wanted(false);
    let state = scanner.state();
    assert!(!state.wanted && !state.running);
    assert_eq!(state.network, NetworkState::Off);
    drop(scanner);
    let scanner = fixture.scanner();
    scanner.start();
    assert!(!scanner.state().running);
}

#[test]
fn a_code_is_offered_and_withdrawn() {
    let fixture = Fixture::new("code");
    let scanner = fixture.scanner();
    assert!(!scanner.begin_pairing(), "no code while the scanner is off");
    scanner.set_wanted(true);
    assert!(scanner.begin_pairing());
    let state = scanner.state();
    let code = state.pairing_code.expect("a code is on offer");
    assert!(code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()), "{code}");
    assert!((290..=300).contains(&state.pairing_seconds_left), "{}", state.pairing_seconds_left);
    assert!(!state.pairing_ended && state.just_paired.is_none() && state.devices.is_empty());

    // A new code replaces the one on offer.
    assert!(scanner.begin_pairing());
    assert!(scanner.state().pairing_code.is_some_and(|next| next.len() == 6));

    scanner.end_pairing();
    let state = scanner.state();
    assert!(state.pairing_code.is_none() && !state.pairing_ended);

    // Turning the scanner off withdraws the code with it.
    assert!(scanner.begin_pairing());
    scanner.set_wanted(false);
    assert!(scanner.state().pairing_code.is_none());
}

#[test]
fn the_firewall_decides_whether_devices_on_the_network_can_come_in() {
    let fixture = Fixture::new("firewall");
    *fixture.platform.access.lock().unwrap() = NetworkAccess::RuleNeeded;
    let scanner = fixture.scanner();
    scanner.set_wanted(true);
    let state = scanner.state();
    // The hub runs for a device on a cable; the network side waits for the user.
    assert!(state.running);
    assert_eq!(state.network, NetworkState::FirewallNeeded);
    assert!(scanner.network_port().is_none() && !state.announced);
    assert!(!scanner.begin_pairing(), "no code without the network side");
    assert_eq!(fixture.platform.announcements.load(Ordering::SeqCst), 0);
    // No certificate is made before it is needed.
    assert!(fixture.secret("HubIdentity").is_none() && fixture.secret("HubToken").is_some());

    // Looking again changes nothing while the rule is missing.
    scanner.refresh_network();
    assert_eq!(scanner.state().network, NetworkState::FirewallNeeded);

    // The user allows it: the hub starts listening for devices.
    *fixture.platform.access.lock().unwrap() = NetworkAccess::Allowed;
    scanner.refresh_network();
    let state = scanner.state();
    assert_eq!(state.network, NetworkState::Ready);
    assert!(state.running && scanner.network_port().is_some() && scanner.begin_pairing());
    assert_eq!(fixture.platform.announcements.load(Ordering::SeqCst), 1);

    // Only public networks: said as it is.
    scanner.set_wanted(false);
    *fixture.platform.access.lock().unwrap() = NetworkAccess::PublicOnly;
    scanner.set_wanted(true);
    assert_eq!(scanner.state().network, NetworkState::PublicNetwork);
    assert!(scanner.state().running && scanner.network_port().is_none());
}

#[test]
fn the_token_and_the_certificate_are_made_once_and_kept() {
    let fixture = Fixture::new("secrets");
    let scanner = fixture.scanner();
    assert!(fixture.secret("HubToken").is_none(), "nothing is made before the scanner is turned on");
    scanner.set_wanted(true);
    let token = fixture.secret("HubToken").expect("a token");
    let identity = fixture.secret("HubIdentity").expect("a certificate");
    assert!(token.len() == 32 && token.iter().all(u8::is_ascii_hexdigit));
    let text = String::from_utf8(identity.clone()).unwrap();
    assert!(text.contains("BEGIN CERTIFICATE") && text.contains("PRIVATE KEY"));
    // Windows' Credential Manager takes no secret longer than this.
    assert!(identity.len() <= 2560, "{} bytes", identity.len());

    // Off and on again, and a new run: the same token and certificate, or
    // every connected device would be cut off.
    scanner.set_wanted(false);
    scanner.set_wanted(true);
    drop(scanner);
    let scanner = fixture.scanner();
    scanner.start();
    assert!(scanner.state().running);
    assert_eq!(fixture.secret("HubToken"), Some(token.clone()));
    assert_eq!(fixture.secret("HubIdentity"), Some(identity.clone()));

    // A token that is not one is replaced.
    scanner.set_wanted(false);
    fixture.platform.secrets.write("HubToken", b"short", "").unwrap();
    scanner.set_wanted(true);
    assert!(fixture.secret("HubToken").is_some_and(|new| new.len() == 32 && new != token));
}

/// A job as the hub leaves it on disk.
fn job(hub: &Path, id: &str, kind: &str, status: &str, progress: f32, text: Option<&str>) {
    let folder = hub.join("jobs").join(id);
    std::fs::create_dir_all(&folder).unwrap();
    let upload = id.replace('a', "b");
    std::fs::create_dir_all(hub.join("uploads").join(&upload)).unwrap();
    std::fs::write(hub.join("uploads").join(&upload).join("page-1.jpg"), b"a picture").unwrap();
    let job = serde_json::json!({
        "id": id, "kind": kind, "upload_id": upload, "status": status, "progress": progress,
        "result": text.map(|_| serde_json::json!({ "pictures": 3 })),
        "files": [], "error": if status == "failed" { Some("the text reader failed") } else { None },
    });
    std::fs::write(folder.join("job.json"), serde_json::to_vec(&job).unwrap()).unwrap();
    if let Some(text) = text {
        std::fs::write(folder.join("document.txt"), text).unwrap();
    }
}

#[test]
fn documents_are_listed_read_and_deleted_with_their_pictures() {
    let fixture = Fixture::new("documents");
    let hub = fixture.hub();
    let (done, reading, failed, other) = (
        "aaaaaaaa-0000-4000-8000-000000000001",
        "aaaaaaaa-0000-4000-8000-000000000002",
        "aaaaaaaa-0000-4000-8000-000000000003",
        "aaaaaaaa-0000-4000-8000-000000000004",
    );
    job(&hub, done, "document", "done", 1.0, Some("\n  Mùa nước nổi trở lại miền Tây\n\nSáng nay, mực nước trên sông Tiền đã lên.\n"));
    job(&hub, reading, "document", "running", 0.5, None);
    job(&hub, failed, "document", "failed", 0.2, None);
    // The hub does other work too; that is not a document.
    job(&hub, other, "inventory", "done", 1.0, None);

    // The list is there whether the scanner is on or off.
    let scanner = fixture.scanner();
    let state = scanner.state();
    assert_eq!(state.documents.len(), 3);
    let find = |id: &str| state.documents.iter().find(|document| document.id == id).expect("listed");
    assert_eq!(find(done).status, DocumentStatus::Done);
    assert_eq!(find(done).title, "Mùa nước nổi trở lại miền Tây");
    assert_eq!(find(done).pictures, 3);
    assert!(find(done).received_at > 1_700_000_000);
    assert!(Path::new(&find(done).folder).join("document.txt").is_file());
    assert_eq!((find(reading).status, find(reading).progress), (DocumentStatus::Reading, 0.5));
    assert!(find(reading).title.is_empty());
    assert_eq!(find(failed).status, DocumentStatus::Failed);
    assert_eq!(find(failed).error.as_deref(), Some("the text reader failed"));
    // Newest first.
    assert!(state.documents.windows(2).all(|pair| pair[0].received_at >= pair[1].received_at));

    assert!(scanner.document_text(done.into()).contains("Sáng nay, mực nước"));
    assert_eq!(scanner.document_text(reading.into()), "");
    // Only a document's own number names it: nothing else leads to a file.
    assert_eq!(scanner.document_text(format!("..\\jobs\\{done}")), "");
    assert_eq!(scanner.document_text("..".into()), "");

    scanner.delete_document(done.into());
    scanner.delete_document("..".into());
    let state = scanner.state();
    assert_eq!(state.documents.len(), 2);
    assert!(!hub.join("jobs").join(done).exists());
    assert!(!hub.join("uploads").join(done.replace('a', "b")).exists(), "the pictures go with the document");
    assert!(hub.join("uploads").join(reading.replace('a', "b")).exists(), "and only those");
    assert!(hub.join("jobs").join(other).exists());
}

#[test]
fn deleting_everything_leaves_nothing_a_device_could_use() {
    let fixture = Fixture::new("everything");
    let scanner = fixture.scanner();
    scanner.set_wanted(true);
    job(&fixture.hub(), "aaaaaaaa-0000-4000-8000-000000000009", "document", "done", 1.0, Some("A page"));
    let (token, identity) = (fixture.secret("HubToken").unwrap(), fixture.secret("HubIdentity").unwrap());

    scanner.delete_everything();
    let state = scanner.state();
    assert!(state.documents.is_empty() && state.devices.is_empty());
    // It was on, so it is on again: with a new token and a new certificate.
    assert!(state.wanted && state.running);
    assert_eq!(state.network, NetworkState::Ready);
    assert!(fixture.secret("HubToken").is_some_and(|new| new != token));
    assert!(fixture.secret("HubIdentity").is_some_and(|new| new != identity));
    assert!(!fixture.hub().join("jobs").join("aaaaaaaa-0000-4000-8000-000000000009").exists());

    // Off, it stays off, and nothing is made again.
    scanner.set_wanted(false);
    scanner.delete_everything();
    assert!(!scanner.state().running);
    assert!(fixture.secret("HubToken").is_none() && fixture.secret("HubIdentity").is_none());
}

#[test]
fn a_try_out_keeps_to_itself() {
    let fixture = Fixture::new("try-out");
    *fixture.platform.access.lock().unwrap() = NetworkAccess::RuleNeeded;
    let scanner = Scanner::with_platform(fixture.config(true), fixture.platform.clone());
    scanner.set_wanted(true);
    let state = scanner.state();
    // It does not ask the firewall and announces nothing: it never leaves this computer.
    assert_eq!((state.network, state.problem), (NetworkState::Ready, None::<ScannerProblem>));
    assert!(state.running && !state.announced);
    assert_eq!(fixture.platform.announcements.load(Ordering::SeqCst), 0);
    assert!(scanner.network_port().is_some_and(|port| port != 0 && port != 47801));
    assert!(scanner.begin_pairing());
    // Without a reader, documents cannot be read, and the state says so.
    assert_eq!(state.reader_missing, Some(String::new()));
}
