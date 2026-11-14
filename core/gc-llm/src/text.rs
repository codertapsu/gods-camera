//! Text from scans: one Unicode form, paragraphs, and long documents cut
//! into pieces a model can take.

use serde_json::Value;
use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};

/// The text in Unicode's composed form, NFC: a letter with a tone mark is one
/// character, as Vietnamese text is normally written. A model or a text
/// reader may give the decomposed form, a letter followed by its marks, which
/// looks the same and compares different.
pub fn nfc(text: &str) -> String {
    match is_nfc_quick(text.chars()) {
        IsNormalized::Yes => text.to_string(),
        _ => text.nfc().collect(),
    }
}

/// Every string in a JSON value, keys included, in NFC.
pub(crate) fn nfc_json(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(nfc(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(nfc_json).collect()),
        Value::Object(fields) => Value::Object(fields.into_iter().map(|(key, value)| (nfc(&key), nfc_json(value))).collect()),
        other => other,
    }
}

/// The paragraphs of a document: blocks of text between blank lines, each
/// trimmed, in NFC. Line ends of any system are read; the line breaks inside
/// a paragraph are kept.
pub fn paragraphs(text: &str) -> Vec<String> {
    let text = nfc(text).replace("\r\n", "\n").replace('\r', "\n");
    let mut found = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            if !current.is_empty() {
                found.push(current.join("\n").trim().to_string());
                current.clear();
            }
        } else {
            current.push(line.trim_end());
        }
    }
    if !current.is_empty() {
        found.push(current.join("\n").trim().to_string());
    }
    found
}

/// About how many tokens a model counts in a text: one for every two
/// characters. English runs nearer four characters a token; Vietnamese, with
/// its marks and short syllables, nearer two. The worse case is used, so a
/// piece is never larger than its budget.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(2)
}

/// A part of a document small enough for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// The piece's text: whole paragraphs separated by blank lines, or a run
    /// of sentences from one paragraph too long to go whole.
    pub text: String,
    /// The paragraphs it holds, counted from 0.
    pub first: usize,
    pub last: usize,
    /// The piece starts inside a paragraph that the piece before began. Its
    /// output joins the one before with a space, not a blank line.
    pub continues: bool,
}

/// Cuts a document's paragraphs into pieces of at most `budget` tokens, as
/// [`estimate_tokens`] counts them. Paragraphs stay whole and in order. A
/// paragraph larger than the budget is cut between sentences, and a sentence
/// larger than the budget between words.
pub fn cut(paragraphs: &[String], budget: usize) -> Vec<Piece> {
    let budget = budget.max(1);
    let mut pieces: Vec<Piece> = Vec::new();
    let mut current: Option<Piece> = None;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if estimate_tokens(paragraph) > budget {
            pieces.extend(current.take());
            for (part, text) in pack(&sentences(paragraph), budget, " ").into_iter().enumerate() {
                pieces.push(Piece { text, first: index, last: index, continues: part > 0 });
            }
            continue;
        }
        match current.as_mut() {
            Some(piece) if estimate_tokens(&piece.text) + 1 + estimate_tokens(paragraph) <= budget => {
                piece.text.push_str("\n\n");
                piece.text.push_str(paragraph);
                piece.last = index;
            }
            _ => {
                pieces.extend(current.take());
                current = Some(Piece { text: paragraph.clone(), first: index, last: index, continues: false });
            }
        }
    }
    pieces.extend(current);
    pieces
}

/// The outputs of the pieces joined as the pieces were: a blank line between
/// paragraphs, a space inside a paragraph that was cut.
pub fn join(pieces: &[Piece], outputs: &[String]) -> String {
    let mut joined = String::new();
    for (index, (piece, output)) in pieces.iter().zip(outputs).enumerate() {
        if index > 0 {
            joined.push_str(if piece.continues { " " } else { "\n\n" });
        }
        joined.push_str(output.trim());
    }
    joined
}

/// A paragraph's sentences, each with its closing mark and any quote or
/// bracket after it. A sentence ends at `.`, `!`, `?` or `…` followed by
/// white space.
fn sentences(paragraph: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut chars = paragraph.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if matches!(c, '.' | '!' | '?' | '…') {
            while let Some(&closing) = chars.peek() {
                if matches!(closing, '"' | '\'' | ')' | ']' | '”' | '’' | '»' | '.' | '!' | '?') {
                    current.push(closing);
                    chars.next();
                } else {
                    break;
                }
            }
            if chars.peek().is_some_and(|next| next.is_whitespace()) {
                found.push(current.trim().to_string());
                current.clear();
            }
        }
    }
    if !current.trim().is_empty() {
        found.push(current.trim().to_string());
    }
    found
}

