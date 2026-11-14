//! What can go wrong, in words an app can show or log.
//!
//! The apps show their own text for each kind in English and Vietnamese; the
//! English here is for logs and for the provider's own message, which comes
//! along where there is one. No error ever holds a key: a provider's message
//! is cleaned of anything that looks like one before it is kept.

use std::fmt;
use std::time::Duration;

use crate::{ApiKey, Feature};

#[derive(Debug, Clone, PartialEq)]
pub enum LlmError {
    /// The provider needs a key and none was given.
    NoKey { provider: String },
    /// The provider did not accept the key: wrong, revoked, or without
    /// access to what was asked.
    KeyRefused { provider: String, message: String },
    /// Too many requests or tokens for the account's limits. `retry_after`
    /// is the wait the provider asked for, when it named one.
    RateLimited { provider: String, retry_after: Option<Duration>, message: String },
    /// The provider is overloaded and asks to come back later.
    Overloaded { provider: String, message: String },
    /// The provider failed with an error of its own.
    ServerError { provider: String, status: u16, message: String },
    /// The provider refused the request as it was: a wrong model name, a
    /// schema it cannot follow, a picture too large. The message is its own.
    BadRequest { provider: String, status: u16, message: String },
    /// The model declined to answer, or the provider's safety filter stopped
    /// it. The caller decides whether to try another provider or model.
    Refused { provider: String, model: String, category: Option<String>, explanation: Option<String> },
    /// No answer came in time.
    TimedOut { provider: String },
    /// The provider could not be reached, or the connection broke.
    Network { provider: String, message: String },
    /// The provider answered with something that could not be read.
    Unexpected { provider: String, message: String },
    /// The feature is set to stay on the user's devices, and the provider is
    /// a cloud service.
    CloudNotAllowed { feature: Feature, provider: String },
    /// The gateway knows no provider by this name.
    UnknownProvider { name: String },
    /// The request cannot be sent as it is.
    InvalidInput { message: String },
    /// A file in the gateway's folder could not be read or written, or the
    /// gateway could not start its threads.
    Storage { message: String },
    /// The caller asked to stop.
    Cancelled,
}

impl LlmError {
    /// Whether the same request may work if it is sent again later.
    pub fn is_temporary(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimited { .. }
                | LlmError::Overloaded { .. }
                | LlmError::ServerError { .. }
                | LlmError::TimedOut { .. }
                | LlmError::Network { .. }
        )
    }

    /// The wait the provider asked for, when it named one.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            LlmError::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// A refusal known only from an error answer does not say which model
    /// declined; the caller knows.
    pub(crate) fn for_model(self, asked: &str) -> LlmError {
        match self {
            LlmError::Refused { provider, model, category, explanation } if model.is_empty() => {
                LlmError::Refused { provider, model: asked.to_string(), category, explanation }
            }
            other => other,
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::NoKey { provider } => write!(f, "there is no key for {provider}; add one in the settings"),
            LlmError::KeyRefused { provider, message } => write!(f, "{provider} did not accept the key: {message}"),
            LlmError::RateLimited { provider, retry_after: Some(wait), message } => {
                write!(f, "{provider} asks to wait {} s before the next request: {message}", wait.as_secs().max(1))
            }
            LlmError::RateLimited { provider, retry_after: None, message } => {
                write!(f, "{provider} asks to wait before the next request: {message}")
            }
            LlmError::Overloaded { provider, message } => write!(f, "{provider} is overloaded; try again later: {message}"),
            LlmError::ServerError { provider, status, message } => write!(f, "{provider} had an error of its own ({status}): {message}"),
            LlmError::BadRequest { provider, status, message } => write!(f, "{provider} did not accept the request ({status}): {message}"),
            LlmError::Refused { provider, model, category, explanation } => {
                write!(f, "{model} at {provider} declined to answer")?;
                if let Some(category) = category {
                    write!(f, " ({category})")?;
                }
                if let Some(explanation) = explanation {
                    write!(f, ": {explanation}")?;
                }
                Ok(())
            }
            LlmError::TimedOut { provider } => write!(f, "{provider} did not answer in time"),
            LlmError::Network { provider, message } => write!(f, "{provider} could not be reached: {message}"),
            LlmError::Unexpected { provider, message } => write!(f, "{provider} sent an answer that could not be read: {message}"),
            LlmError::CloudNotAllowed { feature, provider } => write!(
                f,
                "{} is set to stay on your devices, and {provider} is a cloud service; \
                 allow cloud use for it in the settings, or choose a local model",
                feature.name()
            ),
            LlmError::UnknownProvider { name } => write!(f, "there is no provider called {name}"),
            LlmError::InvalidInput { message } => write!(f, "{message}"),
            LlmError::Storage { message } => write!(f, "{message}"),
            LlmError::Cancelled => write!(f, "stopped"),
        }
    }
}

