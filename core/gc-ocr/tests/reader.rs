//! The text reader with its real models, on two small pictures drawn by
//! `tools/ocrkit/make_test_pictures.py`.
//!
//! Needs the model files. They are looked for in `GC_PACKS_DIR`, then in
//! `../packs` beside the repository. Without them the tests that need them
//! pass with a note.

use std::path::PathBuf;
use std::process::Command;

use gc_doc::text::edit_distance;
use gc_doc::OcrPage;
use gc_ocr::{Settings, TextReader};

/// The text of `tests/data/page.png`.
const TITLE: &str = "Chợ nổi buổi sớm";
const LINES: [&str; 5] = [
    "Trời vừa hửng sáng, ghe xuồng đã tấp nập trên sông.",
    "Người bán treo hàng lên cây bẹo: khóm, bưởi, khoai lang.",
    "Giá hôm nay: 25.000 đồng một ký, rẻ hơn tuần trước.",
    "The market opens at 5 a.m. and closes before noon.",
    "Ở đây, mỗi chuyến đò đều chở đầy ắp tiếng cười.",
];
/// The text of `tests/data/long-line.png`.
const LONG: &str = "Mỗi sáng, bà Tư chèo chiếc xuồng nhỏ chở đầy rau muống, bông súng và cá lóc ra chợ nổi, bán hết hàng trước khi mặt trời lên cao rồi mới thong thả quay về nhà.";

fn packs() -> Option<PathBuf> {
    let folder = std::env::var_os("GC_PACKS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packs"));
    match TextReader::missing_file(std::slice::from_ref(&folder)) {
        None => Some(folder),
        Some(name) => {
            eprintln!("{name} not found in {}; skipping", folder.display());
            None
        }
    }
}

fn picture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn reader(packs: PathBuf) -> TextReader {
    TextReader::load(&[packs], Settings::default()).expect("the reader loads")
}

/// A folder of this test's own, empty.
fn scratch(tag: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("gc-ocr-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

#[test]
fn a_page_is_read_line_by_line() {
    let Some(packs) = packs() else { return };
    let page = reader(packs).read_file(&picture("page.png"));
    assert_eq!(page.error, None);
    assert_eq!((page.width, page.height), (1000, 470));
    let read: Vec<&str> = page.lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(read.len(), 6, "{read:#?}");

    // From the top down, and nearly every character right. A full stop at
    // the end of a line or a tone mark can slip: which ones depends on where
    // exactly the line's box falls, and moving the box by a pixel changes one
    // to four of them. Five in 280 is the plan's 98 %.
    let expected: Vec<&str> = std::iter::once(TITLE).chain(LINES).collect();
    let wrong: usize = read.iter().zip(&expected).map(|(read, expected)| edit_distance(read, expected, 10)).sum();
    assert!(wrong <= 5, "{wrong} characters wrong in {read:#?}");
    // The words themselves must be there, tone marks and all, in the heading at least.
    assert_eq!(read[0], TITLE);

    assert!(page.lines[0].title, "the heading is larger than the rest");
    assert!(page.lines[1..].iter().all(|line| !line.title));
    for line in &page.lines {
        assert!(line.confidence > 0.8, "{}: {}", line.text, line.confidence);
        let [x0, y0, x1, y1, x2, y2, x3, y3] = line.quad;
        // Clockwise from the top-left, inside the picture, starting at the left margin.
        assert!(x0 < x1 && x3 < x2 && y0 < y3 && y1 < y2, "{:?}", line.quad);
        assert!([x0, x1, x2, x3].iter().all(|x| (0.0..1000.0).contains(x)) && [y0, y1, y2, y3].iter().all(|y| (0.0..470.0).contains(y)));
        assert!((x0 - 60.0).abs() < 12.0, "{:?}", line.quad);
    }
    assert!(page.lines.windows(2).all(|pair| pair[1].quad[1] > pair[0].quad[1]), "lines come from the top down");
}

#[test]
fn a_line_too_wide_for_the_recogniser_is_read_in_pieces() {
    let Some(packs) = packs() else { return };
    let page = reader(packs).read_file(&picture("long-line.png"));
    assert_eq!(page.error, None);
    assert_eq!(page.lines.len(), 1, "{:#?}", page.lines);
    let line = &page.lines[0];
    // 45 times as wide as it is high; the recogniser takes 32 times.
    assert!(line.quad[2] - line.quad[0] > 2300.0, "{:?}", line.quad);
    let wrong = edit_distance(&line.text, LONG, 10);
    assert!(wrong <= 3, "{wrong} characters wrong in {}", line.text);
    // The pieces are joined with one space, and no word is cut or said twice.
    assert_eq!(line.text.split(' ').count(), LONG.split(' ').count(), "{}", line.text);
}

#[test]
fn a_file_that_is_no_picture_is_reported_and_the_next_one_is_still_read() {
    let Some(packs) = packs() else { return };
    let folder = scratch("no-picture");
    let not_a_picture = folder.join("notes.jpg");
    std::fs::write(&not_a_picture, b"this is text, whatever its name says").unwrap();
    let mut reader = reader(packs);
    let page = reader.read_file(&not_a_picture);
    assert_eq!(page.error.as_deref(), Some("not a readable picture"));
    assert!(page.lines.is_empty());
    assert_eq!(reader.read_file(&picture("page.png")).lines.len(), 6);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn the_program_prints_what_the_hub_expects() {
    let Some(packs) = packs() else { return };
    let folder = scratch("program");
    let missing = folder.join("missing.png");
    let output = Command::new(env!("CARGO_BIN_EXE_gc-ocr"))
        .arg("--packs")
        .arg(&packs)
        .arg(picture("page.png"))
        .arg(&missing)
        .output()
        .expect("gc-ocr starts");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    // The hub reads exactly this: a list of pages, one per picture, in the order given.
    let printed = String::from_utf8_lossy(&output.stdout);
    let pages: Vec<OcrPage> = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("not a list of pages ({error}): {printed}"));
    assert_eq!(pages.len(), 2, "{printed}");
    assert!(pages[0].path.ends_with("page.png"));
    assert_eq!(pages[0].lines.len(), 6, "{printed}");
    assert_eq!(pages[0].lines[0].text, TITLE, "{printed}");
    assert_eq!(pages[1].error.as_deref(), Some("not a readable picture"));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn the_program_says_which_model_file_is_missing() {
    let folder = scratch("no-models");
    let output = Command::new(env!("CARGO_BIN_EXE_gc-ocr"))
        .arg("--packs")
        .arg(&folder)
        .arg(picture("page.png"))
        .env_remove("GC_PACK_STORE")
        .output()
        .expect("gc-ocr starts");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("was not found"), "{said}");
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn the_program_explains_itself_when_given_no_picture() {
    for arguments in [&[][..], &["--packs"][..], &["--threads", "many", "page.png"][..], &["--nonsense", "page.png"][..]] {
        let output = Command::new(env!("CARGO_BIN_EXE_gc-ocr")).args(arguments).output().expect("gc-ocr starts");
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage: gc-ocr"));
    }
}
