//! Google Gemini's generateContent, streamGenerateContent and models list
//! against a stand-in server.

mod common;

use common::{base64, jpeg, quick, sse, Answer, Mock};
use gc_llm::{ApiKey, ImageRequest, LlmError, Message, ModelKind, Picture, Provider, Request, StopReason, Usage};
use serde_json::{json, Value};

/// Not a key: a string shaped like one of Google's.
const FAKE_KEY: &str = "AIzaFakeKeyForTests000000";

fn provider(mock: &Mock) -> Provider {
    Provider::gemini(Some(ApiKey::new(FAKE_KEY).unwrap())).with_base_url(&mock.url("/v1beta")).unwrap().with_http(quick())
}

/// One candidate with these parts and finish reason, and the counts.
fn candidate(parts: Value, finish: &str, usage: Value) -> Value {
    json!({
        "candidates": [{ "content": { "role": "model", "parts": parts }, "finishReason": finish, "index": 0 }],
        "usageMetadata": usage,
        "modelVersion": "gemini-4-carbon",
        "responseId": "r1"
    })
}

/// An answer whose thoughts are shown, then its text.
fn answer(text: &str, finish: &str) -> Value {
    let usage = json!({
        "promptTokenCount": 400,
        "cachedContentTokenCount": 100,
        "candidatesTokenCount": 30,
        "thoughtsTokenCount": 70,
        "totalTokenCount": 500
    });
    candidate(json!([{ "text": "thinking…", "thought": true }, { "text": text }]), finish, usage)
}

#[tokio::test]
async fn a_plain_answer_and_the_request_behind_it() {
    let mock = Mock::start(|_, _| Answer::json(200, answer("Chợ nổi Cái Răng.", "STOP")));
    let mut request = Request::new("gemini-4-carbon", "Chợ nào?").with_system("Trả lời ngắn.").with_temperature(0.2);
    request.messages.push(Message::assistant("Chợ nổi."));
    request.messages.push(Message::user("Tên đầy đủ?"));
    let reply = provider(&mock).chat(&request).await.unwrap();
    assert_eq!(reply.text, "Chợ nổi Cái Răng.", "thoughts are not the answer");
    assert_eq!(reply.stop, StopReason::End);
    assert_eq!(reply.usage, Usage { input: 300, cached: 100, cache_written: 0, output: 100 });

    let seen = &mock.seen()[0];
    assert_eq!((seen.method.as_str(), seen.path.as_str()), ("POST", "/v1beta/models/gemini-4-carbon:generateContent"));
    assert_eq!(seen.header("x-goog-api-key"), Some(FAKE_KEY));
    assert!(!seen.path.contains("key="), "the key never goes in the URL");
    let body = seen.json();
    assert_eq!(body["systemInstruction"], json!({ "parts": [{ "text": "Trả lời ngắn." }] }));
    assert_eq!(body["contents"][1], json!({ "role": "model", "parts": [{ "text": "Chợ nổi." }] }));
    assert_eq!(body["contents"][2]["role"], "user");
    assert_eq!(body["generationConfig"], json!({ "maxOutputTokens": 16_000 }));
}

#[tokio::test]
async fn a_structured_answer_and_a_picture_in_the_question() {
    let mock = Mock::start(|_, _| Answer::json(200, answer("{\"name\": \"cốc\"}", "STOP")));
    let schema = json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] });
    let mut request = Request::new("gemini-4-carbon", "unused").with_schema(schema.clone());
    request.messages = vec![Message::user_with_pictures("Đây là gì?", vec![Picture::new(jpeg(3, 3)).unwrap()])];
    let reply = provider(&mock).chat(&request).await.unwrap();
    assert_eq!(reply.json, Some(json!({ "name": "cốc" })));
    let body = mock.seen()[0].json();
    assert_eq!(body["generationConfig"]["responseMimeType"], "application/json");
    assert_eq!(body["generationConfig"]["responseJsonSchema"], schema);
    let parts = &body["contents"][0]["parts"];
    assert_eq!(parts[0]["inlineData"], json!({ "mimeType": "image/jpeg", "data": base64(&jpeg(3, 3)) }));
    assert_eq!(parts[1]["text"], "Đây là gì?");
}

