//! tmux as kiosk's backend. Kiosk manages worktrees with git and shows each checkout or
//! folder in a tmux session. A session kiosk makes records its folder in a session option,
//! so it is matched by that whatever it is named or renamed to; other sessions are
//! matched by the folder they were started in. Opening switches the tmux client kiosk
//! runs in, or, outside tmux, attaches once the picker has closed.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use kiosk_core::{
    backend::{
        Annotations, Annotator, Backend, CheckoutState, ManagedWorktrees, Opened,
        WorktreePlacement, Worktrees,
        identity::{LiveFolder, session_in_folder, session_showing_checkout},
    },
    config::{OnOpenConfig, on_open::OnOpenPaneDirection},
    git::GitProvider,
};

use super::{
    SESSION_PATH_OPTION, Server, Session, SessionId, SessionPlace, Tmux, TmuxError, chain,
    path_arg, sessions,
};
use crate::{
    agent::{self, Detector},
    args,
    config::AgentConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Tmux(TmuxError),
    Git(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tmux(error) => error.fmt(formatter),
            Self::Git(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<TmuxError> for Error {
    fn from(error: TmuxError) -> Self {
        Self::Tmux(error)
    }
}

fn git_error(error: &anyhow::Error) -> Error {
    Error::Git(format!("{error:#}"))
}

/// Where kiosk runs relative to the tmux server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// In one of the server's panes or popups, so it can switch its own client.
    InsideServer,
    Outside,
}

impl Placement {
    pub fn of(server: &Server) -> Self {
        if server.is_ambient_server() {
            Self::InsideServer
        } else {
            Self::Outside
        }
    }
}

/// How kiosk shows a session to the user.
enum Client {
    /// Kiosk runs inside the server, so it switches its own client.
    Inside,
    /// Kiosk runs outside tmux; the session to attach to once the picker has closed, and
    /// the notices to print then.
    Outside {
        attach: Mutex<Option<SessionId>>,
        notices: Mutex<Vec<String>>,
    },
}

/// Where a session's folder is, as far as the picker's rows go.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Location {
    /// A checkout of the repository whose main checkout is `repo`.
    Checkout { repo: PathBuf, checkout: PathBuf },
    /// A folder outside any repository.
    Folder,
}

pub struct TmuxBackend {
    tmux: Arc<dyn Tmux>,
    git: Arc<dyn GitProvider>,
    placement: WorktreePlacement,
    on_open: OnOpenConfig,
    agent: AgentConfig,
    client: Client,
    /// Locations of session folders, which only change when worktrees do.
    locations: Mutex<HashMap<PathBuf, Location>>,
}

impl TmuxBackend {
    pub fn new(
        tmux: Arc<dyn Tmux>,
        runs: Placement,
        git: Arc<dyn GitProvider>,
        placement: WorktreePlacement,
        on_open: OnOpenConfig,
        agent: AgentConfig,
    ) -> Self {
        let client = match runs {
            Placement::InsideServer => Client::Inside,
            Placement::Outside => Client::Outside {
                attach: Mutex::default(),
                notices: Mutex::default(),
            },
        };
        Self {
            tmux,
            git,
            placement,
            on_open,
            agent,
            client,
            locations: Mutex::default(),
        }
    }

    /// The session to attach to now that the picker has closed, outside tmux.
    pub fn take_attach(&self) -> Option<SessionId> {
        match &self.client {
            Client::Inside => None,
            Client::Outside { attach, .. } => attach.lock().unwrap().take(),
        }
    }

    /// Notices that could not be shown while the picker was open.
    pub fn take_notices(&self) -> Vec<String> {
        match &self.client {
            Client::Inside => Vec::new(),
            Client::Outside { notices, .. } => std::mem::take(&mut notices.lock().unwrap()),
        }
    }

    fn focus(&self, session: &SessionId) -> Result<(), Error> {
        match &self.client {
            Client::Inside => {
                self.tmux
                    .run(args!["switch-client", "-t", session.as_str()])?;
            }
            Client::Outside { attach, .. } => *attach.lock().unwrap() = Some(session.clone()),
        }
        Ok(())
    }

    fn checkout_paths(&self, repo: &Path) -> Result<Vec<PathBuf>, Error> {
        Ok(self
            .git
            .list_worktrees(repo)
            .map_err(|error| git_error(&error))?
            .items
            .into_iter()
            .map(|worktree| worktree.path)
            .collect())
    }

    /// Makes a session in `folder`, named `name` or, if that is taken, `name-2` and so on,
    /// and returns it with its first pane.
    fn create(
        &self,
        sessions: &[Session],
        folder: &Path,
        name: &str,
    ) -> Result<(SessionId, String), Error> {
        let folder = path_arg("folder", folder)?;
        let name = unique_name(sessions, name);
        let created = self.tmux.run(args![
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_id} #{pane_id}",
            "-s",
            name,
            "-c",
            folder.clone()
        ])?;
        let (session, pane) = created.trim().split_once(' ').ok_or_else(|| {
            TmuxError::InvalidResponse(format!("new-session printed {created:?}"))
        })?;
        let session = SessionId::parse(session)?;
        self.tmux.run(args![
            "set-option",
            "-t",
            session.as_str(),
            SESSION_PATH_OPTION,
            folder
        ])?;
        Ok((session, pane.to_string()))
    }

    /// Splits each `[on_open]` pane off `pane` with its command typed into the new shell.
    /// Panes that fail are reported in the returned warning; the rest still open.
    fn open_panes(&self, pane: &str, folder: &Path) -> Result<Option<String>, Error> {
        let folder = path_arg("folder", folder)?;
        let mut errors = Vec::new();
        for (index, config) in self.on_open.panes.iter().enumerate() {
            let mut split = args![
                "split-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-t",
                pane,
                match config.direction {
                    OnOpenPaneDirection::Right => "-h",
                    OnOpenPaneDirection::Down => "-v",
                },
                "-c",
                folder.clone()
            ];
            if let Some(ratio) = config.ratio {
                split.extend(args!["-l", format!("{}%", percent(ratio))]);
            }
            let result = self.tmux.run(split).and_then(|created| {
                let new_pane = created.trim();
                self.tmux.run(chain([
                    args!["send-keys", "-t", new_pane, "-l", config.command.clone()],
                    args!["send-keys", "-t", new_pane, "Enter"],
                ]))
            });
            if let Err(error) = result {
                errors.push(format!("pane {} failed: {error}", index + 1));
            }
        }
        Ok((!errors.is_empty()).then(|| format!("on_open: {}", errors.join("; "))))
    }

    fn location(&self, folder: &Path) -> Result<Location, Error> {
        if let Some(location) = self.locations.lock().unwrap().get(folder) {
            return Ok(location.clone());
        }
        let location = match self
            .git
            .main_checkout(folder)
            .map_err(|error| git_error(&error))?
        {
            None => Location::Folder,
            Some(repo) => {
                let checkouts = self.checkout_paths(&repo)?;
                let place = [LiveFolder {
                    session: (),
                    folder: folder.to_path_buf(),
                }];
                let checkout = checkouts
                    .iter()
                    .find(|checkout| {
                        session_showing_checkout(&place, checkout, &checkouts).is_some()
                    })
                    .cloned()
                    .unwrap_or_else(|| repo.clone());
                Location::Checkout { repo, checkout }
            }
        };
        self.locations
            .lock()
            .unwrap()
            .insert(folder.to_path_buf(), location.clone());
        Ok(location)
    }
}

