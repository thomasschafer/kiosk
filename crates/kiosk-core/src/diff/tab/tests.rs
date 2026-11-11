use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use super::*;
use crate::git::test_fixtures::{init_test_repo, run_test_git};
use crate::preferences::DiffLayout;

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
    let tab = DiffTab::new(Some("/nowhere".into()), DiffLayout::Unified);
    assert!(matches!(tab.state, DiffState::Unstarted));
}

#[test]
fn a_folder_outside_git_or_no_folder_has_nothing_to_diff() {
    let temp = tempfile::tempdir().unwrap();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, |tab| {
        !matches!(tab.state, DiffState::Resolving(_))
    });
    assert!(
        matches!(&tab.state, DiffState::NoTarget(reason) if reason.contains("not in a git repository"))
    );
    let mut tab = DiffTab::new(None, DiffLayout::Unified);
    tab.start();
    assert!(matches!(tab.state, DiffState::NoTarget(_)));
}

#[test]
fn the_tab_shows_everything_first_then_switches_range_on_request() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
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
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
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
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
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
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
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

#[test]
fn moving_between_files_that_all_fit_changes_the_current_file_without_scrolling() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.set_viewport(100);
    tab.start();
    wait(&mut tab, highlighted);
    let at = |tab: &DiffTab| {
        let shown = tab.shown().unwrap();
        (shown.current_file, shown.offset)
    };
    assert_eq!(at(&tab), (0, 0));
    tab.act(DiffAction::NextFile);
    assert_eq!(at(&tab), (1, 0));
    tab.act(DiffAction::NextFile);
    assert_eq!(at(&tab), (1, 0), "the last file stays current");
    tab.act(DiffAction::PreviousFile);
    assert_eq!(at(&tab), (0, 0));
    tab.act(DiffAction::ToggleFile(None));
    assert!(tab.shown().unwrap().files[0].collapsed);
}

#[test]
fn a_missing_final_newline_gets_a_note_after_its_line() {
    let temp = feature_repo();
    fs::write(temp.path().join("b.rs"), "fn b() {}").unwrap();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, highlighted);
    tab.act(DiffAction::SelectRange(1));
    wait(&mut tab, highlighted);
    let shown = tab.shown().unwrap();
    let b = shown
        .files
        .iter()
        .position(|file| file.diff.path() == Path::new("b.rs"))
        .unwrap();
    let rows = shown
        .rows
        .iter()
        .filter(|row| row.file() == b)
        .copied()
        .collect::<Vec<_>>();
    // The old line ended with a newline and the new one doesn't: only the new one says so.
    assert_eq!(
        rows,
        [
            Row::File(b),
            Row::Hunk { file: b, hunk: 0 },
            Row::Line {
                file: b,
                hunk: 0,
                line: 0
            },
            Row::Line {
                file: b,
                hunk: 0,
                line: 1
            },
            Row::Note(b, Note::NoNewline),
        ]
    );
}

#[test]
fn scrolling_sideways_stops_with_some_of_the_widest_line_still_shown() {
    let temp = feature_repo();
    fs::write(
        temp.path().join("wide.rs"),
        format!("{}\n", "x".repeat(100)),
    )
    .unwrap();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, highlighted);
    let column = |tab: &DiffTab| tab.shown().unwrap().column;
    tab.act(DiffAction::ScrollColumns(-8));
    assert_eq!(column(&tab), 0);
    tab.act(DiffAction::ScrollColumns(30));
    assert_eq!(column(&tab), 30);
    tab.act(DiffAction::ScrollColumns(1000));
    assert_eq!(column(&tab), 100 - MIN_COLUMNS_SHOWN);
}

#[test]
fn switching_range_quickly_ends_on_the_last_one_asked_for() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, |tab| matches!(tab.state, DiffState::Ready(_)));
    for _ in 0..5 {
        tab.act(DiffAction::NextRange);
        tab.act(DiffAction::PreviousRange);
    }
    tab.act(DiffAction::SelectRange(2));
    wait(&mut tab, highlighted);
    assert_eq!(ready(&tab).range, 2);
    assert_eq!(paths(&tab), ["b.rs"]);
}

