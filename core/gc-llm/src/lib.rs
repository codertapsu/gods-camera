//! The provider gateway: one way for the apps to reach language and image
//! models, whether they run in the cloud (Anthropic, OpenAI, Google Gemini) or
//! on a computer of the user's own (Ollama, LM Studio, llama.cpp's server,
//! through the OpenAI-compatible interface they all offer).
//!
//! The parts:
//!
//! - [`Provider`] speaks one provider's own HTTP interface: a chat with text
//!   and pictures, streamed or not, an answer shaped by a JSON schema, the
//!   provider's list of models, and pictures made or edited. Anthropic's
//!   message batches are there too, for large scan jobs. Every provider is
//!   spoken to in plain HTTPS, without an SDK.
//! - [`Gateway`] is what the apps hold. It keeps the providers, refuses a
//!   cloud provider for a feature that is set to stay on the user's devices
//!   ([`Feature`], section 6.11 of the plan), counts what every call used
//!   ([`Meter`]), and runs the document workflows: clean-up, summary,
//!   translation, extraction and questions. Its calls block; the apps make
//!   them from a background thread.
//!
//! Nothing here stores a key. The app reads it from the Keychain or from
//! Credential Manager and hands it in as an [`ApiKey`], which never shows
//! itself in a Debug print or an error.
//!
//! Every text that comes back is in Unicode's composed form, NFC, so that a
//! Vietnamese letter with a tone mark is one character whichever provider
//! wrote it, and compares equal to the same letter read from a scan.

pub mod diff;
mod document;
mod error;
mod features;
mod gateway;
mod http;
mod key;
mod meter;
mod provider;
mod providers;
mod sse;
pub mod text;

use std::fmt;

pub use diff::{Change, ChangeKind, WordDiff};
pub use document::{Answer, CleanUp, DocOptions, Language, Progress, Target};
pub use error::LlmError;
pub use features::{CloudSwitches, Feature};
pub use gateway::{Gateway, ProviderInfo};
pub use http::HttpOptions;
pub use key::ApiKey;
pub use meter::{Meter, Price, PriceTable, UsageLine, UsageReport};
pub use provider::{
    Provider, ProviderKind, ANTHROPIC_MODEL, ANTHROPIC_URL, GEMINI_IMAGE_MODEL, GEMINI_MODEL, GEMINI_URL, LLAMA_CPP_URL,
    LM_STUDIO_URL, OLLAMA_URL, OPENAI_IMAGE_MODEL, OPENAI_MODEL, OPENAI_URL,
};

/// Who speaks in a turn of the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// The two kinds of picture a request can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureKind {
    Png,
    Jpeg,
}

/// A picture to send with a message: a photo of a page, of a person or of a
/// garment. Only PNG and JPEG are taken, since every provider reads both; the
/// kind is told by the bytes themselves, not by a file name.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    bytes: Vec<u8>,
    kind: PictureKind,
}

impl Picture {
    pub fn new(bytes: Vec<u8>) -> Result<Picture, LlmError> {
        let kind = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            PictureKind::Png
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            PictureKind::Jpeg
        } else {
            return Err(LlmError::InvalidInput { message: "a picture must be a PNG or a JPEG file".into() });
        };
        Ok(Picture { bytes, kind })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn kind(&self) -> PictureKind {
        self.kind
    }

    pub fn mime(&self) -> &'static str {
        match self.kind {
            PictureKind::Png => "image/png",
            PictureKind::Jpeg => "image/jpeg",
        }
    }

    /// The picture as a `data:` URL, the way OpenAI and the local servers take it.
    pub(crate) fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime(), self.base64())
    }

    pub(crate) fn base64(&self) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }
}

/// A Debug print gives the kind and the size, not a screenful of bytes.
impl fmt::Debug for Picture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Picture({:?}, {} bytes)", self.kind, self.bytes.len())
    }
}

