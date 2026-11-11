//! Parsing `git diff --raw -z --patch --no-abbrev` output into [`FileDiff`]s.
//!
//! The raw records come first, NUL-separated, then an empty record, then the patch. Each
//! raw record has one `diff --git` section in the patch, in the same order, except a change
//! of type (such as a file becoming a symlink), which has two: the old file's deletion and
//! the new one's addition. Paths, modes,
//! object ids and the kind of change come from the raw records, which git writes exactly
//! (no quoting) with `-z`; only the hunks, and whether the file is binary, come from the
//! patch.

use anyhow::{Context, Result, bail, ensure};

use super::{Change, FileContent, FileDiff, Hunk, Line, LineKind, ObjectId, Side};
use crate::git::path_from_bytes;

const SECTION_START: &[u8] = b"diff --git ";
const HUNK_START: &[u8] = b"@@ ";
const NO_NEWLINE: &[u8] = b"\\ No newline at end of file";

/// The files `output` describes, in git's order.
pub fn parse(output: &[u8]) -> Result<Vec<FileDiff>> {
    let (raw, patch) = split_raw(output)?;
    let records = parse_raw(raw)?;
    let sections = split_sections(patch);
    let expected = records.iter().map(|record| record.sections).sum::<usize>();
    ensure!(
        expected == sections.len(),
        "git diff described {} files in {expected} patches but wrote {}",
        records.len(),
        sections.len()
    );
    let mut sections = sections.into_iter();
    records
        .into_iter()
        .map(|record| {
            let content = (0..record.sections)
                .map(|_| parse_section(sections.next().expect("counted above")))
                .try_fold(
                    FileContent::Text { hunks: Vec::new() },
                    |joined, content| Ok::<_, anyhow::Error>(joined.join(content?)),
                )
                .with_context(|| {
                    format!(
                        "reading the patch of {}",
                        record.new.as_ref().or(record.old.as_ref()).map_or_else(
                            || "a file".into(),
                            |side| side.path.display().to_string()
                        )
                    )
                })?;
            Ok(FileDiff {
                change: record.change,
                old: record.old,
                new: record.new,
                content,
            })
        })
        .collect()
}

/// The raw records, and the patch after the empty record that ends them.
fn split_raw(output: &[u8]) -> Result<(&[u8], &[u8])> {
    if output.is_empty() {
        return Ok((&[], &[]));
    }
    // A record's fields never hold an empty string, so the first empty one ends the raw part.
    let mut start = 0;
    for (index, byte) in output.iter().enumerate() {
        if *byte == 0 {
            if index == start {
                return Ok((&output[..start], &output[index + 1..]));
            }
            start = index + 1;
        }
    }
    bail!("git diff wrote raw records with no patch after them")
}

struct RawRecord {
    change: Change,
    old: Option<Side>,
    new: Option<Side>,
    /// How many `diff --git` sections the patch has for it.
    sections: usize,
}

fn parse_raw(raw: &[u8]) -> Result<Vec<RawRecord>> {
    let mut fields = raw
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut records = Vec::new();
    while let Some(header) = fields.next() {
        let header = std::str::from_utf8(header).context("a raw record's header isn't UTF-8")?;
        let header = header
            .strip_prefix(':')
            .with_context(|| format!("a raw record starts {header:?}"))?;
        let parts = header.split(' ').collect::<Vec<_>>();
        let [old_mode, new_mode, old_id, new_id, status] = parts[..] else {
            bail!("a raw record's header is {header:?}");
        };
        let mut path = || -> Result<std::path::PathBuf> {
            path_from_bytes(fields.next().context("a raw record has no path")?.to_vec())
        };
        let kind = status
            .chars()
            .next()
            .context("a raw record has no status")?;
        let (old_path, new_path) = match kind {
            'R' | 'C' => (path()?, path()?),
            _ => {
                let path = path()?;
                (path.clone(), path)
            }
        };
        // A side the file isn't on has mode 0 and an id of zeros.
        let side = |mode: &str, id: &str, path| -> Result<Option<Side>> {
            let mode = u32::from_str_radix(mode, 8).with_context(|| format!("mode {mode:?}"))?;
            if mode == 0 {
                return Ok(None);
            }
            Ok(Some(Side {
                path,
                mode,
                id: ObjectId::parse(id)?,
            }))
        };
        let similarity = || -> Result<u8> {
            status[1..]
                .parse()
                .with_context(|| format!("status {status:?} has no similarity"))
        };
        let change = match kind {
            'A' => Change::Added,
            'D' => Change::Deleted,
            'M' | 'T' => Change::Modified,
            'R' => Change::Renamed {
                similarity: similarity()?,
            },
            'C' => Change::Copied {
                similarity: similarity()?,
            },
            _ => bail!("git diff reported status {status:?}, which kiosk doesn't know"),
        };
        records.push(RawRecord {
            change,
            old: side(old_mode, old_id, old_path)?,
            new: side(new_mode, new_id, new_path)?,
            sections: if kind == 'T' { 2 } else { 1 },
        });
    }
    Ok(records)
}

