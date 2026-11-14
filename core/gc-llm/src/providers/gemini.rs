//! Google Gemini: `generateContent`, streamed or not, the models list, and
//! the picture models (Nano Banana) for pictures made and edited.
//!
//! Checked against Google's reference on 2026-11-14. Google now calls
//! `generateContent` its legacy interface, beside the newer Interactions
//! API, and still supports it. The key goes in the `x-goog-api-key` header,
//! never in the URL, where an error message could repeat it. Gemini 4 and
//! 3.6 and later ignore `temperature`, so it is not sent.
//!
//! Long answers: one request returns at most 64K tokens when streamed, and
//! 32K when not streamed on Gemini 4 Carbon and Argon, which refuse a larger
//! `maxOutputTokens` there; a request allowed that much is always streamed
//! (see `LONGEST_UNSTREAMED` in the provider). A longer answer stops with the
//! finish reason `CONTINUATION` and a token; the same request sent again with
//! the token goes on where it stopped, and the pieces are joined here.

use serde_json::{json, Value};

use super::{as_png, count, reply, tell, to_bytes, OnText};
use crate::http::{status_error, Who};
use crate::{
    ApiKey, GeneratedImage, ImageRequest, LlmError, Message, ModelInfo, ModelKind, Part, Provider, Reply, Request, Role, StopReason, Usage,
};

/// The most requests one answer may take, continuations included.
const MOST_HOPS: usize = 32;

/// The model's id as a request names it: the list gives `models/…`.
fn model_id(model: &str) -> &str {
    model.trim().trim_start_matches("models/")
}

fn checked_model(model: &str) -> Result<&str, LlmError> {
    let id = model_id(model);
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)) {
        Ok(id)
    } else {
        Err(LlmError::InvalidInput { message: format!("“{model}” is not a Gemini model name") })
    }
}

pub(crate) fn body(request: &Request) -> Value {
    let contents: Vec<Value> = request.messages.iter().map(content).collect();
    let mut config = json!({ "maxOutputTokens": request.max_tokens });
    if let Some(schema) = &request.schema {
        config["responseMimeType"] = json!("application/json");
        config["responseJsonSchema"] = schema.clone();
    }
    let mut body = json!({ "contents": contents, "generationConfig": config });
    if let Some(system) = &request.system {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    body
}

fn content(message: &Message) -> Value {
    let parts: Vec<Value> = message.parts.iter().map(part).collect();
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "model",
    };
    json!({ "role": role, "parts": parts })
}

fn part(part: &Part) -> Value {
    match part {
        Part::Text(text) => json!({ "text": text }),
        Part::Picture(picture) => json!({ "inlineData": { "mimeType": picture.mime(), "data": picture.base64() } }),
    }
}

fn headers(builder: reqwest::RequestBuilder, key: &ApiKey) -> reqwest::RequestBuilder {
    builder.header("x-goog-api-key", key.expose()).header("content-type", "application/json")
}

/// What one answer, or one piece of a streamed answer, holds.
#[derive(Default)]
struct Chunk {
    text: String,
    /// Pictures, as their type and base64 data.
    pictures: Vec<(String, String)>,
    finish: Option<String>,
    /// Why the prompt itself was blocked, if it was; no answer then comes.
    blocked: Option<String>,
    continuation: Option<String>,
    usage: Option<Usage>,
    model: Option<String>,
}

fn read_chunk(who: &Who<'_>, value: &Value) -> Result<Chunk, LlmError> {
    if let Some(error) = value.get("error") {
        let status = error["code"].as_u64().and_then(|code| u16::try_from(code).ok()).unwrap_or(500);
        return Err(status_error(who, status, value.to_string().as_bytes(), None));
    }
    let mut chunk = Chunk {
        blocked: value["promptFeedback"]["blockReason"].as_str().map(str::to_string),
        usage: value.get("usageMetadata").map(usage),
        model: value["modelVersion"].as_str().map(str::to_string),
        ..Chunk::default()
    };
    let candidate = &value["candidates"][0];
    for part in candidate["content"]["parts"].as_array().into_iter().flatten() {
        // The model's thoughts, when it shows them, are not the answer.
        if part["thought"] == true {
            continue;
        }
        if let Some(text) = part["text"].as_str() {
            chunk.text.push_str(text);
        }
        let inline = &part["inlineData"];
        if let (Some(mime), Some(data)) = (inline["mimeType"].as_str(), inline["data"].as_str()) {
            chunk.pictures.push((mime.to_string(), data.to_string()));
        }
    }
    chunk.finish = candidate["finishReason"].as_str().map(str::to_string);
    chunk.continuation = candidate["continuationToken"].as_str().map(str::to_string);
    Ok(chunk)
}

