//! Pack manifests: where each model file came from, its hash and its licence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pack {
    pub name: String,
    /// Short licence statement, with the place it was read.
    pub licence: String,
    /// Whether the pack may ship in a build that is distributed to others.
    pub commercial_ok: bool,
    pub files: Vec<PackFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackFile {
    /// Path relative to the pack store.
    pub path: String,
    pub source_url: String,
    pub sha256: String,
    pub bytes: u64,
    pub role: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("cannot read {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("invalid manifest {path}: {source}")]
    Parse { path: PathBuf, source: serde_json::Error },
    #[error("{path}: expected {expected} bytes, found {found}")]
    Size { path: PathBuf, expected: u64, found: u64 },
    #[error("{path}: SHA-256 mismatch")]
    Hash { path: PathBuf },
    #[error("pack {0} is not cleared for distribution")]
    NotCommercial(String),
}

impl Pack {
    pub fn load(manifest: &Path) -> Result<Pack, PackError> {
        let text = std::fs::read_to_string(manifest)
            .map_err(|source| PackError::Io { path: manifest.to_path_buf(), source })?;
        serde_json::from_str(&text)
            .map_err(|source| PackError::Parse { path: manifest.to_path_buf(), source })
    }

    /// Check every file in the pack against its recorded size and hash.
    pub fn verify(&self, store: &Path) -> Result<(), PackError> {
        self.files.iter().try_for_each(|f| f.verify(store))
    }

    /// Release builds call this and refuse packs that are not cleared.
    pub fn require_commercial(&self) -> Result<(), PackError> {
        if self.commercial_ok {
            Ok(())
        } else {
            Err(PackError::NotCommercial(self.name.clone()))
        }
    }
}

impl PackFile {
    pub fn verify(&self, store: &Path) -> Result<(), PackError> {
        let path = store.join(&self.path);
        let io = |source| PackError::Io { path: path.clone(), source };
        let found = std::fs::metadata(&path).map_err(io)?.len();
        if found != self.bytes {
            return Err(PackError::Size { path, expected: self.bytes, found });
        }
        if sha256_file(&path).map_err(io)? != self.sha256.to_lowercase() {
            return Err(PackError::Hash { path });
        }
        Ok(())
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gc-models-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pack_for(bytes: u64, sha256: &str) -> Pack {
        Pack {
            name: "test".into(),
            licence: "test".into(),
            commercial_ok: false,
            files: vec![PackFile {
                path: "a.bin".into(),
                source_url: "https://example.invalid/a.bin".into(),
                sha256: sha256.into(),
                bytes,
                role: "test".into(),
            }],
        }
    }

    #[test]
    fn verifies_size_and_hash() {
        let store = temp_store("verify");
        std::fs::write(store.join("a.bin"), b"abc").unwrap();
        let good = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        pack_for(3, good).verify(&store).unwrap();
        assert!(matches!(pack_for(4, good).verify(&store), Err(PackError::Size { .. })));
        let bad = "00".repeat(32);
        assert!(matches!(pack_for(3, &bad).verify(&store), Err(PackError::Hash { .. })));
        std::fs::remove_dir_all(store).unwrap();
    }

    #[test]
    fn release_gate_refuses_uncleared_packs() {
        let pack = pack_for(0, "");
        assert!(matches!(pack.require_commercial(), Err(PackError::NotCommercial(_))));
    }
}