#[test]
fn sideways_scrolling_reaches_the_end_of_a_line_whose_controls_show_as_escapes() {
    let temp = feature_repo();
    // Each escape character shows as `\u{1b}`, six columns.
    fs::write(
        temp.path().join("escapes.txt"),
        format!("{}\n", "\u{1b}".repeat(30)),
    )
    .unwrap();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, highlighted);
    tab.act(DiffAction::ScrollColumns(1000));
    assert_eq!(tab.shown().unwrap().column, 180 - MIN_COLUMNS_SHOWN);
}

#[test]
fn help_opens_filters_as_typed_and_closes() {
    let mut tab = DiffTab::new(None, DiffLayout::Unified);
    let keys = KeysConfig::default();
    tab.show_help(&keys);
    let all = tab.help.as_ref().unwrap().list.filtered.len();
    for character in "next file".chars() {
        tab.help_action(Action::Insert(character));
    }
    let help = tab.help.as_ref().unwrap();
    assert!(help.list.filtered.len() < all);
    let (first, _) = help.list.filtered[0];
    assert_eq!(help.rows[first].command_name, "next_file");
    tab.help_action(Action::Edit(crate::state::TextEdit::Clear));
    assert_eq!(tab.help.as_ref().unwrap().list.filtered.len(), all);
    tab.help_action(Action::CloseOverlay);
    assert!(tab.help.is_none());
}

#[test]
fn switching_layout_keeps_the_line_at_the_top_of_the_screen() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    init_test_repo(dir);
    let original = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    commit(dir, "long.txt", &original);
    let edited = original
        .replace("line 30\n", "line thirty\n")
        .replace("line 5\n", "line five\n");
    fs::write(dir.join("long.txt"), edited).unwrap();
    let mut tab = DiffTab::new(Some(dir.to_path_buf()), DiffLayout::Unified);
    tab.set_viewport(4);
    tab.start();
    wait(&mut tab, highlighted);
    tab.act(DiffAction::SelectRange(1));
    wait(&mut tab, highlighted);
    let top_text = |tab: &DiffTab| {
        let shown = tab.shown().unwrap();
        let FileContent::Text { hunks } = &shown.files[0].diff.content else {
            panic!("binary");
        };
        match shown.rows[shown.offset] {
            Row::Line { hunk, line, .. } => hunks[hunk].lines[line].text.clone(),
            Row::Pair { hunk, old, new, .. } => {
                hunks[hunk].lines[old.or(new).unwrap()].text.clone()
            }
            other => format!("{other:?}"),
        }
    };
    // Scroll into the second hunk, onto a context line before the change.
    let target = tab
        .shown()
        .unwrap()
        .rows
        .iter()
        .position(|row| matches!(row, Row::Hunk { hunk: 1, .. }))
        .unwrap();
    tab.act(DiffAction::Scroll((target + 1).cast_signed()));
    let before = top_text(&tab);
    assert!(before.starts_with("line 2"), "{before}");
    tab.toggle_layout();
    assert_eq!(tab.shown().unwrap().layout, DiffLayout::SideBySide);
    assert_eq!(top_text(&tab), before);
    tab.toggle_layout();
    assert_eq!(top_text(&tab), before);
}

#[test]
fn the_diff_follows_changes_and_keeps_the_persons_place() {
    let temp = feature_repo();
    fs::write(temp.path().join(".gitignore"), "target/\n").unwrap();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, highlighted);
    assert_eq!(paths(&tab), [".gitignore", "a.rs", "b.rs"]);
    tab.act(DiffAction::ToggleFile(Some(1)));
    tab.act(DiffAction::ShowFile(2));
    assert!(tab.shown().unwrap().files[1].collapsed);
    assert_eq!(tab.shown().unwrap().current_file, 2);

    // A new file appears, and the person stays on b.rs with a.rs still collapsed.
    fs::write(temp.path().join("c.rs"), "fn c() {}\n").unwrap();
    wait(&mut tab, |tab| {
        tab.shown().is_some_and(|shown| shown.files.len() == 4) && highlighted(tab)
    });
    let shown = tab.shown().unwrap();
    assert_eq!(paths(&tab), [".gitignore", "a.rs", "b.rs", "c.rs"]);
    assert_eq!(shown.current_file, 2);
    assert!(shown.files[1].collapsed);
    assert_eq!(tab.refresh_error, None);
}

