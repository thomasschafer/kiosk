//! Talking to the tmux server: running commands and reading every session and pane in one
//! call.

use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

pub mod backend;
#[cfg(test)]
pub mod mock;

pub use backend::TmuxBackend;

/// Names a tmux socket below tmux's socket directory, as `tmux -L` does. Without it kiosk
/// uses the server tmux itself would pick: the one in `$TMUX`, or the default.
pub const SOCKET_VAR: &str = "KIOSK_TMUX_SOCKET";

/// The session option holding the checkout or folder a kiosk-made session shows.
pub const SESSION_PATH_OPTION: &str = "@kiosk_path";
/// The pane option set while a kiosk collector reads the pane's output.
pub const COLLECTOR_OPTION: &str = "@kiosk_collector";
/// The pane option holding the pane's most urgent OSC 7501 record.
pub const STATUS_OPTION: &str = "@kiosk_status";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxError {
    /// No tmux server is running on the socket.
    NoServer,
    Invocation(String),
    Failed {
        command: String,
        stderr: String,
    },
    InvalidResponse(String),
    InvalidArgument(String),
}

impl fmt::Display for TmuxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoServer => formatter.write_str("no tmux server is running"),
            Self::Invocation(message)
            | Self::InvalidResponse(message)
            | Self::InvalidArgument(message) => formatter.write_str(message),
            Self::Failed { command, stderr } => {
                write!(formatter, "tmux {command} failed: {stderr}")
            }
        }
    }
}

impl std::error::Error for TmuxError {}

/// Runs tmux. One call is one tmux invocation, which can hold several commands separated
/// by `;` arguments.
pub trait Tmux: Send + Sync {
    /// Returns standard output.
    fn run(&self, args: Vec<String>) -> Result<String, TmuxError>;
}

/// Which tmux server to talk to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    Ambient,
    /// A socket name below tmux's socket directory (`tmux -L`).
    Named(String),
    /// A socket path (`tmux -S`).
    Path(PathBuf),
}

impl Server {
    pub fn from_env() -> Self {
        std::env::var(SOCKET_VAR)
            .ok()
            .filter(|name| !name.is_empty())
            .map_or(Self::Ambient, Self::Named)
    }

    /// Whether this process runs inside a pane of this server, so it can switch the client
    /// it runs in rather than attach one.
    pub fn is_ambient_server(&self) -> bool {
        let Some(tmux) = std::env::var_os("TMUX").filter(|value| !value.is_empty()) else {
            return false;
        };
        let tmux = tmux.to_string_lossy();
        let Some(ambient_socket) = tmux.split(',').next().map(Path::new) else {
            return false;
        };
        match self {
            Self::Ambient => true,
            Self::Named(name) => ambient_socket
                .file_name()
                .is_some_and(|file| file.to_string_lossy() == *name),
            Self::Path(path) => kiosk_core::path::equivalent(
                &kiosk_core::path::canonical_or_original(path),
                &kiosk_core::path::canonical_or_original(ambient_socket),
            ),
        }
    }

    /// The arguments that select this server, to put before a command.
    pub fn args(&self) -> Vec<String> {
        match self {
            Self::Ambient => Vec::new(),
            Self::Named(name) => vec!["-L".into(), name.clone()],
            Self::Path(path) => vec!["-S".into(), path.to_string_lossy().into_owned()],
        }
    }
}

#[derive(Debug, Clone)]
pub struct CliTmux {
    server: Server,
}

impl CliTmux {
    pub fn new(server: Server) -> Self {
        Self { server }
    }

    pub fn server(&self) -> &Server {
        &self.server
    }

    /// A tmux command for this server, for running interactively (attaching).
    pub fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.args(self.server.args());
        command
    }
}

