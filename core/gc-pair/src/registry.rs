//! The hub's list of paired devices.
//!
//! The list lives in `devices.json` in the hub's data folder. It holds a
//! digest of each device's token, never the token: someone who reads the file
//! cannot pose as a device.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;

use crate::{random_bytes, Grant};

/// More devices than one person pairs. The limit keeps the file, and the work
/// done for every request, small.
pub const MAX_DEVICES: usize = 32;
/// "Last seen" is written to disk at most this often, in seconds, so that a
/// busy upload does not rewrite the file with every request.
const SEEN_SAVE_INTERVAL: u64 = 60;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("the list of paired devices could not be saved: {0}")]
    Io(#[from] std::io::Error),
    #[error("{MAX_DEVICES} devices are paired already; remove one first")]
    Full,
    #[error("a device name is 1 to 64 characters, without control characters")]
    InvalidName,
}

/// One paired device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    /// SHA-256 of the device's token, in hexadecimal.
    pub token_sha256: String,
    /// When the device was paired, in seconds since 1970.
    pub paired_at: u64,
    /// When the device last made a request, in seconds since 1970.
    pub last_seen: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    devices: Vec<DeviceRecord>,
}

pub struct DeviceRegistry {
    path: PathBuf,
    devices: Vec<DeviceRecord>,
    /// When the file was last written, in seconds since 1970.
    saved_at: u64,
    damaged: bool,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

fn token_digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// A random identifier in the usual 8-4-4-4-12 form.
fn new_id() -> String {
    let mut bytes = [0u8; 16];
    random_bytes(&mut bytes);
    // Marked as a random (version 4) identifier.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Write a file that only its owner can read.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    // A leftover from an interrupted save may have other permissions.
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

impl DeviceRegistry {
    /// Open the list kept in `data_dir`. A folder without a list is an empty list.
    ///
    /// A list that cannot be read is set aside as `devices.damaged.json` and
    /// an empty one takes its place: every device has to pair again, and
    /// nothing that was not on the list gets in. [`DeviceRegistry::was_damaged`]
    /// then says so.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("devices.json");
        let mut registry = Self { path, devices: Vec::new(), saved_at: now(), damaged: false };
        match std::fs::read(&registry.path) {
            Ok(bytes) => match serde_json::from_slice::<Stored>(&bytes) {
                Ok(stored) => registry.devices = stored.devices,
                Err(_) => registry.set_aside(),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => registry.set_aside(),
        }
        registry.devices.truncate(MAX_DEVICES);
        registry
    }

    fn set_aside(&mut self) {
        self.damaged = true;
        let _ = std::fs::rename(&self.path, self.path.with_file_name("devices.damaged.json"));
    }

    /// True when the list on disk could not be read and was replaced by an empty one.
    pub fn was_damaged(&self) -> bool {
        self.damaged
    }

    /// The paired devices, oldest first.
    pub fn list(&self) -> &[DeviceRecord] {
        &self.devices
    }

    /// Pair a device: give it an id and a token, and remember the token's
    /// digest. The grant is the only place the token ever exists on the hub.
    pub fn add(&mut self, name: &str) -> Result<Grant, RegistryError> {
        if crate::clean_device_name(name).as_deref() != Some(name) {
            return Err(RegistryError::InvalidName);
        }
        if self.devices.len() >= MAX_DEVICES {
            return Err(RegistryError::Full);
        }
        let mut secret = [0u8; 32];
        random_bytes(&mut secret);
        let token = hex::encode(secret);
        let at = now();
        let record = DeviceRecord {
            id: new_id(),
            name: name.to_string(),
            token_sha256: token_digest(&token),
            paired_at: at,
            last_seen: at,
        };
        let grant = Grant { device_id: record.id.clone(), token };
        self.devices.push(record);
        if let Err(error) = self.save() {
            // A device that is not on the saved list must not hold a token.
            self.devices.pop();
            return Err(error);
        }
        Ok(grant)
    }

    /// Remove a device. Its token stops working at once, even if the list
    /// cannot be saved; the error then says that the removal is not on disk.
    /// Returns false when there is no such device.
    pub fn revoke(&mut self, id: &str) -> Result<bool, RegistryError> {
        let before = self.devices.len();
        self.devices.retain(|device| device.id != id);
        if self.devices.len() == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// The device a token belongs to. Every record is compared, each in
    /// constant time, so the time taken says nothing about the tokens.
    pub fn authenticate(&self, token: &str) -> Option<&DeviceRecord> {
        if token.is_empty() {
            return None;
        }
        let presented = token_digest(token);
        let mut found = None;
        for device in &self.devices {
            if bool::from(presented.as_bytes().ct_eq(device.token_sha256.as_bytes())) {
                found = Some(device);
            }
        }
        found
    }

    /// Note that a device made a request just now.
    pub fn touch(&mut self, id: &str) {
        self.touch_at(id, now());
    }

    fn touch_at(&mut self, id: &str, at: u64) {
        let Some(device) = self.devices.iter_mut().find(|device| device.id == id) else { return };
        device.last_seen = device.last_seen.max(at);
        if at.saturating_sub(self.saved_at) >= SEEN_SAVE_INTERVAL {
            // "Last seen" is for show; losing a minute of it does no harm.
            let _ = self.save_at(at);
        }
    }

    fn save(&mut self) -> Result<(), RegistryError> {
        self.save_at(now())
    }

    fn save_at(&mut self, at: u64) -> Result<(), RegistryError> {
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let text = serde_json::to_vec_pretty(&Stored { devices: self.devices.clone() })
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        // Written beside the list and moved over it, so that an interrupted
        // save leaves the old list whole.
        let fresh = self.path.with_file_name("devices.json.new");
        write_private(&fresh, &text)?;
        std::fs::rename(&fresh, &self.path)?;
        self.saved_at = at;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        let mut random = [0u8; 8];
        random_bytes(&mut random);
        let dir = std::env::temp_dir().join(format!("gc-pair-registry-{}", hex::encode(random)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_device_is_added_recognised_and_removed() {
        let dir = folder();
        let mut registry = DeviceRegistry::open(&dir);
        assert!(registry.list().is_empty());
        assert!(!registry.was_damaged());

        let tablet = registry.add("Galaxy Tab S9+").unwrap();
        let phone = registry.add("Galaxy S22+").unwrap();
        assert_ne!(tablet.token, phone.token);
        assert_ne!(tablet.device_id, phone.device_id);
        assert_eq!(tablet.token.len(), 64);
        assert_eq!(tablet.device_id.len(), 36);
        assert_eq!(registry.list().len(), 2);

        assert_eq!(registry.authenticate(&tablet.token).unwrap().name, "Galaxy Tab S9+");
        assert_eq!(registry.authenticate(&phone.token).unwrap().id, phone.device_id);
        for wrong in ["", "nope", &tablet.token[1..], &tablet.token.to_uppercase(), &tablet.device_id] {
            assert!(registry.authenticate(wrong).is_none(), "{wrong:?}");
        }
        // The digest on the list is not a token.
        let digest = registry.list()[0].token_sha256.clone();
        assert!(registry.authenticate(&digest).is_none());

        assert!(registry.revoke(&tablet.device_id).unwrap());
        assert!(registry.authenticate(&tablet.token).is_none());
        assert!(registry.authenticate(&phone.token).is_some());
        assert!(!registry.revoke(&tablet.device_id).unwrap(), "already gone");
        assert!(!registry.revoke("no-such-device").unwrap());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_list_is_kept_on_disk_without_tokens() {
        let dir = folder();
        let granted = {
            let mut registry = DeviceRegistry::open(&dir);
            registry.add("Máy tính bảng").unwrap()
        };
        let text = std::fs::read_to_string(dir.join("devices.json")).unwrap();
        assert!(!text.contains(&granted.token), "the token must not be on disk");
        assert!(text.contains(&granted.device_id));
        assert!(text.contains("token_sha256"));
        assert!(!dir.join("devices.json.new").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("devices.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "only the owner may read the list");
        }

        // A hub started later still knows the device, and a removal is kept too.
        let mut reopened = DeviceRegistry::open(&dir);
        let record = reopened.authenticate(&granted.token).unwrap().clone();
        assert_eq!(record.name, "Máy tính bảng");
        assert_eq!(record.paired_at, record.last_seen);
        assert!(record.paired_at > 1_700_000_000);
        assert!(reopened.revoke(&granted.device_id).unwrap());
        assert!(DeviceRegistry::open(&dir).list().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn last_seen_moves_forward_and_is_saved_about_once_a_minute() {
        let dir = folder();
        let mut registry = DeviceRegistry::open(&dir);
        let granted = registry.add("tablet").unwrap();
        let paired = registry.list()[0].paired_at;
        let on_disk = |dir: &Path| DeviceRegistry::open(dir).list()[0].last_seen;

        registry.touch_at(&granted.device_id, paired + 10);
        assert_eq!(registry.list()[0].last_seen, paired + 10);
        assert_eq!(on_disk(&dir), paired, "too soon to write again");

        registry.touch_at(&granted.device_id, paired + SEEN_SAVE_INTERVAL + 5);
        assert_eq!(on_disk(&dir), paired + SEEN_SAVE_INTERVAL + 5);

        // A clock that steps back does not move "last seen" back.
        registry.touch_at(&granted.device_id, paired);
        assert_eq!(registry.list()[0].last_seen, paired + SEEN_SAVE_INTERVAL + 5);
        registry.touch_at("no-such-device", paired + 1000);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_damaged_list_is_set_aside_and_nobody_gets_in() {
        let dir = folder();
        std::fs::write(dir.join("devices.json"), b"{ not json").unwrap();
        let mut registry = DeviceRegistry::open(&dir);
        assert!(registry.was_damaged());
        assert!(registry.list().is_empty());
        assert!(dir.join("devices.damaged.json").exists());
        // Pairing works again afterwards.
        let granted = registry.add("tablet").unwrap();
        assert!(DeviceRegistry::open(&dir).authenticate(&granted.token).is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn names_must_be_tidy_and_the_list_has_a_limit() {
        let dir = folder();
        let mut registry = DeviceRegistry::open(&dir);
        for bad in ["", " padded ", "two  spaces", "line\nbreak"] {
            assert!(matches!(registry.add(bad), Err(RegistryError::InvalidName)), "{bad:?}");
        }
        for index in 0..MAX_DEVICES {
            registry.add(&format!("device {index}")).unwrap();
        }
        assert!(matches!(registry.add("one too many"), Err(RegistryError::Full)));
        assert_eq!(registry.list().len(), MAX_DEVICES);
        std::fs::remove_dir_all(dir).ok();
    }
}
