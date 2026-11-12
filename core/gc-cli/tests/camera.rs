//! `gc camera` end to end, with the test standing in for the camera's media source.
//!
//! Needs the model files, looked for in `GC_PACKS_DIR` and then in `../packs`
//! beside the repository, and the sample photo made by `tools/make-reference.py`.
//! Without them the test passes with a note.
#![cfg(windows)]

use gc_win::protocol::{self, FrameHeader, Wanted, FRAME_HEADER_BYTES};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

const GC: &str = env!("CARGO_BIN_EXE_gc");
const HD: Wanted = Wanted { width: 1280, height: 720, frame_rate: (30, 1) };

fn packs() -> Option<PathBuf> {
    let dir = std::env::var_os("GC_PACKS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packs"));
    let photo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../gc-infer/tests/golden/input.png");
    if dir.join("inswapper_128.onnx").is_file() && photo.is_file() {
        Some(dir)
    } else {
        eprintln!("model files or the sample photo not found; skipping");
        None
    }
}

/// Connect as the media source does, once the tool has made its pipe.
fn connect(name: &str) -> File {
    let started = Instant::now();
    loop {
        match OpenOptions::new().read(true).write(true).open(name) {
            Ok(source) => return source,
            Err(error) => assert!(started.elapsed() < Duration::from_secs(30), "the pipe never appeared: {error}"),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One NV12 picture as packed RGB, for a person to look at.
fn rgb(nv12: &[u8], width: usize, height: usize) -> Vec<u8> {
    let (luma, chroma) = nv12.split_at(width * height);
    let mut out = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        for x in 0..width {
            let pair = (y / 2) * width + (x / 2) * 2;
            let (c, u, v) = (luma[y * width + x] as f32 - 16.0, chroma[pair] as f32 - 128.0, chroma[pair + 1] as f32 - 128.0);
            // BT.709, video range.
            let rgb = [1.164 * c + 1.793 * v, 1.164 * c - 0.213 * u - 0.533 * v, 1.164 * c + 2.112 * u];
            out.extend(rgb.map(|value| value.round().clamp(0.0, 255.0) as u8));
        }
    }
    out
}

#[test]
fn the_swapped_picture_reaches_a_watching_source_live_and_as_a_loop() {
    let Some(packs) = packs() else { return };
    let photo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../gc-infer/tests/golden/input.png");
    let pipe = format!(r"\\.\pipe\godscamera.test.cli.{}", std::process::id());
    // Three seconds live, five seconds recorded, and then the loop.
    let mut tool = Command::new(GC)
        .args(["camera", "--no-camera", "--once", "--seconds", "300", "--motion", "talking", "--loop-after", "3", "--pipe", &pipe])
        .arg("--packs")
        .arg(&packs)
        .arg("--photo")
        .arg(&photo)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut source = connect(&pipe);
    source.write_all(&protocol::request(&HD)).unwrap();

    // Pictures are read on a thread of their own, so a tool that goes wrong
    // fails the test instead of leaving it waiting.
    const LIVE: usize = 80;
    const WANTED: usize = 330;
    let (send, receive) = channel::<(FrameHeader, Instant, Vec<u8>)>();
    let reader = std::thread::spawn(move || {
        for _ in 0..WANTED {
            let mut header = [0u8; FRAME_HEADER_BYTES];
            if source.read_exact(&mut header).is_err() {
                return;
            }
            let Some(header) = protocol::parse_frame_header(&header) else { return };
            let mut payload = vec![0u8; header.payload_bytes as usize];
            if source.read_exact(&mut payload).is_err() || send.send((header, Instant::now(), payload)).is_err() {
                return;
            }
        }
        // Dropping the connection here is what ends the tool: `--once`.
    });

    // Only a few whole pictures are kept; of the others, what the checks need.
    let mut headers: Vec<(FrameHeader, Instant)> = Vec::with_capacity(WANTED);
    let (mut live_picture, mut last_picture) = (Vec::new(), Vec::new());
    // The first picture comes once the models are loaded and warm.
    let mut patience = Duration::from_secs(180);
    while headers.len() < WANTED {
        match receive.recv_timeout(patience) {
            Ok((header, arrived, payload)) => {
                headers.push((header, arrived));
                if headers.len() == LIVE {
                    live_picture = payload;
                } else if headers.len() == WANTED {
                    last_picture = payload;
                }
            }
            Err(_) => {
                let _ = tool.kill();
                panic!("only {} of {WANTED} pictures arrived", headers.len());
            }
        }
        patience = Duration::from_secs(5);
    }
    reader.join().unwrap();

    for pair in headers.windows(2) {
        assert_eq!(pair[1].0.sequence, pair[0].0.sequence + 1, "a picture is missing between two that arrived");
        assert!(pair[1].0.time_100ns > pair[0].0.time_100ns, "the times go backwards");
    }
    assert!(headers.iter().all(|(header, _)| (header.width, header.height) == (HD.width, HD.height)));
    // Thirty a second, by the times the tool stamped and by when the pictures
    // arrived: while live, and over the whole run with the loop in it.
    let pace = |from: usize, to: usize| -> (f64, f64) {
        let spans = (to - from) as f64;
        let stamped = (headers[to].0.time_100ns - headers[from].0.time_100ns) as f64 / 10_000.0 / spans;
        let arrived = (headers[to].1 - headers[from].1).as_secs_f64() * 1000.0 / spans;
        (stamped, arrived)
    };
    for (what, (stamped, arrived)) in [("live", pace(0, LIVE - 1)), ("in all", pace(0, WANTED - 1))] {
        assert!((stamped - 33.33).abs() < 1.0, "{what}: a picture every {stamped:.2} ms by the stamps");
        assert!((arrived - 33.33).abs() < 1.0, "{what}: a picture every {arrived:.2} ms by arrival");
    }
    // Starting to record and going over to the loop leave no gap in the pictures.
    let longest_wait =
        headers.windows(2).map(|pair| (pair[1].1 - pair[0].1).as_secs_f64() * 1000.0).fold(0.0, f64::max);
    assert!(longest_wait < 120.0, "the picture stood still for {longest_wait:.0} ms");

    // The picture itself: the clip's plain wall at the edge, darker at the top
    // than at the bottom, and a face in the middle. Live and in the loop alike.
    let (width, height) = (HD.width as usize, HD.height as usize);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    for (name, picture) in [("live", &live_picture), ("loop", &last_picture)] {
        let luma = |x: usize, y: usize| picture[y * width + x] as i32;
        assert!((70..=84).contains(&luma(4, 4)), "{name}: the wall at the top is {}", luma(4, 4));
        assert!((104..=118).contains(&luma(4, height - 4)), "{name}: the wall at the bottom is {}", luma(4, height - 4));
        let middle: Vec<i32> =
            (0..200).flat_map(|i| (0..200).map(move |j| (540 + i, 240 + j))).map(|(x, y)| luma(x, y)).collect();
        let mean = middle.iter().sum::<i32>() / middle.len() as i32;
        let spread = middle.iter().map(|value| (value - mean).pow(2)).sum::<i32>() / middle.len() as i32;
        assert!(spread > 200, "{name}: the middle of the picture is flat: nothing like a face is there");
        // For a person to look at: target\tmp\gc-camera-live.png and gc-camera-loop.png
        let file = out.join(format!("gc-camera-{name}.png"));
        image::save_buffer(&file, &rgb(picture, width, height), HD.width, HD.height, image::ColorType::Rgb8).unwrap();
    }

    // The tool notices that the source has gone, stops and reports.
    let output = tool.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let report: serde_json::Value = serde_json::from_str(text.lines().last().expect("a report")).expect("the report is JSON");
    assert_eq!(report["camera"], "not asked for");
    assert_eq!(report["write_failures"], 0);
    assert_eq!(report["connects"], 1);
    let made = report["frames_made"].as_u64().unwrap();
    assert!(made >= WANTED as u64, "{report}");
    assert!(report["frames_sent"].as_u64().unwrap() >= WANTED as u64, "{report}");
    assert_eq!(report["frames_dropped"], 0, "{report}");
    // The swapped face is in nearly every picture, and is renewed many times a second.
    assert!(report["frames_with_the_swapped_face"].as_u64().unwrap() * 10 >= made * 9, "{report}");
    assert!(report["swap"]["swaps_per_second"].as_f64().unwrap() > 12.0, "{report}");
    assert!(report["swap"]["last_error"].is_null(), "{report}");
    // Five seconds were recorded at the rate they went out, and played.
    let frames = report["loop"]["frames"].as_u64().unwrap();
    assert!((140..=152).contains(&frames), "{report}");
    assert_eq!(report["loop"]["frame_rate"], 30.0, "{report}");
    assert!(report["frame_periods_in_the_loop"].as_u64().unwrap() >= 45, "{report}");
}

#[test]
fn a_second_copy_is_told_that_the_pipe_is_taken() {
    let pipe = format!(r"\\.\pipe\godscamera.test.cli.taken.{}", std::process::id());
    let _holder = gc_win::Feed::open_named(&pipe).unwrap();
    let output = Command::new(GC)
        .args(["camera", "--no-camera", "--pipe", &pipe, "--packs", "nowhere", "--photo", "nothing.png"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("already holds the camera's pipe"), "{said}");
}
