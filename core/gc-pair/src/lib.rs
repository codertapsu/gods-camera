//! Pairing: how a phone or tablet and the hub come to trust each other.
//!
//! The hub shows a six-digit code and the person types it into the device. The
//! two sides then run SPAKE2, a password-authenticated key exchange: each
//! proves to the other that it knows the code, without sending anything that a
//! listener could test guesses against. Someone on the network gets one guess
//! per attempt, and the hub allows only a few attempts.
//!
//! The exchange also covers the fingerprint of the hub's TLS certificate as the
//! device saw it on its own connection. If someone sits in the middle with a
//! different certificate, the two sides disagree and pairing fails. From then
//! on the device trusts exactly that certificate, and presents the token it was
//! granted with every request.
//!
//! The order of messages:
//!
//! 1. The device sends its name and its SPAKE2 message ([`Initiator::hello`]).
//! 2. The hub answers with its own SPAKE2 message ([`Responder::start`]).
//! 3. The device sends a confirmation ([`Initiator::finish`]).
//! 4. The hub checks it ([`Responder::verify`]) and only then answers, with its
//!    own confirmation and the sealed grant ([`Verified::grant`]).
//! 5. The device checks the hub's confirmation and opens the grant
//!    ([`AwaitingGrant::accept`]).
//!
//! Nothing here touches the network or the clock. The hub's server and a
//! device's HTTP client carry the messages.

mod exchange;
#[cfg(feature = "hub")]
mod identity;
#[cfg(feature = "hub")]
mod registry;

use sha2::{Digest, Sha256};
use std::fmt;

pub use exchange::{AwaitingGrant, Grant, Initiator, Responder, Verified};
#[cfg(feature = "hub")]
pub use identity::{HubIdentity, IdentityError};
#[cfg(feature = "hub")]
pub use registry::{DeviceRecord, DeviceRegistry, RegistryError, MAX_DEVICES};

/// The longest device name, in characters.
pub const MAX_DEVICE_NAME: usize = 64;

/// Why pairing could not go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairError {
    #[error("a pairing code is six digits")]
    InvalidCode,
    #[error("a device name is 1 to 64 characters, without control characters")]
    InvalidName,
    #[error("a certificate fingerprint is 64 hexadecimal digits")]
    InvalidFingerprint,
    /// The other side used a different code, saw a different certificate, or a
    /// message was changed on the way. Which of these it was cannot be told,
    /// by design: telling would help someone who is guessing.
    #[error("pairing failed")]
    Failed,
}

/// The six digits the hub shows and the person types into the device.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode(String);

impl PairingCode {
    /// A new code from the operating system's random generator.
    pub fn generate() -> Self {
        // Numbers at or above the limit are drawn again, so that every code is
        // equally likely.
        const LIMIT: u32 = u32::MAX - u32::MAX % 1_000_000;
        loop {
            let mut bytes = [0u8; 4];
            random_bytes(&mut bytes);
            let value = u32::from_le_bytes(bytes);
            if value < LIMIT {
                return Self(format!("{:06}", value % 1_000_000));
            }
        }
    }

    /// Read a code as a person types it: spaces and dashes are ignored.
    pub fn parse(text: &str) -> Result<Self, PairError> {
        let digits: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit()) {
            Ok(Self(digits))
        } else {
            Err(PairError::InvalidCode)
        }
    }

    /// The six digits without a space.
    pub fn digits(&self) -> &str {
        &self.0
    }
}

/// The code as it is shown: two groups of three, "123 456".
impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", &self.0[..3], &self.0[3..])
    }
}

/// A code never appears in debug output, so it cannot leak into a log by accident.
impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCode(······)")
    }
}

impl std::str::FromStr for PairingCode {
    type Err = PairError;

    fn from_str(text: &str) -> Result<Self, PairError> {
        Self::parse(text)
    }
}

/// A device name in the form both sides sign: trimmed, runs of white space as
/// one space, control characters dropped, at most [`MAX_DEVICE_NAME`]
/// characters. None when nothing is left.
pub fn clean_device_name(raw: &str) -> Option<String> {
    let mut name = String::new();
    for word in raw.split_whitespace() {
        let word: String = word.chars().filter(|c| !c.is_control()).collect();
        if word.is_empty() {
            continue;
        }
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(&word);
    }
    let name: String = name.chars().take(MAX_DEVICE_NAME).collect();
    let name = name.trim_end().to_string();
    (!name.is_empty()).then_some(name)
}

