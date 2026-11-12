//! Small secrets in Windows' Credential Manager: apart from the app's files,
//! and readable only as this user. The key that encrypts saved faces is kept here.

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

/// One entry in Credential Manager.
pub struct Credential {
    name: String,
}

fn refused(error: windows::core::Error) -> String {
    format!("Credential Manager answered {:#010x}", error.code().0)
}

impl Credential {
    /// `name` is what the entry is called; Credential Manager shows it to the user.
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string() }
    }

    /// The secret, or `None` if there is no such entry.
    pub fn read(&self) -> Result<Option<Vec<u8>>, String> {
        let mut found: *mut CREDENTIALW = std::ptr::null_mut();
        match unsafe { CredReadW(&HSTRING::from(self.name.as_str()), CRED_TYPE_GENERIC, None, &mut found) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_NOT_FOUND.to_hresult() => return Ok(None),
            Err(error) => return Err(refused(error)),
        }
        // Copy the secret out and give Windows its memory back.
        let secret = unsafe {
            let credential = &*found;
            let secret = match credential.CredentialBlob.is_null() {
                true => Vec::new(),
                false => std::slice::from_raw_parts(credential.CredentialBlob, credential.CredentialBlobSize as usize).to_vec(),
            };
            CredFree(found.cast());
            secret
        };
        Ok(Some(secret))
    }

    /// Keep `secret` under this name, in place of whatever was there.
    /// `comment` says what it is, for the user who looks into Credential Manager.
    pub fn write(&self, secret: &[u8], comment: &str) -> Result<(), String> {
        let mut name: Vec<u16> = self.name.encode_utf16().chain([0]).collect();
        let mut comment: Vec<u16> = comment.encode_utf16().chain([0]).collect();
        let mut user: Vec<u16> = "God's Camera".encode_utf16().chain([0]).collect();
        let mut blob = secret.to_vec();
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
        written.map_err(refused)
    }

    /// Remove the entry. Nothing to remove is as good as removed.
    pub fn delete(&self) {
        let _ = unsafe { CredDeleteW(&HSTRING::from(self.name.as_str()), CRED_TYPE_GENERIC, None) };
    }
}
