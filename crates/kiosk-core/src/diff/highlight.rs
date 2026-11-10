//! Syntax highlighting for the diff tab: each line of a file as runs of [`SyntaxTone`]s.
//!
//! Whole files are highlighted, not hunks, so multi-line strings and comments come out right.
//! Tones say what a run is rather than its colour, for the frontend's theme to colour, so a
//! `TextMate` theme is never involved: syntect's scopes are matched against [`SELECTORS`]
//! directly.

use std::{
    collections::HashMap,
    path::Path,
    str::FromStr,
    sync::{LazyLock, OnceLock},
};

use serde::Serialize;
use syntect::{
    highlighting::ScopeSelectors,
    parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

/// Files larger than this go unhighlighted: highlighting is far slower than reading.
pub const MAX_HIGHLIGHT_BYTES: usize = 1024 * 1024;
/// A file with a line longer than this, such as minified code, goes unhighlighted.
pub const MAX_HIGHLIGHT_LINE_BYTES: usize = 4096;

/// What a run of code is, for the frontend's theme to colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxTone {
    Plain,
    Comment,
    Keyword,
    Operator,
    String,
    Escape,
    Number,
    Constant,
    Function,
    Type,
    Tag,
    Attribute,
    Punctuation,
    Heading,
}

/// A run of characters (Unicode scalar values, not bytes) in one tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Run {
    pub chars: usize,
    pub tone: SyntaxTone,
}

/// Which tone each kind of scope gets. A scope stack takes the tone of the selector that
/// matches it most specifically, the earlier one on a tie.
const SELECTORS: &[(&str, SyntaxTone)] = &[
    ("comment", SyntaxTone::Comment),
    ("string", SyntaxTone::String),
    ("constant.character.escape", SyntaxTone::Escape),
    ("constant.numeric", SyntaxTone::Number),
    (
        "constant.language, constant.other, support.constant, variable.language",
        SyntaxTone::Constant,
    ),
    ("keyword, storage", SyntaxTone::Keyword),
    ("keyword.operator", SyntaxTone::Operator),
    (
        "entity.name.function, support.function, variable.function, meta.function-call",
        SyntaxTone::Function,
    ),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
         entity.name.trait, entity.other.inherited-class, support.type, support.class",
        SyntaxTone::Type,
    ),
    ("entity.name.tag", SyntaxTone::Tag),
    (
        "entity.other.attribute-name, meta.annotation",
        SyntaxTone::Attribute,
    ),
    // A comment's or string's own punctuation, such as `//` or quotes, is part of it.
    ("punctuation - comment - string", SyntaxTone::Punctuation),
    ("markup.heading", SyntaxTone::Heading),
];

static PARSED_SELECTORS: LazyLock<Vec<(ScopeSelectors, SyntaxTone)>> = LazyLock::new(|| {
    SELECTORS
        .iter()
        .map(|(selector, tone)| {
            (
                ScopeSelectors::from_str(selector).expect("valid scope selector"),
                *tone,
            )
        })
        .collect()
});

/// The grammars syntect ships, loaded once: it takes tens of milliseconds.
fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn tone(stack: &[Scope]) -> SyntaxTone {
    let mut best: Option<(syntect::parsing::MatchPower, SyntaxTone)> = None;
    for (selector, tone) in PARSED_SELECTORS.iter() {
        if let Some(power) = selector.does_match(stack)
            && best.is_none_or(|(best, _)| power > best)
        {
            best = Some((power, *tone));
        }
    }
    best.map_or(SyntaxTone::Plain, |(_, tone)| tone)
}

fn syntax_for(path: &Path, text: &str) -> Option<&'static SyntaxReference> {
    let syntaxes = syntaxes();
    let name = path.file_name()?.to_str()?;
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(|extension| syntaxes.find_syntax_by_extension(extension))
        // Files known by name, such as `Makefile`, are in the extension table too.
        .or_else(|| syntaxes.find_syntax_by_extension(name))
        .or_else(|| syntaxes.find_syntax_by_first_line(text.lines().next()?))
}

