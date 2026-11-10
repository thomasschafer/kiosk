//! What the diff tab shows: the target and its ranges, the files of the range showing, and
//! the rows on screen.
//!
//! Rows are compact arrays, as a screen of them is sent on every scroll: `["file", n]`,
//! `["note", n, text]`, `["hunk", text]` and `["line", kind, old, new, text, runs, wide]`,
//! where `n` indexes the files, `kind` is `+`, `-` or a space, and `runs` is
//! `[chars, tone, chars, tone, ...]` with tones indexing [`DiffView::tones`], or null for a
//! line shown plain. `wide` says the line has a character not one column wide, so it can't be
//! drawn in a column grid.

use serde::Serialize;
use unicode_width::UnicodeWidthChar;

use crate::{
    config::keys::{Command, KeyLayer, KeysConfig},
    diff::{
        Change, FileContent, Range,
        highlight::{Run, SyntaxTone},
        tab::{CollapseReason, DiffState, DiffTab, Note, RangeContent, Ready, Row, ShownFile},
    },
    display::{code_chars, sanitize},
    path::display,
};

use super::Hint;

/// The tones a line's runs index, in order.
pub const TONES: &[SyntaxTone] = &[
    SyntaxTone::Plain,
    SyntaxTone::Comment,
    SyntaxTone::Keyword,
    SyntaxTone::Operator,
    SyntaxTone::String,
    SyntaxTone::Escape,
    SyntaxTone::Number,
    SyntaxTone::Constant,
    SyntaxTone::Function,
    SyntaxTone::Type,
    SyntaxTone::Tag,
    SyntaxTone::Attribute,
    SyntaxTone::Punctuation,
    SyntaxTone::Heading,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffView {
    /// The branch, or what the tab is waiting for.
    pub title: String,
    /// What the target is compared with, such as `compared with origin/main`.
    pub subtitle: Option<String>,
    /// Shown in the body's place while there is no diff to show.
    pub message: Option<Message>,
    pub ranges: Vec<RangeEntry>,
    pub range: Option<usize>,
    pub files: Vec<FileEntry>,
    /// The file at the top of the screen.
    pub current_file: Option<usize>,
    pub body: Option<Body>,
    pub tones: &'static [SyntaxTone],
    pub hints: Vec<Hint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub text: String,
    pub detail: Option<String>,
    /// Work is under way.
    pub busy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RangeEntry {
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    pub path: String,
    /// Where a renamed or copied file came from.
    pub old_path: Option<String>,
    pub change: &'static str,
    pub added: usize,
    pub removed: usize,
    pub collapsed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Body {
    /// The first row shown, of `total`.
    pub offset: usize,
    /// The first column of code shown, when scrolled right.
    pub column: usize,
    pub total: usize,
    pub rows: Vec<RowView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum RowView {
    File(&'static str, usize),
    Note(&'static str, usize, String),
    Hunk(&'static str, String),
    Line(
        &'static str,
        &'static str,
        Option<u32>,
        Option<u32>,
        String,
        Option<Vec<u32>>,
        bool,
    ),
}

pub fn diff(tab: &DiffTab, keys: &KeysConfig) -> DiffView {
    let hints = hints(keys);
    let message = |text: &str, detail: Option<String>, busy| DiffView {
        title: "Diff".into(),
        subtitle: None,
        message: Some(Message {
            text: text.into(),
            detail,
            busy,
        }),
        ranges: Vec::new(),
        range: None,
        files: Vec::new(),
        current_file: None,
        body: None,
        tones: TONES,
        hints: hints.clone(),
    };
    match &tab.state {
        DiffState::Unstarted | DiffState::Resolving(_) => message(
            "Reading the changes…",
            tab.cwd().map(|cwd| display(cwd).into_owned()),
            true,
        ),
        DiffState::NoTarget(reason) => message(
            "Nothing to diff",
            Some(sanitize(reason).into_owned()),
            false,
        ),
        DiffState::Failed(error) => message(
            "Kiosk couldn't read the changes",
            Some(sanitize(error).into_owned()),
            false,
        ),
        DiffState::Ready(ready) => ready_view(ready, tab.viewport(), keys, hints),
    }
}

fn ready_view(ready: &Ready, viewport: usize, keys: &KeysConfig, hints: Vec<Hint>) -> DiffView {
    let target = &ready.target;
    let title = sanitize(&target.head().to_string()).into_owned();
    // On the default branch itself, saying it's compared with itself says nothing.
    let subtitle = target
        .base()
        .filter(|base| !matches!(target.head(), crate::diff::Head::Branch { name, .. } if *name == base.name))
        .map(|base| format!("compared with {}", sanitize(&base.name)));
    let ranges = ready
        .ranges
        .iter()
        .map(|range| range_entry(ready, range))
        .collect();
    let mut view = DiffView {
        title,
        subtitle,
        message: None,
        ranges,
        range: Some(ready.range),
        files: Vec::new(),
        current_file: None,
        body: None,
        tones: TONES,
        hints,
    };
    match &ready.content {
        RangeContent::Loading(_) => {
            view.message = Some(Message {
                text: "Reading the changes…".into(),
                detail: None,
                busy: true,
            });
        }
        RangeContent::Failed(error) => {
            view.message = Some(Message {
                text: "Kiosk couldn't read this range".into(),
                detail: Some(sanitize(error).into_owned()),
                busy: false,
            });
        }
        RangeContent::Shown(shown) => {
            view.files = shown.files.iter().map(file_entry).collect();
            if shown.files.is_empty() {
                view.message = Some(Message {
                    text: "No changes".into(),
                    detail: None,
                    busy: false,
                });
                return view;
            }
            view.current_file = Some(shown.current_file);
            let end = (shown.offset + viewport).min(shown.rows.len());
            view.body = Some(Body {
                offset: shown.offset,
                column: shown.column,
                total: shown.rows.len(),
                rows: shown.rows[shown.offset..end]
                    .iter()
                    .map(|row| row_view(&shown.files, *row, shown.column, keys))
                    .collect(),
            });
        }
    }
    view
}

fn range_entry(ready: &Ready, range: &Range) -> RangeEntry {
    match range {
        Range::All => RangeEntry {
            label: "All changes".into(),
            detail: match ready.commits.len() {
                0 if ready.target.has_working_tree() => "uncommitted work".into(),
                1 if ready.target.has_working_tree() => "1 commit and uncommitted work".into(),
                1 => "1 commit".into(),
                count if ready.target.has_working_tree() => {
                    format!("{count} commits and uncommitted work")
                }
                count => format!("{count} commits"),
            },
        },
        Range::Uncommitted => RangeEntry {
            label: "Uncommitted".into(),
            detail: "staged, unstaged and untracked".into(),
        },
        Range::Commit(id) => {
            let commit = ready.commits.iter().find(|commit| &commit.id == id);
            RangeEntry {
                label: commit.map_or_else(
                    || id.short().to_owned(),
                    |commit| sanitize(&commit.summary).into_owned(),
                ),
                detail: match commit {
                    Some(commit) => format!("{} · {}", id.short(), sanitize(&commit.author)),
                    None => id.short().to_owned(),
                },
            }
        }
    }
}

fn file_entry(file: &ShownFile) -> FileEntry {
    let path = |side: &Option<crate::diff::Side>| {
        side.as_ref()
            .map(|side| sanitize(&side.path.display().to_string()).into_owned())
    };
    let (change, old_path) = match file.diff.change {
        Change::Added => ("added", None),
        Change::Deleted => ("deleted", None),
        Change::Modified => ("modified", None),
        Change::Renamed { .. } => ("renamed", path(&file.diff.old)),
        Change::Copied { .. } => ("copied", path(&file.diff.old)),
    };
    FileEntry {
        path: sanitize(&file.diff.path().display().to_string()).into_owned(),
        old_path,
        change,
        added: file.added,
        removed: file.removed,
        collapsed: file.collapsed,
    }
}

fn row_view(files: &[ShownFile], row: Row, column: usize, keys: &KeysConfig) -> RowView {
    match row {
        Row::File(file) => RowView::File("file", file),
        Row::Note(index, note) => {
            RowView::Note("note", index, note_text(&files[index], note, keys))
        }
        Row::Hunk { file, hunk } => {
            let FileContent::Text { hunks } = &files[file].diff.content else {
                unreachable!("only a text file has hunks");
            };
            let hunk = &hunks[hunk];
            let mut header = format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len
            );
            if !hunk.section.is_empty() {
                header.push(' ');
                header.push_str(&code_line(&hunk.section, None, 0).0);
            }
            RowView::Hunk("hunk", header)
        }
        Row::Line { file, hunk, line } => {
            let shown = &files[file];
            let FileContent::Text { hunks } = &shown.diff.content else {
                unreachable!("only a text file has lines");
            };
            let line = &hunks[hunk].lines[line];
            let runs = shown
                .highlighted
                .as_ref()
                .and_then(|highlighted| highlighted.runs(line));
            let (text, runs) = code_line(&line.text, runs, column);
            let wide = text.chars().any(|character| character.width() != Some(1));
            RowView::Line(
                "line",
                match line.kind {
                    crate::diff::LineKind::Added => "+",
                    crate::diff::LineKind::Removed => "-",
                    crate::diff::LineKind::Context => " ",
                },
                line.old,
                line.new,
                text,
                runs,
                wide,
            )
        }
    }
}

fn note_text(file: &ShownFile, note: Note, keys: &KeysConfig) -> String {
    let expand = keys
        .diff_key(Command::ToggleFile)
        .map(|key| format!(" ({key} to show it)"))
        .unwrap_or_default();
    match note {
        Note::Binary => "Binary file, not shown".into(),
        Note::NoNewline => "No newline at end of file".into(),
        Note::Collapsed => match file.collapse_reason {
            Some(CollapseReason::Generated) => format!("A generated file, collapsed{expand}"),
            Some(CollapseReason::Large) | None => format!(
                "{} changed lines, collapsed{expand}",
                file.added + file.removed
            ),
        },
        Note::NoContentChange => match (&file.diff.old, &file.diff.new, file.diff.change) {
            (Some(old), Some(new), _) if old.mode != new.mode => {
                format!("Mode changed from {:o} to {:o}", old.mode, new.mode)
            }
            (Some(old), _, Change::Renamed { .. }) => format!(
                "Renamed from {}, with no other changes",
                sanitize(&old.path.display().to_string())
            ),
            _ => "No content changes".into(),
        },
    }
}

/// `text` as [`code_chars`] shows it, from column `from`, with `runs` stretched to match and
/// flattened to `[chars, tone index, ...]`.
fn code_line(text: &str, runs: Option<&[Run]>, from: usize) -> (String, Option<Vec<u32>>) {
    let (shown_chars, widths) = code_chars(text);
    let visible: String = shown_chars.iter().skip(from).collect();
    let runs = runs.map(|runs| {
        let mut flat = Vec::with_capacity(runs.len() * 2);
        // Where the run starts, in the line's characters and in the shown ones.
        let (mut at, mut column) = (0, 0);
        for run in runs {
            let end = (at + run.chars).min(widths.len());
            let chars: usize = widths[at.min(end)..end].iter().sum();
            at = end;
            // The part of the run right of `from`.
            let shown = (column + chars).saturating_sub(from.max(column));
            column += chars;
            if shown == 0 {
                continue;
            }
            let tone = TONES
                .iter()
                .position(|tone| *tone == run.tone)
                .expect("every tone is in TONES");
            flat.extend([
                u32::try_from(shown).unwrap_or(u32::MAX),
                u32::try_from(tone).expect("few tones"),
            ]);
        }
        flat
    });
    (visible, runs)
}

fn hints(keys: &KeysConfig) -> Vec<Hint> {
    let pair = |forward: Command, back: Command, label: &'static str| {
        let forward = keys.diff_key(forward)?;
        let back = keys.diff_key(back)?;
        Some(Hint::new(format!("{back}/{forward}"), label))
    };
    [
        pair(Command::ScrollDown, Command::ScrollUp, "scroll"),
        pair(Command::NextFile, Command::PreviousFile, "file"),
        pair(Command::NextRange, Command::PreviousRange, "range"),
        keys.diff_key(Command::ToggleFile)
            .map(|key| Hint::for_key(key, "collapse")),
        keys.layer_key(KeyLayer::Tabs, Command::NextTab)
            .map(|key| Hint::for_key(key, "next tab")),
        keys.diff_key(Command::Quit)
            .map(|key| Hint::for_key(key, "quit")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chars: usize, tone: SyntaxTone) -> Run {
        Run { chars, tone }
    }

    #[test]
    fn tabs_become_spaces_to_the_next_stop_and_stretch_their_run() {
        let runs = [
            run(1, SyntaxTone::Plain),
            run(1, SyntaxTone::Keyword),
            run(2, SyntaxTone::String),
        ];
        let (text, flat) = code_line("\tx\t\"", Some(&runs[..]), 0);
        assert_eq!(text, "    x   \"");
        let index =
            |wanted| u32::try_from(TONES.iter().position(|tone| *tone == wanted).unwrap()).unwrap();
        let (keyword, string) = (index(SyntaxTone::Keyword), index(SyntaxTone::String));
        assert_eq!(flat.unwrap(), [4, 0, 1, keyword, 4, string]);
    }

    #[test]
    fn controls_are_escaped_and_a_crlf_ending_is_dropped() {
        let (text, flat) = code_line("a\u{1b}b\r", Some(&[run(4, SyntaxTone::Plain)][..]), 0);
        assert_eq!(text, "a\\u{1b}b");
        // `\u{1b}` is six characters, and the carriage return is gone.
        assert_eq!(flat.unwrap(), [8, 0]);
        assert_eq!(code_line("plain", None, 0), ("plain".into(), None));
    }

    #[test]
    fn scrolled_right_a_line_starts_at_the_column_with_its_runs_cut_to_match() {
        let runs = [
            run(3, SyntaxTone::Keyword),
            run(3, SyntaxTone::Plain),
            run(4, SyntaxTone::String),
        ];
        let (text, flat) = code_line("let x \"a\";", Some(&runs[..]), 4);
        assert_eq!(text, "x \"a\";");
        let string = u32::try_from(
            TONES
                .iter()
                .position(|tone| *tone == SyntaxTone::String)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(flat.unwrap(), [2, 0, 4, string]);
        assert_eq!(code_line("short", None, 10), (String::new(), None));
    }

    #[test]
    fn a_very_long_line_is_converted_in_one_pass() {
        let long = "x".repeat(200_000);
        let started = std::time::Instant::now();
        let (text, _) = code_line(&long, Some(&[run(200_000, SyntaxTone::Plain)][..]), 0);
        assert_eq!(text.len(), 200_000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn rows_are_compact_arrays_on_the_wire() {
        let line = RowView::Line(
            "line",
            "+",
            None,
            Some(3),
            "x".into(),
            Some(vec![1, 2]),
            false,
        );
        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"["line","+",null,3,"x",[1,2],false]"#
        );
        assert_eq!(
            serde_json::to_string(&RowView::File("file", 2)).unwrap(),
            r#"["file",2]"#
        );
    }

    #[test]
    fn hints_pair_each_direction_preferring_named_keys() {
        let keys = KeysConfig::default();
        let labels = hints(&keys)
            .into_iter()
            .map(|hint| (hint.key, hint.label))
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                ("↑/↓".into(), "scroll"),
                ("p/n".into(), "file"),
                ("[/]".into(), "range"),
                ("e".into(), "collapse"),
                ("ctrl+pagedown".into(), "next tab"),
                ("esc".into(), "quit"),
            ]
        );
    }
}
