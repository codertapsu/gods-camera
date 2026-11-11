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