/// Sessions with a folder, the ones kiosk made first so they win over others in the same
/// folder.
fn live_folders(sessions: &[Session]) -> Vec<LiveFolder<SessionId>> {
    let mut live: Vec<_> = sessions
        .iter()
        .filter_map(|session| Some((session, session.place.as_ref()?)))
        .collect();
    live.sort_by_key(|(_, place)| !matches!(place, SessionPlace::Tagged(_)));
    live.into_iter()
        .map(|(session, place)| LiveFolder {
            session: session.id.clone(),
            folder: place.path().to_path_buf(),
        })
        .collect()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn percent(ratio: f64) -> u8 {
    (ratio * 100.0).round().clamp(1.0, 99.0) as u8
}

/// tmux turns `.` and `:` in session names into `_`; doing it first lets kiosk tell
/// whether a name is taken.
fn session_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| match character {
            '.' | ':' => '_',
            other => other,
        })
        .collect();
    if name.is_empty() {
        "kiosk".into()
    } else {
        name
    }
}

fn unique_name(sessions: &[Session], name: &str) -> String {
    let name = session_name(name);
    let taken: HashSet<&str> = sessions
        .iter()
        .map(|session| session.name.as_str())
        .collect();
    if !taken.contains(name.as_str()) {
        return name;
    }
    // Of these, at most one per existing session is taken.
    (2..=taken.len() + 2)
        .map(|suffix| format!("{name}-{suffix}"))
        .find(|candidate| !taken.contains(candidate.as_str()))
        .expect("an unused suffix")
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy())
        .into_owned()
}

impl Backend for TmuxBackend {
    type SessionId = SessionId;
    type Error = Error;

    fn open_repos(&self) -> Result<Vec<PathBuf>, Error> {
        let mut repos = Vec::new();
        for live in live_folders(&sessions(self.tmux.as_ref())?) {
            if let Location::Checkout { repo, .. } = self.location(&live.folder)?
                && !repos.contains(&repo)
            {
                repos.push(repo);
            }
        }
        Ok(repos)
    }

