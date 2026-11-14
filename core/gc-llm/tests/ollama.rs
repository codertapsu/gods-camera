//! A real local model, when Ollama runs on this computer: its list of models
//! and a short question to the first one. Run by hand:
//!
//! ```text
//! cargo test -p gc-llm --test ollama -- --ignored --nocapture
//! ```
//!
//! Passes with a note when nothing listens on 127.0.0.1:11434 or Ollama has
//! no model. Calls no cloud provider.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use gc_llm::{Feature, Gateway, Provider, Request, StopReason};

#[test]
#[ignore = "needs Ollama running on this computer"]
fn a_question_to_the_local_ollama() {
    let address: SocketAddr = "127.0.0.1:11434".parse().unwrap();
    if TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_err() {
        eprintln!("note: nothing listens on {address}; Ollama is not running, so there is nothing to test");
        return;
    }
    let folder = std::env::temp_dir().join(format!("gc-llm-ollama-{}", std::process::id()));
    let gateway = Gateway::open(&folder).unwrap();
    let ollama = Provider::ollama();
    assert!(ollama.is_local());
    gateway.set_provider(ollama);

    let models = gateway.models("ollama").expect("Ollama lists its models");
    let Some(model) = models.first() else {
        eprintln!("note: Ollama has no model; `ollama pull qwen2.5:7b` gives it one");
        let _ = std::fs::remove_dir_all(folder);
        return;
    };
    eprintln!("Ollama has {} model(s); asking {}", models.len(), model.id);

    // A local model needs no cloud switch.
    let request = Request::new(&model.id, "Trả lời bằng một từ: thủ đô của Việt Nam là gì?").with_max_tokens(400).with_temperature(0.0);
    let mut streamed = String::new();
    let reply = gateway.chat_streaming(Feature::Questions, "ollama", &request, &mut |piece| streamed.push_str(piece));
    let reply = reply.expect("Ollama answers");
    eprintln!("answer: {:?} ({:?}, {} tokens in, {} out)", reply.text, reply.stop, reply.usage.input, reply.usage.output);
    assert!(!reply.text.trim().is_empty() || reply.stop == StopReason::Length);
    assert_eq!(streamed.trim(), reply.text.trim());

    let today = gateway.today();
    let report = gateway.usage(&today, &today).unwrap();
    assert!(report.lines.iter().all(|line| line.local && line.cost == Some(0.0)));
    let _ = std::fs::remove_dir_all(folder);
}
