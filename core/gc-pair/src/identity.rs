//! The hub's TLS identity: one certificate that it signs itself, and the key
//! that goes with it.
//!
//! No authority vouches for this certificate. A device trusts it because the
//! pairing exchange covered its fingerprint, and from then on accepts that
//! certificate and no other.

use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use std::fmt;
use zeroize::Zeroize;

use crate::certificate_fingerprint;

/// How long a new certificate is valid. Devices compare fingerprints, not
/// dates, so this only has to outlast the installation.
const VALID_DAYS: i64 = 3650;

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("the hub's certificate could not be made: {0}")]
    Generate(String),
    #[error("the hub's identity is not a certificate followed by its private key in PEM form")]
    Malformed,
}

/// A certificate and its private key.
#[derive(Clone)]
pub struct HubIdentity {
    certificate_der: Vec<u8>,
    /// PKCS#8.
    private_key_der: Vec<u8>,
    pem: String,
}

impl HubIdentity {
    /// Make a new identity: an ECDSA P-256 key and a certificate signed with
    /// it. `name` is the hub's name as a person knows it, such as the
    /// computer's name; it goes into the certificate's subject.
    ///
    /// Making a new identity cuts off every paired device, because each one
    /// trusts the old certificate only.
    pub fn generate(name: &str) -> Result<Self, IdentityError> {
        let failed = |error: rcgen::Error| IdentityError::Generate(error.to_string());
        // P-256, because Android's TLS does not accept Ed25519 certificates.
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(failed)?;
        let mut params = CertificateParams::new(vec!["godscamera-hub".to_string()]).map_err(failed)?;
        let subject: String = name.chars().filter(|c| !c.is_control()).take(48).collect();
        let subject = if subject.trim().is_empty() { "hub".to_string() } else { subject.trim().to_string() };
        params.distinguished_name.push(DnType::CommonName, format!("God's Camera ({subject})"));
        let now = time::OffsetDateTime::now_utc();
        // A day of slack for a device whose clock runs behind.
        params.not_before = now - time::Duration::days(1);
        params.not_after = now + time::Duration::days(VALID_DAYS);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let certificate = params.self_signed(&key).map_err(failed)?;
        Ok(Self {
            certificate_der: certificate.der().to_vec(),
            private_key_der: key.serialize_der(),
            pem: format!("{}{}", certificate.pem(), key.serialize_pem()),
        })
    }

    /// Read an identity written by [`HubIdentity::to_pem`].
    pub fn from_pem(pem: &str) -> Result<Self, IdentityError> {
        let certificate = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|_| IdentityError::Malformed)?;
        let key = PrivatePkcs8KeyDer::from_pem_slice(pem.as_bytes()).map_err(|_| IdentityError::Malformed)?;
        Ok(Self {
            certificate_der: certificate.to_vec(),
            private_key_der: key.secret_pkcs8_der().to_vec(),
            pem: pem.to_string(),
        })
    }

    /// The certificate and then the private key, in PEM form. This holds the
    /// key: keep it where only the hub can read it.
    pub fn to_pem(&self) -> &str {
        &self.pem
    }

    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    /// The private key in PKCS#8 form.
    pub fn private_key_der(&self) -> &[u8] {
        &self.private_key_der
    }

    /// The SHA-256 of the certificate, as 64 hexadecimal digits.
    pub fn fingerprint(&self) -> String {
        certificate_fingerprint(&self.certificate_der)
    }
}

/// The private key is wiped from memory when the identity is let go.
impl Drop for HubIdentity {
    fn drop(&mut self) {
        self.private_key_der.zeroize();
        self.pem.zeroize();
    }
}

/// The private key never appears in debug output.
impl fmt::Debug for HubIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HubIdentity").field("fingerprint", &self.fingerprint()).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_survives_pem_and_keeps_its_fingerprint() {
        let made = HubIdentity::generate("Máy Mac của tôi").unwrap();
        assert_eq!(made.fingerprint().len(), 64);
        let pem = made.to_pem().to_string();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(pem.contains("-----BEGIN PRIVATE KEY-----"));

        let read = HubIdentity::from_pem(&pem).unwrap();
        assert_eq!(read.fingerprint(), made.fingerprint());
        assert_eq!(read.certificate_der(), made.certificate_der());
        assert_eq!(read.private_key_der(), made.private_key_der());
        assert_eq!(read.to_pem(), pem);
        assert!(!format!("{read:?}").contains("PRIVATE"));
    }

    #[test]
    fn every_identity_is_different() {
        let (one, two) = (HubIdentity::generate("hub").unwrap(), HubIdentity::generate("hub").unwrap());
        assert_ne!(one.fingerprint(), two.fingerprint());
        assert_ne!(one.private_key_der(), two.private_key_der());
        // An empty or strange name still gives a certificate.
        assert!(HubIdentity::generate("").is_ok());
        assert!(HubIdentity::generate("\u{0}\n").is_ok());
    }

    #[test]
    fn text_that_is_not_an_identity_is_refused() {
        let made = HubIdentity::generate("hub").unwrap();
        let pem = made.to_pem();
        let key_at = pem.find("-----BEGIN PRIVATE KEY-----").unwrap();
        for bad in ["", "not pem at all", &pem[..key_at], &pem[key_at..]] {
            assert!(matches!(HubIdentity::from_pem(bad), Err(IdentityError::Malformed)), "{bad:.30}");
        }
    }
}
