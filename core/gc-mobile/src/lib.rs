//! The core as a phone or tablet sees it: a small interface that UniFFI turns
//! into Kotlin and Swift.
//!
//! For now this is the device's half of pairing with a hub. The app does the
//! talking (HTTPS requests, JSON, base64); this library does the arithmetic,
//! the same code the hub runs on its side.
//!
//! How an app pairs:
//!
//! 1. The person types the six digits the hub shows. Make a `PairingClient`
//!    with them and with this device's name.
//! 2. Connect to the hub over TLS, taking its certificate as it comes, and
//!    note that certificate. `POST /v1/pair/start` with `device_name()` and
//!    `hello()`.
//! 3. Call `finish` with the hub's message and `certificate_fingerprint` of
//!    the certificate from step 2. `POST /v1/pair/finish` with the result.
//! 4. Call `accept` with the hub's confirmation and the grant. If it
//!    succeeds, the hub is the one that shows the code: store the
//!    fingerprint, the device id and the token. If it fails, store nothing.
//!
//! Afterwards, accept only a hub whose certificate has the stored
//! fingerprint, and send the token as `Authorization: Bearer`.

use gc_pair::{AwaitingGrant, Initiator, PairError, PairingCode};
use std::sync::{Arc, Mutex};

uniffi::setup_scaffolding!();

/// Why pairing could not go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Error)]
pub enum PairingError {
    /// A pairing code is six digits.
    InvalidCode,
    /// A device name is 1 to 64 characters.
    InvalidName,
    /// A certificate fingerprint is 64 hexadecimal digits.
    InvalidFingerprint,
    /// The code was wrong, or what answered is not the hub that shows the
    /// code. Which of the two cannot be told. Trust nothing that was
    /// received, and begin again with a new `PairingClient`.
    Failed,
    /// The steps were called out of order, or one was called twice.
    OutOfOrder,
}

impl std::fmt::Display for PairingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidCode => "a pairing code is six digits",
            Self::InvalidName => "a device name is 1 to 64 characters",
            Self::InvalidFingerprint => "a certificate fingerprint is 64 hexadecimal digits",
            Self::Failed => "pairing failed",
            Self::OutOfOrder => "the pairing steps were called out of order",
        })
    }
}

impl std::error::Error for PairingError {}

impl From<PairError> for PairingError {
    fn from(error: PairError) -> Self {
        match error {
            PairError::InvalidCode => Self::InvalidCode,
            PairError::InvalidName => Self::InvalidName,
            PairError::InvalidFingerprint => Self::InvalidFingerprint,
            PairError::Failed => Self::Failed,
        }
    }
}

/// What a device keeps after pairing, together with the hub's address and the
/// fingerprint of its certificate. The token is a secret: store it where only
/// this app can read it.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairedHub {
    /// This device's id on the hub.
    pub device_id: String,
    /// What this device presents with every request.
    pub token: String,
}

enum Step {
    /// Made, and `finish` not yet called.
    Begun(Initiator),
    /// `finish` done, and `accept` not yet called.
    Confirmed(AwaitingGrant),
    /// Finished or failed. A new client is needed to try again.
    Spent,
}

/// One attempt to pair with a hub. It is good for one exchange: after a
/// failure, make a new one.
#[derive(uniffi::Object)]
pub struct PairingClient {
    step: Mutex<Step>,
    hello: Vec<u8>,
    device_name: String,
}

#[uniffi::export]
impl PairingClient {
    /// Begin with the code the person typed (spaces and dashes are ignored)
    /// and the name the hub will list this device under.
    #[uniffi::constructor]
    pub fn new(code: String, device_name: String) -> Result<Arc<Self>, PairingError> {
        let initiator = Initiator::new(&PairingCode::parse(&code)?, &device_name)?;
        Ok(Arc::new(Self {
            hello: initiator.hello().to_vec(),
            device_name: initiator.device_name().to_string(),
            step: Mutex::new(Step::Begun(initiator)),
        }))
    }

    /// The device's name in its tidy form. Send exactly this as `device_name`.
    pub fn device_name(&self) -> String {
        self.device_name.clone()
    }

    /// The first message for the hub: `message` of `POST /v1/pair/start`,
    /// before base64.
    pub fn hello(&self) -> Vec<u8> {
        self.hello.clone()
    }

    /// Take the hub's message and return this device's confirmation:
    /// `confirmation` of `POST /v1/pair/finish`, before base64.
    ///
    /// `certificate_fingerprint` must come from the certificate the hub
    /// showed on the connection these requests travel over; see
    /// `certificate_fingerprint`.
    pub fn finish(&self, hub_message: Vec<u8>, certificate_fingerprint: String) -> Result<Vec<u8>, PairingError> {
        // Checked first, so that a slip here does not use the exchange up.
        gc_pair::parse_fingerprint(&certificate_fingerprint)?;
        let mut step = self.step.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match std::mem::replace(&mut *step, Step::Spent) {
            Step::Begun(initiator) => {
                let (awaiting, confirmation) = initiator.finish(&hub_message, &certificate_fingerprint)?;
                *step = Step::Confirmed(awaiting);
                Ok(confirmation)
            }
            other => {
                *step = other;
                Err(PairingError::OutOfOrder)
            }
        }
    }

