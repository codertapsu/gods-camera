//! Sending a request: the HTTPS client, time limits, retries, and turning the
//! provider's error answers into [`LlmError`].

use std::sync::OnceLock;
use std::time::Duration;

use crate::error::{redact, shorten};
use crate::sse;
use crate::{ApiKey, LlmError, ProviderKind};

/// Time limits and retries for one provider's requests.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpOptions {
    /// How long to wait for a connection.
    pub connect_timeout: Duration,
    /// How long a streamed answer may stay silent, between pieces and before
    /// the first. A model that thinks first can be silent for a while.
    pub idle_timeout: Duration,
    /// How long an answer that is not streamed may take in all, retries
    /// included. A long answer with thinking can take many minutes.
    pub answer_timeout: Duration,
    /// How many times a request is sent again after a rate limit, an
    /// overloaded or failing server, or a connection that could not be made.
    pub retries: u32,
    /// The wait before the first retry when the provider names none. It
    /// doubles with each retry.
    pub first_wait: Duration,
    /// The longest wait for a retry. A provider that asks for longer is not
    /// waited for: its error goes back with the wait it asked for, and the
    /// app can tell the user when to try again.
    pub longest_wait: Duration,
}

impl Default for HttpOptions {
    fn default() -> Self {
        HttpOptions {
            connect_timeout: Duration::from_secs(20),
            idle_timeout: Duration::from_secs(300),
            answer_timeout: Duration::from_secs(900),
            retries: 3,
            first_wait: Duration::from_secs(1),
            longest_wait: Duration::from_secs(60),
        }
    }
}

/// Who a request is for: the provider's name for messages, its kind for the
/// shape of its errors, and its key, to take out of anything it says.
#[derive(Clone, Copy)]
pub(crate) struct Who<'a> {
    pub name: &'a str,
    pub kind: ProviderKind,
    pub key: Option<&'a ApiKey>,
}

impl Who<'_> {
    pub fn unexpected(&self, message: impl AsRef<str>) -> LlmError {
        LlmError::Unexpected { provider: self.name.to_string(), message: redact(message.as_ref(), self.key) }
    }

    pub fn network(&self, error: &reqwest::Error) -> LlmError {
        if error.is_timeout() {
            return LlmError::TimedOut { provider: self.name.to_string() };
        }
        // The error's own words name the URL; the cause is further down.
        let mut message = error.to_string();
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            message = format!("{message}: {cause}");
            source = cause.source();
        }
        LlmError::Network { provider: self.name.to_string(), message: redact(&shorten(&message, 400), self.key) }
    }

    pub fn timed_out(&self) -> LlmError {
        LlmError::TimedOut { provider: self.name.to_string() }
    }
}

/// Builds the request for one attempt. Called again for each retry, since a
/// request cannot be sent twice.
pub(crate) type Make<'a> = &'a (dyn Fn(&reqwest::Client) -> reqwest::RequestBuilder + Send + Sync);

pub(crate) struct Http {
    pub options: HttpOptions,
    client: OnceLock<Result<reqwest::Client, String>>,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http").field("options", &self.options).finish_non_exhaustive()
    }
}

impl Http {
    pub fn new(options: HttpOptions) -> Http {
        Http { options, client: OnceLock::new() }
    }