/// Gemini counts the cached input inside `promptTokenCount`, and the thinking
/// apart from the answer; both thinking and answer are billed as output.
fn usage(meta: &Value) -> Usage {
    let prompt = count(&meta["promptTokenCount"]);
    let cached = count(&meta["cachedContentTokenCount"]);
    Usage {
        input: prompt.saturating_sub(cached),
        cached,
        cache_written: 0,
        output: count(&meta["candidatesTokenCount"]) + count(&meta["thoughtsTokenCount"]),
    }
}

fn stop_reason(finish: Option<&str>, blocked: Option<&str>) -> StopReason {
    if let Some(reason) = blocked {
        let explanation = Some("the request itself was blocked".to_string());
        return StopReason::Refusal { category: Some(reason.to_ascii_lowercase()), explanation };
    }
    match finish {
        Some("STOP") => StopReason::End,
        Some("MAX_TOKENS") => StopReason::Length,
        Some(
            reason @ ("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" | "IMAGE_PROHIBITED_CONTENT"
            | "IMAGE_RECITATION" | "ESCALATION" | "PUP_LIMITED_DISABLED"),
        ) => StopReason::Refusal { category: Some(reason.to_ascii_lowercase()), explanation: None },
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::Other("none given".into()),
    }
}

pub(crate) async fn chat(provider: &Provider, request: &Request, mut on_text: OnText<'_>) -> Result<Reply, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    let stream = on_text.is_some();
    let model = checked_model(&request.model)?;
    let url = if stream {
        provider.url(&format!("/models/{model}:streamGenerateContent?alt=sse"))
    } else {
        provider.url(&format!("/models/{model}:generateContent"))
    };
    let mut body = body(request);
    let mut text = String::new();
    let mut total = Usage::default();
    let mut answered_by: Option<String> = None;
    for _ in 0..MOST_HOPS {
        let bytes = to_bytes(&body);
        let make = |client: &reqwest::Client| headers(client.post(&url), key).body(bytes.clone());
        let mut last = Chunk::default();
        let mut hop_usage = Usage::default();
        if stream {
            let mut events = provider.http.events(&who, &make).await?;
            while let Some(event) = events.next(&who).await? {
                let Ok(value) = serde_json::from_str::<Value>(&event.data) else {
                    return Err(who.unexpected(format!("a piece of the answer is not JSON: {}", crate::error::shorten(&event.data, 80))));
                };
                let chunk = read_chunk(&who, &value)?;
                text.push_str(&chunk.text);
                tell(&mut on_text, &chunk.text);
                // Each piece carries the counts so far for this request.
                if let Some(usage) = chunk.usage {
                    hop_usage = usage;
                }
                merge(&mut last, chunk);
            }
        } else {
            let answer = provider.http.json(&who, &make).await?;
            let chunk = read_chunk(&who, &answer)?;
            text.push_str(&chunk.text);
            hop_usage = chunk.usage.unwrap_or_default();
            last = chunk;
        }
        total.add(&hop_usage);
        answered_by = last.model.clone().or(answered_by);
        match (last.finish.as_deref(), last.continuation.take()) {
            (Some("CONTINUATION"), Some(token)) => body["continuationToken"] = json!(token),
            _ => {
                let stop = stop_reason(last.finish.as_deref(), last.blocked.as_deref());
                return Ok(reply(provider, request, &text, stop, total, answered_by.as_deref()));
            }
        }
    }
    Err(who.unexpected(format!("the answer was still going on after {MOST_HOPS} requests")))
}

/// Keeps what a stream's pieces said last: the finish reason and the token
/// come with the last piece, a checkpoint token every few thousand tokens.
fn merge(last: &mut Chunk, chunk: Chunk) {
    if chunk.finish.is_some() {
        last.finish = chunk.finish;
    }
    if chunk.continuation.is_some() {
        last.continuation = chunk.continuation;
    }
    if chunk.blocked.is_some() {
        last.blocked = chunk.blocked;
    }
    if chunk.model.is_some() {
        last.model = chunk.model;
    }
}

/// Makes a picture: the pictures given, then the words, with pictures and
/// text allowed in the answer.
pub(crate) async fn generate_image(provider: &Provider, request: &ImageRequest) -> Result<GeneratedImage, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    let model = checked_model(&request.model)?;
    let mut parts: Vec<Value> =
        request.pictures.iter().map(|picture| json!({ "inlineData": { "mimeType": picture.mime(), "data": picture.base64() } })).collect();
    parts.push(json!({ "text": request.prompt }));
    let body = json!({
        "contents": [{ "role": "user", "parts": parts }],
        "generationConfig": { "responseModalities": ["TEXT", "IMAGE"] }
    });
    let bytes = to_bytes(&body);
    let url = provider.url(&format!("/models/{model}:generateContent"));
    let make = |client: &reqwest::Client| headers(client.post(&url), key).body(bytes.clone());
    let answer = provider.http.json(&who, &make).await?;
    let chunk = read_chunk(&who, &answer)?;
    if let StopReason::Refusal { category, explanation } = stop_reason(chunk.finish.as_deref(), chunk.blocked.as_deref()) {
        return Err(LlmError::Refused { provider: provider.name().to_string(), model: request.model.clone(), category, explanation });
    }
    let Some((_, data)) = chunk.pictures.iter().find(|(mime, _)| mime.starts_with("image/")) else {
        return Err(who.unexpected(format!("no picture came back (finish reason {})", chunk.finish.as_deref().unwrap_or("none"))));
    };
    use base64::Engine as _;
    let picture = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| who.unexpected("the picture is not valid base64"))?;
    Ok(GeneratedImage {
        png: as_png(&who, picture)?,
        text: crate::text::nfc(&chunk.text),
        usage: chunk.usage.unwrap_or_default(),
        provider: provider.name().to_string(),
        model: chunk.model.unwrap_or_else(|| request.model.clone()),
    })
}