/// Joins pieces of text with `separator` into runs of at most `budget`
/// tokens. A piece larger than the budget on its own is cut between words.
fn pack(parts: &[String], budget: usize, separator: &str) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut current = String::new();
    for part in parts {
        let pieces = if estimate_tokens(part) > budget { by_words(part, budget) } else { vec![part.clone()] };
        for piece in pieces {
            if !current.is_empty() && estimate_tokens(&current) + estimate_tokens(separator) + estimate_tokens(&piece) > budget {
                runs.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push_str(separator);
            }
            current.push_str(&piece);
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

fn by_words(text: &str, budget: usize) -> Vec<String> {
    let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    let mut runs = Vec::new();
    let mut current = String::new();
    for word in words {
        if !current.is_empty() && estimate_tokens(&current) + 1 + estimate_tokens(&word) > budget {
            runs.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&word);
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// The JSON in a model's answer: the whole answer, or the answer without a
/// Markdown code fence around it, or the part from the first brace to the
/// last. A local model asked for JSON does not always keep to it.
pub(crate) fn parse_json_loosely(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str(trimmed) {
        return Some(value);
    }
    let opened = trimmed.strip_prefix("```json").or_else(|| trimmed.strip_prefix("```"));
    let unfenced = opened.and_then(|rest| rest.trim_end().strip_suffix("```"));
    if let Some(inner) = unfenced {
        if let Ok(value) = serde_json::from_str(inner.trim()) {
            return Some(value);
        }
    }
    let (start, end) = (trimmed.find(['{', '['])?, trimmed.rfind(['}', ']'])?);
    if start < end {
        serde_json::from_str(&trimmed[start..=end]).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_composed() {
        // "Việt" with its marks written as separate characters.
        let decomposed = "Vie\u{0323}\u{0302}t Nam";
        assert_ne!(decomposed, "Việt Nam");
        assert_eq!(nfc(decomposed), "Việt Nam");
        assert_eq!(nfc("already composed"), "already composed");
        let json = nfc_json(serde_json::json!({ "te\u{0302}n": ["Vie\u{0323}\u{0302}t"], "n": 1 }));
        assert_eq!(json, serde_json::json!({ "tên": ["Việt"], "n": 1 }));
    }

    #[test]
    fn paragraphs_are_found_between_blank_lines() {
        let text = "Tiêu đề\r\n\r\nDòng một\r\ndòng hai  \n \n\n\nĐoạn ba.\n";
        assert_eq!(paragraphs(text), vec!["Tiêu đề", "Dòng một\ndòng hai", "Đoạn ba."]);
        assert!(paragraphs("  \n\n ").is_empty());
    }

    #[test]
    fn pieces_keep_paragraphs_whole_and_in_order() {
        let paragraphs: Vec<String> = (0..10).map(|n| format!("Đoạn số {n} có vài chữ.")).collect();
        let budget = 30;
        let pieces = cut(&paragraphs, budget);
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(|piece| estimate_tokens(&piece.text) <= budget), "{pieces:#?}");
        assert!(pieces.iter().all(|piece| !piece.continues));
        // Every paragraph once, in order.
        let mut next = 0;
        for piece in &pieces {
            assert_eq!(piece.first, next);
            next = piece.last + 1;
        }
        assert_eq!(next, paragraphs.len());
        let outputs: Vec<String> = pieces.iter().map(|piece| piece.text.clone()).collect();
        assert_eq!(join(&pieces, &outputs), paragraphs.join("\n\n"));
    }

    #[test]
    fn a_long_paragraph_is_cut_between_sentences() {
        let long = String::from("Câu một rất dài ở đây. Câu hai “có ngoặc.” Câu ba hỏi gì? Câu bốn… Câu năm kết thúc.");
        let paragraphs = vec!["Mở đầu.".to_string(), long.clone(), "Kết.".to_string()];
        let pieces = cut(&paragraphs, 20);
        assert!(pieces.iter().all(|piece| estimate_tokens(&piece.text) <= 20), "{pieces:#?}");
        let middle: Vec<&Piece> = pieces.iter().filter(|piece| piece.first == 1).collect();
        assert!(middle.len() > 1);
        assert!(!middle[0].continues && middle[1..].iter().all(|piece| piece.continues));
        // Cut at sentence ends, so every piece of it ends a sentence.
        assert!(middle.iter().all(|piece| piece.text.ends_with(['.', '?', '…', '”'])), "{middle:#?}");
        let outputs: Vec<String> = pieces.iter().map(|piece| piece.text.clone()).collect();
        assert_eq!(join(&pieces, &outputs), paragraphs.join("\n\n"));
    }

    #[test]
    fn a_sentence_longer_than_the_budget_is_cut_between_words() {
        let words: Vec<String> = (0..60).map(|n| format!("chữ{n}")).collect();
        let pieces = cut(&[words.join(" ")], 25);
        assert!(pieces.len() > 2);
        assert!(pieces.iter().all(|piece| estimate_tokens(&piece.text) <= 25));
        let outputs: Vec<String> = pieces.iter().map(|piece| piece.text.clone()).collect();
        assert_eq!(join(&pieces, &outputs), words.join(" "));
    }

    #[test]
    fn json_is_found_in_a_loose_answer() {
        assert_eq!(parse_json_loosely(r#" {"a": 1} "#), Some(serde_json::json!({"a": 1})));
        assert_eq!(parse_json_loosely("```json\n{\"a\": [1, 2]}\n```"), Some(serde_json::json!({"a": [1, 2]})));
        assert_eq!(parse_json_loosely("Here it is: {\"b\": true} Hope that helps."), Some(serde_json::json!({"b": true})));
        assert_eq!(parse_json_loosely("no json here"), None);
    }
}
