//! The camera's pipe, with a test standing in for the media source.
#![cfg(windows)]

use gc_win::protocol::{self, FrameHeader, Wanted, FRAME_HEADER_BYTES};
use gc_win::{Feed, FeedError};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const PATIENCE: Duration = Duration::from_secs(10);
const HD: Wanted = Wanted { width: 1280, height: 720, frame_rate: (30, 1) };

/// A pipe name no other test and no running app uses.
fn test_name() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(r"\\.\pipe\godscamera.test.{}.{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < PATIENCE, "waited too long for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Connect as the media source does. The pipe takes one source at a time, and
/// is busy for a moment after the last one left.
fn connect(name: &str) -> File {
    let mut source = None;
    until("the pipe to accept a source", || {
        source = OpenOptions::new().read(true).write(true).open(name).ok();
        source.is_some()
    });
    source.unwrap()
}

/// A picture whose every byte says which picture it is.
fn picture(wanted: &Wanted, mark: u8) -> Vec<u8> {
    vec![mark; protocol::nv12_bytes(wanted.width, wanted.height)]
}

/// Read pictures on a thread of their own, so a test that goes wrong fails instead of waiting for ever.
fn read_pictures(mut source: File) -> Receiver<(FrameHeader, Vec<u8>)> {
    let (send, receive) = channel();
    std::thread::spawn(move || loop {
        let mut header = [0u8; FRAME_HEADER_BYTES];
        if source.read_exact(&mut header).is_err() {
            return;
        }
        let Some(header) = protocol::parse_frame_header(&header) else { return };
        let mut payload = vec![0u8; header.payload_bytes as usize];
        if source.read_exact(&mut payload).is_err() || send.send((header, payload)).is_err() {
            return;
        }
    });
    receive
}

#[test]
fn pictures_reach_the_source_whole_and_in_order() {
    let name = test_name();
    let feed = Feed::open_named(&name).unwrap();
    assert_eq!(feed.pipe_name(), name);
    assert_eq!(feed.watched(), None, "nothing has connected yet");
    assert!(!feed.send(&picture(&HD, 1), HD.width, HD.height, 1), "a picture nobody watches is not taken");

    let mut source = connect(&name);
    source.write_all(&protocol::request(&HD)).unwrap();
    until("the request to arrive", || feed.watched().is_some());
    assert_eq!(feed.watched(), Some(HD));

    let pictures = read_pictures(source);
    for mark in 1..=5u8 {
        let made = gc_win::now_100ns();
        assert!(feed.send(&picture(&HD, mark), HD.width, HD.height, made));
        let (header, payload) = pictures.recv_timeout(PATIENCE).expect("a picture");
        assert_eq!((header.width, header.height), (HD.width, HD.height));
        assert_eq!(header.sequence, mark as u64, "pictures are numbered from one");
        assert_eq!(header.time_100ns, made);
        assert!(payload.iter().all(|&byte| byte == mark), "picture {mark} arrived changed");
    }
    let stats = feed.stats();
    assert_eq!((stats.sent, stats.dropped, stats.connects, stats.write_failures), (5, 0, 1, 0));
}

#[test]
fn a_newer_picture_replaces_one_the_source_has_not_taken() {
    let name = test_name();
    let feed = Feed::open_named(&name).unwrap();
    let mut source = connect(&name);
    source.write_all(&protocol::request(&HD)).unwrap();
    until("the request to arrive", || feed.watched().is_some());

    // Twenty pictures while the source reads nothing: the pipe has room for a few.
    for mark in 1..=20u8 {
        assert!(feed.send(&picture(&HD, mark), HD.width, HD.height, mark as i64), "sending never waits");
    }
    let pictures = read_pictures(source);
    let mut seen = Vec::new();
    loop {
        let (header, payload) = pictures.recv_timeout(PATIENCE).expect("the pictures the pipe held");
        assert!(payload.iter().all(|&byte| byte as u64 == header.sequence), "a picture arrived under another's number");
        seen.push(header.sequence);
        if header.sequence == 20 {
            break;
        }
    }
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]), "out of order: {seen:?}");
    let stats = feed.stats();
    assert_eq!(stats.sent, seen.len() as u64);
    assert_eq!(stats.sent + stats.dropped, 20, "every picture was either written or replaced: {stats:?}");
    assert!(stats.dropped >= 10, "the pipe cannot have held this many: {stats:?}");
    assert_eq!(stats.write_failures, 0);
}