impl std::error::Error for LlmError {}

/// A provider's message with anything that looks like a key taken out.
///
/// The key the request was sent with is removed wherever it appears. So are
/// the shapes keys have, `sk-…` (Anthropic, OpenAI) and `AIza…` (Google),
/// even shortened or partly masked: OpenAI's message for a wrong key quotes
/// the key's last characters.
pub(crate) fn redact(text: &str, key: Option<&ApiKey>) -> String {
    let mut text = text.to_string();
    if let Some(key) = key {
        if !key.expose().is_empty() {
            text = text.replace(key.expose(), "[key]");
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = next_key_shape(rest) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let end = tail.find(|c: char| !(c.is_ascii_alphanumeric() || "-_*.".contains(c))).unwrap_or(tail.len());
        out.push_str("[key]");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Where the next run that looks like a key begins: `sk-` or `AIza` followed
/// by at least eight more key characters.
fn next_key_shape(text: &str) -> Option<usize> {
    let mut from = 0;
    while from < text.len() {
        let found = ["sk-", "AIza"].iter().filter_map(|prefix| text[from..].find(prefix).map(|at| (from + at, prefix.len()))).min();
        let (at, prefix) = found?;
        let after = &text[at + prefix..];
        let run = after.chars().take_while(|c| c.is_ascii_alphanumeric() || "-_*.".contains(*c)).count();
        if run >= 8 {
            return Some(at);
        }
        from = at + prefix;
    }
    None
}

/// Keeps a provider's message short enough to show.
pub(crate) fn shorten(text: &str, most: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= most {
        return text.to_string();
    }
    let cut: String = text.chars().take(most).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_taken_out_of_messages() {
        let key = ApiKey::new("sk-ant-test-0000-not-a-real-key").unwrap();
        let message = "invalid x-api-key sk-ant-test-0000-not-a-real-key given";
        assert_eq!(redact(message, Some(&key)), "invalid x-api-key [key] given");
        // OpenAI quotes the key partly masked.
        let masked = "Incorrect API key provided: sk-proj-**********************abcd. You can find your API key at https://example.com.";
        let cleaned = redact(masked, None);
        assert!(!cleaned.contains("abcd"), "{cleaned}");
        assert!(cleaned.starts_with("Incorrect API key provided: [key]"), "{cleaned}");
        // Google's keys start with AIza.
        assert_eq!(redact("key AIzaSyNotARealKey_123456 expired", None), "key [key] expired");
        // Short runs are ordinary words.
        assert_eq!(redact("ask-me about sk-1 or AIza", None), "ask-me about sk-1 or AIza");
    }

    #[test]
    fn long_messages_are_shortened() {
        assert_eq!(shorten("  short  ", 10), "short");
        assert_eq!(shorten("abcdefghij-klm", 10), "abcdefghij…");
        assert_eq!(shorten("tiếng việt có dấu", 6), "tiếng …");
    }
}
