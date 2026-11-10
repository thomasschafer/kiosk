//! Whole sessions over a real loopback connection, driven the way a frontend drives them.

use std::{
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, TcpStream},
    path::Path,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use kiosk_core::{
    backend::{
        Opened,
        mock::{BackendCall, MockBackend},
    },
    config::{CoreConfig, LoadedConfig, NoExtension, SearchDirEntry},
    git::{GitProvider, Repo, mock::MockGitProvider},
    product::TEST_PRODUCT,
    tabs::TabId,
};
use kiosk_serve::{
    Connection,
    launch::{self, Handshake},
    protocol::{Outcome, PROTOCOL_VERSION},
    session::{self, Start},
};
use serde_json::{Value, json};

struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    fn send(&mut self, message: &Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).unwrap();
    }

    fn key(&mut self, keystroke: &str, key_char: Option<&str>) {
        self.send(&json!({ "t": "key", "keystroke": keystroke, "key_char": key_char }));
    }

    fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            let character = character.to_string();
            self.key(&character, Some(&character));
        }
    }

    /// The next message other than a heartbeat.
    fn receive(&mut self) -> Value {
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).unwrap();
            assert!(read > 0, "the session closed the connection");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["t"] != "ping" {
                return message;
            }
        }
    }

    /// The message that ends the session, skipping the views before it.
    fn end(&mut self) -> Value {
        loop {
            let message = self.receive();
            if message["t"] != "view" {
                return message;
            }
        }
    }

    /// Messages until one satisfies `wanted`, skipping the others.
    fn receive_until(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            let message = self.receive();
            if wanted(&message) {
                return message;
            }
        }
    }

    /// Views until one satisfies `wanted`, failing on any message that is not a view.
    fn view_until(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            let message = self.receive();
            assert_eq!(message["t"], "view", "{message}");
            if wanted(&message["view"]) {
                return message["view"].clone();
            }
        }
    }
}

type Session = JoinHandle<anyhow::Result<Outcome>>;

/// Starts a session for `start` and connects a client to it, past both hellos.
fn connect(
    start: impl FnOnce() -> Start<MockBackend, NoExtension> + Send + 'static,
) -> (Client, Session, Value) {
    let listening = launch::listen().unwrap();
    let Handshake { port, token, .. } = listening.handshake().clone();
    let session = thread::spawn(move || {
        let accepted = listening
            .accept(Duration::from_secs(5))?
            .ok_or_else(|| anyhow::anyhow!("no client connected"))?;
        let connection = Connection::new(accepted.stream)?;
        let result = session::run(&connection, &TEST_PRODUCT, start());
        if let Err(error) = &result {
            connection.fail("kiosk-test stopped", error);
        }
        result
    });
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut client = Client {
        reader: BufReader::new(stream.try_clone().unwrap()),
        writer: stream,
    };
    client.send(&json!({ "t": "hello", "token": token, "protocol": PROTOCOL_VERSION }));
    let hello = client.receive();
    assert_eq!(hello["t"], "hello", "{hello}");
    (client, session, hello)
}

fn repo(root: &Path, name: &str) -> Repo {
    Repo {
        name: name.into(),
        path: root.join(name),
        is_git: true,
        worktrees: Vec::new(),
    }
}

fn loaded(search_dir: &Path) -> LoadedConfig<NoExtension> {
    LoadedConfig {
        core: CoreConfig {
            search_dirs: vec![SearchDirEntry::Simple(search_dir.display().to_string())],
            experimental_tabs: true,
            ..CoreConfig::for_product(&TEST_PRODUCT).unwrap()
        },
        extension: NoExtension {},
        path: None,
        exists: true,
        warnings: Vec::new(),
    }
}

fn row_names(list: &Value) -> Vec<String> {
    list["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"]["text"].as_str().unwrap().to_owned())
        .collect()
}

/// What the tab that shows draws, if it is `tab`.
fn tab_body<'a>(view: &'a Value, tab: &str) -> Option<&'a Value> {
    (view["screen"] == "tabs" && view["body"]["tab"] == tab).then(|| &view["body"])
}