/// The SHA-256 of a certificate in DER form, as 64 lower-case hexadecimal
/// digits. This is what a device pins after pairing.
pub fn certificate_fingerprint(certificate_der: &[u8]) -> String {
    hex::encode(Sha256::digest(certificate_der))
}

/// Read a fingerprint as 32 bytes. Colons and spaces between the digits are
/// allowed, and so are capital letters.
pub fn parse_fingerprint(text: &str) -> Result<[u8; 32], PairError> {
    let digits: String = text.chars().filter(|c| *c != ':' && !c.is_whitespace()).collect();
    let bytes = hex::decode(digits).map_err(|_| PairError::InvalidFingerprint)?;
    bytes.try_into().map_err(|_| PairError::InvalidFingerprint)
}

/// Fill `buffer` from the operating system's random generator.
///
/// Without random numbers nothing here is safe, so a failure stops the caller
/// instead of going on with predictable values.
pub(crate) fn random_bytes(buffer: &mut [u8]) {
    getrandom::fill(buffer).expect("the operating system's random generator is available");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_code_is_six_digits_and_reads_back() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let code = PairingCode::generate();
            assert_eq!(code.digits().len(), 6);
            assert!(code.digits().bytes().all(|b| b.is_ascii_digit()));
            assert_eq!(PairingCode::parse(&code.to_string()).unwrap(), code);
            seen.insert(code.digits().to_string());
        }
        // 200 draws from a million codes: repeats are possible but rare.
        assert!(seen.len() > 190, "codes repeat too often: {} distinct of 200", seen.len());
    }

    #[test]
    fn a_code_is_shown_in_two_groups_and_hidden_from_debug_output() {
        let code = PairingCode::parse("048213").unwrap();
        assert_eq!(code.to_string(), "048 213");
        assert_eq!(code.digits(), "048213");
        assert!(!format!("{code:?}").contains("048"));
    }

    #[test]
    fn a_typed_code_may_hold_spaces_and_dashes_but_nothing_else() {
        for typed in ["123456", "123 456", " 123-456 ", "1 2 3 4 5 6", "12-34-56"] {
            assert_eq!(PairingCode::parse(typed).unwrap().digits(), "123456", "{typed:?}");
        }
        for typed in ["", "12345", "1234567", "12345a", "123.456", "١٢٣٤٥٦", "１２３４５６"] {
            assert_eq!(PairingCode::parse(typed), Err(PairError::InvalidCode), "{typed:?}");
        }
        assert_eq!("987 654".parse::<PairingCode>().unwrap().digits(), "987654");
    }

    #[test]
    fn device_names_are_tidied_the_same_way_on_both_sides() {
        assert_eq!(clean_device_name("  Galaxy Tab S9+  ").as_deref(), Some("Galaxy Tab S9+"));
        assert_eq!(clean_device_name("Máy tính\tbảng\n của tôi").as_deref(), Some("Máy tính bảng của tôi"));
        assert_eq!(clean_device_name("a\u{0}b\u{7}c").as_deref(), Some("abc"));
        assert_eq!(clean_device_name(" \t\n"), None);
        assert_eq!(clean_device_name("\u{1}"), None);
        let long = "ă".repeat(200);
        let cleaned = clean_device_name(&long).unwrap();
        assert_eq!(cleaned.chars().count(), MAX_DEVICE_NAME);
        // Cleaning a clean name changes nothing, which is what the hub checks.
        assert_eq!(clean_device_name(&cleaned).as_deref(), Some(cleaned.as_str()));
        let cut = format!("{} x", "a".repeat(MAX_DEVICE_NAME - 1));
        assert_eq!(clean_device_name(&cut).unwrap(), "a".repeat(MAX_DEVICE_NAME - 1));
    }

    #[test]
    fn fingerprints_are_sha256_in_hex() {
        let fingerprint = certificate_fingerprint(b"abc");
        assert_eq!(fingerprint, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let bytes = parse_fingerprint(&fingerprint).unwrap();
        assert_eq!(hex::encode(bytes), fingerprint);
        // As other tools print them.
        let printed = "BA:78:16:BF:8F:01:CF:EA:41:41:40:DE:5D:AE:22:23:B0:03:61:A3:96:17:7A:9C:B4:10:FF:61:F2:00:15:AD";
        assert_eq!(parse_fingerprint(printed).unwrap(), bytes);
        for bad in ["", "ba78", "zz7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", &fingerprint[1..]] {
            assert_eq!(parse_fingerprint(bad), Err(PairError::InvalidFingerprint), "{bad:?}");
        }
    }
}
