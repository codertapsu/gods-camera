//! Anthropic's Messages, Models and Batches APIs against a stand-in server:
//! what is sent, and how each kind of answer is read.

mod common;

use std::time::{Duration, Instant};

use common::{quick, sse, Answer, Mock};
use gc_llm::{ApiKey, BatchItem, BatchOutcome, BatchState, LlmError, ModelKind, Provider, Request, StopReason, Usage};
use serde_json::{json, Value};

/// Not a key: a string shaped like one, to see that it is sent where it
/// belongs and never shows anywhere else.
const FAKE_KEY: &str = "sk-ant-fake-key-for-tests-0000";

fn provider(mock: &Mock) -> Provider {
    Provider::anthropic(Some(ApiKey::new(FAKE_KEY).unwrap())).with_base_url(&mock.url("/v1")).unwrap().with_http(quick())
}

fn message(text: &str, stop: &str) -> Value {
    json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5-5",
        "content": [
            { "type": "thinking", "thinking": "", "signature": "sig" },
            { "type": "text", "text": text }
        ],
        "stop_reason": stop,
        "stop_details": null,
        "usage": { "input_tokens": 120, "output_tokens": 45, "cache_read_input_tokens": 600, "cache_creation_input_tokens": 0 }
    })
}

fn error(status: u16, kind: &str, text: &str) -> Answer {
    Answer::json(status, json!({ "type": "error", "error": { "type": kind, "message": text } }))
}

// The events of a streamed answer.

fn started(usage: Value) -> (&'static str, Value) {
    let message = json!({ "id": "msg_02", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "content": [], "usage": usage });
    ("message_start", json!({ "type": "message_start", "message": message }))
}

fn block(index: u32, block: Value) -> (&'static str, Value) {
    ("content_block_start", json!({ "type": "content_block_start", "index": index, "content_block": block }))
}

fn delta(index: u32, text: &str) -> (&'static str, Value) {
    ("content_block_delta", json!({ "type": "content_block_delta", "index": index, "delta": { "type": "text_delta", "text": text } }))
}

fn block_end(index: u32) -> (&'static str, Value) {
    ("content_block_stop", json!({ "type": "content_block_stop", "index": index }))
}

fn finished(stop: &str, usage: Value) -> (&'static str, Value) {
    ("message_delta", json!({ "type": "message_delta", "delta": { "stop_reason": stop, "stop_sequence": null }, "usage": usage }))
}

fn ended() -> (&'static str, Value) {
    ("message_stop", json!({ "type": "message_stop" }))
}

