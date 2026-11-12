//! `gc-ocr`: read the text in pictures and print it as JSON.
//!
//! The hub starts this program for every few photos of a document, as it
//! starts the Swift tool of the same name on the Mac, and expects the same
//! answer: a list with one entry per picture, each with its lines.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use gc_ocr::{Settings, TextReader};

const USAGE: &str = "usage: gc-ocr [--packs <folder>]... [--threads <count>] [--timing] <picture>...

Reads the text in each picture and prints one JSON list of pages.
The model files are looked for in the --packs folders; without any, in the
folder named by GC_PACK_STORE, and then in the folder Packs beside this program.";

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut packs: Vec<PathBuf> = Vec::new();
    let mut pictures: Vec<PathBuf> = Vec::new();
    let mut settings = Settings::default();
    let mut timing = false;
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--packs") => match arguments.next() {
                Some(folder) => packs.push(folder.into()),
                None => return usage(),
            },
            Some("--threads") => match arguments.next().and_then(|count| count.to_str()?.parse().ok()) {
                Some(count) => settings.threads = count,
                None => return usage(),
            },
            Some("--timing") => timing = true,
            Some("--help" | "-h") => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some(option) if option.starts_with("--") => return usage(),
            _ => pictures.push(argument.into()),
        }
    }
    if pictures.is_empty() {
        return usage();
    }
    if packs.is_empty() {
        packs.extend(std::env::var_os("GC_PACK_STORE").map(PathBuf::from));
        packs.extend(std::env::current_exe().ok().and_then(|program| Some(program.parent()?.join("Packs"))));
    }

    let mut reader = match TextReader::load(&packs, settings) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("gc-ocr: {error}");
            return ExitCode::FAILURE;
        }
    };
    let pages: Vec<gc_doc::OcrPage> = pictures.iter().map(|picture| reader.read_file(picture)).collect();
    let mut out = std::io::stdout().lock();
    let printed = serde_json::to_writer(&mut out, &pages).map_err(std::io::Error::from).and_then(|()| writeln!(out));
    if let Err(error) = printed {
        eprintln!("gc-ocr: the answer could not be printed: {error}");
        return ExitCode::FAILURE;
    }
    if timing {
        let took = reader.timing();
        eprintln!(
            "{} pictures, {} lines; finding {:.0} ms a picture, reading {:.0} ms a line",
            took.pictures,
            took.lines,
            took.finding.as_secs_f64() * 1000.0 / took.pictures.max(1) as f64,
            took.reading.as_secs_f64() * 1000.0 / took.lines.max(1) as f64,
        );
    }
    ExitCode::SUCCESS
}
