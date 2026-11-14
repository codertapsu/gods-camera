//! The assistant's model with a stand-in for a local model server on
//! 127.0.0.1, and its keys kept in a folder: no cloud provider is called.

use gc_app::assistant::{
    AiFeature, Assistant, AssistantProblem, CloudProvider, DocumentTask, ModelChoice, PictureTask,
    TaskState,
};
use gc_app::model::FaceConsent;
use gc_app::secrets::{FolderSecrets, Secrets};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A server that answers every chat request as OpenAI's Chat Completions do,
/// with `answer`, and counts the requests.
fn stand_in(answer: &'static str) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}/v1", listener.local_addr().unwrap());
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&count);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let streamed = String::from_utf8_lossy(&body).contains("\"stream\":true");
            let reply = if streamed {
                let chunk = serde_json::json!({ "choices": [{ "index": 0, "delta": { "content": answer }, "finish_reason": "stop" }] });
                (
                    "text/event-stream",
                    format!("data: {chunk}\n\ndata: [DONE]\n\n"),
                )
            } else {
                let whole = serde_json::json!({
                    "id": "x", "object": "chat.completion", "created": 1, "model": "local",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": answer }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 12, "completion_tokens": 4, "total_tokens": 16 }
                });
                ("application/json", whole.to_string())
            };
            let mut stream = stream;
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", reply.0, reply.1.len(), reply.1);
        }
    });
    (address, count)
}

/// A server that answers every request with the JSON `body`, and notes each
/// request's first line.
fn answering(body: String) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let noted = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut first, mut length, mut line) = (String::new(), 0usize, String::new());
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if first.is_empty() {
                    first = line.trim().to_string();
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut request = vec![0u8; length];
            let _ = reader.read_exact(&mut request);
            noted.lock().unwrap().push(first);
            let mut stream = stream;
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
        }
    });
    (address, seen)
}