    /// The client, made on first use. HTTPS goes through rustls with the
    /// `ring` library, as everywhere in the core, and the system's own list
    /// of trusted certificates (the Keychain's on a Mac, Windows' store on the PC).
    fn client(&self, who: &Who<'_>) -> Result<&reqwest::Client, LlmError> {
        let made = self.client.get_or_init(|| {
            install_crypto_provider();
            reqwest::Client::builder()
                .connect_timeout(self.options.connect_timeout)
                .user_agent(concat!("gc-llm/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|error| error.to_string())
        });
        let provider = who.name.to_string();
        made.as_ref().map_err(|error| LlmError::Network { provider, message: format!("HTTPS could not be set up: {error}") })
    }

    /// Sends the request until it succeeds or may not be sent again, and
    /// gives back the successful response.
    pub async fn send(&self, who: &Who<'_>, make: Make<'_>) -> Result<reqwest::Response, LlmError> {
        let client = self.client(who)?;
        let mut attempt = 0;
        loop {
            let (error, retry, asked) = match make(client).send().await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => {
                    let status = response.status().as_u16();
                    let asked = retry_after(response.headers());
                    let body = response.bytes().await.unwrap_or_default();
                    let retry = retryable(status, &body);
                    (status_error(who, status, &body, asked), retry, asked)
                }
                // A connection that could not be made sent nothing: safe to
                // try again. A connection that broke later may have started
                // the work, so that is left to the caller.
                Err(error) if error.is_connect() && !error.is_timeout() => (who.network(&error), true, None),
                Err(error) => return Err(who.network(&error)),
            };
            if !retry || attempt >= self.options.retries {
                return Err(error);
            }
            let wait = asked.unwrap_or_else(|| self.options.first_wait.saturating_mul(1 << attempt.min(16)));
            if wait > self.options.longest_wait {
                return Err(error);
            }
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    /// Sends the request and reads the whole answer as JSON, within the answer time limit.
    pub async fn json(&self, who: &Who<'_>, make: Make<'_>) -> Result<serde_json::Value, LlmError> {
        let body = self.bytes(who, make).await?;
        serde_json::from_slice(&body).map_err(|error| who.unexpected(format!("the answer is not JSON ({error})")))
    }

    /// Sends the request and reads the whole answer, within the answer time limit.
    pub async fn bytes(&self, who: &Who<'_>, make: Make<'_>) -> Result<Vec<u8>, LlmError> {
        let work = async {
            let response = self.send(who, make).await?;
            response.bytes().await.map(|bytes| bytes.to_vec()).map_err(|error| who.network(&error))
        };
        tokio::time::timeout(self.options.answer_timeout, work).await.unwrap_or_else(|_| Err(who.timed_out()))
    }

    /// Sends the request and reads the answer as server-sent events as they come.
    pub async fn events(&self, who: &Who<'_>, make: Make<'_>) -> Result<Events, LlmError> {
        let sent = tokio::time::timeout(self.options.idle_timeout, self.send(who, make)).await;
        let response = sent.unwrap_or_else(|_| Err(who.timed_out()))?;
        Ok(Events { response, parser: sse::Parser::default(), idle: self.options.idle_timeout, ended: false })
    }
}

/// A streamed answer, read event by event.
pub(crate) struct Events {
    response: reqwest::Response,
    parser: sse::Parser,
    idle: Duration,
    ended: bool,
}

impl Events {
    /// The next event, or `None` once the stream has ended.
    pub async fn next(&mut self, who: &Who<'_>) -> Result<Option<sse::Event>, LlmError> {
        loop {
            if let Some(event) = self.parser.next_event() {
                return Ok(Some(event));
            }
            if self.ended {
                return Ok(None);
            }
            match tokio::time::timeout(self.idle, self.response.chunk()).await {
                Err(_) => return Err(who.timed_out()),
                Ok(Err(error)) => return Err(who.network(&error)),
                Ok(Ok(Some(bytes))) => self.parser.feed(&bytes),
                Ok(Ok(None)) => {
                    self.parser.finish();
                    self.ended = true;
                }
            }
        }
    }
}

/// Installs `ring` as rustls' way of doing cryptography, unless the app or
/// another part of the core installed one first. The HTTPS client is built
/// without a default of its own, so that the same library serves the hub,
/// the `gc` tool and this crate, on every system the core is built for.
pub(crate) fn install_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// The wait the provider asked for: OpenAI's `retry-after-ms`, or the
/// standard `retry-after` in seconds. A date in `retry-after` is not read;
/// the doubling wait is used instead.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let read = |name: &str| headers.get(name).and_then(|value| value.to_str().ok()).and_then(|value| value.trim().parse::<f64>().ok());
    if let Some(ms) = read("retry-after-ms").filter(|ms| ms.is_finite() && *ms >= 0.0) {
        return Some(Duration::from_secs_f64(ms / 1000.0));
    }
    read("retry-after").filter(|s| s.is_finite() && *s >= 0.0).map(Duration::from_secs_f64)
}

/// Whether an error answer may go away if the request is sent again.
/// OpenAI answers an account without credit with 429 too; that one stays.
fn retryable(status: u16, body: &[u8]) -> bool {
    match status {
        429 => !String::from_utf8_lossy(body).contains("insufficient_quota"),
        500 | 502 | 503 | 504 | 529 => true,
        _ => false,
    }
}

/// An error answer as an [`LlmError`], with the provider's own message.
pub(crate) fn status_error(who: &Who<'_>, status: u16, body: &[u8], retry_after: Option<Duration>) -> LlmError {
    let message = redact(&error_message(body), who.key);
    let provider = who.name.to_string();
    let raw = String::from_utf8_lossy(body);
    // Google answers a wrong key with 400, not 401.
    let gemini_key =
        who.kind == ProviderKind::Gemini && (raw.contains("API_KEY_INVALID") || message.to_lowercase().contains("api key not valid"));
    // OpenAI answers a picture its safety system blocks with 400.
    let openai_blocked =
        who.kind == ProviderKind::OpenAi && (raw.contains("moderation_blocked") || raw.contains("content_policy_violation"));
    match status {
        401 | 403 => LlmError::KeyRefused { provider, message },
        400 if gemini_key => LlmError::KeyRefused { provider, message },
        400 if openai_blocked => {
            LlmError::Refused { provider, model: String::new(), category: Some("safety".into()), explanation: Some(message) }
        }
        408 => LlmError::TimedOut { provider },
        429 => LlmError::RateLimited { provider, retry_after, message },
        503 | 529 => LlmError::Overloaded { provider, message },
        500..=599 => LlmError::ServerError { provider, status, message },
        _ => LlmError::BadRequest { provider, status, message },
    }
}

/// The provider's message in an error answer. Anthropic and OpenAI put it in
/// `error.message`, Google too; some local servers give `error` as a plain
/// string. Otherwise the start of the answer itself.
pub(crate) fn error_message(body: &[u8]) -> String {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        let found = value["error"]["message"]
            .as_str()
            .or_else(|| value["error"].as_str())
            .or_else(|| value["message"].as_str())
            .or_else(|| value["detail"].as_str());
        if let Some(message) = found {
            return shorten(message, 500);
        }
    }
    let text = String::from_utf8_lossy(body);
    if text.trim().is_empty() {
        "no details were given".to_string()
    } else {
        shorten(&text, 300)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(kind: ProviderKind) -> Who<'static> {
        Who { name: "test", kind, key: None }
    }

    #[test]
    fn statuses_become_the_right_errors() {
        let anthropic = who(ProviderKind::Anthropic);
        let body = br#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of request tokens has exceeded your limit"}}"#;
        match status_error(&anthropic, 429, body, Some(Duration::from_secs(7))) {
            LlmError::RateLimited { retry_after, message, .. } => {
                assert_eq!(retry_after, Some(Duration::from_secs(7)));
                assert!(message.starts_with("Number of request tokens"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(status_error(&anthropic, 529, b"{}", None), LlmError::Overloaded { .. }));
        assert!(matches!(status_error(&anthropic, 500, b"", None), LlmError::ServerError { status: 500, .. }));
        assert!(matches!(status_error(&anthropic, 401, b"", None), LlmError::KeyRefused { .. }));
        assert!(matches!(status_error(&anthropic, 404, b"", None), LlmError::BadRequest { status: 404, .. }));
        let gemini = br#"{"error":{"code":400,"message":"API key not valid.","details":[{"reason":"API_KEY_INVALID"}]}}"#;
        assert!(matches!(status_error(&who(ProviderKind::Gemini), 400, gemini, None), LlmError::KeyRefused { .. }));
        let blocked = br#"{"error":{"message":"Your request was rejected by the safety system.","code":"moderation_blocked"}}"#;
        assert!(matches!(status_error(&who(ProviderKind::OpenAi), 400, blocked, None), LlmError::Refused { .. }));
        let ollama = br#"{"error":"model 'x' not found"}"#;
        assert!(matches!(status_error(&who(ProviderKind::Compatible), 400, ollama, None), LlmError::BadRequest { .. }));
    }

    #[test]
    fn only_passing_troubles_are_retried() {
        assert!(retryable(429, br#"{"error":{"code":"rate_limit_exceeded"}}"#));
        assert!(!retryable(429, br#"{"error":{"code":"insufficient_quota"}}"#));
        assert!(retryable(529, b""));
        assert!(retryable(503, b""));
        assert!(!retryable(400, b""));
        assert!(!retryable(401, b""));
    }

    #[test]
    fn a_wait_is_read_from_either_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&headers), None);
        headers.insert("retry-after", "3".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(3)));
        headers.insert("retry-after-ms", "250".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_millis(250)));
        let mut dated = reqwest::header::HeaderMap::new();
        dated.insert("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap());
        assert_eq!(retry_after(&dated), None);
    }

    #[test]
    fn the_message_is_found_in_each_shape() {
        assert_eq!(error_message(br#"{"error":{"message":"bad schema"}}"#), "bad schema");
        assert_eq!(error_message(br#"{"error":"model not found"}"#), "model not found");
        assert_eq!(error_message(b"<html>Bad gateway</html>"), "<html>Bad gateway</html>");
        assert_eq!(error_message(b""), "no details were given");
    }
}
