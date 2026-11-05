use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, atomic::AtomicBool, mpsc},
    time::Duration,
};

use kiosk_core::{
    backend::KnownCheckout,
    event::{AppEvent, WorktreeRemovalOutcome},
    git::{
        CliGitProvider,
        test_fixtures::{repo_fixture, run_test_git},
    },
    spawn::{EventSender, spawn_create_new_branch, spawn_open_branch, spawn_worktree_removal},
};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use kiosk_core::config::{
    OnOpenConfig,
    on_open::{OnOpenPaneConfig, OnOpenPaneDirection},
};

#[derive(Debug, Clone)]
struct FakePane {
    pane: String,
    terminal: String,
    start: PathBuf,
    live: PathBuf,
}

#[derive(Debug, Clone)]
struct FakeSpace {
    id: String,
    machine: Option<String>,
    panes: Vec<FakePane>,
}

#[derive(Default)]
struct FakeState {
    spaces: Vec<FakeSpace>,
    next_id: usize,
    commands: Vec<(String, Value)>,
    /// What `hc send` typed into each terminal, in order.
    typed: Vec<(String, String)>,
    /// Makes `hc send` fail, as when the terminal ends before it is typed into.
    refuse_send: bool,
    focused_pane: Option<String>,
    /// Window commands and typing, in the order they happened: `do <command>` and
    /// `send <terminal>`.
    events: Vec<String>,
    /// How many times the backend asked for the spaces (`herdr-app ls`) and for a terminal's
    /// folder (`hc get`).
    listings: usize,
    folder_lookups: usize,
}

/// The parts of `herdr-app` and `hc` the app backend uses, over an in-memory window.
#[derive(Default)]
struct FakeApp {
    state: Mutex<FakeState>,
    /// When set, each `hc get` waits (up to a deadline) for this many to be in flight at once.
    rendezvous: Option<usize>,
    /// `hc get`s in flight now, and the most there have been at once.
    gets: (Mutex<(usize, usize)>, Condvar),
}

impl FakeApp {
    fn meet_other_gets(&self, want: usize) {
        let (gets, changed) = &self.gets;
        let mut gets = gets.lock().unwrap();
        gets.0 += 1;
        gets.1 = gets.1.max(gets.0);
        changed.notify_all();
        let (mut gets, _) = changed
            .wait_timeout_while(gets, Duration::from_secs(2), |gets| gets.1 < want)
            .unwrap();
        gets.0 -= 1;
    }
}

impl FakeApp {
    fn add_space(&self, folder: &Path) -> FakeSpace {
        let mut state = self.state.lock().unwrap();
        let space = FakeSpace {
            id: format!("sp{}", state.next_id),
            machine: None,
            panes: vec![new_pane(&mut state, folder)],
        };
        state.next_id += 1;
        state.spaces.push(space.clone());
        space
    }

    fn add_remote_space(&self, folder: &Path) {
        let mut state = self.state.lock().unwrap();
        let mut pane = new_pane(&mut state, folder);
        pane.terminal = format!("vps/{}", pane.terminal);
        state.spaces.push(FakeSpace {
            id: "vps/sp-remote".into(),
            machine: Some("vps".into()),
            panes: vec![pane],
        });
    }

