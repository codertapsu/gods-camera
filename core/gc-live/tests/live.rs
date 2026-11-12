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

/// Whether the tests run every model on the processor.
///
/// They do where the processor keeps up with the tests' 30 frames a second,
/// which keeps them independent of what a machine's accelerators are busy
/// with. A PC's processor does not keep up: the Core i5-13400F takes half a
/// second for one swap. There the graphics card does the work, as in the app.
fn cpu_only() -> bool {
    !cfg!(windows)
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
        cpu_only: cpu_only(),
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

/// Whether a frame came out with the swap fully on the face.
fn swapped(result: &gc_live::FrameResult) -> bool {
    result.patch_sequence > 0 && result.opacity > 0.9
}

#[test]
fn hides_the_picture_without_a_swap_and_keeps_objects_in_front_of_the_face() {
    let Some(packs) = packs() else { return };
    let occlusion_model = packs.join("xseg_3.onnx").is_file();
    let (rgb, width, height) = photo();
    let picture = ImageView::new(&rgb, width, height, width as usize * 3, PixelFormat::Rgb).unwrap();
    let packs = vec![packs];
    let mut enroller = Enroller::new(&packs, None).expect("enroller");
    let mut faces = enroller.detect(&picture).expect("faces");
    faces.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
    let actor = faces.first().unwrap().clone();
    let donor = faces.last().unwrap().clone();
    let identity = enroller.identity(&picture, &donor.kps).expect("identity");

    let engine = LiveEngine::new(&LiveConfig {
        packs,
        cache_dir: None,
        detector: DetectorKind::Large,
        detector_size: 640,
        swap_model: SwapModel::Inswapper128,
        cpu_only: cpu_only(),
    })
    .expect("engine");
    assert_eq!(engine.stats().occlusion, occlusion_model);
    let hiding = LiveSettings { blur_when_unswapped: true, ..LiveSettings::default() };
    engine.set_settings(hiding);

    let clip = FaceClip::new(&picture, &actor, 640, 360, 60.0);
    let empty = vec![96u8; 640 * 360 * 4];
    let empty_view = ImageView::new(&empty, 640, 360, 640 * 4, PixelFormat::Bgra).unwrap();
    let mut time_ns = 1u64;
    let mut push = |view: &ImageView<'_>| {
        time_ns += 33_333_333;
        let result = engine.push_frame(view, time_ns);
        std::thread::sleep(Duration::from_millis(33));
        result
    };

    // No face chosen: the option asks for nothing.
    assert_eq!(push(&empty_view).blur, 0.0);
    // A face chosen and nobody in the picture: hidden. A person may be there whom the detector missed.
    engine.set_identity(Some(identity.clone()));
    assert_eq!(push(&empty_view).blur, 1.0);
    // The option off: nothing is hidden.
    engine.set_settings(LiveSettings::default());
    assert_eq!(push(&empty_view).blur, 0.0);
    engine.set_settings(hiding);

    // A face appears. The picture stays hidden until the swap is on it.
    let mut frame = Vec::new();
    let mut frames_until_swapped = None;
    let mut exposed = 0;
    for i in 0..90 {
        let pose = clip.pose(Motion::Still, i as f32 / 30.0);
        clip.render(&pose, &mut frame);
        let view = ImageView::new(&frame, clip.width, clip.height, clip.width as usize * 4, PixelFormat::Bgra).unwrap();
        let result = push(&view);
        if result.blur < 1.0 && result.opacity < 0.5 {
            exposed += 1;
        }
        if swapped(&result) && result.blur == 0.0 {
            frames_until_swapped = Some(i);
            break;
        }
    }
    let frames_until_swapped = frames_until_swapped.expect("the swap came on and the picture cleared");
    assert_eq!(exposed, 0, "the picture was sharp on {exposed} frames while the swap was under half strength");
    eprintln!("swap on and picture clear after {frames_until_swapped} frames");

    // Something moves in front of the mouth and chin: a flat bar, like a cup.
    if occlusion_model {
        let mut covered_mouth = 255u8;
        let mut eye = 0u8;
        for i in 0..45 {
            let pose = clip.pose(Motion::Still, (frames_until_swapped + 1 + i) as f32 / 30.0);
            clip.render(&pose, &mut frame);
            let kps = clip.keypoints(&pose);
            let eyes_apart = kps[1][0] - kps[0][0];
            let (top, bottom) = (kps[2][1] + 0.35 * (kps[3][1] - kps[2][1]), kps[3][1] + 0.7 * eyes_apart);
            let (left, right) = (kps[3][0] - 0.8 * eyes_apart, kps[4][0] + 3.0 * eyes_apart);
            for y in top.max(0.0) as usize..(bottom as usize).min(clip.height as usize) {
                for x in left.max(0.0) as usize..(right as usize).min(clip.width as usize) {
                    let at = (y * clip.width as usize + x) * 4;
                    frame[at..at + 4].copy_from_slice(&[104, 92, 90, 255]);
                }
            }
            let view =
                ImageView::new(&frame, clip.width, clip.height, clip.width as usize * 4, PixelFormat::Bgra).unwrap();
            push(&view);
            let patch = engine.patch().expect("a patch");
            // In the 128-pixel crop the mouth sits near (64, 95) and the left eye near (46, 52).
            covered_mouth = patch.rgba[(95 * 128 + 64) * 4 + 3];
            eye = patch.rgba[(52 * 128 + 46) * 4 + 3];
            if covered_mouth < 13 && eye > 200 {
                break;
            }
        }
        assert!(covered_mouth < 13, "the swap still covers the bar: mask {covered_mouth} of 255");
        assert!(eye > 200, "the swap left the eyes: mask {eye} of 255");
    }

    // The face leaves. The picture is hidden again before the swapped face fades.
    let mut exposed = 0;
    let mut hidden_after = None;
    for i in 0..20 {
        let result = push(&empty_view);
        if result.blur < 1.0 && result.opacity < 0.5 {
            exposed += 1;
        }
        if result.blur == 1.0 && hidden_after.is_none() {
            hidden_after = Some(i);
        }
    }
    assert_eq!(exposed, 0);
    assert!(hidden_after.expect("hidden") <= 3, "hidden only after {hidden_after:?} frames");
    assert!(engine.stats().last_error.is_none(), "{:?}", engine.stats().last_error);
}