fn folder(tag: &str) -> std::path::PathBuf {
    let folder = std::env::temp_dir().join(format!("gc-assistant-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn finished(assistant: &Assistant, id: u64) -> TaskState {
    let started = Instant::now();
    loop {
        let state = assistant.task(id);
        if !state.running {
            return state;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the task did not end: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn keys_are_kept_and_never_given_out() {
    let folder = folder("keys");
    let secrets = Arc::new(FolderSecrets::new(folder.join("Secrets")));
    let assistant = Assistant::with_secrets(&folder, secrets.clone(), 420).unwrap();
    let providers = assistant.providers();
    assert_eq!(
        providers
            .iter()
            .map(|provider| provider.name.as_str())
            .collect::<Vec<_>>(),
        ["anthropic", "openai", "gemini", "ollama"]
    );
    assert!(providers.iter().all(|provider| !provider.has_key));
    assert!(providers
        .iter()
        .find(|provider| provider.name == "ollama")
        .is_some_and(|ollama| ollama.local));

    assert!(
        assistant
            .set_key(CloudProvider::Anthropic, "   ".into())
            .is_some(),
        "an empty key is not kept"
    );
    assert_eq!(
        assistant.set_key(
            CloudProvider::Anthropic,
            "  sk-ant-test-0123456789  ".into()
        ),
        None
    );
    assert!(assistant
        .providers()
        .iter()
        .find(|provider| provider.name == "anthropic")
        .is_some_and(|anthropic| anthropic.has_key));
    // Kept as the user typed it, without the spaces around it, under the app's secrets.
    assert_eq!(
        secrets.read("Anthropic").unwrap().as_deref(),
        Some(&b"sk-ant-test-0123456789"[..])
    );
    // With a key, documents go to that provider's default model.
    assert_eq!(
        assistant.text_model().map(|choice| choice.provider),
        Some("anthropic".into())
    );

    // A new start finds the key again.
    let again = Assistant::with_secrets(&folder, secrets.clone(), 420).unwrap();
    assert!(again
        .providers()
        .iter()
        .find(|provider| provider.name == "anthropic")
        .is_some_and(|anthropic| anthropic.has_key));
    again.remove_key(CloudProvider::Anthropic);
    assert_eq!(secrets.read("Anthropic").unwrap(), None);
    assert!(again.text_model().is_none());
}

#[test]
fn the_cloud_stays_closed_until_the_user_opens_it_for_a_feature() {
    let folder = folder("switch");
    let assistant = Assistant::with_secrets(
        &folder,
        Arc::new(FolderSecrets::new(folder.join("Secrets"))),
        0,
    )
    .unwrap();
    assert!(
        !assistant.cloud_allowed(AiFeature::Summary) && !assistant.cloud_allowed(AiFeature::TryOn)
    );
    assistant.set_key(CloudProvider::OpenAi, "sk-test-0123456789".into());
    // Refused before anything is sent: no key leaves, no text leaves.
    let id = assistant.start_document(
        DocumentTask::Summary { english: true },
        "Một đoạn văn.".into(),
    );
    assert_eq!(
        finished(&assistant, id).problem,
        Some(AssistantProblem::CloudNotAllowed)
    );
    assistant.set_cloud_allowed(AiFeature::Summary, true);
    assert!(
        assistant.cloud_allowed(AiFeature::Summary)
            && !assistant.cloud_allowed(AiFeature::Translation)
    );
    // Without any key there is nothing to use.
    let bare = folder.join("bare");
    let bare =
        Assistant::with_secrets(&bare, Arc::new(FolderSecrets::new(bare.join("Secrets"))), 0)
            .unwrap();
    let id = bare.start_document(DocumentTask::Summary { english: true }, "Text.".into());
    assert_eq!(finished(&bare, id).problem, Some(AssistantProblem::NoKey));
}

#[test]
fn a_try_on_picture_is_kept_with_whose_photo_it_was_made_from() {
    use base64::Engine as _;
    let folder = folder("picture");
    let assistant = Assistant::with_secrets(
        &folder,
        Arc::new(FolderSecrets::new(folder.join("Secrets"))),
        0,
    )
    .unwrap();
    let mut made = Vec::new();
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 120, 40]))
        .write_to(
            &mut std::io::Cursor::new(&mut made),
            image::ImageFormat::Png,
        )
        .unwrap();
    let reply = serde_json::json!({ "created": 1, "data": [{ "b64_json": base64::engine::general_purpose::STANDARD.encode(&made) }], "output_format": "png" });
    let (address, seen) = answering(reply.to_string());
    // OpenAI with a key, at the stand-in's address.
    let key = gc_llm::ApiKey::new("sk-test-0123456789").unwrap();
    assistant.set_provider(
        gc_llm::Provider::openai(Some(key))
            .with_base_url(&address)
            .unwrap(),
    );
    assistant.set_image_model(Some(ModelChoice {
        provider: "openai".into(),
        model: "gpt-image-2.6-sunburst".into(),
    }));

    let photo = |name: &str, colour: [u8; 3]| {
        let path = folder.join(name);
        image::RgbImage::from_pixel(30, 40, image::Rgb(colour))
            .save(&path)
            .unwrap();
        path.to_string_lossy().into_owned()
    };
    let task = PictureTask::TryOn {
        person: photo("person.png", [90, 60, 50]),
        garment: photo("shirt.png", [40, 80, 200]),
        consent: FaceConsent::Permission,
    };
    // The photos leave only once the user allows the cloud for try-on.
    let id = assistant.start_picture(task.clone());
    assert_eq!(
        finished(&assistant, id).problem,
        Some(AssistantProblem::CloudNotAllowed)
    );
    assert!(seen.lock().unwrap().is_empty(), "nothing was sent");
    assistant.set_cloud_allowed(AiFeature::TryOn, true);

    let id = assistant.start_picture(task);
    let state = finished(&assistant, id);
    assert_eq!(state.problem, None);
    let picture = std::path::PathBuf::from(state.result.unwrap().picture.expect("a picture"));
    assert_eq!(
        std::fs::read(&picture).unwrap(),
        made,
        "the picture is the model's, as it came"
    );
    assert!(picture.starts_with(folder.join("Assistant").join("Pictures")));
    assert!(
        seen.lock().unwrap()[0].starts_with("POST /v1/images/edits"),
        "{:?}",
        seen.lock().unwrap()
    );
    // Whose photo it was is kept beside it.
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(picture.with_extension("json")).unwrap()).unwrap();
    assert_eq!(record["made"], "try_on");
    assert_eq!(record["consent"], "the user has the person's permission");
    assert_eq!(record["model"], "openai/gpt-image-2.6-sunburst");
}

#[test]
fn garments_are_kept_tagged_and_taken_out_again() {
    let folder = folder("wardrobe");
    let assistant = Assistant::with_secrets(
        &folder,
        Arc::new(FolderSecrets::new(folder.join("Secrets"))),
        0,
    )
    .unwrap();
    let (address, _) = stand_in(
        r#"{"name":"Blue linen shirt","category":"Shirt","colours":["blue","white"],"material":"linen","season":"summer"}"#,
    );
    assistant.set_provider(gc_llm::Provider::compatible("ollama", &address, None).unwrap());
    assistant.set_text_model(Some(ModelChoice {
        provider: "ollama".into(),
        model: "llava".into(),
    }));

    let photo = folder.join("shirt.png");
    image::RgbImage::from_pixel(40, 50, image::Rgb([40, 80, 200]))
        .save(&photo)
        .unwrap();
    assert_eq!(
        assistant.add_to_wardrobe(photo.to_string_lossy().into_owned()),
        None
    );
    let not_a_picture = folder.join("notes.txt");
    std::fs::write(&not_a_picture, "text").unwrap();
    assert!(assistant
        .add_to_wardrobe(not_a_picture.to_string_lossy().into_owned())
        .is_some());

    // The photo is kept in the app's data, and the model's description arrives.
    let started = Instant::now();
    let item = loop {
        let items = assistant.wardrobe();
        assert_eq!(items.len(), 1);
        if !items[0].name.is_empty() {
            break items[0].clone();
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "no description came"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(item.name, "Blue linen shirt");
    assert_eq!(item.tags, ["Shirt", "blue", "white", "linen", "summer"]);
    assert!(
        std::path::Path::new(&item.picture).starts_with(&folder)
            && std::path::Path::new(&item.picture).is_file()
    );

    assistant.remove_from_wardrobe(item.id.clone());
    assert!(assistant.wardrobe().is_empty());
    assert!(
        !std::path::Path::new(&item.picture).exists(),
        "the photo goes with it"
    );
}

#[test]
fn a_local_model_does_the_document_work_on_this_computer() {
    let folder = folder("local");
    let assistant = Assistant::with_secrets(
        &folder,
        Arc::new(FolderSecrets::new(folder.join("Secrets"))),
        0,
    )
    .unwrap();
    let (address, requests) = stand_in("Chợ nổi buổi sớm, tấp nập ghe xuồng.");
    assistant.set_provider(gc_llm::Provider::compatible("ollama", &address, None).unwrap());
    assistant.set_text_model(Some(ModelChoice {
        provider: "ollama".into(),
        model: "qwen2.5:7b".into(),
    }));
    let text =
        "Chợ nổi buổi sớm\n\nTrời vừa hửng sáng, ghe xuồng đã tấp nập trên sông.".to_string();

    // A local model needs no cloud switch: nothing leaves the computer.
    let id = assistant.start_document(DocumentTask::Summary { english: false }, text.clone());
    let state = finished(&assistant, id);
    assert_eq!(state.problem, None);
    let result = state.result.unwrap();
    assert_eq!(result.text, "Chợ nổi buổi sớm, tấp nập ghe xuồng.");
    assert_eq!(result.model, "ollama/qwen2.5:7b");
    assert!(requests.load(std::sync::atomic::Ordering::Relaxed) >= 1);

    // A clean-up comes back with its changes, word by word, against the original.
    let (address, _) =
        stand_in("Chợ nổi buổi sớm\n\nTrời vừa hửng sáng, ghe xuồng đã tấp nập trên sông.");
    assistant.set_provider(gc_llm::Provider::compatible("ollama", &address, None).unwrap());
    let id = assistant.start_document(
        DocumentTask::CleanUp,
        "Chơ nổi buổi sớm\n\nTrời vừa hửng sáng, ghe xuồng đã tấp nập trên sông.".into(),
    );
    let result = finished(&assistant, id).result.expect("a clean-up");
    assert!(
        result
            .changes
            .iter()
            .any(|change| change.kind == "removed" && change.text.contains("Chơ")),
        "{:?}",
        result.changes
    );
    assert!(
        result
            .changes
            .iter()
            .any(|change| change.kind == "added" && change.text.contains("Chợ")),
        "{:?}",
        result.changes
    );
    assert!(!result.flagged);

    // Photos read again by a model that sees pictures: one request a photo, the pages joined.
    let (address, requests) = stand_in("Viết tay: hẹn gặp lúc 9 giờ.");
    assistant.set_provider(gc_llm::Provider::compatible("ollama", &address, None).unwrap());
    let photos: Vec<String> = (0..2)
        .map(|n| {
            let path = folder.join(format!("page-{n}.png"));
            image::RgbImage::from_pixel(32, 24, image::Rgb([250, 250, 250]))
                .save(&path)
                .unwrap();
            path.to_string_lossy().into_owned()
        })
        .collect();
    let id = assistant.start_document(DocumentTask::ReadPhotos { photos }, String::new());
    let state = finished(&assistant, id);
    assert_eq!(state.problem, None);
    assert_eq!(
        state.result.unwrap().text,
        "Viết tay: hẹn gặp lúc 9 giờ.\n\nViết tay: hẹn gặp lúc 9 giờ."
    );
    assert_eq!(requests.load(std::sync::atomic::Ordering::Relaxed), 2);
    // A photo that is not a picture is said, not sent.
    let id = assistant.start_document(
        DocumentTask::ReadPhotos {
            photos: vec![folder.join("missing.png").to_string_lossy().into_owned()],
        },
        String::new(),
    );
    assert!(matches!(
        finished(&assistant, id).problem,
        Some(AssistantProblem::Other { .. })
    ));

    // What it cost is counted, and a local model costs nothing.
    let usage = assistant.usage();
    assert!(
        usage.today_requests >= 4 && usage.today_cost == 0.0,
        "{usage:?}"
    );
    assistant.forget(id);
    assert!(
        assistant.task(id).problem.is_some(),
        "a forgotten task is gone"
    );
}