/// The models that answer `generateContent`, one page after another.
pub(crate) async fn models(provider: &Provider) -> Result<Vec<ModelInfo>, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    let mut models = Vec::new();
    let mut page_token: Option<String> = None;
    for _ in 0..50 {
        let mut url = provider.url("/models?pageSize=1000");
        if let Some(token) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&encode_query(token));
        }
        let make = |client: &reqwest::Client| client.get(&url).header("x-goog-api-key", key.expose());
        let page = provider.http.json(&who, &make).await?;
        for entry in page["models"].as_array().into_iter().flatten() {
            let Some(name) = entry["name"].as_str() else { continue };
            let methods = entry["supportedGenerationMethods"].as_array();
            if !methods.is_some_and(|methods| methods.iter().any(|method| method == "generateContent")) {
                continue;
            }
            let id = model_id(name).to_string();
            models.push(ModelInfo {
                name: entry["displayName"].as_str().unwrap_or(&id).to_string(),
                kind: kind_of(&id),
                context_tokens: entry["inputTokenLimit"].as_u64(),
                output_tokens: entry["outputTokenLimit"].as_u64(),
                id,
            });
        }
        match page["nextPageToken"].as_str() {
            Some(token) if !token.is_empty() => page_token = Some(token.to_string()),
            _ => return Ok(models),
        }
    }
    Ok(models)
}

fn kind_of(id: &str) -> ModelKind {
    let id = id.to_ascii_lowercase();
    let other = ["tts", "live", "transcribe", "omni", "veo", "lyria", "robotics", "embedding", "imagen", "decision"];
    if other.iter().any(|word| id.contains(word)) {
        ModelKind::Other
    } else if id.contains("image") || id.contains("banana") {
        ModelKind::Image
    } else {
        ModelKind::Text
    }
}

/// A value made safe for a URL's query.
fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_has_what_generate_content_asks_for() {
        let request = Request::new("models/gemini-4-carbon", "Tóm tắt").with_system("Be brief.");
        let request = request.with_schema(json!({"type": "object"})).with_temperature(0.5);
        let plain = body(&request);
        assert_eq!(plain["contents"][0]["role"], "user");
        assert_eq!(plain["contents"][0]["parts"][0]["text"], "Tóm tắt");
        assert_eq!(plain["systemInstruction"]["parts"][0]["text"], "Be brief.");
        assert_eq!(plain["generationConfig"]["responseMimeType"], "application/json");
        assert!(plain["generationConfig"].get("temperature").is_none());
        assert_eq!(plain["generationConfig"]["maxOutputTokens"], 16_000);
        assert_eq!(checked_model("models/gemini-4-carbon").unwrap(), "gemini-4-carbon");
        assert!(checked_model("gemini/../x?").is_err());
    }

    #[test]
    fn finish_reasons_are_named() {
        assert_eq!(stop_reason(Some("STOP"), None), StopReason::End);
        assert_eq!(stop_reason(Some("MAX_TOKENS"), None), StopReason::Length);
        assert!(matches!(stop_reason(Some("SAFETY"), None), StopReason::Refusal { category: Some(category), .. } if category == "safety"));
        assert!(matches!(stop_reason(None, Some("PROHIBITED_CONTENT")), StopReason::Refusal { .. }));
        assert_eq!(stop_reason(Some("MALFORMED_RESPONSE"), None), StopReason::Other("MALFORMED_RESPONSE".into()));
    }

    #[test]
    fn models_are_sorted_by_kind() {
        assert_eq!(kind_of("gemini-4-carbon"), ModelKind::Text);
        assert_eq!(kind_of("gemini-3-pro-image"), ModelKind::Image);
        assert_eq!(kind_of("gemini-nano-banana-2.1"), ModelKind::Image);
        assert_eq!(kind_of("gemini-3.8-flash-tts"), ModelKind::Other);
        assert_eq!(encode_query("a+b/c=="), "a%2Bb%2Fc%3D%3D");
    }
}