fn picker_list(view: &Value) -> Option<&Value> {
    tab_body(view, "picker")
        .filter(|body| body["main"]["state"] == "list")
        .map(|body| &body["main"])
}

fn two_repo_start(root: &Path, backend: Arc<MockBackend>) -> Start<MockBackend, NoExtension> {
    let git = MockGitProvider {
        repos: vec![repo(root, "alpha"), repo(root, "beta")],
        ..MockGitProvider::default()
    };
    Start {
        loaded: loaded(root),
        git: Arc::new(git) as Arc<dyn GitProvider>,
        backend: Some(backend),
        current_cwd: None,
        tab: TabId::Picker,
        preferences: None,
    }
}

fn mock_backend() -> Arc<MockBackend> {
    Arc::new(scripted_backend())
}

/// A backend with what a session that opens one checkout asks of it queued.
fn scripted_backend() -> MockBackend {
    let backend = MockBackend::native();
    for _ in 0..4 {
        backend
            .open_repos_results
            .lock()
            .unwrap()
            .push_back(Ok(Vec::new()));
        backend
            .open_folders_results
            .lock()
            .unwrap()
            .push_back(Ok(Vec::new()));
    }
    backend
        .open_checkout_results
        .lock()
        .unwrap()
        .push_back(Ok(Opened::default()));
    backend
}

#[test]
fn typing_filters_and_enter_opens_the_selection() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let backend = mock_backend();
    let session_backend = Arc::clone(&backend);
    let (mut client, session, hello) =
        connect(move || two_repo_start(&search_dir, session_backend));
    assert_eq!(hello["protocol"], PROTOCOL_VERSION);
    let chords = hello["chords"].as_array().unwrap();
    for chord in ["escape", "enter", "ctrl-h", "ctrl-c", "alt-backspace"] {
        assert!(chords.contains(&json!(chord)), "{chord} in {chords:?}");
    }
    assert!(
        !chords.contains(&json!("q")),
        "typed characters are not claimed"
    );

    client
        .send(&json!({ "t": "viewport", "list": 10, "bases": 5, "help": 10, "scan_warnings": 5, "diff": 20, "side_by_side": true }));
    let view = client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    assert_eq!(row_names(picker_list(&view).unwrap()), ["alpha", "beta"]);

    client.type_text("bet");
    // After "b" alone only beta matches, so wait for the whole query too.
    client.view_until(|view| {
        picker_list(view)
            .is_some_and(|list| row_names(list) == ["beta"] && list["query"]["before"] == "bet")
    });

    client.key("enter", None);
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "opened" }));
    assert_eq!(session.join().unwrap().unwrap(), Outcome::Opened);
    assert!(
        format!("{:?}", backend.calls()).contains("beta"),
        "{:?}",
        backend.calls()
    );
}

#[test]
fn enter_right_after_typing_opens_what_the_query_matches() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let backend = mock_backend();
    let session_backend = Arc::clone(&backend);
    let (mut client, session, _) = connect(move || two_repo_start(&search_dir, session_backend));
    client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    // Sent together, so the session has the enter before filtering for "bet" is done.
    client.type_text("bet");
    client.key("enter", None);
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "opened" }));
    assert_eq!(session.join().unwrap().unwrap(), Outcome::Opened);
    let calls = format!("{:?}", backend.calls());
    assert!(
        calls.contains("beta") && !calls.contains("alpha"),
        "{calls}"
    );
}

#[test]
fn clicking_a_row_opens_it() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let backend = mock_backend();
    let session_backend = Arc::clone(&backend);
    let (mut client, session, _) = connect(move || two_repo_start(&search_dir, session_backend));
    let view = client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    let id = picker_list(&view).unwrap()["rows"][1]["id"].clone();
    client.send(&json!({ "t": "activate", "position": 1, "id": id }));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "opened" }));
    assert_eq!(session.join().unwrap().unwrap(), Outcome::Opened);
    assert!(format!("{:?}", backend.calls()).contains("beta"));
}

