//! The keys the cloud providers ask for.

use std::fmt;

use zeroize::Zeroize;

use crate::LlmError;

/// A key for a provider's API, as the app read it from the Keychain or from
/// Credential Manager. This crate stores it nowhere and sends it only in the
/// request header its provider reads it from: never in a URL, a body, an
/// error or a log. A Debug print says that there is a key, never what it is,
/// and the memory the key sat in is wiped when it is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// The key as the user entered it, without white space at either end.
    /// Refused when it is empty, or has a character no key has: it goes into
    /// an HTTP header, where a line break would end the header early.
    pub fn new(key: impl Into<String>) -> Result<ApiKey, LlmError> {
        let mut entered = key.into();
        let trimmed = entered.trim();
        let usable = !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_graphic());
        let result = if usable {
            Ok(ApiKey(trimmed.to_string()))
        } else {
            Err(LlmError::InvalidInput { message: "the key is empty or has characters a key cannot have".into() })
        };
        entered.zeroize();
        result
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(hidden)")
    }
}

impl Drop for ApiKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_trimmed_and_never_printed() {
        let key = ApiKey::new("  sk-test-not-a-real-key-0123456789\n").unwrap();
        assert_eq!(key.expose(), "sk-test-not-a-real-key-0123456789");
        let printed = format!("{key:?}");
        assert!(!printed.contains("0123456789"), "{printed}");
    }

    #[test]
    fn a_key_that_cannot_go_into_a_header_is_refused() {
        assert!(ApiKey::new("").is_err());
        assert!(ApiKey::new("   ").is_err());
        assert!(ApiKey::new("sk-one\r\nX-Other: two").is_err());
        assert!(ApiKey::new("khóa").is_err());
        // The error does not repeat what was entered.
        let error = ApiKey::new("sk-line\nbreak").unwrap_err().to_string();
        assert!(!error.contains("sk-line"), "{error}");
    }
}
