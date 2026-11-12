//! The whole still-image pipeline with the real models, compared with
//! InsightFace's own output on the same picture.
//!
//! Needs the model files. They are looked for in `GC_PACKS_DIR`, then in
//! `../packs` beside the repository. Without them the tests pass with a note.

#![cfg(feature = "runtime")]

use gc_infer::image::{self, ImageMut, ImageView, MaskShape, PixelFormat};
use gc_infer::runtime::{Accelerator, Detector, IdentityEncoder, Runtime, Swapper};
use gc_infer::SwapperContract;
use serde_json::Value;
use std::path::PathBuf;

fn golden_dir() -> PathBuf {
    std::env::var_os("GC_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden"))
}

fn packs() -> Option<PathBuf> {
    let dir = std::env::var_os("GC_PACKS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packs"));
    if dir.join("inswapper_128.onnx").is_file() {
        Some(dir)
    } else {
        eprintln!("model files not found in {}; skipping", dir.display());
        None
    }
}

/// A golden PNG as packed RGB.
fn rgb(name: &str) -> (Vec<u8>, u32, u32) {
    let picture = ::image::open(golden_dir().join(name)).expect(name).to_rgb8();
    let (width, height) = picture.dimensions();
    (picture.into_raw(), width, height)
}

fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

fn golden() -> Value {
    serde_json::from_str(&std::fs::read_to_string(golden_dir().join("golden.json")).unwrap()).unwrap()
}

fn pair(v: &Value) -> [f32; 2] {
    [v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32]
}

#[test]
fn still_image_pipeline_matches_insightface() {
    let Some(packs) = packs() else { return };
    let g = golden();
    let runtime = Runtime::shared(None).expect("runtime");
    let (frame_rgb, width, height) = rgb("input.png");
    let frame = ImageView::new(&frame_rgb, width, height, width as usize * 3, PixelFormat::Rgb).unwrap();

    // Detector: the same faces, in the same places.
    let mut detector =
        Detector::load(runtime, &packs.join("buffalo_l/det_10g.onnx"), 640, Accelerator::Cpu).expect("detector");
    let mut faces = detector.detect(&frame, 0.5).expect("detect");
    faces.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
    let expected = g["det"]["faces"].as_array().unwrap();
    assert_eq!(faces.len(), expected.len());
    for (face, want) in faces.iter().zip(expected) {
        for i in 0..4 {
            let w = want["bbox"][i].as_f64().unwrap() as f32;
            assert!((face.bbox[i] - w).abs() < 1.5, "box {} vs {w}", face.bbox[i]);
        }
        for (point, want) in face.kps.iter().zip(want["kps"].as_array().unwrap()) {
            let w = pair(want);
            assert!((point[0] - w[0]).abs() < 1.0 && (point[1] - w[1]).abs() < 1.0, "keypoint {point:?} vs {w:?}");
        }
        assert!((face.score - want["score"].as_f64().unwrap() as f32).abs() < 0.03);
    }

    // Alignment: the crop InsightFace cut for the leftmost face.
    let target = &faces[0];
    let source = &faces[faces.len() - 1];
    let matrix = gc_align::estimate_norm(&target.kps, 128).unwrap();
    let crop = image::warp_to_crop(&frame, &matrix, 128);
    let (golden_crop, _, _) = rgb("aligned_128.png");
    let crop_difference = mean_difference(&crop, &golden_crop);
    assert!(crop_difference < 2.0, "aligned crop differs by {crop_difference:.2}");

    // Identity: the same latent for the rightmost face.
    let mut encoder =
        IdentityEncoder::load(runtime, &packs.join("buffalo_l/w600k_r50.onnx"), Accelerator::Cpu).expect("encoder");
    let embedding = encoder.embed(&frame, &source.kps).expect("embed");
    let norm = embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
    let want_norm = g["swap"]["source_embedding_norm"].as_f64().unwrap() as f32;
    assert!((norm - want_norm).abs() / want_norm < 0.03, "embedding norm {norm} vs {want_norm}");

    let mut swapper = Swapper::load(
        runtime,
        &packs.join("inswapper_128.onnx"),
        SwapperContract::INSWAPPER_128,
        Some(&packs.join("inswapper_128.emap.bin")),
        Accelerator::Cpu,
    )
    .expect("swapper");
    let latent = swapper.identity(&embedding).expect("latent");
    for (value, want) in latent.iter().zip(g["swap"]["latent_first8"].as_array().unwrap()) {
        assert!((value - want.as_f64().unwrap() as f32).abs() < 0.02, "latent {value} vs {want}");
    }

    // Swap: the same face comes out.
    let swapped = swapper.swap(&crop, &latent).expect("swap");
    let (golden_swap, _, _) = rgb("swap_128.png");
    let swap_difference = mean_difference(&swapped.rgb, &golden_swap);
    assert!(swap_difference < 3.0, "swapped crop differs by {swap_difference:.2}");
    assert!(swapped.mask.is_none());

    // Paste: close to InsightFace's pasted frame, and different from the input where the face is.
    let mut pasted = frame_rgb.clone();
    let mask = image::crop_mask(128, MaskShape::Box);
    let mut out = ImageMut { data: &mut pasted, width, height, stride: width as usize * 3, format: PixelFormat::Rgb };
    image::paste_crop(&mut out, &swapped.rgb, &mask, 128, &matrix, 1.0);
    let (golden_pasted, _, _) = rgb("pasted.png");
    let pasted_difference = mean_difference(&pasted, &golden_pasted);
    let untouched_difference = mean_difference(&frame_rgb, &golden_pasted);
    assert!(pasted_difference < untouched_difference * 0.5, "{pasted_difference:.3} vs {untouched_difference:.3}");
    eprintln!(
        "crop {crop_difference:.2}, swap {swap_difference:.2}, pasted {pasted_difference:.3} (unswapped frame: {untouched_difference:.3})"
    );
}

