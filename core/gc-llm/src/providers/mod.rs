//! Each provider's own interface, in plain HTTP, behind the one [`Provider`].
//!
//! [`Provider`]: crate::Provider

pub(crate) mod anthropic;
pub(crate) mod compatible;
pub(crate) mod gemini;
pub(crate) mod openai;

use serde_json::Value;

use crate::http::Who;
use crate::text::{nfc, nfc_json, parse_json_loosely};
use crate::{LlmError, Provider, Reply, Request, StopReason, Usage};

/// Where the pieces of a streamed answer go.
pub(crate) type OnText<'a> = Option<&'a mut (dyn FnMut(&str) + Send)>;

pub(crate) fn tell(on_text: &mut OnText<'_>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(on_text) = on_text {
        on_text(text);
    }
}

/// A count in an answer; a count that is missing or not a number is 0.
pub(crate) fn count(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

/// The reply every provider's answer is turned into: its text in NFC and,
/// when the request asked for a schema, the JSON in it.
pub(crate) fn reply(provider: &Provider, request: &Request, text: &str, stop: StopReason, usage: Usage, model: Option<&str>) -> Reply {
    reply_for(provider, &request.model, request.schema.is_some(), text, stop, usage, model)
}

/// The same, for an answer whose request is not at hand: a batch result.
pub(crate) fn reply_for(
    provider: &Provider,
    asked: &str,
    wants_json: bool,
    text: &str,
    stop: StopReason,
    usage: Usage,
    model: Option<&str>,
) -> Reply {
    let text = nfc(text);
    let json = if wants_json { parse_json_loosely(&text).map(nfc_json) } else { None };
    let model = model.filter(|model| !model.is_empty()).unwrap_or(asked).to_string();
    Reply { text, json, stop, usage, provider: provider.name().to_string(), model, fallback_from: None }
}

/// A JSON body as bytes, for a request that may be sent more than once.
pub(crate) fn to_bytes(body: &Value) -> Vec<u8> {
    serde_json::to_vec(body).unwrap_or_default()
}

/// A picture a provider made, as a PNG file. One that came as JPEG or WebP
/// is decoded and written again as PNG, so the apps get one format.
pub(crate) fn as_png(who: &Who<'_>, picture: Vec<u8>) -> Result<Vec<u8>, LlmError> {
    if picture.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(picture);
    }
    let decoded = image::load_from_memory(&picture).map_err(|error| who.unexpected(format!("the picture could not be read ({error})")))?;
    let mut png = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| who.unexpected(format!("the picture could not be written as PNG ({error})")))?;
    Ok(png)
}
