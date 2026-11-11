//! `gc`: the development command-line client and hub launcher.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use gc_link::{FileInfo, HubConfig, Job, UploadInfo};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;

mod face;

#[derive(Parser)]
#[command(name = "gc", about = "God's Camera development tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the development hub on loopback.
    Hub {
        #[arg(long, default_value = "127.0.0.1:47800")]
        addr: SocketAddr,
        /// Where uploads are stored.
        #[arg(long, default_value = ".gc-data")]
        data_dir: PathBuf,
        /// Largest accepted file, in megabytes.
        #[arg(long, default_value_t = 64)]
        max_file_mb: u64,
    },
    /// Upload every file in a folder, resuming partial files, then run an inventory job.
    Upload {
        folder: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:47800")]
        hub: String,
        /// Resume into an existing upload instead of creating one.
        #[arg(long)]
        upload_id: Option<String>,
    },
    /// Check every model file in the pack store against the manifests.
    VerifyPacks {
        /// Folder holding the manifest JSON files.
        #[arg(long, default_value = "../packs")]
        manifests: PathBuf,
        /// Folder holding the model files.
        #[arg(long)]
        store: PathBuf,
    },
    /// Put the face from one picture onto the faces in another.
    Swap {
        /// Folder holding the model files.
        #[arg(long)]
        packs: PathBuf,
        /// Picture of the face to swap in.
        #[arg(long)]
        source: PathBuf,
        /// Picture whose faces are replaced.
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// inswapper or hyperswap.
        #[arg(long, default_value = "inswapper")]
        model: String,
        /// oval or box.
        #[arg(long, default_value = "oval")]
        mask: String,
        /// Swap every face, not only the largest.
        #[arg(long)]
        all: bool,
        /// Leave the swapped face's colours as the model made them.
        #[arg(long)]
        no_colour_match: bool,
    },
    /// Run the live engine over a moving-face clip made from a photo, and report how it kept up.
    Replay {
        #[arg(long)]
        packs: PathBuf,
        /// A photo with a face. Its leftmost face moves in the clip.
        #[arg(long)]
        photo: PathBuf,
        /// Picture of the face to swap in. Defaults to the rightmost face in the photo.
        #[arg(long)]
        source: Option<PathBuf>,
        #[arg(long, default_value = "inswapper")]
        model: String,
        /// large or small.
        #[arg(long, default_value = "large")]
        detector: String,
        #[arg(long, default_value_t = 640)]
        detector_size: u32,
        /// still, talking or fast.
        #[arg(long, default_value = "talking")]
        motion: String,
        #[arg(long, default_value_t = 10.0)]
        seconds: f32,
        #[arg(long, default_value_t = 1280)]
        width: u32,
        #[arg(long, default_value_t = 720)]
        height: u32,
        /// Feed frames as fast as they are taken, instead of 30 a second.
        #[arg(long)]
        unpaced: bool,
        /// Run every model on the CPU.
        #[arg(long)]
        cpu: bool,
        /// Folder for compiled models, so later runs start faster.
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Folder for every fifteenth output frame, as PNG.
        #[arg(long)]
        dump: Option<PathBuf>,
        /// Also write the report to this file.
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

/// The hub token comes from the environment so it never appears in shell history.
fn token() -> Result<String> {
    std::env::var("GC_HUB_TOKEN").context("set GC_HUB_TOKEN to the hub token (16+ characters)")
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Hub { addr, data_dir, max_file_mb } => {
            let config = HubConfig {
                token: token()?,
                data_dir,
                max_file_bytes: max_file_mb << 20,
                allowed_origins: vec![],
            };
            let listener = gc_link::bind_dev(addr, &config).await?;
            println!("hub (development mode) listening on http://{}", listener.local_addr()?);
            gc_link::serve(listener, config).await?;
        }
        Command::Upload { folder, hub, upload_id } => upload(&folder, &hub, upload_id).await?,
        Command::VerifyPacks { manifests, store } => verify_packs(&manifests, &store)?,
        Command::Swap { packs, source, target, out, model, mask, all, no_colour_match } => {
            // The models block the thread, so they stay off the async runtime's workers.
            let args = face::SwapArgs {
                packs,
                source,
                target,
                out,
                model: face::parse_model(&model)?,
                mask: face::parse_mask(&mask)?,
                all,
                colour_match: !no_colour_match,
            };
            tokio::task::spawn_blocking(move || face::swap(args)).await??;
        }
        Command::Replay {
            packs, photo, source, model, detector, detector_size, motion, seconds, width, height, unpaced, cpu,
            cache_dir, dump, json,
        } => {
            let args = face::ReplayArgs {
                packs,
                photo,
                source,
                model: face::parse_model(&model)?,
                detector: match detector.as_str() {
                    "large" => gc_live::DetectorKind::Large,
                    "small" => gc_live::DetectorKind::Small,
                    other => bail!("unknown detector '{other}'; use large or small"),
                },
                detector_size,
                motion: match motion.as_str() {
                    "still" => gc_live::synthetic::Motion::Still,
                    "talking" => gc_live::synthetic::Motion::Talking,
                    "fast" => gc_live::synthetic::Motion::Fast,
                    other => bail!("unknown motion '{other}'; use still, talking or fast"),
                },
                seconds,
                width,
                height,
                unpaced,
                cpu_only: cpu,
                cache_dir,
                dump,
                json,
            };
            tokio::task::spawn_blocking(move || face::replay(args)).await??;
        }
    }
    Ok(())
}

