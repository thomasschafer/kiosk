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

/// Files in a git directory whose change can move the commits, the branch or the index, or
/// what git ignores.
const GIT_FILES: &[&str] = &[
    "HEAD",
    "index",
    "packed-refs",
    "ORIG_HEAD",
    "MERGE_HEAD",
    "info/exclude",
];

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
                    || GIT_FILES.iter().any(|name| git_path == Path::new(name));
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

    /// Which of `paths`, relative to the root, git ignores. A directory's path ends in `/`
    /// to match patterns for directories only.
    fn ignored(&self, paths: &[PathBuf]) -> Result<HashSet<PathBuf>> {
        let mut child = Command::new("git")
            .args(["check-ignore", "-z", "--stdin"])
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("running git check-ignore")?;
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_os_str().as_encoded_bytes());
            input.push(0);
        }
        // Written on a thread of its own while the output is read, so a long list can't fill
        // both pipes with each side waiting on the other.
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let output = child.wait_with_output()?;
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("writing to git check-ignore panicked"))?
            .context("writing to git check-ignore")?;
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
            .map(|path| {
                let path = path.strip_suffix(b"/").unwrap_or(path);
                path_from_bytes(path.to_vec())
            })
            .collect()
    }

    /// The folders directly in the working tree that aren't ignored and aren't git's own.
    fn watched_folders(
        &self,
        candidates: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Vec<PathBuf>> {
        let folders = candidates
            .into_iter()
            .filter(|path| path.is_dir() && !path.is_symlink())
            // The root is canonical, so its folders must be for the paths to compare.
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| *path != self.git_dir)
            .filter_map(|path| path.strip_prefix(&self.root).ok().map(Path::to_path_buf))
            .filter(|relative| relative.components().count() == 1 && relative != Path::new(".git"))
            .collect::<Vec<_>>();
        if folders.is_empty() {
            return Ok(Vec::new());
        }
        let as_folders = folders
            .iter()
            .map(|folder| {
                let mut folder = folder.clone().into_os_string();
                folder.push("/");
                PathBuf::from(folder)
            })
            .collect::<Vec<_>>();
        let ignored = self.ignored(&as_folders)?;
        Ok(folders
            .into_iter()
            .filter(|folder| !ignored.contains(folder))
            .map(|folder| self.root.join(folder))
            .collect())
    }
}

/// A checkout watched for changes, until dropped.
pub struct Watch {
    places: Places,
    events: mpsc::Receiver<notify::Result<notify::Event>>,
    watcher: RecommendedWatcher,
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
    /// Watches the checkout at `root`: its top level and each folder there that git doesn't
    /// ignore, and git's directories. Walking ignored folders such as `target/` or
    /// `node_modules/` would cost time and, on Linux, a watch for each folder in them. This
    /// reads the working tree's top level and runs git, so run it off the session thread.
    pub fn start(root: &Path) -> Result<Self> {
        let places = Places::of(root)?;
        let (sender, events) = mpsc::channel();
        let watcher = notify::recommended_watcher(sender).context("starting a file watcher")?;
        let mut watch = Self {
            places,
            events,
            watcher,
        };
        let places = watch.places.clone();
        watch.add(&places.root, RecursiveMode::NonRecursive)?;
        let top_level = std::fs::read_dir(&places.root)
            .with_context(|| format!("reading {}", places.root.display()))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()?;
        for folder in places.watched_folders(top_level)? {
            watch.add(&folder, RecursiveMode::Recursive)?;
        }
        watch.add(&places.git_dir, RecursiveMode::NonRecursive)?;
        if places.common_dir != places.git_dir {
            watch.add(&places.common_dir, RecursiveMode::NonRecursive)?;
        }
        for folder in ["refs", "info"] {
            let folder = places.common_dir.join(folder);
            if folder.is_dir() {
                watch.add(&folder, RecursiveMode::Recursive)?;
            }
        }
        Ok(watch)
    }