#[tokio::test]
async fn a_plain_answer_and_the_request_behind_it() {
    let mock = Mock::start(|_, _| Answer::json(200, message("Xin chào! Tôi có thể giúp gì?", "end_turn")));
    let request = Request::new("claude-opus-5-5", "Chào bạn").with_system("Trả lời ngắn gọn.").cached().with_temperature(0.4);
    let reply = provider(&mock).chat(&request).await.unwrap();
    assert_eq!(reply.text, "Xin chào! Tôi có thể giúp gì?");
    assert_eq!(reply.stop, StopReason::End);
    assert_eq!(reply.usage, Usage { input: 120, cached: 600, cache_written: 0, output: 45 });
    assert_eq!((reply.provider.as_str(), reply.model.as_str()), ("anthropic", "claude-opus-5-5"));
    assert_eq!(reply.fallback_from, None);
    assert_eq!(reply.json, None);

    let seen = &mock.seen()[0];
    assert_eq!((seen.method.as_str(), seen.path.as_str()), ("POST", "/v1/messages"));
    assert_eq!(seen.header("x-api-key"), Some(FAKE_KEY));
    assert_eq!(seen.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(seen.header("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
    assert_eq!(seen.header("content-type"), Some("application/json"));
    let body = seen.json();
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["max_tokens"], 16_000);
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["system"][0]["text"], "Trả lời ngắn gọn.");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(body["messages"], json!([{ "role": "user", "content": [{ "type": "text", "text": "Chào bạn" }] }]));
    assert!(body.get("temperature").is_none(), "API version 2026-10-21 refuses temperature");
    assert!(body.get("stream").is_none());
}

#[tokio::test]
async fn a_structured_answer_comes_back_parsed_and_composed() {
    // The model's answer has "Việt" with its marks as separate characters.
    let answer = r#"{"title": "Việt Nam", "pages": 3}"#;
    let mock = Mock::start(move |_, _| Answer::json(200, message(answer, "end_turn")));
    let schema = json!({
        "type": "object",
        "properties": { "title": { "type": "string" }, "pages": { "type": "integer" } },
        "required": ["title", "pages"],
        "additionalProperties": false
    });
    let reply = provider(&mock).chat(&Request::new("claude-opus-5-5", "Extract.").with_schema(schema.clone())).await.unwrap();
    assert_eq!(reply.json, Some(json!({ "title": "Việt Nam", "pages": 3 })));
    let body = mock.seen()[0].json();
    assert_eq!(body["output_config"], json!({ "format": { "type": "json_schema", "schema": schema } }));
}

#[tokio::test]
async fn a_streamed_answer_arrives_in_pieces() {
    let stream = sse(&[
        started(json!({ "input_tokens": 50, "output_tokens": 1, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 })),
        block(0, json!({ "type": "thinking", "thinking": "", "signature": "" })),
        ("content_block_delta", json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "" } })),
        block_end(0),
        ("ping", json!({ "type": "ping" })),
        block(1, json!({ "type": "text", "text": "" })),
        delta(1, "Mùa nước "),
        delta(1, "nổi về "),
        delta(1, "sớm."),
        block_end(1),
        finished("end_turn", json!({ "output_tokens": 18 })),
        ended(),
    ]);
    // Windows line ends, as some servers send them.
    let stream = stream.replace('\n', "\r\n");
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let mut pieces = Vec::new();
    let mut on_text = |piece: &str| pieces.push(piece.to_string());
    let reply = provider(&mock).chat_streaming(&Request::new("claude-opus-5-5", "Kể tiếp"), &mut on_text).await.unwrap();
    assert_eq!(pieces, vec!["Mùa nước ", "nổi về ", "sớm."]);
    assert_eq!(reply.text, "Mùa nước nổi về sớm.");
    assert_eq!(reply.stop, StopReason::End);
    assert_eq!(reply.usage, Usage { input: 50, cached: 0, cache_written: 0, output: 18 });
    assert_eq!(mock.seen()[0].json()["stream"], true);
}

#[tokio::test]
async fn a_refusal_is_reported_as_such() {
    let refused = json!({
        "id": "msg_03",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5-5",
        "content": [],
        "stop_reason": "refusal",
        "stop_details": { "type": "refusal", "category": "cyber", "explanation": "This request was declined." },
        "usage": { "input_tokens": 30, "output_tokens": 0 }
    });
    let mock = Mock::start(move |_, _| Answer::json(200, refused.clone()));
    let reply = provider(&mock).chat(&Request::new("claude-opus-5-5", "…")).await.unwrap();
    let declined = StopReason::Refusal { category: Some("cyber".into()), explanation: Some("This request was declined.".into()) };
    assert_eq!(reply.stop, declined);
    assert_eq!(reply.text, "");
}

