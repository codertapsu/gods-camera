//! One provider: who it is, where it is, its key, and the calls it answers.

use std::net::IpAddr;

use crate::http::{Http, HttpOptions, Who};
use crate::providers::{anthropic, compatible, gemini, openai};
use crate::{ApiKey, BatchInfo, BatchItem, BatchResult, GeneratedImage, ImageRequest, LlmError, ModelInfo, Reply, Request};

pub const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1";
pub const OPENAI_URL: &str = "https://api.openai.com/v1";
pub const GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
/// Ollama's OpenAI-compatible interface, on the computer it runs on.
pub const OLLAMA_URL: &str = "http://127.0.0.1:11434/v1";
pub const LM_STUDIO_URL: &str = "http://127.0.0.1:1234/v1";
/// llama.cpp's own server, `llama-server`.
pub const LLAMA_CPP_URL: &str = "http://127.0.0.1:8080/v1";

/// The plan's default model for Claude (section 6.10).
pub const ANTHROPIC_MODEL: &str = "claude-opus-5-5";
/// OpenAI's own advice on 2026-11-14 when unsure: its most capable model.
pub const OPENAI_MODEL: &str = "gpt-6-astra";
/// OpenAI's most capable picture model on 2026-11-14.
pub const OPENAI_IMAGE_MODEL: &str = "gpt-image-2.6-sunburst";
/// Google's strongest Gemini on 2026-11-14.
pub const GEMINI_MODEL: &str = "gemini-4-carbon";
/// Nano Banana Pro, Google's picture model for the most detailed work.
pub const GEMINI_IMAGE_MODEL: &str = "gemini-3-pro-image";

/// A cloud model's answer allowed more tokens than this is streamed
/// underneath, even when the caller waits for the whole of it. At the
/// providers' pace such an answer can take more than ten minutes, a
/// connection that stays silent that long can be dropped on the way, and
/// Gemini 4 Carbon and Argon cap an answer that is not streamed at 32K
/// tokens. Anthropic's SDKs draw the line at the same place. Local servers
/// are asked as the caller asks.
const LONGEST_UNSTREAMED: u32 = 21_333;

/// Which interface a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Gemini,
    /// A server that speaks OpenAI's Chat Completions interface: Ollama, LM
    /// Studio, llama.cpp's server, and many others.
    Compatible,
}

/// A provider the gateway can call.
///
/// The three cloud providers have fixed names, `anthropic`, `openai` and
/// `gemini`, which the meter and the price table use. A compatible server is
/// named by the app (`ollama`, `lm-studio`).
pub struct Provider {
    name: String,
    kind: ProviderKind,
    base: String,
    key: Option<ApiKey>,
    pub(crate) http: Http,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("base", &self.base)
            .field("key", &self.key.as_ref().map(|_| "given"))
            .finish()
    }
}

impl Provider {
    pub fn anthropic(key: Option<ApiKey>) -> Provider {
        Provider::make("anthropic", ProviderKind::Anthropic, ANTHROPIC_URL, key)
    }

    pub fn openai(key: Option<ApiKey>) -> Provider {
        Provider::make("openai", ProviderKind::OpenAi, OPENAI_URL, key)
    }

    pub fn gemini(key: Option<ApiKey>) -> Provider {
        Provider::make("gemini", ProviderKind::Gemini, GEMINI_URL, key)
    }

    /// A server with OpenAI's Chat Completions interface at `base_url` (the
    /// part before `/chat/completions`, usually ending in `/v1`). Most local
    /// servers need no key.
    pub fn compatible(name: &str, base_url: &str, key: Option<ApiKey>) -> Result<Provider, LlmError> {
        let name = name.trim();
        if name.is_empty() || ["anthropic", "openai", "gemini"].contains(&name) {
            return Err(LlmError::InvalidInput { message: format!("“{name}” cannot name a compatible server") });
        }
        Provider::make(name, ProviderKind::Compatible, OLLAMA_URL, key).with_base_url(base_url)
    }

    /// Ollama on this computer.
    pub fn ollama() -> Provider {
        Provider::make("ollama", ProviderKind::Compatible, OLLAMA_URL, None)
    }

    fn make(name: &str, kind: ProviderKind, base: &str, key: Option<ApiKey>) -> Provider {
        Provider { name: name.to_string(), kind, base: base.to_string(), key, http: Http::new(HttpOptions::default()) }
    }

