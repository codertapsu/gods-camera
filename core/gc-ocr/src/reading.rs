//! Turning the recogniser's answer into text, and fitting long lines to it.
//!
//! The recogniser looks at a line 32 pixels high and gives, for every four
//! pixels of its width, how likely each character is there, or that nothing
//! is there (a "blank"). The text is the likeliest character of each step,
//! with repeats and blanks dropped: the usual way to read a model trained
//! with connectionist temporal classification.

use crate::picture::Picture;

/// The characters the recogniser knows, in its order. The first is the blank.
#[derive(Debug, Clone, PartialEq)]
pub struct Alphabet {
    characters: Vec<String>,
}

impl Alphabet {
    /// From the model's own list: `{"chars": ["<blank>", "a", ...]}`.
    pub fn from_json(text: &str) -> Result<Self, String> {
        #[derive(serde::Deserialize)]
        struct File {
            chars: Vec<String>,
        }
        let file: File = serde_json::from_str(text).map_err(|error| format!("the character list cannot be read: {error}"))?;
        if file.chars.len() < 2 {
            return Err("the character list is empty".into());
        }
        Ok(Self { characters: file.chars })
    }

    pub fn len(&self) -> usize {
        self.characters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.characters.is_empty()
    }
}

/// The text in the recogniser's answer for one line, and how sure it is of
/// each character, 0 to 1. `scores` holds one row per step, each with the
/// natural logarithm of every character's likelihood.
pub fn decode(scores: &[f32], alphabet: &Alphabet) -> (String, Vec<f32>) {
    let (mut text, mut sure) = (String::new(), Vec::new());
    let mut last = 0;
    for step in scores.chunks_exact(alphabet.len()) {
        let (best, score) = step.iter().enumerate().fold((0, f32::MIN), |best, (index, score)| if *score > best.1 { (index, *score) } else { best });
        if best != last && best != 0 {
            text.push_str(&alphabet.characters[best]);
            sure.push(score.exp().clamp(0.0, 1.0));
        }
        last = best;
    }
    (text, sure)
}

/// Where to cut a line that is wider than the recogniser takes, so that every
/// piece fits and no word is cut: the widest gap without ink near each place
/// an even split would fall. Returns the pieces' edges, from 0 to the width.
pub fn cuts(line: &Picture, widest: usize) -> Vec<usize> {
    if line.width <= widest {
        return vec![0, line.width];
    }
    // Few enough pixels to a piece that a cut moved to a gap, by up to a fifth
    // of a piece at either end, still leaves every piece within the limit.
    let pieces = (line.width * 7).div_ceil(widest * 5);
    let empty = line.empty_columns();
    let mut edges = vec![0];
    for piece in 1..pieces {
        let ideal = line.width * piece / pieces;
        let reach = line.width / pieces / 5;
        let from = (edges[edges.len() - 1] + 1).max(ideal.saturating_sub(reach));
        let to = (ideal + reach).min(line.width);
        let (mut best, mut widest_gap) = (ideal, 0);
        let mut column = from;
        while column < to {
            if empty[column] {
                let end = (column..line.width).find(|c| !empty[*c]).unwrap_or(line.width);
                if end - column > widest_gap {
                    (best, widest_gap) = ((column + end) / 2, end - column);
                }
                column = end;
            } else {
                column += 1;
            }
        }
        edges.push(best);
    }
    edges.push(line.width);
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alphabet() -> Alphabet {
        Alphabet::from_json(r#"{"chars": ["<blank>", "a", "b", "ế", " "], "size": 5}"#).unwrap()
    }

    /// One row per step: the named character is likely, the others are not.
    fn steps(best: &[usize], likelihood: f32) -> Vec<f32> {
        let mut scores = Vec::new();
        for index in best {
            let mut row = vec![((1.0 - likelihood) / 4.0).ln(); 5];
            row[*index] = likelihood.ln();
            scores.extend(row);
        }
        scores
    }

    #[test]
    fn repeats_and_blanks_are_dropped() {
        // a a _ a b b _ ế ế _ (space) a
        let (text, sure) = decode(&steps(&[1, 1, 0, 1, 2, 2, 0, 3, 3, 0, 4, 1], 0.9), &alphabet());
        assert_eq!(text, "aabế a");
        assert_eq!(sure.len(), 6);
        assert!(sure.iter().all(|s| (s - 0.9).abs() < 1e-4));
    }

    #[test]
    fn nothing_but_blanks_is_no_text() {
        assert_eq!(decode(&steps(&[0, 0, 0], 0.99), &alphabet()), (String::new(), Vec::new()));
    }

    #[test]
    fn a_character_list_must_have_characters() {
        assert!(Alphabet::from_json(r#"{"chars": ["<blank>"]}"#).is_err());
        assert!(Alphabet::from_json("[]").is_err());
        assert_eq!(alphabet().len(), 5);
    }

    /// White paper with a dark word between each pair of the given columns.
    fn words(width: usize, ink: &[(usize, usize)]) -> Picture {
        let mut rgb = vec![250u8; width * 8 * 3];
        for (from, to) in ink {
            for y in 2..6 {
                for x in *from..*to {
                    rgb[(y * width + x) * 3..][..3].copy_from_slice(&[20, 20, 20]);
                }
            }
        }
        Picture { width, height: 8, rgb }
    }

    #[test]
    fn a_line_that_fits_is_not_cut() {
        assert_eq!(cuts(&words(300, &[(10, 290)]), 1024), vec![0, 300]);
    }

    #[test]
    fn a_long_line_is_cut_in_the_gap_nearest_the_middle() {
        // Words up to 480 and from 520: the gap is around 500, and an even split would fall at 600.
        let line = words(1200, &[(10, 200), (230, 480), (520, 800), (830, 1190)]);
        let edges = cuts(&line, 1024);
        assert_eq!(edges.len(), 3);
        assert_eq!((edges[0], edges[2]), (0, 1200));
        assert!((481..520).contains(&edges[1]), "{edges:?}");
        assert!(edges.windows(2).all(|pair| pair[1] - pair[0] <= 1024));
    }

    #[test]
    fn a_line_without_gaps_is_cut_evenly() {
        let edges = cuts(&words(2500, &[(0, 2500)]), 1024);
        assert_eq!(edges, vec![0, 625, 1250, 1875, 2500]);
    }

    #[test]
    fn no_piece_is_wider_than_the_limit_wherever_the_gaps_are() {
        // Gaps only at the far ends of where a cut may move to.
        for width in [1025, 1400, 2047, 2048, 3000, 5000] {
            let pieces = (width * 7usize).div_ceil(1024 * 5);
            let piece = width / pieces;
            let mut ink = Vec::new();
            let mut from = 0;
            for number in 1..pieces {
                // Alternate: the gap just inside the near end, then just inside the far end.
                let gap = if number % 2 == 1 { width * number / pieces - piece / 5 + 1 } else { width * number / pieces + piece / 5 - 3 };
                ink.push((from, gap));
                from = gap + 2;
            }
            ink.push((from, width));
            let edges = cuts(&words(width, &ink), 1024);
            assert!(edges.windows(2).all(|pair| pair[1] > pair[0] && pair[1] - pair[0] <= 1024), "{width}: {edges:?}");
        }
    }
}
