//! Servers that speak OpenAI's Chat Completions interface: Ollama, LM
//! Studio, llama.cpp's server and many more. Only what they all share is
//! used: `/chat/completions`, streamed or not, and `/models`.
//!
//! They are the local models, so they get the request's temperature, and a
//! key only when one was given: Ollama, for one, needs none.

use serde_json::{json, Value};

use super::{count, reply, tell, to_bytes, OnText};
use crate::http::{status_error, Events, Who};
use crate::{LlmError, Message, ModelInfo, ModelKind, Part, Provider, Reply, Request, Role, StopReason, Usage};

pub(crate) fn body(request: &Request, stream: bool) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.extend(request.messages.iter().map(message));
    let mut body = json!({ "model": request.model, "messages": messages, "max_tokens": request.max_tokens });
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(schema) = &request.schema {
        body["response_format"] = json!({ "type": "json_schema", "json_schema": { "name": "result", "schema": schema, "strict": true } });
    }
    if stream {
        body["stream"] = json!(true);
        // Without this the counts never come in a stream.
        body["stream_options"] = json!({ "include_usage": true });
    }
    body
}

/// Text alone goes as a plain string, which every server reads; pictures as
/// a list of parts with `data:` URLs.
fn message(message: &Message) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let has_picture = message.parts.iter().any(|part| matches!(part, Part::Picture(_)));
    if !has_picture {
        return json!({ "role": role, "content": message.text() });
    }
    let content: Vec<Value> = message
        .parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => json!({ "type": "text", "text": text }),
            Part::Picture(picture) => json!({ "type": "image_url", "image_url": { "url": picture.data_url() } }),
        })
        .collect();
    json!({ "role": role, "content": content })
}

fn with_key(builder: reqwest::RequestBuilder, provider: &Provider) -> reqwest::RequestBuilder {
    match provider.key() {
        Some(key) => builder.bearer_auth(key.expose()),
        None => builder,
    }
}

pub(crate) async fn chat(provider: &Provider, request: &Request, on_text: OnText<'_>) -> Result<Reply, LlmError> {
    let who = provider.who();
    let bytes = to_bytes(&body(request, on_text.is_some()));
    let url = provider.url("/chat/completions");
    let make = |client: &reqwest::Client| {
        with_key(client.post(&url), provider).header("content-type", "application/json").body(bytes.clone())
    };
    if on_text.is_some() {
        let events = provider.http.events(&who, &make).await?;
        read_stream(provider, request, events, on_text).await
    } else {
        let answer = provider.http.json(&who, &make).await?;
        if let Some(error) = answer.get("error") {
            return Err(status_error(&who, 500, json!({ "error": error }).to_string().as_bytes(), None));
        }
        let choice = &answer["choices"][0];
        if choice.is_null() {
            return Err(who.unexpected("the answer has no choices"));
        }
        let text = without_thinking(choice["message"]["content"].as_str().unwrap_or(""));
        let stop = stop_reason(choice["finish_reason"].as_str(), choice["message"]["refusal"].as_str());
        Ok(reply(provider, request, text, stop, usage(&answer["usage"]), answer["model"].as_str()))
    }
}

/// The answer without the thinking a local reasoning model may write at its
/// start between `<think>` tags, when the server leaves it in the text. Most
/// servers now send it apart; this is for those that do not, so that a
/// clean-up is not compared against the model's thoughts.
fn without_thinking(text: &str) -> &str {
    let trimmed = text.trim_start();
    match (trimmed.strip_prefix("<think>"), trimmed.find("</think>")) {
        (Some(_), Some(end)) => trimmed[end + "</think>".len()..].trim_start(),
        _ => text,
    }
}