    fn command_names(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();
        state
            .commands
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn commands(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().commands.clone()
    }

    fn spaces(&self) -> Vec<FakeSpace> {
        self.state.lock().unwrap().spaces.clone()
    }
}

fn new_pane(state: &mut FakeState, folder: &Path) -> FakePane {
    let id = state.next_id;
    state.next_id += 1;
    FakePane {
        pane: format!("p{id}"),
        terminal: format!("t{id}"),
        start: folder.to_path_buf(),
        live: folder.to_path_buf(),
    }
}

/// The arguments upstream Herdr's `herdr-app do` takes for each command the backend uses
/// (`src/commands.rs` there); it refuses any other.
const COMMAND_ARGUMENTS: &[(&str, &[&str])] = &[
    (
        "space.new",
        &["name", "folder", "tab", "argv", "task", "machine"],
    ),
    ("space.select", &["index", "id"]),
    ("space.close", &["id", "confirmed"]),
    ("terminal.open", &["argv", "folder", "place", "name"]),
    ("pane.focus", &["pane", "terminal"]),
    ("pane.close", &["pane"]),
];

fn assert_upstream_takes(name: &str, args: &Value) {
    let (_, allowed) = COMMAND_ARGUMENTS
        .iter()
        .find(|(command, _)| *command == name)
        .unwrap_or_else(|| panic!("upstream Herdr has no command {name}"));
    for key in args.as_object().unwrap().keys() {
        assert!(
            allowed.contains(&key.as_str()),
            "upstream Herdr's {name} refuses the argument {key}"
        );
    }
}

fn refused(message: &str) -> AppError {
    AppError::Other {
        code: "herdr_app".into(),
        message: message.into(),
    }
}

impl AppHost for FakeApp {
    fn herdr_app(&self, args: &[String]) -> Result<Vec<u8>, AppError> {
        let mut state = self.state.lock().unwrap();
        if args == ["ls"] {
            state.listings += 1;
            let spaces: Vec<Value> = state
                .spaces
                .iter()
                .map(|space| {
                    json!({
                        "id": space.id,
                        "machine": space.machine,
                        "tabs": [{ "id": "tb", "terminals": space.panes.iter().map(|pane| json!({
                            "terminal": pane.terminal, "pane": pane.pane, "sub_agents": []
                        })).collect::<Vec<_>>() }],
                    })
                })
                .collect();
            return Ok(json!({ "spaces": spaces, "recent": [] }).to_string().into());
        }
        let [verb, name, args] = args else {
            panic!("unexpected herdr-app call {args:?}");
        };
        assert_eq!(verb, "do");
        let args: Value = serde_json::from_str(args).unwrap();
        assert_upstream_takes(name, &args);
        state.commands.push((name.clone(), args.clone()));
        state.events.push(format!("do {name}"));
        let output = match name.as_str() {
            "space.new" => {
                let folder = PathBuf::from(args["folder"].as_str().unwrap());
                let pane = new_pane(&mut state, &folder);
                let id = format!("sp{}", state.next_id);
                state.next_id += 1;
                let output = json!({ "space": id, "tab": "tb", "terminal": pane.terminal });
                state.spaces.push(FakeSpace {
                    id,
                    machine: None,
                    panes: vec![pane],
                });
                output
            }
            "space.select" => {
                let id = args["id"].as_str().unwrap();
                if !state.spaces.iter().any(|space| space.id == id) {
                    return Err(refused("no such space"));
                }
                json!({ "ok": true })
            }
            "space.close" => {
                assert_eq!(args["confirmed"], true);
                let id = args["id"].as_str().unwrap();
                state.spaces.retain(|space| space.id != id);
                json!({ "ok": true })
            }
            "terminal.open" => {
                assert!(
                    matches!(args["place"].as_str(), Some("right" | "down")),
                    "only splits are expected: {args}"
                );
                let target = state
                    .focused_pane
                    .clone()
                    .ok_or_else(|| refused("no pane to split"))?;
                let index = state
                    .spaces
                    .iter()
                    .position(|space| space.panes.iter().any(|pane| pane.pane == target))
                    .ok_or_else(|| refused("no such pane"))?;
                // Without one, the app would start in the pane's live folder.
                let folder = PathBuf::from(args["folder"].as_str().expect("no folder given"));
                let pane = new_pane(&mut state, &folder);
                state.focused_pane = Some(pane.pane.clone());
                let output = json!({ "pane": pane.pane, "terminal": pane.terminal });
                state.spaces[index].panes.push(pane);
                output
            }
            "pane.focus" => {
                let target = args["pane"].as_str().unwrap();
                if !state
                    .spaces
                    .iter()
                    .any(|space| space.panes.iter().any(|pane| pane.pane == target))
                {
                    return Err(refused("no such pane"));
                }
                state.focused_pane = Some(target.into());
                json!({ "ok": true })
            }
            "pane.close" => {
                let target = args["pane"].as_str().unwrap();
                for space in &mut state.spaces {
                    space.panes.retain(|pane| pane.pane != target);
                }
                json!({ "ok": true })
            }
            other => panic!("unexpected command {other}"),
        };
        Ok(output.to_string().into())
    }

