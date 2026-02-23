use kiosk_core::constants::WORKTREE_DIR_NAME;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn kiosk_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_kiosk"))
}

static TEST_ID: AtomicU64 = AtomicU64::new(1);

fn unique_id() -> String {
    let pid = std::process::id();
    let ctr = TEST_ID.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{pid}-{ctr}-{ts}")
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

fn tmux_capture(socket: &str, session: &str) -> String {
    let output = Command::new("tmux")
        .args(["-L", socket, "capture-pane", "-t", session, "-p"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn tmux_send(socket: &str, session: &str, keys: &str) {
    Command::new("tmux")
        .args(["-L", socket, "send-keys", "-t", session, keys])
        .output()
        .unwrap();
}

fn tmux_send_special(socket: &str, session: &str, key: &str) {
    Command::new("tmux")
        .args(["-L", socket, "send-keys", "-t", session, key])
        .output()
        .unwrap();
}

fn cleanup_session(socket: &str, name: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-session", "-t", name])
        .output();
}

fn cleanup_server(socket: &str) {
    let _ = Command::new("tmux")
        .args(["-L", socket, "kill-server"])
        .output();
}

fn wait_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

fn wait_for_screen<F>(env: &TestEnv, timeout_ms: u64, mut predicate: F) -> String
where
    F: FnMut(&str) -> bool,
{
    let start = std::time::Instant::now();
    loop {
        let screen = env.capture();
        if predicate(&screen) {
            return screen;
        }
        if start.elapsed() > Duration::from_millis(timeout_ms) {
            return screen;
        }
        wait_ms(100);
    }
}

fn wait_for_tmux_session(socket: Option<&str>, name: &str, timeout_ms: u64) -> bool {
    let start = std::time::Instant::now();
    loop {
        let mut command = Command::new("tmux");
        if let Some(socket) = socket {
            command.args(["-L", socket]);
        }
        let output = command.args(["has-session", "-t", name]).output().unwrap();
        if output.status.success() {
            return true;
        }
        if start.elapsed() > Duration::from_millis(timeout_ms) {
            return false;
        }
        wait_ms(100);
    }
}

fn selected_line(screen: &str) -> Option<String> {
    screen
        .lines()
        .find(|line| line.contains("▸ "))
        .map(|line| line.trim().to_string())
}

struct TestEnv {
    tmp: tempfile::TempDir,
    config_dir: PathBuf,
    state_dir: PathBuf,
    bin_dir: PathBuf,
    session_name: String,
    tmux_socket: String,
}

struct SessionCleanupGuard {
    session: Option<String>,
}

impl Drop for SessionCleanupGuard {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = Command::new("tmux")
                .args(["kill-session", "-t", &session])
                .output();
        }
    }
}

struct BranchCleanupGuard {
    bin: PathBuf,
    config_dir: PathBuf,
    state_dir: PathBuf,
    repo: String,
    branch: String,
    enabled: bool,
}

impl BranchCleanupGuard {
    fn disable(&mut self) {
        self.enabled = false;
    }
}

impl Drop for BranchCleanupGuard {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        let _ = Command::new(&self.bin)
            .args(["delete", &self.repo, &self.branch, "--force", "--json"])
            .env("XDG_CONFIG_HOME", &self.config_dir)
            .env("XDG_STATE_HOME", &self.state_dir)
            .output();
    }
}

impl TestEnv {
    fn new(test_name: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        let state_dir = tmp.path().join("state");
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();

        let id = unique_id();
        let tmux_socket = format!("kiosk-e2e-{id}");
        let session_name = format!("kiosk-e2e-{test_name}-{id}");

        Self {
            tmp,
            config_dir,
            state_dir,
            bin_dir,
            session_name,
            tmux_socket,
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

    fn config_file_path(&self) -> PathBuf {
        self.config_dir.join("kiosk").join("config.toml")
    }

    fn launch_kiosk(&self) {
        cleanup_session(&self.tmux_socket, &self.session_name);
        let binary = kiosk_binary();
        Command::new("tmux")
            .args([
                "-L",
                &self.tmux_socket,
                "new-session",
                "-d",
                "-s",
                &self.session_name,
                "-x",
                "120",
                "-y",
                "30",
                &format!(
                    "XDG_CONFIG_HOME={} XDG_STATE_HOME={} KIOSK_NO_ALT_SCREEN=1 PATH={}:$PATH {} ; sleep 2",
                    self.config_dir.to_string_lossy(),
                    self.state_dir.to_string_lossy(),
                    self.bin_dir.to_string_lossy(),
                    binary.to_string_lossy()
                ),
            ])
            .output()
            .unwrap();
        wait_ms(500);
    }

    fn launch_kiosk_with_config_arg(&self, config_path: &Path, fake_xdg_config_home: &Path) {
        cleanup_session(&self.tmux_socket, &self.session_name);
        let binary = kiosk_binary();
        Command::new("tmux")
            .args([
                "-L",
                &self.tmux_socket,
                "new-session",
                "-d",
                "-s",
                &self.session_name,
                "-x",
                "120",
                "-y",
                "30",
                &format!(
                    "XDG_CONFIG_HOME={} XDG_STATE_HOME={} KIOSK_NO_ALT_SCREEN=1 PATH={}:$PATH {} --config {} ; sleep 2",
                    fake_xdg_config_home.to_string_lossy(),
                    self.state_dir.to_string_lossy(),
                    self.bin_dir.to_string_lossy(),
                    binary.to_string_lossy(),
                    config_path.to_string_lossy()
                ),
            ])
            .output()
            .unwrap();
        wait_ms(500);
    }

    fn install_slow_git_remove_wrapper(&self, sleep_seconds: u8) {
        let output = Command::new("which").arg("git").output().unwrap();
        assert!(output.status.success(), "git should be available in PATH");
        let real_git = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"worktree\" ] && [ \"$2\" = \"remove\" ]; then\n  sleep {sleep_seconds}\nfi\nexec \"{real_git}\" \"$@\"\n",
        );
        let wrapper = self.bin_dir.join("git");
        fs::write(&wrapper, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&wrapper).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&wrapper, perms).unwrap();
        }
    }

    fn capture(&self) -> String {
        tmux_capture(&self.tmux_socket, &self.session_name)
    }

    fn send(&self, keys: &str) {
        tmux_send(&self.tmux_socket, &self.session_name, keys);
        wait_ms(300);
    }

    fn send_special(&self, key: &str) {
        tmux_send_special(&self.tmux_socket, &self.session_name, key);
        wait_ms(300);
    }

    fn run_cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(kiosk_binary())
            .args(args)
            .env("XDG_CONFIG_HOME", &self.config_dir)
            .env("XDG_STATE_HOME", &self.state_dir)
            .output()
            .unwrap()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        cleanup_server(&self.tmux_socket);
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
    wait_for_screen(&env, 2500, |s| s.contains("select branch"));

    // Search for the branch and select it
    env.send("feat/wt");
    wait_ms(300);
    env.send_special("Enter");

    // Verify the worktree was created in .kiosk_worktrees/ with -- separator
    let wt_root = search_dir.join(WORKTREE_DIR_NAME);
    let expected_wt = wt_root.join("wt-repo--feat-wt-test");
    assert!(
        expected_wt.exists(),
        "Worktree should exist at {}: found {:?}",
        expected_wt.display(),
        fs::read_dir(&wt_root).ok().map(|d| d
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect::<Vec<_>>())
    );

    // Verify tmux session was created with the worktree basename
    let session_name = "wt-repo--feat-wt-test";
    assert!(
        wait_for_tmux_session(Some(&env.tmux_socket), session_name, 4000),
        "tmux session '{session_name}' should exist"
    );

    // Verify session working directory is the worktree path
    let output = Command::new("tmux")
        .args([
            "-L",
            &env.tmux_socket,
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
        Path::new(&pane_path) == expected_wt.as_path()
            || pane_path.ends_with("wt-repo--feat-wt-test"),
        "Session dir should be the worktree path, got: {pane_path}"
    );

    // Cleanup the created session
    cleanup_session(&env.tmux_socket, session_name);
}

#[test]
fn test_e2e_split_command_creates_split_pane_for_new_branch_flow() {
    let env = TestEnv::new("split-pane-new-branch");
    let search_dir = env.search_dir();
    let session_name = "split-repo--feat-split-test";
    cleanup_session(&env.tmux_socket, session_name);

    let repo = search_dir.join("split-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    let extra = r#"
[session]
split_command = "sleep 30"
"#;
    env.write_config_with_extra(&search_dir, extra);
    env.launch_kiosk();

    // Enter branch picker, type a new branch name, and create it from the default base.
    env.send_special("Tab");
    wait_for_screen(&env, 2500, |s| s.contains("select branch"));
    env.send("feat/split-test");
    wait_ms(300);
    env.send_special("C-o");
    wait_ms(400);
    env.send_special("Enter");

    assert!(
        wait_for_tmux_session(Some(&env.tmux_socket), session_name, 5000),
        "tmux session '{session_name}' should exist"
    );

    let output = Command::new("tmux")
        .args([
            "-L",
            &env.tmux_socket,
            "list-panes",
            "-t",
            session_name,
            "-F",
            "#{pane_id}",
        ])
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
        "Expected 2 panes when split_command is set, got: {pane_ids:?}",
    );

    let mut captured = String::new();
    for pane_id in &pane_ids {
        let output = Command::new("tmux")
            .args(["-L", &env.tmux_socket, "capture-pane", "-t", pane_id, "-p"])
            .output()
            .unwrap();
        captured.push_str(&String::from_utf8_lossy(&output.stdout));
        captured.push('\n');
    }

    let output = Command::new("tmux")
        .args([
            "-L",
            &env.tmux_socket,
            "list-panes",
            "-t",
            session_name,
            "-F",
            "#{pane_current_command}",
        ])
        .output()
        .unwrap();
    let pane_commands = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        pane_commands.lines().any(|line| line == "sleep"),
        "Expected one pane running sleep, commands were: {pane_commands}; captured panes: {captured}"
    );

    cleanup_session(&env.tmux_socket, session_name);
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
    wait_for_screen(&env, 2500, |s| s.contains("select branch"));

    // Navigate to the branch that has the worktree
    env.send("feat/to-delete");
    wait_ms(200);

    // Press Ctrl+X to trigger delete
    env.send_special("C-x");

    let screen = wait_for_screen(&env, 2000, |s| {
        s.contains("Delete") || s.contains("remove") || s.contains("confirm")
    });
    assert!(
        screen.contains("Delete") || screen.contains("remove") || screen.contains("confirm"),
        "Should show confirmation dialog: {screen}"
    );

    // Press Enter to confirm deletion
    env.send_special("Enter");
    wait_ms(500);

    // Just verify that we're back to the branch listing (the confirmation was processed)
    let screen = env.capture();
    assert!(
        screen.contains("select branch") || !screen.contains("Confirm delete"),
        "Should return to branch listing after deletion: {screen}"
    );
}

