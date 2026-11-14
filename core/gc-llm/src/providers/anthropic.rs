//! Anthropic: the Messages API, the Models API and message batches.
//!
//! The requests use API version 2026-10-21. Under it a request that names
//! `temperature`, `top_p` or `top_k` is refused, so the request's temperature
//! is never sent. Claude's current models think before they answer (Claude
//! Opus 5.5 always does), the thinking counts towards `max_tokens`, and its
//! blocks come back empty and are skipped.
//!
//! A refusal by the safety classifiers is a normal answer with the stop
//! reason `refusal` and, in `stop_details`, a category. For the models that
//! take it, the request asks for Anthropic's server-side fallback
//! (`"fallbacks": "default"`): a declined request then runs on the model
//! Anthropic recommends for that kind of refusal, in the same call, and a
//! `fallback` block in the answer names the model that declined. When every
//! model declines, the refusal comes back as it is. Batches take no fallback.

use serde_json::{json, Value};

use super::{count, reply, reply_for, tell, to_bytes, OnText};
use crate::error::redact;
use crate::http::{Events, Who};
use crate::{
    ApiKey, BatchInfo, BatchItem, BatchOutcome, BatchResult, BatchState, LlmError, Message, ModelInfo, ModelKind, Part, Provider, Reply,
    Request, Role, StopReason, Usage,
};

const VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// The models that take `"fallbacks": "default"`. Claude Haiku 5.5 has no
/// server-side fallback, and the older models run no such classifiers.
const FALLBACK_MODELS: [&str; 7] = [
    "claude-fable-5-2",
    "claude-mythos-5-2",
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
];

/// The request body. A batch request carries neither `stream` nor `fallbacks`.
pub(crate) fn body(request: &Request, stream: bool, batch: bool) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(message).collect();
    let mut body = json!({ "model": request.model, "max_tokens": request.max_tokens, "messages": messages });
    if let Some(system) = &request.system {
        body["system"] = if request.cache_system {
            // The cache ends after the instructions: everything before this
            // mark is kept for five minutes and read back at a twentieth of
            // the price. A prompt shorter than the model's minimum (512
            // tokens on the current models) is not cached, without an error.
            json!([{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }])
        } else {
            json!(system)
        };
    }
    if let Some(schema) = &request.schema {
        body["output_config"] = json!({ "format": { "type": "json_schema", "schema": schema } });
    }
    if stream {
        body["stream"] = json!(true);
    }
    if !batch && request.allow_fallback && FALLBACK_MODELS.contains(&request.model.as_str()) {
        body["fallbacks"] = json!("default");
    }
    body
}

fn message(message: &Message) -> Value {
    let content: Vec<Value> = message
        .parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => json!({ "type": "text", "text": text }),
            Part::Picture(picture) => {
                json!({ "type": "image", "source": { "type": "base64", "media_type": picture.mime(), "data": picture.base64() } })
            }
        })
        .collect();
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    json!({ "role": role, "content": content })
}

fn headers(builder: reqwest::RequestBuilder, key: &ApiKey, fallback: bool) -> reqwest::RequestBuilder {
    let builder = builder.header("x-api-key", key.expose()).header("anthropic-version", VERSION).header("content-type", "application/json");
    if fallback {
        builder.header("anthropic-beta", FALLBACK_BETA)
    } else {
        builder
    }
}

pub(crate) async fn chat(provider: &Provider, request: &Request, mut on_text: OnText<'_>) -> Result<Reply, LlmError> {
    let key = provider.required_key()?;
    let mut body = body(request, on_text.is_some(), false);
    let answer = exchange(provider, request, key, &body, &mut on_text).await;
    // An account that was not given the fallback hears so in a 400 about the
    // beta header; the request goes again without it, and a refusal then
    // comes back as it is. Nothing was streamed before such an answer.
    let about_fallback = |error: &LlmError| {
        matches!(error, LlmError::BadRequest { message, .. } if message.contains("anthropic-beta") || message.contains("fallbacks"))
    };
    match answer {
        Err(error) if body.get("fallbacks").is_some() && about_fallback(&error) => {
            if let Some(fields) = body.as_object_mut() {
                fields.remove("fallbacks");
            }
            exchange(provider, request, key, &body, &mut on_text).await
        }
        answer => answer,
    }
}

async fn exchange(provider: &Provider, request: &Request, key: &ApiKey, body: &Value, on_text: &mut OnText<'_>) -> Result<Reply, LlmError> {
    let who = provider.who();
    let fallback = body.get("fallbacks").is_some();
    let bytes = to_bytes(body);
    let url = provider.url("/messages");
    let make = |client: &reqwest::Client| headers(client.post(&url), key, fallback).body(bytes.clone());
    if on_text.is_some() {
        let events = provider.http.events(&who, &make).await?;
        read_stream(provider, request, events, on_text).await
    } else {
        let answer = provider.http.json(&who, &make).await?;
        read_message(provider, &request.model, request.schema.is_some(), &answer)
    }
}