/// One piece of a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Picture(Picture),
}

/// One turn of the conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Message {
        Message { role: Role::User, parts: vec![Part::Text(text.into())] }
    }

    /// A question about pictures: the pictures first, then the words, which
    /// is the order the providers recommend.
    pub fn user_with_pictures(text: impl Into<String>, pictures: Vec<Picture>) -> Message {
        let mut parts: Vec<Part> = pictures.into_iter().map(Part::Picture).collect();
        parts.push(Part::Text(text.into()));
        Message { role: Role::User, parts }
    }

    /// An earlier answer, given back so the conversation can go on.
    pub fn assistant(text: impl Into<String>) -> Message {
        Message { role: Role::Assistant, parts: vec![Part::Text(text.into())] }
    }

    /// The message's words, without its pictures.
    pub fn text(&self) -> String {
        let words: Vec<&str> = self
            .parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(text.as_str()),
                Part::Picture(_) => None,
            })
            .collect();
        words.join("\n\n")
    }
}

/// What the app asks a model, the same for every provider.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The model's id as the provider names it, for example `claude-opus-5-5`.
    pub model: String,
    /// Instructions that hold for the whole conversation.
    pub system: Option<String>,
    /// The instructions are sent again and again unchanged, so the provider
    /// may keep them in its prompt cache. Anthropic is told where the cache
    /// ends; OpenAI is given a cache key; Gemini and the local servers cache
    /// on their own.
    pub cache_system: bool,
    pub messages: Vec<Message>,
    /// The most the answer may use, thinking included where the model thinks.
    pub max_tokens: u32,
    /// Used only by the local servers. Anthropic's API version 2026-10-21
    /// refuses it, OpenAI's reasoning models refuse it, and Gemini's newest
    /// models ignore it, so it is not sent to them.
    pub temperature: Option<f32>,
    /// A JSON schema the answer must follow. The answer then also comes back
    /// parsed, in [`Reply::json`]. Write it with `"additionalProperties": false`
    /// and every property listed in `required`: OpenAI's strict mode and
    /// Anthropic's structured outputs both ask for that.
    pub schema: Option<serde_json::Value>,
    /// When Anthropic's safety classifiers decline a request, let Anthropic run
    /// it on the model it recommends for that kind of refusal (its server-side
    /// fallback), and say so in [`Reply::fallback_from`]. On by default; only
    /// Anthropic's newest models take it, and batches never do.
    pub allow_fallback: bool,
}