#[test]
fn a_click_on_a_row_that_has_moved_does_nothing() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let backend = mock_backend();
    let session_backend = Arc::clone(&backend);
    let (mut client, session, _) = connect(move || two_repo_start(&search_dir, session_backend));
    let view = client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    let alpha = picker_list(&view).unwrap()["rows"][0]["id"].clone();
    client.type_text("bet");
    client.view_until(|view| picker_list(view).is_some_and(|list| row_names(list) == ["beta"]));
    // Position 0 showed alpha when drawn and shows beta now.
    client.send(&json!({ "t": "activate", "position": 0, "id": alpha }));
    client.send(&json!({ "t": "close" }));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    assert_eq!(session.join().unwrap().unwrap(), Outcome::Quit);
    assert!(
        !format!("{:?}", backend.calls()).contains("OpenCheckout"),
        "{:?}",
        backend.calls()
    );
}

#[test]
fn ctrl_u_clears_the_search_and_escape_quits() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (mut client, session, _) = connect(move || two_repo_start(&search_dir, mock_backend()));
    client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    client.type_text("al");
    client.view_until(|view| picker_list(view).is_some_and(|list| row_names(list) == ["alpha"]));
    client.key("ctrl-u", None);
    let view = client
        .view_until(|view| picker_list(view).is_some_and(|list| list["query"]["before"] == ""));
    assert_eq!(row_names(picker_list(&view).unwrap()), ["alpha", "beta"]);
    client.type_text("al");
    client.view_until(|view| picker_list(view).is_some_and(|list| row_names(list) == ["alpha"]));
    client.key("escape", None);
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    assert_eq!(session.join().unwrap().unwrap(), Outcome::Quit);
}

#[test]
fn closing_while_something_opens_ends_the_session_only_once_it_has_opened() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let backend = Arc::new(MockBackend {
        open_checkout_gate: Some(Arc::clone(&gate)),
        ..scripted_backend()
    });
    let session_backend = Arc::clone(&backend);
    let (mut client, session, _) = connect(move || two_repo_start(&search_dir, session_backend));
    client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    client.key("enter", None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !backend
        .calls()
        .iter()
        .any(|call| matches!(call, BackendCall::OpenCheckout { .. }))
    {
        assert!(Instant::now() < deadline, "the open never started");
        thread::sleep(Duration::from_millis(1));
    }

    client.send(&json!({ "t": "close" }));
    // A session that ended without waiting would have ended before this.
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    });

    assert_eq!(session.join().unwrap().unwrap(), Outcome::Quit);
    assert!(
        backend.open_checkout_results.lock().unwrap().is_empty(),
        "the session ended while the checkout was still opening"
    );
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    release.join().unwrap();
}

#[test]
fn a_client_that_goes_away_ends_the_session() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (client, session, _) = connect(move || two_repo_start(&search_dir, mock_backend()));
    drop(client);
    let outcome = session.join().unwrap();
    assert!(matches!(outcome, Ok(Outcome::Quit) | Err(_)), "{outcome:?}");
}

#[test]
fn an_invalid_message_stops_the_session_and_says_why() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (mut client, session, _) = connect(move || two_repo_start(&search_dir, mock_backend()));
    client.send(&json!({ "t": "nonsense" }));
    let error = loop {
        let message = client.receive();
        if message["t"] == "error" {
            break message;
        }
        assert_eq!(message["t"], "view", "{message}");
    };
    assert!(
        error["body"].as_str().unwrap().contains("invalid message"),
        "{error}"
    );
    assert!(session.join().unwrap().is_err());
}

