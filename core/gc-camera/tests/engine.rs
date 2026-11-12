//! The camera engine with a stand-in for the virtual camera and for the face swap.
//! No model files are needed.

use gc_align::Affine;
use gc_camera::{
    CameraEngine, Clip, LoopMode, LoopPoints, Mode, Output, SwapReport, SwapStage, Swapped, SwappedFace,
};
use gc_infer::image::{ImageView, PixelFormat};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (320, 180);
/// Source frames come a 120th of a second apart, so loops made of them play quickly.
const FRAME_NS: u64 = 8_333_333;
const PATIENCE: Duration = Duration::from_secs(20);

/// One picture as the virtual camera received it.
#[derive(Debug, Clone)]
struct Sent {
    width: u32,
    height: u32,
    host_ns: u64,
    nv12: Vec<u8>,
}

impl Sent {
    /// Brightness at a pixel, as sent.
    fn luma(&self, x: u32, y: u32) -> f32 {
        self.nv12[(y * self.width + x) as usize] as f32
    }

    fn mean_luma(&self) -> f32 {
        let plane = &self.nv12[..(self.width * self.height) as usize];
        plane.iter().map(|&value| value as u32).sum::<u32>() as f32 / plane.len() as f32
    }
}

#[derive(Default)]
struct Collected {
    wanted: Option<(u32, u32)>,
    sent: Vec<Sent>,
}

/// Stands in for the virtual camera.
#[derive(Clone, Default)]
struct Collector(Arc<Mutex<Collected>>);

impl Collector {
    fn asking_for(size: (u32, u32)) -> Self {
        let collector = Self::default();
        collector.0.lock().unwrap().wanted = Some(size);
        collector
    }

    fn sent(&self) -> Vec<Sent> {
        self.0.lock().unwrap().sent.clone()
    }

    fn count(&self) -> usize {
        self.0.lock().unwrap().sent.len()
    }

    fn wait_for(&self, count: usize) -> Vec<Sent> {
        let started = Instant::now();
        while self.count() < count {
            assert!(started.elapsed() < PATIENCE, "only {} of {count} pictures went out", self.count());
            std::thread::sleep(Duration::from_millis(1));
        }
        self.sent()
    }
}

impl Output for Collector {
    fn wanted(&self) -> Option<(u32, u32)> {
        self.0.lock().unwrap().wanted
    }

    fn send(&mut self, nv12: &[u8], width: u32, height: u32, host_ns: u64) {
        assert_eq!(nv12.len(), (width * height * 3 / 2) as usize);
        self.0.lock().unwrap().sent.push(Sent { width, height, host_ns, nv12: nv12.to_vec() });
    }
}

/// The brightness a grey of `level` is sent as: BT.709, video range.
fn sent_as(level: f32) -> f32 {
    level * 220.0 / 256.0 + 16.0
}

/// A grey picture, four bytes a pixel.
fn flat(level: u8) -> Vec<u8> {
    [level, level, level, 255].repeat((SIZE.0 * SIZE.1) as usize)
}

/// A picture that is `left` on its left half and `right` on its right half.
fn halves(left: u8, right: u8) -> Vec<u8> {
    let mut bgra = Vec::with_capacity((SIZE.0 * SIZE.1 * 4) as usize);
    for _ in 0..SIZE.1 {
        for x in 0..SIZE.0 {
            let level = if x < SIZE.0 / 2 { left } else { right };
            bgra.extend_from_slice(&[level, level, level, 255]);
        }
    }
    bgra
}

fn view(bgra: &[u8]) -> ImageView<'_> {
    ImageView::new(bgra, SIZE.0, SIZE.1, SIZE.0 as usize * 4, PixelFormat::Bgra).unwrap()
}

/// An engine that sends pictures of the test's size to a collector.
fn engine() -> (CameraEngine, Collector) {
    let collector = Collector::asking_for(SIZE);
    let engine = CameraEngine::new(Some(Box::new(collector.clone())));
    engine.source_started();
    engine.sync();
    (engine, collector)
}

