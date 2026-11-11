//! Parity with InsightFace's Python implementation.
//!
//! The golden files come from `tools/modelkit/reference_run.py`. The detector's
//! raw outputs are decoded here and compared with InsightFace's own decode, and
//! the alignment matrix is compared with `face_align.norm_crop2`.

use gc_infer::scrfd::{self, ScrfdOutputs};
use serde_json::Value;
use std::path::PathBuf;

/// The golden files sit next to this test. On a device, where the build
/// machine's paths do not exist, `GC_GOLDEN_DIR` points at a pushed copy.
fn golden_dir() -> PathBuf {
    std::env::var_os("GC_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden"))
}

fn golden() -> Value {
    let text = std::fs::read_to_string(golden_dir().join("golden.json")).expect("golden.json");
    serde_json::from_str(&text).expect("valid golden.json")
}

fn floats(v: &Value) -> Vec<f32> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect()
}

#[test]
fn detector_decode_matches_insightface() {
    let g = golden();
    let det = &g["det"];
    let canvas = det["input_size"].as_u64().unwrap() as u32;
    let det_scale = det["det_scale"].as_f64().unwrap() as f32;

    let bytes = std::fs::read(golden_dir().join("det_raw.bin")).expect("det_raw.bin");
    let raw: Vec<f32> =
        bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
    // Output order: scores, boxes, keypoints, each for strides 8, 16, 32.
    let lens: Vec<usize> = det["output_shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_array().unwrap().iter().map(|d| d.as_u64().unwrap() as usize).product())
        .collect();
    assert_eq!(lens.iter().sum::<usize>(), raw.len());
    let mut parts = Vec::new();
    let mut offset = 0;
    for len in &lens {
        parts.push(&raw[offset..offset + len]);
        offset += len;
    }
    let out = ScrfdOutputs {
        scores: [parts[0], parts[1], parts[2]],
        bboxes: [parts[3], parts[4], parts[5]],
        kps: [parts[6], parts[7], parts[8]],
    };

    let geometry = scrfd::letterbox_geometry(
        g["image"]["width"].as_u64().unwrap() as u32,
        g["image"]["height"].as_u64().unwrap() as u32,
        canvas,
    );
    assert!((geometry.det_scale - det_scale).abs() < 1e-6);

    let mut ours = scrfd::decode(
        &out,
        canvas,
        det_scale,
        scrfd::DEFAULT_SCORE_THRESHOLD,
        scrfd::DEFAULT_NMS_THRESHOLD,
    )
    .unwrap();
    ours.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));

    let expected = det["faces"].as_array().unwrap();
    assert_eq!(ours.len(), expected.len(), "face count");
    for (o, e) in ours.iter().zip(expected) {
        let bbox = floats(&e["bbox"]);
        for i in 0..4 {
            assert!((o.bbox[i] - bbox[i]).abs() < 0.01, "bbox {:?} vs {:?}", o.bbox, bbox);
        }
        assert!((o.score - e["score"].as_f64().unwrap() as f32).abs() < 1e-5);
        for (p, q) in o.kps.iter().zip(e["kps"].as_array().unwrap()) {
            let q = floats(q);
            assert!((p[0] - q[0]).abs() < 0.01 && (p[1] - q[1]).abs() < 0.01);
        }
    }
}

#[test]
fn alignment_matrix_matches_insightface() {
    let g = golden();
    let kps_json = g["align"]["target_kps"].as_array().unwrap();
    let mut kps = [[0f32; 2]; 5];
    for (k, v) in kps.iter_mut().zip(kps_json) {
        let p = floats(v);
        *k = [p[0], p[1]];
    }
    let ours = gc_align::estimate_norm(&kps, 128).unwrap();
    let rows = g["align"]["matrix_128"].as_array().unwrap();
    for (i, expected_row) in rows.iter().enumerate() {
        for (j, expected) in floats(expected_row).into_iter().enumerate() {
            // Translation is in pixels; rotation and scale terms are unitless.
            let tolerance = if j == 2 { 0.02 } else { 1e-4 };
            let actual = ours.0[i][j];
            assert!((actual - expected).abs() < tolerance, "matrix[{i}][{j}]: {actual} vs {expected}");
        }
    }
}