#[test]
fn several_photos_of_one_person_make_one_identity() {
    let Some(packs) = packs() else { return };
    let (rgb, width, height) = photo();
    let picture = ImageView::new(&rgb, width, height, width as usize * 3, PixelFormat::Rgb).unwrap();
    let mut enroller = Enroller::new(&[packs], None).expect("enroller");
    let mut faces = enroller.detect(&picture).expect("faces");
    faces.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
    let (person, other) = (faces.first().unwrap().clone(), faces.last().unwrap().clone());

    // The same person three times: as in the photo, mirrored, and darker.
    let plain = enroller.identity(&picture, &person.kps).expect("identity");
    let mut mirrored_rgb = Vec::with_capacity(rgb.len());
    for row in rgb.chunks_exact(width as usize * 3) {
        for pixel in row.as_chunks::<3>().0.iter().rev() {
            mirrored_rgb.extend_from_slice(pixel);
        }
    }
    let mirrored_picture = ImageView::new(&mirrored_rgb, width, height, width as usize * 3, PixelFormat::Rgb).unwrap();
    // The person's keypoints in the mirrored photo: flipped, with left and right exchanged.
    let flip = |p: [f32; 2]| [width as f32 - 1.0 - p[0], p[1]];
    let k = person.kps;
    let mirrored_kps = [flip(k[1]), flip(k[0]), flip(k[2]), flip(k[4]), flip(k[3])];
    let mirrored = enroller.identity(&mirrored_picture, &mirrored_kps).expect("identity");
    let darker_rgb: Vec<u8> = rgb.iter().map(|v| (*v as f32 * 0.6) as u8).collect();
    let darker_picture = ImageView::new(&darker_rgb, width, height, width as usize * 3, PixelFormat::Rgb).unwrap();
    let darker = enroller.identity(&darker_picture, &person.kps).expect("identity");
    let somebody_else = enroller.identity(&picture, &other.kps).expect("identity");

    let combined = gc_live::combine_identities(&[plain.clone(), somebody_else, mirrored, darker]);
    let used: Vec<bool> = combined.photos.iter().map(|photo| photo.used).collect();
    let alike: Vec<String> = combined.photos.iter().map(|photo| format!("{:.2}", photo.similarity)).collect();
    eprintln!("likeness to the combined identity: {alike:?}");
    assert_eq!(used, [true, false, true, true]);
    assert_eq!(combined.identity.len(), 512);
    assert!(combined.photos[1].similarity < 0.2, "another person scored {}", combined.photos[1].similarity);
    for index in [0, 2, 3] {
        assert!(combined.photos[index].similarity > 0.6, "photo {index} scored {}", combined.photos[index].similarity);
    }
}