async fn read_stream(provider: &Provider, request: &Request, mut events: Events, mut on_text: OnText<'_>) -> Result<Reply, LlmError> {
    let who = provider.who();
    let mut text = String::new();
    let mut refusal = String::new();
    let mut finish: Option<String> = None;
    let mut counted = Usage::default();
    let mut model: Option<String> = None;
    let mut done = false;
    while let Some(event) = events.next(&who).await? {
        if event.data.trim() == "[DONE]" {
            done = true;
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(&event.data) else {
            return Err(who.unexpected(format!("a piece of the answer is not JSON: {}", crate::error::shorten(&event.data, 80))));
        };
        if let Some(error) = chunk.get("error") {
            return Err(stream_error(&who, error));
        }
        if let Some(name) = chunk["model"].as_str() {
            model = Some(name.to_string());
        }
        let choice = &chunk["choices"][0];
        // Reasoning models send their thinking in other fields, which are skipped.
        if let Some(piece) = choice["delta"]["content"].as_str() {
            text.push_str(piece);
            tell(&mut on_text, piece);
        }
        if let Some(piece) = choice["delta"]["refusal"].as_str() {
            refusal.push_str(piece);
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            finish = Some(reason.to_string());
        }
        // The counts come last, in a piece of their own.
        if chunk["usage"].is_object() {
            counted = usage(&chunk["usage"]);
        }
    }
    if !done && finish.is_none() {
        return Err(LlmError::Network { provider: who.name.to_string(), message: "the answer stopped before it was complete".into() });
    }
    let stop = stop_reason(finish.as_deref(), Some(refusal.as_str()).filter(|refusal| !refusal.is_empty()));
    // The pieces handed out went as they came; the reply's text is the answer alone.
    Ok(reply(provider, request, without_thinking(&text), stop, counted, model.as_deref()))
}

fn stop_reason(finish: Option<&str>, refusal: Option<&str>) -> StopReason {
    if let Some(refusal) = refusal.filter(|refusal| !refusal.is_empty()) {
        return StopReason::Refusal { category: None, explanation: Some(refusal.to_string()) };
    }
    match finish {
        Some("stop") => StopReason::End,
        Some("length") => StopReason::Length,
        Some("content_filter") => StopReason::Refusal { category: Some("content_filter".into()), explanation: None },
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::Other("none given".into()),
    }
}

fn usage(value: &Value) -> Usage {
    let prompt = count(&value["prompt_tokens"]);
    let cached = count(&value["prompt_tokens_details"]["cached_tokens"]);
    Usage { input: prompt.saturating_sub(cached), cached, cache_written: 0, output: count(&value["completion_tokens"]) }
}

fn stream_error(who: &Who<'_>, error: &Value) -> LlmError {
    let message = error["message"].as_str().or_else(|| error.as_str()).unwrap_or("no details were given");
    LlmError::ServerError { provider: who.name.to_string(), status: 500, message: crate::error::redact(message, who.key) }
}

/// Every model the server has. The list does not say what a model is for:
/// all are taken for text.
pub(crate) async fn models(provider: &Provider) -> Result<Vec<ModelInfo>, LlmError> {
    let who = provider.who();
    let url = provider.url("/models");
    let make = |client: &reqwest::Client| with_key(client.get(&url), provider);
    let answer = provider.http.json(&who, &make).await?;
    let Some(entries) = answer["data"].as_array() else {
        return Err(who.unexpected("the list of models has no data"));
    };
    let mut models: Vec<ModelInfo> = entries
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .map(|id| ModelInfo { id: id.to_string(), name: id.to_string(), kind: ModelKind::Text, context_tokens: None, output_tokens: None })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Picture;

    #[test]
    fn the_body_has_what_chat_completions_asks_for() {
        let request =
            Request::new("qwen2.5:7b", "Xin chào").with_system("Be brief.").with_temperature(0.0).with_schema(json!({"type": "object"}));
        let plain = body(&request, false);
        assert_eq!(plain["messages"][0], json!({ "role": "system", "content": "Be brief." }));
        assert_eq!(plain["messages"][1], json!({ "role": "user", "content": "Xin chào" }));
        assert_eq!(plain["temperature"], 0.0);
        assert_eq!(plain["max_tokens"], 16_000);
        assert_eq!(plain["response_format"]["type"], "json_schema");
        assert!(plain.get("stream").is_none());
        let streamed = body(&request, true);
        assert_eq!(streamed["stream_options"]["include_usage"], true);

        let jpeg = Picture::new(vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3]).unwrap();
        let mut with_picture = Request::new("llava", "x");
        with_picture.messages = vec![Message::user_with_pictures("Đọc trang này", vec![jpeg])];
        let parts = &body(&with_picture, false)["messages"][0]["content"];
        assert!(parts[0]["image_url"]["url"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
        assert_eq!(parts[1]["text"], "Đọc trang này");
    }

    #[test]
    fn a_reasoning_models_thoughts_are_left_out() {
        assert_eq!(without_thinking("<think>\nThe user wants…\n</think>\n\nĐồng ý."), "Đồng ý.");
        assert_eq!(without_thinking("Plain answer"), "Plain answer");
        // An answer that only mentions the tag keeps it.
        assert_eq!(without_thinking("Use </think> to close"), "Use </think> to close");
        // Thinking that never ended is left as it is, for the caller to see.
        assert_eq!(without_thinking("<think> still going"), "<think> still going");
    }

    #[test]
    fn finish_reasons_are_named() {
        assert_eq!(stop_reason(Some("stop"), None), StopReason::End);
        assert_eq!(stop_reason(Some("length"), None), StopReason::Length);
        assert!(matches!(stop_reason(Some("stop"), Some("I can't help with that.")), StopReason::Refusal { .. }));
        assert!(matches!(stop_reason(Some("content_filter"), None), StopReason::Refusal { .. }));
    }
}