/// A whole message, as a plain answer or a batch result gives it.
fn read_message(provider: &Provider, asked: &str, wants_json: bool, answer: &Value) -> Result<Reply, LlmError> {
    let who = provider.who();
    let Some(blocks) = answer["content"].as_array() else {
        return Err(who.unexpected("the answer has no content"));
    };
    let mut text = String::new();
    let mut fallback_from = None;
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
            Some("fallback") => fallback_from = block["from"]["model"].as_str().map(str::to_string),
            _ => {}
        }
    }
    let stop = stop_reason(&answer["stop_reason"], &answer["stop_details"]);
    let usage = usage(&answer["usage"]);
    let mut reply = reply_for(provider, asked, wants_json, &text, stop, usage, answer["model"].as_str());
    // A conversation that fell back once is served by the fallback model for
    // a while without a new block; the usage still says so.
    reply.fallback_from = fallback_from.or_else(|| fallback_ran(&answer["usage"]).then(|| asked.to_string()));
    Ok(reply)
}

async fn read_stream(provider: &Provider, request: &Request, mut events: Events, on_text: &mut OnText<'_>) -> Result<Reply, LlmError> {
    let who = provider.who();
    let mut text = String::new();
    let mut model: Option<String> = None;
    let mut usage = Usage::default();
    let mut reason = Value::Null;
    let mut details = Value::Null;
    let mut fallback_from = None;
    let mut fallback = false;
    let mut complete = false;
    while let Some(event) = events.next(&who).await? {
        let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
            return Err(who.unexpected(format!("a piece of the answer is not JSON: {}", crate::error::shorten(&event.data, 80))));
        };
        let kind = data["type"].as_str().unwrap_or(event.name.as_str()).to_string();
        match kind.as_str() {
            "message_start" => {
                model = data["message"]["model"].as_str().map(str::to_string);
                merge_usage(&mut usage, &data["message"]["usage"]);
            }
            "content_block_start" => {
                let block = &data["content_block"];
                match block["type"].as_str() {
                    Some("text") => {
                        let piece = block["text"].as_str().unwrap_or("");
                        text.push_str(piece);
                        tell(on_text, piece);
                    }
                    Some("fallback") => fallback_from = block["from"]["model"].as_str().map(str::to_string),
                    _ => {}
                }
            }
            "content_block_delta" => {
                if data["delta"]["type"] == "text_delta" {
                    let piece = data["delta"]["text"].as_str().unwrap_or("");
                    text.push_str(piece);
                    tell(on_text, piece);
                }
            }
            "message_delta" => {
                if !data["delta"]["stop_reason"].is_null() {
                    reason = data["delta"]["stop_reason"].clone();
                }
                for found in [&data["delta"]["stop_details"], &data["stop_details"]] {
                    if !found.is_null() {
                        details = found.clone();
                    }
                }
                merge_usage(&mut usage, &data["usage"]);
                fallback |= fallback_ran(&data["usage"]);
            }
            "message_stop" => {
                complete = true;
                break;
            }
            "error" => return Err(stream_error(&who, &data["error"])),
            _ => {}
        }
    }
    if !complete && reason.is_null() {
        return Err(LlmError::Network { provider: who.name.to_string(), message: "the answer stopped before it was complete".into() });
    }
    let mut reply = reply(provider, request, &text, stop_reason(&reason, &details), usage, model.as_deref());
    reply.fallback_from = fallback_from.or_else(|| fallback.then(|| request.model.clone()));
    Ok(reply)
}

fn stop_reason(reason: &Value, details: &Value) -> StopReason {
    match reason.as_str() {
        Some("end_turn" | "stop_sequence") => StopReason::End,
        Some("max_tokens" | "model_context_window_exceeded") => StopReason::Length,
        Some("refusal") => StopReason::Refusal {
            category: details["category"].as_str().map(str::to_string),
            explanation: details["explanation"].as_str().map(str::to_string),
        },
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::Other("none given".into()),
    }
}

/// Anthropic counts the cached input apart: `input_tokens` is only what was
/// read at the full price.
fn usage(value: &Value) -> Usage {
    Usage {
        input: count(&value["input_tokens"]),
        cached: count(&value["cache_read_input_tokens"]),
        cache_written: count(&value["cache_creation_input_tokens"]),
        output: count(&value["output_tokens"]),
    }
}

