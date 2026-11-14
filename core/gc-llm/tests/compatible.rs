//! A server with OpenAI's Chat Completions interface, as Ollama, LM Studio
//! and llama.cpp's server offer it, against a stand-in.

mod common;

use common::{quick, sse, Answer, Mock};
use gc_llm::{ApiKey, LlmError, Provider, Request, StopReason, Usage};
use serde_json::{json, Value};

fn provider(mock: &Mock, key: Option<ApiKey>) -> Provider {
    Provider::compatible("ollama", &mock.url("/v1"), key).unwrap().with_http(quick())
}

fn completion(content: &str, finish: &str) -> Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "qwen2.5:7b",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": finish }],
        "usage": { "prompt_tokens": 40, "completion_tokens": 12, "total_tokens": 52 }
    })
}

/// One piece of a streamed answer.
fn chunk(delta: Value, finish: Option<&str>) -> (&'static str, Value) {
    let choice = json!({ "index": 0, "delta": delta, "finish_reason": finish });
    ("", json!({ "id": "c1", "object": "chat.completion.chunk", "model": "qwen2.5:7b", "choices": [choice] }))
}

#[tokio::test]
async fn a_plain_answer_without_a_key() {
    let mock = Mock::start(|_, _| Answer::json(200, completion("Xin chào!", "stop")));
    let provider = provider(&mock, None);
    assert!(provider.is_local());
    let reply = provider.chat(&Request::new("qwen2.5:7b", "Chào").with_system("Ngắn gọn.").with_temperature(0.0)).await.unwrap();
    assert_eq!((reply.text.as_str(), reply.stop.clone()), ("Xin chào!", StopReason::End));
    assert_eq!(reply.usage, Usage { input: 40, cached: 0, cache_written: 0, output: 12 });
    assert_eq!(reply.provider, "ollama");

    let seen = &mock.seen()[0];
    assert_eq!((seen.method.as_str(), seen.path.as_str()), ("POST", "/v1/chat/completions"));
    assert_eq!(seen.header("authorization"), None);
    let body = seen.json();
    assert_eq!(body["messages"], json!([{ "role": "system", "content": "Ngắn gọn." }, { "role": "user", "content": "Chào" }]));
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["max_tokens"], 16_000);
}

#[tokio::test]
async fn a_key_when_one_is_given() {
    let mock = Mock::start(|_, _| Answer::json(200, completion("OK", "stop")));
    let key = ApiKey::new("fake-local-server-key").unwrap();
    provider(&mock, Some(key)).chat(&Request::new("model", "x")).await.unwrap();
    assert_eq!(mock.seen()[0].header("authorization"), Some("Bearer fake-local-server-key"));
}

#[tokio::test]
async fn a_streamed_answer_with_its_counts_at_the_end() {
    let usage = json!({ "prompt_tokens": 15, "completion_tokens": 6 });
    let counts = json!({ "id": "c1", "object": "chat.completion.chunk", "choices": [], "usage": usage });
    let mut stream = sse(&[
        // A reasoning model's thinking comes in a field of its own, and is skipped.
        chunk(json!({ "role": "assistant", "reasoning": "hmm" }), None),
        chunk(json!({ "content": "Ghe " }), None),
        chunk(json!({ "content": "xuồng tấp nập." }), None),
        chunk(json!({}), Some("stop")),
        ("", counts),
    ]);
    stream.push_str("data: [DONE]\n\n");
    let mock = Mock::start(move |_, _| Answer::events(&stream));
    let mut pieces = Vec::new();
    let mut on_text = |piece: &str| pieces.push(piece.to_string());
    let reply = provider(&mock, None).chat_streaming(&Request::new("qwen2.5:7b", "Tả chợ"), &mut on_text).await.unwrap();
    assert_eq!(pieces, vec!["Ghe ", "xuồng tấp nập."]);
    assert_eq!(reply.text, "Ghe xuồng tấp nập.");
    assert_eq!(reply.stop, StopReason::End);
    assert_eq!(reply.usage, Usage { input: 15, cached: 0, cache_written: 0, output: 6 });
    let body = mock.seen()[0].json();
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({ "include_usage": true }));
}

#[tokio::test]
async fn json_in_a_fence_is_still_read() {
    let mock = Mock::start(|_, _| Answer::json(200, completion("```json\n{\"title\": \"Chợ nổi\"}\n```", "stop")));
    let schema = json!({
        "type": "object",
        "properties": { "title": { "type": "string" } },
        "required": ["title"],
        "additionalProperties": false
    });
    let reply = provider(&mock, None).chat(&Request::new("qwen2.5:7b", "Tiêu đề?").with_schema(schema.clone())).await.unwrap();
    assert_eq!(reply.json, Some(json!({ "title": "Chợ nổi" })));
    let format = json!({ "type": "json_schema", "json_schema": { "name": "result", "schema": schema, "strict": true } });
    assert_eq!(mock.seen()[0].json()["response_format"], format);
}

#[tokio::test]
async fn a_reasoning_models_thoughts_stay_out_of_the_answer() {
    let mock = Mock::start(|_, _| Answer::json(200, completion("<think>\nNgười dùng hỏi giá.\n</think>\n\n25.000 đồng.", "stop")));
    let reply = provider(&mock, None).chat(&Request::new("qwen3:8b", "Giá?")).await.unwrap();
    assert_eq!(reply.text, "25.000 đồng.");
}

#[tokio::test]
async fn the_model_list() {
    let mock = Mock::start(|_, _| {
        let model = |id: &str| json!({ "id": id, "object": "model", "created": 1, "owned_by": "library" });
        Answer::json(200, json!({ "object": "list", "data": [model("qwen2.5:7b"), model("gemma3:12b")] }))
    });
    let models = provider(&mock, None).models().await.unwrap();
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, vec!["gemma3:12b", "qwen2.5:7b"]);
    assert_eq!(mock.seen()[0].path, "/v1/models");
}

#[tokio::test]
async fn a_server_that_is_not_running_is_a_network_error() {
    // A port that was free a moment ago: nothing listens there.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut options = quick();
    options.retries = 0;
    let provider = Provider::compatible("lm-studio", &format!("http://127.0.0.1:{port}/v1"), None).unwrap().with_http(options);
    let error = provider.chat(&Request::new("model", "x")).await.unwrap_err();
    assert!(matches!(error, LlmError::Network { .. }), "{error:?}");
}

#[tokio::test]
async fn a_server_error_in_ollamas_words() {
    let message = "model \"llama9\" not found, try pulling it first";
    let mock = Mock::start(move |_, _| Answer::json(404, json!({ "error": { "message": message, "type": "api_error" } })));
    let error = provider(&mock, None).chat(&Request::new("llama9", "x")).await.unwrap_err();
    assert_eq!(error, LlmError::BadRequest { provider: "ollama".into(), status: 404, message: message.into() });
}