    fn hc(&self, args: &[String]) -> Result<Vec<u8>, AppError> {
        if let Some(want) = self.rendezvous {
            self.meet_other_gets(want);
        }
        let mut state = self.state.lock().unwrap();
        let pane = |state: &FakeState, terminal: &str| {
            state
                .spaces
                .iter()
                .flat_map(|space| &space.panes)
                .find(|pane| pane.terminal == terminal)
                .cloned()
        };
        match args {
            [get, terminal, flag] if get == "get" && flag == "--json" => {
                assert!(
                    !terminal.contains('/'),
                    "asked hc for another machine's terminal {terminal}"
                );
                state.folder_lookups += 1;
                let pane = pane(&state, terminal).expect("hc get of an unknown terminal");
                Ok(json!({
                    "address": terminal,
                    "cwd": pane.start,
                    "foreground": { "cwd": pane.live, "name": "zsh" },
                })
                .to_string()
                .into())
            }
            [send, separator, terminal, text] if send == "send" && separator == "--" => {
                if state.refuse_send || pane(&state, terminal).is_none() {
                    return Err(refused("no such terminal"));
                }
                state.typed.push((terminal.clone(), text.clone()));
                state.events.push(format!("send {terminal}"));
                Ok(Vec::new())
            }
            _ => panic!("unexpected hc call {args:?}"),
        }
    }
}

struct Fixture {
    temp: TempDir,
    repo: PathBuf,
    app: Arc<FakeApp>,
    backend: Arc<HerdrApp>,
    notices: Arc<PostedNotices>,
    /// Where the backend's `[on_open]` panes wait until `background.wait()`.
    background: Arc<Background>,
    later: Arc<SavedNotices>,
}

/// Notices saved for the mod to show later, in order.
#[derive(Default)]
struct SavedNotices(Mutex<Vec<(String, String)>>);

impl LaterNotices for SavedNotices {
    fn save(&self, title: &str, body: &str) -> Result<(), String> {
        self.0.lock().unwrap().push((title.into(), body.into()));
        Ok(())
    }
}

/// Notifications posted, in order.
#[derive(Default)]
struct PostedNotices(Mutex<Vec<(String, String)>>);

impl Notices for PostedNotices {
    fn post(&self, title: &str, body: &str) -> Result<(), String> {
        self.0.lock().unwrap().push((title.into(), body.into()));
        Ok(())
    }
}

fn backend(
    app: &Arc<FakeApp>,
    notices: &Arc<PostedNotices>,
    temp: &Path,
    on_open: OnOpenConfig,
) -> HerdrApp {
    backend_with(
        app,
        notices,
        temp,
        on_open,
        &Arc::new(Background::deferred()),
        &Arc::new(SavedNotices::default()),
    )
}

fn backend_with(
    app: &Arc<FakeApp>,
    notices: &Arc<PostedNotices>,
    temp: &Path,
    on_open: OnOpenConfig,
    background: &Arc<Background>,
    later: &Arc<SavedNotices>,
) -> HerdrApp {
    HerdrApp::new(
        Arc::clone(app) as Arc<dyn AppHost>,
        Arc::new(CliGitProvider),
        WorktreePlacement::new(temp.join("worktrees")).unwrap(),
        Arc::clone(notices) as Arc<dyn Notices>,
        on_open,
        Arc::clone(background),
        Arc::clone(later) as Arc<dyn LaterNotices>,
    )
}

/// The spaces on this machine as the app's `kiosk` mod would pass them.
fn snapshot(app: &FakeApp) -> Vec<Space> {
    app.spaces()
        .into_iter()
        .filter(|space| space.machine.is_none())
        .map(|space| Space {
            id: space.id,
            folders: space
                .panes
                .into_iter()
                .map(|pane| Some(pane.live))
                .collect(),
        })
        .collect()
}

/// Real Git that counts how often the backend asks for a folder's main checkout.
#[derive(Default)]
struct CountingGit {
    main_checkouts: Mutex<usize>,
}

impl GitProvider for CountingGit {
    fn scan_repos_streaming(
        &self,
        dir: &Path,
        depth: u16,
        include_non_git: bool,
        is_cancelled: &dyn Fn() -> bool,
        on_found: &dyn Fn(kiosk_core::git::Repo),
    ) -> anyhow::Result<Vec<kiosk_core::git::ScanWarning>> {
        CliGitProvider.scan_repos_streaming(dir, depth, include_non_git, is_cancelled, on_found)
    }
    fn list_branches(&self, repo: &Path) -> anyhow::Result<kiosk_core::git::Listed<String>> {
        CliGitProvider.list_branches(repo)
    }
    fn list_remote_branches_for_remote(
        &self,
        repo: &Path,
        remote: &str,
    ) -> anyhow::Result<kiosk_core::git::Listed<String>> {
        CliGitProvider.list_remote_branches_for_remote(repo, remote)
    }
    fn list_worktrees(
        &self,
        repo: &Path,
    ) -> anyhow::Result<kiosk_core::git::Listed<kiosk_core::git::Worktree>> {
        CliGitProvider.list_worktrees(repo)
    }
    fn list_remotes(&self, repo: &Path) -> anyhow::Result<Vec<String>> {
        CliGitProvider.list_remotes(repo)
    }
    fn fetch_remote(&self, repo: &Path, remote: &str) -> anyhow::Result<()> {
        CliGitProvider.fetch_remote(repo, remote)
    }
    fn create_tracking_branch(
        &self,
        repo: &Path,
        branch: &str,
        remote: &str,
    ) -> anyhow::Result<()> {
        CliGitProvider.create_tracking_branch(repo, branch, remote)
    }
    fn is_valid_branch_name(&self, repo: &Path, branch: &str) -> anyhow::Result<bool> {
        CliGitProvider.is_valid_branch_name(repo, branch)
    }
    fn add_worktree(
        &self,
        repo: &Path,
        worktree: &Path,
        branch: &str,
        base: Option<&str>,
    ) -> anyhow::Result<()> {
        CliGitProvider.add_worktree(repo, worktree, branch, base)
    }
    fn main_checkout(&self, path: &Path) -> anyhow::Result<Option<PathBuf>> {
        *self.main_checkouts.lock().unwrap() += 1;
        CliGitProvider.main_checkout(path)
    }
    fn remove_worktree(&self, repo: &Path, worktree: &Path, force: bool) -> anyhow::Result<()> {
        CliGitProvider.remove_worktree(repo, worktree, force)
    }
    fn prune_worktrees(&self, repo: &Path) -> anyhow::Result<()> {
        CliGitProvider.prune_worktrees(repo)
    }
    fn default_branch(&self, repo: &Path, local: &[String]) -> anyhow::Result<Option<String>> {
        CliGitProvider.default_branch(repo, local)
    }
}

impl Fixture {
    fn new() -> Self {
        Self::with_on_open(OnOpenConfig::default())
    }

