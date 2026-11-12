//! Secrets in Credential Manager. The test uses an entry of its own and
//! removes it; the app's entries are never touched.
#![cfg(windows)]

use gc_win::Credential;

#[test]
fn a_secret_is_kept_replaced_and_removed() {
    let name = format!("GodsCamera.Test.{}", std::process::id());
    let entry = Credential::new(&name);
    entry.delete();
    assert_eq!(entry.read().unwrap(), None);

    entry.write(&[7u8; 32], "A test of God's Camera. Safe to remove.").unwrap();
    assert_eq!(entry.read().unwrap().as_deref(), Some(&[7u8; 32][..]));
    // Another handle to the same name, as after the app is started again, finds it too.
    assert_eq!(Credential::new(&name).read().unwrap().as_deref(), Some(&[7u8; 32][..]));

    entry.write(b"shorter", "A test of God's Camera. Safe to remove.").unwrap();
    assert_eq!(entry.read().unwrap().as_deref(), Some(&b"shorter"[..]));

    entry.delete();
    assert_eq!(entry.read().unwrap(), None);
    entry.delete();
}
