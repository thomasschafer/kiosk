//! The Herdr desktop app as a backend. The app has no worktree API, so kiosk does the
//! Git work and this backend drives the window through the `herdr-app` CLI (spaces,
//! splits, focus) and herdr-core's `hc` (terminal folders, typing into terminals). Kiosk sessions are app
//! spaces: a space counts as showing a checkout when its first terminal's live folder is
//! inside it, the rule the app's own `worktree` mod uses.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use kiosk_core::{
    backend::{
        Backend, CheckoutState, ManagedWorktrees, Opened, WorktreePlacement, Worktrees,
        identity::{LiveFolder, session_showing_checkout},
    },
    git::GitProvider,
};

use crate::{
    background::Background,
    error::{AppError, PaneOpenError, checked},
    notices::LaterNotices,
};
use kiosk_core::{
    backend::{
        args,
        identity::focus_or_create_folder,
        on_open::{self, PaneSplitRequest, PaneSplitResponse},
    },
    config::OnOpenConfig,
};

/// Where new checkouts go, below the user's home: the same place as the herdr
/// TUI's default `worktrees.directory` and the app's `worktree` mod.
pub const WORKTREE_ROOT: &str = ".herdr/worktrees";

/// The two programs the app backend runs.
pub trait AppHost: Send + Sync {
    /// Runs `herdr-app` and returns its standard output.
    fn herdr_app(&self, args: &[String]) -> Result<Vec<u8>, AppError>;
    /// Runs herdr-core's `hc` and returns its standard output.
    fn hc(&self, args: &[String]) -> Result<Vec<u8>, AppError>;
}

#[derive(Debug, Clone)]
pub struct CliAppHost {
    herdr_app: PathBuf,
    hc: PathBuf,
    /// The app's home, which selects the window `herdr-app` drives. Without it, `herdr-app`
    /// uses its default.
    app_home: Option<PathBuf>,
}

impl CliAppHost {
    pub fn new(
        herdr_app: impl Into<PathBuf>,
        hc: impl Into<PathBuf>,
        app_home: Option<PathBuf>,
    ) -> Self {
        Self {
            herdr_app: herdr_app.into(),
            hc: hc.into(),
            app_home,
        }
    }
}

/// Set for `herdr-app` so it drives the window the picker was opened from.
const APP_HOME_VAR: &str = "HERDR_APP_HOME";

fn run_program(mut command: Command, binary: &Path) -> Result<std::process::Output, AppError> {
    command.output().map_err(|error| {
        AppError::Invocation(format!("could not execute {}: {error}", binary.display()))
    })
}