    fn with_on_open(on_open: OnOpenConfig) -> Self {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        let app = Arc::new(FakeApp::default());
        let notices = Arc::new(PostedNotices::default());
        let background = Arc::new(Background::deferred());
        let later = Arc::new(SavedNotices::default());
        let backend = Arc::new(backend_with(
            &app,
            &notices,
            temp.path(),
            on_open,
            &background,
            &later,
        ));
        Self {
            temp,
            repo,
            app,
            backend,
            notices,
            background,
            later,
        }
    }

    fn linked(&self, branch: &str) -> PathBuf {
        let path = self.temp.path().join(format!("repo-{branch}"));
        run_test_git(
            &self.repo,
            &["worktree", "add", "-b", branch, path.to_str().unwrap()],
        );
        path
    }

    fn worktree_root(&self) -> PathBuf {
        self.temp.path().join("worktrees")
    }

    fn checkout_state(&self, path: &Path) -> CheckoutState<SpaceId> {
        self.backend
            .checkouts(&self.repo)
            .unwrap()
            .into_iter()
            .find(|state| canonical(&state.path) == canonical(path))
            .unwrap_or_else(|| panic!("{} is not a checkout", path.display()))
    }

    /// Runs one of kiosk's branch operations against this backend and returns its event.
    fn run(
        &self,
        start: impl FnOnce(&Arc<dyn GitProvider>, &Arc<HerdrApp>, &EventSender),
    ) -> AppEvent {
        next(&self.start(start))
    }

    /// Starts one of kiosk's branch operations and returns the events it sends.
    fn start(
        &self,
        start: impl FnOnce(&Arc<dyn GitProvider>, &Arc<HerdrApp>, &EventSender),
    ) -> mpsc::Receiver<AppEvent> {
        let git: Arc<dyn GitProvider> = Arc::new(CliGitProvider);
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        start(&git, &self.backend, &sender);
        rx
    }

    /// Removes `checkout` as the picker does, which lets the space close once git has
    /// removed the checkout.
    fn remove(&self, branch: &str, checkout: &Path, force: bool) -> WorktreeRemovalOutcome {
        let events = self.start(|git, backend, sender| {
            spawn_worktree_removal(
                git,
                backend,
                sender,
                crate::PRODUCT.backend_name,
                self.repo.clone(),
                branch.into(),
                checkout.into(),
                force,
            );
        });
        let mut event = next(&events);
        if let AppEvent::CheckoutRemoved { gate, .. } = event {
            // Dropping the gate lets the removal go on to close the space.
            drop(gate);
            event = next(&events);
        }
        let AppEvent::WorktreeRemovalFinished { outcome, .. } = event else {
            panic!("expected a removal, got {event:?}");
        };
        outcome
    }
}

fn next(events: &mpsc::Receiver<AppEvent>) -> AppEvent {
    events.recv_timeout(Duration::from_secs(10)).unwrap()
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

#[test]
fn opening_a_closed_repo_makes_an_unnamed_space_in_it() {
    let fixture = Fixture::new();

    let opened = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(opened, Opened::default());
    assert_eq!(fixture.app.spaces().len(), 1);
    let commands = fixture.app.commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, "space.new");
    assert_eq!(
        commands[0]
            .1
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["folder"],
        "the space must stay unnamed so the app names it after its folder"
    );
}

#[test]
fn opening_a_repo_already_shown_selects_its_space() {
    let fixture = Fixture::with_on_open(editor_on_open());
    let subdir = fixture.repo.join("src");
    fs::create_dir(&subdir).unwrap();
    fixture.app.add_space(Path::new("/elsewhere"));
    let shown = fixture.app.add_space(&subdir);

    fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(
        fixture.app.commands(),
        [("space.select".into(), json!({ "id": shown.id }))],
        "an existing space gets no on_open panes"
    );
}

#[test]
fn a_space_follows_its_first_terminals_live_folder() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(Path::new("/elsewhere"));
    fixture.app.state.lock().unwrap().spaces[0].panes[0].live = fixture.repo.clone();

    assert_eq!(
        fixture.checkout_state(&fixture.repo).session,
        Some(SpaceId(space.id))
    );
}