    /// The same provider at another address: a proxy, a server on another
    /// computer, or a test's stand-in.
    pub fn with_base_url(mut self, base_url: &str) -> Result<Provider, LlmError> {
        let base = base_url.trim().trim_end_matches('/');
        if !(base.starts_with("http://") || base.starts_with("https://")) || host(base).is_none() {
            return Err(LlmError::InvalidInput { message: format!("“{base_url}” is not an http or https address") });
        }
        self.base = base.to_string();
        Ok(self)
    }

    pub fn with_http(mut self, options: HttpOptions) -> Provider {
        self.http = Http::new(options);
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> ProviderKind {
        self.kind
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// Whether data sent to this provider stays on the user's own computers.
    ///
    /// The three cloud providers never do, whatever their address. A
    /// compatible server does when its address is this computer or one on
    /// the home network: a loopback or private address, a link-local one, a
    /// `.local` or `.lan` name, or a name without dots. Any other name could
    /// be anywhere, so it counts as a cloud service.
    pub fn is_local(&self) -> bool {
        self.kind == ProviderKind::Compatible && host(&self.base).is_some_and(|host| is_local_host(&host))
    }

    /// The model to use when the user has not chosen one.
    pub fn default_model(&self) -> Option<&'static str> {
        match self.kind {
            ProviderKind::Anthropic => Some(ANTHROPIC_MODEL),
            ProviderKind::OpenAi => Some(OPENAI_MODEL),
            ProviderKind::Gemini => Some(GEMINI_MODEL),
            ProviderKind::Compatible => None,
        }
    }

    /// The picture model to use when the user has not chosen one.
    pub fn default_image_model(&self) -> Option<&'static str> {
        match self.kind {
            ProviderKind::OpenAi => Some(OPENAI_IMAGE_MODEL),
            ProviderKind::Gemini => Some(GEMINI_IMAGE_MODEL),
            ProviderKind::Anthropic | ProviderKind::Compatible => None,
        }
    }

    pub(crate) fn who(&self) -> Who<'_> {
        Who { name: &self.name, kind: self.kind, key: self.key.as_ref() }
    }

    /// The key, for a provider that cannot work without one.
    pub(crate) fn required_key(&self) -> Result<&ApiKey, LlmError> {
        self.key.as_ref().ok_or_else(|| LlmError::NoKey { provider: self.name.clone() })
    }

    pub(crate) fn key(&self) -> Option<&ApiKey> {
        self.key.as_ref()
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Asks the model and waits for the whole answer.
    pub async fn chat(&self, request: &Request) -> Result<Reply, LlmError> {
        self.chat_with(request, None).await
    }

    /// Asks the model and hands each piece of the answer's text to `on_text`
    /// as it arrives; returns the same reply as [`Provider::chat`] at the end.
    pub async fn chat_streaming(&self, request: &Request, on_text: &mut (dyn FnMut(&str) + Send)) -> Result<Reply, LlmError> {
        self.chat_with(request, Some(on_text)).await
    }

    pub(crate) async fn chat_with(&self, request: &Request, on_text: Option<&mut (dyn FnMut(&str) + Send)>) -> Result<Reply, LlmError> {
        request.check()?;
        let mut unheard = |_: &str| {};
        let reply = match on_text {
            None if self.kind != ProviderKind::Compatible && request.max_tokens > LONGEST_UNSTREAMED => {
                self.ask(request, Some(&mut unheard)).await
            }
            on_text => self.ask(request, on_text).await,
        };
        reply.map_err(|error| error.for_model(&request.model))
    }

    async fn ask(&self, request: &Request, on_text: Option<&mut (dyn FnMut(&str) + Send)>) -> Result<Reply, LlmError> {
        match self.kind {
            ProviderKind::Anthropic => anthropic::chat(self, request, on_text).await,
            ProviderKind::OpenAi => openai::chat(self, request, on_text).await,
            ProviderKind::Gemini => gemini::chat(self, request, on_text).await,
            ProviderKind::Compatible => compatible::chat(self, request, on_text).await,
        }
    }

    /// The models the provider offers this key, for the model picker.
    pub async fn models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        match self.kind {
            ProviderKind::Anthropic => anthropic::models(self).await,
            ProviderKind::OpenAi => openai::models(self).await,
            ProviderKind::Gemini => gemini::models(self).await,
            ProviderKind::Compatible => compatible::models(self).await,
        }
    }

    /// Makes a picture, or one from the pictures given. OpenAI and Gemini make
    /// pictures; Anthropic's models and the local servers do not.
    pub async fn generate_image(&self, request: &ImageRequest) -> Result<GeneratedImage, LlmError> {
        if request.prompt.trim().is_empty() || request.model.trim().is_empty() {
            return Err(LlmError::InvalidInput { message: "a picture needs a model and a description".into() });
        }
        let image = match self.kind {
            ProviderKind::OpenAi => openai::generate_image(self, request).await,
            ProviderKind::Gemini => gemini::generate_image(self, request).await,
            ProviderKind::Anthropic | ProviderKind::Compatible => {
                Err(LlmError::InvalidInput { message: format!("{} does not make pictures", self.name) })
            }
        };
        image.map_err(|error| error.for_model(&request.model))
    }

    /// Sends many requests at once as an Anthropic message batch, at half
    /// the price; results come back within a day. Poll with
    /// [`Provider::batch`] and fetch with [`Provider::batch_results`].
    pub async fn submit_batch(&self, items: &[BatchItem]) -> Result<BatchInfo, LlmError> {
        self.only_anthropic("message batches")?;
        anthropic::submit_batch(self, items).await
    }

    pub async fn batch(&self, id: &str) -> Result<BatchInfo, LlmError> {
        self.only_anthropic("message batches")?;
        anthropic::batch(self, id).await
    }

    /// The results of an ended batch, under the ids the requests were sent with.
    pub async fn batch_results(&self, id: &str) -> Result<Vec<BatchResult>, LlmError> {
        self.only_anthropic("message batches")?;
        anthropic::batch_results(self, id).await
    }

    fn only_anthropic(&self, what: &str) -> Result<(), LlmError> {
        match self.kind {
            ProviderKind::Anthropic => Ok(()),
            _ => Err(LlmError::InvalidInput { message: format!("{what} are only offered by Anthropic") }),
        }
    }
}

