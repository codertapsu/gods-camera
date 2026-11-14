//! OpenAI's Responses, Models and Images APIs against a stand-in server.

mod common;

use std::time::{Duration, Instant};

use common::{base64, png, quick, sse, Answer, Mock};
use gc_llm::{ApiKey, ImageRequest, LlmError, Message, ModelKind, Picture, Provider, Request, StopReason, Usage};
use serde_json::{json, Value};

/// Not a key: a string shaped like one.
const FAKE_KEY: &str = "sk-proj-fake-key-for-tests-0000";

fn provider(mock: &Mock) -> Provider {
    Provider::openai(Some(ApiKey::new(FAKE_KEY).unwrap())).with_base_url(&mock.url("/v1")).unwrap().with_http(quick())
}

/// A whole response: a reasoning item, then the message with these parts.
fn response(parts: Value, status: &str, incomplete: Option<&str>) -> Value {
    json!({
        "id": "resp_1",
        "object": "response",
        "status": status,
        "model": "gpt-6-astra-2026-09-03",
        "incomplete_details": incomplete.map(|reason| json!({ "reason": reason })),
        "output": [
            { "id": "rs_1", "type": "reasoning", "summary": [] },
            { "id": "msg_1", "type": "message", "role": "assistant", "status": "completed", "content": parts }
        ],
        "usage": {
            "input_tokens": 900,
            "input_tokens_details": { "cached_tokens": 800, "cache_write_tokens": 0 },
            "output_tokens": 70,
            "output_tokens_details": { "reasoning_tokens": 40 },
            "total_tokens": 970
        }
    })
}

fn text(text: &str) -> Value {
    json!([{ "type": "output_text", "text": text, "annotations": [] }])
}

fn delta(sequence: u32, text: &str) -> (&'static str, Value) {
    let event = json!({
        "type": "response.output_text.delta",
        "sequence_number": sequence,
        "item_id": "msg_1",
        "output_index": 1,
        "content_index": 0,
        "delta": text
    });
    ("response.output_text.delta", event)
}

fn error(status: u16, code: &str, message: &str) -> Answer {
    Answer::json(status, json!({ "error": { "message": message, "type": "invalid_request_error", "code": code } }))
}

#[tokio::test]
async fn a_plain_answer_and_the_request_behind_it() {
    let mock = Mock::start(|_, _| Answer::json(200, response(text("Trang này nói về chợ nổi."), "completed", None)));
    let page = Picture::new(png(4, 3)).unwrap();
    let mut request = Request::new("gpt-6-astra", "unused").with_system("Đọc trang.").with_temperature(0.7);
    request.messages = vec![Message::user_with_pictures("Trang này nói gì?", vec![page])];
    let reply = provider(&mock).chat(&request).await.unwrap();
    assert_eq!(reply.text, "Trang này nói về chợ nổi.");
    assert_eq!(reply.stop, StopReason::End);
    assert_eq!(reply.usage, Usage { input: 100, cached: 800, cache_written: 0, output: 70 });
    assert_eq!(reply.model, "gpt-6-astra-2026-09-03");

    let seen = &mock.seen()[0];
    assert_eq!((seen.method.as_str(), seen.path.as_str()), ("POST", "/v1/responses"));
    assert_eq!(seen.header("authorization"), Some(format!("Bearer {FAKE_KEY}").as_str()));
    let body = seen.json();
    assert_eq!(body["model"], "gpt-6-astra");
    assert_eq!(body["instructions"], "Đọc trang.");
    assert_eq!(body["max_output_tokens"], 16_000);
    assert_eq!(body["store"], false);
    assert!(body.get("temperature").is_none(), "the reasoning models refuse temperature");
    let content = &body["input"][0]["content"];
    assert_eq!(content[0]["type"], "input_image");
    assert_eq!(content[0]["image_url"], format!("data:image/png;base64,{}", base64(&png(4, 3))));
    assert_eq!(content[1], json!({ "type": "input_text", "text": "Trang này nói gì?" }));
}