/// A stream gives the counts at the start and again, grown, near the end.
fn merge_usage(usage: &mut Usage, value: &Value) {
    let fields: [(&str, &mut u64); 4] = [
        ("input_tokens", &mut usage.input),
        ("cache_read_input_tokens", &mut usage.cached),
        ("cache_creation_input_tokens", &mut usage.cache_written),
        ("output_tokens", &mut usage.output),
    ];
    for (name, field) in fields {
        if let Some(found) = value[name].as_u64() {
            *field = found;
        }
    }
}

/// Whether a fallback model served the answer: its attempt is listed in the
/// usage as a `fallback_message`.
fn fallback_ran(usage: &Value) -> bool {
    usage["iterations"].as_array().is_some_and(|attempts| attempts.iter().any(|attempt| attempt["type"] == "fallback_message"))
}

/// An error sent inside a stream that had already begun.
fn stream_error(who: &Who<'_>, error: &Value) -> LlmError {
    let message = redact(error["message"].as_str().unwrap_or("no details were given"), who.key);
    let provider = who.name.to_string();
    match error["type"].as_str() {
        Some("overloaded_error") => LlmError::Overloaded { provider, message },
        Some("rate_limit_error") => LlmError::RateLimited { provider, retry_after: None, message },
        Some("api_error") => LlmError::ServerError { provider, status: 500, message },
        Some("authentication_error" | "permission_error") => LlmError::KeyRefused { provider, message },
        Some("invalid_request_error") => LlmError::BadRequest { provider, status: 400, message },
        _ => LlmError::Unexpected { provider, message },
    }
}

/// The models this key may use, newest first, one page after another.
pub(crate) async fn models(provider: &Provider) -> Result<Vec<ModelInfo>, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    let mut models = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut url = provider.url("/models?limit=1000");
        if let Some(last) = &after {
            url.push_str("&after_id=");
            url.push_str(last);
        }
        let make = |client: &reqwest::Client| headers(client.get(&url), key, false);
        let page = provider.http.json(&who, &make).await?;
        let Some(entries) = page["data"].as_array() else {
            return Err(who.unexpected("the list of models has no data"));
        };
        for entry in entries {
            let Some(id) = entry["id"].as_str() else { continue };
            models.push(ModelInfo {
                id: id.to_string(),
                name: entry["display_name"].as_str().unwrap_or(id).to_string(),
                kind: ModelKind::Text,
                context_tokens: entry["max_input_tokens"].as_u64(),
                output_tokens: entry["max_tokens"].as_u64(),
            });
        }
        let last = page["last_id"].as_str().filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)));
        match (page["has_more"].as_bool(), last) {
            (Some(true), Some(last)) if after.as_deref() != Some(last) => after = Some(last.to_string()),
            _ => break,
        }
    }
    Ok(models)
}

/// Whether an id can go into a URL and a batch: 1 to 64 letters, digits, `-` or `_`.
fn plain_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub(crate) async fn submit_batch(provider: &Provider, items: &[BatchItem]) -> Result<BatchInfo, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    if items.is_empty() {
        return Err(LlmError::InvalidInput { message: "a batch needs at least one request".into() });
    }
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if !plain_id(&item.id) || !seen.insert(item.id.as_str()) {
            let message = format!("“{}” cannot name a request in a batch: use 1 to 64 letters, digits, - or _, once each", item.id);
            return Err(LlmError::InvalidInput { message });
        }
        item.request.check()?;
    }
    let requests: Vec<Value> =
        items.iter().map(|item| json!({ "custom_id": item.id, "params": body(&item.request, false, true) })).collect();
    let bytes = to_bytes(&json!({ "requests": requests }));
    let url = provider.url("/messages/batches");
    let make = |client: &reqwest::Client| headers(client.post(&url), key, false).body(bytes.clone());
    read_batch(&who, &provider.http.json(&who, &make).await?)
}

pub(crate) async fn batch(provider: &Provider, id: &str) -> Result<BatchInfo, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    if !plain_id(id) {
        return Err(LlmError::InvalidInput { message: format!("“{id}” is not a batch id") });
    }
    let url = provider.url(&format!("/messages/batches/{id}"));
    let make = |client: &reqwest::Client| headers(client.get(&url), key, false);
    read_batch(&who, &provider.http.json(&who, &make).await?)
}

