//! What the diff tab shows: a checkout's or a branch's changes against the default branch,
//! read from git as files, hunks and lines.
//!
//! A [`Target`] is what to diff: a checkout, whose working tree counts, or a branch without
//! one. Its [`Range`]s are everything since its merge-base with the default branch, the
//! working tree's uncommitted changes, and each commit since the merge-base. Every line keeps
//! its number on each side it is on, so a line can be pointed at by file, side and number.

mod git;
pub mod highlight;
mod patch;

use std::{fmt, path::PathBuf};

use anyhow::{Result, bail};

pub use git::{Commit, Head};

/// A git object id: 40 hex digits, or 64 in a SHA-256 repository.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectId(String);

impl ObjectId {
    pub fn parse(id: &str) -> Result<Self> {
        if !matches!(id.len(), 40 | 64) || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("{id:?} is not a git object id");
        }
        Ok(Self(id.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first seven digits, as git abbreviates ids for people.
    pub fn short(&self) -> &str {
        &self.0[..7]
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Deleted,
    /// Its content, its mode, or both.
    Modified,
    /// Moved, with `similarity` percent of its content the same.
    Renamed {
        similarity: u8,
    },
    Copied {
        similarity: u8,
    },
}

/// A file on one side of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    /// Relative to the repository's root.
    pub path: PathBuf,
    /// Its git mode, such as `0o100644`.
    pub mode: u32,
    /// The blob holding its content on this side.
    pub id: ObjectId,
}

impl Side {
    /// The type bits of a git mode, and those of a regular file.
    const TYPE: u32 = 0o170_000;
    const REGULAR_FILE: u32 = 0o100_000;

    /// Whether the side is a regular file, executable or not, rather than a symlink or a
    /// submodule.
    pub const fn is_regular_file(&self) -> bool {
        self.mode & Self::TYPE == Self::REGULAR_FILE
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub change: Change,
    /// The file before, unless it was added.
    pub old: Option<Side>,
    /// The file after, unless it was deleted.
    pub new: Option<Side>,
    pub content: FileContent,
}

impl FileDiff {
    /// The file's path after the change, or before it for a deleted file.
    pub fn path(&self) -> &std::path::Path {
        &self
            .new
            .as_ref()
            .or(self.old.as_ref())
            .expect("a file is on at least one side")
            .path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    /// The changed lines with their context; none when only the name or mode changed.
    Text {
        hunks: Vec<Hunk>,
    },
    Binary,
}

impl FileContent {
    /// Both contents as one, for a file git patches in two parts: binary if either is.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Text { mut hunks }, Self::Text { hunks: more }) => {
                hunks.extend(more);
                Self::Text { hunks }
            }
            _ => Self::Binary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    /// What git found around the hunk, such as the function it is in; possibly empty.
    pub section: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// The line's number in the old file, unless it was added.
    pub old: Option<u32>,
    /// The line's number in the new file, unless it was removed.
    pub new: Option<u32>,
    /// Without its newline; invalid UTF-8 is replaced.
    pub text: String,
    /// The file ends here without a newline.
    pub missing_newline: bool,
}

/// A file's lines highlighted on each side it is on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Highlighted {
    old: Option<Vec<Vec<highlight::Run>>>,
    new: Option<Vec<Vec<highlight::Run>>>,
}

impl Highlighted {
    /// `line`'s runs, from the side it is on (the new side for context), or `None` when that
    /// side went unhighlighted or its runs don't cover the line's text exactly.
    pub fn runs(&self, line: &Line) -> Option<&[highlight::Run]> {
        let (side, number) = match line.kind {
            LineKind::Removed => (&self.old, line.old?),
            LineKind::Added | LineKind::Context => (&self.new, line.new?),
        };
        let runs = side
            .as_ref()?
            .get(usize::try_from(number).ok()?.checked_sub(1)?)?;
        (runs.iter().map(|run| run.chars).sum::<usize>() == line.text.chars().count())
            .then_some(runs.as_slice())
    }
}

/// What the diff tab diffs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// A checkout of the repository, where git runs.
    root: PathBuf,
    /// The checkout's working tree counts, or only `head`'s commits do.
    working_tree: bool,
    head: Head,
    /// Where the target's history joins the default branch's, if it has one.
    base: Option<Base>,
}

/// The default branch the target is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    /// As people write it, such as `origin/main` or `main`.
    pub name: String,
    pub merge_base: ObjectId,
}

