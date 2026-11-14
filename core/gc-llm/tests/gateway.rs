//! The gateway as the apps use it: the cloud switch per feature, the meter,
//! refusals as errors, and calls that block.

mod common;

use common::{png, quick, scratch, sse, Answer, Mock};
use gc_llm::{ApiKey, BatchItem, CloudSwitches, Feature, Gateway, ImageRequest, LlmError, Picture, Provider, ProviderKind, Request};
use serde_json::json;

/// Not a key: a string shaped like one.
const FAKE_KEY: &str = "sk-ant-fake-key-for-tests-0000";

fn anthropic(mock: &Mock) -> Provider {
    Provider::anthropic(Some(ApiKey::new(FAKE_KEY).unwrap())).with_base_url(&mock.url("/v1")).unwrap().with_http(quick())
}

fn message(text: &str) -> serde_json::Value {
    json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [{ "type": "text", "text": text }], "stop_reason": "end_turn",
        "usage": { "input_tokens": 1000, "output_tokens": 500, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 }
    })
}

#[test]
fn a_cloud_provider_is_refused_until_the_feature_allows_it() {
    let folder = scratch("switch");
    let mock = Mock::start(|_, _| Answer::json(200, message("Tóm tắt.")));
    let gateway = Gateway::open(&folder).unwrap();
    gateway.set_provider(anthropic(&mock));
    let request = Request::new("claude-opus-5-5", "Tóm tắt giúp tôi.");

    // Every feature starts on the user's devices, and nothing is sent.
    assert!(Feature::ALL.iter().all(|feature| !gateway.cloud_allowed(*feature)));
    let error = gateway.chat(Feature::Summary, "anthropic", &request).unwrap_err();
    assert_eq!(error, LlmError::CloudNotAllowed { feature: Feature::Summary, provider: "anthropic".into() });
    assert!(error.to_string().contains("Summaries"), "{error}");
    assert!(mock.seen().is_empty());

    gateway.set_cloud_allowed(Feature::Summary, true).unwrap();
    assert_eq!(gateway.chat(Feature::Summary, "anthropic", &request).unwrap().text, "Tóm tắt.");
    // Another feature is still kept on the devices.
    assert!(matches!(gateway.chat(Feature::Translation, "anthropic", &request), Err(LlmError::CloudNotAllowed { .. })));
    assert_eq!(mock.seen().len(), 1);

    // The choice is kept for the next run.
    drop(gateway);
    let again = Gateway::open(&folder).unwrap();
    assert!(again.cloud_allowed(Feature::Summary) && !again.cloud_allowed(Feature::TryOn));
    assert!(CloudSwitches::load(&folder).allowed(Feature::Summary));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_local_server_needs_no_switch() {
    let folder = scratch("local");
    let mock = Mock::start(|_, _| {
        let choice = json!({ "message": { "content": "Được." }, "finish_reason": "stop" });
        Answer::json(200, json!({ "model": "qwen2.5:7b", "choices": [choice], "usage": { "prompt_tokens": 5, "completion_tokens": 2 } }))
    });
    let gateway = Gateway::open(&folder).unwrap();
    gateway.set_provider(Provider::compatible("ollama", &mock.url("/v1"), None).unwrap().with_http(quick()));
    let reply = gateway.chat(Feature::TryOn, "ollama", &Request::new("qwen2.5:7b", "x")).unwrap();
    assert_eq!(reply.text, "Được.");
    let infos = gateway.providers();
    assert_eq!(infos.len(), 1);
    assert!(infos[0].local && !infos[0].has_key && infos[0].kind == ProviderKind::Compatible);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn every_call_is_metered_with_its_cost() {
    let folder = scratch("meter");
    let mock = Mock::start(|_, _| Answer::json(200, message("Xong.")));
    let gateway = Gateway::open(&folder).unwrap().with_utc_offset(7 * 60);
    gateway.set_provider(anthropic(&mock));
    gateway.set_cloud_allowed(Feature::CleanUp, true).unwrap();
    for _ in 0..3 {
        gateway.chat(Feature::CleanUp, "anthropic", &Request::new("claude-opus-5-5", "x")).unwrap();
    }
    let today = gateway.today();
    let report = gateway.usage(&today, &today).unwrap();
    assert_eq!(report.lines.len(), 1);
    let line = &report.lines[0];
    assert_eq!((line.provider.as_str(), line.model.as_str()), ("anthropic", "claude-opus-5-5"));
    assert_eq!((line.requests, line.input, line.output), (3, 3000, 1500));
    // 3000 x $4 + 1500 x $20 per million tokens.
    assert!((report.total_cost - 0.042).abs() < 1e-9, "{}", report.total_cost);
    assert!(report.unpriced.is_empty());
    assert!(folder.join("usage.json").exists() && folder.join("prices.json").exists());
    assert_eq!(gateway.meter_problem(), None);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_refusal_is_an_error_and_still_counted() {
    let folder = scratch("refusal");
    let mock = Mock::start(|_, _| {
        Answer::json(200, json!({
            "type": "message", "model": "claude-opus-5-5", "content": [],
            "stop_reason": "refusal", "stop_details": { "type": "refusal", "category": "bio", "explanation": null },
            "usage": { "input_tokens": 40, "output_tokens": 0 }
        }))
    });
    let gateway = Gateway::open(&folder).unwrap();
    gateway.set_provider(anthropic(&mock));
    gateway.set_cloud_allowed(Feature::Questions, true).unwrap();
    let error = gateway.chat(Feature::Questions, "anthropic", &Request::new("claude-opus-5-5", "…")).unwrap_err();
    let (provider, model) = ("anthropic".to_string(), "claude-opus-5-5".to_string());
    let declined = LlmError::Refused { provider, model, category: Some("bio".into()), explanation: None };
    assert_eq!(error, declined);
    let today = gateway.today();
    assert_eq!(gateway.usage(&today, &today).unwrap().lines[0].input, 40);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_streamed_answer_through_the_gateway() {
    let folder = scratch("stream");
    let delta = |text: &str| json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } });
    let start = json!({ "model": "claude-opus-5-5", "usage": { "input_tokens": 10, "output_tokens": 1 } });
    let stream = sse(&[
        ("message_start", json!({ "type": "message_start", "message": start })),
        ("content_block_delta", delta("Từng ")),
        ("content_block_delta", delta("chữ một.")),
        ("message_delta", json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 4 } })),
        ("message_stop", json!({ "type": "message_stop" })),
    ]);
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let gateway = Gateway::open(&folder).unwrap();
    gateway.set_provider(anthropic(&mock));
    gateway.set_cloud_allowed(Feature::Questions, true).unwrap();
    let mut shown = String::new();
    let request = Request::new("claude-opus-5-5", "…");
    let reply = gateway.chat_streaming(Feature::Questions, "anthropic", &request, &mut |piece| shown.push_str(piece)).unwrap();
    assert_eq!((shown.as_str(), reply.text.as_str()), ("Từng chữ một.", "Từng chữ một."));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_picture_for_try_on_is_counted() {
    let folder = scratch("picture");
    let made = png(3, 3);
    let encoded = common::base64(&made);
    let mock = Mock::start(move |_, _| {
        Answer::json(200, json!({ "data": [{ "b64_json": encoded }], "usage": { "input_tokens": 300, "output_tokens": 4000 } }))
    });
    let gateway = Gateway::open(&folder).unwrap();
    let openai = Provider::openai(Some(ApiKey::new("sk-fake-key-for-tests-0000").unwrap()));
    gateway.set_provider(openai.with_base_url(&mock.url("/v1")).unwrap().with_http(quick()));
    let person = Picture::new(png(2, 2)).unwrap();
    let request = ImageRequest { model: "gpt-image-2.6-sunburst".into(), prompt: "Mặc thử áo".into(), pictures: vec![person] };
    assert!(matches!(gateway.generate_image(Feature::TryOn, "openai", &request), Err(LlmError::CloudNotAllowed { .. })));
    gateway.set_cloud_allowed(Feature::TryOn, true).unwrap();
    assert_eq!(gateway.generate_image(Feature::TryOn, "openai", &request).unwrap().png, made);
    let today = gateway.today();
    let report = gateway.usage(&today, &today).unwrap();
    // 300 x $8 + 4000 x $30 per million tokens.
    assert!((report.total_cost - 0.1224).abs() < 1e-9, "{}", report.total_cost);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn batch_results_are_counted_once_at_the_batch_price() {
    let folder = scratch("batch");
    let mock = Mock::start(|seen, _| match seen.method.as_str() {
        "POST" => Answer::json(200, json!({ "id": "msgbatch_9", "processing_status": "in_progress", "request_counts": { "processing": 1 } })),
        _ if seen.path.ends_with("/results") => {
            let line = json!({ "custom_id": "p1", "result": { "type": "succeeded", "message": message("Trang một") } });
            Answer::text(200, "application/binary", &format!("{line}\n"))
        }
        _ => Answer::json(200, json!({ "id": "msgbatch_9", "processing_status": "ended", "request_counts": { "succeeded": 1 } })),
    });
    let gateway = Gateway::open(&folder).unwrap();
    gateway.set_provider(anthropic(&mock));
    let items = [BatchItem { id: "p1".into(), request: Request::new("claude-opus-5-5", "Trang 1") }];
    assert!(matches!(gateway.submit_batch(Feature::CleanUp, "anthropic", &items), Err(LlmError::CloudNotAllowed { .. })));
    gateway.set_cloud_allowed(Feature::CleanUp, true).unwrap();
    gateway.submit_batch(Feature::CleanUp, "anthropic", &items).unwrap();
    assert_eq!(gateway.batch("anthropic", "msgbatch_9").unwrap().state, gc_llm::BatchState::Ended);
    gateway.batch_results("anthropic", "msgbatch_9").unwrap();
    gateway.batch_results("anthropic", "msgbatch_9").unwrap();
    let today = gateway.today();
    let report = gateway.usage(&today, &today).unwrap();
    assert_eq!(report.lines.len(), 1);
    assert!(report.lines[0].batch && report.lines[0].requests == 1);
    // (1000 x $4 + 500 x $20) per million, at half price.
    assert!((report.total_cost - 0.007).abs() < 1e-9, "{}", report.total_cost);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn unknown_providers_and_missing_keys_are_named() {
    let folder = scratch("unknown");
    let gateway = Gateway::open(&folder).unwrap();
    let unknown = gateway.chat(Feature::Summary, "mistral", &Request::new("m", "x")).unwrap_err();
    assert_eq!(unknown, LlmError::UnknownProvider { name: "mistral".into() });
    gateway.set_provider(Provider::gemini(None));
    gateway.set_cloud_allowed(Feature::Summary, true).unwrap();
    let no_key = LlmError::NoKey { provider: "gemini".into() };
    assert_eq!(gateway.chat(Feature::Summary, "gemini", &Request::new("gemini-4-carbon", "x")).unwrap_err(), no_key);
    assert_eq!(gateway.models("gemini").unwrap_err(), no_key);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_key_is_never_printed() {
    let folder = scratch("printed");
    let gateway = Gateway::open(&folder).unwrap();
    let key = ApiKey::new(FAKE_KEY).unwrap();
    let provider = Provider::anthropic(Some(key.clone()));
    let mut printed = vec![format!("{key:?}"), format!("{provider:?}")];
    gateway.set_provider(provider);
    printed.push(format!("{:?}", gateway.providers()));
    for text in printed {
        assert!(!text.contains("fake-key-for-tests"), "{text}");
    }
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn the_gateway_will_not_block_inside_async_code() {
    let folder = scratch("async");
    let gateway = Gateway::open(&folder).unwrap();
    gateway.set_provider(Provider::ollama());
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let result = runtime.block_on(async { gateway.chat(Feature::Summary, "ollama", &Request::new("m", "x")) });
    assert!(matches!(result, Err(LlmError::InvalidInput { .. })), "{result:?}");
    let _ = std::fs::remove_dir_all(folder);
}