#[test]
fn the_setup_wizard_writes_the_config_then_the_picker_starts() {
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.toml");
    let search_dir = root.path().join("code");
    std::fs::create_dir(&search_dir).unwrap();
    let start_config = config_path.clone();
    let start_dir = search_dir.clone();
    let (mut client, session, _) = connect(move || {
        let git = MockGitProvider {
            repos: vec![repo(&start_dir, "alpha")],
            ..MockGitProvider::default()
        };
        Start {
            loaded: LoadedConfig {
                core: CoreConfig::for_product(&TEST_PRODUCT).unwrap(),
                extension: NoExtension {},
                path: Some(start_config),
                exists: false,
                warnings: Vec::new(),
            },
            git: Arc::new(git) as Arc<dyn GitProvider>,
            backend: Some(mock_backend()),
            current_cwd: None,
            tab: TabId::Picker,
            preferences: None,
        }
    });
    let step = |view: &Value| view["step"].as_str().map(str::to_owned);
    let view = client.view_until(|view| view["screen"] == "setup");
    assert_eq!(step(&view).as_deref(), Some("welcome"));
    client.key("enter", None);
    client.view_until(|view| view["step"] == "directories");
    client.send(&json!({ "t": "paste", "text": search_dir.display().to_string() }));
    client.view_until(|view| {
        view["step"] == "directories"
            && !view["path"]["query"]["before"].as_str().unwrap().is_empty()
    });
    client.key("enter", None);
    let view = client.view_until(|view| view["depth"].is_object());
    assert_eq!(view["depth"]["query"]["before"], "1");
    client.key("enter", None);
    client.view_until(|view| view["step"] == "folder_mode");
    client.send(&json!({ "t": "folder_mode", "mode": "all_folders" }));
    let view = client.view_until(|view| view["options"][1]["selected"] == true);
    assert_eq!(view["options"][1]["mode"], "all_folders");
    client.key("enter", None);
    let view =
        client.view_until(|view| view["step"] == "directories" && view["added"][0].is_object());
    assert_eq!(view["added"][0]["all_folders"], true);
    client.key("enter", None);
    client.view_until(|view| view["step"] == "confirm");
    client.key("enter", None);
    let view = client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 1)
    });
    assert_eq!(row_names(picker_list(&view).unwrap()), ["alpha"]);
    let written = std::fs::read_to_string(&config_path).unwrap();
    assert!(written.contains("include_non_git = true"), "{written}");
    client.send(&json!({ "t": "close" }));
    assert_eq!(client.end()["outcome"], "quit");
    session.join().unwrap().unwrap();
}

#[test]
fn leaving_the_setup_wizard_quits_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.toml");
    let start_config = config_path.clone();
    let (mut client, session, _) = connect(move || Start {
        loaded: LoadedConfig {
            core: CoreConfig::for_product(&TEST_PRODUCT).unwrap(),
            extension: NoExtension {},
            path: Some(start_config),
            exists: false,
            warnings: Vec::new(),
        },
        git: Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>,
        backend: None,
        current_cwd: None,
        tab: TabId::Picker,
        preferences: None,
    });
    client.view_until(|view| view["step"] == "welcome");
    client.key("escape", None);
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
    assert!(!config_path.exists());
}

#[test]
fn a_config_without_search_directories_says_so_until_the_user_leaves() {
    let (mut client, session, _) = connect(|| Start {
        loaded: LoadedConfig {
            core: CoreConfig::for_product(&TEST_PRODUCT).unwrap(),
            extension: NoExtension {},
            path: Some("/config/kiosk.toml".into()),
            exists: true,
            warnings: Vec::new(),
        },
        git: Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>,
        backend: None,
        current_cwd: None,
        tab: TabId::Picker,
        preferences: None,
    });
    let view = client.view_until(|view| view["screen"] == "no_search_dirs");
    assert_eq!(view["title"], "No search directories configured");
    client.type_text("x");
    client.key("q", Some("q"));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
}