#[tokio::test]
async fn a_streamed_answer_arrives_in_pieces() {
    let chunk = |text: &str, finish: Option<&str>| {
        let mut candidate = json!({ "content": { "role": "model", "parts": [{ "text": text }] }, "index": 0 });
        if let Some(finish) = finish {
            candidate["finishReason"] = json!(finish);
        }
        let usage = json!({ "promptTokenCount": 20, "candidatesTokenCount": 9 });
        ("", json!({ "candidates": [candidate], "usageMetadata": usage, "modelVersion": "gemini-4-carbon" }))
    };
    let stream = sse(&[chunk("Bà Tư chèo ", None), chunk("xuồng ra chợ.", Some("STOP"))]);
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let mut pieces = Vec::new();
    let mut on_text = |piece: &str| pieces.push(piece.to_string());
    let reply = provider(&mock).chat_streaming(&Request::new("gemini-4-carbon", "Kể"), &mut on_text).await.unwrap();
    assert_eq!(pieces, vec!["Bà Tư chèo ", "xuồng ra chợ."]);
    assert_eq!(reply.text, "Bà Tư chèo xuồng ra chợ.");
    assert_eq!(reply.usage, Usage { input: 20, cached: 0, cache_written: 0, output: 9 });
    assert_eq!(mock.seen()[0].path, "/v1beta/models/gemini-4-carbon:streamGenerateContent?alt=sse");
}

#[tokio::test]
async fn a_long_answer_is_continued_with_its_token() {
    let mock = Mock::start(|seen, _| match seen.json()["continuationToken"].as_str() {
        None => {
            let usage = json!({ "promptTokenCount": 100, "candidatesTokenCount": 10, "thoughtsTokenCount": 32_000 });
            let mut first = candidate(json!([{ "text": "Phần một. " }]), "CONTINUATION", usage);
            first["candidates"][0]["continuationToken"] = json!("tok-1");
            Answer::json(200, first)
        }
        Some("tok-1") => {
            let usage = json!({ "promptTokenCount": 100, "cachedContentTokenCount": 95, "candidatesTokenCount": 12, "thoughtsTokenCount": 0 });
            Answer::json(200, candidate(json!([{ "text": "Phần hai." }]), "STOP", usage))
        }
        Some(other) => panic!("unexpected token {other}"),
    });
    let reply = provider(&mock).chat(&Request::new("gemini-4-carbon", "Viết dài").with_max_tokens(20_000)).await.unwrap();
    assert_eq!(reply.text, "Phần một. Phần hai.");
    assert_eq!(reply.stop, StopReason::End);
    // Each request is counted: 100 + 5 at full price, 95 from the cache.
    assert_eq!(reply.usage, Usage { input: 105, cached: 95, cache_written: 0, output: 32_022 });
    let seen = mock.seen();
    assert_eq!(seen.len(), 2);
    // The same request both times, with the token added.
    assert_eq!(seen[0].path, "/v1beta/models/gemini-4-carbon:generateContent");
    assert_eq!(seen[0].json()["generationConfig"]["maxOutputTokens"], 20_000);
    assert_eq!(seen[1].json()["contents"], seen[0].json()["contents"]);
    assert_eq!(seen[1].json()["generationConfig"], seen[0].json()["generationConfig"]);
}