fn read_batch(who: &Who<'_>, answer: &Value) -> Result<BatchInfo, LlmError> {
    let state = match answer["processing_status"].as_str() {
        Some("in_progress") => BatchState::InProgress,
        Some("canceling") => BatchState::Canceling,
        Some("ended") => BatchState::Ended,
        other => return Err(who.unexpected(format!("a batch in an unknown state: {other:?}"))),
    };
    let counts = &answer["request_counts"];
    Ok(BatchInfo {
        id: answer["id"].as_str().ok_or_else(|| who.unexpected("the batch has no id"))?.to_string(),
        state,
        processing: count(&counts["processing"]),
        succeeded: count(&counts["succeeded"]),
        errored: count(&counts["errored"]),
        canceled: count(&counts["canceled"]),
        expired: count(&counts["expired"]),
    })
}

/// The results file has one JSON line per request, in no particular order.
/// The requests are not at hand any more, so an answer is read as JSON when
/// it looks like JSON.
pub(crate) async fn batch_results(provider: &Provider, id: &str) -> Result<Vec<BatchResult>, LlmError> {
    let key = provider.required_key()?;
    let who = provider.who();
    if !plain_id(id) {
        return Err(LlmError::InvalidInput { message: format!("“{id}” is not a batch id") });
    }
    let url = provider.url(&format!("/messages/batches/{id}/results"));
    let make = |client: &reqwest::Client| headers(client.get(&url), key, false);
    let body = provider.http.bytes(&who, &make).await?;
    let text = String::from_utf8_lossy(&body);
    let mut results = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let entry: Value = serde_json::from_str(line).map_err(|error| who.unexpected(format!("a result line is not JSON ({error})")))?;
        let id = entry["custom_id"].as_str().ok_or_else(|| who.unexpected("a result has no id"))?.to_string();
        let result = &entry["result"];
        let outcome = match result["type"].as_str() {
            Some("succeeded") => {
                let message = &result["message"];
                let asked = message["model"].as_str().unwrap_or("");
                let blocks = message["content"].as_array().into_iter().flatten();
                let first_text = blocks.filter(|block| block["type"] == "text").find_map(|block| block["text"].as_str()).unwrap_or("");
                let looks_like_json = first_text.trim_start().starts_with(['{', '[']);
                BatchOutcome::Succeeded(read_message(provider, asked, looks_like_json, message)?)
            }
            Some("errored") => {
                let error = &result["error"];
                let message = error["error"]["message"].as_str().or_else(|| error["message"].as_str()).unwrap_or("no details were given");
                BatchOutcome::Errored(redact(message, who.key))
            }
            Some("canceled") => BatchOutcome::Canceled,
            Some("expired") => BatchOutcome::Expired,
            other => return Err(who.unexpected(format!("a result of an unknown kind: {other:?}"))),
        };
        results.push(BatchResult { id, outcome });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_has_what_the_messages_api_asks_for() {
        let request = Request::new("claude-opus-5-5", "Xin chào").with_system("Be brief.").with_temperature(0.2);
        let plain = body(&request, false, false);
        assert_eq!(plain["model"], "claude-opus-5-5");
        assert_eq!(plain["system"], "Be brief.");
        assert_eq!(plain["messages"][0]["content"][0]["text"], "Xin chào");
        assert_eq!(plain["fallbacks"], "default");
        // API version 2026-10-21 refuses any temperature.
        assert!(plain.get("temperature").is_none());
        assert!(plain.get("stream").is_none());

        let cached = body(&request.clone().cached().with_schema(json!({"type": "object"})), true, false);
        assert_eq!(cached["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(cached["output_config"]["format"]["type"], "json_schema");
        assert_eq!(cached["stream"], true);

        // No fallback for a batch, for a model without one, or when it is not wanted.
        assert!(body(&request, false, true).get("fallbacks").is_none());
        assert!(body(&Request::new("claude-haiku-5-5", "x"), false, false).get("fallbacks").is_none());
        let mut declined = request.clone();
        declined.allow_fallback = false;
        assert!(body(&declined, false, false).get("fallbacks").is_none());
    }

    #[test]
    fn stop_reasons_are_named() {
        assert_eq!(stop_reason(&json!("end_turn"), &Value::Null), StopReason::End);
        assert_eq!(stop_reason(&json!("max_tokens"), &Value::Null), StopReason::Length);
        assert_eq!(
            stop_reason(&json!("refusal"), &json!({"type": "refusal", "category": "bio", "explanation": null})),
            StopReason::Refusal { category: Some("bio".into()), explanation: None }
        );
        assert_eq!(stop_reason(&json!("pause_turn"), &Value::Null), StopReason::Other("pause_turn".into()));
    }

    #[test]
    fn ids_for_urls_are_plain() {
        assert!(plain_id("page-001_a"));
        assert!(!plain_id(""));
        assert!(!plain_id("../x"));
        assert!(!plain_id(&"a".repeat(65)));
    }
}