#[test]
fn tab_keys_and_clicks_move_between_tabs_and_esc_leaves_the_diff_tab() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (mut client, session, hello) = connect(move || two_repo_start(&search_dir, mock_backend()));
    let chords = hello["chords"].as_array().unwrap();
    for chord in ["ctrl-1", "ctrl-2", "ctrl-tab", "ctrl-pagedown"] {
        assert!(chords.contains(&json!(chord)), "{chord} unclaimed: {hello}");
    }
    let view = client.view_until(|view| picker_list(view).is_some());
    let bar = view["bar"].as_array().unwrap();
    assert_eq!(bar[0]["id"], "picker");
    assert_eq!(bar[0]["active"], true);
    assert_eq!(bar[1]["id"], "diff");
    assert_eq!(bar[1]["key"], "ctrl+2");

    client.key("ctrl-tab", None);
    let view = client.view_until(|view| tab_body(view, "diff").is_some());
    assert_eq!(view["bar"][1]["active"], true);
    client.send(&json!({ "t": "show_tab", "tab": "picker" }));
    client.view_until(|view| picker_list(view).is_some());
    client.key("ctrl-2", None);
    client.view_until(|view| tab_body(view, "diff").is_some());
    client.key("escape", None);
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
}

#[test]
fn the_picker_keeps_its_search_and_selection_while_another_tab_shows() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (mut client, session, _) = connect(move || two_repo_start(&search_dir, mock_backend()));
    client.type_text("a");
    let view = client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    let second = row_names(picker_list(&view).unwrap())[1].clone();
    client.key("down", None);
    client.view_until(|view| picker_list(view).is_some_and(|list| list["selected"] == 1));
    client.key("ctrl-2", None);
    client.view_until(|view| tab_body(view, "diff").is_some());
    client.type_text("x");
    client.key("ctrl-1", None);
    let view = client.view_until(|view| picker_list(view).is_some());
    let list = picker_list(&view).unwrap();
    assert_eq!(list["query"]["before"], "a");
    assert_eq!(list["selected"], 1);
    assert_eq!(row_names(list)[1], second);
    client.send(&json!({ "t": "close" }));
    assert_eq!(client.end()["outcome"], "quit");
    session.join().unwrap().unwrap();
}

#[test]
fn a_session_opens_on_the_tab_asked_for_and_launching_it_again_closes() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (mut client, session, _) = connect(move || Start {
        current_cwd: Some("/work/repo".into()),
        tab: TabId::Diff,
        preferences: None,
        ..two_repo_start(&search_dir, mock_backend())
    });
    let view = client.view_until(|view| tab_body(view, "diff").is_some());
    assert_eq!(view["body"]["title"], "Diff");
    let view = client.view_until(|view| {
        tab_body(view, "diff").is_some_and(|body| body["message"]["busy"] == false)
    });
    assert_eq!(view["body"]["message"]["text"], "Nothing to diff");
    client.send(&json!({ "t": "launch", "tab": "picker" }));
    client.view_until(|view| picker_list(view).is_some());
    client.send(&json!({ "t": "launch", "tab": "picker" }));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
}

#[test]
fn without_experimental_tabs_kiosk_is_only_the_picker_and_leaves_the_tab_keys() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (mut client, session, hello) = connect(move || {
        let mut start = two_repo_start(&search_dir, mock_backend());
        start.loaded.core.experimental_tabs = false;
        start.tab = TabId::Diff;
        start
    });
    let chords = hello["chords"].as_array().unwrap();
    for chord in ["ctrl-1", "ctrl-2", "ctrl-tab", "ctrl-pagedown"] {
        assert!(!chords.contains(&json!(chord)), "{chord} claimed: {hello}");
    }
    assert_eq!(hello["tabs"], json!(["picker", "diff"]));
    let view = client.view_until(|view| picker_list(view).is_some());
    assert_eq!(view["bar"], json!([]));
    client.send(&json!({ "t": "show_tab", "tab": "diff" }));
    client.send(&json!({ "t": "launch", "tab": "diff" }));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
}

#[test]
fn hello_lists_the_tabs_and_a_launch_during_setup_counts() {
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.toml");
    let start_config = config_path.clone();
    let (mut client, session, hello) = connect(move || Start {
        loaded: LoadedConfig {
            core: CoreConfig {
                experimental_tabs: true,
                ..CoreConfig::for_product(&TEST_PRODUCT).unwrap()
            },
            extension: NoExtension {},
            path: Some(start_config),
            exists: false,
            warnings: Vec::new(),
        },
        git: Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>,
        backend: None,
        current_cwd: None,
        tab: TabId::Picker,
        preferences: None,
    });
    assert_eq!(hello["tabs"], json!(["picker", "diff"]));
    client.view_until(|view| view["step"] == "welcome");
    client.send(&json!({ "t": "launch", "tab": "diff" }));
    client.send(&json!({ "t": "launch", "tab": "diff" }));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
    assert!(!config_path.exists());
}

