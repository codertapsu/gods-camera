//! `gc mcp` as an assistant starts it: JSON-RPC on standard input and output,
//! over a hub folder laid out as the desktop app's.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use gc_doc::{OcrLine, OcrPage};
use serde_json::{json, Value};

const DOCUMENT: &str = "2f0c6b1e-6c1d-4b8e-9d7a-0e5b2a9c1f00";

struct Server {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Server {
    fn start(hub: &std::path::Path) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_gc"))
            .args(["mcp", "--hub-dir"])
            .arg(hub)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Server { child, input, output, next: 1 }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.input, "{message}").unwrap();
        self.input.flush().unwrap();
    }

    fn ask(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(answer["id"], id, "{answer}");
        answer
    }

    /// A tool's text, and whether it reported an error.
    fn tool(&mut self, name: &str, arguments: Value) -> (String, bool) {
        let answer = self.ask("tools/call", json!({ "name": name, "arguments": arguments }));
        let result = &answer["result"];
        (result["content"][0]["text"].as_str().unwrap_or_default().to_string(), result["isError"].as_bool().unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A hub folder with one finished document, one that failed, and an inventory job.
fn hub(tag: &str) -> std::path::PathBuf {
    let hub = std::env::temp_dir().join(format!("gc-mcp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&hub);
    let line = |text: &str, y: f32, title: bool| OcrLine { text: text.into(), confidence: 0.95, quad: [100.0, y, 900.0, y, 900.0, y + 40.0, 100.0, y + 40.0], title };
    let pages = [OcrPage {
        path: "page-001.jpg".into(),
        width: 1000,
        height: 1400,
        lines: vec![line("Chợ nổi Cái Răng", 100.0, true), line("Trời vừa hửng sáng, ghe xuồng đã tấp nập trên sông.", 200.0, false)],
        error: None,
    }];
    let document = gc_doc::build(&pages);
    let done = hub.join("jobs").join(DOCUMENT);
    std::fs::create_dir_all(&done).unwrap();
    let job = json!({ "id": DOCUMENT, "kind": "document", "upload_id": "11111111-2222-3333-4444-555555555555", "status": "done", "result": { "pictures": 1 } });
    std::fs::write(done.join("job.json"), job.to_string()).unwrap();
    std::fs::write(done.join("document.md"), gc_doc::export::to_markdown(&document)).unwrap();
    std::fs::write(done.join("document.txt"), gc_doc::export::to_text(&document)).unwrap();
    std::fs::write(done.join("document.json"), gc_doc::export::to_json(&document)).unwrap();
    let failed = hub.join("jobs").join("aaaaaaaa-0000-0000-0000-000000000000");
    std::fs::create_dir_all(&failed).unwrap();
    let job = json!({ "id": "aaaaaaaa-0000-0000-0000-000000000000", "kind": "document", "upload_id": "x", "status": "failed", "result": null, "error": "No text was found in the photos." });
    std::fs::write(failed.join("job.json"), job.to_string()).unwrap();
    let inventory = hub.join("jobs").join("bbbbbbbb-0000-0000-0000-000000000000");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(inventory.join("job.json"), json!({ "id": "bbbbbbbb-0000-0000-0000-000000000000", "kind": "inventory", "upload_id": "y", "status": "done", "result": null }).to_string()).unwrap();
    hub
}

#[test]
fn an_assistant_lists_reads_and_saves_the_scanned_documents() {
    let hub = hub("documents");
    let mut server = Server::start(&hub);

    let started = server.ask("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }));
    assert_eq!(started["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(started["result"]["serverInfo"]["name"], "gods-camera");
    assert!(started["result"]["capabilities"]["tools"].is_object());
    // A notification gets no answer: the next line answers the next question.
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    assert_eq!(server.ask("ping", json!({}))["result"], json!({}));

    let tools = server.ask("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["list_documents", "read_document", "export_document"]);

    // The documents, not the inventory job; the finished one by its first line.
    let (listed, error) = server.tool("list_documents", json!({}));
    assert!(!error, "{listed}");
    let listed: Vec<Value> = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed.len(), 2, "{listed:?}");
    let done = listed.iter().find(|document| document["id"] == DOCUMENT).unwrap();
    assert_eq!((done["title"].as_str(), done["state"].as_str(), done["photos"].as_u64()), (Some("Chợ nổi Cái Răng"), Some("done"), Some(1)));
    assert!(done["made"].as_str().unwrap().ends_with(" UTC"));
    let failed = listed.iter().find(|document| document["state"] == "failed").unwrap();
    assert_eq!(failed["error"], "No text was found in the photos.");

    let (markdown, error) = server.tool("read_document", json!({ "id": DOCUMENT }));
    assert!(!error, "{markdown}");
    assert_eq!(markdown, std::fs::read_to_string(hub.join("jobs").join(DOCUMENT).join("document.md")).unwrap());
    assert!(markdown.contains("Chợ nổi Cái Răng") && markdown.contains("ghe xuồng đã tấp nập"), "{markdown}");
    let (text, _) = server.tool("read_document", json!({ "id": DOCUMENT, "format": "text" }));
    assert_eq!(text, std::fs::read_to_string(hub.join("jobs").join(DOCUMENT).join("document.txt")).unwrap());
    let (why, error) = server.tool("read_document", json!({ "id": "aaaaaaaa-0000-0000-0000-000000000000" }));
    assert!(error && why.contains("not done"), "{why}");
    // A path where an id belongs leads nowhere.
    let (why, error) = server.tool("read_document", json!({ "id": "../../secrets" }));
    assert!(error && why.contains("no document"), "{why}");

    // Saved as new files only, in the format the name says.
    let out = hub.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let docx = out.join("chợ nổi.docx");
    let (saved, error) = server.tool("export_document", json!({ "id": DOCUMENT, "format": "docx", "path": docx }));
    assert!(!error, "{saved}");
    assert!(std::fs::read(&docx).unwrap().starts_with(b"PK"), "a Word file is a zip");
    let pdf = out.join("chợ nổi.pdf");
    assert!(!server.tool("export_document", json!({ "id": DOCUMENT, "format": "pdf", "path": pdf })).1);
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF-"));
    let (why, error) = server.tool("export_document", json!({ "id": DOCUMENT, "format": "docx", "path": docx }));
    assert!(error && why.contains("cannot be made"), "an existing file stays: {why}");
    let (why, error) = server.tool("export_document", json!({ "id": DOCUMENT, "format": "pdf", "path": out.join("wrong.docx") }));
    assert!(error && why.contains(".pdf"), "{why}");
    let (why, error) = server.tool("export_document", json!({ "id": DOCUMENT, "format": "docx", "path": "relative.docx" }));
    assert!(error && why.contains("absolute"), "{why}");

    // What is not there is said so.
    let unknown = server.ask("tools/call", json!({ "name": "delete_everything", "arguments": {} }));
    assert_eq!(unknown["error"]["code"], -32602);
    assert_eq!(server.ask("resources/list", json!({}))["error"]["code"], -32601);

    drop(server);
    std::fs::remove_dir_all(hub).ok();
}

#[test]
fn an_empty_hub_lists_nothing() {
    let hub = std::env::temp_dir().join(format!("gc-mcp-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&hub);
    let mut server = Server::start(&hub);
    // An older client is answered in its own version.
    let started = server.ask("initialize", json!({ "protocolVersion": "2024-11-05", "capabilities": {} }));
    assert_eq!(started["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(server.tool("list_documents", json!({})), ("[]".to_string(), false));
}
