//! `gc`: the development command-line client and hub launcher.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use gc_link::{FileInfo, HubConfig, Job, UploadInfo};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(windows)]
mod camera;
mod face;
mod paired;

use paired::HubAccess;

#[derive(Parser)]
#[command(name = "gc", about = "God's Camera development tool")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the hub: on loopback for development, and with --lan also on the local network for paired devices.
    Hub {
        #[arg(long, default_value = "127.0.0.1:47800")]
        addr: SocketAddr,
        /// Where uploads are stored.
        #[arg(long, default_value = ".gc-data")]
        data_dir: PathBuf,
        /// Largest accepted file, in megabytes.
        #[arg(long, default_value_t = 64)]
        max_file_mb: u64,
        /// The program that reads text from pictures, such as `gc-ocr`. Without it the hub refuses document jobs.
        #[arg(long, env = "GC_OCR")]
        ocr: Option<PathBuf>,
        /// Also listen on the local network, over TLS. The hub's certificate is kept in the data folder.
        #[arg(long)]
        lan: bool,
        /// The port of the network listener.
        #[arg(long, default_value_t = 47801, requires = "lan")]
        lan_port: u16,
        /// The address the network listener binds to. The default is every
        /// interface. `127.0.0.1` keeps it on this computer, which is what tests
        /// want: a firewall asks about a program that listens on the network.
        #[arg(long, default_value = "::", requires = "lan")]
        lan_addr: std::net::IpAddr,
        /// Show a six-digit code at the start, so that one device can pair.
        #[arg(long, requires = "lan")]
        pair: bool,
        /// The hub's name, as a device shows it.
        #[arg(long, default_value = "gc hub", requires = "lan")]
        name: String,
    },
    /// Pair with a hub on the network, as a device does, and save what is needed to use it afterwards.
    Pair {
        /// Where the hub's network listener is, as host:port.
        #[arg(long)]
        address: String,
        /// The six digits the hub shows.
        #[arg(long, env = "GC_PAIR_CODE")]
        code: String,
        /// The name the hub lists this device under.
        #[arg(long)]
        name: String,
        /// The file to save the pairing in. It holds this device's token: keep it private.
        #[arg(long)]
        out: PathBuf,
    },
    /// Turn a folder of photos into one document: upload them, read and merge the text on the hub, fetch the result.
    Scan {
        folder: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:47800")]
        hub: String,
        /// Use the hub as a paired device: the file written by `gc pair`.
        #[arg(long, conflicts_with = "hub")]
        paired: Option<PathBuf>,
        /// Folder for document.md, document.txt and document.json.
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
    /// Upload every file in a folder, resuming partial files, then run an inventory job.
    Upload {
        folder: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:47800")]
        hub: String,
        /// Use the hub as a paired device: the file written by `gc pair`.
        #[arg(long, conflicts_with = "hub")]
        paired: Option<PathBuf>,
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
    /// Merge text already read from overlapping photos into one document.
    MergeOcr {
        /// JSON as `gc-ocr` prints it: a list of photos with their lines.
        ocr: PathBuf,
        /// Folder for document.md, document.txt and document.json.
        #[arg(long)]
        out: Option<PathBuf>,
        /// The true text, to report the character error rate against.
        #[arg(long)]
        truth: Option<PathBuf>,
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
        /// Leave the occlusion mask out, to see what it costs.
        #[arg(long)]
        no_occlusion: bool,
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
    /// Windows only: show the live face swap through the camera "God's Camera", from a moving-face clip made from a photo.
    #[cfg(windows)]
    Camera {
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
        /// still, talking or fast.
        #[arg(long, default_value = "talking")]
        motion: String,
        /// Stop after this many seconds. Without it, run until Ctrl+C.
        #[arg(long)]
        seconds: Option<f32>,
        /// Stop once the first app that watched the camera has stopped watching.
        #[arg(long)]
        once: bool,
        /// Only make the pipe: for tests that load the camera's media source themselves.
        #[arg(long)]
        no_camera: bool,
        /// A pipe name of the caller's own instead of the camera's. The media
        /// source never opens it; a test that stands in for the source does.
        #[arg(long, hide = true, requires = "no_camera")]
        pipe: Option<String>,
        /// Send the clip as it is, without the face swap.
        #[arg(long)]
        no_swap: bool,
        /// Leave the occlusion mask out.
        #[arg(long)]
        no_occlusion: bool,
        /// Folder for compiled models, so later runs start faster.
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Also write the report to this file.
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

fn parse_motion(name: &str) -> Result<gc_live::synthetic::Motion> {
    Ok(match name {
        "still" => gc_live::synthetic::Motion::Still,
        "talking" => gc_live::synthetic::Motion::Talking,
        "fast" => gc_live::synthetic::Motion::Fast,
        other => bail!("unknown motion '{other}'; use still, talking or fast"),
    })
}

/// The hub token comes from the environment so it never appears in shell history.
pub(crate) fn token() -> Result<String> {
    std::env::var("GC_HUB_TOKEN").context("set GC_HUB_TOKEN to the hub token (16+ characters)")
}

#[tokio::main]
async fn main() -> Result<()> {
    // The HTTP client and the hub share one TLS library.
    gc_link::tls::install_crypto_provider();
    match Cli::parse().command {
        Command::Hub { addr, data_dir, max_file_mb, ocr, lan, lan_port, lan_addr, pair, name } => {
            if let Some(program) = &ocr {
                if !program.is_file() {
                    bail!("the text reader {} does not exist", program.display());
                }
            }
            let lan = match lan {
                true => Some(gc_link::LanConfig {
                    bind: SocketAddr::from((lan_addr, lan_port)),
                    identity_pem: paired::hub_identity(&data_dir, &name)?,
                    name,
                }),
                false => None,
            };
            let config = HubConfig {
                token: token()?,
                data_dir,
                max_file_bytes: max_file_mb << 20,
                allowed_origins: vec![],
                ocr_command: ocr.map(|program| vec![program.to_string_lossy().into_owned()]),
                lan,
            };
            let bound = gc_link::Hub::bind(addr, config).await?;
            let hub = bound.hub();
            println!("hub (development mode) listening on http://{}", bound.local_addr()?);
            if let (Some(address), Some(fingerprint)) = (bound.lan_addr(), hub.fingerprint()) {
                println!("hub (paired devices) listening on https://{address}");
                println!("certificate fingerprint {fingerprint}");
            }
            if pair {
                tokio::spawn(paired::show_code(hub));
            }
            bound.serve().await?;
        }
        Command::Pair { address, code, name, out } => paired::pair(&address, &code, &name, &out).await?,
        Command::Upload { folder, hub, paired, upload_id } => {
            let hub = HubAccess::open(hub, paired.as_deref())?;
            let (id, _) = upload(&folder, &hub, upload_id).await?;
            let job = start_job(&hub, "inventory", &id).await?;
            println!("job {} {}: {}", job.id, job.status, job.result.unwrap_or_default());
        }
        Command::Scan { folder, hub, paired, out } => scan(&folder, &HubAccess::open(hub, paired.as_deref())?, &out).await?,
        Command::VerifyPacks { manifests, store } => verify_packs(&manifests, &store)?,
        Command::MergeOcr { ocr, out, truth } => merge_ocr(&ocr, out.as_deref(), truth.as_deref())?,
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
            no_occlusion, cache_dir, dump, json,
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
                motion: parse_motion(&motion)?,
                seconds,
                width,
                height,
                unpaced,
                cpu_only: cpu,
                no_occlusion,
                cache_dir,
                dump,
                json,
            };
            tokio::task::spawn_blocking(move || face::replay(args)).await??;
        }
        #[cfg(windows)]
        Command::Camera {
            packs, photo, source, model, motion, seconds, once, no_camera, pipe, no_swap, no_occlusion, cache_dir, json,
        } => {
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let args = camera::CameraArgs {
                packs,
                photo,
                source,
                model: face::parse_model(&model)?,
                motion: parse_motion(&motion)?,
                seconds,
                once,
                no_camera,
                pipe,
                no_swap,
                no_occlusion,
                cache_dir,
                json,
                stop: stop.clone(),
            };
            // Ctrl+C ends the run in order: the camera leaves the list and the report is printed.
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            });
            tokio::task::spawn_blocking(move || camera::run(args)).await??;
        }
    }
    Ok(())
}