/// A mean of mask bytes over a rectangle of a square mask, 0 to 1.
fn mask_mean(mask: &[u8], side: usize, x: std::ops::Range<usize>, y: std::ops::Range<usize>) -> f64 {
    let mut sum = 0f64;
    let count = (x.len() * y.len()) as f64;
    for row in y {
        for column in x.clone() {
            sum += mask[row * side + column] as f64;
        }
    }
    sum / count / 255.0
}

#[test]
fn occlusion_mask_keeps_the_face_and_drops_what_covers_it() {
    let Some(packs) = packs() else { return };
    let model = packs.join("xseg_3.onnx");
    if !model.is_file() {
        eprintln!("{} not found; skipping", model.display());
        return;
    }
    let runtime = Runtime::shared(None).expect("runtime");
    let (crop, width, height) = rgb("aligned_128.png");
    assert_eq!((width, height), (128, 128));
    let mut occluder = gc_infer::runtime::Occluder::load(runtime, &model, Accelerator::Cpu).expect("occluder");

    let clear = occluder.mask(&crop, 128).expect("mask");
    assert_eq!(clear.len(), 128 * 128);
    assert!(mask_mean(&clear, 128, 48..80, 48..80) > 0.98, "the middle of the face is face");
    assert!(mask_mean(&clear, 128, 0..12, 0..12) < 0.02, "the corner is not");

    // A flat grey bar across the mouth and chin, like a cup held in front of the face.
    let mut covered = crop.clone();
    for y in 75..108 {
        for x in 20..128 {
            covered[(y * 128 + x) * 3..(y * 128 + x) * 3 + 3].copy_from_slice(&[90, 92, 102]);
        }
    }
    let mask = occluder.mask(&covered, 128).expect("mask");
    assert!(mask_mean(&mask, 128, 40..100, 82..102) < 0.05, "the bar is not face");
    assert!(mask_mean(&mask, 128, 40..88, 46..58) > 0.9, "the eyes still are");

    // The same crop at 256, the size of the sharper swap model.
    let big = image::resize_crop(&covered, 128, 256);
    let mask = occluder.mask(&big, 256).expect("mask");
    assert_eq!(mask.len(), 256 * 256);
    assert!(mask_mean(&mask, 256, 80..200, 164..204) < 0.05);
    assert!(mask_mean(&mask, 256, 80..176, 92..116) > 0.9);
}

/// Not a check: prints how long the occlusion model takes on each processor.
/// Run with `cargo test -p gc-infer --features runtime --release --test runtime -- --ignored --nocapture occlusion_speed`.
#[test]
#[ignore]
fn occlusion_speed() {
    let Some(packs) = packs() else { return };
    let model = packs.join("xseg_3.onnx");
    let cache = std::env::temp_dir().join("gc-occlusion-speed");
    let runtime = Runtime::shared(Some(&cache)).expect("runtime");
    let (crop, _, _) = rgb("aligned_128.png");
    for accelerator in [Accelerator::Cpu, Accelerator::Gpu, Accelerator::NeuralEngine] {
        let loading = std::time::Instant::now();
        let mut occluder = match gc_infer::runtime::Occluder::load(runtime, &model, accelerator) {
            Ok(occluder) => occluder,
            Err(error) => {
                println!("{accelerator:?}: failed to load: {error}");
                continue;
            }
        };
        let loaded = loading.elapsed();
        let first = std::time::Instant::now();
        let reference = occluder.mask(&crop, 128).expect("mask");
        let first = first.elapsed();
        let mut times = Vec::new();
        for _ in 0..40 {
            let started = std::time::Instant::now();
            occluder.mask(&crop, 128).expect("mask");
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{accelerator:?}: load {:.1} s, first run {:.0} ms, median {:.1} ms, worst {:.1} ms, face share {:.3}",
            loaded.as_secs_f64(),
            first.as_secs_f64() * 1000.0,
            times[times.len() / 2],
            times[times.len() - 1],
            reference.iter().map(|v| *v as f64).sum::<f64>() / reference.len() as f64 / 255.0
        );
    }
}

