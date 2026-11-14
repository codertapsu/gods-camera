//! OpenAI: the Responses API for text, the models list, and the Images API
//! for pictures made and edited.
//!
//! Checked against OpenAI's API reference on 2026-11-14. The current models
//! (GPT-6 Astra, GPT-6.1 Sol and the rest) reason before they answer, and
//! while they reason they refuse `temperature`, so it is not sent. Responses
//! are not stored on OpenAI's side (`store: false`). A refusal comes back as
//! a `refusal` part of the answer instead of text; a stop by the content
//! filter as an incomplete answer.

use serde_json::{json, Value};

use super::{count, reply, tell, to_bytes, OnText};
use crate::error::redact;
use crate::http::{Events, Who};
use crate::{ApiKey, GeneratedImage, ImageRequest, LlmError, ModelInfo, ModelKind, Part, Provider, Reply, Request, Role, StopReason, Usage};

pub(crate) fn body(request: &Request, stream: bool) -> Value {
    let input: Vec<Value> = request.messages.iter().map(input_message).collect();
    let mut body = json!({ "model": request.model, "input": input, "max_output_tokens": request.max_tokens, "store": false });
    if let Some(system) = &request.system {
        body["instructions"] = json!(system);
        if request.cache_system {
            // OpenAI caches a repeated start of a prompt on its own; a key
            // sends requests that share the instructions to the same cache.
            body["prompt_cache_key"] = json!(cache_key(system));
        }
    }
    if let Some(schema) = &request.schema {
        body["text"] = json!({ "format": { "type": "json_schema", "name": "result", "schema": schema, "strict": true } });
    }
    if stream {
        body["stream"] = json!(true);
    }
    body
}

fn input_message(message: &crate::Message) -> Value {
    match message.role {
        // An earlier answer goes back as plain text.
        Role::Assistant => json!({ "role": "assistant", "content": message.text() }),
        Role::User => {
            let content: Vec<Value> = message
                .parts
                .iter()
                .map(|part| match part {
                    Part::Text(text) => json!({ "type": "input_text", "text": text }),
                    // Pages of small print need the picture in detail.
                    Part::Picture(picture) => json!({ "type": "input_image", "image_url": picture.data_url(), "detail": "high" }),
                })
                .collect();
            json!({ "role": "user", "content": content })
        }
    }
}

/// A short name for the instructions that stays the same from run to run:
/// FNV-1a over their bytes. Not a secret, only a label.
fn cache_key(system: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in system.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("gc-{hash:016x}")
}

fn headers(builder: reqwest::RequestBuilder, key: &ApiKey) -> reqwest::RequestBuilder {
    builder.bearer_auth(key.expose()).header("content-type", "application/json")
}

pub(crate) async fn chat(provider: &Provider, request: &Request, on_text: OnText<'_>) -> Result<Reply, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    let bytes = to_bytes(&body(request, on_text.is_some()));
    let url = provider.url("/responses");
    let make = |client: &reqwest::Client| headers(client.post(&url), key).body(bytes.clone());
    if on_text.is_some() {
        let events = provider.http.events(&who, &make).await?;
        read_stream(provider, request, events, on_text).await
    } else {
        let answer = provider.http.json(&who, &make).await?;
        let (text, stop, usage, model) = read_response(&who, &answer)?;
        Ok(reply(provider, request, &text, stop, usage, model.as_deref()))
    }
}

/// The text, stop reason, usage and model of a whole response.
fn read_response(who: &Who<'_>, response: &Value) -> Result<(String, StopReason, Usage, Option<String>), LlmError> {
    if response["status"] == "failed" {
        return Err(failure(who, &response["error"]));
    }
    let Some(output) = response["output"].as_array() else {
        return Err(who.unexpected("the answer has no output"));
    };
    let mut text = String::new();
    let mut refusal = String::new();
    for item in output.iter().filter(|item| item["type"] == "message") {
        for part in item["content"].as_array().into_iter().flatten() {
            match part["type"].as_str() {
                Some("output_text") => text.push_str(part["text"].as_str().unwrap_or("")),
                Some("refusal") => refusal.push_str(part["refusal"].as_str().unwrap_or("")),
                _ => {}
            }
        }
    }
    let stop = stop_reason(&response["status"], &response["incomplete_details"]["reason"], &refusal);
    Ok((text, stop, usage(&response["usage"]), response["model"].as_str().map(str::to_string)))
}

