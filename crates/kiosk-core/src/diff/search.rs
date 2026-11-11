//! Finding text in the lines of the diff tab's range.

use std::ops::Range;

use super::FileContent;

/// Where the text was found: a line, by its file, hunk and place in the hunk, and the
/// characters of it that match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub file: usize,
    pub hunk: usize,
    pub line: usize,
    pub chars: Range<usize>,
}

impl Found {
    /// The line, for ordering and finding a line's matches.
    pub const fn line_key(&self) -> (usize, usize, usize) {
        (self.file, self.hunk, self.line)
    }
}

/// Every place `query` is in the lines of the files with `contents`, in order, the matches in
/// a line not overlapping. Case is ignored unless the query has a capital letter.
pub fn find<'a>(contents: impl IntoIterator<Item = &'a FileContent>, query: &str) -> Vec<Found> {
    if query.is_empty() {
        return Vec::new();
    }
    let exact = query.chars().any(char::is_uppercase);
    let fold = |character: char| if exact { character } else { lower(character) };
    let needle: Vec<char> = query.chars().map(fold).collect();
    let mut found = Vec::new();
    for (file_index, content) in contents.into_iter().enumerate() {
        let FileContent::Text { hunks } = content else {
            continue;
        };
        for (hunk_index, hunk) in hunks.iter().enumerate() {
            for (line_index, line) in hunk.lines.iter().enumerate() {
                let haystack: Vec<char> = line.text.chars().map(fold).collect();
                let mut start = 0;
                while start + needle.len() <= haystack.len() {
                    if haystack[start..start + needle.len()] == needle[..] {
                        found.push(Found {
                            file: file_index,
                            hunk: hunk_index,
                            line: line_index,
                            chars: start..start + needle.len(),
                        });
                        start += needle.len();
                    } else {
                        start += 1;
                    }
                }
            }
        }
    }
    found
}

/// `character` in lower case where that is one character, and otherwise as it is, so a
/// match's place is still counted in the line's own characters.
fn lower(character: char) -> char {
    let mut lower = character.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) => one,
        _ => character,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{Hunk, Line, LineKind};

    fn text(lines: &[&str]) -> FileContent {
        FileContent::Text {
            hunks: vec![Hunk {
                old_start: 1,
                old_len: 0,
                new_start: 1,
                new_len: u32::try_from(lines.len()).unwrap(),
                section: String::new(),
                lines: lines
                    .iter()
                    .enumerate()
                    .map(|(index, text)| Line {
                        kind: LineKind::Added,
                        old: None,
                        new: Some(u32::try_from(index + 1).unwrap()),
                        text: (*text).into(),
                        missing_newline: false,
                    })
                    .collect(),
            }],
        }
    }

    fn places(found: &[Found]) -> Vec<(usize, usize, usize, std::ops::Range<usize>)> {
        found
            .iter()
            .map(|found| (found.file, found.hunk, found.line, found.chars.clone()))
            .collect()
    }

    #[test]
    fn finds_every_match_in_order_ignoring_case_unless_asked() {
        let files = [
            text(&["let Answer = 42;", "no"]),
            FileContent::Binary,
            text(&["answer answeranswer"]),
        ];
        assert_eq!(
            places(&find(&files, "answer")),
            [
                (0, 0, 0, 4..10),
                (2, 0, 0, 0..6),
                (2, 0, 0, 7..13),
                (2, 0, 0, 13..19)
            ]
        );
        assert_eq!(places(&find(&files, "Answer")), [(0, 0, 0, 4..10)]);
        assert_eq!(find(&files, ""), []);
        assert_eq!(
            places(&find(&[text(&["aaaa"])], "aa")),
            [(0, 0, 0, 0..2), (0, 0, 0, 2..4)]
        );
    }

    #[test]
    fn a_match_is_placed_by_the_lines_own_characters() {
        // `İ` lowers to two characters; it is compared as it is, keeping the places after it.
        assert_eq!(
            places(&find(&[text(&["İé x é"])], "é")),
            [(0, 0, 0, 1..2), (0, 0, 0, 5..6)]
        );
    }
}
