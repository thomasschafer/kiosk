use std::path::{Path, PathBuf};

use kiosk_core::{
    backend::{Label, Tone},
    git::{Worktree, mock::MockGitProvider},
};

use super::*;
use crate::{
    config::OnOpenPaneConfig,
    tmux::mock::{MockTmux, PaneLine, listing},
};

fn root() -> PathBuf {
    std::env::temp_dir().join("kiosk-backend-tests")
}

fn path(relative: &str) -> PathBuf {
    root().join(relative)
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

struct Fixture {
    tmux: Arc<MockTmux>,
    backend: TmuxBackend,
}

/// A repository `repo` with a linked worktree at `trees/feat`.
fn fixture(runs: Placement, on_open: OnOpenConfig) -> Fixture {
    let git = MockGitProvider {
        worktrees: vec![
            Worktree {
                path: path("repo"),
                branch: Some("main".into()),
            },
            Worktree {
                path: path("trees/feat"),
                branch: Some("feat".into()),
            },
        ],
        main_checkouts: [
            (path("repo"), path("repo")),
            (path("repo/src"), path("repo")),
            (path("trees/feat"), path("repo")),
        ]
        .into(),
        ..MockGitProvider::default()
    };
    let tmux = Arc::new(MockTmux::default());
    let backend = TmuxBackend::new(
        Arc::clone(&tmux) as Arc<dyn Tmux>,
        runs,
        Arc::new(git),
        WorktreePlacement::new(path("trees")).unwrap(),
        on_open,
        AgentConfig::default(),
    );
    Fixture { tmux, backend }
}

fn pane<'a>(session: &'a str, name: &'a str, started_in: &'a str, tagged: &'a str) -> PaneLine<'a> {
    PaneLine {
        session,
        name,
        started_in,
        tagged,
        pane: "%1",
        command: "vim",
        status: None,
    }
}

#[test]
fn a_session_kiosk_made_wins_over_another_in_the_same_folder() {
    let Fixture { tmux, backend } = fixture(Placement::InsideServer, OnOpenConfig::default());
    let feat = text(&path("trees/feat"));
    tmux.respond(Ok(listing(&[
        pane("$1", "mine", &feat, ""),
        pane("$2", "kiosk-made", "/elsewhere", &feat),
    ])));
    tmux.respond(Ok(String::new()));
    backend
        .open_checkout(&path("repo"), &path("trees/feat"))
        .unwrap();
    assert_eq!(tmux.calls()[1], args!["switch-client", "-t", "$2"]);
}

#[test]
fn sessions_started_inside_a_checkout_show_it_but_not_the_checkout_above() {
    let Fixture { tmux, backend } = fixture(Placement::InsideServer, OnOpenConfig::default());
    tmux.respond(Ok(listing(&[pane(
        "$1",
        "src",
        &text(&path("repo/src")),
        "",
    )])));
    let states = backend.checkouts(&path("repo")).unwrap();
    assert_eq!(
        states,
        [
            CheckoutState {
                path: path("repo"),
                session: Some(SessionId::parse("$1").unwrap()),
            },
            CheckoutState {
                path: path("trees/feat"),
                session: None,
            },
        ]
    );
}

#[test]
fn opening_a_closed_checkout_makes_a_tagged_uniquely_named_session_with_its_panes() {
    let on_open = OnOpenConfig {
        panes: vec![OnOpenPaneConfig {
            command: "hx .".into(),
            direction: OnOpenPaneDirection::Right,
            ratio: Some(0.4),
        }],
    };
    let Fixture { tmux, backend } = fixture(Placement::InsideServer, on_open);
    let feat = text(&path("trees/feat"));
    tmux.respond(Ok(listing(&[pane("$1", "repo/feat", "/other", "")])));
    tmux.respond(Ok("$7 %9\n".into()));
    tmux.respond(Ok(String::new()));
    tmux.respond(Ok("%10\n".into()));
    tmux.respond(Ok(String::new()));
    tmux.respond(Ok(String::new()));
    let opened = backend
        .open_checkout(&path("repo"), &path("trees/feat"))
        .unwrap();
    assert_eq!(opened.warning, None);
    let calls = tmux.calls();
    assert_eq!(
        calls[1],
        args![
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_id} #{pane_id}",
            "-s",
            "repo/feat-2",
            "-c",
            feat.clone()
        ]
    );
    assert_eq!(
        calls[2],
        args!["set-option", "-t", "$7", "@kiosk_path", feat.clone()]
    );
    assert_eq!(
        calls[3],
        args![
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            "%9",
            "-h",
            "-c",
            feat,
            "-l",
            "40%"
        ]
    );
    assert_eq!(
        calls[4],
        args![
            "send-keys",
            "-t",
            "%10",
            "-l",
            "hx .",
            ";",
            "send-keys",
            "-t",
            "%10",
            "Enter"
        ]
    );
    assert_eq!(calls[5], args!["switch-client", "-t", "$7"]);
}