/// Each `diff --git` section of the patch.
fn split_sections(patch: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in patch.split_inclusive(|byte| *byte == b'\n') {
        if line.starts_with(SECTION_START) {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| &patch[start..starts.get(index + 1).copied().unwrap_or(patch.len())])
        .collect()
}

fn parse_section(section: &[u8]) -> Result<FileContent> {
    // The section's last line ends with a newline, which isn't another line.
    let section = section.strip_suffix(b"\n").unwrap_or(section);
    let mut lines = section.split(|byte| *byte == b'\n').skip(1).peekable();
    let mut hunks = Vec::new();
    while let Some(line) = lines.next() {
        if line.starts_with(b"Binary files ") || line == b"GIT binary patch" {
            return Ok(FileContent::Binary);
        }
        if !line.starts_with(HUNK_START) {
            // Extended headers (modes, renames, `index`, `---`/`+++`) carry nothing the raw
            // record doesn't; a hunk's own lines are read with the hunk.
            continue;
        }
        let mut hunk = parse_hunk_header(line)?;
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        let (mut old_left, mut new_left) = (hunk.old_len, hunk.new_len);
        while old_left > 0 || new_left > 0 {
            let line = lines.next().context("a hunk ends before its counts")?;
            // With `diff.suppressBlankEmpty`, a blank context line has no leading space.
            let (kind, text) = line.split_first().unwrap_or((&b' ', &[]));
            let (kind, old_number, new_number) = match kind {
                b' ' => (LineKind::Context, Some(old), Some(new)),
                b'-' => (LineKind::Removed, Some(old), None),
                b'+' => (LineKind::Added, None, Some(new)),
                other => bail!("a hunk line starts with {:?}", char::from(*other)),
            };
            if old_number.is_some() {
                ensure!(old_left > 0, "a hunk has more old lines than it said");
                old += 1;
                old_left -= 1;
            }
            if new_number.is_some() {
                ensure!(new_left > 0, "a hunk has more new lines than it said");
                new += 1;
                new_left -= 1;
            }
            let mut line = Line {
                kind,
                old: old_number,
                new: new_number,
                text: String::from_utf8_lossy(text).into_owned(),
                missing_newline: false,
            };
            if lines.peek() == Some(&NO_NEWLINE) {
                lines.next();
                line.missing_newline = true;
            }
            hunk.lines.push(line);
        }
        hunks.push(hunk);
    }
    Ok(FileContent::Text { hunks })
}

/// `@@ -old_start[,old_len] +new_start[,new_len] @@ section`.
pub(super) fn parse_hunk_header(line: &[u8]) -> Result<Hunk> {
    let line = String::from_utf8_lossy(line);
    let rest = line
        .strip_prefix("@@ -")
        .with_context(|| format!("hunk header {line:?}"))?;
    let (ranges, section) = rest
        .split_once(" @@")
        .with_context(|| format!("hunk header {line:?}"))?;
    let (old, new) = ranges
        .split_once(" +")
        .with_context(|| format!("hunk header {line:?}"))?;
    let range = |range: &str| -> Result<(u32, u32)> {
        let (start, len) = range.split_once(',').unwrap_or((range, "1"));
        Ok((
            start
                .parse()
                .with_context(|| format!("hunk header {line:?}"))?,
            len.parse()
                .with_context(|| format!("hunk header {line:?}"))?,
        ))
    };
    let (old_start, old_len) = range(old)?;
    let (new_start, new_len) = range(new)?;
    Ok(Hunk {
        old_start,
        old_len,
        new_start,
        new_len,
        section: section.strip_prefix(' ').unwrap_or(section).to_owned(),
        lines: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const ID_A: &str = "1111111111111111111111111111111111111111";
    const ID_B: &str = "2222222222222222222222222222222222222222";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    fn raw(records: &[(&str, &[&str])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (header, paths) in records {
            out.extend_from_slice(header.as_bytes());
            out.push(0);
            for path in *paths {
                out.extend_from_slice(path.as_bytes());
                out.push(0);
            }
        }
        out.push(0);
        out
    }

    fn output(records: &[(&str, &[&str])], patch: &str) -> Vec<u8> {
        let mut out = raw(records);
        out.extend_from_slice(patch.as_bytes());
        out
    }

    fn hunks(file: &FileDiff) -> &[Hunk] {
        match &file.content {
            FileContent::Text { hunks } => hunks,
            FileContent::Binary => panic!("binary"),
        }
    }

    #[test]
    fn nothing_changed_is_no_files() {
        assert_eq!(parse(b"").unwrap(), []);
    }

    #[test]
    fn a_modified_file_numbers_each_line_on_its_sides() {
        let out = output(
            &[(&format!(":100644 100644 {ID_A} {ID_B} M"), &["src/a.rs"])],
            "diff --git a/src/a.rs b/src/a.rs\n\
             index 1111111..2222222 100644\n\
             --- a/src/a.rs\n\
             +++ b/src/a.rs\n\
             @@ -1,3 +1,3 @@ fn main() {\n \
             one\n\
             -two\n\
             +TWO\n \
             three\n",
        );
        let files = parse(&out).unwrap();
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.change, Change::Modified);
        assert_eq!(file.new.as_ref().unwrap().path, Path::new("src/a.rs"));
        assert_eq!(
            file.old.as_ref().unwrap().id,
            ObjectId::parse(ID_A).unwrap()
        );
        let hunk = &hunks(file)[0];
        assert_eq!(hunk.section, "fn main() {");
        let lines = hunk
            .lines
            .iter()
            .map(|line| (line.kind, line.old, line.new, line.text.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            [
                (LineKind::Context, Some(1), Some(1), "one"),
                (LineKind::Removed, Some(2), None, "two"),
                (LineKind::Added, None, Some(2), "TWO"),
                (LineKind::Context, Some(3), Some(3), "three"),
            ]
        );
    }

    #[test]
    fn hunk_lines_that_look_like_headers_are_content() {
        let out = output(
            &[(&format!(":100644 100644 {ID_A} {ID_B} M"), &["a"])],
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n---- a\n+++++ b\n diff --git x\n",
        );
        let lines = hunks(&parse(&out).unwrap()[0])[0]
            .lines
            .iter()
            .map(|line| (line.kind, line.text.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            [
                (LineKind::Removed, "--- a".into()),
                (LineKind::Added, "++++ b".into()),
                (LineKind::Context, "diff --git x".into()),
            ]
        );
    }

    #[test]
    fn added_deleted_renamed_and_mode_only_changes_come_from_the_raw_records() {
        let out = output(
            &[
                (&format!(":000000 100644 {ZERO} {ID_A} A"), &["new.txt"]),
                (&format!(":100644 000000 {ID_A} {ZERO} D"), &["gone.txt"]),
                (
                    &format!(":100644 100644 {ID_A} {ID_A} R100"),
                    &["old name.txt", "new\tname.txt"],
                ),
                (&format!(":100644 100755 {ID_B} {ID_B} M"), &["run.sh"]),
            ],
            "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\n\
             diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n\
             diff --git a/old name.txt \"b/new\\tname.txt\"\nsimilarity index 100%\nrename from old name.txt\nrename to \"new\\tname.txt\"\n\
             diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n",
        );
        let files = parse(&out).unwrap();
        assert_eq!(files[0].change, Change::Added);
        assert!(files[0].old.is_none());
        assert_eq!(hunks(&files[0])[0].lines[0].new, Some(1));
        assert_eq!(files[1].change, Change::Deleted);
        assert!(files[1].new.is_none());
        assert_eq!(files[2].change, Change::Renamed { similarity: 100 });
        assert_eq!(
            files[2].old.as_ref().unwrap().path,
            Path::new("old name.txt")
        );
        assert_eq!(
            files[2].new.as_ref().unwrap().path,
            Path::new("new\tname.txt")
        );
        assert_eq!(hunks(&files[2]), []);
        assert_eq!(files[3].old.as_ref().unwrap().mode, 0o100_644);
        assert_eq!(files[3].new.as_ref().unwrap().mode, 0o100_755);
        assert_eq!(hunks(&files[3]), []);
    }

    #[test]
    fn a_type_change_joins_its_two_patches() {
        let out = output(
            &[(&format!(":100644 120000 {ID_A} {ID_B} T"), &["link"])],
            "diff --git a/link b/link\ndeleted file mode 100644\n--- a/link\n+++ /dev/null\n@@ -1 +0,0 @@\n-file\n\
             diff --git a/link b/link\nnew file mode 120000\n--- /dev/null\n+++ b/link\n@@ -0,0 +1 @@\n+target\n\\ No newline at end of file\n",
        );
        let files = parse(&out).unwrap();
        assert_eq!(files.len(), 1);
        let kinds = hunks(&files[0])
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .map(|line| line.kind)
            .collect::<Vec<_>>();
        assert_eq!(kinds, [LineKind::Removed, LineKind::Added]);
    }

    #[test]
    fn a_blank_context_line_without_its_space_is_context() {
        let out = output(
            &[(&format!(":100644 100644 {ID_A} {ID_B} M"), &["a"])],
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n\n-x\n+y\n",
        );
        let files = parse(&out).unwrap();
        let first = &hunks(&files[0])[0].lines[0];
        assert_eq!((first.kind, first.text.as_str()), (LineKind::Context, ""));
    }

    #[test]
    fn binary_files_have_no_hunks() {
        let out = output(
            &[(&format!(":100644 100644 {ID_A} {ID_B} M"), &["logo.png"])],
            "diff --git a/logo.png b/logo.png\nindex 1111111..2222222 100644\nBinary files a/logo.png and b/logo.png differ\n",
        );
        assert_eq!(parse(&out).unwrap()[0].content, FileContent::Binary);
    }

    #[test]
    fn a_missing_final_newline_marks_its_line_on_either_side() {
        let out = output(
            &[(&format!(":100644 100644 {ID_A} {ID_B} M"), &["a"])],
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n",
        );
        let files = parse(&out).unwrap();
        let lines = &hunks(&files[0])[0].lines;
        assert!(lines.iter().all(|line| line.missing_newline));
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn non_utf8_content_is_kept_lossily() {
        let mut out = output(
            &[(&format!(":100644 100644 {ID_A} {ID_B} M"), &["a"])],
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-",
        );
        out.extend_from_slice(b"caf\xe9\n+cafe\n");
        let files = parse(&out).unwrap();
        let lines = &hunks(&files[0])[0].lines;
        assert_eq!(lines[0].text, "caf\u{fffd}");
    }

    #[test]
    fn a_patch_that_disagrees_with_its_records_is_refused() {
        let out = output(&[(&format!(":100644 100644 {ID_A} {ID_B} M"), &["a"])], "");
        assert!(
            parse(&out)
                .unwrap_err()
                .to_string()
                .contains("described 1 files in 1 patches but wrote 0")
        );
        let short = output(
            &[(&format!(":100644 100644 {ID_A} {ID_B} M"), &["a"])],
            "diff --git a/a b/a\n@@ -1,2 +1,2 @@\n x\n",
        );
        assert!(format!("{:#}", parse(&short).unwrap_err()).contains("ends before its counts"));
    }
}
