//! The exchange itself: SPAKE2, key confirmation, and the sealed grant.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

use crate::{clean_device_name, parse_fingerprint, random_bytes, PairError};

/// Names of the two roles inside SPAKE2. They differ, so a message can never
/// be played back to the side that made it.
const DEVICE_SIDE: &[u8] = b"gc-device";
const HUB_SIDE: &[u8] = b"gc-hub";
/// Separates this protocol and this version of it from any other use of the
/// same primitives.
const CONTEXT: &[u8] = b"gods-camera/pair/v1";
/// The grant's layout. A device refuses a layout it does not know.
const GRANT_LAYOUT: u8 = 1;
const NONCE_BYTES: usize = 12;

/// What the hub hands a device at the end of pairing: who the device is from
/// now on, and the secret it presents with every request.
#[derive(Clone, PartialEq, Eq)]
pub struct Grant {
    pub device_id: String,
    pub token: String,
}

/// The token never appears in debug output.
impl fmt::Debug for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Grant").field("device_id", &self.device_id).field("token", &"…").finish()
    }
}

/// The keys both sides derive once SPAKE2 has finished. They agree only if
/// both used the same code and described the same exchange.
struct Keys {
    /// A digest of everything both sides must agree on.
    transcript: [u8; 32],
    device_confirmation: [u8; 32],
    hub_confirmation: [u8; 32],
    grant: [u8; 32],
}

/// The keys are wiped from memory when they are no longer needed.
impl Drop for Keys {
    fn drop(&mut self) {
        self.device_confirmation.zeroize();
        self.hub_confirmation.zeroize();
        self.grant.zeroize();
    }
}

impl Keys {
    fn derive(shared: &[u8], device_hello: &[u8], hub_hello: &[u8], device_name: &str, fingerprint: &[u8; 32]) -> Self {
        // Every field is written with its length, so that no two different
        // exchanges can produce the same bytes.
        let mut hash = Sha256::new();
        for field in [CONTEXT, device_hello, hub_hello, device_name.as_bytes(), fingerprint.as_slice()] {
            hash.update((field.len() as u32).to_be_bytes());
            hash.update(field);
        }
        let transcript: [u8; 32] = hash.finalize().into();

        let schedule = Hkdf::<Sha256>::new(Some(&transcript), shared);
        let key = |label: &[u8]| {
            let mut out = [0u8; 32];
            schedule.expand(&[CONTEXT, b" ", label].concat(), &mut out).expect("32 bytes is a valid HKDF output length");
            out
        };
        Self {
            transcript,
            device_confirmation: key(b"device confirmation"),
            hub_confirmation: key(b"hub confirmation"),
            grant: key(b"grant"),
        }
    }
}

fn confirmation(key: &[u8; 32], transcript: &[u8; 32]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(transcript);
    mac.finalize().into_bytes().to_vec()
}

/// Compare in constant time.
fn check_confirmation(key: &[u8; 32], transcript: &[u8; 32], presented: &[u8]) -> Result<(), PairError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(transcript);
    mac.verify_slice(presented).map_err(|_| PairError::Failed)
}

fn start(code: &crate::PairingCode, hub: bool) -> (Spake2<Ed25519Group>, Vec<u8>) {
    let password = Password::new(code.digits());
    let (device, hub_side) = (Identity::new(DEVICE_SIDE), Identity::new(HUB_SIDE));
    if hub {
        Spake2::<Ed25519Group>::start_b(&password, &device, &hub_side)
    } else {
        Spake2::<Ed25519Group>::start_a(&password, &device, &hub_side)
    }
}

/// The device's side of pairing.
pub struct Initiator {
    exchange: Spake2<Ed25519Group>,
    hello: Vec<u8>,
    device_name: String,
}

impl Initiator {
    /// Begin with the code the person typed and the name the hub will list
    /// this device under. The name is tidied with [`clean_device_name`].
    pub fn new(code: &crate::PairingCode, device_name: &str) -> Result<Self, PairError> {
        let device_name = clean_device_name(device_name).ok_or(PairError::InvalidName)?;
        let (exchange, hello) = start(code, false);
        Ok(Self { exchange, hello, device_name })
    }

    /// The device's name as it must be sent to the hub, unchanged.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The first message to the hub.
    pub fn hello(&self) -> &[u8] {
        &self.hello
    }

    /// Take the hub's answer and make the device's confirmation.
    ///
    /// `certificate_fingerprint` must be that of the certificate the hub
    /// showed on the very connection these messages travel over.
    pub fn finish(self, hub_hello: &[u8], certificate_fingerprint: &str) -> Result<(AwaitingGrant, Vec<u8>), PairError> {
        let fingerprint = parse_fingerprint(certificate_fingerprint)?;
        let shared = Zeroizing::new(self.exchange.finish(hub_hello).map_err(|_| PairError::Failed)?);
        let keys = Keys::derive(&shared, &self.hello, hub_hello, &self.device_name, &fingerprint);
        let confirmation = confirmation(&keys.device_confirmation, &keys.transcript);
        Ok((AwaitingGrant { keys }, confirmation))
    }
}

/// A device that has sent its confirmation and waits for the hub's.
pub struct AwaitingGrant {
    keys: Keys,
}