    fn add(&mut self, path: &Path, mode: RecursiveMode) -> Result<()> {
        self.watcher
            .watch(path, mode)
            .with_context(|| format!("watching {}", path.display()))
    }

    pub const fn places(&self) -> &Places {
        &self.places
    }

    /// The paths that changed since last asked, watching any folder made in the top level
    /// that git doesn't ignore. A watcher error is returned as one, as the tab can no longer
    /// trust it to notice changes. Reading a file isn't a change: git reads the index on
    /// every diff.
    pub fn changed(&mut self) -> Result<Vec<PathBuf>> {
        use notify::{
            EventKind,
            event::{AccessKind, AccessMode},
        };
        let mut paths = Vec::new();
        let mut created = Vec::new();
        for event in self.events.try_iter() {
            let event = event.context("watching for changes")?;
            match event.kind {
                EventKind::Access(AccessKind::Close(AccessMode::Write)) => {}
                EventKind::Access(_) => continue,
                EventKind::Create(_) => created.extend(event.paths.iter().cloned()),
                _ => {}
            }
            paths.extend(event.paths);
        }
        let new_folders = created
            .into_iter()
            .filter(|path| path.parent() == Some(self.places.root.as_path()))
            .collect::<Vec<_>>();
        for folder in self.places.watched_folders(new_folders)? {
            self.add(&folder, RecursiveMode::Recursive)?;
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

    /// Collects what `watch` reports until `done` holds of it, failing after a few seconds.
    fn collect_until(watch: &mut Watch, done: impl Fn(&[PathBuf]) -> bool) -> Vec<PathBuf> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = Vec::new();
        while !done(&seen) {
            assert!(std::time::Instant::now() < deadline, "saw only {seen:?}");
            seen.extend(watch.changed().unwrap());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        seen
    }

    #[test]
    fn reading_files_isnt_a_change() {
        let (temp, places) = repo();
        let mut watch = Watch::start(temp.path()).unwrap();
        // Let anything from starting arrive and go.
        std::thread::sleep(std::time::Duration::from_millis(300));
        watch.changed().unwrap();
        fs::read(places.git_dir.join("HEAD")).unwrap();
        fs::read(places.git_dir.join("index")).unwrap();
        fs::read(temp.path().join("README.md")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(watch.changed().unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn ignored_folders_go_unwatched_and_new_folders_are_watched() {
        let (temp, places) = repo();
        fs::create_dir(temp.path().join("target")).unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        let mut watch = Watch::start(temp.path()).unwrap();
        assert_eq!(
            places
                .watched_folders([temp.path().join("target"), temp.path().join("src")])
                .unwrap(),
            [places.root.join("src")]
        );
        fs::create_dir(temp.path().join("docs")).unwrap();
        collect_until(&mut watch, |seen| {
            seen.iter().any(|path| path.ends_with("docs"))
        });
        fs::write(temp.path().join("docs/guide.md"), "guide\n").unwrap();
        collect_until(&mut watch, |seen| {
            seen.iter().any(|path| path.ends_with("guide.md"))
        });
    }

    #[test]
    fn a_long_list_of_paths_is_checked_without_blocking() {
        let (_temp, places) = repo();
        let paths = (0..50_000)
            .map(|n| {
                places
                    .root
                    .join(format!("target/very/long/path/to/build/output/{n}.o"))
            })
            .collect::<Vec<_>>();
        assert!(!places.any_relevant(&paths).unwrap());
    }

    #[test]
    fn editing_the_local_exclude_file_counts() {
        let (_temp, places) = repo();
        assert!(
            places
                .any_relevant(&[places.common_dir.join("info/exclude")])
                .unwrap()
        );
    }

    #[test]
    fn writing_a_file_is_noticed() {
        let (temp, _) = repo();
        let mut watch = Watch::start(temp.path()).unwrap();
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