#[test]
fn a_checkout_nested_in_the_repo_owns_the_spaces_inside_it() {
    let fixture = Fixture::new();
    let nested = fixture.repo.join(".worktrees").join("nested");
    run_test_git(
        &fixture.repo,
        &["worktree", "add", "-b", "nested", nested.to_str().unwrap()],
    );
    let space = fixture.app.add_space(&nested);

    let checkouts = fixture.backend.checkouts(&fixture.repo).unwrap();

    let [main, nested_state] = checkouts.as_slice() else {
        panic!("expected two checkouts, got {checkouts:?}");
    };
    assert_eq!(canonical(&main.path), canonical(&fixture.repo));
    assert_eq!(main.session, None);
    assert_eq!(canonical(&nested_state.path), canonical(&nested));
    assert_eq!(nested_state.session, Some(SpaceId(space.id)));
}

#[test]
fn opening_a_branch_uses_its_checkout() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");

    let event = fixture.run(|git, backend, sender| {
        spawn_open_branch(
            git,
            backend,
            sender,
            fixture.repo.clone(),
            "feature".into(),
            KnownCheckout::Exists,
        );
    });

    assert!(
        matches!(event, AppEvent::RepoOpened { warning: None }),
        "{event:?}"
    );
    let spaces = fixture.app.spaces();
    assert_eq!(spaces.len(), 1);
    assert_eq!(canonical(&spaces[0].panes[0].start), canonical(&feature));
}

#[test]
fn creating_a_branch_checks_it_out_beside_the_herdr_checkouts() {
    let fixture = Fixture::new();
    run_test_git(&fixture.repo, &["branch", "base"]);

    let event = fixture.run(|git, backend, sender| {
        spawn_create_new_branch(
            git,
            backend,
            sender,
            fixture.repo.clone(),
            "Feature/Login_Page.v2".into(),
            "base".into(),
        );
    });

    assert!(
        matches!(event, AppEvent::RepoOpened { warning: None }),
        "{event:?}"
    );
    let expected = fixture
        .worktree_root()
        .join("repo")
        .join("feature-login-page-v2");
    assert!(expected.join("README.md").exists());
    assert!(fixture.checkout_state(&expected).session.is_some());
}

#[test]
fn a_branch_without_a_checkout_gets_one_in_the_herdr_worktree_folder() {
    let fixture = Fixture::new();
    run_test_git(&fixture.repo, &["branch", "existing"]);

    let event = fixture.run(|git, backend, sender| {
        spawn_open_branch(
            git,
            backend,
            sender,
            fixture.repo.clone(),
            "existing".into(),
            KnownCheckout::Missing,
        );
    });

    assert!(
        matches!(event, AppEvent::RepoOpened { warning: None }),
        "{event:?}"
    );
    let checkout = fixture.worktree_root().join("repo").join("existing");
    assert!(checkout.join("README.md").exists());
    assert_eq!(fixture.app.spaces().len(), 1);
}

#[test]
fn removing_a_worktree_deletes_the_checkout_then_closes_its_space() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    fixture.app.add_space(&feature);

    let outcome = fixture.remove("feature", &feature, false);

    assert_eq!(outcome, WorktreeRemovalOutcome::Removed { warning: None });
    assert!(!feature.exists());
    assert!(fixture.app.spaces().is_empty());
    assert_eq!(fixture.app.command_names(), ["space.close"]);
}

#[test]
fn removing_a_dirty_worktree_needs_force_and_keeps_the_space() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    fs::write(feature.join("scratch.txt"), "unsaved").unwrap();
    fixture.app.add_space(&feature);

    assert_eq!(
        fixture.remove("feature", &feature, false),
        WorktreeRemovalOutcome::DirtyRequiresForce
    );
    assert!(feature.exists());
    assert_eq!(fixture.app.spaces().len(), 1);

    assert_eq!(
        fixture.remove("feature", &feature, true),
        WorktreeRemovalOutcome::Removed { warning: None }
    );
    assert!(!feature.exists());
    assert!(fixture.app.spaces().is_empty());
}

#[test]
fn spaces_report_their_repository_and_other_machines_are_ignored() {
    let fixture = Fixture::new();
    let feature = fixture.linked("feature");
    let plain = fixture.temp.path().join("plain");
    fs::create_dir(&plain).unwrap();
    fixture.app.add_space(&fixture.repo);
    fixture.app.add_space(&feature);
    fixture.app.add_space(&plain);
    fixture.app.add_remote_space(&fixture.repo);

    let roots: Vec<_> = fixture
        .backend
        .open_repos()
        .unwrap()
        .iter()
        .map(|root| canonical(root))
        .collect();
    assert_eq!(roots, [canonical(&fixture.repo), canonical(&fixture.repo)]);

    let folders = fixture.backend.open_folders().unwrap();
    assert_eq!(folders.len(), 3);
    assert_eq!(canonical(&folders[2]), canonical(&plain));
}

