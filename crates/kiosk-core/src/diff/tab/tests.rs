use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use super::*;
use crate::git::test_fixtures::{init_test_repo, run_test_git};

fn commit(dir: &Path, name: &str, content: &str) {
    fs::write(dir.join(name), content).unwrap();
    run_test_git(dir, &["add", name]);
    run_test_git(dir, &["commit", "-q", "-m", name]);
}

/// A feature branch with two commits on `main`, and an uncommitted change.
fn feature_repo() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    init_test_repo(dir);
    run_test_git(dir, &["branch", "-M", "main"]);
    run_test_git(dir, &["checkout", "-q", "-b", "feat"]);
    commit(dir, "a.rs", "fn a() {}\n");
    commit(dir, "b.rs", "fn b() {}\n");
    fs::write(dir.join("a.rs"), "fn a() {}\nfn more() {}\n").unwrap();
    temp
}

/// Ticks `tab` until `done` holds, failing after a few seconds.
fn wait(tab: &mut DiffTab, done: impl Fn(&DiffTab) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(tab) {
        assert!(
            Instant::now() < deadline,
            "gave up waiting: {:?}",
            tab.state
        );
        tab.tick();
        thread::sleep(Duration::from_millis(5));
    }
}

fn highlighted(tab: &DiffTab) -> bool {
    tab.shown().is_some_and(|shown| {
        shown.worker.is_none() && shown.files.iter().all(|file| file.highlighted.is_some())
    })
}

fn ready(tab: &DiffTab) -> &Ready {
    let DiffState::Ready(ready) = &tab.state else {
        panic!("{:?}", tab.state);
    };
    ready
}

fn paths(tab: &DiffTab) -> Vec<String> {
    tab.shown()
        .unwrap()
        .files
        .iter()
        .map(|file| file.diff.path().display().to_string())
        .collect()
}

#[test]
fn nothing_is_read_until_the_tab_starts() {
    let tab = DiffTab::new(Some("/nowhere".into()));
    assert!(matches!(tab.state, DiffState::Unstarted));
}

#[test]
fn a_folder_outside_git_or_no_folder_has_nothing_to_diff() {
    let temp = tempfile::tempdir().unwrap();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()));
    tab.start();
    wait(&mut tab, |tab| {
        !matches!(tab.state, DiffState::Resolving(_))
    });
    assert!(
        matches!(&tab.state, DiffState::NoTarget(reason) if reason.contains("not in a git repository"))
    );
    let mut tab = DiffTab::new(None);
    tab.start();
    assert!(matches!(tab.state, DiffState::NoTarget(_)));
}

#[test]
fn the_tab_shows_everything_first_then_switches_range_on_request() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()));
    tab.start();
    wait(&mut tab, highlighted);
    assert_eq!(ready(&tab).range, 0);
    assert_eq!(ready(&tab).ranges.len(), 4);
    assert_eq!(paths(&tab), ["a.rs", "b.rs"]);
    let a = &tab.shown().unwrap().files[0];
    assert_eq!((a.added, a.removed), (2, 0));

    tab.act(DiffAction::NextRange);
    assert!(matches!(ready(&tab).content, RangeContent::Loading(_)));
    wait(&mut tab, highlighted);
    assert_eq!(ready(&tab).ranges[ready(&tab).range], Range::Uncommitted);
    assert_eq!(paths(&tab), ["a.rs"]);
    assert_eq!(tab.shown().unwrap().files[0].added, 1);

    tab.act(DiffAction::SelectRange(3));
    wait(&mut tab, highlighted);
    assert_eq!(paths(&tab), ["a.rs"]);
    tab.act(DiffAction::NextRange);
    assert_eq!(ready(&tab).range, 3, "the last range stays");
}

#[test]
fn files_lay_out_as_headers_hunks_and_lines_and_lockfiles_start_collapsed() {
    let temp = feature_repo();
    fs::write(temp.path().join("Cargo.lock"), "lock\n".repeat(10)).unwrap();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()));
    tab.start();
    wait(&mut tab, highlighted);
    let shown = tab.shown().unwrap();
    assert_eq!(paths(&tab), ["Cargo.lock", "a.rs", "b.rs"]);
    assert_eq!(
        shown.files[0].collapse_reason,
        Some(CollapseReason::Generated)
    );
    assert_eq!(
        shown.rows[..6],
        [
            Row::File(0),
            Row::Note(0, Note::Collapsed),
            Row::File(1),
            Row::Hunk { file: 1, hunk: 0 },
            Row::Line {
                file: 1,
                hunk: 0,
                line: 0
            },
            Row::Line {
                file: 1,
                hunk: 0,
                line: 1
            },
        ]
    );
    tab.act(DiffAction::ToggleFile(Some(0)));
    let shown = tab.shown().unwrap();
    assert!(!shown.files[0].collapsed);
    assert_eq!(shown.rows[1], Row::Hunk { file: 0, hunk: 0 });
    assert_eq!(shown.offset, 0);
}

#[test]
fn scrolling_stays_in_bounds_and_file_keys_move_between_headers() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()));
    tab.set_viewport(3);
    tab.start();
    wait(&mut tab, highlighted);
    let rows = tab.shown().unwrap().rows.len();
    let offset = |tab: &DiffTab| tab.shown().unwrap().offset;
    tab.act(DiffAction::Scroll(-5));
    assert_eq!(offset(&tab), 0);
    tab.act(DiffAction::Bottom);
    assert_eq!(offset(&tab), rows - 3);
    tab.act(DiffAction::Page(5));
    assert_eq!(offset(&tab), rows - 3);
    tab.act(DiffAction::Top);
    tab.act(DiffAction::NextFile);
    let b_header = tab
        .shown()
        .unwrap()
        .rows
        .iter()
        .position(|row| *row == Row::File(1))
        .unwrap();
    assert_eq!(offset(&tab), b_header.min(rows - 3));
    tab.act(DiffAction::PreviousFile);
    assert_eq!(offset(&tab), 0);
    tab.act(DiffAction::ShowFile(1));
    assert_eq!(offset(&tab), b_header.min(rows - 3));
}

#[test]
fn leaving_a_range_stops_its_worker() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()));
    tab.start();
    wait(&mut tab, |tab| matches!(tab.state, DiffState::Ready(_)));
    let cancelled = match &ready(&tab).content {
        RangeContent::Loading(worker)
        | RangeContent::Shown(Shown {
            worker: Some(worker),
            ..
        }) => Arc::clone(&worker.cancelled),
        other => panic!("{other:?}"),
    };
    tab.act(DiffAction::NextRange);
    assert!(cancelled.load(Ordering::Relaxed));
}
