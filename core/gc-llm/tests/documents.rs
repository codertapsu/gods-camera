//! The document workflows against a stand-in model on 127.0.0.1 that
//! answers by rule: clean-up, summary, translation, extraction and
//! questions, long documents cut into pieces, and NFC everywhere.

mod common;

use std::sync::{Arc, Mutex};

use common::{quick, scratch, Answer, Mock, Seen};
use gc_llm::{ApiKey, ChangeKind, DocOptions, Feature, Gateway, Language, LlmError, Provider, Target};
use serde_json::{json, Value};

/// A document as the text reader gives it, with its usual mistakes.
const SCANNED: &str = "Chợ nổi buổi sớm\n\n\
                       Giá hôm nay: 25.000 dong một ký, rẻ hơn tuần trước.\n\n\
                       Người bán ở Viet Nam treo hàng lên cây bẹo.";
const CLEAN: &str = "Chợ nổi buổi sớm\n\n\
                     Giá hôm nay: 25.000 đồng một ký, rẻ hơn tuần trước.\n\n\
                     Người bán ở Việt Nam treo hàng lên cây bẹo.";

fn completion(content: &str) -> Answer {
    let choice = json!({ "message": { "role": "assistant", "content": content }, "finish_reason": "stop" });
    Answer::json(200, json!({ "model": "local-model", "choices": [choice], "usage": { "prompt_tokens": 10, "completion_tokens": 10 } }))
}

/// The stretches of a diff of one kind.
fn of_kind(result: &gc_llm::CleanUp, kind: ChangeKind) -> Vec<&str> {
    result.diff.changes.iter().filter(|change| change.kind == kind).map(|change| change.text.as_str()).collect()
}

fn system_of(seen: &Seen) -> String {
    seen.json()["messages"][0]["content"].as_str().unwrap_or("").to_string()
}

