//! The key for saved faces in Credential Manager. The test uses an entry of
//! its own and removes it; the app's entry is never touched.
#![cfg(windows)]

use gc_app::faces::{FaceLibrary, KeyStore};
use gc_win::CredentialKey;

#[test]
fn the_key_is_made_once_kept_and_gone_after_deleting() {
    let store = CredentialKey::new(&format!("GodsCamera.Test.{}", std::process::id()));
    store.delete();
    let key = store.load_or_create().unwrap();
    assert_ne!(key, [0u8; 32]);
    assert_eq!(store.load_or_create().unwrap(), key, "the second asking finds the first key");
    // Another program instance of the same user finds it too.
    assert_eq!(CredentialKey::new(&format!("GodsCamera.Test.{}", std::process::id())).load_or_create().unwrap(), key);

    // A face sealed with it opens with it, and not after the key is replaced.
    let folder = std::env::temp_dir().join(format!("gc-win-vault-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let face = gc_app::faces::SavedFace {
        id: gc_app::faces::new_id(),
        name: "Mai".into(),
        added: 1,
        consent: gc_app::faces::Consent::Myself,
        identity: vec![0.5; 512],
        photos: vec![gc_app::faces::FacePhoto { id: gc_app::faces::new_id(), identity: vec![0.5; 512], thumbnail: vec![1, 2, 3] }],
    };
    FaceLibrary::new(&folder, key).unwrap().save(&face).unwrap();
    assert_eq!(FaceLibrary::new(&folder, store.load_or_create().unwrap()).unwrap().list(), std::slice::from_ref(&face));

    store.delete();
    let fresh = store.load_or_create().unwrap();
    assert_ne!(fresh, key, "deleting the key makes the old faces unreadable for good");
    assert!(FaceLibrary::new(&folder, fresh).unwrap().list().is_empty());
    store.delete();
    store.delete();
    let _ = std::fs::remove_dir_all(&folder);
}
