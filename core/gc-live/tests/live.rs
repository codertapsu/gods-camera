//! The live engine on a moving-face clip made from InsightFace's sample photo.
//!
//! Needs the model files, looked for in `GC_PACKS_DIR` and then in `../packs`
//! beside the repository. Without them the test passes with a note.

use gc_live::synthetic::{FaceClip, Motion};
use gc_live::{DetectorKind, Enroller, ImageView, LiveConfig, LiveEngine, LiveSettings, PixelFormat, SwapModel};
use std::path::PathBuf;
use std::time::Duration;

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

/// InsightFace's sample photo. On a device, where the build machine's paths
/// do not exist, `GC_GOLDEN_DIR` points at a pushed copy.
fn photo() -> (Vec<u8>, u32, u32) {
    let folder = std::env::var_os("GC_GOLDEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../gc-infer/tests/golden"));
    let picture = image::open(folder.join("input.png")).expect("sample photo").to_rgb8();
    let (width, height) = picture.dimensions();
    (picture.into_raw(), width, height)
}

#[test]
fn follows_and_swaps_a_moving_face() {
    let Some(packs) = packs() else { return };
    let (rgb, width, height) = photo();
    let picture = ImageView::new(&rgb, width, height, width as usize * 3, PixelFormat::Rgb).unwrap();

    // The leftmost face moves in the clip; the rightmost is the identity swapped in.
    let packs = vec![packs];
    let mut enroller = Enroller::new(&packs, None).expect("enroller");
    let mut faces = enroller.detect(&picture).expect("faces");
    faces.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
    let actor = faces.first().unwrap().clone();
    let donor = faces.last().unwrap().clone();
    let identity = enroller.identity(&picture, &donor.kps).expect("identity");
    assert_eq!(identity.len(), 512);

    let engine = LiveEngine::new(&LiveConfig {
        packs,
        cache_dir: None,
        detector: DetectorKind::Large,
        detector_size: 640,
        swap_model: SwapModel::Inswapper128,
        cpu_only: true,
    })
    .expect("engine");
    engine.set_settings(LiveSettings::default());
    engine.set_identity(Some(identity));
    assert_eq!(engine.crop_size(), 128);

    let clip = FaceClip::new(&picture, &actor, 640, 360, 60.0);
    let mut frame = Vec::new();
    let (mut errors, mut swapped_frames, mut last_sequence) = (Vec::new(), 0, 0);
    // Each frame's keypoints carried back onto the photo, where a perfect result would not move at all.
    let mut on_photo: Vec<[[f32; 2]; 5]> = Vec::new();
    let zoom = 60.0 / ((actor.kps[1][0] - actor.kps[0][0]).powi(2) + (actor.kps[1][1] - actor.kps[0][1]).powi(2)).sqrt();
    let frames = 75;
    for i in 0..frames {
        let seconds = i as f32 / 30.0;
        let pose = clip.pose(Motion::Talking, seconds);
        clip.render(&pose, &mut frame);
        let view = ImageView::new(&frame, clip.width, clip.height, clip.width as usize * 4, PixelFormat::Bgra).unwrap();
        let result = engine.push_frame(&view, (seconds * 1e9) as u64 + 1);
        // The models run on other threads; real frames arrive 33 ms apart.
        std::thread::sleep(Duration::from_millis(33));
        if result.face {
            let truth = clip.keypoints(&pose);
            let error = result.keypoints.iter().zip(&truth)
                .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
                .sum::<f32>() / 5.0;
            errors.push(error);
            let back = pose.invert().unwrap();
            on_photo.push(result.keypoints.map(|p| back.apply(p)));
        }
        if result.patch_sequence > 0 && result.opacity > 0.9 {
            swapped_frames += 1;
        }
        assert!(result.patch_sequence >= last_sequence, "patches must not go backwards");
        last_sequence = result.patch_sequence;
    }

    // Split the error in two. The offset is a constant difference between this
    // detector and the one that made the ground truth; it shifts the crop a
    // little and is harmless. The wobble is how much the keypoints move about
    // on a face that, seen from the photo, stands still; that is what shows.
    let count = on_photo.len().max(1) as f32;
    let mut offset = 0.0;
    let mut wobble = 0.0;
    for k in 0..5 {
        let mean = on_photo.iter().fold([0.0; 2], |m, f| [m[0] + f[k][0] / count, m[1] + f[k][1] / count]);
        offset += ((mean[0] - actor.kps[k][0]).powi(2) + (mean[1] - actor.kps[k][1]).powi(2)).sqrt() * zoom / 5.0;
        wobble += on_photo.iter().map(|f| (f[k][0] - mean[0]).powi(2) + (f[k][1] - mean[1]).powi(2)).sum::<f32>() / count / 5.0;
    }
    let wobble = wobble.sqrt() * zoom;

    let stats = engine.stats();
    eprintln!(
        "faces on {} of {frames} frames, swapped on {swapped_frames}, {} patches, keypoint offset {offset:.2} px, \
         wobble {wobble:.2} px, frame {:.2} ms, detect {:.1} ms, swap {:.0} ms, swap age {:.0} ms",
        errors.len(), stats.swaps, stats.frame_ms, stats.detect_ms, stats.swap_ms, stats.swap_age_ms,
    );
    assert!(stats.last_error.is_none(), "{:?}", stats.last_error);
    assert!(errors.len() >= frames * 8 / 10, "the face was followed on only {} frames", errors.len());
    assert!(offset < 6.0, "keypoint offset {offset:.2} px");
    assert!(wobble < 1.8, "keypoint wobble {wobble:.2} px");
    assert!(stats.swaps >= 3, "only {} patches", stats.swaps);
    assert!(swapped_frames >= frames / 3, "swap visible on only {swapped_frames} frames");

    // The patch is a full square with a mask that is open in the middle and closed at the corners.
    let patch = engine.patch().expect("a patch");
    assert_eq!(patch.rgba.len(), 128 * 128 * 4);
    assert_eq!(patch.rgba[(64 * 128 + 64) * 4 + 3], 255);
    assert_eq!(patch.rgba[3], 0);

    // Without an identity nothing is swapped.
    engine.set_identity(None);
    assert!(engine.patch().is_none());
}
