use std::{fs, path::Path};

use tempfile::TempDir;

use super::*;
use crate::git::test_fixtures::{init_test_repo, run_test_git};

fn git_output(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit_file(dir: &Path, name: &str, content: &str, message: &str) -> ObjectId {
    if let Some(parent) = Path::new(name).parent() {
        fs::create_dir_all(dir.join(parent)).unwrap();
    }
    fs::write(dir.join(name), content).unwrap();
    run_test_git(dir, &["add", name]);
    run_test_git(dir, &["commit", "-q", "-m", message]);
    ObjectId::parse(&git_output(dir, &["rev-parse", "HEAD"])).unwrap()
}

/// A repository on `main`, without a remote.
fn repo() -> (TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("repo");
    fs::create_dir(&dir).unwrap();
    init_test_repo(&dir);
    run_test_git(&dir, &["branch", "-M", "main"]);
    (temp, dir)
}

fn paths(files: &[FileDiff]) -> Vec<String> {
    let mut paths = files
        .iter()
        .map(|file| file.path().display().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn added_lines(file: &FileDiff) -> Vec<String> {
    let FileContent::Text { hunks } = &file.content else {
        panic!("binary");
    };
    hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind == LineKind::Added)
        .map(|line| line.text.clone())
        .collect()
}

#[test]
fn a_folder_outside_any_repository_has_no_target() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(Target::checkout(temp.path()).unwrap(), None);
}

#[test]
fn a_feature_branch_shows_everything_since_main_with_uncommitted_and_untracked_files() {
    let (_temp, dir) = repo();
    run_test_git(&dir, &["checkout", "-q", "-b", "feat"]);
    let first = commit_file(&dir, "src/a.rs", "one\n", "add a");
    let second = commit_file(&dir, "src/b.rs", "two\n", "add b");
    fs::write(dir.join("src/a.rs"), "one\nmore\n").unwrap();
    fs::write(dir.join("untracked.txt"), "new\n").unwrap();
    fs::write(dir.join(".gitignore"), "ignored.log\n").unwrap();
    fs::write(dir.join("ignored.log"), "noise\n").unwrap();

    let target = Target::checkout(&dir.join("src")).unwrap().unwrap();
    assert!(matches!(target.head(), Head::Branch { name, .. } if name == "feat"));
    let base = target.base().unwrap();
    assert_eq!(base.name, "main");
    let commits = target.commits().unwrap();
    assert_eq!(
        commits.iter().map(|commit| &commit.id).collect::<Vec<_>>(),
        [&second, &first]
    );
    assert_eq!(commits[0].summary, "add b");
    assert_eq!(
        target.ranges(&commits),
        [
            Range::All,
            Range::Uncommitted,
            Range::Commit(second.clone()),
            Range::Commit(first.clone()),
        ]
    );

    let all = target.diff(&Range::All).unwrap();
    assert_eq!(
        paths(&all),
        [".gitignore", "src/a.rs", "src/b.rs", "untracked.txt"]
    );
    let a = all
        .iter()
        .find(|file| file.path() == Path::new("src/a.rs"))
        .unwrap();
    assert_eq!(a.change, Change::Added);
    assert_eq!(added_lines(a), ["one", "more"]);

    let uncommitted = target.diff(&Range::Uncommitted).unwrap();
    assert_eq!(
        paths(&uncommitted),
        [".gitignore", "src/a.rs", "untracked.txt"]
    );
    let a = uncommitted
        .iter()
        .find(|file| file.path() == Path::new("src/a.rs"))
        .unwrap();
    assert_eq!(a.change, Change::Modified);
    assert_eq!(added_lines(a), ["more"]);

    let first_only = target.diff(&Range::Commit(first)).unwrap();
    assert_eq!(paths(&first_only), ["src/a.rs"]);
    assert_eq!(added_lines(&first_only[0]), ["one"]);
}

#[test]
fn diffing_the_working_tree_leaves_the_checkouts_own_index_alone() {
    let (_temp, dir) = repo();
    fs::write(dir.join("staged.txt"), "staged\n").unwrap();
    run_test_git(&dir, &["add", "staged.txt"]);
    fs::write(dir.join("untracked.txt"), "untracked\n").unwrap();
    let staged_before = git_output(&dir, &["diff", "--cached", "--name-only"]);

    let target = Target::checkout(&dir).unwrap().unwrap();
    assert_eq!(
        paths(&target.diff(&Range::Uncommitted).unwrap()),
        ["staged.txt", "untracked.txt"]
    );
    assert_eq!(
        git_output(&dir, &["diff", "--cached", "--name-only"]),
        staged_before
    );
    assert_eq!(
        git_output(&dir, &["status", "--porcelain", "--untracked-files=all"]),
        "A  staged.txt\n?? untracked.txt"
    );
}

#[test]
fn the_base_is_the_remotes_default_branch_even_when_local_main_is_stale() {
    let temp = tempfile::tempdir().unwrap();
    let origin = temp.path().join("origin");
    fs::create_dir(&origin).unwrap();
    init_test_repo(&origin);
    run_test_git(&origin, &["branch", "-M", "main"]);
    let clone = temp.path().join("clone");
    run_test_git(
        temp.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    run_test_git(&clone, &["config", "user.email", "test@example.com"]);
    run_test_git(&clone, &["config", "user.name", "Test"]);
    // Someone else's work lands on origin's main, which the local main hasn't pulled.
    commit_file(&origin, "theirs.txt", "theirs\n", "theirs");
    run_test_git(&clone, &["fetch", "-q"]);
    run_test_git(&clone, &["checkout", "-q", "-b", "feat", "origin/main"]);
    commit_file(&clone, "mine.txt", "mine\n", "mine");

    let target = Target::checkout(&clone).unwrap().unwrap();
    let base = target.base().unwrap();
    assert_eq!(base.name, "origin/main");
    assert_eq!(
        base.merge_base.as_str(),
        git_output(&clone, &["rev-parse", "origin/main"])
    );
    assert_eq!(paths(&target.diff(&Range::All).unwrap()), ["mine.txt"]);
}

#[test]
fn on_the_default_branch_everything_since_it_is_only_uncommitted_work() {
    let (_temp, dir) = repo();
    fs::write(dir.join("README.md"), "# changed\n").unwrap();
    let target = Target::checkout(&dir).unwrap().unwrap();
    assert_eq!(target.commits().unwrap(), []);
    assert_eq!(paths(&target.diff(&Range::All).unwrap()), ["README.md"]);
}

#[test]
fn a_branch_without_a_checkout_shows_only_its_commits() {
    let (_temp, dir) = repo();
    run_test_git(&dir, &["checkout", "-q", "-b", "feat"]);
    let id = commit_file(&dir, "feat.txt", "feat\n", "feat");
    run_test_git(&dir, &["checkout", "-q", "main"]);
    fs::write(dir.join("README.md"), "# not feat's\n").unwrap();

    let target = Target::branch(&dir, "feat").unwrap();
    assert!(!target.has_working_tree());
    let commits = target.commits().unwrap();
    assert_eq!(target.ranges(&commits), [Range::All, Range::Commit(id)]);
    assert_eq!(paths(&target.diff(&Range::All).unwrap()), ["feat.txt"]);
    assert!(target.diff(&Range::Uncommitted).is_err());
    assert!(Target::branch(&dir, "nope").is_err());
}

#[test]
fn a_new_repository_diffs_its_files_against_nothing() {
    let temp = tempfile::tempdir().unwrap();
    run_test_git(temp.path(), &["init", "-q", "-b", "main"]);
    fs::write(temp.path().join("first.txt"), "first\n").unwrap();
    let target = Target::checkout(temp.path()).unwrap().unwrap();
    assert_eq!(
        target.head(),
        &Head::Unborn {
            name: "main".into()
        }
    );
    assert_eq!(target.base(), None);
    assert_eq!(target.ranges(&[]), [Range::Uncommitted]);
    let files = target.diff(&Range::Uncommitted).unwrap();
    assert_eq!(paths(&files), ["first.txt"]);
    assert_eq!(files[0].change, Change::Added);
}

#[test]
fn a_detached_head_is_compared_with_main_too() {
    let (_temp, dir) = repo();
    let id = commit_file(&dir, "more.txt", "more\n", "more");
    run_test_git(&dir, &["checkout", "-q", "--detach", "HEAD"]);
    let target = Target::checkout(&dir).unwrap().unwrap();
    assert_eq!(target.head(), &Head::Detached(id.clone()));
    assert_eq!(
        target.head().to_string(),
        format!("detached HEAD at {}", id.short())
    );
    assert_eq!(target.base().unwrap().merge_base, id);
}

#[test]
fn renames_are_found_with_their_old_and_new_paths() {
    let (_temp, dir) = repo();
    commit_file(&dir, "old.txt", "a\nb\nc\nd\ne\n", "old");
    run_test_git(&dir, &["mv", "old.txt", "new name.txt"]);
    let target = Target::checkout(&dir).unwrap().unwrap();
    let files = target.diff(&Range::Uncommitted).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].change, Change::Renamed { similarity: 100 });
    assert_eq!(files[0].old.as_ref().unwrap().path, Path::new("old.txt"));
    assert_eq!(
        files[0].new.as_ref().unwrap().path,
        Path::new("new name.txt")
    );
}

#[test]
fn a_root_commit_is_diffed_against_nothing() {
    let (_temp, dir) = repo();
    let root =
        ObjectId::parse(&git_output(&dir, &["rev-list", "--max-parents=0", "HEAD"])).unwrap();
    let target = Target::checkout(&dir).unwrap().unwrap();
    let files = target.diff(&Range::Commit(root)).unwrap();
    assert_eq!(paths(&files), ["README.md"]);
    assert_eq!(files[0].change, Change::Added);
}
