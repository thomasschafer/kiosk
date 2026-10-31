//! The picker driven in a real tmux server. tmux does not run on Windows.
#![cfg(unix)]

mod support;

use kiosk_core::git::test_fixtures::run_test_git;
use support::{HARNESS, TestEnv, wait_for};

#[test]
fn repos_are_listed_and_filtered() {
    let env = TestEnv::new("");
    env.repo("alpha");
    env.repo("beta");
    env.start_picker(&env.path("projects"));
    env.wait_for_screen("both repos", |screen| {
        screen.contains("alpha") && screen.contains("beta")
    });
    env.type_text("bet");
    env.wait_for_screen("only beta", |screen| {
        screen.contains("1 of 2 repos") && screen.contains("beta") && !screen.contains("alpha")
    });
}

/// Restores a directory's permissions so the test's temporary directory can be removed.
struct RestorePermissions(std::path::PathBuf);

impl Drop for RestorePermissions {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn scan_warning_details_list_each_unreadable_directory() {
    use std::os::unix::fs::PermissionsExt;

    let env = TestEnv::new("");
    env.repo("alpha");
    let locked = env.path("projects/locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o0)).unwrap();
    let _restore = RestorePermissions(locked.clone());
    assert!(
        std::fs::read_dir(&locked).is_err(),
        "this test needs a user that unreadable directories block, not root"
    );

    env.start_picker(&env.path("projects"));
    env.wait_for_screen("the scan warning", |screen| {
        screen.contains("1 directory could not be scanned")
            && screen.contains("ctrl+e details / ctrl+x dismiss")
    });
    env.keys(&["C-e"]);
    let screen = env.wait_for_screen("the details view", |screen| {
        screen.contains("failed to inspect .git entry")
    });
    assert!(
        screen.contains(&locked.join(".git").display().to_string()),
        "{screen}"
    );
    env.keys(&["Escape"]);
    env.wait_for_screen("the picker again", |screen| {
        screen.contains("alpha") && !screen.contains("failed to inspect .git entry")
    });
}

#[test]
fn opening_a_repo_makes_a_tagged_session_and_switches_to_it_once() {
    let env = TestEnv::new("");
    let alpha = env.repo("alpha");
    env.start_picker(&env.path("projects"));
    env.wait_for_screen("alpha", |screen| screen.contains("alpha"));
    env.keys(&["Enter"]);
    wait_for("the client to switch", || env.client_session() == "alpha");
    let alpha_session = env
        .sessions()
        .into_iter()
        .find(|(_, name, _)| name == "alpha")
        .unwrap();
    assert_eq!(alpha_session.2, alpha.display().to_string());

    // A second open focuses the session rather than making another.
    env.tmux_ok(&["switch-client", "-t", HARNESS]);
    env.restart_picker(&env.path("projects"));
    let screen = env.wait_for_screen("alpha marked open", |screen| screen.contains("● open"));
    assert!(screen.contains("alpha"), "{screen}");
    env.keys(&["Enter"]);
    wait_for("the client to switch back", || {
        env.client_session() == "alpha"
    });
    assert_eq!(env.sessions().len(), 2);
}

#[test]
fn opening_a_branch_makes_its_worktree_in_the_configured_directory() {
    let env = TestEnv::new("");
    let alpha = env.repo("alpha");
    run_test_git(&alpha, &["branch", "feat/login"]);
    env.start_picker(&alpha);
    env.wait_for_screen("alpha", |screen| screen.contains("alpha"));
    env.keys(&["Tab"]);
    env.wait_for_screen("the branch view", |screen| screen.contains("feat/login"));
    env.type_text("login");
    env.keys(&["Enter"]);
    wait_for("the client to switch", || {
        env.client_session() == "alpha/feat-login"
    });
    let checkout = env.path("trees/alpha/feat-login");
    assert!(checkout.join(".git").exists());
    assert!(env.sessions().iter().any(
        |(_, name, path)| name == "alpha/feat-login" && *path == checkout.display().to_string()
    ));
}

#[test]
fn a_new_branch_gets_a_worktree_session_and_on_open_panes() {
    let env = TestEnv::new(
        "[on_open]\npanes = [{ command = \"echo on-open-ran\", direction = \"down\" }]\n",
    );
    let alpha = env.repo("alpha");
    env.start_picker(&alpha);
    env.wait_for_screen("alpha", |screen| screen.contains("alpha"));
    env.keys(&["Tab"]);
    env.wait_for_screen("the branch view", |screen| screen.contains("select branch"));
    env.type_text("fresh");
    env.keys(&["C-o"]);
    env.wait_for_screen("the base picker", |screen| screen.contains("fresh"));
    env.keys(&["Enter"]);
    wait_for("the client to switch", || {
        env.client_session() == "alpha/fresh"
    });
    let panes = env.tmux_ok(&["list-panes", "-t", "alpha/fresh", "-F", "#{pane_id}"]);
    let panes: Vec<&str> = panes.lines().collect();
    assert_eq!(panes.len(), 2);
    wait_for("the on_open command", || {
        env.screen_of(panes[1]).contains("on-open-ran")
    });
    assert!(env.path("trees/alpha/fresh").exists());
}

#[test]
fn a_linked_worktree_outside_the_search_dirs_preselects_its_repo_and_branch() {
    let env = TestEnv::new("");
    env.repo("alpha");
    let beta = env.repo("beta");
    let checkout = env.path("trees/beta/feature");
    run_test_git(
        &beta,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            checkout.to_str().unwrap(),
        ],
    );
    env.start_picker(&checkout);
    env.wait_for_screen("beta preselected", |screen| screen.contains("▸ beta"));
    env.keys(&["Tab"]);
    env.wait_for_screen("the worktree's branch preselected", |screen| {
        screen.lines().any(|line| line.contains("▸ feature"))
    });
}

#[test]
fn deleting_a_worktree_closes_its_session() {
    let env = TestEnv::new("");
    let alpha = env.repo("alpha");
    let checkout = env.path("trees/alpha/doomed");
    run_test_git(
        &alpha,
        &[
            "worktree",
            "add",
            "-b",
            "doomed",
            checkout.to_str().unwrap(),
        ],
    );
    env.start_picker(&alpha);
    env.tmux_ok(&[
        "new-session",
        "-d",
        "-s",
        "doomed-session",
        "-c",
        checkout.to_str().unwrap(),
    ]);
    env.wait_for_screen("alpha", |screen| screen.contains("alpha"));
    env.keys(&["Tab"]);
    env.type_text("doomed");
    env.wait_for_screen("the open worktree", |screen| {
        screen.contains("doomed (worktree)") && screen.contains("● open")
    });
    env.keys(&["C-x"]);
    env.wait_for_screen("the confirmation", |screen| screen.contains("tmux"));
    env.keys(&["Enter"]);
    wait_for("the worktree to go", || !checkout.exists());
    wait_for("the session to close", || {
        env.sessions()
            .iter()
            .all(|(_, name, _)| name != "doomed-session")
    });
}

#[test]
fn the_setup_wizard_runs_without_a_config() {
    let env = TestEnv::new("");
    std::fs::remove_file(env.path("config/config.toml")).unwrap();
    env.start(&env.path("projects"), support::KIOSK);
    env.wait_for_screen("the wizard", |screen| {
        screen.contains("open them in tmux sessions")
    });
    env.keys(&["Escape"]);
}

#[test]
fn outside_tmux_kiosk_needs_no_server_to_list_agents() {
    let env = TestEnv::new("");
    assert_eq!(env.kiosk_ok(&["agents", "--json"]).trim(), "[]");
    let next = env.kiosk(&["next"]);
    assert!(!next.status.success());
    assert!(String::from_utf8_lossy(&next.stderr).contains("must run inside tmux"));
}

#[test]
fn a_session_started_in_a_folder_with_a_tab_or_newline_does_not_break_reads() {
    let env = TestEnv::new("");
    let odd = env.path("odd\tfolder\nname\\t");
    std::fs::create_dir_all(&odd).unwrap();
    env.tmux_ok(&[
        "new-session",
        "-d",
        "-s",
        "odd",
        "-c",
        odd.to_str().unwrap(),
    ]);
    let agents = env.kiosk_ok(&["agents", "--json"]);
    assert!(agents.trim_start().starts_with('['), "{agents}");
}