/// Hand over a picture and wait until the engine has dealt with it.
fn show(engine: &CameraEngine, bgra: &[u8], number: u64) {
    engine.push_frame(&view(bgra), number * FRAME_NS);
    engine.sync();
}

fn close(a: f32, b: f32, within: f32) -> bool {
    (a - b).abs() <= within
}

/// Record `levels`, one picture each, and return the clip.
fn record(engine: &CameraEngine, levels: &[u8], first_number: u64) -> Clip {
    engine.start_recording();
    for (index, &level) in levels.iter().enumerate() {
        show(engine, &flat(level), first_number + index as u64);
        // Wait until the recording has taken this picture in, so that none is left out.
        let started = Instant::now();
        while engine.status().recorded_seconds + 1e-6 < index as f64 * FRAME_NS as f64 / 1e9 {
            assert!(started.elapsed() < PATIENCE, "the recording stopped taking pictures in");
            std::thread::sleep(Duration::from_millis(1));
            engine.sync();
        }
    }
    engine.stop_recording();
    let started = Instant::now();
    loop {
        if let Some(clip) = engine.take_clips().pop() {
            return Arc::try_unwrap(clip).expect("nothing else holds a clip that was just made");
        }
        assert!(started.elapsed() < PATIENCE, "the clip never arrived");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_live_picture_goes_out_as_it_came_in() {
    let (engine, collector) = engine();
    assert_eq!(engine.status().mode, Mode::Live);
    show(&engine, &flat(100), 7);
    let sent = collector.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].width, sent[0].height, sent[0].host_ns), (SIZE.0, SIZE.1, 7 * FRAME_NS));
    assert!(close(sent[0].mean_luma(), sent_as(100.0), 1.0), "sent as {}", sent[0].mean_luma());
    // Grey has no colour: both colour bytes sit at the middle.
    assert!(sent[0].nv12[(SIZE.0 * SIZE.1) as usize..].iter().all(|&byte| byte == 128));

    let status = engine.status();
    assert_eq!((status.mode, status.frames_out, status.output, status.watched), (Mode::Live, 1, SIZE, true));
    assert!(close(status.live_luma, 100.0, 1.0));
    assert!(!status.swapping && !status.picture_hidden && !status.is_recording);

    let mut preview = Vec::new();
    let info = engine.preview(&mut preview).expect("a preview");
    assert_eq!((info.width, info.height, info.sequence), (SIZE.0, SIZE.1, 1));
    assert_eq!(&preview[..4], &[100, 100, 100, preview[3]]);
    // Now that someone looks, every new picture is copied for them.
    show(&engine, &flat(50), 8);
    let info = engine.preview(&mut preview).unwrap();
    assert_eq!((info.sequence, preview[0]), (2, 50));
}

#[test]
fn nothing_goes_out_without_a_source() {
    let collector = Collector::asking_for(SIZE);
    let engine = CameraEngine::new(Some(Box::new(collector.clone())));
    assert_eq!(engine.status().mode, Mode::Idle);
    show(&engine, &flat(100), 1);
    assert_eq!(collector.count(), 0);
    engine.source_started();
    show(&engine, &flat(100), 2);
    assert_eq!(collector.count(), 1);
    engine.source_stopped();
    show(&engine, &flat(100), 3);
    assert_eq!((collector.count(), engine.status().mode), (1, Mode::Idle));
    assert_eq!(engine.preview(&mut Vec::new()).map(|info| info.sequence), Some(1));
}

#[test]
fn the_live_picture_can_be_flipped() {
    let (engine, collector) = engine();
    show(&engine, &halves(40, 200), 1);
    engine.update(|settings| settings.mirror = true);
    show(&engine, &halves(40, 200), 2);
    let sent = collector.sent();
    assert!(close(sent[0].luma(10, 90), sent_as(40.0), 1.0) && close(sent[0].luma(310, 90), sent_as(200.0), 1.0));
    assert!(close(sent[1].luma(10, 90), sent_as(200.0), 1.0) && close(sent[1].luma(310, 90), sent_as(40.0), 1.0));
}