fn user_of(seen: &Seen) -> String {
    let body = seen.json();
    body["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap_or("").to_string()
}

/// A gateway with the stand-in model as a local server called `local`.
fn gateway(tag: &str, mock: &Mock) -> (Gateway, std::path::PathBuf) {
    let folder = scratch(tag);
    let gateway = Gateway::open(&folder).unwrap();
    gateway.set_provider(Provider::compatible("local", &mock.url("/v1"), None).unwrap().with_http(quick()));
    (gateway, folder)
}

fn target() -> Target {
    Target::new("local", "local-model")
}

/// The stand-in's clean-up: it puts the tone marks back, and writes "Việt"
/// with its marks as separate characters, as some models do.
fn fixed(text: &str) -> String {
    text.replace("dong", "đồng").replace("Viet", "Vie\u{0323}\u{0302}t")
}

#[test]
fn a_clean_up_fixes_what_the_reader_got_wrong_and_shows_it() {
    let mock = Mock::start(|seen, _| completion(&fixed(&user_of(seen))));
    let (gateway, folder) = gateway("clean", &mock);
    let result = gateway.clean_up(&target(), SCANNED, &DocOptions::default(), None).unwrap();
    assert_eq!(result.text, CLEAN, "fixed, and in NFC");
    assert_eq!((result.diff.words_removed, result.diff.words_added, result.diff.breaks_changed), (2, 2, 0));
    assert!(!result.flagged, "{}", result.share_changed);
    assert_eq!(of_kind(&result, ChangeKind::Removed), vec![" dong", " Viet"]);
    assert_eq!(of_kind(&result, ChangeKind::Added), vec![" đồng", " Việt"]);

    let seen = mock.seen();
    assert_eq!(seen.len(), 1, "a short document goes in one piece");
    assert!(system_of(&seen[0]).starts_with("You correct the mistakes that text recognition (OCR) made"));
    assert_eq!(user_of(&seen[0]), SCANNED);
    assert_eq!(seen[0].json()["temperature"], 0.0);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_clean_up_that_rewrites_or_joins_is_flagged() {
    let mock = Mock::start(|seen, number| {
        let text = user_of(seen);
        match number {
            // Rewritten: every other word changed.
            0 => {
                let words = text.split_whitespace().enumerate();
                let rewritten: Vec<String> =
                    words.map(|(index, word)| if index % 2 == 0 { word.to_uppercase() } else { word.to_string() }).collect();
                completion(&rewritten.join(" "))
            }
            // Paragraphs joined into one.
            _ => completion(&fixed(&text).replace("\n\n", " ")),
        }
    });
    let (gateway, folder) = gateway("flag", &mock);
    let rewritten = gateway.clean_up(&target(), SCANNED, &DocOptions::default(), None).unwrap();
    assert!(rewritten.flagged && rewritten.share_changed > 0.3, "{}", rewritten.share_changed);
    let joined = gateway.clean_up(&target(), SCANNED, &DocOptions::default(), None).unwrap();
    assert!(joined.flagged);
    assert_eq!(joined.diff.breaks_changed, 2);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_long_document_goes_in_pieces_with_progress_and_can_be_stopped() {
    let paragraphs: Vec<String> = (1..=12).map(|n| format!("Đoạn {n}: giá {n}.000 dong một ký ở chợ Viet.")).collect();
    let document = paragraphs.join("\n\n");
    let mock = Mock::start(|seen, _| completion(&fixed(&user_of(seen))));
    let (gateway, folder) = gateway("pieces", &mock);
    let options = DocOptions { piece_tokens: Some(60), ..DocOptions::default() };
    let mut steps = Vec::new();
    let mut progress = |done: usize, total: usize| {
        steps.push((done, total));
        true
    };
    let result = gateway.clean_up(&target(), &document, &options, Some(&mut progress)).unwrap();
    let pieces = mock.seen().len();
    assert!(pieces > 3, "{pieces} pieces");
    assert_eq!(result.text, gc_llm::text::nfc(&fixed(&document)));
    assert_eq!(result.diff.words_removed, 24);
    assert_eq!(steps.first(), Some(&(0, pieces)));
    assert_eq!(steps.last(), Some(&(pieces, pieces)));
    // Every piece is whole paragraphs.
    for seen in mock.seen() {
        assert!(user_of(&seen).split("\n\n").all(|paragraph| paragraphs.iter().any(|whole| whole == paragraph)));
    }

    let before = mock.seen().len();
    let mut stop_after_one = |done: usize, _: usize| done < 1;
    let stopped = gateway.clean_up(&target(), &document, &options, Some(&mut stop_after_one));
    assert_eq!(stopped.unwrap_err(), LlmError::Cancelled);
    assert_eq!(mock.seen().len(), before + 1);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_long_summary_is_made_from_the_parts_summaries() {
    let mock = Mock::start(|seen, _| {
        let system = system_of(seen);
        let user = user_of(seen);
        if system.starts_with("You are given summaries") {
            completion("Tóm tắt chung của cả tài liệu.")
        } else {
            let part = user.split_whitespace().nth(1).unwrap_or("?").to_string();
            completion(&format!("Tóm tắt phần {part}."))
        }
    });
    let (gateway, folder) = gateway("summary", &mock);
    let long: Vec<String> = (1..=10).map(|n| format!("Đoạn {n} kể chuyện chợ nổi và ghe xuồng.")).collect();
    let options = DocOptions { piece_tokens: Some(70), ..DocOptions::default() };
    let summary = gateway.summarise(&target(), &long.join("\n\n"), Language::Vietnamese, &options, None).unwrap();
    assert_eq!(summary, "Tóm tắt chung của cả tài liệu.");
    let seen = mock.seen();
    assert!(seen.len() >= 3);
    assert!(user_of(&seen[0]).starts_with("Part 1 of "));
    assert!(seen[..seen.len() - 1].iter().all(|part| system_of(part).contains("Write the summary in Vietnamese")));
    let last = seen.last().unwrap();
    assert!(system_of(last).starts_with("You are given summaries"));
    assert!(user_of(last).contains("Part 1:\nTóm tắt phần 1.") && user_of(last).contains("Part 2:"), "{}", user_of(last));

    // A short document is summarised in one request.
    let single = gateway.summarise(&target(), SCANNED, Language::English, &DocOptions::default(), None).unwrap();
    assert!(single.starts_with("Tóm tắt phần"), "{single}");
    assert!(system_of(mock.seen().last().unwrap()).contains("Write the summary in English"));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_translation_keeps_the_paragraphs() {
    let mock = Mock::start(|seen, _| {
        assert!(system_of(seen).contains("into English"));
        completion(&user_of(seen).split("\n\n").map(|paragraph| format!("EN({paragraph})")).collect::<Vec<_>>().join("\n\n"))
    });
    let (gateway, folder) = gateway("translate", &mock);
    let options = DocOptions { piece_tokens: Some(30), ..DocOptions::default() };
    let english = gateway.translate(&target(), CLEAN, Language::English, &options, None).unwrap();
    let expected: Vec<String> = CLEAN.split("\n\n").map(|paragraph| format!("EN({paragraph})")).collect();
    assert_eq!(english, expected.join("\n\n"));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn values_are_extracted_to_the_schema_and_merged_across_pieces() {
    let schema = json!({
        "type": "object",
        "properties": { "prices": { "type": "array", "items": { "type": "string" } } },
        "required": ["prices"], "additionalProperties": false
    });
    let mock = Mock::start(|seen, _| {
        let body = seen.json();
        assert_eq!(body["response_format"]["type"], "json_schema");
        if system_of(seen).starts_with("You are given values") {
            let user = user_of(seen);
            let parts: Value = serde_json::from_str(user.split_once("\n\n").unwrap().1).unwrap();
            let all: Vec<Value> = parts.as_array().unwrap().iter().flat_map(|part| part["prices"].as_array().unwrap().clone()).collect();
            completion(&json!({ "prices": all }).to_string())
        } else {
            let prices: Vec<String> = user_of(seen).split_whitespace().filter(|word| word.ends_with(".000")).map(str::to_string).collect();
            completion(&json!({ "prices": prices }).to_string())
        }
    });
    let (gateway, folder) = gateway("extract", &mock);
    let one = gateway.extract(&target(), CLEAN, &schema, &DocOptions::default(), None).unwrap();
    assert_eq!(one, json!({ "prices": ["25.000"] }));
    assert_eq!(mock.seen().len(), 1);

    let document: String = (1..=6).map(|n| format!("Ngày {n}: giá {n}0.000 đồng.")).collect::<Vec<_>>().join("\n\n");
    let options = DocOptions { piece_tokens: Some(25), ..DocOptions::default() };
    let merged = gateway.extract(&target(), &document, &schema, &options, None).unwrap();
    assert_eq!(merged, json!({ "prices": ["10.000", "20.000", "30.000", "40.000", "50.000", "60.000"] }));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn an_answer_names_its_paragraphs_and_quotes_them_from_the_document() {
    let mock = Mock::start(|seen, _| {
        let system = system_of(seen);
        assert!(system.contains("<document>") && system.contains("[2] Giá hôm nay"), "{system}");
        assert_eq!(user_of(seen), "Giá bao nhiêu?");
        assert_eq!(seen.json()["response_format"]["json_schema"]["schema"]["required"], json!(["answer", "found", "paragraphs"]));
        // Paragraph 7 does not exist.
        completion(r#"{"answer": "25.000 đồng một ký.", "found": true, "paragraphs": [2, 7]}"#)
    });
    let (gateway, folder) = gateway("ask", &mock);
    let answer = gateway.ask(&target(), CLEAN, "Giá bao nhiêu?", &DocOptions::default(), None).unwrap();
    assert!(answer.found);
    assert_eq!(answer.text, "25.000 đồng một ký.");
    assert_eq!(answer.paragraphs, vec![2]);
    assert_eq!(answer.quotes, vec!["Giá hôm nay: 25.000 đồng một ký, rẻ hơn tuần trước.".to_string()]);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn answers_from_several_pieces_are_combined() {
    let combined_once = Arc::new(Mutex::new(0));
    let counter = combined_once.clone();
    let mock = Mock::start(move |seen, _| {
        let system = system_of(seen);
        if system.starts_with("You are given answers") {
            *counter.lock().unwrap() += 1;
            // 9 was not given by any part, so it is dropped.
            return completion(r#"{"answer": "Có hai giá: 10.000 và 50.000 đồng.", "found": true, "paragraphs": [1, 5, 9]}"#);
        }
        assert!(system.contains("This is one part of a longer document"));
        if system.contains("[1] ") && system.contains("10.000") {
            completion(r#"{"answer": "10.000 đồng.", "found": true, "paragraphs": [1]}"#)
        } else if system.contains("[5] ") {
            completion(r#"{"answer": "50.000 đồng.", "found": true, "paragraphs": [5]}"#)
        } else {
            completion(r#"{"answer": "Phần này không nói.", "found": false, "paragraphs": []}"#)
        }
    });
    let (gateway, folder) = gateway("ask-pieces", &mock);
    let document: Vec<String> = (1..=6).map(|n| format!("Ngày {n}: giá {n}0.000 đồng.")).collect();
    let options = DocOptions { piece_tokens: Some(20), ..DocOptions::default() };
    let answer = gateway.ask(&target(), &document.join("\n\n"), "Giá bao nhiêu?", &options, None).unwrap();
    assert_eq!(answer.text, "Có hai giá: 10.000 và 50.000 đồng.");
    assert_eq!(answer.paragraphs, vec![1, 5]);
    assert_eq!(answer.quotes, vec![document[0].clone(), document[4].clone()]);
    assert_eq!(*combined_once.lock().unwrap(), 1);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_question_the_document_does_not_answer() {
    let mock = Mock::start(|_, _| completion(r#"{"answer": "Tài liệu không nói điều này.", "found": false, "paragraphs": [1]}"#));
    let (gateway, folder) = gateway("not-found", &mock);
    let answer = gateway.ask(&target(), CLEAN, "Ai là tác giả?", &DocOptions::default(), None).unwrap();
    assert!(!answer.found);
    assert!(answer.paragraphs.is_empty() && answer.quotes.is_empty());
    assert_eq!(answer.text, "Tài liệu không nói điều này.");
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_decomposed_document_is_sent_and_returned_composed() {
    let mock = Mock::start(|seen, _| completion(&user_of(seen)));
    let (gateway, folder) = gateway("nfc", &mock);
    let decomposed = "Vie\u{0323}\u{0302}t Nam\n\nNu\u{031b}o\u{031b}\u{0301}c";
    let result = gateway.translate(&target(), decomposed, Language::Vietnamese, &DocOptions::default(), None).unwrap();
    assert_eq!(result, "Việt Nam\n\nNước");
    assert_eq!(user_of(&mock.seen()[0]), "Việt Nam\n\nNước");
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_workflow_keeps_to_the_cloud_switch() {
    let mock = Mock::start(|_, _| completion("x"));
    let folder = scratch("doc-switch");
    let gateway = Gateway::open(&folder).unwrap();
    let anthropic = Provider::anthropic(Some(ApiKey::new("sk-ant-fake-key-for-tests-0000").unwrap()));
    gateway.set_provider(anthropic.with_base_url(&mock.url("/v1")).unwrap());
    let target = Target::new("anthropic", "claude-opus-5-5");
    let error = gateway.clean_up(&target, SCANNED, &DocOptions::default(), None).unwrap_err();
    assert_eq!(error, LlmError::CloudNotAllowed { feature: Feature::CleanUp, provider: "anthropic".into() });
    let asked = gateway.ask(&target, SCANNED, "?", &DocOptions::default(), None);
    assert!(matches!(asked, Err(LlmError::CloudNotAllowed { feature: Feature::Questions, .. })));
    assert!(mock.seen().is_empty());
    assert!(matches!(gateway.clean_up(&target, " \n\n ", &DocOptions::default(), None), Err(LlmError::CloudNotAllowed { .. })));
    let _ = std::fs::remove_dir_all(folder);
}
