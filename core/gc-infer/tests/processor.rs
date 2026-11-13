//! Where a model runs when the code says "on the processor".
//!
//! Not a check: prints how long the face detector and the identity encoder
//! take when asked for the processor, and the first numbers they give, so
//! that two machines, or two versions of the code, can be compared.
//!
//!     cargo test -p gc-infer --features runtime --release --test processor -- --ignored --nocapture
//!
//! Needs the model files and the reference pictures, as `runtime.rs` does.

#![cfg(feature = "runtime")]

use gc_infer::image::{ImageView, PixelFormat};
use gc_infer::runtime::{Accelerator, Detector, IdentityEncoder, Runtime};
use std::path::PathBuf;

fn folder(variable: &str, beside: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(beside))
}

fn median(mut times: Vec<f64>) -> f64 {
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

#[test]
#[ignore]
fn on_the_processor() {
    let packs = folder("GC_PACKS_DIR", "../../../packs");
    let golden = folder("GC_GOLDEN_DIR", "tests/golden");
    if !packs.join("buffalo_l/det_10g.onnx").is_file() || !golden.join("input.png").is_file() {
        eprintln!("model files or reference pictures not found; nothing to measure");
        return;
    }
    let picture = image::open(golden.join("input.png")).expect("input.png").to_rgb8();
    let (width, height) = picture.dimensions();
    let frame = ImageView::new(picture.as_raw(), width, height, width as usize * 3, PixelFormat::Rgb).unwrap();
    let runtime = Runtime::shared(None).expect("runtime");

    for (name, size) in [("buffalo_l/det_10g.onnx", 640), ("buffalo_s/det_500m.onnx", 640)] {
        let model = packs.join(name);
        if !model.is_file() {
            continue;
        }
        let loading = std::time::Instant::now();
        let mut detector = Detector::load(runtime, &model, size, Accelerator::Cpu).expect("detector");
        let loaded = loading.elapsed();
        let faces = detector.detect(&frame, 0.5).expect("faces");
        let mut times = Vec::new();
        for _ in 0..40 {
            let started = std::time::Instant::now();
            detector.detect(&frame, 0.5).expect("faces");
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        let first = faces.first().expect("a face in the reference picture");
        println!(
            "{name}: load {:.2} s, median {:.1} ms; {} face(s); first: score {:.6}, box {:.3?}, eye {:.3?}",
            loaded.as_secs_f64(),
            median(times),
            faces.len(),
            first.score,
            first.bbox,
            first.kps[0]
        );
        if name.starts_with("buffalo_l") {
            let encoder_file = packs.join("buffalo_l/w600k_r50.onnx");
            let loading = std::time::Instant::now();
            let mut encoder = IdentityEncoder::load(runtime, &encoder_file, Accelerator::Cpu).expect("encoder");
            let loaded = loading.elapsed();
            let identity = encoder.embed(&frame, &first.kps).expect("identity");
            let mut times = Vec::new();
            for _ in 0..20 {
                let started = std::time::Instant::now();
                encoder.embed(&frame, &first.kps).expect("identity");
                times.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            let length = identity.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
            println!(
                "buffalo_l/w600k_r50.onnx: load {:.2} s, median {:.1} ms; length {length:.5}, first numbers {:.5?}",
                loaded.as_secs_f64(),
                median(times),
                &identity[..3]
            );
        }
    }
}
