//! The `gc` tool end to end: a hub in one process, a device in others.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};

const GC: &str = env!("CARGO_BIN_EXE_gc");

/// A hub process that is stopped when the test ends, however it ends.
struct HubProcess {
    child: Child,
    lines: std::io::Lines<BufReader<ChildStdout>>,
}

impl HubProcess {
    fn start(data_dir: &Path, extra: &[&str]) -> Self {
        let mut child = Command::new(GC)
            .args(["hub", "--addr", "127.0.0.1:0", "--data-dir"])
            .arg(data_dir)
            .args(extra)
            .env("GC_HUB_TOKEN", "test-token-0123456789")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self { child, lines }
    }

    /// Read what the hub prints until a line starts with `prefix`, and return the rest of that line.
    fn wait_for(&mut self, prefix: &str) -> String {
        for line in self.lines.by_ref() {
            let line = line.unwrap();
            if let Some(rest) = line.strip_prefix(prefix) {
                return rest.to_string();
            }
        }
        panic!("the hub stopped before it printed '{prefix}…'");
    }
}

impl Drop for HubProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn gc(args: &[&str]) -> (bool, String) {
    let output = Command::new(GC).args(args).env_remove("GC_HUB_TOKEN").env_remove("GC_PAIR_CODE").output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

#[test]
fn a_device_pairs_and_uploads_from_the_command_line() {
    let dir = std::env::temp_dir().join(format!("gc-cli-pairing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (data, photos) = (dir.join("hub"), dir.join("photos"));
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::write(photos.join("page-1.jpg"), vec![1u8; 4000]).unwrap();
    std::fs::write(photos.join("page-2.jpg"), vec![2u8; 6000]).unwrap();
    let pairing = dir.join("pairing.json");
    let (pairing_arg, photos_arg) = (pairing.to_str().unwrap(), photos.to_str().unwrap());

    let mut hub = HubProcess::start(&data, &["--lan", "--lan-addr", "127.0.0.1", "--lan-port", "0", "--pair", "--name", "Máy thử"]);
    let listening = hub.wait_for("hub (paired devices) listening on https://");
    let port = listening.rsplit(':').next().unwrap().to_string();
    let fingerprint = hub.wait_for("certificate fingerprint ");
    assert_eq!(fingerprint.len(), 64);
    let code = hub.wait_for("pairing code ");
    let code: String = code.chars().take(7).collect();
    let address = format!("127.0.0.1:{port}");

    // A wrong code first: the hub refuses, and nothing is saved.
    let digits: String = code.chars().filter(char::is_ascii_digit).collect();
    let wrong = format!("{:06}", (digits.parse::<u32>().unwrap() + 1) % 1_000_000);
    let (ok, said) = gc(&["pair", "--address", &address, "--code", &wrong, "--name", "CLI test", "--out", pairing_arg]);
    assert!(!ok, "{said}");
    assert!(said.contains("403") && said.contains("pairing failed"), "{said}");
    assert!(!pairing.exists());

    // The code as the hub prints it, with its space.
    let (ok, said) = gc(&["pair", "--address", &address, "--code", &code, "--name", "  CLI   test ", "--out", pairing_arg]);
    assert!(ok, "{said}");
    assert!(said.contains("paired with Máy thử"), "{said}");
    assert_eq!(hub.wait_for("paired with "), "CLI test");

    // What the device keeps: the hub's fingerprint and its own token, for its owner's eyes only.
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&pairing).unwrap()).unwrap();
    assert_eq!(saved["fingerprint"], fingerprint.as_str());
    assert_eq!(saved["address"], address.as_str());
    assert_eq!(saved["token"].as_str().unwrap().len(), 64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&pairing).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(data.join("hub-identity.pem")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(data.join("devices.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(!said.contains(saved["token"].as_str().unwrap()), "the token is not printed");

    // As a paired device, without the hub's own token in the environment.
    let (ok, said) = gc(&["upload", photos_arg, "--paired", pairing_arg]);
    assert!(ok, "{said}");
    assert!(said.contains("2 files") && said.contains("done") && said.contains("\"bytes\":10000"), "{said}");

    // The code is spent: a second device is turned away.
    let other = dir.join("other.json");
    let (ok, said) = gc(&["pair", "--address", &address, "--code", &code, "--name", "second", "--out", other.to_str().unwrap()]);
    assert!(!ok && said.contains("is not showing a code"), "{said}");

    // A pairing that names another certificate does not reach the hub at all.
    let mut forged = saved.clone();
    forged["fingerprint"] = serde_json::Value::String("00".repeat(32));
    let forged_file = dir.join("forged.json");
    std::fs::write(&forged_file, forged.to_string()).unwrap();
    let (ok, said) = gc(&["upload", photos_arg, "--paired", forged_file.to_str().unwrap()]);
    assert!(!ok, "{said}");

    // After a restart the hub has the same certificate and still knows the device.
    drop(hub);
    let mut hub = HubProcess::start(&data, &["--lan", "--lan-addr", "127.0.0.1", "--lan-port", &port]);
    assert_eq!(hub.wait_for("certificate fingerprint "), fingerprint);
    let (ok, said) = gc(&["upload", photos_arg, "--paired", pairing_arg]);
    assert!(ok, "{said}");
    drop(hub);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_hub_without_the_network_option_is_as_before() {
    let dir = std::env::temp_dir().join(format!("gc-cli-local-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut hub = HubProcess::start(&dir, &[]);
    let address = hub.wait_for("hub (development mode) listening on ");
    assert!(address.starts_with("http://127.0.0.1:"), "{address}");
    assert!(!dir.join("hub-identity.pem").exists());
    // Pairing options make no sense without the network listener.
    let (ok, said) = gc(&["hub", "--pair"]);
    assert!(!ok && said.contains("--lan"), "{said}");
    drop(hub);
    std::fs::remove_dir_all(dir).ok();
}