    fn open_folders(&self) -> Result<Vec<PathBuf>, Error> {
        Ok(live_folders(&sessions(self.tmux.as_ref())?)
            .into_iter()
            .map(|live| live.folder)
            .collect())
    }

    fn checkouts(&self, repo: &Path) -> Result<Vec<CheckoutState<SessionId>>, Error> {
        let checkouts = self.checkout_paths(repo)?;
        let live = live_folders(&sessions(self.tmux.as_ref())?);
        Ok(checkouts
            .iter()
            .map(|path| CheckoutState {
                path: path.clone(),
                session: session_showing_checkout(&live, path, &checkouts).cloned(),
            })
            .collect())
    }

    fn open_checkout(&self, repo: &Path, checkout: &Path) -> Result<Opened, Error> {
        let checkouts = self.checkout_paths(repo)?;
        let sessions = sessions(self.tmux.as_ref())?;
        if let Some(session) =
            session_showing_checkout(&live_folders(&sessions), checkout, &checkouts)
        {
            self.focus(session)?;
            return Ok(Opened::default());
        }
        let repo_name = folder_name(repo);
        let name = if kiosk_core::path::equivalent(
            &kiosk_core::path::canonical_or_original(repo),
            &kiosk_core::path::canonical_or_original(checkout),
        ) {
            repo_name
        } else {
            format!("{repo_name}/{}", folder_name(checkout))
        };
        let (session, pane) = self.create(&sessions, checkout, &name)?;
        let warning = self.open_panes(&pane, checkout)?;
        self.focus(&session)?;
        Ok(Opened { warning })
    }

    fn open_folder(&self, folder: &Path) -> Result<Opened, Error> {
        let sessions = sessions(self.tmux.as_ref())?;
        if let Some(session) = session_in_folder(&live_folders(&sessions), folder) {
            self.focus(session)?;
            return Ok(Opened::default());
        }
        let (session, pane) = self.create(&sessions, folder, &folder_name(folder))?;
        let warning = self.open_panes(&pane, folder)?;
        self.focus(&session)?;
        Ok(Opened { warning })
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), Error> {
        let message = format!("{title}: {body}");
        match &self.client {
            Client::Inside => {
                // The message is a format, in which `#` starts an expansion.
                self.tmux.run(args![
                    "display-message",
                    "-d",
                    "5000",
                    message.replace('#', "##")
                ])?;
            }
            Client::Outside { notices, .. } => notices.lock().unwrap().push(message),
        }
        Ok(())
    }

    fn worktrees(&self) -> Worktrees<'_, Self> {
        Worktrees::Managed(self)
    }

    fn annotator(&self) -> Option<&dyn Annotator<Self>> {
        self.agent.enabled.then_some(self as &dyn Annotator<Self>)
    }
}

impl ManagedWorktrees<Self> for TmuxBackend {
    fn placement(&self) -> &WorktreePlacement {
        &self.placement
    }

    fn close(&self, session: &SessionId) -> Result<(), Error> {
        self.tmux
            .run(args!["kill-session", "-t", session.as_str()])?;
        Ok(())
    }
}

impl Annotator<Self> for TmuxBackend {
    fn interval(&self) -> Duration {
        self.agent.poll_interval()
    }

    fn annotations(&self) -> Result<Annotations, Error> {
        let sessions = sessions(self.tmux.as_ref())?;
        let detector = Detector::new(&self.agent, self.tmux.as_ref());
        let mut repos: HashMap<PathBuf, Vec<_>> = HashMap::new();
        let mut checkouts: HashMap<PathBuf, Vec<_>> = HashMap::new();
        for session in &sessions {
            let Some(place) = &session.place else {
                continue;
            };
            let statuses = detector.session(session);
            if statuses.is_empty() {
                continue;
            }
            match self.location(place.path())? {
                Location::Checkout { repo, checkout } => {
                    repos.entry(repo).or_default().extend(statuses.clone());
                    checkouts.entry(checkout).or_default().extend(statuses);
                }
                Location::Folder => repos
                    .entry(place.path().to_path_buf())
                    .or_default()
                    .extend(statuses),
            }
        }
        let labelled = |map: HashMap<PathBuf, Vec<_>>| {
            map.into_iter()
                .map(|(path, mut statuses)| {
                    agent::sort(&mut statuses);
                    (path, agent::labels(&statuses))
                })
                .collect()
        };
        Ok(Annotations {
            repos: labelled(repos),
            checkouts: labelled(checkouts),
        })
    }
}

#[cfg(test)]
mod tests;