#[test]
fn folders_open_as_spaces_and_an_open_folder_selects_its_space() {
    let fixture = Fixture::with_on_open(editor_on_open());
    let plain = fixture.temp.path().join("plain");
    fs::create_dir(&plain).unwrap();

    fixture.backend.open_folder(&plain).unwrap();
    fixture.backend.open_folder(&plain).unwrap();

    assert_eq!(
        fixture.app.command_names(),
        ["space.new", "space.select"],
        "folders get no on_open panes"
    );
}

fn split_request(pane_id: &str, cwd: &Path) -> PaneSplitRequest {
    PaneSplitRequest {
        pane_id: pane_id.into(),
        direction: OnOpenPaneDirection::Right,
        ratio: None,
        cwd: cwd.to_path_buf(),
        focus: false,
    }
}

#[test]
fn a_pane_opens_beside_the_pane_with_its_command_typed_and_gives_focus_back() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    let root = &space.panes[0].pane;

    let split = fixture
        .backend
        .window
        .pane_open(&split_request(root, &fixture.repo), "hx .")
        .unwrap();

    assert_ne!(&split.pane_id, root);
    let folder = fixture.repo.to_str().unwrap();
    assert_eq!(
        fixture.app.commands(),
        [
            ("pane.focus".into(), json!({ "pane": root })),
            (
                "terminal.open".into(),
                json!({ "place": "right", "folder": folder })
            ),
            ("pane.focus".into(), json!({ "pane": root })),
        ]
    );
    let state = fixture.app.state.lock().unwrap();
    let new = &state.spaces[0].panes[1];
    assert_eq!(
        (new.pane.as_str(), new.start.as_path()),
        (split.pane_id.as_str(), fixture.repo.as_path())
    );
    assert_eq!(state.typed, [(new.terminal.clone(), "hx .".into())]);
    assert_eq!(state.focused_pane.as_ref(), Some(root));
    assert_eq!(
        state.events.last(),
        Some(&format!("send {}", new.terminal)),
        "focus goes back before the command is typed"
    );
}

#[test]
fn a_pane_starts_in_the_checkout_wherever_the_split_pane_has_gone() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    fixture.app.state.lock().unwrap().spaces[0].panes[0].live = "/elsewhere".into();

    fixture
        .backend
        .window
        .pane_open(&split_request(&space.panes[0].pane, &fixture.repo), "hx")
        .unwrap();

    assert_eq!(fixture.app.spaces()[0].panes[1].start, fixture.repo);
}

#[test]
fn a_pane_that_cannot_open_as_asked_is_refused_before_the_app_is_called() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    let relative = split_request(&space.panes[0].pane, Path::new("repo"));
    let request = split_request(&space.panes[0].pane, &fixture.repo);

    for (request, command, why) in [(&relative, "hx", "absolute"), (&request, "", "command")] {
        let Err(PaneOpenError::Split(error)) = fixture.backend.window.pane_open(request, command)
        else {
            panic!("{why}: expected a refused split");
        };
        assert!(error.to_string().contains(why), "{error}");
    }
    assert_eq!(fixture.app.commands(), []);
}

#[test]
fn a_command_that_cannot_be_typed_is_reported_as_a_pane_made_without_it() {
    let fixture = Fixture::new();
    let space = fixture.app.add_space(&fixture.repo);
    fixture.app.state.lock().unwrap().refuse_send = true;

    let error = fixture
        .backend
        .window
        .pane_open(&split_request(&space.panes[0].pane, &fixture.repo), "hx")
        .unwrap_err();

    assert_eq!(error, PaneOpenError::Run(refused("no such terminal")));
    assert_eq!(fixture.app.spaces()[0].panes.len(), 2);
    assert_eq!(
        fixture.app.state.lock().unwrap().focused_pane.as_ref(),
        Some(&space.panes[0].pane),
        "focus goes back to the first pane"
    );
}

#[test]
fn a_refused_split_is_reported_as_no_pane_made() {
    let fixture = Fixture::new();

    let error = fixture
        .backend
        .window
        .pane_open(&split_request("missing", &fixture.repo), "hx")
        .unwrap_err();

    assert_eq!(error, PaneOpenError::Split(refused("no such pane")));
}

#[test]
fn every_terminal_folder_is_looked_up_at_once_and_kept_with_its_pane() {
    let temp = TempDir::new().unwrap();
    let folders: Vec<PathBuf> = (0..5).map(|i| temp.path().join(format!("f{i}"))).collect();
    let app = Arc::new(FakeApp {
        rendezvous: Some(folders.len()),
        ..FakeApp::default()
    });
    for folder in &folders {
        app.add_space(folder);
    }
    let backend = backend(
        &app,
        &Arc::new(PostedNotices::default()),
        temp.path(),
        OnOpenConfig::default(),
    );

    let spaces = backend.spaces().unwrap();

    assert_eq!(
        app.gets.0.lock().unwrap().1,
        folders.len(),
        "all in flight together"
    );
    let listed: Vec<_> = spaces
        .iter()
        .map(|space| (space.id.clone(), space.folders[0].clone().unwrap()))
        .collect();
    let expected: Vec<_> = app
        .spaces()
        .iter()
        .map(|space| (space.id.clone(), space.panes[0].live.clone()))
        .collect();
    assert_eq!(listed, expected);
}

