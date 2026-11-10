//! Which removed line each added line replaces, and the words that changed between them.
//!
//! Within a hunk, a run of removed lines followed directly by a run of added lines is paired
//! line by line, in order: what side-by-side shows on one row, and what word-level
//! highlighting compares.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use similar::{Algorithm, DiffTag, capture_diff_slices_deadline};

use super::{Hunk, LineKind};

/// Lines sharing less than this share of their characters are rewritten rather than edited,
/// so marking the words that changed would mark nearly all of them.
const MIN_UNCHANGED_SHARE: f64 = 0.4;
/// The longest a pair of lines is compared for.
const DEADLINE: Duration = Duration::from_millis(20);

/// The rows of `hunk` side by side: each a line on the old side, the new side, or both, by
/// index into the hunk's lines.
pub fn side_by_side(hunk: &Hunk) -> Vec<(Option<usize>, Option<usize>)> {
    let mut rows = Vec::with_capacity(hunk.lines.len());
    let mut index = 0;
    while index < hunk.lines.len() {
        if hunk.lines[index].kind == LineKind::Context {
            rows.push((Some(index), Some(index)));
            index += 1;
            continue;
        }
        let (removed, added) = block(hunk, index);
        let count = removed.len().max(added.len());
        let at = |run: &Range<usize>, k: usize| (k < run.len()).then_some(run.start + k);
        rows.extend((0..count).map(|k| (at(&removed, k), at(&added, k))));
        index = removed.end.max(added.end);
    }
    rows
}

/// Each line's partner on the other side, by index into the hunk's lines: a removed line's
/// replacement and an added line's original, or `None`.
pub fn partners(hunk: &Hunk) -> Vec<Option<usize>> {
    let mut partners = vec![None; hunk.lines.len()];
    for (old, new) in side_by_side(hunk) {
        if let (Some(old), Some(new)) = (old, new)
            && old != new
        {
            partners[old] = Some(new);
            partners[new] = Some(old);
        }
    }
    partners
}

/// The run of removed lines starting at `start`, if any, and the run of added lines after it.
fn block(hunk: &Hunk, start: usize) -> (Range<usize>, Range<usize>) {
    let run = |from: usize, kind: LineKind| {
        let end = hunk.lines[from..]
            .iter()
            .position(|line| line.kind != kind)
            .map_or(hunk.lines.len(), |length| from + length);
        from..end
    };
    let removed = if hunk.lines[start].kind == LineKind::Removed {
        run(start, LineKind::Removed)
    } else {
        start..start
    };
    let added = if hunk.lines.get(removed.end).map(|line| line.kind) == Some(LineKind::Added) {
        run(removed.end, LineKind::Added)
    } else {
        removed.end..removed.end
    };
    (removed, added)
}

/// The characters of a pair of lines, by index rather than byte, that differ between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedWords {
    pub old: Vec<Range<usize>>,
    pub new: Vec<Range<usize>>,
}

/// What differs between `old` and `new`, word by word, or `None` when they differ too much
/// for that to help.
pub fn changed_words(old: &str, new: &str) -> Option<ChangedWords> {
    let old_words = words(old);
    let new_words = words(new);
    let old_text = old_words.iter().map(|word| word.1).collect::<Vec<_>>();
    let new_text = new_words.iter().map(|word| word.1).collect::<Vec<_>>();
    let ops = capture_diff_slices_deadline(
        Algorithm::Myers,
        &old_text,
        &new_text,
        Some(Instant::now() + DEADLINE),
    );
    let (mut old_changed, mut new_changed) = (Vec::new(), Vec::new());
    let mut unchanged = 0;
    for op in ops {
        if op.tag() == DiffTag::Equal {
            unchanged += old_words[op.old_range()]
                .iter()
                .map(|word| word.1.chars().count())
                .sum::<usize>();
            continue;
        }
        push_span(&mut old_changed, &old_words, op.old_range());
        push_span(&mut new_changed, &new_words, op.new_range());
    }
    let longer = old.chars().count().max(new.chars().count());
    #[expect(
        clippy::cast_precision_loss,
        reason = "line lengths are far below 2^52"
    )]
    let share = unchanged as f64 / longer.max(1) as f64;
    (share >= MIN_UNCHANGED_SHARE && !(old_changed.is_empty() && new_changed.is_empty())).then_some(
        ChangedWords {
            old: old_changed,
            new: new_changed,
        },
    )
}