/// A part of the target's changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Range {
    /// Everything since the merge-base, uncommitted changes included: what a pull request
    /// would hold if they were committed.
    All,
    /// The working tree against `HEAD`, untracked files included.
    Uncommitted,
    Commit(ObjectId),
}

impl Target {
    /// The checkout containing `path`, with its working tree, or `None` when `path` is in no
    /// repository.
    pub fn checkout(path: &std::path::Path) -> Result<Option<Self>> {
        let Some(root) = git::toplevel(path)? else {
            return Ok(None);
        };
        Self::at(root, true, "HEAD").map(Some)
    }

    /// `branch` of the repository a checkout of which is at `checkout`, without any working
    /// tree: only its commits.
    pub fn branch(checkout: &std::path::Path, branch: &str) -> Result<Self> {
        let Some(root) = git::toplevel(checkout)? else {
            bail!("{} is not in a git repository", checkout.display());
        };
        Self::at(root, false, &format!("refs/heads/{branch}"))
    }

    fn at(root: PathBuf, working_tree: bool, head: &str) -> Result<Self> {
        let head = git::head(&root, head)?;
        let base = match head.id() {
            Some(id) => git::base(&root, id)?,
            None => None,
        };
        Ok(Self {
            root,
            working_tree,
            head,
            base,
        })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn head(&self) -> &Head {
        &self.head
    }

    pub fn base(&self) -> Option<&Base> {
        self.base.as_ref()
    }

    pub const fn has_working_tree(&self) -> bool {
        self.working_tree
    }

    /// The commits since the merge-base, newest first; none without a base.
    pub fn commits(&self) -> Result<Vec<Commit>> {
        match (&self.base, self.head.id()) {
            (Some(base), Some(head)) => git::commits(&self.root, &base.merge_base, head),
            _ => Ok(Vec::new()),
        }
    }

    /// The ranges there are to show, in the order to offer them.
    pub fn ranges(&self, commits: &[Commit]) -> Vec<Range> {
        let mut ranges = Vec::new();
        if self.base.is_some() {
            ranges.push(Range::All);
        }
        if self.working_tree {
            ranges.push(Range::Uncommitted);
        }
        ranges.extend(
            commits
                .iter()
                .map(|commit| Range::Commit(commit.id.clone())),
        );
        ranges
    }

    /// `file`'s lines highlighted on each side, as far as they can be.
    pub fn highlight(&self, file: &FileDiff) -> Result<Highlighted> {
        // A binary file has no lines, and nor does a rename or mode change with no hunks:
        // reading and parsing both sides would be for nothing.
        let FileContent::Text { hunks } = &file.content else {
            return Ok(Highlighted::default());
        };
        if hunks.iter().all(|hunk| hunk.lines.is_empty()) {
            return Ok(Highlighted::default());
        }
        let side = |side: &Option<Side>| -> Result<Option<Vec<Vec<highlight::Run>>>> {
            // Only a regular file's blob is code: a symlink's is its target, and a
            // submodule's id names a commit, often only in the submodule.
            let Some(side) = side.as_ref().filter(|side| side.is_regular_file()) else {
                return Ok(None);
            };
            let Some(content) = git::blob(&self.root, &side.id, highlight::MAX_HIGHLIGHT_BYTES)?
            else {
                return Ok(None);
            };
            Ok(highlight::highlight(
                &side.path,
                &String::from_utf8_lossy(&content),
            ))
        };
        Ok(Highlighted {
            old: side(&file.old)?,
            new: side(&file.new)?,
        })
    }

    /// The files `range` changed.
    pub fn diff(&self, range: &Range) -> Result<Vec<FileDiff>> {
        let to = |head: Option<&ObjectId>| -> Result<git::Endpoint> {
            if self.working_tree {
                return Ok(git::Endpoint::WorkingTree);
            }
            match head {
                Some(id) => Ok(git::Endpoint::Commit(id.clone())),
                None => bail!("{} has no commits", self.head),
            }
        };
        match range {
            Range::All => {
                let Some(base) = &self.base else {
                    bail!("{} has no default branch to compare with", self.head);
                };
                git::diff(&self.root, Some(&base.merge_base), &to(self.head.id())?)
            }
            Range::Uncommitted => {
                if !self.working_tree {
                    bail!("{} has no working tree", self.head);
                }
                git::diff(&self.root, self.head.id(), &git::Endpoint::WorkingTree)
            }
            Range::Commit(id) => git::diff(
                &self.root,
                git::first_parent(&self.root, id)?.as_ref(),
                &git::Endpoint::Commit(id.clone()),
            ),
        }
    }
}

#[cfg(test)]
mod tests;
