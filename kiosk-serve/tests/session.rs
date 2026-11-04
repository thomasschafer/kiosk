//! Whole sessions over a real loopback connection, driven the way a frontend drives them.

use std::{
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, TcpStream},
    path::Path,
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use kiosk_core::{
    backend::{Opened, mock::MockBackend},
    config::{CoreConfig, LoadedConfig, NoExtension, SearchDirEntry},
    git::{GitProvider, Repo, mock::MockGitProvider},
    product::TEST_PRODUCT,
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
        let accepted = listening.accept(Duration::from_secs(5))?;
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
            ..CoreConfig::default()
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

fn picker_list(view: &Value) -> Option<&Value> {
    (view["screen"] == "picker" && view["main"]["state"] == "list").then(|| &view["main"])
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
    }
}

fn mock_backend() -> Arc<MockBackend> {
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
    Arc::new(backend)
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
        .send(&json!({ "t": "viewport", "list": 10, "bases": 5, "help": 10, "scan_warnings": 5 }));
    let view = client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    assert_eq!(row_names(picker_list(&view).unwrap()), ["alpha", "beta"]);

    client.type_text("bet");
    let view =
        client.view_until(|view| picker_list(view).is_some_and(|list| row_names(list) == ["beta"]));
    assert_eq!(picker_list(&view).unwrap()["query"]["before"], "bet");

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
fn escape_reaches_the_picker_and_closing_quits() {
    let root = tempfile::tempdir().unwrap();
    let search_dir = root.path().to_path_buf();
    let (mut client, session, _) = connect(move || two_repo_start(&search_dir, mock_backend()));
    client.view_until(|view| {
        picker_list(view).is_some_and(|list| list["rows"].as_array().unwrap().len() == 2)
    });
    client.type_text("al");
    client.view_until(|view| picker_list(view).is_some_and(|list| row_names(list) == ["alpha"]));
    client.key("escape", None);
    let view = client
        .view_until(|view| picker_list(view).is_some_and(|list| list["query"]["before"] == ""));
    assert_eq!(row_names(picker_list(&view).unwrap()), ["alpha", "beta"]);
    client.send(&json!({ "t": "close" }));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    assert_eq!(session.join().unwrap().unwrap(), Outcome::Quit);
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
                core: CoreConfig::default(),
                extension: NoExtension {},
                path: Some(start_config),
                exists: false,
                warnings: Vec::new(),
            },
            git: Arc::new(git) as Arc<dyn GitProvider>,
            backend: Some(mock_backend()),
            current_cwd: None,
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
            core: CoreConfig::default(),
            extension: NoExtension {},
            path: Some(start_config),
            exists: false,
            warnings: Vec::new(),
        },
        git: Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>,
        backend: None,
        current_cwd: None,
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
            core: CoreConfig::default(),
            extension: NoExtension {},
            path: Some("/config/kiosk.toml".into()),
            exists: true,
            warnings: Vec::new(),
        },
        git: Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>,
        backend: None,
        current_cwd: None,
    });
    let view = client.view_until(|view| view["screen"] == "no_search_dirs");
    assert_eq!(view["title"], "No search directories configured");
    client.type_text("x");
    client.key("q", Some("q"));
    assert_eq!(client.end(), json!({ "t": "exit", "outcome": "quit" }));
    session.join().unwrap().unwrap();
}