impl Request {
    /// A request with one message from the user and room for a long answer.
    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> Request {
        Request {
            model: model.into(),
            system: None,
            cache_system: false,
            messages: vec![Message::user(prompt)],
            max_tokens: 16_000,
            temperature: None,
            schema: None,
            allow_fallback: true,
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Request {
        self.system = Some(system.into());
        self
    }

    /// The system prompt is sent unchanged with many requests: let the provider cache it.
    pub fn cached(mut self) -> Request {
        self.cache_system = true;
        self
    }

    pub fn with_schema(mut self, schema: serde_json::Value) -> Request {
        self.schema = Some(schema);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Request {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Request {
        self.temperature = Some(temperature);
        self
    }

    /// Checks what every provider needs, before anything is sent.
    pub(crate) fn check(&self) -> Result<(), LlmError> {
        let problem = if self.model.trim().is_empty() {
            Some("no model was chosen")
        } else if self.messages.is_empty() {
            Some("the request has no message")
        } else if self.max_tokens == 0 {
            Some("the answer must be allowed at least one token")
        } else if self.messages.iter().any(|message| message.parts.is_empty()) {
            Some("a message is empty")
        } else if self
            .messages
            .iter()
            .any(|message| message.role == Role::Assistant && message.parts.iter().any(|part| matches!(part, Part::Picture(_))))
        {
            Some("pictures can only be sent in the user's messages")
        } else {
            None
        };
        match problem {
            Some(message) => Err(LlmError::InvalidInput { message: message.into() }),
            None => Ok(()),
        }
    }
}

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// It finished its answer.
    End,
    /// It reached the most it was allowed to write; the answer is cut off.
    Length,
    /// It declined, or the provider's safety filter stopped it. Any text that
    /// came before is a fragment and must not be taken as an answer. The
    /// category is the provider's (`cyber`, `bio`, `safety`, ...), when it
    /// gives one.
    Refusal { category: Option<String>, explanation: Option<String> },
    /// Something else, in the provider's own words.
    Other(String),
}

/// Tokens a call used, as the provider counted them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    /// Input read at the full price.
    pub input: u64,
    /// Input read from the provider's prompt cache, at its lower price.
    pub cached: u64,
    /// Input written into the prompt cache, which some providers charge more for.
    pub cache_written: u64,
    /// Output, the model's thinking included.
    pub output: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.cached += other.cached;
        self.cache_written += other.cache_written;
        self.output += other.output;
    }

    pub fn total(&self) -> u64 {
        self.input + self.cached + self.cache_written + self.output
    }
}

/// A model's answer, the same for every provider.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// The answer's text, in NFC.
    pub text: String,
    /// The answer parsed, when the request gave a schema and the answer is
    /// JSON. Its strings are in NFC too.
    pub json: Option<serde_json::Value>,
    pub stop: StopReason,
    pub usage: Usage,
    /// The provider's name in the gateway: `anthropic`, `openai`, `gemini`, or
    /// the name a local server was given.
    pub provider: String,
    /// The model that wrote the answer, as the provider named it. With a
    /// fallback, this is the model that took over.
    pub model: String,
    /// The model that declined before another took over (Anthropic's
    /// server-side fallback), if that happened.
    pub fallback_from: Option<String>,
}

/// What a model is for, as far as its listing tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// Writes text: chat, documents, pictures read.
    Text,
    /// Makes or edits pictures.
    Image,
    /// Something else: speech, embeddings, video.
    Other,
}

/// One entry in a provider's list of models, for the model picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// What a request names it by.
    pub id: String,
    /// A name to show, when the provider gives one; otherwise the id.
    pub name: String,
    pub kind: ModelKind,
    /// How much input it takes, when the provider says.
    pub context_tokens: Option<u64>,
    /// How long its answers may be, when the provider says.
    pub output_tokens: Option<u64>,
}

/// A picture to make, or to make from other pictures: a person photo and a
/// garment photo in, the person wearing it out.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRequest {
    pub model: String,
    pub prompt: String,
    /// Pictures to start from. Empty to make a picture from the words alone.
    pub pictures: Vec<Picture>,
}

/// A picture a model made.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedImage {
    /// The picture as a PNG file.
    pub png: Vec<u8>,
    /// Any words the model wrote beside the picture.
    pub text: String,
    pub usage: Usage,
    pub provider: String,
    pub model: String,
}

/// One request in a message batch, under an id of the caller's choosing.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchItem {
    /// 1 to 64 letters, digits, `-` or `_`. The results come back under it,
    /// in any order.
    pub id: String,
    pub request: Request,
}

/// How far a batch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchState {
    InProgress,
    Canceling,
    /// Every request has a result; fetch them.
    Ended,
}

/// A batch as the provider reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchInfo {
    pub id: String,
    pub state: BatchState,
    pub processing: u64,
    pub succeeded: u64,
    pub errored: u64,
    pub canceled: u64,
    pub expired: u64,
}

/// What became of one request in a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchOutcome {
    /// The answer. Its stop reason can still be a refusal; check it.
    Succeeded(Reply),
    /// The request failed, in the provider's words.
    Errored(String),
    Canceled,
    /// The batch ran out of time before this request was done.
    Expired,
}

/// The result for one request, under the id it was sent with.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchResult {
    pub id: String,
    pub outcome: BatchOutcome,
}
