//! The git commands behind [`super::Target`].

use std::{
    ffi::OsStr,
    fmt, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use super::{Base, FileDiff, ObjectId, patch};
use crate::git::{
    LOCAL_DEFAULT_BRANCHES, path_from_bytes, remote_default_branch, run_git, run_git_with_env,
};

/// What a target's `HEAD` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch {
        name: String,
        id: ObjectId,
    },
    Detached(ObjectId),
    /// A branch with no commits yet, as in a new repository.
    Unborn {
        name: String,
    },
}

impl Head {
    pub const fn id(&self) -> Option<&ObjectId> {
        match self {
            Self::Branch { id, .. } | Self::Detached(id) => Some(id),
            Self::Unborn { .. } => None,
        }
    }
}

impl fmt::Display for Head {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Branch { name, .. } | Self::Unborn { name } => formatter.write_str(name),
            Self::Detached(id) => write!(formatter, "detached HEAD at {}", id.short()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub id: ObjectId,
    /// The first line of its message.
    pub summary: String,
    pub author: String,
    /// When it was authored, in seconds since the Unix epoch.
    pub time: i64,
}

/// Where a diff ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Commit(ObjectId),
    /// The checkout's files as they are, untracked ones included and ignored ones not.
    WorkingTree,
}

/// The arguments every diff is read with. Git's config can't change what kiosk parses:
/// colour, external diff tools and text conversion are off, and object ids are written whole.
const DIFF_ARGS: [&str; 9] = [
    "diff",
    "--raw",
    "-z",
    "--patch",
    "--no-abbrev",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--find-renames",
];

/// Runs git, answering `None` when it fails, for questions whose answer can be "no": a ref
/// that doesn't exist, histories with no merge-base, a detached `HEAD`.
fn git_optional(root: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run git in {}", root.display()))?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

/// The top of the checkout `path` is in, or `None` when it is in no repository.
pub fn toplevel(path: &Path) -> Result<Option<PathBuf>> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .with_context(|| format!("failed to run git in {}", path.display()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let mut stdout = output.stdout;
    while stdout
        .last()
        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
    {
        stdout.pop();
    }
    path_from_bytes(stdout).map(Some)
}

/// The commit `rev` names, if it names one.
fn commit(root: &Path, rev: &str) -> Result<Option<ObjectId>> {
    git_optional(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )?
    .map(|id| ObjectId::parse(&id))
    .transpose()
}

/// `rev`, which is `HEAD` or a branch's full ref.
pub fn head(root: &Path, rev: &str) -> Result<Head> {
    let refname = if rev == "HEAD" {
        git_optional(root, &["symbolic-ref", "--quiet", "HEAD"])?
    } else {
        Some(rev.to_owned())
    };
    let id = commit(root, rev)?;
    match (refname, id) {
        (Some(refname), id) => {
            let name = refname
                .strip_prefix("refs/heads/")
                .unwrap_or(&refname)
                .to_owned();
            match id {
                Some(id) => Ok(Head::Branch { name, id }),
                None if rev == "HEAD" => Ok(Head::Unborn { name }),
                None => bail!("there is no branch {name}"),
            }
        }
        (None, Some(id)) => Ok(Head::Detached(id)),
        (None, None) => bail!("HEAD in {} names no commit", root.display()),
    }
}

/// The default branch `head` is compared with: the one a remote names (`origin` first),
/// else a local `main` or `master`, with the merge-base. `None` when there is none, or it
/// shares no history with `head`.
pub fn base(root: &Path, head: &ObjectId) -> Result<Option<Base>> {
    let mut candidates = Vec::new();
    if let Some(remote) = remote_default_branch(root)? {
        candidates.push((
            format!("refs/remotes/{}/{}", remote.remote, remote.branch),
            format!("{}/{}", remote.remote, remote.branch),
        ));
    }
    for branch in LOCAL_DEFAULT_BRANCHES {
        candidates.push((format!("refs/heads/{branch}"), branch.to_owned()));
    }
    for (refname, name) in candidates {
        if commit(root, &refname)?.is_none() {
            continue;
        }
        let Some(merge_base) = git_optional(root, &["merge-base", &refname, head.as_str()])? else {
            continue;
        };
        return Ok(Some(Base {
            name,
            merge_base: ObjectId::parse(&merge_base)?,
        }));
    }
    Ok(None)
}

/// The commits after `from` up to `to`, newest first.
pub fn commits(root: &Path, from: &ObjectId, to: &ObjectId) -> Result<Vec<Commit>> {
    const FIELD: char = '\u{1f}';
    const RECORD: char = '\u{1e}';
    let range = format!("{from}..{to}");
    let output = run_git(root, ["log", "--format=%H%x1f%s%x1f%an%x1f%at%x1e", &range])?;
    String::from_utf8_lossy(&output.stdout)
        .split(RECORD)
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let fields = record.split(FIELD).collect::<Vec<_>>();
            let [id, summary, author, time] = fields[..] else {
                bail!("git log wrote {record:?}");
            };
            Ok(Commit {
                id: ObjectId::parse(id)?,
                summary: summary.to_owned(),
                author: author.to_owned(),
                time: time
                    .parse()
                    .with_context(|| format!("git log wrote the time {time:?}"))?,
            })
        })
        .collect()
}