#[test]
fn changes_git_ignores_dont_reload_the_diff() {
    let temp = feature_repo();
    fs::write(temp.path().join(".gitignore"), "target/\n").unwrap();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, highlighted);
    fs::create_dir(temp.path().join("target")).unwrap();
    fs::write(temp.path().join("target/out.o"), "build output").unwrap();
    // The change is seen, checked, and found not to matter: the refresh ends unchanged.
    wait(&mut tab, |tab| tab.refresh.is_some());
    wait(&mut tab, |tab| {
        tab.refresh.is_none() && tab.changes.is_none()
    });
    assert_eq!(paths(&tab), [".gitignore", "a.rs", "b.rs"]);
}

#[test]
fn a_commit_is_followed_with_the_range_showing_kept() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, highlighted);
    assert_eq!(ready(&tab).ranges.len(), 4);
    run_test_git(temp.path(), &["commit", "-q", "-am", "more"]);
    wait(&mut tab, |tab| {
        matches!(&tab.state, DiffState::Ready(ready) if ready.ranges.len() == 5) && highlighted(tab)
    });
    assert_eq!(ready(&tab).ranges[ready(&tab).range], Range::All);
}

/// The text of the line at the top of the screen, or what row is there instead.
fn top_text(tab: &DiffTab) -> String {
    let shown = tab.shown().unwrap();
    let row = shown.rows[shown.offset];
    let FileContent::Text { hunks } = &shown.files[row.file()].diff.content else {
        return format!("{row:?}");
    };
    match row {
        Row::Line { hunk, line, .. } => hunks[hunk].lines[line].text.clone(),
        Row::Pair { hunk, old, new, .. } => hunks[hunk].lines[new.or(old).unwrap()].text.clone(),
        other => format!("{other:?}"),
    }
}

#[test]
fn a_refresh_keeps_the_line_at_the_top_when_an_earlier_hunk_appears() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    init_test_repo(dir);
    let original = (1..=60)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    commit(dir, "long.txt", &original);
    fs::write(
        dir.join("long.txt"),
        original.replace("line 50\n", "line fifty\n"),
    )
    .unwrap();
    let mut tab = DiffTab::new(Some(dir.to_path_buf()), DiffLayout::Unified);
    tab.set_viewport(3);
    tab.start();
    wait(&mut tab, highlighted);
    tab.act(DiffAction::SelectRange(1));
    wait(&mut tab, highlighted);
    tab.act(DiffAction::Scroll(4));
    let before = top_text(&tab);
    assert!(before.starts_with("line 4"), "{before}");
    // An edit far above adds a hunk before the one showing.
    fs::write(
        dir.join("long.txt"),
        original
            .replace("line 50\n", "line fifty\n")
            .replace("line 3\n", "line three\n"),
    )
    .unwrap();
    wait(&mut tab, |tab| {
        matches!(&tab.state, DiffState::Ready(ready)
            if matches!(&ready.content, RangeContent::Shown(shown)
                if matches!(&shown.files[0].diff.content, FileContent::Text { hunks } if hunks.len() == 2)))
    });
    assert_eq!(top_text(&tab), before);
}

#[test]
fn a_failed_refresh_is_reported_and_the_next_change_tries_again() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, highlighted);
    tab.apply(Event::RefreshFailed("git was busy".into()));
    assert_eq!(
        tab.notice().as_deref(),
        Some("Couldn't read the latest changes: git was busy")
    );
    fs::write(temp.path().join("c.rs"), "fn c() {}\n").unwrap();
    wait(&mut tab, |tab| {
        tab.notice().is_none() && tab.shown().is_some_and(|shown| shown.files.len() == 3)
    });
}

#[test]
fn switching_range_mid_refresh_keeps_what_the_refresh_was_for() {
    let temp = feature_repo();
    let mut tab = DiffTab::new(Some(temp.path().to_path_buf()), DiffLayout::Unified);
    tab.start();
    wait(&mut tab, highlighted);
    let path = temp.path().join("a.rs");
    tab.refresh = Some(Refresh {
        worker: Worker::spawn(|_, _| {}),
        paths: Changed::Paths(vec![path.clone()]),
    });
    tab.act(DiffAction::NextRange);
    assert!(tab.refresh.is_none());
    let Some(Changes {
        paths: Changed::Paths(paths),
        ..
    }) = &tab.changes
    else {
        panic!("{:?}", tab.changes);
    };
    assert_eq!(paths, &[path]);
}