/// The host part of an http or https address, without brackets around an IPv6 address.
fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split_once(']')?.0
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn is_local_host(host: &str) -> bool {
    if let Ok(address) = host.parse::<IpAddr>() {
        return match address {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            IpAddr::V6(v6) => {
                let first = v6.segments()[0];
                v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
            }
        };
    }
    host == "localhost" || [".localhost", ".local", ".lan", ".home.arpa"].iter().any(|ending| host.ends_with(ending)) || !host.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(url: &str) -> bool {
        Provider::compatible("server", url, None).unwrap().is_local()
    }

    #[test]
    fn local_servers_are_told_from_cloud_services() {
        assert!(local("http://127.0.0.1:11434/v1"));
        assert!(local("http://localhost:1234/v1"));
        assert!(local("http://[::1]:8080/v1"));
        assert!(local("http://192.168.1.20:11434/v1"));
        assert!(local("http://10.0.0.5/v1"));
        assert!(local("http://172.20.0.2/v1"));
        assert!(local("http://studio.local:1234/v1"));
        assert!(local("http://desktop-pc:11434/v1"));
        assert!(local("http://[fd12:3456::1]:8080/v1"));
        assert!(!local("https://api.example.com/v1"));
        assert!(!local("https://8.8.8.8/v1"));
        assert!(!local("https://172.32.0.1/v1"));
        assert!(!local("https://ollama.com/v1"));
        // A cloud provider is a cloud provider wherever its address points.
        let stand_in = Provider::anthropic(None).with_base_url("http://127.0.0.1:9/v1").unwrap();
        assert!(!stand_in.is_local());
    }

    #[test]
    fn addresses_are_checked() {
        assert!(Provider::compatible("server", "ftp://example.com", None).is_err());
        assert!(Provider::compatible("server", "http://", None).is_err());
        assert!(Provider::compatible("anthropic", OLLAMA_URL, None).is_err());
        let provider = Provider::compatible("lm-studio", "http://127.0.0.1:1234/v1/", None).unwrap();
        assert_eq!(provider.url("/models"), "http://127.0.0.1:1234/v1/models");
    }

    #[test]
    fn a_debug_print_shows_no_key() {
        let key = ApiKey::new("sk-test-not-a-real-key-0123456789").unwrap();
        let printed = format!("{:?}", Provider::openai(Some(key)));
        assert!(!printed.contains("0123456789"), "{printed}");
        assert!(printed.contains("given"), "{printed}");
    }
}