impl AwaitingGrant {
    /// Check the hub's confirmation and open the grant. An error means the hub
    /// did not know the code or showed another certificate: trust nothing.
    pub fn accept(self, hub_confirmation: &[u8], sealed_grant: &[u8]) -> Result<Grant, PairError> {
        check_confirmation(&self.keys.hub_confirmation, &self.keys.transcript, hub_confirmation)?;
        if sealed_grant.len() < NONCE_BYTES {
            return Err(PairError::Failed);
        }
        let (nonce, sealed) = sealed_grant.split_at(NONCE_BYTES);
        let nonce = Nonce::try_from(nonce).map_err(|_| PairError::Failed)?;
        let cipher = ChaCha20Poly1305::new(&Key::from(self.keys.grant));
        let plain = Zeroizing::new(
            cipher
                .decrypt(&nonce, Payload { msg: sealed, aad: &self.keys.transcript })
                .map_err(|_| PairError::Failed)?,
        );
        decode_grant(&plain).ok_or(PairError::Failed)
    }
}

/// The hub's side of pairing, after the device's first message.
pub struct Responder {
    keys: Keys,
    device_name: String,
}

impl Responder {
    /// Answer a device's first message. Returns the hub's state and its own
    /// message for the device.
    ///
    /// `device_name` must already be in its tidy form: the hub signs what the
    /// device sent, it does not correct it. `certificate_fingerprint` is the
    /// hub's own.
    pub fn start(
        code: &crate::PairingCode,
        device_name: &str,
        device_hello: &[u8],
        certificate_fingerprint: &str,
    ) -> Result<(Self, Vec<u8>), PairError> {
        if clean_device_name(device_name).as_deref() != Some(device_name) {
            return Err(PairError::InvalidName);
        }
        let fingerprint = parse_fingerprint(certificate_fingerprint)?;
        let (exchange, hello) = start(code, true);
        let shared = Zeroizing::new(exchange.finish(device_hello).map_err(|_| PairError::Failed)?);
        let keys = Keys::derive(&shared, device_hello, &hello, device_name, &fingerprint);
        Ok((Self { keys, device_name: device_name.to_string() }, hello))
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Check the device's confirmation. Only a device that knew the code, and
    /// that saw this hub's certificate, passes. Nothing is revealed to one
    /// that does not.
    pub fn verify(self, device_confirmation: &[u8]) -> Result<Verified, PairError> {
        check_confirmation(&self.keys.device_confirmation, &self.keys.transcript, device_confirmation)?;
        Ok(Verified { keys: self.keys, device_name: self.device_name })
    }
}

/// A hub that has checked the device and may now answer it.
pub struct Verified {
    keys: Keys,
    device_name: String,
}

impl Verified {
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The hub's confirmation, and the grant sealed so that only this device
    /// can open it.
    pub fn grant(self, grant: &Grant) -> (Vec<u8>, Vec<u8>) {
        let mut nonce = [0u8; NONCE_BYTES];
        random_bytes(&mut nonce);
        let cipher = ChaCha20Poly1305::new(&Key::from(self.keys.grant));
        let plain = Zeroizing::new(encode_grant(grant));
        let sealed = cipher
            .encrypt(&Nonce::from(nonce), Payload { msg: &plain, aad: &self.keys.transcript })
            .expect("a grant is far below the cipher's message limit");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&sealed);
        (confirmation(&self.keys.hub_confirmation, &self.keys.transcript), out)
    }
}

/// Layout: one byte for the layout's number, then the device id and the token,
/// each as a two-byte length and that many bytes of UTF-8.
fn encode_grant(grant: &Grant) -> Vec<u8> {
    let mut out = vec![GRANT_LAYOUT];
    for field in [&grant.device_id, &grant.token] {
        let length = u16::try_from(field.len()).expect("ids and tokens are short");
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(field.as_bytes());
    }
    out
}

fn decode_grant(bytes: &[u8]) -> Option<Grant> {
    let (&layout, mut rest) = bytes.split_first()?;
    if layout != GRANT_LAYOUT {
        return None;
    }
    let mut field = || {
        let (length, after) = rest.split_first_chunk::<2>()?;
        let length = u16::from_be_bytes(*length) as usize;
        if after.len() < length {
            return None;
        }
        let (value, after) = after.split_at(length);
        rest = after;
        String::from_utf8(value.to_vec()).ok()
    };
    let grant = Grant { device_id: field()?, token: field()? };
    rest.is_empty().then_some(grant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_survives_its_layout() {
        let grant = Grant { device_id: "3f2b".into(), token: "t".repeat(64) };
        assert_eq!(decode_grant(&encode_grant(&grant)), Some(grant.clone()));
        let empty = Grant { device_id: String::new(), token: String::new() };
        assert_eq!(decode_grant(&encode_grant(&empty)), Some(empty));

        let bytes = encode_grant(&grant);
        // Cut short, padded, or in an unknown layout: refused.
        assert_eq!(decode_grant(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode_grant(&[bytes.clone(), vec![0]].concat()), None);
        assert_eq!(decode_grant(&[vec![2], bytes[1..].to_vec()].concat()), None);
        assert_eq!(decode_grant(&[]), None);
        assert!(!format!("{grant:?}").contains("tttt"));
    }
}
