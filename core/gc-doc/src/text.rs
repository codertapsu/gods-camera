//! Comparing pieces of text.

/// Text reduced to what matters for comparing two readings of the same line:
/// lower case, single spaces, no punctuation at the ends.
pub fn normalise(text: &str) -> String {
    let lowered = text.to_lowercase();
    let collapsed: Vec<&str> = lowered.split_whitespace().collect();
    collapsed.join(" ").trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

/// Edit distance between two texts, counted in characters. Stops early and
/// returns `limit + 1` once the distance must exceed `limit`.
pub fn edit_distance(a: &str, b: &str, limit: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > limit {
        return limit + 1;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        let mut best = current[0];
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitute.min(previous[j + 1] + 1).min(current[j] + 1);
            best = best.min(current[j + 1]);
        }
        if best > limit {
            return limit + 1;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// How alike two texts are, 0 to 1, by edit distance over the longer length.
pub fn similarity(a: &str, b: &str) -> f32 {
    let longer = a.chars().count().max(b.chars().count());
    if longer == 0 {
        return 1.0;
    }
    1.0 - edit_distance(a, b, longer) as f32 / longer as f32
}

/// Whether one text is a cut-off reading of the other: the shorter one, with
/// a little slack, appears at the start or the end of the longer one. This is
/// how a line looks when a photo's edge runs through it.
pub fn is_fragment_of(short: &str, long: &str) -> bool {
    let s: Vec<char> = short.chars().collect();
    let l: Vec<char> = long.chars().collect();
    if s.len() < 3 || s.len() > l.len() {
        return false;
    }
    let slack = (s.len() / 5).max(1);
    let head: String = l[..s.len()].iter().collect();
    let tail: String = l[l.len() - s.len()..].iter().collect();
    edit_distance(short, &head, slack) <= slack || edit_distance(short, &tail, slack) <= slack
}

/// Character error rate of `got` against `truth`: edits needed, over the
/// truth's length. White space is collapsed first, since line breaks differ.
pub fn character_error_rate(got: &str, truth: &str) -> f32 {
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (got, truth) = (squash(got), squash(truth));
    let length = truth.chars().count().max(1);
    edit_distance(&got, &truth, length + got.chars().count()) as f32 / length as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalising_keeps_tone_marks() {
        assert_eq!(normalise("  Mùa  Nước nổi. "), "mùa nước nổi");
        assert_ne!(normalise("được"), normalise("duoc"));
    }

    #[test]
    fn edit_distance_counts_characters_not_bytes() {
        assert_eq!(edit_distance("được", "dược", 5), 1);
        assert_eq!(edit_distance("abc", "abc", 5), 0);
        assert_eq!(edit_distance("kitten", "sitting", 10), 3);
        // Past the limit the exact number is not worked out.
        assert_eq!(edit_distance("aaaaaaaa", "bbbbbbbb", 2), 3);
    }

    #[test]
    fn fragments_are_recognised_at_either_end() {
        assert!(is_fragment_of("nước về sớm", "năm nay nước về sớm"));
        assert!(is_fragment_of("năm nay nuớc", "năm nay nước về sớm"));
        assert!(!is_fragment_of("một dòng khác hẳn", "năm nay nước về sớm hơn mọi năm"));
        assert!(!is_fragment_of("ab", "abcdef"));
    }

    #[test]
    fn error_rate_ignores_line_breaks() {
        assert_eq!(character_error_rate("a b\nc", "a b c"), 0.0);
        assert!((character_error_rate("abcd", "abcf") - 0.25).abs() < 1e-6);
    }
}