    /// Check the hub's confirmation and open the grant. Success means the hub
    /// is the one that shows the code, and this device is now paired.
    pub fn accept(&self, hub_confirmation: Vec<u8>, grant: Vec<u8>) -> Result<PairedHub, PairingError> {
        let mut step = self.step.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match std::mem::replace(&mut *step, Step::Spent) {
            Step::Confirmed(awaiting) => {
                let grant = awaiting.accept(&hub_confirmation, &grant)?;
                Ok(PairedHub { device_id: grant.device_id, token: grant.token })
            }
            other => {
                *step = other;
                Err(PairingError::OutOfOrder)
            }
        }
    }
}

/// The SHA-256 of a certificate in DER form (`X509Certificate.getEncoded()` on
/// Android), as 64 hexadecimal digits. This goes into `PairingClient::finish`,
/// and is what the app pins once pairing has succeeded.
#[uniffi::export]
pub fn certificate_fingerprint(certificate_der: Vec<u8>) -> String {
    gc_pair::certificate_fingerprint(&certificate_der)
}

/// The version of the core, for diagnostics.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gc_pair::{Grant, Responder};

    fn hub_certificate() -> String {
        certificate_fingerprint(b"the hub's certificate".to_vec())
    }

    fn granted() -> Grant {
        Grant { device_id: "0d9f6e7a-52b1-4c1e-9a37-1f1f0c6f2f10".into(), token: "ab".repeat(32) }
    }

    /// The hub's side, for a device that sent `name` and `hello`.
    fn hub(code: &str, name: &str, hello: &[u8]) -> (Responder, Vec<u8>) {
        Responder::start(&PairingCode::parse(code).unwrap(), name, hello, &hub_certificate()).unwrap()
    }

    #[test]
    fn a_device_pairs_step_by_step() {
        let client = PairingClient::new("482 913".into(), "  Galaxy   Tab S9+ ".into()).unwrap();
        assert_eq!(client.device_name(), "Galaxy Tab S9+");
        assert_eq!(client.hello().len(), 33);
        let (responder, hub_hello) = hub("482913", &client.device_name(), &client.hello());
        let confirmation = client.finish(hub_hello, hub_certificate()).unwrap();
        let (hub_confirmation, grant) = responder.verify(&confirmation).unwrap().grant(&granted());
        let paired = client.accept(hub_confirmation, grant).unwrap();
        assert_eq!(paired.device_id, granted().device_id);
        assert_eq!(paired.token, granted().token);
    }

    #[test]
    fn a_wrong_code_or_another_certificate_is_a_failure_without_details() {
        // The person mistyped: the hub refuses the confirmation.
        let client = PairingClient::new("482914".into(), "tablet".into()).unwrap();
        let (responder, hub_hello) = hub("482913", "tablet", &client.hello());
        let confirmation = client.finish(hub_hello, hub_certificate()).unwrap();
        assert!(responder.verify(&confirmation).is_err());
        // Whatever comes back instead of a grant is refused by the device.
        assert_eq!(client.accept(vec![0; 32], vec![0; 64]).err(), Some(PairingError::Failed));

        // The right code, but the device's connection ended somewhere else.
        let client = PairingClient::new("482913".into(), "tablet".into()).unwrap();
        let (responder, hub_hello) = hub("482913", "tablet", &client.hello());
        let elsewhere = certificate_fingerprint(b"somebody else's certificate".to_vec());
        let confirmation = client.finish(hub_hello, elsewhere).unwrap();
        assert!(responder.verify(&confirmation).is_err());
    }

    #[test]
    fn the_steps_go_in_order_and_only_once() {
        let client = PairingClient::new("482913".into(), "tablet".into()).unwrap();
        // `accept` before `finish` changes nothing.
        assert_eq!(client.accept(vec![0; 32], vec![0; 64]).err(), Some(PairingError::OutOfOrder));
        // A fingerprint that is not one does not use the exchange up either.
        let (responder, hub_hello) = hub("482913", "tablet", &client.hello());
        assert_eq!(client.finish(hub_hello.clone(), "abc".into()).err(), Some(PairingError::InvalidFingerprint));
        let confirmation = client.finish(hub_hello.clone(), hub_certificate()).unwrap();
        assert_eq!(client.finish(hub_hello, hub_certificate()).err(), Some(PairingError::OutOfOrder));
        let (hub_confirmation, grant) = responder.verify(&confirmation).unwrap().grant(&granted());
        assert!(client.accept(hub_confirmation.clone(), grant.clone()).is_ok());
        assert_eq!(client.accept(hub_confirmation, grant).err(), Some(PairingError::OutOfOrder));

        // A message that is not from a hub spends the client.
        let client = PairingClient::new("482913".into(), "tablet".into()).unwrap();
        assert_eq!(client.finish(vec![1, 2, 3], hub_certificate()).err(), Some(PairingError::Failed));
        assert_eq!(client.finish(vec![1, 2, 3], hub_certificate()).err(), Some(PairingError::OutOfOrder));
    }

    #[test]
    fn what_the_person_typed_is_checked_at_once() {
        assert_eq!(PairingClient::new("12345".into(), "tablet".into()).err(), Some(PairingError::InvalidCode));
        assert_eq!(PairingClient::new("12345a".into(), "tablet".into()).err(), Some(PairingError::InvalidCode));
        assert_eq!(PairingClient::new("123456".into(), "  ".into()).err(), Some(PairingError::InvalidName));
        assert!(PairingClient::new("123-456".into(), "tablet".into()).is_ok());
        assert_eq!(certificate_fingerprint(b"abc".to_vec()), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
    }
}
