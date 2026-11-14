//! Reading server-sent events, the way every provider streams an answer.
//!
//! The network hands the bytes over in pieces of any size: a piece can end
//! in the middle of a line, between the `\r` and the `\n` of a line end, or
//! inside a Vietnamese letter that takes three bytes. So bytes are kept until
//! a whole line is there, and only whole lines are turned into text; a line
//! end is a single byte that never occurs inside a UTF-8 character.

use std::collections::VecDeque;

/// One event: its name (`message` when the server gave none) and its data,
/// with the lines of a multi-line `data` field joined by `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Event {
    pub name: String,
    pub data: String,
}

#[derive(Debug, Default)]
pub(crate) struct Parser {
    pending: Vec<u8>,
    name: String,
    data: String,
    has_data: bool,
    ready: VecDeque<Event>,
}

impl Parser {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        let mut start = 0;
        let mut at = 0;
        while at < self.pending.len() {
            match self.pending[at] {
                b'\n' => {
                    self.line(start, at);
                    at += 1;
                    start = at;
                }
                b'\r' => {
                    // A `\r` at the very end may be the first half of `\r\n`:
                    // wait for the next piece to know.
                    if at + 1 == self.pending.len() {
                        break;
                    }
                    self.line(start, at);
                    at += if self.pending[at + 1] == b'\n' { 2 } else { 1 };
                    start = at;
                }
                _ => at += 1,
            }
        }
        self.pending.drain(..start);
    }

    /// The stream has ended: what is left counts as a last line, and an event
    /// still being gathered is handed out even without the blank line that
    /// should close it.
    pub fn finish(&mut self) {
        if !self.pending.is_empty() {
            let end = if self.pending.last() == Some(&b'\r') { self.pending.len() - 1 } else { self.pending.len() };
            self.line(0, end);
            self.pending.clear();
        }
        self.dispatch();
    }

    pub fn next_event(&mut self) -> Option<Event> {
        self.ready.pop_front()
    }

    fn line(&mut self, start: usize, end: usize) {
        let line = String::from_utf8_lossy(&self.pending[start..end]).into_owned();
        if line.is_empty() {
            self.dispatch();
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_str(), ""),
        };
        match field {
            "event" => self.name = value.to_string(),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            _ => {}
        }
    }

    fn dispatch(&mut self) {
        if self.has_data {
            let name = if self.name.is_empty() { "message".to_string() } else { std::mem::take(&mut self.name) };
            self.ready.push_back(Event { name, data: std::mem::take(&mut self.data) });
        }
        self.name.clear();
        self.data.clear();
        self.has_data = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(chunks: &[&[u8]]) -> Vec<Event> {
        let mut parser = Parser::default();
        let mut out = Vec::new();
        for chunk in chunks {
            parser.feed(chunk);
            while let Some(event) = parser.next_event() {
                out.push(event);
            }
        }
        parser.finish();
        while let Some(event) = parser.next_event() {
            out.push(event);
        }
        out
    }

    #[test]
    fn events_are_read_whole() {
        let stream = b"event: message_start\ndata: {\"a\":1}\n\n: a comment\nevent: ping\ndata: {}\n\ndata: plain\n\n";
        let read = events(&[stream]);
        assert_eq!(
            read,
            vec![
                Event { name: "message_start".into(), data: "{\"a\":1}".into() },
                Event { name: "ping".into(), data: "{}".into() },
                Event { name: "message".into(), data: "plain".into() },
            ]
        );
    }

    #[test]
    fn any_split_gives_the_same_events() {
        // Windows line ends, a three-byte letter, and a data field over two lines.
        let stream = "event: delta\r\ndata: {\"text\":\"Mùa nước nổi\"}\r\n\r\n\
                      event: two\r\ndata: first\r\ndata: second\r\n\r\n"
            .as_bytes();
        let whole = events(&[stream]);
        assert_eq!(whole.len(), 2);
        assert_eq!(whole[0].data, "{\"text\":\"Mùa nước nổi\"}");
        assert_eq!(whole[1].data, "first\nsecond");
        for size in 1..stream.len() {
            let pieces: Vec<&[u8]> = stream.chunks(size).collect();
            assert_eq!(events(&pieces), whole, "pieces of {size} bytes");
        }
        // Split between `\r` and `\n` exactly.
        let at = stream.iter().position(|b| *b == b'\r').unwrap() + 1;
        assert_eq!(events(&[&stream[..at], &stream[at..]]), whole);
    }

    #[test]
    fn a_last_event_without_its_blank_line_is_kept() {
        let read = events(&[b"data: [DONE]"]);
        assert_eq!(read, vec![Event { name: "message".into(), data: "[DONE]".into() }]);
        let read = events(&[b"data: one\r"]);
        assert_eq!(read, vec![Event { name: "message".into(), data: "one".into() }]);
    }

    #[test]
    fn a_field_without_a_space_or_a_value_is_read() {
        let read = events(&[b"data:x\ndata\n\n"]);
        assert_eq!(read, vec![Event { name: "message".into(), data: "x\n".into() }]);
    }
}