#[test]
fn the_source_may_leave_and_come_back_and_change_its_mind() {
    let name = test_name();
    let feed = Feed::open_named(&name).unwrap();

    let mut first = connect(&name);
    first.write_all(&protocol::request(&HD)).unwrap();
    until("the first request", || feed.watched() == Some(HD));
    // The app reading the camera switches to another size.
    let full_hd = Wanted { width: 1920, height: 1080, frame_rate: (30, 1) };
    first.write_all(&protocol::request(&full_hd)).unwrap();
    until("the second request", || feed.watched() == Some(full_hd));
    drop(first);
    until("the feed to notice the source has gone", || feed.watched().is_none());
    assert!(!feed.send(&picture(&HD, 1), HD.width, HD.height, 1));

    let mut second = connect(&name);
    second.write_all(&protocol::request(&HD)).unwrap();
    until("the request after coming back", || feed.watched() == Some(HD));
    let pictures = read_pictures(second);
    assert!(feed.send(&picture(&HD, 7), HD.width, HD.height, 1));
    let (header, _) = pictures.recv_timeout(PATIENCE).expect("a picture after coming back");
    assert_eq!(header.sequence, 1, "the picture refused while nobody watched took no number");
    assert_eq!(feed.stats().connects, 2);
}

#[test]
fn a_request_that_makes_no_sense_ends_the_connection() {
    let name = test_name();
    let feed = Feed::open_named(&name).unwrap();
    for spoil in [0usize, 4, 16, 9] {
        // A wrong magic, a wrong version, a format that is not NV12, a width far too large.
        let mut request = protocol::request(&HD);
        request[spoil] ^= 0x40;
        let mut source = connect(&name);
        source.write_all(&request).unwrap();
        let mut byte = [0u8; 1];
        assert!(source.read(&mut byte).map_or(true, |read| read == 0), "the feed should have hung up");
        assert_eq!(feed.watched(), None);
    }
    assert_eq!(feed.stats().sent, 0);
    assert_eq!(feed.stats().connects, 4);
}

#[test]
fn pictures_of_the_wrong_shape_are_refused() {
    let name = test_name();
    let feed = Feed::open_named(&name).unwrap();
    let mut source = connect(&name);
    source.write_all(&protocol::request(&HD)).unwrap();
    until("the request to arrive", || feed.watched().is_some());
    let good = picture(&HD, 1);
    assert!(!feed.send(&good[1..], HD.width, HD.height, 1), "too few bytes");
    assert!(!feed.send(&good, HD.width + 1, HD.height, 1), "an odd width");
    assert!(!feed.send(&[0u8; 96], 8, 8, 1), "too small");
    assert!(!feed.send(&[], 7680, 4320, 1), "too large");
    assert!(feed.send(&good, HD.width, HD.height, 1));
}

#[test]
fn a_source_that_stops_reading_is_cut_off() {
    let name = test_name();
    let feed = Feed::open_named(&name).unwrap();
    let mut source = connect(&name);
    source.write_all(&protocol::request(&HD)).unwrap();
    until("the request to arrive", || feed.watched().is_some());

    // Keep sending to a source that takes nothing. Once the pipe is full a
    // write waits, and after a second the feed gives the source up.
    let started = Instant::now();
    until("the feed to give the source up", || {
        feed.send(&picture(&HD, 1), HD.width, HD.height, 1);
        feed.stats().write_failures > 0
    });
    assert!(started.elapsed() >= Duration::from_millis(900), "gave up too early: {:?}", started.elapsed());
    until("the feed to stop being watched", || feed.watched().is_none());
    assert_eq!(feed.stats().write_failures, 1);

    // The camera works again for the next source.
    drop(source);
    let mut next = connect(&name);
    next.write_all(&protocol::request(&HD)).unwrap();
    until("the next source's request", || feed.watched() == Some(HD));
}

#[test]
fn only_one_program_can_hold_the_pipe() {
    let name = test_name();
    let first = Feed::open_named(&name).unwrap();
    assert!(matches!(Feed::open_named(&name), Err(FeedError::Taken)));
    drop(first);
    // Dropping the feed stops its thread and frees the name.
    Feed::open_named(&name).expect("the name is free again");
}

