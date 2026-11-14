//! `gc mcp`: the scanner's documents for an assistant that speaks the Model
//! Context Protocol (plan, Phase 10: "an MCP server so assistants can run
//! scans and exports").
//!
//! It speaks JSON-RPC 2.0 over standard input and output, one message a line,
//! as MCP's stdio transport asks, and offers three tools over the hub's folder
//! of the desktop app: list the documents, read one, and save one as a Word
//! file or a PDF. It reads that folder, writes only the new file it is asked
//! for, starts nothing and listens on no port. What it reads goes to whichever
//! assistant the user connects it to.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// The protocol versions this server speaks, newest first.
const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Where the desktop app keeps its hub, documents and photos.
pub fn default_hub_folder() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library/Application Support");
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".local/share");
    base.join("GodsCamera").join("Hub")
}

/// Answer messages from `input` on `output` until `input` ends.
pub fn serve(hub: &Path, input: impl BufRead, mut output: impl Write) -> anyhow::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => answer(hub, &message),
            Err(error) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("not JSON: {error}") } })),
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// The answer to one message, or nothing for a notification.
fn answer(hub: &Path, message: &Value) -> Option<Value> {
    // A notification has no id, and is not answered.
    let id = message.get("id")?.clone();
    let params = &message["params"];
    let result = match message["method"].as_str().unwrap_or("") {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            let version = VERSIONS.into_iter().find(|version| *version == asked).unwrap_or(VERSIONS[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "gods-camera", "title": "God's Camera", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "The documents God's Camera made from photos of pages sent by a phone or tablet. \
List them, read one as Markdown or plain text, or save one as a Word file or a PDF."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call(hub, params),
        method => Err((-32601, format!("there is no method {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, words)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": words } }),
    })
}

fn tools() -> Value {
    json!([
        {
            "name": "list_documents",
            "title": "List scanned documents",
            "description": "The documents God's Camera made from photos of pages, newest first: each one's id, title (its first line), state (done, running, queued or failed), when it was made, and how many photos went in.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "read_document",
            "title": "Read a scanned document",
            "description": "The text of one document: as Markdown, with its headings and paragraphs, or as plain text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The document's id, from list_documents." },
                    "format": { "type": "string", "enum": ["markdown", "text"], "default": "markdown" }
                },
                "required": ["id"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "export_document",
            "title": "Save a scanned document as Word or PDF",
            "description": "Write one document to a new file, as a Word document or as a PDF set as text on A4 pages. An existing file is never replaced.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The document's id, from list_documents." },
                    "format": { "type": "string", "enum": ["docx", "pdf"] },
                    "path": { "type": "string", "description": "An absolute path for the new file, ending in .docx or .pdf." }
                },
                "required": ["id", "format", "path"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false }
        }
    ])
}

/// A tool's result. A tool that could not do its work says why in its
/// result, for the model to read; an unknown tool is a protocol error.
fn call(hub: &Path, params: &Value) -> Result<Value, (i64, String)> {
    let arguments = &params["arguments"];
    let outcome = match params["name"].as_str().unwrap_or("") {
        "list_documents" => list(hub),
        "read_document" => read(hub, arguments),
        "export_document" => export(hub, arguments),
        other => return Err((-32602, format!("there is no tool named {other}"))),
    };
    Ok(match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
    })
}

fn list(hub: &Path) -> Result<String, String> {
    let mut documents = Vec::new();
    for entry in std::fs::read_dir(hub.join("jobs")).into_iter().flatten().flatten() {
        let folder = entry.path();
        let Some(job) = job(&folder) else { continue };
        if job["kind"] != "document" || !job["id"].as_str().is_some_and(valid) {
            continue;
        }
        let status = job["status"].as_str().unwrap_or("failed");
        let title = match status {
            "done" => std::fs::read_to_string(folder.join("document.txt"))
                .ok()
                .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string))
                .unwrap_or_default(),
            _ => String::new(),
        };
        let made = std::fs::metadata(folder.join("job.json")).and_then(|file| file.modified()).ok();
        let mut document = json!({ "id": job["id"], "title": title, "state": status, "made": made.map(utc).unwrap_or_default() });
        if let Some(photos) = job["result"]["pictures"].as_u64() {
            document["photos"] = json!(photos);
        }
        if let Some(error) = job["error"].as_str() {
            document["error"] = json!(error);
        }
        documents.push((made, document));
    }
    documents.sort_by_key(|(made, _)| std::cmp::Reverse(*made));
    let documents: Vec<Value> = documents.into_iter().map(|(_, document)| document).collect();
    Ok(serde_json::to_string_pretty(&documents).unwrap_or_default())
}