#[test]
fn the_diff_tab_shows_a_checkouts_changes_and_moves_through_them() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let repo = root.path().join("work");
    std::fs::create_dir(&repo).unwrap();
    kiosk_core::git::test_fixtures::init_test_repo(&repo);
    std::fs::write(repo.join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(repo.join("b.rs"), "fn b() {}\n").unwrap();
    let (mut client, session, hello) = connect(move || Start {
        current_cwd: Some(repo),
        tab: TabId::Diff,
        preferences: None,
        ..two_repo_start(&search_dir, mock_backend())
    });
    for chord in ["ctrl-d", "ctrl-u", "pagedown"] {
        assert!(
            hello["chords"].as_array().unwrap().contains(&json!(chord)),
            "{chord} unclaimed: {hello}"
        );
    }
    client.send(&json!({ "t": "viewport", "list": 10, "bases": 5, "help": 10, "scan_warnings": 5, "diff": 3, "side_by_side": true }));
    let view = client.view_until(|view| {
        tab_body(view, "diff").is_some_and(|body| body["body"]["rows"].is_array())
    });
    let body = &view["body"];
    let files = body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(files, ["a.rs", "b.rs"]);
    assert_eq!(body["ranges"][0]["label"], "All changes");
    assert_eq!(body["ranges"][1]["label"], "Uncommitted");
    assert_eq!(body["body"]["rows"][0], json!(["file", 0]));
    assert_eq!(body["body"]["rows"].as_array().unwrap().len(), 3);
    let line = &body["body"]["rows"][2];
    assert_eq!(line[0], "line");
    assert_eq!(line[1], "+");
    assert_eq!(line[4], "fn a() {}");

    client.key("n", Some("n"));
    let view = client
        .view_until(|view| tab_body(view, "diff").is_some_and(|body| body["current_file"] == 1));
    assert_eq!(view["body"]["body"]["rows"][0], json!(["file", 1]));
    client.send(&json!({ "t": "diff", "click": { "on": "file", "index": 0 } }));
    client.view_until(|view| tab_body(view, "diff").is_some_and(|body| body["current_file"] == 0));
    client.send(&json!({ "t": "diff", "click": { "on": "toggle", "index": 0 } }));
    let view = client.view_until(|view| {
        tab_body(view, "diff").is_some_and(|body| body["files"][0]["collapsed"] == true)
    });
    assert_eq!(view["body"]["body"]["rows"][1][0], "note");
    client.key("escape", None);
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
}