#[test]
fn notifications_are_posted_in_order() {
    let fixture = Fixture::new();

    fixture.backend.notify("herdr-kiosk", "first").unwrap();
    fixture
        .backend
        .notify("herdr-kiosk", "second \"quoted\"")
        .unwrap();

    assert_eq!(
        *fixture.notices.0.lock().unwrap(),
        [
            ("herdr-kiosk".to_owned(), "first".to_owned()),
            ("herdr-kiosk".to_owned(), "second \"quoted\"".to_owned()),
        ]
    );
}

#[test]
fn empty_notifications_are_refused() {
    let fixture = Fixture::new();
    assert!(fixture.backend.notify("herdr-kiosk", "").is_err());
    assert!(fixture.notices.0.lock().unwrap().is_empty());
}

fn editor_on_open() -> OnOpenConfig {
    OnOpenConfig {
        panes: vec![OnOpenPaneConfig {
            command: "hx".into(),
            direction: OnOpenPaneDirection::Right,
            ratio: None,
        }],
    }
}

#[test]
fn opening_a_repo_reports_it_opened_before_its_on_open_panes() {
    let fixture = Fixture::with_on_open(editor_on_open());

    let opened = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(opened, Opened::default());
    assert_eq!(fixture.app.command_names(), ["space.new"]);
    assert_eq!(fixture.app.state.lock().unwrap().typed, Vec::new());
    fixture.background.wait();
    assert_eq!(fixture.app.spaces()[0].panes.len(), 2);
}

#[test]
fn opening_a_repo_runs_on_open_panes_in_the_new_space() {
    let fixture = Fixture::with_on_open(editor_on_open());

    fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();
    fixture.background.wait();

    let spaces = fixture.app.spaces();
    assert_eq!(spaces.len(), 1);
    let [root, editor] = spaces[0].panes.as_slice() else {
        panic!("expected two panes, got {:?}", spaces[0].panes);
    };
    assert_eq!(
        fixture.app.state.lock().unwrap().typed,
        [(editor.terminal.clone(), "hx".into())]
    );
    assert_eq!(
        kiosk_core::path::canonical_or_original(&editor.start),
        kiosk_core::path::canonical_or_original(&fixture.repo)
    );
    assert_eq!(
        fixture.app.state.lock().unwrap().focused_pane.as_ref(),
        Some(&root.pane)
    );
}

#[test]
fn an_on_open_pane_with_a_ratio_opens_in_half_with_one_warning_at_start() {
    let mut on_open = editor_on_open();
    assert_eq!(ignored_ratio_warning(&on_open), None);
    on_open.panes[0].ratio = Some(0.3);
    let warning = ignored_ratio_warning(&on_open).unwrap();
    assert!(warning.contains("always splits panes in half"), "{warning}");
    let fixture = Fixture::with_on_open(on_open);

    let opened = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();

    assert_eq!(opened.warning, None);
    fixture.background.wait();
    assert_eq!(fixture.app.spaces()[0].panes.len(), 2);
    assert_eq!(*fixture.later.0.lock().unwrap(), Vec::new());
}

#[test]
fn on_open_panes_that_fail_are_saved_for_the_mod_to_show_later() {
    let fixture = Fixture::with_on_open(editor_on_open());
    fixture.app.state.lock().unwrap().refuse_send = true;

    let opened = fixture
        .backend
        .open_checkout(&fixture.repo, &fixture.repo)
        .unwrap();
    fixture.background.wait();

    assert_eq!(opened, Opened::default());
    let later = fixture.later.0.lock().unwrap();
    let [(title, body)] = later.as_slice() else {
        panic!("expected one saved notice, got {later:?}");
    };
    assert_eq!(title, ON_OPEN_FAILED);
    assert!(body.contains("no such terminal"), "{body}");
}

#[cfg(unix)]
mod cli {
    use super::*;
    use crate::test_support::{retry_while_text_file_busy, write_script};

    fn script(temp: &TempDir, name: &str, body: &str) -> PathBuf {
        let path = temp.path().join(name);
        write_script(&path, body);
        path
    }

    #[test]
    fn refusals_from_either_program_keep_their_message() {
        let temp = TempDir::new().unwrap();
        let app = script(
            &temp,
            "herdr-app",
            r#"printf '{"error":"no such space"}'; exit 1"#,
        );
        let hc = script(
            &temp,
            "hc",
            r#"printf '{"error":{"kind":"not_found","message":"no terminal t9"}}' >&2; exit 3"#,
        );
        let host = CliAppHost::new(app, hc, None);

        assert_eq!(
            retry_while_text_file_busy(|| host.herdr_app(&["do".into()])).unwrap_err(),
            refused("no such space")
        );
        assert_eq!(
            retry_while_text_file_busy(|| host.hc(&["get".into()])).unwrap_err(),
            AppError::Other {
                code: "not_found".into(),
                message: "no terminal t9".into(),
            }
        );
    }