#[tokio::test]
async fn a_fallback_in_a_stream_names_the_model_that_declined() {
    let stream = sse(&[
        started(json!({ "input_tokens": 10, "output_tokens": 1 })),
        block(0, json!({ "type": "text", "text": "" })),
        delta(0, "Phần đầu, "),
        block_end(0),
        block(1, json!({ "type": "fallback", "from": { "model": "claude-opus-5-5" }, "to": { "model": "claude-opus-5" } })),
        block_end(1),
        block(2, json!({ "type": "text", "text": "" })),
        delta(2, "phần sau."),
        block_end(2),
        finished("end_turn", json!({ "output_tokens": 9, "iterations": [{ "type": "message" }, { "type": "fallback_message" }] })),
        ended(),
    ]);
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let mut on_text = |_: &str| {};
    let reply = provider(&mock).chat_streaming(&Request::new("claude-opus-5-5", "…"), &mut on_text).await.unwrap();
    assert_eq!(reply.text, "Phần đầu, phần sau.");
    assert_eq!(reply.fallback_from.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(reply.stop, StopReason::End);
}

#[tokio::test]
async fn a_long_answer_is_streamed_underneath() {
    let stream = sse(&[started(json!({ "input_tokens": 5 })), delta(0, "Một câu trả lời dài."), finished("end_turn", json!({})), ended()]);
    let mock = Mock::start(move |seen, _| {
        if seen.json()["stream"] == true {
            Answer::events(&stream)
        } else {
            Answer::json(200, message("not streamed", "end_turn"))
        }
    });
    // The caller waits for the whole answer, which may take many minutes.
    let reply = provider(&mock).chat(&Request::new("claude-opus-5-5", "Viết dài").with_max_tokens(64_000)).await.unwrap();
    assert_eq!(reply.text, "Một câu trả lời dài.");
    assert_eq!(mock.seen()[0].json()["stream"], true);
}

#[tokio::test]
async fn an_account_without_the_fallback_is_asked_again_without_it() {
    let mock = Mock::start(|seen, _| {
        if seen.json().get("fallbacks").is_some() {
            error(400, "invalid_request_error", "Unexpected value(s) `server-side-fallback-2026-07-01` for the `anthropic-beta` header.")
        } else {
            Answer::json(200, message("Được.", "end_turn"))
        }
    });
    let reply = provider(&mock).chat(&Request::new("claude-opus-5-5", "…")).await.unwrap();
    assert_eq!(reply.text, "Được.");
    let seen = mock.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].header("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
    assert!(seen[1].header("anthropic-beta").is_none() && seen[1].json().get("fallbacks").is_none());
}

#[tokio::test]
async fn an_error_inside_a_stream_is_typed() {
    let stream = sse(&[
        started(json!({ "input_tokens": 10 })),
        ("error", json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } })),
    ]);
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let mut on_text = |_: &str| {};
    let error = provider(&mock).chat_streaming(&Request::new("claude-opus-5-5", "…"), &mut on_text).await.unwrap_err();
    assert!(matches!(error, LlmError::Overloaded { .. }), "{error:?}");
}

#[tokio::test]
async fn a_stream_cut_short_is_a_network_error() {
    let stream = sse(&[started(json!({})), delta(0, "Một nửa")]);
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let mut on_text = |_: &str| {};
    let error = provider(&mock).chat_streaming(&Request::new("claude-opus-5-5", "…"), &mut on_text).await.unwrap_err();
    assert!(matches!(error, LlmError::Network { .. }), "{error:?}");
}

#[tokio::test]
async fn the_model_list_is_read_page_by_page() {
    let mock = Mock::start(|seen, _| {
        if seen.path.contains("after_id=claude-opus-5-5") {
            let haiku = json!({ "type": "model", "id": "claude-haiku-5-5", "display_name": "Claude Haiku 5.5" });
            Answer::json(200, json!({ "data": [haiku], "has_more": false, "first_id": "claude-haiku-5-5", "last_id": "claude-haiku-5-5" }))
        } else {
            let opus = json!({
                "type": "model",
                "id": "claude-opus-5-5",
                "display_name": "Claude Opus 5.5",
                "max_input_tokens": 1_000_000,
                "max_tokens": 128_000
            });
            Answer::json(200, json!({ "data": [opus], "has_more": true, "first_id": "claude-opus-5-5", "last_id": "claude-opus-5-5" }))
        }
    });
    let models = provider(&mock).models().await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!((models[0].id.as_str(), models[0].name.as_str()), ("claude-opus-5-5", "Claude Opus 5.5"));
    assert_eq!((models[0].context_tokens, models[0].output_tokens), (Some(1_000_000), Some(128_000)));
    assert_eq!(models[0].kind, ModelKind::Text);
    let seen = mock.seen();
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("GET", "/v1/models?limit=1000"));
    assert_eq!(seen[1].path, "/v1/models?limit=1000&after_id=claude-opus-5-5");
    assert_eq!(seen[1].header("anthropic-version"), Some("2026-10-21"));
}

#[tokio::test]
async fn a_refused_key_never_shows_in_the_error() {
    let mock = Mock::start(|_, _| error(401, "authentication_error", &format!("invalid x-api-key: {FAKE_KEY}")));
    let error = provider(&mock).chat(&Request::new("claude-opus-5-5", "…")).await.unwrap_err();
    assert!(matches!(error, LlmError::KeyRefused { .. }), "{error:?}");
    for printed in [error.to_string(), format!("{error:?}")] {
        assert!(!printed.contains("fake-key-for-tests"), "{printed}");
    }
    assert_eq!(mock.seen().len(), 1, "a refused key is not tried again");
}