#[test]
fn opening_in_an_editor_goes_to_the_change_being_looked_at() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    init_test_repo(dir);
    let original = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    commit(dir, "long.txt", &original);
    commit(dir, "gone.txt", "bye\n");
    fs::write(
        dir.join("long.txt"),
        original
            .replace("line 5\n", "line five\n")
            .replace("line 30\n", "line thirty\n"),
    )
    .unwrap();
    fs::remove_file(dir.join("gone.txt")).unwrap();
    let mut tab = DiffTab::new(Some(dir.to_path_buf()), DiffLayout::Unified);
    tab.set_viewport(8);
    tab.start();
    wait(&mut tab, highlighted);
    tab.act(DiffAction::SelectRange(1));
    wait(&mut tab, highlighted);
    assert_eq!(paths(&tab), ["gone.txt", "long.txt"]);

    let error = tab.place().unwrap_err();
    assert_eq!(error, "gone.txt was deleted");

    tab.act(DiffAction::NextFile);
    let place = tab.place().unwrap();
    assert_eq!(place.file, place.checkout.join("long.txt"));
    assert_eq!(
        place.line, 5,
        "the first change on screen, though context is above it"
    );

    let second_hunk = tab
        .shown()
        .unwrap()
        .rows
        .iter()
        .position(|row| matches!(row, Row::Hunk { file: 1, hunk: 1 }))
        .unwrap();
    let offset = tab.shown().unwrap().offset;
    tab.act(DiffAction::Scroll((second_hunk + 1 - offset).cast_signed()));
    assert!(top_text(&tab).starts_with("line 2"), "{}", top_text(&tab));
    assert_eq!(tab.place().unwrap().line, 30);
    tab.toggle_layout();
    assert_eq!(tab.shown().unwrap().layout, DiffLayout::SideBySide);
    assert_eq!(tab.place().unwrap().line, 30);

    fs::remove_file(dir.join("long.txt")).unwrap();
    assert_eq!(
        tab.place().unwrap_err(),
        "long.txt is no longer in the checkout"
    );
}

#[test]
fn an_error_opening_the_editor_shows_until_the_person_moves_on() {
    let mut tab = DiffTab::new(None, DiffLayout::Unified);
    assert_eq!(tab.place().unwrap_err(), "there's no file showing");
    tab.show_open_error("no editor".into());
    assert_eq!(
        tab.notice().as_deref(),
        Some("Couldn't open the file: no editor")
    );
    tab.act(DiffAction::Scroll(1));
    assert_eq!(tab.notice(), None);
}

fn hunk(new_start: u32, new_len: u32, kinds: &[LineKind]) -> Hunk {
    let mut old = 1;
    let mut new = new_start;
    let lines = kinds
        .iter()
        .map(|&kind| {
            let (old_number, new_number) = match kind {
                LineKind::Context => (Some(old), Some(new)),
                LineKind::Removed => (Some(old), None),
                LineKind::Added => (None, Some(new)),
            };
            old += u32::from(old_number.is_some());
            new += u32::from(new_number.is_some());
            super::super::Line {
                kind,
                old: old_number,
                new: new_number,
                text: String::new(),
                missing_newline: false,
            }
        })
        .collect();
    Hunk {
        old_start: 1,
        old_len: 0,
        new_start,
        new_len,
        section: String::new(),
        lines,
    }
}

#[test]
fn a_removed_line_opens_at_the_line_now_in_its_place() {
    use LineKind::{Added, Context, Removed};
    let middle = hunk(
        7,
        6,
        &[
            Context, Context, Context, Removed, Context, Context, Context,
        ],
    );
    assert_eq!(new_position(&middle, 2), 9);
    assert_eq!(new_position(&middle, 3), 10);
    assert_eq!(new_position(&middle, 4), 10);
    let replaced = hunk(4, 2, &[Removed, Removed, Added, Added]);
    assert_eq!(new_position(&replaced, 0), 4);
    assert_eq!(new_position(&replaced, 1), 4);
    // Everything after line 12 was removed, and from the top of a file that is now empty.
    assert_eq!(new_position(&hunk(12, 0, &[Removed, Removed]), 1), 13);
    assert_eq!(new_position(&hunk(0, 0, &[Removed]), 0), 1);
}