impl Tmux for CliTmux {
    fn run(&self, args: Vec<String>) -> Result<String, TmuxError> {
        let output = self
            .command()
            .args(&args)
            .output()
            .map_err(|error| TmuxError::Invocation(format!("could not run tmux: {error}")))?;
        if output.status.success() {
            return String::from_utf8(output.stdout).map_err(|error| {
                TmuxError::InvalidResponse(format!("tmux printed invalid UTF-8: {error}"))
            });
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if is_no_server(&stderr) {
            return Err(TmuxError::NoServer);
        }
        Err(TmuxError::Failed {
            command: args.first().cloned().unwrap_or_default(),
            stderr,
        })
    }
}

fn is_no_server(stderr: &str) -> bool {
    stderr.starts_with("no server running") || stderr.starts_with("error connecting to")
}

/// Builds the argument list of one invocation running `commands` in order.
pub fn chain(commands: impl IntoIterator<Item = Vec<String>>) -> Vec<String> {
    let mut args = Vec::new();
    for command in commands {
        if !args.is_empty() {
            args.push(";".into());
        }
        args.extend(command);
    }
    args
}

/// Turns string literals into an owned argument list.
#[macro_export]
macro_rules! args {
    ($($arg:expr),* $(,)?) => {
        vec![$(::std::string::String::from($arg)),*]
    };
}

/// A tmux session id such as `$3`. Unlike a name, it cannot be changed or reused while the
/// server runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    pub fn parse(id: &str) -> Result<Self, TmuxError> {
        let valid = id
            .strip_prefix('$')
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
        if valid {
            Ok(Self(id.into()))
        } else {
            Err(TmuxError::InvalidResponse(format!(
                "{id:?} is not a tmux session id"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A tmux pane id such as `%7`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(String);

impl PaneId {
    pub fn parse(id: &str) -> Result<Self, TmuxError> {
        let valid = id
            .strip_prefix('%')
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
        if valid {
            Ok(Self(id.into()))
        } else {
            Err(TmuxError::InvalidArgument(format!(
                "{id:?} is not a tmux pane id"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a session is, which is how kiosk matches sessions to checkouts and folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPlace {
    /// Kiosk made the session for this checkout or folder.
    Tagged(PathBuf),
    /// Someone else made the session; this is the directory it was started in.
    Started(PathBuf),
}

impl SessionPlace {
    pub fn path(&self) -> &Path {
        match self {
            Self::Tagged(path) | Self::Started(path) => path,
        }
    }
}

/// Who reads a pane's output through tmux's one `pipe-pane`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanePipe {
    None,
    Kiosk,
    /// The user's own pipe, which kiosk leaves alone.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: PaneId,
    pub pid: u32,
    pub current_command: String,
    pub title: String,
    pub dead: bool,
    pub pipe: PanePipe,
    /// The collector's summary of the pane's OSC 7501 records, if it has any.
    pub program_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    /// Unix time of the session's last activity.
    pub activity: u64,
    pub place: Option<SessionPlace>,
    pub panes: Vec<Pane>,
}

pub(crate) const FIELD_SEPARATOR: char = '\u{1f}';
const SNAPSHOT_FIELDS: &[&str] = &[
    "#{session_id}",
    "#{session_name}",
    "#{session_activity}",
    "#{session_path}",
    "#{@kiosk_path}",
    "#{pane_id}",
    "#{pane_pid}",
    "#{pane_dead}",
    "#{pane_pipe}",
    "#{@kiosk_collector}",
    "#{@kiosk_status}",
    "#{pane_current_command}",
    // Last, so a separator in a title cannot shift the other fields.
    "#{pane_title}",
];

/// Every session on the server with its panes, in tmux's order. No server means no
/// sessions.
pub fn sessions(tmux: &dyn Tmux) -> Result<Vec<Session>, TmuxError> {
    let format = SNAPSHOT_FIELDS.join(&FIELD_SEPARATOR.to_string());
    let output = match tmux.run(args!["list-panes", "-a", "-F", format]) {
        Ok(output) => output,
        Err(TmuxError::NoServer) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut sessions: Vec<Session> = Vec::new();
    for line in output.lines().filter(|line| !line.is_empty()) {
        let fields: Vec<&str> = line
            .splitn(SNAPSHOT_FIELDS.len(), FIELD_SEPARATOR)
            .collect();
        let [
            session_id,
            name,
            activity,
            session_path,
            kiosk_path,
            pane_id,
            pid,
            dead,
            pipe,
            collector,
            status,
            command,
            title,
        ] = fields[..]
        else {
            return Err(TmuxError::InvalidResponse(format!(
                "unexpected list-panes line {line:?}"
            )));
        };
        let id = SessionId::parse(session_id)?;
        let pane = Pane {
            id: PaneId::parse(pane_id)
                .map_err(|error| TmuxError::InvalidResponse(error.to_string()))?,
            pid: parse_number(pid, "pane pid")?,
            current_command: command.into(),
            title: title.into(),
            dead: flag(dead, "pane_dead")?,
            pipe: match (flag(pipe, "pane_pipe")?, collector.is_empty()) {
                (false, _) => PanePipe::None,
                (true, false) => PanePipe::Kiosk,
                (true, true) => PanePipe::Other,
            },
            program_status: (!status.is_empty()).then(|| status.to_string()),
        };
        if let Some(session) = sessions.last_mut().filter(|session| session.id == id) {
            session.panes.push(pane);
            continue;
        }
        let place = if kiosk_path.is_empty() {
            (!session_path.is_empty()).then(|| SessionPlace::Started(session_path.into()))
        } else {
            Some(SessionPlace::Tagged(kiosk_path.into()))
        };
        sessions.push(Session {
            id,
            name: name.into(),
            activity: parse_number(activity, "session activity")?,
            place,
            panes: vec![pane],
        });
    }
    Ok(sessions)
}

fn parse_number<T: std::str::FromStr>(value: &str, what: &str) -> Result<T, TmuxError> {
    value
        .parse()
        .map_err(|_| TmuxError::InvalidResponse(format!("{what} {value:?} is not a number")))
}

fn flag(value: &str, what: &str) -> Result<bool, TmuxError> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(TmuxError::InvalidResponse(format!(
            "{what} {value:?} is not 0 or 1"
        ))),
    }
}

/// A path as one tmux argument. Control characters are refused: tmux would show or store
/// them escaped, and kiosk could no longer match the session to its folder.
pub fn path_arg(name: &str, path: &Path) -> Result<String, TmuxError> {
    if !path.is_absolute() {
        return Err(TmuxError::InvalidArgument(format!(
            "{name} must be absolute: {}",
            path.display()
        )));
    }
    let Some(value) = path.to_str() else {
        return Err(TmuxError::InvalidArgument(format!(
            "{name} is not valid UTF-8 and cannot be passed to tmux"
        )));
    };
    if value.chars().any(char::is_control) {
        return Err(TmuxError::InvalidArgument(format!(
            "{name} contains control characters: {}",
            kiosk_core::display::sanitize(value)
        )));
    }
    Ok(value.into())
}

#[cfg(test)]
mod tests {
    use super::{mock::MockTmux, *};

    fn line(fields: &[&str]) -> String {
        fields.join(&FIELD_SEPARATOR.to_string())
    }

    #[test]
    fn panes_are_grouped_by_session_with_their_place_and_pipe() {
        let tmux = MockTmux::default();
        tmux.respond(Ok(format!(
            "{}\n{}\n{}\n",
            line(&[
                "$1",
                "repo",
                "100",
                "/start",
                "/repo",
                "%1",
                "10",
                "0",
                "1",
                "123",
                "state=idle",
                "zsh",
                "t\u{1f}itle"
            ]),
            line(&[
                "$1", "repo", "100", "/start", "/repo", "%2", "11", "1", "1", "", "", "claude", ""
            ]),
            line(&[
                "$2", "notes", "50", "/notes", "", "%3", "12", "0", "0", "", "", "zsh", "x"
            ]),
        )));
        let sessions = sessions(&tmux).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0].place,
            Some(SessionPlace::Tagged("/repo".into()))
        );
        assert_eq!(sessions[0].panes[0].pipe, PanePipe::Kiosk);
        assert_eq!(sessions[0].panes[0].title, "t\u{1f}itle");
        assert_eq!(
            sessions[0].panes[0].program_status.as_deref(),
            Some("state=idle")
        );
        assert_eq!(sessions[0].panes[1].pipe, PanePipe::Other);
        assert!(sessions[0].panes[1].dead);
        assert_eq!(
            sessions[1].place,
            Some(SessionPlace::Started("/notes".into()))
        );
        assert_eq!(sessions[1].panes[0].pipe, PanePipe::None);
        assert_eq!(sessions[1].activity, 50);
    }

    #[test]
    fn no_server_means_no_sessions_but_garbage_is_an_error() {
        let tmux = MockTmux::default();
        tmux.respond(Err(TmuxError::NoServer));
        assert_eq!(sessions(&tmux).unwrap(), []);
        tmux.respond(Ok("not a pane line\n".into()));
        assert!(matches!(
            sessions(&tmux),
            Err(TmuxError::InvalidResponse(_))
        ));
    }

    #[test]
    fn ids_and_paths_are_validated() {
        assert!(SessionId::parse("$12").is_ok());
        assert!(SessionId::parse("12").is_err());
        assert!(PaneId::parse("%3").is_ok());
        assert!(PaneId::parse("%").is_err());
        assert!(path_arg("folder", Path::new("relative")).is_err());
        let root = std::env::temp_dir();
        assert!(path_arg("folder", &root.join("a\nb")).is_err());
        let spaced = root.join("a b");
        assert_eq!(
            path_arg("folder", &spaced).unwrap(),
            spaced.to_str().unwrap()
        );
    }

    #[test]
    fn commands_chain_with_separate_semicolons() {
        assert_eq!(
            chain([args!["a", "1"], args!["b"]]),
            args!["a", "1", ";", "b"]
        );
    }
}
