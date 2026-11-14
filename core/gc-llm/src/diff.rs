//! Which words a clean-up changed, worked out on this computer and not taken
//! from the model's word for it.
//!
//! The two texts are compared word by word, with paragraph breaks as marks
//! of their own, by Myers' method: the fewest words removed and added that
//! turn one into the other. A word with a corrected tone mark is one word
//! removed and one added.

/// Whether a stretch of text is in both versions, or in one only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Same,
    /// In the original only.
    Removed,
    /// In the new version only.
    Added,
}

/// A stretch of words of one kind, with the space or paragraph break that
/// comes before it. Put the `Same` and `Added` stretches one after the other
/// and the new text comes out; the `Same` and `Removed` give the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub text: String,
}

/// The changes between two versions of a text, and how many words changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WordDiff {
    pub changes: Vec<Change>,
    pub words_before: usize,
    pub words_after: usize,
    pub words_removed: usize,
    pub words_added: usize,
    /// Paragraph breaks removed or added: paragraphs were joined or split.
    pub breaks_changed: usize,
}

impl WordDiff {
    /// The share of words changed, 0 to 1: words removed and added over the
    /// words of both versions. One word corrected in a hundred is 0.01.
    pub fn share_changed(&self) -> f32 {
        let all = self.words_before + self.words_after;
        if all == 0 {
            0.0
        } else {
            (self.words_removed + self.words_added) as f32 / all as f32
        }
    }

    /// Adds the diff of the next part of the same text, after `separator`
    /// (`"\n\n"` between paragraphs, `" "` inside one).
    pub fn append(&mut self, next: WordDiff, separator: &str) {
        let mut changes = next.changes.into_iter();
        if let Some(mut first) = changes.next() {
            if !self.changes.is_empty() {
                first.text.insert_str(0, separator);
            }
            self.push(first.kind, &first.text);
            for change in changes {
                self.push(change.kind, &change.text);
            }
        }
        self.words_before += next.words_before;
        self.words_after += next.words_after;
        self.words_removed += next.words_removed;
        self.words_added += next.words_added;
        self.breaks_changed += next.breaks_changed;
    }

