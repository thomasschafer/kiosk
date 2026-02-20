use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

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
        let kiosk_config_dir = self.config_dir.join("kiosk");
        fs::create_dir_all(&kiosk_config_dir).unwrap();
        let config = format!(r#"search_dirs = ["{}"]"#, search_dir.to_string_lossy());
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

    env.send_special("Enter");
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
    env.send_special("Enter");
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
    env.send_special("Enter");
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
    env.send_special("Enter");
    wait_ms(300);

    // Search for the branch and select it
    env.send("feat/wt");
    wait_ms(300);
    env.send_special("Enter");
    wait_ms(2000); // Wait for worktree creation + session

    // Verify the worktree was created in .kiosk_worktrees/ with -- separator
    let wt_root = search_dir.join(".kiosk_worktrees");
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