/// A face swap that always answers the same.
struct FixedSwap(Swapped);

impl SwapStage for FixedSwap {
    fn process(&self, _: &ImageView<'_>, _: u64) -> Swapped {
        self.0.clone()
    }

    fn report(&self) -> SwapReport {
        SwapReport { face_found: self.0.face.is_some(), swap_ms: 21.0, swap_age_ms: 60.0, error: None }
    }
}

/// A white square of 64 pixels placed with its corner at (100, 50).
fn white_square(opacity: f32) -> SwappedFace {
    let patch = gc_live::Patch { sequence: 1, size: 64, rgba: vec![255; 64 * 64 * 4], frame_index: 0, frame_time_ns: 0 };
    SwappedFace { patch: Arc::new(patch), crop_to_frame: Affine([[1.0, 0.0, 100.0], [0.0, 1.0, 50.0]]), opacity }
}

#[test]
fn a_swapped_face_is_drawn_with_the_ai_badge() {
    let (engine, collector) = engine();
    engine.set_swap(Some(Arc::new(FixedSwap(Swapped { face: Some(white_square(1.0)), blur: 0.0 }))));
    show(&engine, &flat(100), 1);
    let with_badge = collector.sent().pop().unwrap();
    // The square is where it was placed, and nowhere else.
    assert!(close(with_badge.luma(132, 82), sent_as(255.0), 1.0), "the face is {}", with_badge.luma(132, 82));
    assert!(close(with_badge.luma(40, 150), sent_as(100.0), 1.0));
    let status = engine.status();
    assert!(status.swapping && status.face_found && !status.picture_hidden);
    assert_eq!((status.swap_ms, status.swap_age_ms), (21.0, 60.0));

    // The badge is in the top right corner: darker than the picture, with white in it.
    let corner = |sent: &Sent| -> Vec<f32> {
        (2..16).flat_map(|y| (280..316).map(move |x| (x, y))).map(|(x, y)| sent.luma(x, y)).collect()
    };
    let badge = corner(&with_badge);
    assert!(badge.iter().any(|&value| value < sent_as(60.0)), "no dark plate in the corner");
    assert!(badge.iter().any(|&value| value > sent_as(180.0)), "no white letters in the corner");

    engine.update(|settings| settings.show_ai_badge = false);
    show(&engine, &flat(100), 2);
    let without = collector.sent().pop().unwrap();
    assert!(corner(&without).iter().all(|&value| close(value, sent_as(100.0), 1.0)));
    assert!(close(without.luma(132, 82), sent_as(255.0), 1.0), "the face stays when the badge goes");

    // Mirrored, the face follows the picture to the other side.
    engine.update(|settings| settings.mirror = true);
    show(&engine, &flat(100), 3);
    let mirrored = collector.sent().pop().unwrap();
    assert!(close(mirrored.luma(319 - 132, 82), sent_as(255.0), 1.0));
    assert!(close(mirrored.luma(132, 82), sent_as(100.0), 1.0));

    // With the swap off, the picture is as it came.
    engine.set_swap(None);
    show(&engine, &flat(100), 4);
    assert!(close(collector.sent().pop().unwrap().luma(319 - 132, 82), sent_as(100.0), 1.0));
    assert!(!engine.status().swapping);
}

#[test]
fn the_picture_is_blurred_while_the_swap_is_not_on_the_face() {
    let (engine, collector) = engine();
    show(&engine, &halves(40, 200), 1);
    engine.set_swap(Some(Arc::new(FixedSwap(Swapped { face: None, blur: 1.0 }))));
    show(&engine, &halves(40, 200), 2);
    let sent = collector.sent();
    // Sharp: a pixel a little left of the middle is as dark as the far left.
    assert!(close(sent[0].luma(150, 90), sent_as(40.0), 1.0));
    // Blurred: the bright half has spread into it.
    assert!(sent[1].luma(150, 90) > sent_as(60.0), "still sharp: {}", sent[1].luma(150, 90));
    let status = engine.status();
    assert!(status.picture_hidden && !status.swapping);
}