fn stop_reason(status: &Value, incomplete: &Value, refusal: &str) -> StopReason {
    if !refusal.is_empty() {
        return StopReason::Refusal { category: None, explanation: Some(refusal.to_string()) };
    }
    match (status.as_str(), incomplete.as_str()) {
        (Some("completed"), _) => StopReason::End,
        (Some("incomplete"), Some("max_output_tokens")) => StopReason::Length,
        (Some("incomplete"), Some("content_filter")) => StopReason::Refusal { category: Some("content_filter".into()), explanation: None },
        (_, Some(reason)) => StopReason::Other(reason.to_string()),
        (Some(status), None) => StopReason::Other(status.to_string()),
        (None, None) => StopReason::Other("none given".into()),
    }
}

/// OpenAI counts the cached input inside `input_tokens`; it is taken out so
/// that `input` is only what was read at the full price.
fn usage(value: &Value) -> Usage {
    let all = count(&value["input_tokens"]);
    let cached = count(&value["input_tokens_details"]["cached_tokens"]);
    let written = count(&value["input_tokens_details"]["cache_write_tokens"]);
    Usage { input: all.saturating_sub(cached + written), cached, cache_written: written, output: count(&value["output_tokens"]) }
}

/// A response that failed on OpenAI's side.
fn failure(who: &Who<'_>, error: &Value) -> LlmError {
    let message = redact(error["message"].as_str().unwrap_or("no details were given"), who.key);
    let provider = who.name.to_string();
    let code = error["code"].as_str().unwrap_or("");
    if code.contains("rate_limit") || code == "slow_down" {
        LlmError::RateLimited { provider, retry_after: None, message }
    } else if code.contains("overloaded") {
        LlmError::Overloaded { provider, message }
    } else if code == "server_error" || code.is_empty() {
        LlmError::ServerError { provider, status: 500, message }
    } else {
        LlmError::BadRequest { provider, status: 400, message: format!("{code}: {message}") }
    }
}

async fn read_stream(provider: &Provider, request: &Request, mut events: Events, mut on_text: OnText<'_>) -> Result<Reply, LlmError> {
    let who = provider.who();
    let mut text = String::new();
    let mut refusal = String::new();
    let mut last: Option<Value> = None;
    while let Some(event) = events.next(&who).await? {
        if event.data.trim() == "[DONE]" {
            break;
        }
        let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
            return Err(who.unexpected(format!("a piece of the answer is not JSON: {}", crate::error::shorten(&event.data, 80))));
        };
        match data["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => {
                let piece = data["delta"].as_str().unwrap_or("");
                text.push_str(piece);
                tell(&mut on_text, piece);
            }
            "response.refusal.delta" => refusal.push_str(data["delta"].as_str().unwrap_or("")),
            "response.completed" | "response.incomplete" => {
                last = Some(data["response"].clone());
                break;
            }
            "response.failed" => return Err(failure(&who, &data["response"]["error"])),
            "error" => return Err(failure(&who, &data)),
            _ => {}
        }
    }
    let Some(response) = last else {
        return Err(LlmError::Network { provider: who.name.to_string(), message: "the answer stopped before it was complete".into() });
    };
    let (whole, stop, usage, model) = read_response(&who, &response)?;
    // The deltas make the text; the final response repeats it, and is used
    // when no delta came. A refusal streamed in pieces counts even if the
    // final response left its part out.
    let text = if text.is_empty() { whole } else { text };
    let stop = match stop {
        StopReason::Refusal { .. } => stop,
        _ if !refusal.is_empty() => StopReason::Refusal { category: None, explanation: Some(refusal) },
        _ => stop,
    };
    Ok(reply(provider, request, &text, stop, usage, model.as_deref()))
}

/// The models list names every model, for speech and embeddings too; the
/// kind is told by the name, since the list says nothing more.
pub(crate) async fn models(provider: &Provider) -> Result<Vec<ModelInfo>, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    let url = provider.url("/models");
    let make = |client: &reqwest::Client| client.get(&url).bearer_auth(key.expose());
    let answer = provider.http.json(&who, &make).await?;
    let Some(entries) = answer["data"].as_array() else {
        return Err(who.unexpected("the list of models has no data"));
    };
    let mut models: Vec<ModelInfo> = entries
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .map(|id| ModelInfo { id: id.to_string(), name: id.to_string(), kind: kind_of(id), context_tokens: None, output_tokens: None })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