pub(crate) async fn expect_ok(res: reqwest::Response) -> Result<reqwest::Response> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status();
    bail!("hub answered {status}: {}", res.text().await.unwrap_or_default())
}

/// Start a job on the hub.
async fn start_job(hub: &HubAccess, kind: &str, upload_id: &str) -> Result<Job> {
    let job = expect_ok(
        hub.http
            .post(format!("{}/v1/jobs", hub.base))
            .bearer_auth(&hub.token)
            .json(&serde_json::json!({ "kind": kind, "upload_id": upload_id }))
            .send()
            .await?,
    )
    .await?
    .json()
    .await?;
    Ok(job)
}

/// Upload a folder of photos, have the hub read and merge their text, and save the document.
async fn scan(folder: &Path, access: &HubAccess, out: &Path) -> Result<()> {
    let started = Instant::now();
    let (upload_id, files) = upload(folder, access, None).await?;
    let mut job = start_job(access, "document", &upload_id).await?;
    let (hub, http, token) = (&access.base, &access.http, &access.token);
    let mut shown = -1i32;
    while job.status == "queued" || job.status == "running" {
        let percent = (job.progress * 100.0) as i32;
        if percent / 10 != shown / 10 {
            println!("reading: {percent} %");
            shown = percent;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        job = expect_ok(http.get(format!("{hub}/v1/jobs/{}", job.id)).bearer_auth(token).send().await?).await?.json().await?;
    }
    if job.status != "done" {
        bail!("the job failed: {}", job.error.unwrap_or_else(|| "no reason given".into()));
    }
    std::fs::create_dir_all(out)?;
    for name in &job.files {
        let url = format!("{hub}/v1/jobs/{}/files/{name}", job.id);
        let bytes = expect_ok(http.get(url).bearer_auth(token).send().await?).await?.bytes().await?;
        std::fs::write(out.join(name), &bytes)?;
    }
    println!(
        "{files} files in {:.1} s: {}",
        started.elapsed().as_secs_f64(),
        job.result.map(|r| r.to_string()).unwrap_or_default()
    );
    println!("wrote {} to {}", job.files.join(", "), out.display());
    Ok(())
}

/// Upload every file in a folder, resuming partial files. Returns the upload's id and the number of files.
async fn upload(folder: &Path, access: &HubAccess, upload_id: Option<String>) -> Result<(String, usize)> {
    let (hub, http, token) = (&access.base, &access.http, &access.token);
    let mut names: Vec<PathBuf> = std::fs::read_dir(folder)
        .with_context(|| format!("cannot read {}", folder.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    names.sort();

    let existing: UploadInfo = match upload_id {
        Some(id) => expect_ok(http.get(format!("{hub}/v1/uploads/{id}")).bearer_auth(token).send().await?)
            .await?
            .json()
            .await?,
        None => expect_ok(http.post(format!("{hub}/v1/uploads")).bearer_auth(token).send().await?)
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
            expect_ok(http.put(url).bearer_auth(token).body(rest).send().await?).await?.json().await?;
        if info.bytes != data.len() as u64 {
            bail!("{name}: the hub stored {} of {} bytes", info.bytes, data.len());
        }
    }
    let secs = started.elapsed().as_secs_f64();
    println!(
        "upload {}: {} files ({} already complete), {:.1} MB sent in {:.2} s",
        existing.id,
        names.len(),
        skipped,
        sent as f64 / 1e6,
        secs
    );
    Ok((existing.id, names.len()))
}

fn merge_ocr(ocr: &Path, out: Option<&Path>, truth: Option<&Path>) -> Result<()> {
    let text = std::fs::read_to_string(ocr).with_context(|| format!("cannot read {}", ocr.display()))?;
    let pages: Vec<gc_doc::OcrPage> = serde_json::from_str(&text).context("not gc-ocr output")?;
    let document = gc_doc::build(&pages);
    let lines: usize = pages.iter().map(|page| page.lines.len()).sum();
    println!(
        "{} photos, {} lines read, {} repeated readings removed, {} blocks, {} group(s)",
        document.pages,
        lines,
        document.duplicates_removed,
        document.blocks.len(),
        document.groups
    );
    let plain = gc_doc::export::to_text(&document);
    if let Some(truth) = truth {
        let truth = std::fs::read_to_string(truth)?;
        let rate = gc_doc::text::character_error_rate(&plain, &truth);
        println!("character error rate {:.2} % ({:.2} % accuracy)", rate * 100.0, (1.0 - rate) * 100.0);
    }
    if let Some(out) = out {
        std::fs::create_dir_all(out)?;
        std::fs::write(out.join("document.txt"), &plain)?;
        std::fs::write(out.join("document.md"), gc_doc::export::to_markdown(&document))?;
        std::fs::write(out.join("document.json"), gc_doc::export::to_json(&document))?;
        println!("wrote document.txt, document.md and document.json to {}", out.display());
    }
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