#[tokio::test]
async fn a_structured_answer_comes_back_parsed() {
    let mock = Mock::start(|_, _| Answer::json(200, response(text("{\"date\": \"2026-11-13\"}"), "completed", None)));
    let schema = json!({
        "type": "object",
        "properties": { "date": { "type": "string" } },
        "required": ["date"],
        "additionalProperties": false
    });
    let reply = provider(&mock).chat(&Request::new("gpt-6-astra", "When?").with_schema(schema.clone())).await.unwrap();
    assert_eq!(reply.json, Some(json!({ "date": "2026-11-13" })));
    let format = json!({ "type": "json_schema", "name": "result", "schema": schema, "strict": true });
    assert_eq!(mock.seen()[0].json()["text"], json!({ "format": format }));
}

#[tokio::test]
async fn refusals_and_cut_answers_are_named() {
    let mock = Mock::start(|_, number| match number {
        0 => Answer::json(200, response(json!([{ "type": "refusal", "refusal": "I can't help with that." }]), "completed", None)),
        1 => Answer::json(200, response(text("Một nửa"), "incomplete", Some("max_output_tokens"))),
        _ => Answer::json(200, response(json!([]), "incomplete", Some("content_filter"))),
    });
    let provider = provider(&mock);
    let refused = provider.chat(&Request::new("gpt-6-astra", "…")).await.unwrap();
    assert_eq!(refused.stop, StopReason::Refusal { category: None, explanation: Some("I can't help with that.".into()) });
    let cut = provider.chat(&Request::new("gpt-6-astra", "…")).await.unwrap();
    assert_eq!((cut.text.as_str(), cut.stop), ("Một nửa", StopReason::Length));
    let filtered = provider.chat(&Request::new("gpt-6-astra", "…")).await.unwrap();
    assert!(matches!(filtered.stop, StopReason::Refusal { category: Some(category), .. } if category == "content_filter"));
}

#[tokio::test]
async fn a_streamed_answer_arrives_in_pieces() {
    let finished = response(text("Xin chào, thế giới."), "completed", None);
    let stream = sse(&[
        ("response.created", json!({ "type": "response.created", "sequence_number": 0, "response": { "status": "in_progress" } })),
        delta(1, "Xin chào, "),
        delta(2, "thế giới."),
        ("response.output_text.done", json!({ "type": "response.output_text.done", "sequence_number": 3, "text": "Xin chào, thế giới." })),
        ("response.completed", json!({ "type": "response.completed", "sequence_number": 4, "response": finished })),
    ]);
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let mut pieces = Vec::new();
    let mut on_text = |piece: &str| pieces.push(piece.to_string());
    let reply = provider(&mock).chat_streaming(&Request::new("gpt-6-astra", "Chào"), &mut on_text).await.unwrap();
    assert_eq!(pieces, vec!["Xin chào, ", "thế giới."]);
    assert_eq!(reply.text, "Xin chào, thế giới.");
    assert_eq!(reply.stop, StopReason::End);
    assert_eq!(reply.usage.output, 70);
    assert_eq!(mock.seen()[0].json()["stream"], true);
}

#[tokio::test]
async fn a_failed_stream_is_typed() {
    let failed = json!({ "status": "failed", "error": { "code": "server_is_overloaded", "message": "The server is overloaded." } });
    let stream = sse(&[
        ("response.created", json!({ "type": "response.created", "response": { "status": "in_progress" } })),
        ("response.failed", json!({ "type": "response.failed", "response": failed })),
    ]);
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let mut on_text = |_: &str| {};
    let error = provider(&mock).chat_streaming(&Request::new("gpt-6-astra", "…"), &mut on_text).await.unwrap_err();
    assert!(matches!(error, LlmError::Overloaded { .. }), "{error:?}");
}

#[tokio::test]
async fn the_model_list_is_sorted_by_kind() {
    let mock = Mock::start(|_, _| {
        let model = |id: &str| json!({ "id": id, "object": "model", "created": 1, "owned_by": "system" });
        Answer::json(200, json!({ "object": "list", "data": [model("gpt-image-2.6-sunburst"), model("gpt-6-astra"), model("gpt-transcribe")] }))
    });
    let models = provider(&mock).models().await.unwrap();
    let kinds: Vec<(&str, ModelKind)> = models.iter().map(|model| (model.id.as_str(), model.kind)).collect();
    let expected = vec![("gpt-6-astra", ModelKind::Text), ("gpt-image-2.6-sunburst", ModelKind::Image), ("gpt-transcribe", ModelKind::Other)];
    assert_eq!(kinds, expected);
    let seen = &mock.seen()[0];
    assert_eq!((seen.method.as_str(), seen.path.as_str()), ("GET", "/v1/models"));
    assert_eq!(seen.header("authorization"), Some(format!("Bearer {FAKE_KEY}").as_str()));
}

