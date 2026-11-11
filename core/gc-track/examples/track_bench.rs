//! Spike S4: tracker speed and accuracy on a synthetic sequence.
//!
//! A face moves, turns and changes size across a 640x360 frame, which is the
//! half-resolution luma plane of a 720p camera. The tracker follows it with and
//! without periodic re-anchoring, the way the detector re-anchors it live.
//!
//! Run with: cargo run --release -p gc-track --example track_bench

use gc_align::{Affine, Keypoints};
use gc_track::flow::{FlowConfig, FlowTracker, TrackOutcome};
use gc_track::synthetic::Scene;
use std::time::Instant;

const FRAMES: u32 = 300;

fn pose(scene: &Scene, t: u32) -> Affine {
    let t = t as f32;
    // Up to about 6 px per frame of travel, with slow rotation and zoom.
    let shift = [40.0 * (0.15 * t).sin(), 22.0 * (0.11 * t).cos() - 22.0];
    scene.motion_about_face(0.09 * (0.05 * t).sin(), 1.0 + 0.05 * (0.04 * t).sin(), shift)
}

fn apply(m: &Affine, kps: &Keypoints) -> Keypoints {
    let mut out = *kps;
    for k in &mut out {
        *k = m.apply(*k);
    }
    out
}

fn max_error(a: &Keypoints, b: &Keypoints) -> f32 {
    a.iter().zip(b).map(|(p, q)| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()).fold(0.0, f32::max)
}

fn run(scene: &Scene, frames: &[gc_track::synthetic::Frame], anchor_every: Option<u32>, noise_label: &str) {
    let kps0 = scene.keypoints();
    let mut tracker = FlowTracker::new(FlowConfig::default());
    tracker.observe(&frames[0].gray(), &apply(&pose(scene, 0), &kps0));
    let (mut times, mut errors, mut lost) = (Vec::new(), Vec::new(), 0u32);
    for t in 1..FRAMES {
        let truth = apply(&pose(scene, t), &kps0);
        let started = Instant::now();
        let outcome = tracker.track(&frames[t as usize].gray());
        times.push(started.elapsed().as_secs_f64() * 1000.0);
        match outcome {
            TrackOutcome::Tracked { kps, .. } => errors.push(max_error(&kps, &truth)),
            TrackOutcome::Lost => {
                lost += 1;
                tracker.observe(&frames[t as usize].gray(), &truth);
            }
        }
        if anchor_every.is_some_and(|n| t % n == 0) {
            tracker.observe(&frames[t as usize].gray(), &truth);
        }
    }
    times.sort_by(f64::total_cmp);
    errors.sort_by(f32::total_cmp);
    let mean_ms = times.iter().sum::<f64>() / times.len() as f64;
    let p95_ms = times[(times.len() as f64 * 0.95) as usize - 1];
    let mean_err = errors.iter().sum::<f32>() / errors.len() as f32;
    let p95_err = errors[(errors.len() as f32 * 0.95) as usize - 1];
    let anchoring = match anchor_every {
        Some(n) => format!("re-anchored every {n} frames"),
        None => "never re-anchored".to_string(),
    };
    println!(
        "{noise_label:<10} {anchoring:<28} time mean {mean_ms:.2} ms, p95 {p95_ms:.2} ms | \
         error mean {mean_err:.2} px, p95 {p95_err:.2} px, worst {:.2} px | lost {lost}",
        errors.last().copied().unwrap_or(f32::NAN)
    );
}

fn main() {
    let scene = Scene::new(640, 360, 21);
    println!("{FRAMES} frames of 640x360, eye distance {:.0} px", scene.eye_distance());
    for (noise, label) in [(0.0, "clean"), (3.0, "noise 3")] {
        let frames: Vec<_> = (0..FRAMES).map(|t| scene.render(&pose(&scene, t), noise, t + 1)).collect();
        run(&scene, &frames, None, label);
        run(&scene, &frames, Some(8), label);
    }
}