#[tokio::test]
async fn a_longer_answer_is_streamed_underneath_and_continued_too() {
    let hop = |text: &str, finish: &str, token: Option<&str>| {
        let mut candidate = json!({ "content": { "role": "model", "parts": [{ "text": text }] }, "finishReason": finish });
        if let Some(token) = token {
            candidate["continuationToken"] = json!(token);
        }
        sse(&[("", json!({ "candidates": [candidate], "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 } }))])
    };
    let mock = Mock::start(move |seen, _| match seen.json()["continuationToken"].as_str() {
        None => Answer::events(&hop("Chặng một. ", "CONTINUATION", Some("tok-9"))),
        Some(_) => Answer::events(&hop("Chặng hai.", "STOP", None)),
    });
    // Allowed more than one request returns when not streamed: the caller
    // waits for the whole answer, and it is streamed underneath.
    let reply = provider(&mock).chat(&Request::new("gemini-4-carbon", "Viết rất dài").with_max_tokens(200_000)).await.unwrap();
    assert_eq!(reply.text, "Chặng một. Chặng hai.");
    assert_eq!(reply.usage, Usage { input: 20, cached: 0, cache_written: 0, output: 10 });
    let seen = mock.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|request| request.path.ends_with(":streamGenerateContent?alt=sse")));
    assert_eq!(seen[0].json()["generationConfig"]["maxOutputTokens"], 200_000);
    assert_eq!(seen[1].json()["continuationToken"], "tok-9");
}

#[tokio::test]
async fn a_blocked_prompt_and_a_safety_stop_are_refusals() {
    let mock = Mock::start(|_, number| match number {
        0 => Answer::json(200, json!({ "promptFeedback": { "blockReason": "PROHIBITED_CONTENT" }, "usageMetadata": { "promptTokenCount": 8 } })),
        _ => Answer::json(200, answer("", "SAFETY")),
    });
    let provider = provider(&mock);
    let blocked = provider.chat(&Request::new("gemini-4-carbon", "…")).await.unwrap();
    assert!(matches!(&blocked.stop, StopReason::Refusal { category: Some(category), .. } if category == "prohibited_content"));
    let stopped = provider.chat(&Request::new("gemini-4-carbon", "…")).await.unwrap();
    assert!(matches!(&stopped.stop, StopReason::Refusal { category: Some(category), .. } if category == "safety"));
}

#[tokio::test]
async fn the_model_list_is_read_page_by_page_and_kept_to_generating_models() {
    let mock = Mock::start(|seen, _| {
        if seen.path.contains("pageToken=") {
            assert!(seen.path.ends_with("pageToken=next%2Bpage%3D"), "{}", seen.path);
            let methods = json!(["generateContent"]);
            let image = json!({ "name": "models/gemini-3-pro-image", "displayName": "Nano Banana Pro", "supportedGenerationMethods": methods });
            Answer::json(200, json!({ "models": [image] }))
        } else {
            let carbon = json!({
                "name": "models/gemini-4-carbon",
                "displayName": "Gemini 4 Carbon",
                "inputTokenLimit": 1_048_576,
                "outputTokenLimit": 65_536,
                "supportedGenerationMethods": ["generateContent", "countTokens"]
            });
            let (methods, name) = (json!(["embedContent"]), "Gemini Embedding 2");
            let embedding = json!({ "name": "models/gemini-embedding-2", "displayName": name, "supportedGenerationMethods": methods });
            Answer::json(200, json!({ "models": [carbon, embedding], "nextPageToken": "next+page=" }))
        }
    });
    let models = provider(&mock).models().await.unwrap();
    let found: Vec<(&str, &str, ModelKind)> = models.iter().map(|model| (model.id.as_str(), model.name.as_str(), model.kind)).collect();
    let expected = vec![("gemini-4-carbon", "Gemini 4 Carbon", ModelKind::Text), ("gemini-3-pro-image", "Nano Banana Pro", ModelKind::Image)];
    assert_eq!(found, expected);
    assert_eq!(models[0].context_tokens, Some(1_048_576));
    assert_eq!(mock.seen()[0].path, "/v1beta/models?pageSize=1000");
    assert_eq!(mock.seen()[0].header("x-goog-api-key"), Some(FAKE_KEY));
}

#[tokio::test]
async fn a_picture_comes_back_as_png_whatever_its_format() {
    let made = jpeg(5, 4);
    let encoded = base64(&made);
    let mock = Mock::start(move |_, _| {
        let parts = json!([{ "text": "Đây là ảnh." }, { "inlineData": { "mimeType": "image/jpeg", "data": encoded } }]);
        let mut made = candidate(parts, "STOP", json!({ "promptTokenCount": 600, "candidatesTokenCount": 1120 }));
        made["modelVersion"] = json!("gemini-3-pro-image");
        Answer::json(200, made)
    });
    let person = Picture::new(common::png(2, 2)).unwrap();
    let request = ImageRequest { model: "gemini-3-pro-image".into(), prompt: "Người này mặc áo kia".into(), pictures: vec![person] };
    let image = provider(&mock).generate_image(&request).await.unwrap();
    assert!(image.png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let decoded = image::load_from_memory(&image.png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (5, 4));
    assert_eq!(image.text, "Đây là ảnh.");
    assert_eq!(image.usage.output, 1120);
    let body = mock.seen()[0].json();
    assert_eq!(body["generationConfig"]["responseModalities"], json!(["TEXT", "IMAGE"]));
    assert_eq!(body["contents"][0]["parts"][0]["inlineData"]["mimeType"], "image/png");
    assert_eq!(body["contents"][0]["parts"][1]["text"], "Người này mặc áo kia");
}

#[tokio::test]
async fn a_picture_refused_for_safety_is_a_refusal() {
    let mock = Mock::start(|_, _| Answer::json(200, json!({ "candidates": [{ "content": { "parts": [] }, "finishReason": "IMAGE_SAFETY" }] })));
    let request = ImageRequest { model: "gemini-3-pro-image".into(), prompt: "…".into(), pictures: vec![] };
    let error = provider(&mock).generate_image(&request).await.unwrap_err();
    assert!(matches!(&error, LlmError::Refused { category, .. } if category.as_deref() == Some("image_safety")), "{error:?}");
}

#[tokio::test]
async fn googles_answer_to_a_wrong_key_is_a_refused_key() {
    let mock = Mock::start(|_, _| {
        let details = json!([{ "@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "API_KEY_INVALID" }]);
        let message = "API key not valid. Please pass a valid API key.";
        Answer::json(400, json!({ "error": { "code": 400, "message": message, "status": "INVALID_ARGUMENT", "details": details } }))
    });
    let error = provider(&mock).chat(&Request::new("gemini-4-carbon", "…")).await.unwrap_err();
    assert!(matches!(error, LlmError::KeyRefused { .. }), "{error:?}");
}
