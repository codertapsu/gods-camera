//! Reading pictures through Media Foundation, tested on a video file the
//! test makes itself. No camera is opened here: a camera shows the room it
//! is in, and turning one on is the user's to allow.
#![cfg(windows)]

use gc_win::{cameras, Capture, CaptureSource};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::{
    IMFByteStream, IMFAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video,
    MFShutdown, MFStartup, MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive, MFSTARTUP_FULL, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

const SIZE: (u32, u32) = (640, 360);
const FRAMES: usize = 45;

/// What the lower part of frame `index` is painted: red, then green, then blue, as blue, green, red.
fn colour(index: usize) -> [u8; 3] {
    match index * 3 / FRAMES {
        0 => [30, 30, 220],
        1 => [30, 200, 30],
        _ => [220, 40, 40],
    }
}

/// Write a short H.264 film: a white band along the top, and below it a
/// colour that changes twice. Thirty pictures a second for a second and a half.
fn write_film(path: &Path) -> windows::core::Result<()> {
    let (width, height) = SIZE;
    let pack = |high: u32, low: u32| ((high as u64) << 32) | low as u64;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None::<&IMFByteStream>, None::<&IMFAttributes>)?;

        let film = MFCreateMediaType()?;
        film.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        film.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        film.SetUINT32(&MF_MT_AVG_BITRATE, 2_000_000)?;
        film.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        film.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
        film.SetUINT64(&MF_MT_FRAME_RATE, pack(30, 1))?;
        film.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        let stream = writer.AddStream(&film)?;

        let pictures = MFCreateMediaType()?;
        pictures.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        pictures.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        pictures.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        pictures.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
        pictures.SetUINT64(&MF_MT_FRAME_RATE, pack(30, 1))?;
        pictures.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        // The first row in memory is the top row of the picture.
        pictures.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
        writer.SetInputMediaType(stream, &pictures, None::<&IMFAttributes>)?;
        writer.BeginWriting()?;

        let bytes = (width * height * 4) as usize;
        for index in 0..FRAMES {
            let buffer = MFCreateMemoryBuffer(bytes as u32)?;
            let mut memory = std::ptr::null_mut();
            buffer.Lock(&mut memory, None, None)?;
            let picture = std::slice::from_raw_parts_mut(memory, bytes);
            let [b, g, r] = colour(index);
            for (y, row) in picture.chunks_exact_mut(width as usize * 4).enumerate() {
                let pixel = if y < height as usize / 4 { [255, 255, 255, 255] } else { [b, g, r, 255] };
                for target in row.as_chunks_mut::<4>().0 {
                    *target = pixel;
                }
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(bytes as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(index as i64 * 333_333)?;
            sample.SetSampleDuration(333_333)?;
            writer.WriteSample(stream, &sample)?;
        }
        writer.Finalize()?;
        drop(writer);
        MFShutdown()?;
    }
    Ok(())
}

/// One picture as the reader delivered it: its size, a pixel from the band at
/// the top and one from the coloured part, and when it came.
#[derive(Debug, Clone, Copy)]
struct Seen {
    size: (u32, u32),
    top: [u8; 3],
    below: [u8; 3],
    at: Instant,
    host_ns: u64,
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 40)
}

#[test]
fn a_video_file_plays_at_its_own_speed_the_right_way_up_and_starts_again() {
    let film: PathBuf = std::env::temp_dir().join(format!("gc-win-film-{}.mp4", std::process::id()));
    let _ = std::fs::remove_file(&film);
    write_film(&film).expect("writing the test film");
    assert!(std::fs::metadata(&film).unwrap().len() > 2_000, "the film is empty");

    let seen = Arc::new(Mutex::new(Vec::<Seen>::new()));
    let collected = Arc::clone(&seen);
    let capture = Capture::start(
        CaptureSource::File(film.clone()),
        Box::new(move |picture, host_ns| {
            let pixel = |x: usize, y: usize| {
                let row = picture.row(y);
                [row[x * 4], row[x * 4 + 1], row[x * 4 + 2]]
            };
            collected.lock().unwrap().push(Seen {
                size: (picture.width, picture.height),
                top: pixel(320, 20),
                below: pixel(320, 300),
                at: Instant::now(),
                host_ns,
            });
        }),
    )
    .expect("the film should open");

    // One pass is a second and a half; wait for well into the second pass.
    let started = Instant::now();
    while seen.lock().unwrap().len() < FRAMES + 20 {
        assert!(started.elapsed() < Duration::from_secs(30), "only {} pictures came", seen.lock().unwrap().len());
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(capture);
    let count = seen.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(seen.lock().unwrap().len(), count, "pictures still come after the capture was dropped");
    let seen = seen.lock().unwrap().clone();

    assert!(seen.iter().all(|picture| picture.size == SIZE));
    // The white band is at the top: the picture is the right way up.
    assert!(seen.iter().all(|picture| near(picture.top, [255, 255, 255])), "the first picture's top is {:?}", seen[0].top);
    // Red, then green, then blue, a third of the film each; then red again.
    for (index, picture) in seen.iter().enumerate().take(FRAMES) {
        // The picture at a change of colour may be either side of it.
        if index % (FRAMES / 3) == 0 {
            continue;
        }
        assert!(near(picture.below, colour(index)), "picture {index} is {:?}, not {:?}", picture.below, colour(index));
    }
    assert!(near(seen[FRAMES + 5].below, colour(5)), "the film did not start again: {:?}", seen[FRAMES + 5].below);

    // Thirty a second by the clock, and the stamps go up with the clock.
    let per_picture = (seen[FRAMES - 1].at - seen[5].at).as_secs_f64() * 1000.0 / (FRAMES - 6) as f64;
    assert!((per_picture - 33.3).abs() < 3.0, "a picture every {per_picture:.1} ms");
    assert!(seen.windows(2).all(|pair| pair[1].host_ns >= pair[0].host_ns));
    let _ = std::fs::remove_file(&film);
}

#[test]
fn what_cannot_be_opened_says_so_at_once() {
    let missing = std::env::temp_dir().join("gc-win-no-such-film.mp4");
    let error = Capture::start(CaptureSource::File(missing), Box::new(|_, _| {})).err().expect("no such file");
    assert_ne!(error.code, 0, "{error}");

    let not_a_film = std::env::temp_dir().join(format!("gc-win-not-a-film-{}.mp4", std::process::id()));
    std::fs::write(&not_a_film, b"words, not pictures").unwrap();
    assert!(Capture::start(CaptureSource::File(not_a_film.clone()), Box::new(|_, _| {})).is_err());
    let _ = std::fs::remove_file(&not_a_film);

    // A camera that is not there.
    let error = Capture::start(CaptureSource::Camera(r"\\?\usb#no_such_camera".into()), Box::new(|_, _| {})).err().expect("no such camera");
    assert!(!error.to_string().is_empty());
}

#[test]
fn the_list_of_cameras_leaves_out_the_apps_own() {
    // Asking which cameras there are opens none of them.
    let found = cameras();
    assert!(found.iter().all(|camera| !camera.name.starts_with("God's Camera")));
    assert!(found.iter().all(|camera| !camera.id.is_empty() && !camera.name.is_empty()));
    println!("{} camera(s) attached", found.len());
}