#[test]
fn dropping_the_feed_does_not_wait_for_the_source() {
    let name = test_name();
    let feed = Feed::open_named(&name).unwrap();
    let mut source = connect(&name);
    source.write_all(&protocol::request(&HD)).unwrap();
    until("the request to arrive", || feed.watched().is_some());
    // Fill the pipe so that a write is waiting when the feed is dropped.
    for mark in 1..=6u8 {
        feed.send(&picture(&HD, mark), HD.width, HD.height, 1);
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    drop(feed);
    assert!(started.elapsed() < Duration::from_millis(500), "dropping took {:?}", started.elapsed());
    let mut rest = Vec::new();
    // The source sees the end of the pipe, not a hang.
    let _ = source.read_to_end(&mut rest);
}

#[test]
fn the_clock_counts_in_tenths_of_a_microsecond() {
    let (before, started) = (gc_win::now_100ns(), Instant::now());
    std::thread::sleep(Duration::from_millis(50));
    let (ticks, real) = (gc_win::now_100ns() - before, started.elapsed());
    let expected = real.as_nanos() as i64 / 100;
    assert!((ticks - expected).abs() < 20_000, "{ticks} ticks in {real:?}");
}

/// The C header is the contract; `protocol` repeats it for Rust.
mod header {
    use gc_win::protocol;

    fn text() -> String {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/windows/shared/gc_vcam_protocol.h");
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
    }

    /// What a `#define` in the header says, without its comment.
    fn define(header: &str, name: &str) -> String {
        let line = header
            .lines()
            .find_map(|line| line.strip_prefix("#define ")?.strip_prefix(name)?.strip_prefix(' '))
            .unwrap_or_else(|| panic!("the header no longer defines {name}"));
        line.split("//").next().unwrap().trim().to_string()
    }

    /// A wide string literal's characters.
    fn string(literal: &str) -> String {
        literal.strip_prefix("L\"").and_then(|rest| rest.strip_suffix('"')).expect("a wide string").replace("\\\\", "\\")
    }

    fn number(literal: &str) -> u32 {
        let digits = literal.trim_end_matches('u');
        match digits.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).unwrap(),
            None => digits.parse().unwrap(),
        }
    }

    /// The fields of a packed struct in the header, as (type, name).
    fn fields(header: &str, name: &str) -> Vec<(String, String)> {
        let start = header.find(&format!("typedef struct {name} {{")).unwrap_or_else(|| panic!("no struct {name}"));
        let body = &header[start..start + header[start..].find(&format!("}} {name};")).unwrap()];
        body.lines()
            .skip(1)
            .filter_map(|line| {
                let mut words = line.split("//").next()?.split_whitespace();
                Some((words.next()?.to_string(), words.next()?.trim_end_matches(';').to_string()))
            })
            .collect()
    }

    fn bytes(fields: &[(String, String)]) -> usize {
        fields
            .iter()
            .map(|(kind, _)| match kind.as_str() {
                "uint16_t" => 2,
                "uint32_t" => 4,
                "uint64_t" | "int64_t" => 8,
                other => panic!("a field type this test does not know: {other}"),
            })
            .sum()
    }

    fn names(fields: &[(String, String)]) -> Vec<&str> {
        fields.iter().map(|(_, name)| name.as_str()).collect()
    }

    #[test]
    fn the_rust_copy_says_what_the_header_says() {
        let header = text();
        assert_eq!(string(&define(&header, "GC_VCAM_CLSID_STRING")), protocol::CLSID);
        assert_eq!(
            string(&define(&header, "GC_VCAM_ATTRIBUTE_PIPE_NAME_STRING")).trim_matches(['{', '}']).replace('-', ""),
            format!("{:032X}", protocol::ATTRIBUTE_PIPE_NAME)
        );
        assert_eq!(string(&define(&header, "GC_VCAM_FRIENDLY_NAME")), protocol::FRIENDLY_NAME);
        assert_eq!(string(&define(&header, "GC_VCAM_PIPE_PREFIX")), protocol::PIPE_PREFIX);
        assert_eq!(number(&define(&header, "GC_VCAM_MAGIC_REQUEST")), protocol::MAGIC_REQUEST);
        assert_eq!(number(&define(&header, "GC_VCAM_MAGIC_FRAME")), protocol::MAGIC_FRAME);
        assert_eq!(number(&define(&header, "GC_VCAM_PROTOCOL_VERSION")), protocol::VERSION as u32);
        assert_eq!(number(&define(&header, "GC_VCAM_FOURCC_NV12")), protocol::FOURCC_NV12);
        assert_eq!(number(&define(&header, "GC_VCAM_MAX_WIDTH")), protocol::MAX_WIDTH);
        assert_eq!(number(&define(&header, "GC_VCAM_MAX_HEIGHT")), protocol::MAX_HEIGHT);
        assert!(header.contains("#pragma pack(push, 1)"), "the structs are no longer packed");

        let request = fields(&header, "gc_vcam_request");
        assert_eq!(bytes(&request), protocol::REQUEST_BYTES);
        assert_eq!(
            names(&request),
            ["magic", "version", "size", "width", "height", "format", "frame_rate_num", "frame_rate_den"]
        );
        let frame = fields(&header, "gc_vcam_frame_header");
        assert_eq!(bytes(&frame), protocol::FRAME_HEADER_BYTES);
        assert_eq!(
            names(&frame),
            ["magic", "version", "header_size", "width", "height", "format", "payload_size", "sequence", "time_100ns"]
        );
    }

    #[test]
    fn a_header_and_a_request_survive_the_trip() {
        let wanted = protocol::Wanted { width: 1920, height: 1080, frame_rate: (30000, 1001) };
        assert_eq!(protocol::parse_request(&protocol::request(&wanted)), Some(wanted));
        let header = protocol::parse_frame_header(&protocol::frame_header(1920, 1080, 42, -7)).unwrap();
        assert_eq!((header.width, header.height, header.sequence, header.time_100ns), (1920, 1080, 42, -7));
        assert_eq!(header.payload_bytes, 1920 * 1080 * 3 / 2);
        // The numbers go out least significant byte first, as the C structs lie in memory.
        assert_eq!(&protocol::frame_header(1920, 1080, 42, -7)[..8], [0x47, 0x43, 0x56, 0x46, 1, 0, 40, 0]);
    }
}
