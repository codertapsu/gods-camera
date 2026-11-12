//! Small secrets the app keeps for its user: the key that encrypts saved
//! faces, and the scanner's token and certificate.
//!
//! The app proper hands them to the system's own store, apart from its files
//! (Credential Manager on Windows). A try-out on a scratch data folder keeps
//! them in that folder instead: it must not read, replace or delete what the
//! user's own app has stored.

use std::path::PathBuf;

/// A place for secrets, each under a short name of letters and digits.
pub trait Secrets: Send + Sync {
    /// The secret, or `None` if there is none under this name.
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String>;
    /// Keep `secret` in place of whatever was there. `note` says what it is,
    /// for a user who looks into the system's store.
    fn write(&self, name: &str, secret: &[u8], note: &str) -> Result<(), String>;
    /// Remove it. Nothing to remove is as good as removed.
    fn delete(&self, name: &str);
}

/// Secrets as files in a folder. Only as safe as the folder: for try-outs and tests.
pub struct FolderSecrets {
    folder: PathBuf,
}

impl FolderSecrets {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self { folder: folder.into() }
    }

    fn file(&self, name: &str) -> Result<PathBuf, String> {
        match !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()) {
            true => Ok(self.folder.join(format!("{name}.secret"))),
            false => Err(format!("{name} is not a name for a secret")),
        }
    }
}

impl Secrets for FolderSecrets {
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(self.file(name)?) {
            Ok(secret) => Ok(Some(secret)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn write(&self, name: &str, secret: &[u8], _note: &str) -> Result<(), String> {
        let file = self.file(name)?;
        std::fs::create_dir_all(&self.folder).and_then(|()| std::fs::write(file, secret)).map_err(|error| error.to_string())
    }

    fn delete(&self, name: &str) {
        if let Ok(file) = self.file(name) {
            let _ = std::fs::remove_file(file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_keeps_reads_replaces_and_forgets() {
        let folder = std::env::temp_dir().join(format!("gc-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let secrets = FolderSecrets::new(&folder);
        assert_eq!(secrets.read("HubToken"), Ok(None));
        secrets.write("HubToken", b"first", "a note").unwrap();
        secrets.write("HubToken", b"second", "a note").unwrap();
        assert_eq!(secrets.read("HubToken"), Ok(Some(b"second".to_vec())));
        assert_eq!(secrets.read("FaceStore"), Ok(None));
        secrets.delete("HubToken");
        secrets.delete("HubToken");
        assert_eq!(secrets.read("HubToken"), Ok(None));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_name_cannot_lead_out_of_the_folder() {
        let secrets = FolderSecrets::new(std::env::temp_dir().join("gc-secrets-names"));
        for name in ["", "..", "../other", "a/b", "a\\b", "token.txt"] {
            assert!(secrets.read(name).is_err(), "{name}");
            assert!(secrets.write(name, b"x", "").is_err(), "{name}");
        }
    }
}
