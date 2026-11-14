//! Jobs: keeping them on disk, and the document job.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{AppState, HubConfig, Job};

/// Pictures handed to the text reader in one go. Smaller batches mean finer progress.
const BATCH: usize = 4;
/// The longest the text reader may take over one batch.
const BATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub(crate) fn job_dir(config: &HubConfig, id: &str) -> PathBuf {
    config.data_dir.join("jobs").join(id)
}

pub(crate) fn is_picture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["jpg", "jpeg", "png", "heic", "heif", "tif", "tiff", "webp", "bmp"].iter().any(|ext| lower.ends_with(&format!(".{ext}")))
}

/// Jobs left on disk by an earlier run. A job that was still going when the
/// hub stopped is marked failed, since nothing is working on it any more.
pub(crate) fn load(config: &HubConfig) -> HashMap<String, Job> {
    let mut jobs = HashMap::new();
    let Ok(entries) = std::fs::read_dir(config.data_dir.join("jobs")) else { return jobs };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path().join("job.json")) else { continue };
        let Ok(mut job) = serde_json::from_str::<Job>(&text) else { continue };
        if job.status == "queued" || job.status == "running" {
            job.status = "failed".into();
            job.error = Some("the hub was stopped while this job was running".into());
            write(config, &job);
        }
        jobs.insert(job.id.clone(), job);
    }
    jobs
}

fn write(config: &HubConfig, job: &Job) {
    let dir = job_dir(config, &job.id);
    let saved = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("job.json"), serde_json::to_vec_pretty(job).unwrap_or_default()));
    if let Err(error) = saved {
        eprintln!("hub: could not save job {}: {error}", job.id);
    }
}

/// Record a job's state, in memory and on disk.
pub(crate) fn store(state: &AppState, job: &Job) {
    state.jobs.lock().unwrap().insert(job.id.clone(), job.clone());
    write(&state.config, job);
}

fn update(state: &AppState, id: &str, change: impl FnOnce(&mut Job)) {
    let job = {
        let mut jobs = state.jobs.lock().unwrap();
        let Some(job) = jobs.get_mut(id) else { return };
        change(job);
        job.clone()
    };
    write(&state.config, &job);
}

/// Run the text reader over some pictures and return what it printed.
async fn read_pictures(command: &[String], pictures: &[PathBuf]) -> Result<Vec<gc_doc::OcrPage>, String> {
    let (program, leading) = command.split_first().ok_or("no text reader is set up")?;
    let run = tokio::process::Command::new(program).args(leading).args(pictures).kill_on_drop(true).output();
    let output = tokio::time::timeout(BATCH_TIMEOUT, run)
        .await
        .map_err(|_| "reading the text took too long".to_string())?
        .map_err(|error| {
            eprintln!("hub: text reader {program} did not start: {error}");
            "the text reader could not be started".to_string()
        })?;
    if !output.status.success() {
        eprintln!("hub: text reader failed: {}", String::from_utf8_lossy(&output.stderr));
        return Err("the text reader failed".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        eprintln!("hub: text reader printed something unexpected: {error}");
        "the text reader's answer could not be understood".to_string()
    })
}

/// Read the text from every picture, merge it into one document, and write the results.
pub(crate) async fn run_document(state: AppState, id: String, pictures: Vec<PathBuf>) {
    // One heavy job at a time. A closed semaphore cannot happen while the hub runs.
    let Ok(_turn) = state.heavy.clone().acquire_owned().await else { return };
    update(&state, &id, |job| job.status = "running".into());
    let outcome = document(&state, &id, &pictures).await;
    update(&state, &id, |job| match outcome {
        Ok((summary, files)) => {
            job.status = "done".into();
            job.progress = 1.0;
            job.result = Some(summary);
            job.files = files;
        }
        Err(reason) => {
            job.status = "failed".into();
            job.error = Some(reason);
        }
    });
}

async fn document(state: &AppState, id: &str, pictures: &[PathBuf]) -> Result<(serde_json::Value, Vec<String>), String> {
    let command = state.config.ocr_command.clone().ok_or("no text reader is set up")?;
    let mut pages = Vec::with_capacity(pictures.len());
    for (done, batch) in pictures.chunks(BATCH).enumerate() {
        pages.extend(read_pictures(&command, batch).await?);
        // Reading is nearly all of the work; merging takes a moment at the end.
        let progress = ((done + 1) * BATCH).min(pictures.len()) as f32 / pictures.len() as f32 * 0.95;
        update(state, id, |job| job.progress = progress);
    }
    let unreadable = pages.iter().filter(|page| page.error.is_some()).count();
    let lines: usize = pages.iter().map(|page| page.lines.len()).sum();
    // The page paths are the hub's own; clients get file names only.
    for page in &mut pages {
        page.path = Path::new(&page.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    }
    // What was read from each photo, by its file name in the upload: a
    // searchable PDF lays these lines over the photos themselves.
    let read = serde_json::to_string(&pages).unwrap_or_else(|_| "[]".into());
    let built = tokio::task::spawn_blocking(move || gc_doc::build(&pages)).await.map_err(|_| "merging the text failed".to_string())?;

    let dir = job_dir(&state.config, id);
    let outputs = [
        ("document.md", gc_doc::export::to_markdown(&built)),
        ("document.txt", gc_doc::export::to_text(&built)),
        ("document.json", gc_doc::export::to_json(&built)),
        ("pages.json", read),
    ];
    for (name, content) in &outputs {
        tokio::fs::write(dir.join(name), content).await.map_err(|error| {
            eprintln!("hub: could not write {name}: {error}");
            "the results could not be saved".to_string()
        })?;
    }
    let lowest = built.blocks.iter().map(|block| block.confidence).fold(1.0f32, f32::min);
    let summary = serde_json::json!({
        "pictures": pictures.len(),
        "unreadable_pictures": unreadable,
        "lines_read": lines,
        "repeated_lines_removed": built.duplicates_removed,
        "blocks": built.blocks.len(),
        "groups": built.groups,
        "characters": outputs[1].1.chars().count(),
        "lowest_confidence": lowest,
    });
    Ok((summary, outputs.iter().map(|(name, _)| name.to_string()).collect()))
}
