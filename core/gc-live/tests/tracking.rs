//! The tracker on real face texture, without any model: a face from
//! InsightFace's sample photo moves in three ways, and the tracker, started
//! once on the true keypoints, has to stay on it with no help from a detector.

use gc_infer::image::half_luma;
use gc_infer::Detection;
use gc_live::synthetic::{FaceClip, Motion};
use gc_live::{ImageView, PixelFormat};
use gc_track::{FlowConfig, FlowTracker, Gray, TrackOutcome};
use std::path::PathBuf;

fn golden_dir() -> PathBuf {
    std::env::var_os("GC_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../gc-infer/tests/golden"))
}

/// The leftmost face InsightFace found in the sample photo.
fn sample_face() -> Detection {
    let text = std::fs::read_to_string(golden_dir().join("golden.json")).expect("golden.json");
    let golden: serde_json::Value = serde_json::from_str(&text).unwrap();
    let face = &golden["det"]["faces"][0];
    let number = |v: &serde_json::Value| v.as_f64().unwrap() as f32;
    let mut detection = Detection { bbox: [0.0; 4], score: number(&face["score"]), kps: [[0.0; 2]; 5] };
    for i in 0..4 {
        detection.bbox[i] = number(&face["bbox"][i]);
    }
    for i in 0..5 {
        detection.kps[i] = [number(&face["kps"][i][0]), number(&face["kps"][i][1])];
    }
    detection
}

#[test]
fn tracker_stays_on_a_real_face_without_the_detector() {
    let picture = image::open(golden_dir().join("input.png")).expect("sample photo").to_rgb8();
    let (width, height) = picture.dimensions();
    let rgb = picture.into_raw();
    let photo = ImageView::new(&rgb, width, height, width as usize * 3, PixelFormat::Rgb).unwrap();
    let clip = FaceClip::new(&photo, &sample_face(), 640, 360, 60.0);

    for (motion, limit) in [(Motion::Still, 0.5), (Motion::Talking, 1.0), (Motion::Fast, 1.5)] {
        let mut tracker = FlowTracker::new(FlowConfig::default());
        let (mut frame, mut gray) = (Vec::new(), Vec::new());
        let mut worst = 0f32;
        for i in 0..90 {
            let pose = clip.pose(motion, i as f32 / 30.0);
            clip.render(&pose, &mut frame);
            let view = ImageView::new(&frame, 640, 360, 640 * 4, PixelFormat::Bgra).unwrap();
            let (w, h) = half_luma(&view, &mut gray);
            let luma = Gray { data: &gray, width: w, height: h, stride: w };
            // The tracker works at half size.
            let truth = clip.keypoints(&pose).map(|p| [p[0] / 2.0, p[1] / 2.0]);
            if i == 0 {
                tracker.observe(&luma, &truth);
                continue;
            }
            match tracker.track(&luma) {
                TrackOutcome::Tracked { kps, .. } => {
                    let error = kps
                        .iter()
                        .zip(&truth)
                        .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
                        .fold(0.0, f32::max);
                    // In frame pixels.
                    worst = worst.max(error * 2.0);
                }
                TrackOutcome::Lost => panic!("{motion:?}: lost the face on frame {i}"),
            }
        }
        eprintln!("{motion:?}: worst keypoint error over 3 seconds {worst:.2} px");
        assert!(worst < limit, "{motion:?}: {worst:.2} px");
    }
}