#[test]
fn searching_goes_through_the_matches_expanding_files_and_follows_changes() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    init_test_repo(dir);
    let original = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    commit(dir, "a.txt", &original);
    let edited = original
        .replace("line 3\n", "needle one\n")
        .replace("line 30\n", "Needle two\n");
    fs::write(dir.join("a.txt"), &edited).unwrap();
    fs::write(dir.join("Cargo.lock"), "needle three\n").unwrap();
    let mut tab = DiffTab::new(Some(dir.to_path_buf()), DiffLayout::Unified);
    tab.set_viewport(6);
    tab.start();
    wait(&mut tab, highlighted);
    tab.act(DiffAction::SelectRange(1));
    wait(&mut tab, highlighted);
    assert_eq!(paths(&tab), ["Cargo.lock", "a.txt"]);
    assert!(tab.shown().unwrap().files[0].collapsed);

    tab.open_search();
    for character in "needle".chars() {
        tab.search_action(Action::Insert(character));
    }
    let current = |tab: &DiffTab| {
        let search = tab.search.as_ref().unwrap();
        let found = &search.found[search.current.unwrap()];
        (found.file, found.line_key(), search.found.len())
    };
    let on_screen = |tab: &DiffTab| {
        let shown = tab.shown().unwrap();
        let search = tab.search.as_ref().unwrap();
        let found = &search.found[search.current.unwrap()];
        shown.rows[shown.offset..(shown.offset + 6).min(shown.rows.len())]
            .iter()
            .any(|row| matches!(*row, Row::Line { file, hunk, line } if (file, hunk, line) == found.line_key()))
    };
    assert_eq!(current(&tab).0, 0);
    assert_eq!(
        current(&tab).2,
        3,
        "case is ignored for a query in lower case"
    );
    assert!(
        !tab.shown().unwrap().files[0].collapsed,
        "a match in a collapsed file expands it"
    );
    assert!(on_screen(&tab));

    tab.search_action(Action::MoveSelection(1));
    assert_eq!(current(&tab).0, 1);
    assert_eq!(tab.shown().unwrap().current_file, 1);
    assert!(on_screen(&tab));
    tab.search_action(Action::MoveSelection(1));
    assert!(on_screen(&tab));
    let last = current(&tab).1;
    tab.search_action(Action::MoveSelection(1));
    assert_eq!(
        current(&tab).0,
        0,
        "moving on from the last match goes to the first"
    );
    tab.search_action(Action::MoveSelection(-1));
    assert_eq!(current(&tab).1, last);

    tab.search_action(Action::Insert('X'));
    assert_eq!(tab.search.as_ref().unwrap().found, []);
    assert_eq!(tab.search.as_ref().unwrap().current, None);
    tab.search_action(Action::Edit(crate::state::TextEdit::Backspace));
    assert_eq!(current(&tab).2, 3);

    fs::write(
        dir.join("a.txt"),
        edited.replace("line 20\n", "needle four\n"),
    )
    .unwrap();
    wait(&mut tab, |tab| {
        tab.search
            .as_ref()
            .is_some_and(|search| search.found.len() == 4)
    });

    tab.search_action(Action::CloseOverlay);
    assert!(tab.search.is_none());
}

#[test]
fn a_match_left_of_the_screen_is_scrolled_back_to_and_a_paste_searches_once() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    init_test_repo(dir);
    commit(dir, "a.txt", "start\n");
    fs::write(
        dir.join("a.txt"),
        format!("\tfind me {}\n", "x".repeat(200)),
    )
    .unwrap();
    let mut tab = DiffTab::new(Some(dir.to_path_buf()), DiffLayout::Unified);
    tab.set_viewport(10);
    tab.start();
    wait(&mut tab, highlighted);
    tab.act(DiffAction::ScrollColumns(100));
    assert_eq!(tab.shown().unwrap().column, 100);

    tab.open_search();
    tab.search_paste("me\u{7}");
    let search = tab.search.as_ref().unwrap();
    assert_eq!(search.input.text, "me", "control characters aren't typed");
    assert_eq!(search.current, Some(0));
    // `me` starts at column 9, after the tab's four columns and `find `.
    assert_eq!(tab.shown().unwrap().column, 1);
}
