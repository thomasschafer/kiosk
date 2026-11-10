//! Noticing when a checkout's changes change, so the diff tab can show them again.
//!
//! The working tree is watched for files, and git's own directories for what moves the
//! commits or the index: `HEAD`, the index and refs. Paths git ignores, such as build output,
//! and the rest of git's directories, such as the objects kiosk's own diffs write, never
//! count.

use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
};

use anyhow::{Context, Result, bail};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::git::{path_from_bytes, run_git};

/// Files in a git directory whose change can move the commits, the branch or the index.
const GIT_FILES: &[&str] = &["HEAD", "index", "packed-refs", "ORIG_HEAD", "MERGE_HEAD"];

/// Where a checkout's files and git's own are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// The top of the working tree.
    root: PathBuf,
    /// This checkout's git directory, which holds its `HEAD` and index.
    git_dir: PathBuf,
    /// The repository's shared git directory, which holds the refs; the same as `git_dir`
    /// except in a linked worktree.
    common_dir: PathBuf,
}

impl Places {
    pub fn of(root: &Path) -> Result<Self> {
        let path = |args: &[&str]| -> Result<PathBuf> {
            let mut output = run_git(root, args.iter().copied())?.stdout;
            if output.pop() != Some(b'\n') {
                bail!("git printed a path without a newline after it");
            }
            let path = root.join(path_from_bytes(output)?);
            path.canonicalize()
                .with_context(|| format!("finding {}", path.display()))
        };
        Ok(Self {
            root: root
                .canonicalize()
                .with_context(|| format!("finding {}", root.display()))?,
            git_dir: path(&["rev-parse", "--absolute-git-dir"])?,
            common_dir: path(&["rev-parse", "--git-common-dir"])?,
        })
    }

    /// Whether any of `paths` could change what the diff shows: a file git doesn't ignore in
    /// the working tree, or one of git's files that moves the commits or the index.
    pub fn any_relevant(&self, paths: &[PathBuf]) -> Result<bool> {
        let mut working_tree = Vec::new();
        for path in paths {
            if let Some(git_path) = [&self.git_dir, &self.common_dir]
                .iter()
                .find_map(|dir| path.strip_prefix(dir).ok())
            {
                let moves_history = git_path.starts_with("refs")
                    || git_path
                        .to_str()
                        .is_some_and(|name| GIT_FILES.contains(&name));
                if moves_history {
                    return Ok(true);
                }
            } else if let Ok(relative) = path.strip_prefix(&self.root)
                && !relative.as_os_str().is_empty()
            {
                working_tree.push(relative.to_path_buf());
            }
        }
        if working_tree.is_empty() {
            return Ok(false);
        }
        let ignored = self.ignored(&working_tree)?;
        Ok(working_tree.iter().any(|path| !ignored.contains(path)))
    }

    /// Which of `paths`, relative to the root, git ignores.
    fn ignored(&self, paths: &[PathBuf]) -> Result<HashSet<PathBuf>> {
        let mut child = Command::new("git")
            .args(["check-ignore", "-z", "--stdin"])
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("running git check-ignore")?;
        {
            let mut stdin = child.stdin.take().expect("stdin is piped");
            for path in paths {
                stdin.write_all(path.as_os_str().as_encoded_bytes())?;
                stdin.write_all(&[0])?;
            }
        }
        let output = child.wait_with_output()?;
        // It exits 1 when it ignores none of them.
        if !output.status.success() && output.status.code() != Some(1) {
            bail!(
                "git check-ignore failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| path_from_bytes(path.to_vec()))
            .collect()
    }
}

/// A checkout watched for changes, until dropped.
pub struct Watch {
    places: Places,
    events: mpsc::Receiver<notify::Result<notify::Event>>,
    _watcher: RecommendedWatcher,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Watch")
            .field("places", &self.places)
            .finish_non_exhaustive()
    }
}

impl Watch {
    pub fn start(root: &Path) -> Result<Self> {
        let places = Places::of(root)?;
        let (sender, events) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender).context("starting a file watcher")?;
        let mut watch = |path: &Path, mode| {
            watcher
                .watch(path, mode)
                .with_context(|| format!("watching {}", path.display()))
        };
        watch(&places.root, RecursiveMode::Recursive)?;
        // A linked worktree's git directories are outside its working tree.
        if !places.git_dir.starts_with(&places.root) {
            watch(&places.git_dir, RecursiveMode::NonRecursive)?;
        }
        if !places.common_dir.starts_with(&places.root) {
            watch(&places.common_dir, RecursiveMode::NonRecursive)?;
            let refs = places.common_dir.join("refs");
            if refs.is_dir() {
                watch(&refs, RecursiveMode::Recursive)?;
            }
        }
        Ok(Self {
            places,
            events,
            _watcher: watcher,
        })
    }

    pub const fn places(&self) -> &Places {
        &self.places
    }

    /// The paths that changed since last asked. A watcher error is returned as one, as the
    /// tab can no longer trust it to notice changes.
    pub fn changed(&self) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for event in self.events.try_iter() {
            paths.extend(event.context("watching for changes")?.paths);
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::git::test_fixtures::{init_test_repo, run_test_git};

    fn repo() -> (tempfile::TempDir, Places) {
        let temp = tempfile::tempdir().unwrap();
        init_test_repo(temp.path());
        fs::write(temp.path().join(".gitignore"), "target/\n").unwrap();
        let places = Places::of(temp.path()).unwrap();
        (temp, places)
    }

    #[test]
    fn ignored_files_and_git_objects_dont_count_and_others_do() {
        let (_temp, places) = repo();
        let root = places.root.clone();
        let relevant = |paths: &[PathBuf]| places.any_relevant(paths).unwrap();
        assert!(!relevant(&[]));
        assert!(!relevant(&[root.join("target/debug/build.o")]));
        assert!(!relevant(&[places.git_dir.join("objects/ab/cdef")]));
        assert!(relevant(&[root.join("target/x"), root.join("src/main.rs")]));
        assert!(relevant(&[places.git_dir.join("index")]));
        assert!(relevant(&[places.git_dir.join("HEAD")]));
        assert!(relevant(&[places.common_dir.join("refs/heads/main")]));
    }

    #[test]
    fn a_linked_worktree_finds_its_own_git_directory_and_the_shared_one() {
        let (temp, places) = repo();
        let linked = temp.path().join("linked");
        run_test_git(
            temp.path(),
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                linked.to_str().unwrap(),
            ],
        );
        let linked = Places::of(&linked).unwrap();
        assert_ne!(linked.git_dir, linked.common_dir);
        assert_eq!(linked.common_dir, places.git_dir);
        assert!(linked.any_relevant(&[linked.git_dir.join("HEAD")]).unwrap());
    }

    #[test]
    fn writing_a_file_is_noticed() {
        let (temp, _) = repo();
        let watch = Watch::start(temp.path()).unwrap();
        fs::write(temp.path().join("new.txt"), "new\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = Vec::new();
        while !seen.iter().any(|path: &PathBuf| path.ends_with("new.txt")) {
            assert!(std::time::Instant::now() < deadline, "saw only {seen:?}");
            seen.extend(watch.changed().unwrap());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(watch.places().any_relevant(&seen).unwrap());
    }
}