#[test]
fn test_e2e_delete_worktree_indicator_persists_after_restart() {
    let env = TestEnv::new("delete-persist");
    let search_dir = env.search_dir();
    env.install_slow_git_remove_wrapper(3);

    let repo = search_dir.join("persist-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    run_git(&repo, &["branch", "feat/persist-delete"]);
    let wt_dir = search_dir
        .join(WORKTREE_DIR_NAME)
        .join("persist-repo--feat-persist-delete");
    fs::create_dir_all(&wt_dir).unwrap();
    run_git(
        &repo,
        &[
            "worktree",
            "add",
            &wt_dir.to_string_lossy(),
            "feat/persist-delete",
        ],
    );

    env.write_config(&search_dir);
    env.launch_kiosk();

    env.send_special("Tab");
    wait_for_screen(&env, 2500, |s| s.contains("select branch"));
    env.send("feat/persist-delete");
    env.send_special("C-x");
    env.send_special("Enter");

    let screen = wait_for_screen(&env, 2000, |s| s.contains("deleting"));
    assert!(
        screen.contains("deleting"),
        "Should show deleting indicator immediately after confirm: {screen}"
    );

    cleanup_session(&env.tmux_socket, &env.session_name);
    wait_ms(150);
    assert!(
        wt_dir.exists(),
        "Worktree should still exist before restart"
    );

    env.launch_kiosk();
    env.send_special("Tab");
    wait_for_screen(&env, 2500, |s| s.contains("select branch"));
    env.send("feat/persist-delete");

    let screen = wait_for_screen(&env, 2000, |s| s.contains("deleting"));
    assert!(
        screen.contains("deleting"),
        "Deleting indicator should persist after restart: {screen}"
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
fn test_e2e_config_flag_overrides_xdg_config_home() {
    let env = TestEnv::new("config-flag");
    let search_dir = env.search_dir();

    let repo = search_dir.join("flag-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Config that remaps EnterRepo from Tab to F1 in repo selection
    let extra = r#"
[keys.repo_select]
F1 = "enter_repo"
tab = "noop"
"#;
    env.write_config_with_extra(&search_dir, extra);

    // Point XDG_CONFIG_HOME to a dir that does not contain kiosk/config.toml.
    let fake_xdg = env.tmp.path().join("fake-xdg");
    fs::create_dir_all(&fake_xdg).unwrap();

    env.launch_kiosk_with_config_arg(&env.config_file_path(), &fake_xdg);

    // Tab should be unbound by config loaded from --config.
    env.send_special("Tab");
    let screen = wait_for_screen(&env, 800, |s| s.contains("select branch"));
    assert!(
        !screen.contains("select branch"),
        "Tab should not enter branch view when loading config from --config: {screen}"
    );

    // F1 should work if --config took effect
    env.send_special("F1");
    let screen = wait_for_screen(&env, 2500, |s| s.contains("select branch"));
    assert!(
        screen.contains("select branch"),
        "F1 should enter repo when loading config from --config: {screen}"
    );
}

#[test]
fn test_e2e_modal_bindings_override_general() {
    let env = TestEnv::new("modal-overrides-general");
    let search_dir = env.search_dir();

    let repo = search_dir.join("modal-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Create a branch and worktree for delete confirmation flow
    run_git(&repo, &["branch", "feat/modal-test"]);
    let wt_dir = search_dir
        .join(WORKTREE_DIR_NAME)
        .join("modal-repo--feat-modal-test");
    fs::create_dir_all(&wt_dir).unwrap();
    run_git(
        &repo,
        &[
            "worktree",
            "add",
            &wt_dir.to_string_lossy(),
            "feat/modal-test",
        ],
    );

    // C-c quits generally, but should cancel inside modal due to [keys.modal].
    let extra = r#"
[keys.modal]
C-c = "cancel"
"#;
    env.write_config_with_extra(&search_dir, extra);
    env.launch_kiosk();

    env.send_special("Tab");
    wait_for_screen(&env, 2500, |s| s.contains("select branch"));
    env.send("feat/modal-test");
    env.send_special("C-x");

    let screen = wait_for_screen(&env, 2000, |s| s.contains("Confirm delete"));
    assert!(
        screen.contains("Confirm delete"),
        "Delete confirm dialog should be visible: {screen}"
    );

    // In modal, C-c should cancel dialog (not quit app).
    env.send_special("C-c");
    let screen = wait_for_screen(&env, 2000, |s| s.contains("select branch"));
    assert!(
        screen.contains("select branch") && !screen.contains("Confirm delete"),
        "C-c should cancel modal and stay in branch view: {screen}"
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
        screen.contains("Help") && screen.contains("key bindings"),
        "Help overlay should be visible: {screen}"
    );

    // Dismiss with Esc
    env.send_special("Escape");
    let screen = env.capture();
    assert!(
        screen.contains("select repo") && !screen.contains("key bindings"),
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
fn test_e2e_ctrl_u_clears_search_input() {
    let env = TestEnv::new("ctrl-u-clears-search");
    let search_dir = env.search_dir();

    let repo_a = search_dir.join("alpha-repo");
    let repo_b = search_dir.join("beta-repo");
    fs::create_dir_all(&repo_a).unwrap();
    fs::create_dir_all(&repo_b).unwrap();
    init_test_repo(&repo_a);
    init_test_repo(&repo_b);

    env.write_config(&search_dir);
    env.launch_kiosk();

    env.send("zzzz-no-match");
    let screen = env.capture();
    assert!(
        screen.contains("0 repos"),
        "Search should filter to 0 repos: {screen}"
    );

    env.send_special("C-u");
    let screen = env.capture();
    assert!(
        screen.contains("2 repos"),
        "Ctrl+U should clear search and restore repo list: {screen}"
    );
}

#[test]
fn test_e2e_alt_j_and_alt_k_half_page_navigation() {
    let env = TestEnv::new("alt-j-k-half-page");
    let search_dir = env.search_dir();

    for i in 0..30 {
        let repo = search_dir.join(format!("half-page-{i:02}"));
        fs::create_dir_all(&repo).unwrap();
        init_test_repo(&repo);
    }

    env.write_config(&search_dir);
    env.launch_kiosk();

    let screen = env.capture();
    let initial_selected =
        selected_line(&screen).expect("Expected an initially selected repo line in UI");

    env.send_special("M-j");
    let screen = env.capture();
    let after_down = selected_line(&screen).expect("Expected selected line after A-j");
    assert_ne!(
        after_down, initial_selected,
        "Alt+j should move selection by half page. screen:\n{screen}"
    );

    env.send_special("M-k");
    let screen = env.capture();
    let after_up = selected_line(&screen).expect("Expected selected line after A-k");
    assert_eq!(
        after_up, initial_selected,
        "Alt+k should move selection back by half page. screen:\n{screen}"
    );
}

#[test]
fn test_e2e_ctrl_v_and_alt_v_page_navigation() {
    let env = TestEnv::new("ctrl-v-alt-v-page");
    let search_dir = env.search_dir();

    for i in 0..40 {
        let repo = search_dir.join(format!("page-nav-{i:02}"));
        fs::create_dir_all(&repo).unwrap();
        init_test_repo(&repo);
    }

    env.write_config(&search_dir);
    env.launch_kiosk();

    let screen = env.capture();
    let initial_selected =
        selected_line(&screen).expect("Expected an initially selected repo line in UI");

    env.send_special("C-v");
    let screen = env.capture();
    let after_page_down = selected_line(&screen).expect("Expected selected line after C-v");
    assert_ne!(
        after_page_down, initial_selected,
        "Ctrl+v should move selection by page down. screen:\n{screen}"
    );

    env.send_special("M-v");
    let screen = env.capture();
    let after_page_up = selected_line(&screen).expect("Expected selected line after A-v");
    assert_eq!(
        after_page_up, initial_selected,
        "Alt+v should move selection back by page up. screen:\n{screen}"
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
    wait_for_screen(&env, 3000, |s| {
        s.contains("feat-search-target") && s.contains("(remote)")
    });

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

#[test]
fn test_e2e_repo_ordering_current_first() {
    let env = TestEnv::new("repo-order-current");
    let search_dir = env.search_dir();

    let repo_alpha = search_dir.join("alpha-repo");
    let repo_beta = search_dir.join("beta-repo");
    fs::create_dir_all(&repo_alpha).unwrap();
    fs::create_dir_all(&repo_beta).unwrap();
    init_test_repo(&repo_alpha);
    init_test_repo(&repo_beta);

    env.write_config(&search_dir);

    // Launch kiosk from inside beta-repo's directory so it becomes the current repo
    cleanup_session(&env.tmux_socket, &env.session_name);
    let binary = kiosk_binary();
    Command::new("tmux")
        .args([
            "-L",
            &env.tmux_socket,
            "new-session",
            "-d",
            "-s",
            &env.session_name,
            "-x",
            "120",
            "-y",
            "30",
            "-c",
            &repo_beta.to_string_lossy(),
            &format!(
                "XDG_CONFIG_HOME={} XDG_STATE_HOME={} KIOSK_NO_ALT_SCREEN=1 PATH={}:$PATH {} ; sleep 2",
                env.config_dir.to_string_lossy(),
                env.state_dir.to_string_lossy(),
                env.bin_dir.to_string_lossy(),
                binary.to_string_lossy()
            ),
        ])
        .output()
        .unwrap();
    wait_ms(500);

    let screen = wait_for_screen(&env, 3000, |s| {
        s.contains("alpha-repo") && s.contains("beta-repo")
    });

    // beta-repo should appear before alpha-repo since we launched from beta's dir
    let beta_pos = screen
        .find("beta-repo")
        .expect("beta-repo should be visible");
    let alpha_pos = screen
        .find("alpha-repo")
        .expect("alpha-repo should be visible");
    assert!(
        beta_pos < alpha_pos,
        "beta-repo (current) should appear before alpha-repo. Screen:\n{screen}"
    );
}

#[test]
fn test_e2e_repo_ordering_sessions_before_no_sessions() {
    let env = TestEnv::new("repo-order-sessions");
    let search_dir = env.search_dir();

    let repo_aaa = search_dir.join("aaa-repo");
    let repo_mmm = search_dir.join("mmm-repo");
    let repo_zzz = search_dir.join("zzz-repo");
    fs::create_dir_all(&repo_aaa).unwrap();
    fs::create_dir_all(&repo_mmm).unwrap();
    fs::create_dir_all(&repo_zzz).unwrap();
    init_test_repo(&repo_aaa);
    init_test_repo(&repo_mmm);
    init_test_repo(&repo_zzz);

    env.write_config(&search_dir);

    // Create a tmux session for mmm-repo (matching kiosk's session naming: repo name)
    Command::new("tmux")
        .args([
            "-L",
            &env.tmux_socket,
            "new-session",
            "-d",
            "-s",
            "mmm-repo",
            "-x",
            "80",
            "-y",
            "24",
        ])
        .output()
        .unwrap();
    wait_ms(200);

    env.launch_kiosk();

    let screen = wait_for_screen(&env, 3000, |s| {
        s.contains("aaa-repo") && s.contains("mmm-repo") && s.contains("zzz-repo")
    });

    let mmm_pos = screen.find("mmm-repo").expect("mmm-repo should be visible");
    let aaa_pos = screen.find("aaa-repo").expect("aaa-repo should be visible");
    let zzz_pos = screen.find("zzz-repo").expect("zzz-repo should be visible");

    // mmm-repo has a session, should appear before aaa-repo and zzz-repo
    assert!(
        mmm_pos < aaa_pos && mmm_pos < zzz_pos,
        "mmm-repo (has session) should appear before repos without sessions. Screen:\n{screen}"
    );
}

#[test]
fn test_e2e_branch_ordering() {
    let env = TestEnv::new("branch-order");
    let search_dir = env.search_dir();

    let repo = search_dir.join("order-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);

    // Create branches
    run_git(&repo, &["branch", "aaa-plain"]);
    run_git(&repo, &["branch", "mmm-worktree"]);
    run_git(&repo, &["branch", "zzz-plain"]);

    // Create a worktree for mmm-worktree
    let wt_dir = search_dir
        .join(WORKTREE_DIR_NAME)
        .join("order-repo--mmm-worktree");
    fs::create_dir_all(&wt_dir).unwrap();
    run_git(
        &repo,
        &["worktree", "add", &wt_dir.to_string_lossy(), "mmm-worktree"],
    );

    env.write_config(&search_dir);
    env.launch_kiosk();

    // Enter the repo
    env.send_special("Tab");
    wait_for_screen(&env, 2500, |s| s.contains("select branch"));

    let screen = env.capture();

    // main is current → first
    // mmm-worktree has a worktree → before plain branches
    // aaa-plain and zzz-plain are plain → alphabetical after worktree branches
    let main_pos = screen.find("main").expect("main should be visible");
    let mmm_pos = screen
        .find("mmm-worktree")
        .expect("mmm-worktree should be visible");
    let aaa_pos = screen
        .find("aaa-plain")
        .expect("aaa-plain should be visible");

    assert!(
        main_pos < mmm_pos,
        "main (current) should appear before mmm-worktree. Screen:\n{screen}"
    );
    assert!(
        mmm_pos < aaa_pos,
        "mmm-worktree (has worktree) should appear before aaa-plain. Screen:\n{screen}"
    );
}

#[test]
fn test_e2e_headless_list_and_branches_json() {
    let env = TestEnv::new("headless-list-branches");
    let search_dir = env.search_dir();
    let repo = search_dir.join("headless-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    run_git(&repo, &["branch", "feat/headless"]);

    env.write_config(&search_dir);

    let list_output = env.run_cli(&["list", "--json"]);
    assert!(
        list_output.status.success(),
        "list should succeed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_json: Value = serde_json::from_slice(&list_output.stdout).unwrap();
    assert!(
        list_json
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "headless-repo"),
        "headless-repo should be listed: {list_json}"
    );

    let branches_output = env.run_cli(&["branches", "headless-repo", "--json"]);
    assert!(
        branches_output.status.success(),
        "branches should succeed: {}",
        String::from_utf8_lossy(&branches_output.stderr)
    );
    let branches_json: Value = serde_json::from_slice(&branches_output.stdout).unwrap();
    assert!(
        branches_json
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "feat/headless"),
        "feat/headless should be present: {branches_json}"
    );
}

#[test]
fn test_e2e_headless_open_status_delete_workflow() {
    let env = TestEnv::new("headless-workflow");
    let search_dir = env.search_dir();
    let id = unique_id();
    let repo_name = format!("workflow-repo-{id}");
    let branch_name = format!("feat/e2e-headless-{id}");
    let repo = search_dir.join(&repo_name);
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    env.write_config(&search_dir);
    let mut cleanup = BranchCleanupGuard {
        bin: kiosk_binary(),
        config_dir: env.config_dir.clone(),
        state_dir: env.state_dir.clone(),
        repo: repo_name.clone(),
        branch: branch_name.clone(),
        enabled: true,
    };

    let open_output = env.run_cli(&[
        "open",
        &repo_name,
        "--new-branch",
        &branch_name,
        "--base",
        "main",
        "--no-switch",
        "--run",
        "echo KIOSK_TEST_MARKER",
        "--json",
    ]);
    assert!(
        open_output.status.success(),
        "open should succeed: {}",
        String::from_utf8_lossy(&open_output.stderr)
    );
    let open_json: Value = serde_json::from_slice(&open_output.stdout).unwrap();
    let session = open_json["session"].as_str().unwrap().to_string();
    assert!(
        wait_for_tmux_session(None, &session, 5000),
        "tmux session {session} should exist"
    );

    let mut output_text = String::new();
    for _ in 0..20 {
        let status_output = env.run_cli(&[
            "status",
            &repo_name,
            &branch_name,
            "--json",
            "--lines",
            "40",
        ]);
        assert!(
            status_output.status.success(),
            "status should succeed: {}",
            String::from_utf8_lossy(&status_output.stderr)
        );
        let status_json: Value = serde_json::from_slice(&status_output.stdout).unwrap();
        output_text = status_json["output"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if output_text.contains("KIOSK_TEST_MARKER") {
            break;
        }
        wait_ms(150);
    }
    assert!(
        output_text.contains("KIOSK_TEST_MARKER"),
        "status output should include marker: {output_text}"
    );

    let delete_output = env.run_cli(&["delete", &repo_name, &branch_name, "--force", "--json"]);
    assert!(
        delete_output.status.success(),
        "delete should succeed: {}",
        String::from_utf8_lossy(&delete_output.stderr)
    );
    let delete_json: Value = serde_json::from_slice(&delete_output.stdout).unwrap();
    assert_eq!(delete_json["deleted"], Value::Bool(true));
    assert_eq!(delete_json["repo"], Value::String(repo_name));
    assert_eq!(delete_json["branch"], Value::String(branch_name));
    cleanup.disable();
}

#[test]
fn test_e2e_headless_open_is_idempotent() {
    let env = TestEnv::new("headless-idempotent");
    let search_dir = env.search_dir();
    let id = unique_id();
    let repo_name = format!("idempotent-repo-{id}");
    let repo = search_dir.join(&repo_name);
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    env.write_config(&search_dir);

    let mut guard = SessionCleanupGuard { session: None };

    let first = env.run_cli(&["open", &repo_name, "main", "--no-switch", "--json"]);
    assert!(
        first.status.success(),
        "first open should succeed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["created"], Value::Bool(true));
    guard.session = first_json["session"].as_str().map(String::from);

    let second = env.run_cli(&["open", &repo_name, "main", "--no-switch", "--json"]);
    assert!(
        second.status.success(),
        "second open should succeed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_json: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second_json["created"], Value::Bool(false));
}

#[test]
fn test_e2e_headless_sessions_json() {
    let env = TestEnv::new("headless-sessions");
    let search_dir = env.search_dir();
    let id = unique_id();
    let repo_name = format!("sessions-repo-{id}");
    let repo = search_dir.join(&repo_name);
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    env.write_config(&search_dir);

    let open_output = env.run_cli(&["open", &repo_name, "main", "--no-switch", "--json"]);
    assert!(
        open_output.status.success(),
        "open should succeed: {}",
        String::from_utf8_lossy(&open_output.stderr)
    );
    let open_json: Value = serde_json::from_slice(&open_output.stdout).unwrap();
    let session = open_json["session"].as_str().unwrap().to_string();
    assert!(
        wait_for_tmux_session(None, &session, 5000),
        "tmux session {session} should exist"
    );

    let sessions_output = env.run_cli(&["sessions", "--json"]);
    assert!(
        sessions_output.status.success(),
        "sessions should succeed: {}",
        String::from_utf8_lossy(&sessions_output.stderr)
    );
    let sessions_json: Value = serde_json::from_slice(&sessions_output.stdout).unwrap();
    let sessions = sessions_json.as_array().unwrap();
    let matching = sessions
        .iter()
        .find(|s| s["session"].as_str() == Some(&session));
    assert!(
        matching.is_some(),
        "sessions output should include {session}: {sessions_json}"
    );
    let entry = matching.unwrap();
    assert_eq!(entry["repo"].as_str(), Some(repo_name.as_str()));
    assert!(
        entry["attached"].is_boolean(),
        "attached should be a boolean"
    );

    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &session])
        .output();
}

#[test]
fn test_e2e_headless_open_json_includes_repo_and_branch() {
    let env = TestEnv::new("headless-open-fields");
    let search_dir = env.search_dir();
    let id = unique_id();
    let repo_name = format!("fields-repo-{id}");
    let repo = search_dir.join(&repo_name);
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    env.write_config(&search_dir);

    let output = env.run_cli(&["open", &repo_name, "main", "--no-switch", "--json"]);
    assert!(
        output.status.success(),
        "open should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["repo"].as_str(), Some(repo_name.as_str()));
    assert_eq!(json["branch"].as_str(), Some("main"));

    if let Some(session) = json["session"].as_str() {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", session])
            .output();
    }
}

#[test]
fn test_e2e_headless_status_source_field() {
    let env = TestEnv::new("headless-status-source");
    let search_dir = env.search_dir();
    let id = unique_id();
    let repo_name = format!("source-repo-{id}");
    let repo = search_dir.join(&repo_name);
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    env.write_config(&search_dir);

    let open_output = env.run_cli(&["open", &repo_name, "main", "--no-switch", "--json"]);
    assert!(
        open_output.status.success(),
        "open should succeed: {}",
        String::from_utf8_lossy(&open_output.stderr)
    );
    let open_json: Value = serde_json::from_slice(&open_output.stdout).unwrap();
    let session = open_json["session"].as_str().unwrap().to_string();
    assert!(
        wait_for_tmux_session(None, &session, 5000),
        "tmux session {session} should exist"
    );

    let status_output = env.run_cli(&["status", &repo_name, "--json"]);
    assert!(
        status_output.status.success(),
        "status should succeed: {}",
        String::from_utf8_lossy(&status_output.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status_output.stdout).unwrap();
    assert_eq!(
        status_json["source"].as_str(),
        Some("live"),
        "source should be 'live' for an active session: {status_json}"
    );

    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &session])
        .output();
}

#[test]
fn test_e2e_headless_open_log_and_status_log_fallback() {
    let env = TestEnv::new("headless-log-fallback");
    let search_dir = env.search_dir();
    let id = unique_id();
    let repo_name = format!("log-repo-{id}");
    let branch_name = format!("feat/log-{id}");
    let repo = search_dir.join(&repo_name);
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    env.write_config(&search_dir);
    let mut cleanup = BranchCleanupGuard {
        bin: kiosk_binary(),
        config_dir: env.config_dir.clone(),
        state_dir: env.state_dir.clone(),
        repo: repo_name.clone(),
        branch: branch_name.clone(),
        enabled: true,
    };

    let open_output = env.run_cli(&[
        "open",
        &repo_name,
        "--new-branch",
        &branch_name,
        "--base",
        "main",
        "--no-switch",
        "--log",
        "--run",
        "echo LOG_MARKER_TEST",
        "--json",
    ]);
    assert!(
        open_output.status.success(),
        "open with --log should succeed: {}",
        String::from_utf8_lossy(&open_output.stderr)
    );
    let open_json: Value = serde_json::from_slice(&open_output.stdout).unwrap();
    let session = open_json["session"].as_str().unwrap().to_string();
    assert!(
        wait_for_tmux_session(None, &session, 5000),
        "tmux session {session} should exist"
    );

    // Wait for the command to produce output in the log
    wait_ms(2000);

    // Kill the session so status falls back to the log file
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &session])
        .output();
    wait_ms(500);

    let status_output = env.run_cli(&[
        "status",
        &repo_name,
        &branch_name,
        "--json",
        "--lines",
        "50",
    ]);
    assert!(
        status_output.status.success(),
        "status should succeed via log fallback: {}",
        String::from_utf8_lossy(&status_output.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status_output.stdout).unwrap();
    assert_eq!(
        status_json["source"].as_str(),
        Some("log"),
        "source should be 'log' when falling back to log file: {status_json}"
    );
    let log_output = status_json["output"].as_str().unwrap_or_default();
    assert!(
        log_output.contains("LOG_MARKER_TEST"),
        "log output should contain the marker: {log_output}"
    );

    cleanup.disable();
    let _ = env.run_cli(&["delete", &repo_name, &branch_name, "--force", "--json"]);
}

#[test]
fn test_e2e_headless_error_unknown_repo() {
    let env = TestEnv::new("headless-error-repo");
    let search_dir = env.search_dir();
    let repo = search_dir.join("exists-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    env.write_config(&search_dir);

    let output = env.run_cli(&["branches", "nonexistent-repo", "--json"]);
    assert!(!output.status.success(), "should fail for unknown repo");
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code should be 1 for user error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let error_json: Value = serde_json::from_str(stderr.trim()).unwrap();
    assert!(
        error_json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("nonexistent-repo"),
        "JSON error should mention the repo name: {error_json}"
    );
}

#[test]
fn test_e2e_headless_error_unknown_branch() {
    let env = TestEnv::new("headless-error-branch");
    let search_dir = env.search_dir();
    let repo = search_dir.join("err-branch-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    env.write_config(&search_dir);

    let output = env.run_cli(&[
        "open",
        "err-branch-repo",
        "nonexistent-branch",
        "--no-switch",
        "--json",
    ]);
    assert!(!output.status.success(), "should fail for unknown branch");
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code should be 1 for user error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let error_json: Value = serde_json::from_str(stderr.trim()).unwrap();
    assert!(
        error_json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("nonexistent-branch"),
        "JSON error should mention the branch name: {error_json}"
    );
}

#[test]
fn test_e2e_headless_error_delete_no_worktree() {
    let env = TestEnv::new("headless-error-del-nowt");
    let search_dir = env.search_dir();
    let repo = search_dir.join("del-nowt-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    run_git(&repo, &["branch", "feat/no-worktree"]);
    env.write_config(&search_dir);

    let output = env.run_cli(&["delete", "del-nowt-repo", "feat/no-worktree", "--json"]);
    assert!(
        !output.status.success(),
        "should fail when branch has no worktree"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "exit code should be 1 for user error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let error_json: Value = serde_json::from_str(stderr.trim()).unwrap();
    assert!(
        error_json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no worktree"),
        "JSON error should mention 'no worktree': {error_json}"
    );
}

#[test]
fn test_e2e_headless_branches_json_stable_schema() {
    let env = TestEnv::new("headless-branches-schema");
    let search_dir = env.search_dir();
    let repo = search_dir.join("schema-repo");
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    run_git(&repo, &["branch", "feat/schema-test"]);
    env.write_config(&search_dir);

    let output = env.run_cli(&["branches", "schema-repo", "--json"]);
    assert!(
        output.status.success(),
        "branches should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let branches = json.as_array().unwrap();
    assert!(!branches.is_empty());

    let first = &branches[0];
    assert!(first.get("name").is_some(), "should have 'name' field");
    assert!(
        first.get("has_session").is_some(),
        "should have 'has_session' field"
    );
    assert!(
        first.get("is_current").is_some(),
        "should have 'is_current' field"
    );
    assert!(
        first.get("is_remote").is_some(),
        "should have 'is_remote' field"
    );
    // Internal fields should NOT be exposed
    assert!(
        first.get("is_default").is_none(),
        "should NOT have internal 'is_default' field"
    );
    assert!(
        first.get("session_activity_ts").is_none(),
        "should NOT have internal 'session_activity_ts' field"
    );
}