    fn push(&mut self, kind: ChangeKind, text: &str) {
        match self.changes.last_mut() {
            Some(last) if last.kind == kind => last.text.push_str(text),
            _ => self.changes.push(Change { kind, text: text.to_string() }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Word(&'a str),
    Break,
}

/// The words of a text, with a mark for each paragraph break (a blank line).
fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut found = Vec::new();
    for paragraph in text.split("\n\n").map(str::trim).filter(|paragraph| !paragraph.is_empty()) {
        if !found.is_empty() {
            found.push(Token::Break);
        }
        found.extend(paragraph.split_whitespace().map(Token::Word));
    }
    found
}

/// How a token is written after the token before it in the same version.
fn written(token: Token<'_>, previous: Option<Token<'_>>) -> String {
    match (token, previous) {
        (Token::Break, _) => "\n\n".to_string(),
        (Token::Word(word), None | Some(Token::Break)) => word.to_string(),
        (Token::Word(word), Some(Token::Word(_))) => format!(" {word}"),
    }
}

/// Above this many edits the two texts are taken as rewritten from the first
/// difference to the last, instead of searching on: the search would cost
/// time and memory, and a text changed that much is flagged anyway.
const MOST_EDITS: usize = 2000;

/// The word-by-word changes from `before` to `after`. Both should be in NFC,
/// or a letter written two ways counts as changed.
pub fn diff_words(before: &str, after: &str) -> WordDiff {
    let old = tokens(before);
    let new = tokens(after);
    let mut diff = WordDiff {
        words_before: old.iter().filter(|token| matches!(token, Token::Word(_))).count(),
        words_after: new.iter().filter(|token| matches!(token, Token::Word(_))).count(),
        ..WordDiff::default()
    };
    // What both share at the start and at the end needs no search.
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let end = old[start..].iter().rev().zip(new[start..].iter().rev()).take_while(|(a, b)| a == b).count();
    let mut steps = vec![Step::Same; start];
    let middle = myers(&old[start..old.len() - end], &new[start..new.len() - end]);
    steps.extend(middle);
    steps.extend(std::iter::repeat_n(Step::Same, end));

    let (mut i, mut j) = (0, 0);
    for step in steps {
        match step {
            Step::Same => {
                diff.push(ChangeKind::Same, &written(new[j], j.checked_sub(1).map(|k| new[k])));
                i += 1;
                j += 1;
            }
            Step::Removed => {
                match old[i] {
                    Token::Word(_) => diff.words_removed += 1,
                    Token::Break => diff.breaks_changed += 1,
                }
                diff.push(ChangeKind::Removed, &written(old[i], i.checked_sub(1).map(|k| old[k])));
                i += 1;
            }
            Step::Added => {
                match new[j] {
                    Token::Word(_) => diff.words_added += 1,
                    Token::Break => diff.breaks_changed += 1,
                }
                diff.push(ChangeKind::Added, &written(new[j], j.checked_sub(1).map(|k| new[k])));
                j += 1;
            }
        }
    }
    diff
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Same,
    Removed,
    Added,
}

/// Myers' shortest edit script. For each number of edits `d` it keeps only
/// the furthest point reached on each diagonal `k` from `-d` to `d`, so the
/// memory grows with the square of the edits, not with the texts' lengths.
fn myers(a: &[Token<'_>], b: &[Token<'_>]) -> Vec<Step> {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return std::iter::repeat_n(Step::Removed, n).chain(std::iter::repeat_n(Step::Added, m)).collect();
    }
    let most = (n + m).min(MOST_EDITS);
    let offset = most as isize + 1;
    let mut v = vec![0usize; 2 * most + 3];
    // trace[d] holds v for the diagonals -d..=d after d edits.
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let mut reached = None;
    'search: for d in 0..=most as isize {
        for k in (-d..=d).step_by(2) {
            let index = (k + offset) as usize;
            let mut x = if k == -d || (k != d && v[index - 1] < v[index + 1]) { v[index + 1] } else { v[index - 1] + 1 };
            let mut y = (x as isize - k) as usize;
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[index] = x;
            if x >= n && y >= m {
                trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
                reached = Some(d);
                break 'search;
            }
        }
        trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
    }
    let rewritten = || std::iter::repeat_n(Step::Removed, n).chain(std::iter::repeat_n(Step::Added, m)).collect();
    // Too many edits: all of one out, all of the other in.
    let Some(edits) = reached else { return rewritten() };
    // The steps must use up both texts exactly; anything else would be a
    // fault in the search, and the plain answer is still right.
    let uses_up = |steps: &[Step]| {
        steps.iter().filter(|step| **step != Step::Added).count() == n && steps.iter().filter(|step| **step != Step::Removed).count() == m
    };
    match walk_back(&trace, edits, n, m) {
        Some(steps) if uses_up(&steps) => steps,
        _ => rewritten(),
    }
}

/// The steps from the start to `(n, m)`, found by walking back from the end
/// one edit at a time through what the search kept.
fn walk_back(trace: &[Vec<usize>], edits: isize, n: usize, m: usize) -> Option<Vec<Step>> {
    let mut steps = Vec::with_capacity(n + m);
    let (mut x, mut y) = (n as isize, m as isize);
    for d in (1..=edits).rev() {
        let before = trace.get((d - 1) as usize)?;
        let at = |k: isize| -> Option<isize> { usize::try_from(k + d - 1).ok().and_then(|index| before.get(index)).map(|x| *x as isize) };
        let k = x - y;
        let down = k == -d || (k != d && at(k - 1)? < at(k + 1)?);
        let previous_k = if down { k + 1 } else { k - 1 };
        let previous_x = at(previous_k)?;
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            steps.push(Step::Same);
            x -= 1;
            y -= 1;
        }
        steps.push(if x == previous_x { Step::Added } else { Step::Removed });
        x = previous_x;
        y = previous_y;
    }
    while x > 0 && y > 0 {
        steps.push(Step::Same);
        x -= 1;
        y -= 1;
    }
    if x != 0 || y != 0 {
        return None;
    }
    steps.reverse();
    Some(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rebuild(diff: &WordDiff, keep: ChangeKind) -> String {
        let kept = diff.changes.iter().filter(|change| change.kind == ChangeKind::Same || change.kind == keep);
        kept.map(|change| change.text.as_str()).collect()
    }

    #[test]
    fn a_corrected_tone_mark_is_one_word_out_and_one_in() {
        let diff = diff_words("Giá hôm nay: 25.000 dong một ký.", "Giá hôm nay: 25.000 đồng một ký.");
        assert_eq!((diff.words_removed, diff.words_added, diff.breaks_changed), (1, 1, 0));
        assert_eq!(
            diff.changes,
            vec![
                Change { kind: ChangeKind::Same, text: "Giá hôm nay: 25.000".into() },
                Change { kind: ChangeKind::Removed, text: " dong".into() },
                Change { kind: ChangeKind::Added, text: " đồng".into() },
                Change { kind: ChangeKind::Same, text: " một ký.".into() },
            ]
        );
        // Seven words in each version, one changed in each.
        assert!((diff.share_changed() - 2.0 / 14.0).abs() < 1e-6);
    }

    #[test]
    fn both_versions_can_be_put_back_together() {
        let before = "Trời vừa hửng sáng, ghe xuồng đã tấp nập.\n\n\
                      Người bán treo hàng lên cây bẹo khóm bưởi.\n\n\
                      The market opens at 5 a.m.";
        let after = "Trời vừa hửng sáng, ghe xuồng đã tấp nập trên sông.\n\n\
                     Người bán treo hàng lên cây bẹo: khóm, bưởi.\n\n\
                     The market opens at 5 a.m.";
        let diff = diff_words(before, after);
        assert_eq!(rebuild(&diff, ChangeKind::Added), after);
        assert_eq!(rebuild(&diff, ChangeKind::Removed), before);
        assert_eq!(diff.breaks_changed, 0);
    }

    #[test]
    fn joined_paragraphs_are_counted() {
        let diff = diff_words("Một.\n\nHai.", "Một. Hai.");
        assert_eq!(diff.breaks_changed, 1);
        assert_eq!((diff.words_removed, diff.words_added), (0, 0));
        assert_eq!(rebuild(&diff, ChangeKind::Added), "Một. Hai.");
    }

    #[test]
    fn empty_and_equal_texts() {
        assert_eq!(diff_words("", "").changes, vec![]);
        let same = diff_words("a b c", "a b c");
        assert_eq!(same.changes, vec![Change { kind: ChangeKind::Same, text: "a b c".into() }]);
        assert_eq!(same.share_changed(), 0.0);
        let all_new = diff_words("", "x y");
        assert_eq!((all_new.words_added, all_new.words_removed), (2, 0));
        assert_eq!(rebuild(&all_new, ChangeKind::Added), "x y");
    }

    #[test]
    fn a_rewritten_text_is_found_out() {
        let before: Vec<String> = (0..400).map(|n| format!("w{n}")).collect();
        let after: Vec<String> = (0..400).map(|n| if n % 2 == 0 { format!("w{n}") } else { format!("v{n}") }).collect();
        let diff = diff_words(&before.join(" "), &after.join(" "));
        assert_eq!((diff.words_removed, diff.words_added), (200, 200));
        assert!((diff.share_changed() - 0.5).abs() < 1e-6);
        assert_eq!(rebuild(&diff, ChangeKind::Added), after.join(" "));
        // Past the search limit the middle counts as rewritten, and the texts still come back whole.
        let many: Vec<String> = (0..3000).map(|n| format!("x{n}")).collect();
        let others: Vec<String> = (0..3000).map(|n| format!("y{n}")).collect();
        let diff = diff_words(&format!("start {} end", many.join(" ")), &format!("start {} end", others.join(" ")));
        assert_eq!((diff.words_removed, diff.words_added), (3000, 3000));
        assert_eq!(rebuild(&diff, ChangeKind::Removed), format!("start {} end", many.join(" ")));
    }

    #[test]
    fn parts_are_appended_in_order() {
        let mut whole = diff_words("Một hai", "Một hai");
        whole.append(diff_words("ba bôn", "ba bốn"), " ");
        whole.append(diff_words("Năm.", "Năm."), "\n\n");
        assert_eq!(rebuild(&whole, ChangeKind::Added), "Một hai ba bốn\n\nNăm.");
        assert_eq!(rebuild(&whole, ChangeKind::Removed), "Một hai ba bôn\n\nNăm.");
        assert_eq!((whole.words_removed, whole.words_added, whole.words_before), (1, 1, 5));
    }
}