#[test]
fn failed_panes_are_a_warning_and_the_session_still_opens() {
    let on_open = OnOpenConfig {
        panes: vec![OnOpenPaneConfig {
            command: "hx".into(),
            direction: OnOpenPaneDirection::Down,
            ratio: None,
        }],
    };
    let Fixture { tmux, backend } = fixture(Placement::InsideServer, on_open);
    tmux.respond(Err(TmuxError::NoServer));
    tmux.respond(Ok("$0 %0".into()));
    tmux.respond(Ok(String::new()));
    tmux.respond(Err(TmuxError::Failed {
        command: "split-window".into(),
        stderr: "no space for new pane".into(),
    }));
    tmux.respond(Ok(String::new()));
    let opened = backend.open_checkout(&path("repo"), &path("repo")).unwrap();
    assert_eq!(
        opened.warning.as_deref(),
        Some("on_open: pane 1 failed: tmux split-window failed: no space for new pane")
    );
    let calls = tmux.calls();
    assert_eq!(calls[1][6], "repo");
    assert_eq!(calls[4], args!["switch-client", "-t", "$0"]);
}

#[test]
fn outside_tmux_the_session_is_attached_after_the_picker_and_notices_wait() {
    let Fixture { tmux, backend } = fixture(Placement::Outside, OnOpenConfig::default());
    let notes = text(&path("notes"));
    tmux.respond(Ok(listing(&[pane("$3", "notes", &notes, "")])));
    backend.open_folder(&path("notes")).unwrap();
    backend.notify("kiosk", "something # happened").unwrap();
    assert_eq!(tmux.calls().len(), 1);
    assert_eq!(backend.take_attach(), Some(SessionId::parse("$3").unwrap()));
    assert_eq!(backend.take_attach(), None);
    assert_eq!(backend.take_notices(), ["kiosk: something # happened"]);
}

#[test]
fn inside_tmux_notices_are_shown_with_formats_escaped() {
    let Fixture { tmux, backend } = fixture(Placement::InsideServer, OnOpenConfig::default());
    tmux.respond(Ok(String::new()));
    backend.notify("kiosk", "50# done").unwrap();
    assert_eq!(
        tmux.calls(),
        [args!["display-message", "-d", "5000", "kiosk: 50## done"]]
    );
}

#[test]
fn closing_kills_the_session_by_id() {
    let Fixture { tmux, backend } = fixture(Placement::InsideServer, OnOpenConfig::default());
    tmux.respond(Ok(String::new()));
    backend.close(&SessionId::parse("$4").unwrap()).unwrap();
    assert_eq!(tmux.calls(), [args!["kill-session", "-t", "$4"]]);
}

#[test]
fn open_repos_and_folders_come_from_session_places() {
    let Fixture { tmux, backend } = fixture(Placement::InsideServer, OnOpenConfig::default());
    let listed = listing(&[
        pane("$1", "a", &text(&path("repo/src")), ""),
        pane("$2", "b", "/x", &text(&path("trees/feat"))),
        pane("$3", "c", &text(&path("notes")), ""),
    ]);
    tmux.respond(Ok(listed.clone()));
    tmux.respond(Ok(listed));
    assert_eq!(backend.open_repos().unwrap(), [path("repo")]);
    assert_eq!(
        backend.open_folders().unwrap(),
        [path("trees/feat"), path("repo/src"), path("notes")]
    );
}

#[test]
fn agent_labels_go_to_their_checkout_and_repository_rows() {
    let Fixture { tmux, backend } = fixture(Placement::InsideServer, OnOpenConfig::default());
    let feat = text(&path("trees/feat"));
    let src = text(&path("repo/src"));
    let notes = text(&path("notes"));
    tmux.respond(Ok(listing(&[
        PaneLine {
            status: Some("state=working:app=claude-code"),
            ..pane("$1", "feat", "/x", &feat)
        },
        PaneLine {
            status: Some("state=blocked:app=codex"),
            ..pane("$2", "src", &src, "")
        },
        PaneLine {
            status: Some("state=idle"),
            ..pane("$3", "notes", &notes, "")
        },
        pane("$4", "quiet", &feat, ""),
    ])));
    let annotations = backend.annotations().unwrap();
    let label = |text: &str, tone| Label {
        text: text.into(),
        tone,
    };
    let sorted = |mut rows: Vec<(PathBuf, Vec<Label>)>| {
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        rows
    };
    assert_eq!(
        sorted(annotations.repos),
        sorted(vec![
            (
                path("repo"),
                vec![
                    label("codex waiting", Tone::Attention),
                    label("claude-code running", Tone::Progress)
                ]
            ),
            (path("notes"), vec![label("idle", Tone::Neutral)]),
        ])
    );
    assert_eq!(
        sorted(annotations.checkouts),
        sorted(vec![
            (
                path("trees/feat"),
                vec![label("claude-code running", Tone::Progress)]
            ),
            (path("repo"), vec![label("codex waiting", Tone::Attention)]),
        ])
    );
}

#[test]
fn session_names_follow_tmux_rules_and_avoid_taken_names() {
    assert_eq!(session_name("my.repo:x"), "my_repo_x");
    assert_eq!(session_name("\u{7}"), "kiosk");
    let sessions = vec![
        Session {
            id: SessionId::parse("$1").unwrap(),
            name: "a".into(),
            activity: 0,
            place: None,
            panes: Vec::new(),
        },
        Session {
            id: SessionId::parse("$2").unwrap(),
            name: "a-2".into(),
            activity: 0,
            place: None,
            panes: Vec::new(),
        },
    ];
    assert_eq!(unique_name(&sessions, "a"), "a-3");
    assert_eq!(unique_name(&sessions, "b"), "b");
}
