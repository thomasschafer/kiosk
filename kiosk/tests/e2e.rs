use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};
use kiosk_core::constants::WORKTREE_DIR_NAME;

fn kiosk_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kiosk"))
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_test_repo(dir: &Path) {
    run_git(dir, &["init"]);
    run_git(dir, &["config", "user.email", "test@test.com"]);
    run_git(dir, &["config", "user.name", "Test"]);
    run_git(dir, &["config", "init.defaultBranch", "main"]);
    // Rename the branch to main in case git init used a different default
    let _ = Command::new("git")
        .args(["branch", "-M", "main"])
        .current_dir(dir)
        .output();
    let dummy = dir.join("README.md");
    fs::write(&dummy, "# test").unwrap();
    run_git(dir, &["add", "."]);
    run_git(dir, &["commit", "-m", "init"]);
}

fn tmux_capture(session: &str) -> String {
    let output = Command::new("tmux")
        .args(["capture-pane", "-t", session, "-p"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn tmux_send(session: &str, keys: &str) {
    Command::new("tmux")
        .args(["send-keys", "-t", session, keys])
        .output()
        .unwrap();
}

fn tmux_send_special(session: &str, key: &str) {
    Command::new("tmux")
        .args(["send-keys", "-t", session, key])
        .output()
        .unwrap();
}

fn cleanup_session(name: &str) {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output();
}

fn wait_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

struct TestEnv {
    tmp: tempfile::TempDir,
    config_dir: PathBuf,
    session_name: String,
}

impl TestEnv {
    fn new(test_name: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();

        let session_name = format!("kiosk-e2e-{test_name}");
        cleanup_session(&session_name);

        Self {
            tmp,
            config_dir,
            session_name,
        }
    }

    fn search_dir(&self) -> PathBuf {
        let d = self.tmp.path().join("projects");
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_config(&self, search_dir: &Path) {
        self.write_config_with_extra(search_dir, "");
    }

    fn write_config_with_extra(&self, search_dir: &Path, extra: &str) {
        let kiosk_config_dir = self.config_dir.join("kiosk");
        fs::create_dir_all(&kiosk_config_dir).unwrap();
        let config = format!(
            "search_dirs = [\"{}\"]\n{}",
            search_dir.to_string_lossy(),
            extra
        );
        fs::write(kiosk_config_dir.join("config.toml"), config).unwrap();
    }

    fn launch_kiosk(&self) {
        let binary = kiosk_binary();
        Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                &self.session_name,
                "-x",
                "120",
                "-y",
                "30",
                &format!(
                    "XDG_CONFIG_HOME={} {} ; sleep 2",
                    self.config_dir.to_string_lossy(),
                    binary.to_string_lossy()
                ),
            ])
            .output()
            .unwrap();
        wait_ms(500);
    }

    fn capture(&self) -> String {
        tmux_capture(&self.session_name)
    }

    fn send(&self, keys: &str) {
        tmux_send(&self.session_name, keys);
        wait_ms(300);
    }

    fn send_special(&self, key: &str) {
        tmux_send_special(&self.session_name, key);
        wait_ms(300);
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        cleanup_session(&self.session_name);
    }
}

#[test]
fn test_e2e_repo_list_shows_repos() {
    let env = TestEnv::new("repo-list");
    let search_dir = env.search_dir();

    let repo_a = search_dir.join("alpha-project");
    let repo_b = search_dir.join("beta-project");
    fs::create_dir_all(&repo_a).unwrap();
    fs::create_dir_all(&repo_b).unwrap();
    init_test_repo(&repo_a);
    init_test_repo(&repo_b);

    env.write_config(&search_dir);
    env.launch_kiosk();

    let screen = env.capture();
    assert!(
        screen.contains("alpha-project"),
        "Should show alpha-project: {screen}"
    );
    assert!(
        screen.contains("beta-project"),
        "Should show beta-project: {screen}"
    );
    assert!(
        screen.contains("2 repos"),
        "Should show repo count: {screen}"
    );
}

#[test]
fn test_e2e_fuzzy_search_filters() {
    let env = TestEnv::new("fuzzy");
    let search_dir = env.search_dir();

    let repo_a = search_dir.join("my-cool-project");
    let repo_b = search_dir.join("other-thing");
    fs::create_dir_all(&repo_a).unwrap();
    fs::create_dir_all(&repo_b).unwrap();
    init_test_repo(&repo_a);
    init_test_repo(&repo_b);

    env.write_config(&search_dir);
    env.launch_kiosk();

    env.send("cool");
    let screen = env.capture();
    assert!(
        screen.contains("my-cool-project"),
        "Should show matching repo: {screen}"
    );
    assert!(screen.contains("1 repos"), "Should filter to 1: {screen}");
}

#[test]
fn test_e2e_enter_repo_shows_branches() {
    let env = TestEnv::new("branches");
    let search_dir = env.search_dir();

    let repo = search_dir.join("test-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Add a branch
    run_git(&repo, &["branch", "feat/awesome"]);

    env.write_config(&search_dir);
    env.launch_kiosk();

    env.send_special("Tab");
    let screen = env.capture();
    assert!(
        screen.contains("select branch"),
        "Should be in branch picker: {screen}"
    );
    assert!(screen.contains("main"), "Should show main: {screen}");
    assert!(
        screen.contains("feat/awesome"),
        "Should show feat/awesome: {screen}"
    );
}

#[test]
fn test_e2e_esc_goes_back() {
    let env = TestEnv::new("esc-back");
    let search_dir = env.search_dir();

    let repo = search_dir.join("nav-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter branch picker
    env.send_special("Tab");
    let screen = env.capture();
    assert!(
        screen.contains("select branch"),
        "Should be in branch picker: {screen}"
    );

    // Esc back to repo list
    env.send_special("Escape");
    let screen = env.capture();
    assert!(
        screen.contains("select repo"),
        "Should be back in repo list: {screen}"
    );
}

#[test]
fn test_e2e_new_branch_flow() {
    let env = TestEnv::new("new-branch");
    let search_dir = env.search_dir();

    let repo = search_dir.join("branch-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter repo
    env.send_special("Tab");
    wait_ms(200);

    // Type a new branch name
    env.send("feat/brand-new");
    let screen = env.capture();
    assert!(
        screen.contains("Create branch"),
        "Should show create branch option: {screen}"
    );
}

#[test]
fn test_e2e_worktree_creation() {
    let env = TestEnv::new("worktree");
    let search_dir = env.search_dir();

    let repo = search_dir.join("wt-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Create a branch (but no worktree for it)
    run_git(&repo, &["branch", "feat/wt-test"]);

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter the repo
    env.send_special("Tab");
    wait_ms(300);

    // Search for the branch and select it
    env.send("feat/wt");
    wait_ms(300);
    env.send_special("Enter");
    wait_ms(2000); // Wait for worktree creation + session

    // Verify the worktree was created in .kiosk_worktrees/ with -- separator
    let wt_root = search_dir.join(WORKTREE_DIR_NAME);
    let expected_wt = wt_root.join("wt-repo--feat-wt-test");
    assert!(
        expected_wt.exists(),
        "Worktree should exist at {}: found {:?}",
        expected_wt.display(),
        fs::read_dir(&wt_root).ok().map(|d| d
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect::<Vec<_>>())
    );

    // Verify tmux session was created with the worktree basename
    let session_name = "wt-repo--feat-wt-test";
    let output = Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tmux session '{}' should exist",
        session_name
    );

    // Verify session working directory is the worktree path
    let output = Command::new("tmux")
        .args([
            "display-message",
            "-t",
            session_name,
            "-p",
            "#{pane_current_path}",
        ])
        .output()
        .unwrap();
    let pane_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        PathBuf::from(&pane_path) == expected_wt || pane_path.ends_with("wt-repo--feat-wt-test"),
        "Session dir should be the worktree path, got: {pane_path}"
    );

    // Cleanup the created session
    cleanup_session(session_name);
}

#[test]
fn test_e2e_split_command_creates_split_pane() {
    let env = TestEnv::new("split-pane");
    let search_dir = env.search_dir();

    let repo = search_dir.join("split-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    let extra = r#"
[session]
split_command = "printf KIOSK_SPLIT_OK && exec cat"
"#;
    env.write_config_with_extra(&search_dir, extra);
    env.launch_kiosk();

    // Open the selected repo directly from the repo list.
    env.send_special("Enter");
    wait_ms(1200);

    let session_name = "split-repo";
    let output = Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tmux session '{}' should exist",
        session_name
    );

    let output = Command::new("tmux")
        .args(["list-panes", "-t", session_name, "-F", "#{pane_id}"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "list-panes should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let pane_ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        pane_ids.len(),
        2,
        "Expected 2 panes when split_command is set, got: {:?}",
        pane_ids
    );

    let output = Command::new("tmux")
        .args(["list-panes", "-t", session_name, "-F", "#{pane_current_command}"])
        .output()
        .unwrap();
    let pane_commands: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert!(
        pane_commands.iter().any(|cmd| cmd == "cat"),
        "Expected split pane to run 'cat', commands were: {:?}",
        pane_commands
    );

    let mut captured = String::new();
    for pane_id in &pane_ids {
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", pane_id, "-p"])
            .output()
            .unwrap();
        captured.push_str(&String::from_utf8_lossy(&output.stdout));
        captured.push('\n');
    }
    assert!(
        captured.contains("KIOSK_SPLIT_OK"),
        "Expected split pane output marker in captured panes, got: {captured}"
    );

    cleanup_session(session_name);
}

#[test]
fn test_e2e_ctrl_n_new_branch() {
    let env = TestEnv::new("ctrl-n-new-branch");
    let search_dir = env.search_dir();

    let repo = search_dir.join("ctrl-n-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Create an additional branch so we have more than one
    run_git(&repo, &["branch", "develop"]);

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter the repo
    env.send_special("Tab");
    wait_ms(200);

    // Type a branch name, then press Ctrl+O to trigger new branch flow
    env.send("feat/new-thing");
    wait_ms(200);
    env.send_special("C-o");
    wait_ms(300);

    let screen = env.capture();
    assert!(
        screen.contains("pick base")
            || screen.contains("New branch")
            || screen.contains("base branch"),
        "Should show new branch dialog or base picker: {screen}"
    );
}

#[test]
fn test_e2e_delete_worktree() {
    let env = TestEnv::new("delete-worktree");
    let search_dir = env.search_dir();

    let repo = search_dir.join("delete-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Create a branch and worktree for it
    run_git(&repo, &["branch", "feat/to-delete"]);
    let wt_dir = search_dir
        .join(WORKTREE_DIR_NAME)
        .join("delete-repo--feat-to-delete");
    fs::create_dir_all(&wt_dir).unwrap();
    run_git(
        &repo,
        &[
            "worktree",
            "add",
            &wt_dir.to_string_lossy(),
            "feat/to-delete",
        ],
    );

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter the repo
    env.send_special("Tab");
    wait_ms(500);

    // Navigate to the branch that has the worktree
    env.send("feat/to-delete");
    wait_ms(200);

    // Press Ctrl+X to trigger delete
    env.send_special("C-x");
    wait_ms(500);

    let screen = env.capture();
    assert!(
        screen.contains("Delete") || screen.contains("remove") || screen.contains("confirm"),
        "Should show confirmation dialog: {screen}"
    );

    // Press 'y' to confirm deletion
    env.send("y");
    wait_ms(500);

    // Just verify that we're back to the branch listing (the confirmation was processed)
    let screen = env.capture();
    assert!(
        screen.contains("select branch") || !screen.contains("Confirm Delete"),
        "Should return to branch listing after deletion: {screen}"
    );
}

#[test]
fn test_e2e_clean_dry_run() {
    let env = TestEnv::new("clean-dry-run");
    let search_dir = env.search_dir();

    // Create .kiosk_worktrees directory
    let worktrees_dir = search_dir.join(WORKTREE_DIR_NAME);
    fs::create_dir_all(&worktrees_dir).unwrap();

    // Create a fake/broken worktree directory (just a regular dir, no valid .git file)
    let fake_worktree = worktrees_dir.join("fake-repo--broken-branch");
    fs::create_dir_all(&fake_worktree).unwrap();
    fs::write(fake_worktree.join("some_file.txt"), "fake content").unwrap();

    env.write_config(&search_dir);

    // Run `kiosk clean --dry-run` with the config via environment variable
    let output = Command::new(kiosk_binary())
        .args(["clean", "--dry-run"])
        .env("XDG_CONFIG_HOME", &env.config_dir)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Verify the command succeeded
    assert!(
        output.status.success(),
        "clean --dry-run should succeed. stdout: {stdout}, stderr: {stderr}"
    );

    // Verify the output lists the orphaned worktree
    assert!(
        stdout.contains("fake-repo--broken-branch") || stderr.contains("fake-repo--broken-branch"),
        "Should list the orphaned worktree. stdout: {stdout}, stderr: {stderr}"
    );

    // Verify the directory still exists (dry-run shouldn't remove it)
    assert!(
        fake_worktree.exists(),
        "Dry-run should not remove the directory: {}",
        fake_worktree.display()
    );
}

#[test]
fn test_e2e_custom_keybindings() {
    let env = TestEnv::new("custom-keys");
    let search_dir = env.search_dir();

    let repo = search_dir.join("keys-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Remap Tab (enter repo) to F1
    let extra = r#"
[keys.repo_select]
F1 = "enter_repo"
tab = "noop"
"#;
    env.write_config_with_extra(&search_dir, extra);
    env.launch_kiosk();

    // Tab should NOT enter the repo (unbound)
    env.send_special("Tab");
    let screen = env.capture();
    assert!(
        screen.contains("select repo"),
        "Tab should be unbound, still on repo list: {screen}"
    );

    // F1 should enter the repo
    env.send_special("F1");
    let screen = env.capture();
    assert!(
        screen.contains("select branch"),
        "F1 should enter repo (custom binding): {screen}"
    );
}

#[test]
fn test_e2e_help_esc_dismiss() {
    let env = TestEnv::new("help-esc");
    let search_dir = env.search_dir();

    let repo = search_dir.join("help-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Open help
    env.send_special("C-h");
    let screen = env.capture();
    assert!(
        screen.contains("Help") && screen.contains("Keybindings"),
        "Help overlay should be visible: {screen}"
    );

    // Dismiss with Esc
    env.send_special("Escape");
    let screen = env.capture();
    assert!(
        screen.contains("select repo") && !screen.contains("Keybindings"),
        "Help should be dismissed by Esc: {screen}"
    );
}

#[test]
fn test_e2e_delete_no_worktree_error() {
    let env = TestEnv::new("delete-no-wt");
    let search_dir = env.search_dir();

    let repo = search_dir.join("no-wt-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Create a branch without a worktree
    run_git(&repo, &["branch", "feat/no-worktree"]);

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter the repo
    env.send_special("Tab");
    wait_ms(300);

    // Navigate to the branch without a worktree
    env.send("feat/no-worktree");
    wait_ms(200);

    // Try to delete
    env.send_special("C-x");
    let screen = env.capture();
    assert!(
        screen.contains("No worktree"),
        "Should show error for branch without worktree: {screen}"
    );
}

#[test]
fn test_e2e_empty_branch_name_error() {
    let env = TestEnv::new("empty-branch");
    let search_dir = env.search_dir();

    let repo = search_dir.join("empty-name-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter the repo
    env.send_special("Tab");
    wait_ms(300);

    // Try new branch with empty search
    env.send_special("C-o");
    let screen = env.capture();
    assert!(
        screen.contains("branch name"),
        "Should show error for empty branch name: {screen}"
    );

    // Should still be in branch select, not new branch base
    assert!(
        screen.contains("select branch"),
        "Should stay in branch select mode: {screen}"
    );
}

#[test]
fn test_e2e_dynamic_hints() {
    let env = TestEnv::new("dynamic-hints");
    let search_dir = env.search_dir();

    let repo = search_dir.join("hints-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Check repo hints show actual keybindings
    let screen = env.capture();
    assert!(
        screen.contains("enter: open") && screen.contains("tab: branches"),
        "Repo hints should show dynamic bindings: {screen}"
    );

    // Enter branch view and check hints
    env.send_special("Tab");
    let screen = env.capture();
    assert!(
        screen.contains("C-o: new branch") && screen.contains("C-x: delete worktree"),
        "Branch hints should show dynamic bindings: {screen}"
    );
}

#[test]
fn test_e2e_delete_confirm_dialog_text() {
    let env = TestEnv::new("delete-dialog-text");
    let search_dir = env.search_dir();

    let repo = search_dir.join("dialog-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Create a branch and worktree
    run_git(&repo, &["branch", "feat/dialog-test"]);
    let wt_dir = search_dir
        .join(WORKTREE_DIR_NAME)
        .join("dialog-repo--feat-dialog-test");
    fs::create_dir_all(&wt_dir).unwrap();
    run_git(
        &repo,
        &[
            "worktree",
            "add",
            &wt_dir.to_string_lossy(),
            "feat/dialog-test",
        ],
    );

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter the repo
    env.send_special("Tab");
    wait_ms(500);

    // Search for and select the branch with worktree
    env.send("feat/dialog-test");
    wait_ms(200);

    // Trigger delete
    env.send_special("C-x");
    wait_ms(500);

    let screen = env.capture();
    // No tmux session, so dialog should say just "Delete worktree"
    assert!(
        screen.contains("Delete worktree for branch"),
        "Should show worktree-only delete text: {screen}"
    );
    assert!(
        !screen.contains("kill tmux session"),
        "Should NOT mention tmux session when none exists: {screen}"
    );
}

#[test]
fn test_e2e_remote_branches_shown() {
    let env = TestEnv::new("remote-branches");
    let search_dir = env.search_dir();

    // Create a "remote" repo with extra branches
    let remote_repo = search_dir.join("remote-origin");
    fs::create_dir_all(&remote_repo).unwrap();
    init_test_repo(&remote_repo);
    run_git(&remote_repo, &["branch", "feature-alpha"]);
    run_git(&remote_repo, &["branch", "feature-beta"]);

    // Clone it to create a repo with a real remote
    let repo = search_dir.join("cloned-repo");
    Command::new("git")
        .args([
            "clone",
            &remote_repo.to_string_lossy(),
            &repo.to_string_lossy(),
        ])
        .output()
        .unwrap();

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter the cloned repo
    env.send("cloned");
    wait_ms(200);
    env.send_special("Tab");
    wait_ms(1500); // Extra time for remote branch loading

    let screen = env.capture();

    // Should show local branch
    assert!(
        screen.contains("main"),
        "Should show local main branch: {screen}"
    );

    // Should show remote branches with (remote) indicator
    assert!(
        screen.contains("feature-alpha") && screen.contains("(remote)"),
        "Should show remote branches with (remote) tag: {screen}"
    );
    assert!(
        screen.contains("feature-beta"),
        "Should show feature-beta remote branch: {screen}"
    );
}

#[test]
fn test_e2e_remote_branches_searchable() {
    let env = TestEnv::new("remote-search");
    let search_dir = env.search_dir();

    let remote_repo = search_dir.join("remote-origin");
    fs::create_dir_all(&remote_repo).unwrap();
    init_test_repo(&remote_repo);
    run_git(&remote_repo, &["branch", "feat-search-target"]);

    let repo = search_dir.join("search-repo");
    Command::new("git")
        .args([
            "clone",
            &remote_repo.to_string_lossy(),
            &repo.to_string_lossy(),
        ])
        .output()
        .unwrap();

    env.write_config(&search_dir);
    env.launch_kiosk();

    env.send("search-repo");
    wait_ms(200);
    env.send_special("Tab");
    wait_ms(1500);

    // Search for the remote branch
    env.send("search-target");
    wait_ms(300);

    let screen = env.capture();
    assert!(
        screen.contains("feat-search-target") && screen.contains("(remote)"),
        "Should find remote branch via search: {screen}"
    );
    // "main" should be filtered out
    assert!(
        !screen.contains("▸ main") && !screen.contains("  main"),
        "Local 'main' should be filtered out by search: {screen}"
    );
}