#[test]
fn the_picture_takes_the_size_the_reading_app_asks_for() {
    let collector = Collector::default();
    let engine = CameraEngine::new(Some(Box::new(collector.clone())));
    engine.source_started();
    // Nothing reads the camera: nothing is sent, and the preview still works.
    show(&engine, &flat(100), 1);
    assert_eq!(collector.count(), 0);
    let status = engine.status();
    assert_eq!((status.watched, status.frames_out, status.output), (false, 1, (1280, 720)));
    assert_eq!(engine.preview(&mut Vec::new()).map(|info| (info.width, info.height)), Some((1280, 720)));

    collector.0.lock().unwrap().wanted = Some((640, 360));
    show(&engine, &flat(100), 2);
    collector.0.lock().unwrap().wanted = Some((1920, 1080));
    show(&engine, &flat(100), 3);
    let sizes: Vec<(u32, u32)> = collector.sent().iter().map(|sent| (sent.width, sent.height)).collect();
    assert_eq!(sizes, [(640, 360), (1920, 1080)]);
    let status = engine.status();
    assert_eq!((status.watched, status.output), (true, (1920, 1080)));
}

#[test]
fn pictures_that_come_too_fast_are_passed_over_not_queued() {
    let (engine, collector) = engine();
    let pictures: Vec<Vec<u8>> = (0..40u8).map(|index| flat(40 + index * 4)).collect();
    for (number, picture) in pictures.iter().enumerate() {
        engine.push_frame(&view(picture), number as u64 * FRAME_NS);
    }
    engine.sync();
    let sent = collector.sent();
    let status = engine.status();
    assert_eq!(status.frames_out as usize, sent.len());
    assert_eq!(status.frames_out + status.frames_skipped, 40, "{status:?}");
    // Whatever was passed over, the newest picture is the one shown last.
    assert_eq!(sent.last().unwrap().host_ns, 39 * FRAME_NS);
    assert!(sent.windows(2).all(|pair| pair[0].host_ns < pair[1].host_ns));
}

#[test]
fn a_recording_becomes_a_clip() {
    let (engine, _collector) = engine();
    // Not live yet as far as a recording goes: a recording needs a picture's rate.
    let levels: Vec<u8> = (0..30).map(|index| 60 + index * 6).collect();
    show(&engine, &flat(10), 1);
    let clip = record(&engine, &levels, 2);
    assert_eq!((clip.frame_count(), clip.width, clip.height), (30, SIZE.0, SIZE.1));
    assert_eq!(clip.name, "Loop 1");
    assert_eq!(clip.frame_rate, 120.0, "the clip plays at the rate its pictures arrived at");
    assert_eq!(clip.mode, LoopMode::Forward);
    // The clip holds what went out, in order.
    let brightness: Vec<f32> = clip.thumbnails().iter().map(|thumbnail| thumbnail.mean()).collect();
    for (index, &level) in levels.iter().enumerate() {
        assert!(close(brightness[index], level as f32, 1.0), "frame {index} is {}", brightness[index]);
    }
    assert!(!engine.status().is_recording);

    // The next one gets the next name, and stopping twice makes no second clip.
    let second = record(&engine, &levels[..10], 40);
    assert_eq!((second.name.as_str(), second.frame_count()), ("Loop 2", 10));
    engine.stop_recording();
    engine.sync();
    std::thread::sleep(Duration::from_millis(50));
    assert!(engine.take_clips().is_empty());
}