fn exit_failure(binary: &Path, output: &std::process::Output) -> AppError {
    AppError::Invocation(format!(
        "{} exited with {}: {}",
        binary.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

impl AppHost for CliAppHost {
    fn herdr_app(&self, args: &[String]) -> Result<Vec<u8>, AppError> {
        let mut command = Command::new(&self.herdr_app);
        command.args(args);
        if let Some(home) = &self.app_home {
            command.env(APP_HOME_VAR, home);
        }
        let output = run_program(command, &self.herdr_app)?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        // `herdr-app do` reports refusals as `{ "error": "..." }` on stdout.
        match serde_json::from_slice::<ErrorReply>(&output.stdout) {
            Ok(ErrorReply { error }) => Err(AppError::Other {
                code: "herdr_app".into(),
                message: error,
            }),
            Err(_) => Err(exit_failure(&self.herdr_app, &output)),
        }
    }

    fn hc(&self, args: &[String]) -> Result<Vec<u8>, AppError> {
        let mut command = Command::new(&self.hc);
        command.args(args);
        let output = run_program(command, &self.hc)?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        match serde_json::from_slice::<HcError>(&output.stderr) {
            Ok(HcError { error }) => Err(AppError::Other {
                code: error.kind,
                message: error.message,
            }),
            Err(_) => Err(exit_failure(&self.hc, &output)),
        }
    }
}

#[derive(Deserialize)]
struct ErrorReply {
    error: String,
}

#[derive(Deserialize)]
struct HcError {
    error: HcErrorBody,
}

#[derive(Deserialize)]
struct HcErrorBody {
    kind: String,
    message: String,
}

#[derive(Deserialize)]
struct AppList {
    spaces: Vec<AppSpace>,
}

#[derive(Deserialize)]
struct AppSpace {
    id: String,
    #[serde(default)]
    machine: Option<String>,
    tabs: Vec<AppTab>,
}

#[derive(Deserialize)]
struct AppTab {
    terminals: Vec<AppTerminal>,
}

#[derive(Deserialize)]
struct AppTerminal {
    terminal: String,
    #[serde(default)]
    pane: Option<String>,
}

#[derive(Deserialize)]
struct SpaceCreated {
    space: String,
    terminal: String,
}

#[derive(Deserialize)]
struct PaneCreated {
    pane: String,
    terminal: String,
}

#[derive(Deserialize)]
struct HcTerminal {
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    foreground: Option<HcForeground>,
}

#[derive(Deserialize)]
struct HcForeground {
    #[serde(default)]
    cwd: Option<PathBuf>,
}

/// A space on this machine, with the live folder of each of its panes' terminals in the
/// app's order. The app's `kiosk` mod passes these with `--spaces`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Space {
    pub id: String,
    pub folders: Vec<Option<PathBuf>>,
}

impl Space {
    /// Where the space is: the folder of its first pane with a known one.
    fn place(&self) -> Option<PathBuf> {
        self.folders
            .iter()
            .flatten()
            .next()
            .map(|folder| kiosk_core::path::canonical_or_original(folder))
    }
}

/// An app space id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpaceId(pub String);

/// Each space with a known folder and where it is.
fn space_places(spaces: &[Space]) -> Vec<LiveFolder<SpaceId>> {
    spaces
        .iter()
        .filter_map(|space| {
            Some(LiveFolder {
                session: SpaceId(space.id.clone()),
                folder: space.place()?,
            })
        })
        .collect()
}

pub struct HerdrApp {
    host: Arc<dyn AppHost>,
    git: Arc<dyn GitProvider>,
    placement: WorktreePlacement,
    notices: Arc<dyn Notices>,
    on_open: OnOpenConfig,
    /// Where the commands of a repository's `[on_open]` panes are typed, after the picker has
    /// reported it opened, so the popup closes without waiting for their shells to start.
    background: Arc<Background>,
    /// Where failures to type them go, as the popup has closed by then.
    later: Arc<dyn LaterNotices>,
    /// The spaces as the mod saw them when it started kiosk, used instead of asking the app
    /// (one `herdr-app ls` and an `hc get` per terminal, which add up where starting a
    /// process is slow) until kiosk changes the window itself. The mod's popup is modal, so
    /// only a terminal changing folder while kiosk is open can make them stale.
    snapshot: Mutex<Option<Vec<Space>>>,
    /// Each space folder's main checkout, as Git reported it since kiosk last changed the
    /// window.
    main_checkouts: Mutex<HashMap<PathBuf, Option<PathBuf>>>,
}

/// Where the app backend's notifications go: the app mod, which shows them as the app's own.
pub trait Notices: Send + Sync {
    fn post(&self, title: &str, body: &str) -> Result<(), String>;
}

impl Notices for kiosk_serve::Notifier {
    fn post(&self, title: &str, body: &str) -> Result<(), String> {
        self.notify(title, body, kiosk_serve::NoticeLevel::Warning)
            .map_err(|error| format!("{error:#}"))
    }
}

/// An `[on_open]` pane's command, to be typed into the pane's new terminal by
/// `type_commands`.
#[derive(Debug, PartialEq, Eq)]
struct Typing {
    /// The pane's place in `[on_open]`, from 1.
    pane: usize,
    terminal: String,
    command: String,
}

/// Types each command into its terminal, all at once, as each waits for its new shell to
/// start. Returns a warning for those that didn't reach their terminal.
fn type_commands(host: &dyn AppHost, typing: &[Typing]) -> Option<String> {
    let errors: Vec<String> = std::thread::scope(|scope| {
        let sends: Vec<_> = typing
            .iter()
            .map(|typing| {
                let send = scope.spawn(move || {
                    host.hc(&[
                        "send".into(),
                        "--".into(),
                        typing.terminal.clone(),
                        typing.command.clone(),
                    ])
                });
                (typing.pane, send)
            })
            .collect();
        sends
            .into_iter()
            .filter_map(|(pane, send)| match send.join() {
                Ok(Ok(_)) => None,
                Ok(Err(error)) => Some(format!("pane {pane} run failed: {error}")),
                Err(panic) => std::panic::resume_unwind(panic),
            })
            .collect()
    });
    (!errors.is_empty()).then(|| format!("on_open: {}", errors.join("; ")))
}

/// The title of the notice for `[on_open]` commands that weren't typed.
const ON_OPEN_FAILED: &str = "Kiosk couldn't run every [on_open] command";

impl HerdrApp {
    /// New checkouts go in `placement`, notifications go to `notices`, and `[on_open]`
    /// commands are typed in `background`, with their failures going to `later`.
    pub fn new(
        host: Arc<dyn AppHost>,
        git: Arc<dyn GitProvider>,
        placement: WorktreePlacement,
        notices: Arc<dyn Notices>,
        on_open: OnOpenConfig,
        background: Arc<Background>,
        later: Arc<dyn LaterNotices>,
    ) -> Self {
        Self {
            host,
            git,
            placement,
            notices,
            on_open,
            background,
            later,
            snapshot: Mutex::new(None),
            main_checkouts: Mutex::new(HashMap::new()),
        }
    }

    /// Starts from the spaces the mod saw, rather than asking the app for them.
    #[must_use]
    pub fn with_spaces(self, spaces: Vec<Space>) -> Self {
        *self.snapshot.lock().expect("snapshot lock") = Some(spaces);
        self
    }

    fn command(&self, name: &str, args: &Value) -> Result<Vec<u8>, AppError> {
        // The window is about to change, so what was known about it no longer holds.
        self.snapshot.lock().expect("snapshot lock").take();
        self.main_checkouts
            .lock()
            .expect("main checkouts lock")
            .clear();
        self.host
            .herdr_app(&["do".into(), name.into(), args.to_string()])
    }

    fn command_json<T: DeserializeOwned>(&self, name: &str, args: &Value) -> Result<T, AppError> {
        let output = self.command(name, args)?;
        parse(&output, &format!("herdr-app do {name}"))
    }

    fn list(&self) -> Result<AppList, AppError> {
        let output = self.host.herdr_app(&["ls".into()])?;
        parse(&output, "herdr-app ls")
    }

    fn terminal(&self, address: &str) -> Result<HcTerminal, AppError> {
        let output = self
            .host
            .hc(&["get".into(), address.into(), "--json".into()])?;
        parse(&output, "hc get")
    }

    /// The spaces on this machine. Spaces on other machines are left out: their
    /// folders are not paths here.
    fn spaces(&self) -> Result<Vec<Space>, AppError> {
        if let Some(spaces) = self.snapshot.lock().expect("snapshot lock").clone() {
            return Ok(spaces);
        }
        let spaces: Vec<(String, Vec<String>)> = self
            .list()?
            .spaces
            .into_iter()
            .filter(|space| space.machine.is_none())
            .map(|space| {
                let terminals = space
                    .tabs
                    .into_iter()
                    .flat_map(|tab| tab.terminals)
                    .filter(|terminal| terminal.pane.is_some())
                    .map(|terminal| terminal.terminal)
                    .collect();
                (space.id, terminals)
            })
            .collect();
        let terminals: Vec<&str> = spaces
            .iter()
            .flat_map(|(_, terminals)| terminals.iter().map(String::as_str))
            .collect();
        let mut folders = self.live_folders(&terminals)?.into_iter();
        Ok(spaces
            .into_iter()
            .map(|(id, terminals)| Space {
                id,
                folders: terminals
                    .iter()
                    .map(|_| folders.next().expect("a folder for every terminal"))
                    .collect(),
            })
            .collect())
    }

    /// Each terminal's live folder, in order. One `hc get` each, all at once:
    /// one after another they add up to most of the time an open takes.
    fn live_folders(&self, terminals: &[&str]) -> Result<Vec<Option<PathBuf>>, AppError> {
        std::thread::scope(|scope| {
            let lookups: Vec<_> = terminals
                .iter()
                .map(|terminal| {
                    scope.spawn(move || {
                        let info = self.terminal(terminal)?;
                        Ok(info.foreground.and_then(|f| f.cwd).or(info.cwd))
                    })
                })
                .collect();
            lookups
                .into_iter()
                .map(|lookup| {
                    lookup
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect()
        })
    }

    /// Makes a space whose first terminal starts in `folder`, shown at once. It is
    /// left unnamed so the app names it after its live folder, as its own
    /// `worktree` mod does (which also nests worktree spaces under their repo).
    /// Returns the space and its first pane.
    fn open_space(&self, folder: &Path) -> Result<(String, String), AppError> {
        let folder = checked(args::utf8_path("folder", folder))?;
        let created: SpaceCreated = self.command_json("space.new", &json!({ "folder": folder }))?;
        let pane = self
            .list()?
            .spaces
            .into_iter()
            .filter(|space| space.id == created.space)
            .flat_map(|space| space.tabs)
            .flat_map(|tab| tab.terminals)
            .find(|terminal| terminal.terminal == created.terminal)
            .and_then(|terminal| terminal.pane)
            .ok_or_else(|| {
                AppError::InvalidResponse(format!(
                    "the new space {} has no pane for its terminal {}",
                    created.space, created.terminal
                ))
            })?;
        Ok((created.space, pane))
    }

    fn select_space(&self, id: &str) -> Result<(), AppError> {
        checked(args::nonempty("space id", id))?;
        self.command("space.select", &json!({ "id": id }))?;
        Ok(())
    }

    /// Opens a shell beside the pane in the checkout and gives focus back to the pane. The
    /// command is added to `typing`, as `[on_open]` pane `number`, for `type_commands` to type
    /// into the shell, so the shell keeps it in its history and stays when it ends.
    fn pane_open(
        &self,
        request: &PaneSplitRequest,
        command: &str,
        number: usize,
        typing: &mut Vec<Typing>,
    ) -> Result<PaneSplitResponse, PaneOpenError> {
        let created = self.split(request, command).map_err(PaneOpenError::Split)?;
        // Typed even when focus doesn't go back: `hc send` finds the terminal by its address.
        typing.push(Typing {
            pane: number,
            terminal: created.terminal,
            command: command.into(),
        });
        if !request.focus {
            self.command("pane.focus", &json!({ "pane": request.pane_id }))
                .map_err(PaneOpenError::Run)?;
        }
        Ok(PaneSplitResponse {
            pane_id: created.pane,
        })
    }

    /// `pane.split` starts in the split pane's live folder, wherever its shell has gone,
    /// so this opens the terminal with `terminal.open` beside the pane, focused first,
    /// which takes the folder.
    fn split(&self, request: &PaneSplitRequest, command: &str) -> Result<PaneCreated, AppError> {
        checked(args::nonempty("pane id", &request.pane_id))?;
        checked(args::nonempty("command", command))?;
        checked(args::absolute("cwd", &request.cwd))?;
        // The app always splits in half; `ignored_ratio_warning` says so once, at start.
        let folder = checked(args::utf8_path("cwd", &request.cwd))?;
        self.command("pane.focus", &json!({ "pane": request.pane_id }))?;
        self.command_json(
            "terminal.open",
            &json!({ "place": request.direction.as_str(), "folder": folder }),
        )
    }

    /// The main checkout of the repository `folder` is in, if any.
    fn main_checkout(&self, folder: &Path) -> Result<Option<PathBuf>, AppError> {
        if let Some(known) = self
            .main_checkouts
            .lock()
            .expect("main checkouts lock")
            .get(folder)
        {
            return Ok(known.clone());
        }
        let found = self
            .git
            .main_checkout(folder)
            .map_err(|error| git_error(&error))?;
        self.main_checkouts
            .lock()
            .expect("main checkouts lock")
            .insert(folder.to_path_buf(), found.clone());
        Ok(found)
    }

    fn checkout_paths(&self, repo: &Path) -> Result<Vec<PathBuf>, AppError> {
        self.git
            .list_worktrees(repo)
            .map(|listed| {
                listed
                    .items
                    .into_iter()
                    .map(|worktree| worktree.path)
                    .collect()
            })
            .map_err(|error| git_error(&error))
    }
}

/// What to tell the person when `[on_open]` panes set a `ratio`, which the app can't
/// follow: it always splits in half. The same config then still works in a terminal.
pub fn ignored_ratio_warning(on_open: &OnOpenConfig) -> Option<String> {
    on_open
        .panes
        .iter()
        .any(|pane| pane.ratio.is_some())
        .then(|| {
            "[on_open] pane ratios are ignored here: the Herdr app always splits panes in half"
                .to_owned()
        })
}

impl Backend for HerdrApp {
    type SessionId = SpaceId;
    type Error = AppError;

    fn open_repos(&self) -> Result<Vec<PathBuf>, AppError> {
        self.spaces()?
            .iter()
            .filter_map(Space::place)
            .filter_map(|place| self.main_checkout(&place).transpose())
            .collect()
    }

    fn open_folders(&self) -> Result<Vec<PathBuf>, AppError> {
        Ok(self
            .spaces()?
            .into_iter()
            .flat_map(|space| space.folders)
            .flatten()
            .collect())
    }

    fn checkouts(&self, repo: &Path) -> Result<Vec<CheckoutState<SpaceId>>, AppError> {
        checked(args::absolute("repository", repo))?;
        let checkouts = self.checkout_paths(repo)?;
        let places = space_places(&self.spaces()?);
        Ok(checkouts
            .iter()
            .map(|path| CheckoutState {
                path: path.clone(),
                session: session_showing_checkout(&places, path, &checkouts).cloned(),
            })
            .collect())
    }

    fn open_checkout(&self, repo: &Path, checkout: &Path) -> Result<Opened, AppError> {
        checked(args::absolute("checkout", checkout))?;
        let checkouts = self.checkout_paths(repo)?;
        let places = space_places(&self.spaces()?);
        if let Some(space) = session_showing_checkout(&places, checkout, &checkouts) {
            self.select_space(&space.0)?;
            return Ok(Opened::default());
        }
        // Git's spelling of the path, which the app names the space after.
        let canonical = kiosk_core::path::canonical_or_original(checkout);
        let checkout = checkouts
            .iter()
            .find(|path| {
                kiosk_core::path::equivalent(
                    &kiosk_core::path::canonical_or_original(path),
                    &canonical,
                )
            })
            .map_or(checkout, PathBuf::as_path);
        let (_, pane) = self.open_space(checkout)?;
        let mut typing = Vec::new();
        let mut number = 0;
        let warning = on_open::apply(&self.on_open, &pane, checkout, |request, command| {
            number += 1;
            self.pane_open(request, command, number, &mut typing)
        });
        if !typing.is_empty() {
            let host = Arc::clone(&self.host);
            let later = Arc::clone(&self.later);
            self.background.spawn(move || {
                if let Some(warning) = type_commands(host.as_ref(), &typing)
                    && let Err(error) = later.save(ON_OPEN_FAILED, &warning)
                {
                    eprintln!(
                        "{ON_OPEN_FAILED}: {warning} (and the notice couldn't be saved: {error})"
                    );
                }
            });
        }
        Ok(Opened { warning })
    }

    fn open_folder(&self, folder: &Path) -> Result<Opened, AppError> {
        checked(args::absolute("folder", folder))?;
        let panes: Vec<_> = self
            .spaces()?
            .into_iter()
            .flat_map(|space| {
                space
                    .folders
                    .into_iter()
                    .flatten()
                    .map(move |folder| LiveFolder {
                        session: space.id.clone(),
                        folder,
                    })
            })
            .collect();
        let warning = focus_or_create_folder(
            &panes,
            folder,
            |space| self.select_space(space),
            || self.open_space(folder).map(|_| None),
        )?;
        Ok(Opened { warning })
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), AppError> {
        checked(args::nonempty("notification title", title))?;
        checked(args::nonempty("notification body", body))?;
        self.notices.post(title, body).map_err(|error| {
            AppError::Invocation(format!("could not show a notification: {error}"))
        })
    }

    fn worktrees(&self) -> Worktrees<'_, Self> {
        Worktrees::Managed(self)
    }
}

impl ManagedWorktrees<Self> for HerdrApp {
    fn placement(&self) -> &WorktreePlacement {
        &self.placement
    }

    fn close(&self, space: &SpaceId) -> Result<(), AppError> {
        checked(args::nonempty("space id", &space.0))?;
        self.command("space.close", &json!({ "id": space.0, "confirmed": true }))?;
        Ok(())
    }
}

fn parse<T: DeserializeOwned>(output: &[u8], what: &str) -> Result<T, AppError> {
    serde_json::from_slice(output).map_err(|error| {
        AppError::InvalidResponse(format!(
            "{what}: {error}; stdout was {:?}",
            String::from_utf8_lossy(output).trim()
        ))
    })
}

fn git_error(error: &anyhow::Error) -> AppError {
    AppError::Other {
        code: "git".into(),
        message: format!("{error:#}"),
    }
}

#[cfg(test)]
mod tests;