async fn expect_ok(res: reqwest::Response) -> Result<reqwest::Response> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status();
    bail!("hub answered {status}: {}", res.text().await.unwrap_or_default())
}

async fn upload(folder: &Path, hub: &str, upload_id: Option<String>) -> Result<()> {
    let token = token()?;
    let http = reqwest::Client::new();
    let mut names: Vec<PathBuf> = std::fs::read_dir(folder)
        .with_context(|| format!("cannot read {}", folder.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    names.sort();

    let existing: UploadInfo = match upload_id {
        Some(id) => expect_ok(http.get(format!("{hub}/v1/uploads/{id}")).bearer_auth(&token).send().await?)
            .await?
            .json()
            .await?,
        None => expect_ok(http.post(format!("{hub}/v1/uploads")).bearer_auth(&token).send().await?)
            .await?
            .json()
            .await?,
    };
    let started = Instant::now();
    let (mut sent, mut skipped) = (0u64, 0usize);
    for path in &names {
        let name = path.file_name().unwrap().to_string_lossy();
        let data = tokio::fs::read(path).await?;
        let stored = existing.files.iter().find(|f| f.name == name).map_or(0, |f| f.bytes);
        if stored == data.len() as u64 {
            skipped += 1;
            continue;
        }
        if stored > data.len() as u64 {
            bail!("{name}: the hub holds more bytes than the local file");
        }
        let rest = data[stored as usize..].to_vec();
        sent += rest.len() as u64;
        let url = format!("{hub}/v1/uploads/{}/files/{name}?offset={stored}", existing.id);
        let info: FileInfo =
            expect_ok(http.put(url).bearer_auth(&token).body(rest).send().await?).await?.json().await?;
        if info.bytes != data.len() as u64 {
            bail!("{name}: the hub stored {} of {} bytes", info.bytes, data.len());
        }
    }
    let job: Job = expect_ok(
        http.post(format!("{hub}/v1/jobs"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "kind": "inventory", "upload_id": existing.id }))
            .send()
            .await?,
    )
    .await?
    .json()
    .await?;
    let secs = started.elapsed().as_secs_f64();
    println!(
        "upload {}: {} files ({} already complete), {:.1} MB sent in {:.2} s",
        existing.id,
        names.len(),
        skipped,
        sent as f64 / 1e6,
        secs
    );
    println!("job {} {}: {}", job.id, job.status, job.result.unwrap_or_default());
    Ok(())
}

fn verify_packs(manifests: &Path, store: &Path) -> Result<()> {
    let mut failed = false;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(manifests)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    for path in entries {
        let pack = gc_models::Pack::load(&path)?;
        for file in &pack.files {
            match file.verify(store) {
                Ok(()) => println!("ok    {:<14} {}", pack.name, file.path),
                Err(e) => {
                    failed = true;
                    println!("FAIL  {:<14} {e}", pack.name);
                }
            }
        }
    }
    if failed {
        bail!("some pack files did not verify");
    }
    Ok(())
}
