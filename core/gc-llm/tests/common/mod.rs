//! A stand-in for a provider's server, listening on 127.0.0.1 only, for
//! tests that need no network and no key. It writes down every request it
//! gets and answers from the test's script: whole answers, answers streamed
//! in pieces cut at awkward places, or silence.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gc_llm::HttpOptions;
use serde_json::Value;

/// A request as the server got it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    /// The path with its query.
    pub path: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("the request body is JSON")
    }

    /// Everything the request carried, as one text: to look for a key in.
    pub fn everything(&self) -> String {
        let headers: Vec<String> = self.headers.iter().map(|(key, value)| format!("{key}: {value}")).collect();
        format!("{} {}\n{}\n{}", self.method, self.path, headers.join("\n"), String::from_utf8_lossy(&self.body))
    }
}

/// What the server answers.
pub enum Answer {
    Whole { status: u16, headers: Vec<(String, String)>, body: Vec<u8> },
    /// Sent in these pieces, with a pause after each, as chunked transfer.
    Pieces { status: u16, content_type: String, pieces: Vec<Vec<u8>>, pause: Duration },
    /// Nothing for this long, then the connection closes.
    Silence(Duration),
}

impl Answer {
    pub fn json(status: u16, body: Value) -> Answer {
        Answer::Whole { status, headers: vec![("content-type".into(), "application/json".into())], body: body.to_string().into_bytes() }
    }

    pub fn json_with(status: u16, headers: &[(&str, &str)], body: Value) -> Answer {
        let mut all = vec![("content-type".to_string(), "application/json".to_string())];
        all.extend(headers.iter().map(|(key, value)| (key.to_string(), value.to_string())));
        Answer::Whole { status, headers: all, body: body.to_string().into_bytes() }
    }

    pub fn text(status: u16, content_type: &str, body: &str) -> Answer {
        Answer::Whole { status, headers: vec![("content-type".into(), content_type.into())], body: body.as_bytes().to_vec() }
    }

    /// Server-sent events, cut into pieces at awkward places.
    pub fn events(stream: &str) -> Answer {
        let pieces = awkward_pieces(stream.as_bytes());
        Answer::Pieces { status: 200, content_type: "text/event-stream".into(), pieces, pause: Duration::from_millis(2) }
    }
}

/// Cuts bytes into pieces of 1, 5, 2, 11, 3 and 7 bytes in turn, so that the
/// cuts fall inside lines, between `\r` and `\n`, and inside letters that
/// take more than one byte.
pub fn awkward_pieces(bytes: &[u8]) -> Vec<Vec<u8>> {
    let sizes = [1, 5, 2, 11, 3, 7];
    let mut pieces = Vec::new();
    let mut at = 0;
    let mut turn = 0;
    while at < bytes.len() {
        let end = (at + sizes[turn % sizes.len()]).min(bytes.len());
        pieces.push(bytes[at..end].to_vec());
        at = end;
        turn += 1;
    }
    pieces
}

/// Writes events in the form the providers send them.
pub fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| if name.is_empty() { format!("data: {data}\n\n") } else { format!("event: {name}\ndata: {data}\n\n") })
        .collect()
}

type Script = dyn Fn(&Seen, usize) -> Answer + Send + Sync;

pub struct Mock {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    /// Starts the server. The script is given each request and how many came
    /// before it, and says what to answer.
    pub fn start(script: impl Fn(&Seen, usize) -> Answer + Send + Sync + 'static) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port on 127.0.0.1");
        let address = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let script: Arc<Script> = Arc::new(script);
        let record = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (record, script) = (record.clone(), script.clone());
                std::thread::spawn(move || serve(stream, &record, &*script));
            }
        });
        Mock { address, seen }
    }

    /// The server's address with `path` after it.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(stream: TcpStream, record: &Mutex<Vec<Seen>>, script: &Script) {
    let mut writer = stream.try_clone().expect("a second handle on the connection");
    let mut reader = BufReader::new(stream);
    loop {
        let Some(seen) = read_request(&mut reader) else { return };
        let number = {
            let mut all = record.lock().unwrap();
            all.push(seen.clone());
            all.len() - 1
        };
        match script(&seen, number) {
            Answer::Whole { status, headers, body } => {
                let mut head = format!("HTTP/1.1 {status} Status\r\ncontent-length: {}\r\n", body.len());
                for (key, value) in headers {
                    head.push_str(&format!("{key}: {value}\r\n"));
                }
                head.push_str("\r\n");
                if writer.write_all(head.as_bytes()).and_then(|()| writer.write_all(&body)).and_then(|()| writer.flush()).is_err() {
                    return;
                }
            }
            Answer::Pieces { status, content_type, pieces, pause } => {
                let head = format!("HTTP/1.1 {status} Status\r\ncontent-type: {content_type}\r\ntransfer-encoding: chunked\r\n\r\n");
                if writer.write_all(head.as_bytes()).is_err() {
                    return;
                }
                for piece in pieces {
                    let chunk = [format!("{:x}\r\n", piece.len()).into_bytes(), piece, b"\r\n".to_vec()].concat();
                    if writer.write_all(&chunk).and_then(|()| writer.flush()).is_err() {
                        return;
                    }
                    std::thread::sleep(pause);
                }
                if writer.write_all(b"0\r\n\r\n").and_then(|()| writer.flush()).is_err() {
                    return;
                }
            }
            Answer::Silence(how_long) => {
                std::thread::sleep(how_long);
                return;
            }
        }
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Option<Seen> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut words = line.split_whitespace();
    let (method, path) = (words.next()?.to_string(), words.next()?.to_string());
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (key, value) = header.split_once(':')?;
        headers.push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let length = headers.iter().find(|(key, _)| key == "content-length").and_then(|(_, value)| value.parse().ok()).unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Seen { method, path, headers, body })
}

/// Short time limits and waits, so that the retry and timeout tests are quick.
pub fn quick() -> HttpOptions {
    HttpOptions {
        connect_timeout: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(5),
        answer_timeout: Duration::from_secs(20),
        retries: 3,
        first_wait: Duration::from_millis(10),
        longest_wait: Duration::from_secs(5),
    }
}

/// A folder of the test's own, empty.
pub fn scratch(tag: &str) -> std::path::PathBuf {
    let folder = std::env::temp_dir().join(format!("gc-llm-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

/// A small PNG file, made here.
pub fn png(width: u32, height: u32) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(width, height, |x, y| image::Rgb([(x * 40) as u8, (y * 40) as u8, 128]));
    let mut bytes = Vec::new();
    picture.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    bytes
}

/// A small JPEG file, made here.
pub fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(width, height, |x, y| image::Rgb([(x * 40) as u8, (y * 40) as u8, 128]));
    let mut bytes = Vec::new();
    picture.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg).unwrap();
    bytes
}

pub fn base64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