#[test]
fn a_recording_ends_by_itself_at_its_longest() {
    let (engine, _collector) = engine();
    // A fifth of a second at most. The expected rate also caps the number of
    // pictures, for sources faster than expected; here the clock ends it first.
    engine.update(|settings| {
        settings.max_clip_seconds = 0.2;
        settings.frame_rate = 240.0;
    });
    engine.start_recording();
    let picture = flat(90);
    let started = Instant::now();
    let mut number = 0;
    while engine.status().is_recording || number == 0 {
        number += 1;
        show(&engine, &picture, number);
        std::thread::sleep(Duration::from_millis(2));
        assert!(started.elapsed() < PATIENCE, "the recording never ended");
    }
    let started = Instant::now();
    let clip = loop {
        if let Some(clip) = engine.take_clips().pop() {
            break clip;
        }
        assert!(started.elapsed() < PATIENCE, "the clip never arrived");
        std::thread::sleep(Duration::from_millis(2));
    };
    // A fifth of a second at 120 pictures a second is 24 pictures after the
    // first; one more may slip in before the engine hears that the clip is full.
    assert!((25..=27).contains(&clip.frame_count()), "{} pictures", clip.frame_count());
    assert_eq!(clip.frame_rate, 120.0);
}

#[test]
fn a_loop_plays_its_frames_in_order_and_fades_over_its_seam() {
    let collector = Collector::asking_for(SIZE);
    let engine = CameraEngine::new(Some(Box::new(collector.clone())));
    engine.source_started();
    let levels: Vec<u8> = (0..30).map(|index| 60 + index * 6).collect();
    let mut clip = record(&engine, &levels, 1);
    // Without a live picture there is nothing to fade from: the loop starts at once.
    engine.source_stopped();
    engine.sync();
    let before = collector.count();

    clip.points = LoopPoints { start: 10, end: 20, crossfade: 4 };
    clip.mode = LoopMode::Forward;
    engine.start_loop(Arc::new(clip));
    let sent = collector.wait_for(before + 24)[before..].to_vec();
    assert_eq!(engine.status().mode, Mode::Looping);

    let level = |frame: usize| sent_as(levels[frame] as f32);
    for (index, picture) in sent.iter().enumerate().take(24) {
        let frame = 10 + index % 10;
        // Over the last four frames the loop fades into the four before its start.
        let expected = match frame >= 16 {
            true => {
                let mix = (frame - 15) as f32 / 5.0;
                level(frame) * (1.0 - mix) + level(frame - 10) * mix
            }
            false => level(frame),
        };
        assert!(close(picture.mean_luma(), expected, 2.0), "picture {index}: {} for {expected}", picture.mean_luma());
    }
    // A picture every 120th of a second, as the clip was recorded.
    let spacing = (sent[23].host_ns - sent[3].host_ns) as f64 / 20.0 / 1e6;
    assert!((spacing - 8.33).abs() < 1.0, "a picture every {spacing:.2} ms");

    // No source to go back to: stopping ends the loop at once.
    engine.stop_loop();
    engine.sync();
    assert_eq!(engine.status().mode, Mode::Idle);
    let after = collector.count();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(collector.count(), after, "pictures still go out after the loop stopped");
}

#[test]
fn a_loop_can_play_back_and_forth() {
    let collector = Collector::asking_for(SIZE);
    let engine = CameraEngine::new(Some(Box::new(collector.clone())));
    engine.source_started();
    let levels: Vec<u8> = (0..12).map(|index| 60 + index * 12).collect();
    let mut clip = record(&engine, &levels, 1);
    engine.source_stopped();
    engine.sync();
    let before = collector.count();
    clip.points = LoopPoints { start: 2, end: 6, crossfade: 0 };
    clip.mode = LoopMode::PingPong;
    engine.start_loop(Arc::new(clip));
    let sent = collector.wait_for(before + 12)[before..].to_vec();
    for (picture, frame) in sent.iter().zip([2usize, 3, 4, 5, 4, 3, 2, 3, 4, 5, 4, 3]) {
        assert!(close(picture.mean_luma(), sent_as(levels[frame] as f32), 2.0));
    }
}

