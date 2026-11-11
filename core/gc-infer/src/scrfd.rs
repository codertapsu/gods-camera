//! SCRFD detector: input geometry, anchor decoding and non-maximum suppression.

use gc_align::Keypoints;

/// Input normalisation: `(pixel - MEAN) * SCALE`, channels in RGB order.
pub const INPUT_MEAN: f32 = 127.5;
pub const INPUT_SCALE: f32 = 1.0 / 128.0;
pub const DEFAULT_SCORE_THRESHOLD: f32 = 0.5;
pub const DEFAULT_NMS_THRESHOLD: f32 = 0.4;

const STRIDES: [u32; 3] = [8, 16, 32];
const ANCHORS_PER_CELL: usize = 2;

/// How a frame is placed on the square detector canvas.
///
/// The frame is resized keeping its aspect ratio and placed in the top-left
/// corner; the rest of the canvas is zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Letterbox {
    pub resized_width: u32,
    pub resized_height: u32,
    /// Multiply frame coordinates by this to get canvas coordinates.
    pub det_scale: f32,
}

pub fn letterbox_geometry(frame_width: u32, frame_height: u32, canvas: u32) -> Letterbox {
    let im_ratio = frame_height as f32 / frame_width as f32;
    let (w, h) = if im_ratio > 1.0 {
        ((canvas as f32 / im_ratio) as u32, canvas)
    } else {
        (canvas, (canvas as f32 * im_ratio) as u32)
    };
    Letterbox { resized_width: w, resized_height: h, det_scale: h as f32 / frame_height as f32 }
}

/// A detected face in frame coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    /// `[x1, y1, x2, y2]`.
    pub bbox: [f32; 4],
    pub score: f32,
    pub kps: Keypoints,
}

/// Raw network outputs, one slice per stride (8, 16, 32).
pub struct ScrfdOutputs<'a> {
    /// One score per anchor.
    pub scores: [&'a [f32]; 3],
    /// Four distances per anchor: left, top, right, bottom, in stride units.
    pub bboxes: [&'a [f32]; 3],
    /// Ten offsets per anchor: five (x, y) pairs, in stride units.
    pub kps: [&'a [f32]; 3],
}

#[derive(Debug, PartialEq, Eq)]
pub struct ShapeError {
    pub stride: u32,
    pub expected_anchors: usize,
}

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stride {} output does not hold {} anchors", self.stride, self.expected_anchors)
    }
}

impl std::error::Error for ShapeError {}

/// Decode the outputs for a `canvas` x `canvas` input into faces in frame
/// coordinates, sorted by score, after suppression.
pub fn decode(
    out: &ScrfdOutputs<'_>,
    canvas: u32,
    det_scale: f32,
    score_threshold: f32,
    nms_threshold: f32,
) -> Result<Vec<Detection>, ShapeError> {
    let mut found = Vec::new();
    for (level, &stride) in STRIDES.iter().enumerate() {
        let cells = (canvas / stride) as usize;
        let anchors = cells * cells * ANCHORS_PER_CELL;
        let (scores, bboxes, kps) = (out.scores[level], out.bboxes[level], out.kps[level]);
        if scores.len() != anchors || bboxes.len() != anchors * 4 || kps.len() != anchors * 10 {
            return Err(ShapeError { stride, expected_anchors: anchors });
        }
        let s = stride as f32;
        for (i, &score) in scores.iter().enumerate() {
            if score < score_threshold {
                continue;
            }
            let cell = i / ANCHORS_PER_CELL;
            let (cx, cy) = ((cell % cells) as f32 * s, (cell / cells) as f32 * s);
            let b = &bboxes[i * 4..i * 4 + 4];
            let bbox = [
                (cx - b[0] * s) / det_scale,
                (cy - b[1] * s) / det_scale,
                (cx + b[2] * s) / det_scale,
                (cy + b[3] * s) / det_scale,
            ];
            let k = &kps[i * 10..i * 10 + 10];
            let mut pts = [[0f32; 2]; 5];
            for (j, p) in pts.iter_mut().enumerate() {
                p[0] = (cx + k[2 * j] * s) / det_scale;
                p[1] = (cy + k[2 * j + 1] * s) / det_scale;
            }
            found.push(Detection { bbox, score, kps: pts });
        }
    }
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(nms(found, nms_threshold))
}

/// Greedy non-maximum suppression over detections already sorted by score.
///
/// Uses the reference's `+1` area convention.
pub fn nms(sorted: Vec<Detection>, threshold: f32) -> Vec<Detection> {
    let area = |b: &[f32; 4]| (b[2] - b[0] + 1.0) * (b[3] - b[1] + 1.0);
    let mut keep: Vec<Detection> = Vec::new();
    'candidates: for d in sorted {
        for k in &keep {
            let w = (d.bbox[2].min(k.bbox[2]) - d.bbox[0].max(k.bbox[0]) + 1.0).max(0.0);
            let h = (d.bbox[3].min(k.bbox[3]) - d.bbox[1].max(k.bbox[1]) + 1.0).max(0.0);
            let inter = w * h;
            if inter / (area(&d.bbox) + area(&k.bbox) - inter) > threshold {
                continue 'candidates;
            }
        }
        keep.push(d);
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_matches_reference() {
        // Landscape: full width, height from the aspect ratio.
        let l = letterbox_geometry(1280, 720, 640);
        assert_eq!((l.resized_width, l.resized_height), (640, 360));
        assert!((l.det_scale - 0.5).abs() < 1e-6);
        // Portrait: full height.
        let p = letterbox_geometry(720, 1280, 640);
        assert_eq!((p.resized_width, p.resized_height), (360, 640));
    }

    #[test]
    fn nms_drops_overlaps_and_keeps_separate_faces() {
        let d = |x: f32, score: f32| Detection {
            bbox: [x, 0.0, x + 100.0, 100.0],
            score,
            kps: [[0.0; 2]; 5],
        };
        let kept = nms(vec![d(0.0, 0.9), d(5.0, 0.8), d(300.0, 0.7)], 0.4);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.9);
        assert_eq!(kept[1].score, 0.7);
    }

    #[test]
    fn wrong_shapes_are_rejected() {
        let e: [f32; 0] = [];
        let out = ScrfdOutputs { scores: [&e, &e, &e], bboxes: [&e, &e, &e], kps: [&e, &e, &e] };
        assert_eq!(
            decode(&out, 640, 1.0, 0.5, 0.4),
            Err(ShapeError { stride: 8, expected_anchors: 12800 })
        );
    }
}