#[tokio::test]
async fn a_picture_is_made_from_words_or_from_pictures() {
    let made = png(8, 8);
    let encoded = base64(&made);
    let mock = Mock::start(move |_, _| {
        let usage = json!({ "input_tokens": 50, "input_tokens_details": { "image_tokens": 0, "text_tokens": 50 }, "output_tokens": 1056 });
        Answer::json(200, json!({ "created": 1, "data": [{ "b64_json": encoded }], "output_format": "png", "usage": usage }))
    });
    let provider = provider(&mock);
    let words = ImageRequest { model: "gpt-image-2.6-sunburst".into(), prompt: "A floating market at dawn".into(), pictures: vec![] };
    let from_words = provider.generate_image(&words).await.unwrap();
    assert_eq!(from_words.png, made);
    assert_eq!(from_words.usage, Usage { input: 50, cached: 0, cache_written: 0, output: 1056 });

    let person = Picture::new(png(2, 2)).unwrap();
    let garment = Picture::new(common::jpeg(2, 2)).unwrap();
    let prompt = "The person wearing the shirt".to_string();
    let try_on = ImageRequest { model: "gpt-image-2.6-sunburst".into(), prompt, pictures: vec![person, garment] };
    provider.generate_image(&try_on).await.unwrap();

    let seen = mock.seen();
    assert_eq!(seen[0].path, "/v1/images/generations");
    let expected = json!({ "model": "gpt-image-2.6-sunburst", "prompt": "A floating market at dawn", "n": 1, "output_format": "png" });
    assert_eq!(seen[0].json(), expected);
    assert_eq!(seen[1].path, "/v1/images/edits");
    let body = seen[1].json();
    assert!(body["images"][0]["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert!(body["images"][1]["image_url"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
}

#[tokio::test]
async fn a_picture_the_safety_system_blocks_is_a_refusal() {
    let mock = Mock::start(|_, _| error(400, "moderation_blocked", "Your request was rejected by the safety system."));
    let request = ImageRequest { model: "gpt-image-2.6-sunburst".into(), prompt: "…".into(), pictures: vec![] };
    match provider(&mock).generate_image(&request).await.unwrap_err() {
        LlmError::Refused { model, category, .. } => {
            assert_eq!((model.as_str(), category.as_deref()), ("gpt-image-2.6-sunburst", Some("safety")));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_wait_in_milliseconds_is_honoured_and_the_masked_key_hidden() {
    let mock = Mock::start(|_, number| match number {
        0 => {
            let body = json!({ "error": { "message": "Rate limit reached", "type": "requests", "code": "rate_limit_exceeded" } });
            Answer::json_with(429, &[("retry-after-ms", "300")], body)
        }
        1 => Answer::json(200, response(text("OK"), "completed", None)),
        // OpenAI quotes the last characters of a wrong key.
        _ => error(401, "invalid_api_key", "Incorrect API key provided: sk-proj-********************0000. Find your key online."),
    });
    let provider = provider(&mock);
    let started = Instant::now();
    assert_eq!(provider.chat(&Request::new("gpt-6-astra", "…")).await.unwrap().text, "OK");
    assert!(started.elapsed() >= Duration::from_millis(300));
    let error = provider.chat(&Request::new("gpt-6-astra", "…")).await.unwrap_err();
    assert!(matches!(error, LlmError::KeyRefused { .. }), "{error:?}");
    assert!(!error.to_string().contains("0000"), "{error}");
}

#[tokio::test]
async fn an_account_without_credit_is_not_tried_again() {
    let mock = Mock::start(|_, _| error(429, "insufficient_quota", "You exceeded your current quota."));
    let error = provider(&mock).chat(&Request::new("gpt-6-astra", "…")).await.unwrap_err();
    assert!(matches!(error, LlmError::RateLimited { retry_after: None, .. }), "{error:?}");
    assert_eq!(mock.seen().len(), 1);
}