/// Each line of `text`, the content of the file at `path`, as runs of tones, without the
/// line's newline. `None` when there is no grammar for the file or it is too big to highlight.
pub fn highlight(path: &Path, text: &str) -> Option<Vec<Vec<Run>>> {
    if text.len() > MAX_HIGHLIGHT_BYTES
        || text
            .split('\n')
            .any(|line| line.len() > MAX_HIGHLIGHT_LINE_BYTES)
    {
        return None;
    }
    let syntax = syntax_for(path, text)?;
    if syntax.name == "Plain Text" {
        return None;
    }
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut tones: HashMap<Vec<Scope>, SyntaxTone> = HashMap::new();
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(text) {
        let ops = state.parse_line(line, syntaxes()).ok()?;
        let content = line.strip_suffix('\n').unwrap_or(line);
        let mut runs: Vec<Run> = Vec::new();
        let mut start = 0;
        let mut push = |stack: &ScopeStack, end: usize, runs: &mut Vec<Run>| {
            let end = end.min(content.len());
            if end <= start {
                return;
            }
            let chars = content[start..end].chars().count();
            let tone = *tones
                .entry(stack.as_slice().to_vec())
                .or_insert_with(|| tone(stack.as_slice()));
            match runs.last_mut() {
                Some(last) if last.tone == tone => last.chars += chars,
                _ => runs.push(Run { chars, tone }),
            }
            start = end;
        };
        for (at, op) in ops {
            push(&stack, at, &mut runs);
            stack.apply(&op).ok()?;
        }
        push(&stack, content.len(), &mut runs);
        lines.push(runs);
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each run's text with its tone.
    fn spans(line: &str, runs: &[Run]) -> Vec<(String, SyntaxTone)> {
        let mut chars = line.chars();
        runs.iter()
            .map(|run| (chars.by_ref().take(run.chars).collect(), run.tone))
            .collect()
    }

    #[test]
    fn rust_comes_out_in_tones_covering_every_character() {
        let text = "// note\nfn main() {\n    let s = \"a\\n\";\n}\n";
        let lines = highlight(Path::new("src/main.rs"), text).unwrap();
        assert_eq!(lines.len(), 4);
        for (line, runs) in text.lines().zip(&lines) {
            assert_eq!(
                runs.iter().map(|run| run.chars).sum::<usize>(),
                line.chars().count(),
                "{line:?}"
            );
            assert!(runs.windows(2).all(|pair| pair[0].tone != pair[1].tone));
        }
        assert_eq!(
            spans("// note", &lines[0]),
            [("// note".into(), SyntaxTone::Comment)]
        );
        let main = spans("fn main() {", &lines[1]);
        assert!(
            main.contains(&("fn".into(), SyntaxTone::Keyword)),
            "{main:?}"
        );
        assert!(
            main.contains(&("main".into(), SyntaxTone::Function)),
            "{main:?}"
        );
        let string = spans("    let s = \"a\\n\";", &lines[2]);
        assert!(
            string.contains(&("\\n".into(), SyntaxTone::Escape)),
            "{string:?}"
        );
    }

    #[test]
    fn a_multi_line_comment_stays_a_comment_on_its_later_lines() {
        let lines = highlight(Path::new("a.rs"), "/*\nstill a comment\n*/\nfn x() {}\n").unwrap();
        assert_eq!(
            lines[1],
            [Run {
                chars: "still a comment".len(),
                tone: SyntaxTone::Comment
            }]
        );
    }

    #[test]
    fn runs_count_characters_not_bytes() {
        let lines = highlight(Path::new("a.py"), "s = \"héllo → 世界\"\n").unwrap();
        assert_eq!(
            lines[0].iter().map(|run| run.chars).sum::<usize>(),
            "s = \"héllo → 世界\"".chars().count()
        );
    }

    #[test]
    fn files_are_found_by_name_and_by_first_line() {
        assert!(highlight(Path::new("Makefile"), "all:\n\techo hi\n").is_some());
        assert!(highlight(Path::new("run"), "#!/bin/bash\necho hi\n").is_some());
    }

    #[test]
    fn unknown_huge_and_minified_files_go_unhighlighted() {
        assert_eq!(
            highlight(Path::new("notes.unknownext"), "just text\n"),
            None
        );
        assert_eq!(highlight(Path::new("a.txt"), "just text\n"), None);
        let huge = "let x = 1;\n".repeat(MAX_HIGHLIGHT_BYTES / 10);
        assert_eq!(highlight(Path::new("a.rs"), &huge), None);
        let minified = format!("var a={};\n", "1+".repeat(MAX_HIGHLIGHT_LINE_BYTES));
        assert_eq!(highlight(Path::new("a.js"), &minified), None);
    }

    #[test]
    fn a_carriage_return_is_part_of_its_line() {
        let lines = highlight(Path::new("a.rs"), "fn a() {}\r\nfn b() {}\r\n").unwrap();
        assert_eq!(
            lines[0].iter().map(|run| run.chars).sum::<usize>(),
            "fn a() {}\r".chars().count()
        );
    }
}