/// On Windows the models run on the graphics card through DirectML. Speed is
/// measured elsewhere; this checks that the card gives the same pictures as
/// the processor, for every model the live engine puts on it.
#[cfg(windows)]
#[test]
fn graphics_card_gives_the_same_pictures_as_the_processor() {
    let Some(packs) = packs() else { return };
    let runtime = Runtime::shared(None).expect("runtime");
    let (frame_rgb, width, height) = rgb("input.png");
    let frame = ImageView::new(&frame_rgb, width, height, width as usize * 3, PixelFormat::Rgb).unwrap();

    // Detector: the same faces, within a pixel.
    let model = packs.join("buffalo_l/det_10g.onnx");
    let mut on_cpu = Detector::load(runtime, &model, 640, Accelerator::Cpu).expect("detector");
    let mut on_card = Detector::load(runtime, &model, 640, Accelerator::Gpu).expect("detector on the card");
    let by_x = |mut faces: Vec<gc_infer::Detection>| {
        faces.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
        faces
    };
    let faces = by_x(on_cpu.detect(&frame, 0.5).expect("detect"));
    let card_faces = by_x(on_card.detect(&frame, 0.5).expect("detect on the card"));
    assert_eq!(faces.len(), card_faces.len());
    for (a, b) in faces.iter().zip(&card_faces) {
        for (p, q) in a.kps.iter().zip(&b.kps) {
            assert!((p[0] - q[0]).abs() < 1.0 && (p[1] - q[1]).abs() < 1.0, "keypoint {p:?} vs {q:?}");
        }
    }

    // Identity: the same direction.
    let (target, source) = (&faces[0], &faces[faces.len() - 1]);
    let model = packs.join("buffalo_l/w600k_r50.onnx");
    let embedding = IdentityEncoder::load(runtime, &model, Accelerator::Cpu).expect("encoder").embed(&frame, &source.kps).unwrap();
    let card_embedding =
        IdentityEncoder::load(runtime, &model, Accelerator::Gpu).expect("encoder on the card").embed(&frame, &source.kps).unwrap();
    let dot: f32 = embedding.iter().zip(&card_embedding).map(|(a, b)| a * b).sum();
    let norms = embedding.iter().map(|v| v * v).sum::<f32>().sqrt() * card_embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!(dot / norms > 0.9995, "identity vectors differ: cosine {}", dot / norms);

    // inswapper_128, as downloaded and as the mixed-precision copy the engine
    // uses on a card, against InsightFace's own swapped crop. A wholly
    // half-precision copy is 14 levels off here, which is why it is not used.
    let crop = image::warp_to_crop(&frame, &gc_align::estimate_norm(&target.kps, 128).unwrap(), 128);
    let (golden_swap, _, _) = rgb("swap_128.png");
    let emap = packs.join("inswapper_128.emap.bin");
    for file in ["inswapper_128.onnx", "inswapper_128.mixed.onnx"] {
        let model = packs.join(file);
        if !model.is_file() {
            eprintln!("{file} not found; skipping");
            continue;
        }
        let mut swapper =
            Swapper::load(runtime, &model, SwapperContract::INSWAPPER_128, Some(&emap), Accelerator::Gpu).expect("swapper on the card");
        let latent = swapper.identity(&embedding).unwrap();
        let swapped = swapper.swap(&crop, &latent).expect("swap on the card");
        let difference = mean_difference(&swapped.rgb, &golden_swap);
        eprintln!("{file} on the card: {difference:.2} levels from InsightFace's crop");
        assert!(difference < 1.5, "{file} on the card differs by {difference:.2}");
    }

    // HyperSwap 256 has no reference picture: its mixed-precision copy on the
    // card against the downloaded file on the processor.
    let (downloaded, mixed) = (packs.join("hyperswap_1b_256.onnx"), packs.join("hyperswap_1b_256.mixed.onnx"));
    if downloaded.is_file() && mixed.is_file() {
        let big = image::warp_to_crop(&frame, &gc_align::estimate_norm(&target.kps, 256).unwrap(), 256);
        let mut results = Vec::new();
        for (model, accelerator) in [(&downloaded, Accelerator::Cpu), (&mixed, Accelerator::Gpu)] {
            let mut swapper = Swapper::load(runtime, model, SwapperContract::HYPERSWAP_256, None, accelerator).expect("hyperswap");
            let latent = swapper.identity(&embedding).unwrap();
            results.push(swapper.swap(&big, &latent).expect("hyperswap").rgb);
        }
        let difference = mean_difference(&results[0], &results[1]);
        eprintln!("hyperswap, mixed copy on the card against the processor: {difference:.2} levels");
        assert!(difference < 1.0, "hyperswap on the card differs by {difference:.2}");
    }
    let model = packs.join("xseg_3.onnx");
    if model.is_file() {
        let mut masks = Vec::new();
        for accelerator in [Accelerator::Cpu, Accelerator::Gpu] {
            masks.push(gc_infer::runtime::Occluder::load(runtime, &model, accelerator).expect("occluder").mask(&crop, 128).unwrap());
        }
        let difference = mean_difference(&masks[0], &masks[1]);
        eprintln!("occlusion mask, card against processor: {difference:.2} levels");
        assert!(difference < 1.0, "the occlusion mask on the card differs by {difference:.2}");
    }
}