#[test]
fn live_and_loop_fade_into_each_other() {
    let (engine, collector) = engine();
    let mut clip = record(&engine, &[200; 30], 1);
    clip.points = LoopPoints { start: 0, end: 30, crossfade: 0 };
    // The live picture is dark; the loop is bright.
    show(&engine, &flat(20), 100);
    let before = collector.count();
    engine.start_loop(Arc::new(clip));
    let sent = collector.wait_for(before + 12)[before..].to_vec();
    let (live, looped) = (sent_as(20.0), sent_as(200.0));
    // Eight pictures from live to loop, each a step further.
    for (index, picture) in sent.iter().enumerate().take(8) {
        let progress = (index + 1) as f32 / 8.0;
        let expected = live + (looped - live) * progress;
        assert!(close(picture.mean_luma(), expected, 2.0), "fading in, picture {index}: {}", picture.mean_luma());
    }
    assert!(sent[8..12].iter().all(|picture| close(picture.mean_luma(), looped, 2.0)));

    // While the loop plays, live pictures are not sent, but the newest one is
    // what the picture returns to.
    let during = collector.count();
    show(&engine, &flat(60), 101);
    assert!(collector.sent()[during..].iter().all(|picture| close(picture.mean_luma(), looped, 2.0)));
    assert_eq!(engine.status().mode, Mode::Looping);

    engine.stop_loop();
    let started = Instant::now();
    while engine.status().mode != Mode::Live {
        assert!(started.elapsed() < PATIENCE, "the loop never ended");
        std::thread::sleep(Duration::from_millis(1));
    }
    // The last eight pictures the loop sent are the way back to live, and the
    // one before them was still the loop alone.
    let sent = collector.sent();
    let faded = &sent[sent.len() - 8..];
    assert!(close(sent[sent.len() - 9].mean_luma(), looped, 2.0));
    let live = sent_as(60.0);
    for (index, picture) in faded.iter().enumerate() {
        let progress = (index + 1) as f32 / 8.0;
        let expected = looped + (live - looped) * progress;
        assert!(close(picture.mean_luma(), expected, 2.0), "fading out, picture {index}: {}", picture.mean_luma());
    }
    // And the live picture goes out again.
    show(&engine, &flat(60), 102);
    assert!(close(collector.sent().pop().unwrap().mean_luma(), live, 1.0));
}

#[test]
fn a_loop_can_carry_a_badge_and_a_clip_that_does_not_fit_is_refused() {
    let collector = Collector::asking_for(SIZE);
    let engine = CameraEngine::new(Some(Box::new(collector.clone())));
    engine.source_started();
    let mut clip = record(&engine, &[120; 12], 1);
    engine.source_stopped();
    engine.update(|settings| settings.show_loop_badge = true);
    engine.sync();

    // Loop points beyond the clip's end: nothing starts.
    let honest = clip.points;
    clip.points = LoopPoints { start: 0, end: 500, crossfade: 0 };
    let clip = Arc::new(clip);
    engine.start_loop(Arc::clone(&clip));
    engine.sync();
    assert_eq!(engine.status().mode, Mode::Idle);

    let mut clip = Arc::try_unwrap(clip).expect("the engine kept a clip it refused");
    clip.points = honest;
    let before = collector.count();
    engine.start_loop(Arc::new(clip));
    let picture = collector.wait_for(before + 1).pop().unwrap();
    // Top left: the red plate with white letters.
    let corner: Vec<f32> = (6..17).flat_map(|y| (8..50).map(move |x| (x, y))).map(|(x, y)| picture.luma(x, y)).collect();
    assert!(corner.iter().any(|&value| value > sent_as(200.0)), "no white letters in the corner");
    assert!(corner.iter().any(|&value| value < sent_as(105.0)), "no plate in the corner");
    assert!(close(picture.luma(300, 170), sent_as(120.0), 2.0));
}

#[test]
fn dropping_the_engine_stops_everything() {
    let (engine, collector) = engine();
    let clip = record(&engine, &[90; 12], 1);
    engine.start_recording();
    engine.start_loop(Arc::new(clip));
    engine.sync();
    let started = Instant::now();
    drop(engine);
    assert!(started.elapsed() < Duration::from_secs(2));
    let count = collector.count();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(collector.count(), count);
}