/// Adds the characters of `words[range]` to `spans`, joined to the last span if they touch.
fn push_span(spans: &mut Vec<Range<usize>>, words: &[(usize, &str)], range: Range<usize>) {
    let Some(first) = words.get(range.start) else {
        return;
    };
    let last = &words[range.end - 1];
    let span = first.0..last.0 + last.1.chars().count();
    match spans.last_mut() {
        Some(previous) if previous.end == span.start => previous.end = span.end,
        _ => spans.push(span),
    }
}

/// `text` split into words, runs of whitespace and single other characters, each with the
/// index of its first character.
fn words(text: &str) -> Vec<(usize, &str)> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |character: char| {
        if character.is_alphanumeric() || character == '_' {
            Class::Word
        } else if character.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut words = Vec::new();
    let mut start: Option<(usize, usize, Class)> = None;
    for (index, (byte, character)) in text.char_indices().enumerate() {
        let this = class(character);
        if let Some((_, _, previous)) = &start
            && *previous == this
            && this != Class::Other
        {
            continue;
        }
        if let Some((start_index, start_byte, _)) = start.take() {
            words.push((start_index, &text[start_byte..byte]));
        }
        start = Some((index, byte, this));
    }
    if let Some((start_index, start_byte, _)) = start {
        words.push((start_index, &text[start_byte..]));
    }
    words
}

#[cfg(test)]
#[expect(
    clippy::single_range_in_vec_init,
    reason = "the expected spans are ranges, and some are one span"
)]
mod tests {
    use super::*;
    use crate::diff::Line;

    fn hunk(kinds: &str) -> Hunk {
        Hunk {
            old_start: 1,
            old_len: 0,
            new_start: 1,
            new_len: 0,
            section: String::new(),
            lines: kinds
                .chars()
                .map(|kind| Line {
                    kind: match kind {
                        '-' => LineKind::Removed,
                        '+' => LineKind::Added,
                        _ => LineKind::Context,
                    },
                    old: None,
                    new: None,
                    text: String::new(),
                    missing_newline: false,
                })
                .collect(),
        }
    }

    #[test]
    fn removed_lines_pair_with_the_added_lines_after_them() {
        // context, two removed replaced by three added, context, one added alone.
        let hunk = hunk(" --+++ +");
        assert_eq!(
            side_by_side(&hunk),
            [
                (Some(0), Some(0)),
                (Some(1), Some(3)),
                (Some(2), Some(4)),
                (None, Some(5)),
                (Some(6), Some(6)),
                (None, Some(7)),
            ]
        );
        assert_eq!(
            partners(&hunk),
            [None, Some(3), Some(4), Some(1), Some(2), None, None, None]
        );
    }

    #[test]
    fn removed_lines_with_nothing_after_them_stand_alone() {
        assert_eq!(
            side_by_side(&hunk("--")),
            [(Some(0), None), (Some(1), None)]
        );
        assert_eq!(partners(&hunk("-+-")), [Some(1), Some(0), None]);
    }

    #[test]
    fn an_edited_line_marks_only_the_words_that_changed() {
        let ChangedWords { old, new } =
            changed_words("let total = count + 1;", "let total = count + step;").unwrap();
        assert_eq!(old, [20..21]);
        assert_eq!(new, [20..24]);
    }

    #[test]
    fn character_positions_count_characters_not_bytes() {
        let ChangedWords { old, new } = changed_words("née = 1", "née = 2").unwrap();
        assert_eq!((old, new), (vec![6..7], vec![6..7]));
    }

    #[test]
    fn a_rewritten_line_or_an_identical_one_marks_nothing() {
        assert_eq!(changed_words("alpha beta", "gamma delta epsilon"), None);
        assert_eq!(changed_words("same", "same"), None);
    }

    #[test]
    fn adjacent_changed_words_join_into_one_span() {
        let ChangedWords { old, new } = changed_words("call(a, b)", "call(x, y)").unwrap();
        assert_eq!(old, [5..6, 8..9]);
        assert_eq!(new, [5..6, 8..9]);
        let ChangedWords { new, .. } = changed_words("f(a)", "f(a.b.c)").unwrap();
        assert_eq!(new, [3..7]);
    }
}