#[test]
fn help_in_the_diff_tab_lists_its_keys_filters_and_closes() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (mut client, session, _) = connect(move || Start {
        tab: TabId::Diff,
        preferences: None,
        ..two_repo_start(&search_dir, mock_backend())
    });
    client.view_until(|view| tab_body(view, "diff").is_some());
    client.key("ctrl-h", None);
    let view = client.view_until(|view| {
        tab_body(view, "diff").is_some_and(|body| body["overlay"]["kind"] == "help")
    });
    let sections = view["body"]["overlay"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "section")
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(sections[0], "diff");
    client.type_text("toggle");
    let view = client.view_until(|view| {
        tab_body(view, "diff").is_some_and(|body| body["overlay"]["query"]["before"] == "toggle")
    });
    let commands = view["body"]["overlay"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["command"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(commands[0], "toggle_file");
    client.key("escape", None);
    client.view_until(|view| tab_body(view, "diff").is_some_and(|body| body["overlay"].is_null()));
    client.key("escape", None);
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
}

#[test]
fn opening_on_the_diff_tab_needs_no_search_directories() {
    let repo = tempfile::tempdir().unwrap();
    kiosk_core::git::test_fixtures::init_test_repo(repo.path());
    std::fs::write(repo.path().join("new.txt"), "new\n").unwrap();
    let cwd = repo.path().to_path_buf();
    let (mut client, session, _) = connect(move || Start {
        loaded: LoadedConfig {
            core: CoreConfig {
                experimental_tabs: true,
                ..CoreConfig::for_product(&TEST_PRODUCT).unwrap()
            },
            extension: NoExtension {},
            path: Some("/config/kiosk.toml".into()),
            exists: true,
            warnings: Vec::new(),
        },
        git: Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>,
        backend: None,
        current_cwd: Some(cwd),
        tab: TabId::Diff,
        preferences: None,
    });
    let view = client.view_until(|view| {
        tab_body(view, "diff").is_some_and(|body| {
            body["files"]
                .as_array()
                .is_some_and(|files| !files.is_empty())
        })
    });
    assert_eq!(view["body"]["files"][0]["path"], "new.txt");
    client.key("ctrl-1", None);
    client.view_until(|view| tab_body(view, "picker").is_some());
    client.send(&json!({ "t": "close" }));
    assert_eq!(client.end()["outcome"], "quit");
    session.join().unwrap().unwrap();
}

#[test]
fn s_switches_the_diff_tab_to_side_by_side_where_it_fits_and_remembers_it() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let repo = root.path().join("work");
    std::fs::create_dir(&repo).unwrap();
    kiosk_core::git::test_fixtures::init_test_repo(&repo);
    std::fs::write(repo.join("README.md"), "# changed\n").unwrap();
    let preferences = root.path().join("state/preferences.toml");
    let start_preferences = preferences.clone();
    let (mut client, session, _) = connect(move || Start {
        current_cwd: Some(repo),
        tab: TabId::Diff,
        preferences: Some(start_preferences),
        ..two_repo_start(&search_dir, mock_backend())
    });
    let viewport = |side_by_side| json!({ "t": "viewport", "list": 10, "bases": 5, "help": 10, "scan_warnings": 5, "diff": 20, "side_by_side": side_by_side });
    client.send(&viewport(true));
    let has_row = |kind: &'static str| {
        move |view: &Value| {
            tab_body(view, "diff").is_some_and(|body| {
                body["body"]["rows"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|row| row[0] == kind))
            })
        }
    };
    client.view_until(has_row("line"));
    client.key("s", Some("s"));
    let view = client.view_until(has_row("pair"));
    let pair = view["body"]["body"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[0] == "pair" && !row[1].is_null() && !row[2].is_null() && row[1][0] == "-")
        .unwrap()
        .clone();
    assert_eq!(pair[1][2], "# test");
    assert_eq!(pair[2][2], "# changed");
    assert!(
        std::fs::read_to_string(&preferences)
            .unwrap()
            .contains("side_by_side")
    );
    // Where side by side doesn't fit, the tab shows unified, keeping the choice.
    client.send(&viewport(false));
    client.view_until(has_row("line"));
    client.send(&viewport(true));
    client.view_until(has_row("pair"));
    client.send(&json!({ "t": "close" }));
    assert_eq!(client.end()["outcome"], "quit");
    session.join().unwrap().unwrap();
}

#[test]
fn preferences_kiosk_cant_read_are_reported_and_never_saved_over() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let preferences = root.path().join("preferences.toml");
    std::fs::write(&preferences, "diff_layout = \"sideways\"\n").unwrap();
    let start_preferences = preferences.clone();
    let (mut client, session, _) = connect(move || Start {
        tab: TabId::Diff,
        preferences: Some(start_preferences),
        ..two_repo_start(&search_dir, mock_backend())
    });
    let notice = client.receive_until(|message| message["t"] == "notify");
    assert_eq!(notice["title"], "Kiosk couldn't read its preferences");
    client.key("s", Some("s"));
    client.send(&json!({ "t": "close" }));
    assert_eq!(client.end()["outcome"], "quit");
    session.join().unwrap().unwrap();
    assert_eq!(
        std::fs::read_to_string(&preferences).unwrap(),
        "diff_layout = \"sideways\"\n"
    );
}