/// `id`'s first parent, or `None` for a root commit.
pub fn first_parent(root: &Path, id: &ObjectId) -> Result<Option<ObjectId>> {
    commit(root, &format!("{id}^1"))
}

/// The files that changed from `from` (the empty tree when `None`) to `to`.
pub fn diff(root: &Path, from: Option<&ObjectId>, to: &Endpoint) -> Result<Vec<FileDiff>> {
    let from = match from {
        Some(id) => id.clone(),
        None => empty_tree(root)?,
    };
    let output = match to {
        Endpoint::Commit(to) => {
            let mut args = DIFF_ARGS.to_vec();
            args.extend([from.as_str(), to.as_str()]);
            run_git(root, args)?
        }
        Endpoint::WorkingTree => {
            let index = WorkingTreeIndex::new(root)?;
            let mut args = DIFF_ARGS.to_vec();
            args.extend(["--cached", from.as_str()]);
            run_git_with_env(root, args, &index.env())?
        }
    };
    patch::parse(&output.stdout)
}

/// The id of the empty tree, which differs between SHA-1 and SHA-256 repositories.
fn empty_tree(root: &Path) -> Result<ObjectId> {
    let output = run_git(root, ["hash-object", "-t", "tree", "--stdin"])?;
    ObjectId::parse(String::from_utf8_lossy(&output.stdout).trim())
}

/// A copy of a checkout's index with every file in its working tree added, so a diff against
/// it shows untracked files too. The checkout's own index is left alone. Adding writes blobs
/// for the files into the repository, as `git add` does; git cleans them up in time if
/// nothing refers to them.
struct WorkingTreeIndex {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl WorkingTreeIndex {
    fn new(root: &Path) -> Result<Self> {
        let dir = tempfile::tempdir().context("making a directory for a copy of the index")?;
        let path = dir.path().join("index");
        // Relative to the checkout's top when git gives a relative path.
        let own = run_git(root, ["rev-parse", "--git-path", "index"])?;
        let own = root.join(path_from_bytes(
            String::from_utf8_lossy(&own.stdout)
                .trim()
                .as_bytes()
                .to_vec(),
        )?);
        // Starting from the checkout's index keeps git's record of unchanged files, so it
        // doesn't read them all again. A new repository has none yet.
        if own.exists() {
            fs::copy(&own, &path)
                .with_context(|| format!("copying {} to {}", own.display(), path.display()))?;
        }
        let index = Self { _dir: dir, path };
        run_git_with_env(root, ["add", "--all"], &index.env())?;
        Ok(index)
    }

    fn env(&self) -> [(&str, &OsStr); 1] {
        [("GIT_INDEX_FILE", self.path.as_os_str())]
    }
}