#[tokio::test]
async fn a_rate_limit_is_waited_out_as_the_server_asks() {
    let mock = Mock::start(|_, number| {
        if number == 0 {
            let body = json!({ "type": "error", "error": { "type": "rate_limit_error", "message": "slow down" } });
            Answer::json_with(429, &[("retry-after", "1")], body)
        } else {
            Answer::json(200, message("Được rồi.", "end_turn"))
        }
    });
    let started = Instant::now();
    let reply = provider(&mock).chat(&Request::new("claude-opus-5-5", "…")).await.unwrap();
    assert_eq!(reply.text, "Được rồi.");
    assert!(started.elapsed() >= Duration::from_secs(1), "waited {:?}", started.elapsed());
    assert_eq!(mock.seen().len(), 2);
}

#[tokio::test]
async fn a_wait_longer_than_allowed_goes_back_to_the_caller() {
    let body = json!({ "type": "error", "error": { "type": "rate_limit_error", "message": "daily limit" } });
    let mock = Mock::start(move |_, _| Answer::json_with(429, &[("retry-after", "3600")], body.clone()));
    let error = provider(&mock).chat(&Request::new("claude-opus-5-5", "…")).await.unwrap_err();
    match error {
        LlmError::RateLimited { retry_after, message, .. } => {
            assert_eq!(retry_after, Some(Duration::from_secs(3600)));
            assert_eq!(message, "daily limit");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(mock.seen().len(), 1);
}

#[tokio::test]
async fn server_errors_are_tried_again_a_few_times() {
    let mock = Mock::start(|_, number| match number {
        0 => error(500, "api_error", "Internal server error"),
        1 => error(529, "overloaded_error", "Overloaded"),
        _ => Answer::json(200, message("Xong.", "end_turn")),
    });
    assert_eq!(provider(&mock).chat(&Request::new("claude-opus-5-5", "…")).await.unwrap().text, "Xong.");
    assert_eq!(mock.seen().len(), 3);

    let always = Mock::start(|_, _| error(529, "overloaded_error", "Overloaded"));
    let error = provider(&always).chat(&Request::new("claude-opus-5-5", "…")).await.unwrap_err();
    assert!(matches!(error, LlmError::Overloaded { .. }), "{error:?}");
    assert_eq!(always.seen().len(), 4, "the first try and three more");
}

#[tokio::test]
async fn a_bad_request_carries_the_providers_words_and_is_not_repeated() {
    let mock = Mock::start(|_, _| error(400, "invalid_request_error", "max_tokens: must be at most 128000"));
    let error = provider(&mock).chat(&Request::new("claude-opus-5-5", "…").with_max_tokens(500_000)).await.unwrap_err();
    let expected = LlmError::BadRequest { provider: "anthropic".into(), status: 400, message: "max_tokens: must be at most 128000".into() };
    assert_eq!(error, expected);
    assert_eq!(mock.seen().len(), 1);
}

#[tokio::test]
async fn a_silent_server_runs_into_the_time_limit() {
    let mock = Mock::start(|_, _| Answer::Silence(Duration::from_secs(3)));
    let mut options = quick();
    options.idle_timeout = Duration::from_millis(300);
    options.answer_timeout = Duration::from_millis(300);
    let provider = provider(&mock).with_http(options);
    let error = provider.chat(&Request::new("claude-opus-5-5", "…")).await.unwrap_err();
    assert!(matches!(error, LlmError::TimedOut { .. }), "{error:?}");
    let mut on_text = |_: &str| {};
    let error = provider.chat_streaming(&Request::new("claude-opus-5-5", "…"), &mut on_text).await.unwrap_err();
    assert!(matches!(error, LlmError::TimedOut { .. }), "{error:?}");
}

#[tokio::test]
async fn without_a_key_nothing_is_sent() {
    let mock = Mock::start(|_, _| Answer::json(200, json!({})));
    let keyless = Provider::anthropic(None).with_base_url(&mock.url("/v1")).unwrap();
    let error = keyless.chat(&Request::new("claude-opus-5-5", "…")).await.unwrap_err();
    assert_eq!(error, LlmError::NoKey { provider: "anthropic".into() });
    assert!(mock.seen().is_empty());
}

fn batch_state(status: &str, counts: Value) -> Answer {
    Answer::json(200, json!({ "id": "msgbatch_01", "type": "message_batch", "processing_status": status, "request_counts": counts }))
}

fn result_line(id: &str, result: Value) -> String {
    format!("{}\n", json!({ "custom_id": id, "result": result }))
}

#[tokio::test]
async fn a_batch_is_sent_polled_and_read_by_id() {
    let mock = Mock::start(|seen, _| match (seen.method.as_str(), seen.path.as_str()) {
        ("POST", "/v1/messages/batches") => batch_state("in_progress", json!({ "processing": 3, "succeeded": 0, "errored": 0 })),
        ("GET", "/v1/messages/batches/msgbatch_01") => batch_state("ended", json!({ "processing": 0, "succeeded": 2, "errored": 1 })),
        ("GET", "/v1/messages/batches/msgbatch_01/results") => {
            let failed = json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "image too large" } });
            let lines = [
                result_line("page-2", json!({ "type": "succeeded", "message": message("{\"text\": \"Trang hai\"}", "end_turn") })),
                result_line("page-1", json!({ "type": "succeeded", "message": message("Trang một", "end_turn") })),
                result_line("page-3", json!({ "type": "errored", "error": failed })),
            ];
            Answer::text(200, "application/binary", &lines.concat())
        }
        _ => error(404, "not_found_error", "no such route"),
    });
    let provider = provider(&mock);
    let item = |page: u32| BatchItem { id: format!("page-{page}"), request: Request::new("claude-opus-5-5", format!("Trang {page}")) };
    let items: Vec<BatchItem> = (1..=3).map(item).collect();
    let submitted = provider.submit_batch(&items).await.unwrap();
    assert_eq!((submitted.id.as_str(), submitted.state, submitted.processing), ("msgbatch_01", BatchState::InProgress, 3));
    let body = mock.seen()[0].json();
    assert_eq!(body["requests"].as_array().unwrap().len(), 3);
    assert_eq!(body["requests"][0]["custom_id"], "page-1");
    let params = &body["requests"][0]["params"];
    assert_eq!(params["messages"][0]["content"][0]["text"], "Trang 1");
    // A batch takes neither a stream nor a fallback.
    assert!(params.get("stream").is_none() && params.get("fallbacks").is_none());
    assert!(mock.seen()[0].header("anthropic-beta").is_none());

    let ended = provider.batch("msgbatch_01").await.unwrap();
    assert_eq!((ended.state, ended.succeeded, ended.errored), (BatchState::Ended, 2, 1));

    let results = provider.batch_results("msgbatch_01").await.unwrap();
    assert_eq!(results.len(), 3);
    let by_id = |id: &str| &results.iter().find(|result| result.id == id).unwrap().outcome;
    match by_id("page-1") {
        BatchOutcome::Succeeded(reply) => assert_eq!((reply.text.as_str(), reply.json.as_ref()), ("Trang một", None)),
        other => panic!("{other:?}"),
    }
    match by_id("page-2") {
        BatchOutcome::Succeeded(reply) => assert_eq!(reply.json, Some(json!({ "text": "Trang hai" }))),
        other => panic!("{other:?}"),
    }
    assert_eq!(by_id("page-3"), &BatchOutcome::Errored("image too large".into()));
}

#[tokio::test]
async fn batch_ids_are_checked_before_sending() {
    let mock = Mock::start(|_, _| Answer::json(200, json!({})));
    let provider = provider(&mock);
    let item = |id: &str| BatchItem { id: id.into(), request: Request::new("claude-opus-5-5", "x") };
    assert!(matches!(provider.submit_batch(&[item("a"), item("a")]).await, Err(LlmError::InvalidInput { .. })));
    assert!(matches!(provider.submit_batch(&[item("trang một")]).await, Err(LlmError::InvalidInput { .. })));
    assert!(matches!(provider.submit_batch(&[]).await, Err(LlmError::InvalidInput { .. })));
    assert!(matches!(provider.batch("../secrets").await, Err(LlmError::InvalidInput { .. })));
    assert!(mock.seen().is_empty());
}