pub(crate) fn kind_of(id: &str) -> ModelKind {
    let id = id.to_ascii_lowercase();
    if id.contains("image") || id.starts_with("dall-e") {
        ModelKind::Image
    } else if ["embedding", "tts", "transcribe", "whisper", "realtime", "audio", "moderation", "live", "sora", "search", "speech"]
        .iter()
        .any(|word| id.contains(word))
    {
        ModelKind::Other
    } else {
        ModelKind::Text
    }
}

/// A picture from words alone goes to `images/generations`; one made from
/// other pictures to `images/edits`, which takes them as `data:` URLs in JSON.
pub(crate) async fn generate_image(provider: &Provider, request: &ImageRequest) -> Result<GeneratedImage, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    let mut body = json!({ "model": request.model, "prompt": request.prompt, "n": 1, "output_format": "png" });
    let path = if request.pictures.is_empty() {
        "/images/generations"
    } else {
        let images: Vec<Value> = request.pictures.iter().map(|picture| json!({ "image_url": picture.data_url() })).collect();
        body["images"] = json!(images);
        "/images/edits"
    };
    let bytes = to_bytes(&body);
    let url = provider.url(path);
    let make = |client: &reqwest::Client| headers(client.post(&url), key).body(bytes.clone());
    // A refusal by the safety system comes as an error answer; the provider
    // gives it the model's name.
    let answer = provider.http.json(&who, &make).await?;
    let Some(encoded) = answer["data"][0]["b64_json"].as_str() else {
        return Err(who.unexpected("no picture came back"));
    };
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| who.unexpected("the picture is not valid base64"))?;
    let png = super::as_png(&who, png)?;
    let usage = &answer["usage"];
    Ok(GeneratedImage {
        png,
        text: String::new(),
        usage: Usage { input: count(&usage["input_tokens"]), cached: 0, cache_written: 0, output: count(&usage["output_tokens"]) },
        provider: provider.name().to_string(),
        model: request.model.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, Picture};

    #[test]
    fn the_body_has_what_the_responses_api_asks_for() {
        let png = Picture::new(b"\x89PNG\r\n\x1a\nrest".to_vec()).unwrap();
        let mut request = Request::new("gpt-6-astra", "unused").with_system("Read the page.").cached().with_temperature(0.3);
        request.messages =
            vec![Message::user_with_pictures("What does it say?", vec![png]), Message::assistant("It says hello."), Message::user("More?")];
        let body = body(&request, true);
        assert_eq!(body["instructions"], "Read the page.");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert!(body.get("temperature").is_none());
        assert!(body["prompt_cache_key"].as_str().unwrap().starts_with("gc-"));
        let first = &body["input"][0]["content"];
        assert_eq!(first[0]["type"], "input_image");
        assert!(first[0]["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert_eq!(first[1]["text"], "What does it say?");
        assert_eq!(body["input"][1], json!({ "role": "assistant", "content": "It says hello." }));
        // The same instructions always give the same key.
        assert_eq!(cache_key("x"), cache_key("x"));
        assert_ne!(cache_key("x"), cache_key("y"));
    }

    #[test]
    fn models_are_sorted_by_kind() {
        assert_eq!(kind_of("gpt-6-astra"), ModelKind::Text);
        assert_eq!(kind_of("gpt-image-2.6-sunburst"), ModelKind::Image);
        assert_eq!(kind_of("text-embedding-3-large"), ModelKind::Other);
        assert_eq!(kind_of("gpt-live-transcribe"), ModelKind::Other);
    }

    #[test]
    fn cached_input_is_counted_apart() {
        let details = json!({ "cached_tokens": 600, "cache_write_tokens": 100 });
        let counted = usage(&json!({ "input_tokens": 1000, "input_tokens_details": details, "output_tokens": 50 }));
        assert_eq!(counted, Usage { input: 300, cached: 600, cache_written: 100, output: 50 });
    }
}
