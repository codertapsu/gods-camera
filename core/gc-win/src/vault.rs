//! The key that encrypts saved faces, kept in Windows' Credential Manager:
//! apart from the files it protects, and readable only as this user.

use gc_app::faces::{new_key, FaceError, KeyStore};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

/// The name the app's key for saved faces goes under.
pub const FACE_KEY: &str = "GodsCamera.FaceStore";

/// One key in Credential Manager.
pub struct CredentialKey {
    name: String,
}

impl CredentialKey {
    /// `name` is what the entry is called; Credential Manager shows it to the user.
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string() }
    }

    fn read(&self) -> Result<Option<[u8; 32]>, FaceError> {
        let mut found: *mut CREDENTIALW = std::ptr::null_mut();
        match unsafe { CredReadW(&HSTRING::from(self.name.as_str()), CRED_TYPE_GENERIC, None, &mut found) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_NOT_FOUND.to_hresult() => return Ok(None),
            Err(error) => return Err(FaceError::Key(format!("Credential Manager answered {:#010x}", error.code().0))),
        }
        // Copy the key out and give Windows its memory back.
        let key = unsafe {
            let credential = &*found;
            let key = (credential.CredentialBlobSize == 32 && !credential.CredentialBlob.is_null())
                .then(|| std::slice::from_raw_parts(credential.CredentialBlob, 32).try_into().ok())
                .flatten();
            CredFree(found.cast());
            key
        };
        // An entry of another length is not a key of ours: a new one replaces it.
        Ok(key)
    }

    fn write(&self, key: &[u8; 32]) -> Result<(), FaceError> {
        let mut name: Vec<u16> = self.name.encode_utf16().chain([0]).collect();
        let mut comment: Vec<u16> = "The key that encrypts the faces saved in God's Camera.".encode_utf16().chain([0]).collect();
        let mut user: Vec<u16> = "God's Camera".encode_utf16().chain([0]).collect();
        let mut blob = *key;
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_mut_ptr()),
            Comment: PWSTR(comment.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            // On this computer only: it does not travel with a roaming profile.
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        };
        let written = unsafe { CredWriteW(&credential, 0) };
        blob.fill(0);
        written.map_err(|error| FaceError::Key(format!("Credential Manager answered {:#010x}", error.code().0)))
    }
}

impl KeyStore for CredentialKey {
    fn load_or_create(&self) -> Result<[u8; 32], FaceError> {
        if let Some(key) = self.read()? {
            return Ok(key);
        }
        let key = new_key();
        self.write(&key)?;
        Ok(key)
    }

    fn delete(&self) {
        // Nothing to delete is as good as deleted.
        let _ = unsafe { CredDeleteW(&HSTRING::from(self.name.as_str()), CRED_TYPE_GENERIC, None) };
    }
}