    #[test]
    fn herdr_app_drives_the_window_of_the_app_home_it_was_given() {
        let temp = TempDir::new().unwrap();
        let app = script(
            &temp,
            "herdr-app",
            r#"printf '%s' "${HERDR_APP_HOME-unset}""#,
        );
        let hc = script(&temp, "hc", r#"printf '%s' "${HERDR_APP_HOME-unset}""#);
        let home = temp.path().join("app-home");
        let host = CliAppHost::new(&app, &hc, Some(home.clone()));
        let output = retry_while_text_file_busy(|| host.herdr_app(&["ls".into()])).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            home.display().to_string()
        );

        let default = CliAppHost::new(&app, &hc, None);
        let inherited = std::env::var("HERDR_APP_HOME").unwrap_or_else(|_| "unset".into());
        let output = retry_while_text_file_busy(|| default.herdr_app(&["ls".into()])).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), inherited);
    }

    #[test]
    fn other_failures_report_the_exit_and_stderr() {
        let temp = TempDir::new().unwrap();
        let app = script(&temp, "herdr-app", "echo 'window is gone' >&2; exit 2");
        let host = CliAppHost::new(app, temp.path().join("missing-hc"), None);

        let error = retry_while_text_file_busy(|| host.herdr_app(&["ls".into()])).unwrap_err();
        assert!(
            matches!(&error, AppError::Invocation(message) if message.contains("window is gone")),
            "{error}"
        );
        assert!(matches!(
            host.hc(&["ls".into()]).unwrap_err(),
            AppError::Invocation(_)
        ));
    }
}

#[test]
fn the_spaces_the_mod_saw_are_used_without_asking_the_app() {
    let fixture = Fixture::new();
    let subdir = fixture.repo.join("src");
    fs::create_dir(&subdir).unwrap();
    fixture.app.add_space(Path::new("/elsewhere"));
    let shown = fixture.app.add_space(&subdir);
    let backend = backend(
        &fixture.app,
        &fixture.notices,
        fixture.temp.path(),
        OnOpenConfig::default(),
    )
    .with_spaces(snapshot(&fixture.app));

    let roots = backend.open_repos().unwrap();
    backend.open_checkout(&fixture.repo, &fixture.repo).unwrap();

    assert_eq!(
        roots.iter().map(|root| canonical(root)).collect::<Vec<_>>(),
        [canonical(&fixture.repo)]
    );
    assert_eq!(
        fixture.app.commands(),
        [("space.select".into(), json!({ "id": shown.id }))]
    );
    let state = fixture.app.state.lock().unwrap();
    assert_eq!((state.listings, state.folder_lookups), (0, 0));
}

#[test]
fn once_kiosk_changes_the_window_it_asks_the_app_again() {
    let fixture = Fixture::new();
    let backend = backend(
        &fixture.app,
        &fixture.notices,
        fixture.temp.path(),
        OnOpenConfig::default(),
    )
    .with_spaces(snapshot(&fixture.app));
    assert_eq!(backend.open_repos().unwrap(), Vec::<PathBuf>::new());

    backend.open_checkout(&fixture.repo, &fixture.repo).unwrap();

    assert_eq!(
        backend
            .open_repos()
            .unwrap()
            .iter()
            .map(|root| canonical(root))
            .collect::<Vec<_>>(),
        [canonical(&fixture.repo)],
        "the space kiosk made is found"
    );
    assert!(fixture.app.state.lock().unwrap().listings > 0);
}

#[test]
fn each_space_folders_main_checkout_is_asked_once_until_the_window_changes() {
    let fixture = Fixture::new();
    fixture.app.add_space(&fixture.repo);
    let git = Arc::new(CountingGit::default());
    let backend = HerdrApp::new(
        Arc::clone(&fixture.app) as Arc<dyn AppHost>,
        Arc::clone(&git) as Arc<dyn GitProvider>,
        WorktreePlacement::new(fixture.temp.path().join("worktrees")).unwrap(),
        Arc::clone(&fixture.notices) as Arc<dyn Notices>,
        OnOpenConfig::default(),
        Arc::clone(&fixture.background),
        Arc::clone(&fixture.later) as Arc<dyn LaterNotices>,
    );

    backend.open_repos().unwrap();
    backend.open_repos().unwrap();
    assert_eq!(*git.main_checkouts.lock().unwrap(), 1);

    let other = fixture.temp.path().join("other");
    fs::create_dir(&other).unwrap();
    backend.open_folder(&other).unwrap();
    backend.open_repos().unwrap();
    assert_eq!(
        *git.main_checkouts.lock().unwrap(),
        3,
        "asked again for both spaces"
    );
}