fn read(hub: &Path, arguments: &Value) -> Result<String, String> {
    let folder = document_folder(hub, arguments)?;
    let name = match arguments["format"].as_str().unwrap_or("markdown") {
        "markdown" => "document.md",
        "text" => "document.txt",
        other => return Err(format!("the format is markdown or text, not {other}")),
    };
    std::fs::read_to_string(folder.join(name)).map_err(|_| "this document has no text: it is not done yet, or it failed".to_string())
}

fn export(hub: &Path, arguments: &Value) -> Result<String, String> {
    let folder = document_folder(hub, arguments)?;
    let format = arguments["format"].as_str().unwrap_or("");
    if !matches!(format, "docx" | "pdf") {
        return Err("the format is docx or pdf".into());
    }
    let path = PathBuf::from(arguments["path"].as_str().ok_or("a path for the new file is needed")?);
    if !path.is_absolute() {
        return Err("the path must be absolute".into());
    }
    if path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase).as_deref() != Some(format) {
        return Err(format!("the file's name must end in .{format}"));
    }
    let document: gc_doc::Document = std::fs::read(folder.join("document.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or("this document cannot be read: it is not done yet, or it failed")?;
    let bytes = match format {
        "docx" => gc_doc::export::to_docx(&document, None),
        _ => gc_doc::export::to_pdf(&document, None),
    };
    // A new file only: nothing of the user's is replaced.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("{} cannot be made: {error}", path.display()))?;
    file.write_all(&bytes).map_err(|error| format!("{} cannot be written: {error}", path.display()))?;
    Ok(format!("Saved {} ({} KB).", path.display(), bytes.len().div_ceil(1024)))
}

/// The folder of the document named in `arguments`.
fn document_folder(hub: &Path, arguments: &Value) -> Result<PathBuf, String> {
    let id = arguments["id"].as_str().ok_or("the document's id is needed; list_documents gives them")?;
    let folder = hub.join("jobs").join(id);
    match job(&folder) {
        Some(job) if valid(id) && job["kind"] == "document" => Ok(folder),
        _ => Err(format!("there is no document with the id {id}")),
    }
}

fn job(folder: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(folder.join("job.json")).ok()?).ok()
}

/// An id as the hub makes them: a UUID. Anything else could lead out of the hub's folder.
fn valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// A moment as `2026-11-14 09:30 UTC`.
fn utc(moment: SystemTime) -> String {
    let seconds = moment.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Days since 1970 to a date in the Gregorian calendar (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", rest / 3_600, rest % 3_600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_written_as_people_read_them() {
        assert_eq!(utc(UNIX_EPOCH), "1970-01-01 00:00 UTC");
        assert_eq!(utc(UNIX_EPOCH + std::time::Duration::from_secs(951_782_400 + 3_600 * 13 + 60 * 5)), "2000-02-29 13:05 UTC");
        assert_eq!(utc(UNIX_EPOCH + std::time::Duration::from_secs(1_794_614_400)), "2026-11-14 00:00 UTC");
    }

    #[test]
    fn only_a_hub_id_names_a_folder() {
        assert!(valid("2f0c6b1e-6c1d-4b8e-9d7a-0e5b2a9c1f00"));
        assert!(!valid("") && !valid("..") && !valid("../jobs") && !valid("a/b") && !valid("C:"));
    }
}
