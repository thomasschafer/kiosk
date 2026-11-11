//! The diff tab's state: what it diffs, the range showing, the files and rows of that range,
//! and where it is scrolled to. Git work runs on a worker thread owned by the state that
//! waits for it, so leaving that state cancels it.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::bail;

use super::{
    Commit, FileContent, FileDiff, Highlighted, Hunk, LineKind, Range, Target, pairs,
    watch::{Places, Watch},
};
use crate::{
    config::keys::{BindingMode, KeysConfig},
    keyboard::Action,
    preferences::DiffLayout,
    state::HelpOverlayState,
};

/// How long changes must settle before the diff is read again, so a burst of saves or a
/// checkout is read once.
const SETTLE: Duration = Duration::from_millis(300);
/// The longest changes are waited on before the diff is read again anyway.
const MAX_WAIT: Duration = Duration::from_secs(2);
/// How long after watching fails it is tried again.
const RETRY_WATCH: Duration = Duration::from_secs(5);

/// Files changing more lines than this start collapsed: reading them is rarely the point.
pub const COLLAPSE_LINES: usize = 1500;

/// Lockfiles and other generated files that start collapsed, by name.
const GENERATED: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "Gemfile.lock",
    "composer.lock",
    "go.sum",
    "flake.lock",
    "mix.lock",
    "pubspec.lock",
    "Package.resolved",
];

/// What the diff tab shows.
#[derive(Debug)]
pub enum DiffState {
    /// Not shown yet, so nothing has been read.
    Unstarted,
    /// Reading what to diff.
    Resolving(Worker),
    /// There is nothing to diff, and why.
    NoTarget(String),
    Failed(String),
    Ready(Box<Ready>),
}

/// A target with its ranges, and the range showing.
#[derive(Debug)]
pub struct Ready {
    pub target: Arc<Target>,
    pub commits: Vec<Commit>,
    pub ranges: Vec<Range>,
    pub range: usize,
    pub content: RangeContent,
}

#[derive(Debug)]
pub enum RangeContent {
    Loading(Worker),
    Failed(String),
    Shown(Shown),
}

/// A range's files, laid out as rows.
#[derive(Debug, Default)]
pub struct Shown {
    pub files: Vec<ShownFile>,
    pub rows: Vec<Row>,
    /// The first row on screen.
    pub offset: usize,
    /// The file the person is on: the one they moved to, or the one at the top of the
    /// screen once they scroll. Kept apart from `offset`, as moving to a file that is already
    /// on screen doesn't scroll.
    pub current_file: usize,
    /// The first column of code on screen, scrolled right past long lines.
    pub column: usize,
    /// The widest line, in columns, which bounds `column`.
    widest: usize,
    /// How the rows are laid out.
    pub layout: DiffLayout,
    /// Highlighting still runs on the range's worker until every file has it.
    worker: Option<Worker>,
}

#[derive(Debug)]
pub struct ShownFile {
    pub diff: FileDiff,
    pub highlighted: Option<Highlighted>,
    pub collapsed: bool,
    pub added: usize,
    pub removed: usize,
    /// Why the file starts collapsed, if it does.
    pub collapse_reason: Option<CollapseReason>,
    /// For each hunk, each line's words changed from its partner, once the worker has
    /// compared them alongside highlighting.
    pub changes: Option<LineChanges>,
}

/// For each hunk of a file, each line's changed words (see [`pairs::changed_lines`]).
pub type LineChanges = Vec<Vec<Option<Vec<std::ops::Range<usize>>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollapseReason {
    Generated,
    Large,
}

/// A row of the diff body, by where its content is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    File(usize),
    /// Something to say about a file in place of, or before, its lines.
    Note(usize, Note),
    Hunk {
        file: usize,
        hunk: usize,
    },
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
    /// Side by side: the old line on the left and the new on the right, by index into the
    /// hunk's lines; a context line is on both.
    Pair {
        file: usize,
        hunk: usize,
        old: Option<usize>,
        new: Option<usize>,
    },
}

impl Row {
    /// Whether this row shows what `other` showed, in either layout: a line of a hunk,
    /// whether alone or paired, or the same header or note.
    fn shows(self, other: Self) -> bool {
        let line = |row: Self| match row {
            Self::Line { file, hunk, line } => Some((file, hunk, line, line)),
            Self::Pair {
                file,
                hunk,
                old,
                new,
            } => {
                let first = old.or(new)?;
                Some((file, hunk, first, new.unwrap_or(first)))
            }
            _ => None,
        };
        match (line(self), line(other)) {
            (
                Some((file, hunk, old, new)),
                Some((other_file, other_hunk, other_old, other_new)),
            ) => {
                (file, hunk) == (other_file, other_hunk)
                    && [old, new]
                        .iter()
                        .any(|line| [other_old, other_new].contains(line))
            }
            (None, None) => self == other,
            _ => false,
        }
    }

    /// The file the row belongs to.
    pub const fn file(self) -> usize {
        match self {
            Self::File(file)
            | Self::Note(file, _)
            | Self::Hunk { file, .. }
            | Self::Line { file, .. }
            | Self::Pair { file, .. } => file,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    Binary,
    Collapsed,
    /// Its name or mode changed, and nothing else.
    NoContentChange,
    /// The line before has no newline after it: the end of its side of the file.
    NoNewline,
}

/// What the person does in the diff tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffAction {
    Scroll(isize),
    /// Columns to the right, or the left when negative.
    ScrollColumns(isize),
    /// Whole screens.
    Page(isize),
    Top,
    Bottom,
    NextFile,
    PreviousFile,
    /// Scroll to file `n` of the range.
    ShowFile(usize),
    NextRange,
    PreviousRange,
    SelectRange(usize),
    /// Collapse or expand file `n`, or with `None` the file at the top of the screen.
    ToggleFile(Option<usize>),
}

/// A file to open in an editor, at a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The checkout the file is in.
    pub checkout: PathBuf,
    pub file: PathBuf,
    /// From 1, in the file as it is now.
    pub line: u32,
}

/// Background work that reports on a channel, stopped when dropped.
#[derive(Debug)]
pub struct Worker {
    events: mpsc::Receiver<Event>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug)]
enum Event {
    /// The checkout's watch, started before anything is read from it.
    Watching(anyhow::Result<Watch>),
    Resolved {
        target: Arc<Target>,
        commits: Vec<Commit>,
    },
    NoTarget(String),
    Diffed(Vec<FileDiff>),
    /// A file's highlighting and the words changed in each line of each of its hunks.
    Highlighted(usize, Highlighted, LineChanges),
    Failed(String),
    /// What the diff shows changed; the worker highlights the new files next.
    Refreshed {
        target: Arc<Target>,
        commits: Vec<Commit>,
        range: usize,
        files: Vec<FileDiff>,
    },
    /// What changed doesn't change what the diff shows.
    Unchanged,
    /// The diff couldn't be read again; what shows stays.
    RefreshFailed(String),
}

impl Worker {
    /// Runs `work` on a thread, giving it a way to report, which answers whether anyone still
    /// listens, and a way to ask whether it has been cancelled.
    fn spawn(
        work: impl FnOnce(&dyn Fn(Event) -> bool, &dyn Fn() -> bool) + Send + 'static,
    ) -> Self {
        let (sender, events) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        thread::spawn(move || {
            let cancelled = || stop.load(Ordering::Relaxed);
            work(
                &|event| !cancelled() && sender.send(event).is_ok(),
                &cancelled,
            );
        });
        Self { events, cancelled }
    }

    /// Finds what to diff from `cwd`, then loads its first range.
    fn resolve(cwd: PathBuf) -> Self {
        Self::spawn(move |send, _| {
            if !cwd.is_dir() {
                send(Event::NoTarget(format!(
                    "{} is not a folder",
                    crate::path::display(&cwd)
                )));
                return;
            }
            let resolved = Target::checkout(&cwd).and_then(|target| {
                let Some(target) = target else {
                    return Ok(None);
                };
                send(Event::Watching(Watch::start(target.root())));
                let commits = target.commits()?;
                Ok(Some((target, commits)))
            });
            match resolved {
                Ok(Some((target, commits))) => {
                    send(Event::Resolved {
                        target: Arc::new(target),
                        commits,
                    });
                }
                Ok(None) => {
                    send(Event::NoTarget(format!(
                        "{} is not in a git repository",
                        crate::path::display(&cwd)
                    )));
                }
                Err(error) => {
                    send(Event::Failed(format!("{error:#}")));
                }
            }
        })
    }

    /// Diffs `range`, then highlights its files one by one. Ranges take `turns` one at a
    /// time, so switching range quickly queues them rather than running git for each at once,
    /// and one abandoned while it waits never runs.
    fn range(target: Arc<Target>, range: Range, turns: Arc<Mutex<()>>) -> Self {
        Self::spawn(move |send, cancelled| {
            let _turn = turns.lock().unwrap_or_else(PoisonError::into_inner);
            if cancelled() {
                return;
            }
            let files = match target.diff(&range) {
                Ok(files) => files,
                Err(error) => {
                    send(Event::Failed(format!("{error:#}")));
                    return;
                }
            };
            let to_highlight = files.clone();
            if send(Event::Diffed(files)) {
                highlight_all(&target, &to_highlight, send);
            }
        })
    }

    /// Checks whether `paths` changed what the diff shows and, if they did, finds what to
    /// diff from `cwd` again and loads the range like `wanted`, or the first if it has gone.
    fn refresh(
        cwd: PathBuf,
        places: Places,
        paths: Changed,
        wanted: Range,
        turns: Arc<Mutex<()>>,
    ) -> Self {
        Self::spawn(move |send, cancelled| {
            let _turn = turns.lock().unwrap_or_else(PoisonError::into_inner);
            if cancelled() {
                return;
            }
            let relevant = match &paths {
                Changed::Paths(paths) => places.any_relevant(paths),
                Changed::Anything => Ok(true),
            };
            let refreshed = relevant.and_then(|relevant| {
                if !relevant {
                    return Ok(None);
                }
                let Some(target) = Target::checkout(&cwd)? else {
                    bail!(
                        "{} is no longer in a git repository",
                        crate::path::display(&cwd)
                    );
                };
                let commits = target.commits()?;
                let ranges = target.ranges(&commits);
                let range = ranges
                    .iter()
                    .position(|range| *range == wanted)
                    .unwrap_or(0);
                let files = match ranges.get(range) {
                    Some(range) => target.diff(range)?,
                    None => Vec::new(),
                };
                Ok(Some((Arc::new(target), commits, range, files)))
            });
            match refreshed {
                Ok(None) => {
                    send(Event::Unchanged);
                }
                Ok(Some((target, commits, range, files))) => {
                    let to_highlight = files.clone();
                    if send(Event::Refreshed {
                        target: Arc::clone(&target),
                        commits,
                        range,
                        files,
                    }) {
                        highlight_all(&target, &to_highlight, send);
                    }
                }
                Err(error) => {
                    send(Event::RefreshFailed(format!("{error:#}")));
                }
            }
        })
    }
}

/// Highlights `files` one by one, sending each with its changed words, until no one listens.
fn highlight_all(target: &Target, files: &[FileDiff], send: &dyn Fn(Event) -> bool) {
    for (index, file) in files.iter().enumerate() {
        // A file that can't be highlighted shows plain; the diff itself is fine.
        let highlighted = target.highlight(file).unwrap_or_default();
        let changes = match &file.content {
            FileContent::Text { hunks } => hunks.iter().map(pairs::changed_lines).collect(),
            FileContent::Binary => Vec::new(),
        };
        if !send(Event::Highlighted(index, highlighted, changes)) {
            return;
        }
    }
}

/// The diff tab: where it was opened, and what it shows.
#[derive(Debug)]
pub struct DiffTab {
    cwd: Option<PathBuf>,
    pub state: DiffState,
    /// How many body rows fit on screen.
    viewport: usize,
    /// Taken by each range's worker while it reads from git.
    turns: Arc<Mutex<()>>,
    /// The key bindings, shown over the tab while open.
    pub help: Option<HelpOverlayState>,
    /// The layout the person chose.
    layout: DiffLayout,
    /// Whether side by side fits on screen; when it doesn't, the tab shows unified.
    side_by_side_fits: bool,
    /// Watching the checkout for changes, once the tab has one.
    watch: Watching,
    /// Why the tab can't follow changes just now, if it can't.
    watch_error: Option<String>,
    /// Why the last refresh failed, until one works.
    refresh_error: Option<String>,
    /// Paths changed since the last refresh began.
    changes: Option<Changes>,
    /// Reading the diff again after changes.
    refresh: Option<Refresh>,
    /// Why the file couldn't be opened in an editor, until the person does something else.
    open_error: Option<String>,
}

#[derive(Debug)]
enum Watching {
    /// Waiting for the worker finding what to diff, which starts the watch before the first
    /// diff is read so no change slips between them.
    Pending,
    /// Starting again after failing, off the session thread, as it reads the working tree's
    /// top level and runs git.
    Starting(mpsc::Receiver<anyhow::Result<Watch>>),
    On(Watch),
    /// Failed; tried again from this time.
    RetryAt(Instant),
}

#[derive(Debug)]
struct Changes {
    first: Instant,
    last: Instant,
    paths: Changed,
}

/// What has changed, as far as the tab knows.
#[derive(Debug, Clone)]
enum Changed {
    Paths(Vec<PathBuf>),
    /// Anything may have: the tab wasn't watching for a while.
    Anything,
}

impl Changed {
    fn extend(&mut self, more: Self) {
        match (self, more) {
            (Self::Paths(paths), Self::Paths(more)) => paths.extend(more),
            (this, _) => *this = Self::Anything,
        }
    }
}

impl Changes {
    fn new(paths: Changed) -> Self {
        let now = Instant::now();
        Self {
            first: now,
            last: now,
            paths,
        }
    }

    /// Adds `paths`, which changed just now.
    fn add(changes: &mut Option<Self>, paths: Changed) {
        match changes {
            Some(changes) => {
                changes.last = Instant::now();
                changes.paths.extend(paths);
            }
            None => *changes = Some(Self::new(paths)),
        }
    }

    /// Whether changes have stopped for [`SETTLE`], or have gone on for [`MAX_WAIT`], which
    /// keeps a constant stream, such as a build writing ignored files, from holding off a
    /// refresh for good.
    fn settled(&self) -> bool {
        self.last.elapsed() >= SETTLE || self.first.elapsed() >= MAX_WAIT
    }
}

#[derive(Debug)]
struct Refresh {
    worker: Worker,
    /// What the refresh is for, for another to take up if this one is abandoned.
    paths: Changed,
}

impl DiffTab {
    /// A diff tab for the checkout `cwd` is in, laid out as `layout` where it fits, which
    /// reads nothing until [`Self::start`].
    pub fn new(cwd: Option<PathBuf>, layout: DiffLayout) -> Self {
        Self {
            cwd,
            state: DiffState::Unstarted,
            viewport: 20,
            turns: Arc::default(),
            help: None,
            layout,
            side_by_side_fits: true,
            watch: Watching::Pending,
            watch_error: None,
            refresh_error: None,
            changes: None,
            refresh: None,
            open_error: None,
        }
    }

    /// The layout the person chose, which may not fit.
    pub const fn layout(&self) -> DiffLayout {
        self.layout
    }

    /// The layout shown: the one chosen, or unified where side by side doesn't fit.
    pub const fn shown_layout(&self) -> DiffLayout {
        match self.layout {
            DiffLayout::SideBySide if self.side_by_side_fits => DiffLayout::SideBySide,
            DiffLayout::SideBySide | DiffLayout::Unified => DiffLayout::Unified,
        }
    }

    /// Says whether side by side fits on screen, laying the rows out again if that changes
    /// what shows.
    pub fn set_side_by_side_fits(&mut self, fits: bool) {
        self.side_by_side_fits = fits;
        self.lay_out_again();
    }

    pub fn toggle_layout(&mut self) {
        self.open_error = None;
        self.layout = self.layout.toggled();
        self.lay_out_again();
    }

    fn lay_out_again(&mut self) {
        let layout = self.shown_layout();
        let viewport = self.viewport;
        if let Some(shown) = self.shown_mut()
            && shown.layout != layout
        {
            shown.layout = layout;
            shown.lay_out();
            shown.offset = shown.offset.min(max_offset(shown.rows.len(), viewport));
        }
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Starts reading what to diff, the first time the tab shows.
    pub fn start(&mut self) {
        if !matches!(self.state, DiffState::Unstarted) {
            return;
        }
        self.state = match &self.cwd {
            Some(cwd) => DiffState::Resolving(Worker::resolve(cwd.clone())),
            None => DiffState::NoTarget(
                "Open kiosk from a terminal in a git checkout to see its changes".into(),
            ),
        };
    }

    pub fn set_viewport(&mut self, rows: usize) {
        self.viewport = rows.max(1);
        if let Some(shown) = self.shown_mut() {
            shown.offset = shown.offset.min(max_offset(shown.rows.len(), rows.max(1)));
        }
    }

    pub const fn viewport(&self) -> usize {
        self.viewport
    }

    pub fn shown(&self) -> Option<&Shown> {
        match &self.state {
            DiffState::Ready(ready) => match &ready.content {
                RangeContent::Shown(shown) => Some(shown),
                RangeContent::Loading(_) | RangeContent::Failed(_) => None,
            },
            _ => None,
        }
    }

    fn shown_mut(&mut self) -> Option<&mut Shown> {
        match &mut self.state {
            DiffState::Ready(ready) => match &mut ready.content {
                RangeContent::Shown(shown) => Some(shown),
                RangeContent::Loading(_) | RangeContent::Failed(_) => None,
            },
            _ => None,
        }
    }

    /// Applies what the workers have reported, and refreshes after changes. Returns whether
    /// anything changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = self.follow_changes();
        loop {
            let event = if let Some(event) = self
                .refresh
                .as_ref()
                .and_then(|refresh| refresh.worker.events.try_recv().ok())
            {
                Some(event)
            } else {
                match &self.state {
                    DiffState::Resolving(worker) => worker.events.try_recv().ok(),
                    DiffState::Ready(ready) => match &ready.content {
                        RangeContent::Loading(worker)
                        | RangeContent::Shown(Shown {
                            worker: Some(worker),
                            ..
                        }) => worker.events.try_recv().ok(),
                        _ => None,
                    },
                    _ => None,
                }
            };
            let Some(event) = event else {
                return changed;
            };
            changed = true;
            self.apply(event);
        }
    }

    /// Takes up a watch that has started, or retries later if it failed. Returns whether the
    /// tab's notice changed.
    fn watch_started(&mut self, started: anyhow::Result<Watch>) -> bool {
        match started {
            Ok(watch) => {
                self.watch = Watching::On(watch);
                self.watch_error.take().is_some()
            }
            Err(error) => {
                self.watch_error = Some(format!("{error:#}"));
                self.watch = Watching::RetryAt(Instant::now() + RETRY_WATCH);
                true
            }
        }
    }

    /// Why opening a file failed, or why the tab isn't showing the latest changes, if it may
    /// not be.
    pub fn notice(&self) -> Option<String> {
        self.open_error
            .as_ref()
            .map(|error| format!("Couldn't open the file: {error}"))
            .or_else(|| {
                self.watch_error
                    .as_ref()
                    .map(|error| format!("Not following changes: {error}"))
            })
            .or_else(|| {
                self.refresh_error
                    .as_ref()
                    .map(|error| format!("Couldn't read the latest changes: {error}"))
            })
    }

    /// Watches the checkout once there is one, gathers what changed, and starts a refresh
    /// once changes have settled and none is running. Returns whether the tab changed.
    fn follow_changes(&mut self) -> bool {
        let DiffState::Ready(ready) = &self.state else {
            return false;
        };
        let mut changed = false;
        match &mut self.watch {
            Watching::Pending => {}
            Watching::Starting(receiver) => match receiver.try_recv() {
                Ok(started) => {
                    changed = self.watch_started(started);
                    // What changed while it wasn't watched isn't known, so read it all again.
                    if matches!(self.watch, Watching::On(_)) {
                        Changes::add(&mut self.changes, Changed::Anything);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    changed = self.watch_started(Err(anyhow::anyhow!(
                        "the file watcher stopped as it started"
                    )));
                }
            },
            Watching::On(watch) => match watch.changed() {
                Ok(paths) if !paths.is_empty() => {
                    Changes::add(&mut self.changes, Changed::Paths(paths));
                }
                Ok(_) => {}
                Err(error) => changed = self.watch_started(Err(error)),
            },
            Watching::RetryAt(at) => {
                if Instant::now() >= *at {
                    let root = ready.target.root().to_path_buf();
                    let (sender, receiver) = mpsc::channel();
                    thread::spawn(move || {
                        let _ = sender.send(Watch::start(&root));
                    });
                    self.watch = Watching::Starting(receiver);
                }
            }
        }
        let DiffState::Ready(ready) = &self.state else {
            return changed;
        };
        let settled = self.changes.as_ref().is_some_and(Changes::settled);
        // A range still loading will show the files as they are now anyway.
        let showing = matches!(ready.content, RangeContent::Shown(_));
        let (Some(cwd), Watching::On(watch)) = (&self.cwd, &self.watch) else {
            return changed;
        };
        if !settled || self.refresh.is_some() || !showing {
            return changed;
        }
        let paths = self.changes.take().expect("settled changes").paths;
        self.refresh = Some(Refresh {
            worker: Worker::refresh(
                cwd.clone(),
                watch.places().clone(),
                paths.clone(),
                ready.ranges[ready.range].clone(),
                Arc::clone(&self.turns),
            ),
            paths,
        });
        changed
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Resolved { target, commits } => {
                let ranges = target.ranges(&commits);
                if ranges.is_empty() {
                    self.state =
                        DiffState::NoTarget(format!("{} has nothing to diff", target.head()));
                    return;
                }
                let content = RangeContent::Loading(Worker::range(
                    Arc::clone(&target),
                    ranges[0].clone(),
                    Arc::clone(&self.turns),
                ));
                self.state = DiffState::Ready(Box::new(Ready {
                    target,
                    commits,
                    ranges,
                    range: 0,
                    content,
                }));
            }
            Event::NoTarget(reason) => self.state = DiffState::NoTarget(reason),
            Event::Watching(started) => {
                self.watch_started(started);
            }
            Event::Unchanged => {
                self.refresh = None;
                self.refresh_error = None;
            }
            // What shows stays, and the next change tries again.
            Event::RefreshFailed(error) => {
                self.refresh = None;
                self.refresh_error = Some(error);
            }
            Event::Refreshed {
                target,
                commits,
                range,
                files,
            } => self.show_refreshed(target, commits, range, files),
            Event::Failed(message) => match &mut self.state {
                DiffState::Ready(ready) => ready.content = RangeContent::Failed(message),
                _ => self.state = DiffState::Failed(message),
            },
            Event::Diffed(files) => {
                let layout = self.shown_layout();
                let DiffState::Ready(ready) = &mut self.state else {
                    return;
                };
                let worker = match std::mem::replace(
                    &mut ready.content,
                    RangeContent::Failed(String::new()),
                ) {
                    RangeContent::Loading(worker) => worker,
                    other => {
                        ready.content = other;
                        return;
                    }
                };
                let mut shown = Shown {
                    // With no files, there is nothing left for the worker to highlight.
                    worker: (!files.is_empty()).then_some(worker),
                    files: files.into_iter().map(ShownFile::new).collect(),
                    rows: Vec::new(),
                    offset: 0,
                    current_file: 0,
                    column: 0,
                    widest: 0,
                    layout,
                };
                shown.lay_out();
                ready.content = RangeContent::Shown(shown);
            }
            Event::Highlighted(index, highlighted, changes) => {
                if let Some(shown) = self.shown_mut() {
                    if let Some(file) = shown.files.get_mut(index) {
                        file.highlighted = Some(highlighted);
                        file.changes = Some(changes);
                    }
                    if index + 1 == shown.files.len() {
                        shown.worker = None;
                    }
                }
            }
        }
    }

    /// Shows the diff tab's key bindings.
    pub fn show_help(&mut self, keys: &KeysConfig) {
        self.open_error = None;
        self.help = Some(HelpOverlayState::for_mode(keys, BindingMode::Diff));
    }

    /// Applies `action`, one of the help overlay's, while help shows.
    pub fn help_action(&mut self, action: Action) {
        let Some(help) = &mut self.help else {
            return;
        };
        match action {
            Action::CloseOverlay => self.help = None,
            Action::MoveSelection(delta) => help.list.move_selection(delta),
            Action::Select(position) => help.list.select(position),
            Action::Insert(character) => {
                help.list.input.insert_char(character);
                help.filter_now();
            }
            Action::Edit(edit) if help.list.input.apply(edit) => help.filter_now(),
            _ => {}
        }
    }

    /// The file the person is on, at the line they're looking at, to open in an editor.
    pub fn place(&self) -> Result<Place, String> {
        let nothing = || "there's no file showing".to_owned();
        let DiffState::Ready(ready) = &self.state else {
            return Err(nothing());
        };
        let RangeContent::Shown(shown) = &ready.content else {
            return Err(nothing());
        };
        if !ready.target.has_working_tree() {
            return Err("only the files of a checkout can be opened".into());
        }
        let (file, line) = shown.place(self.viewport).ok_or_else(nothing)?;
        let diff = &shown.files[file].diff;
        let Some(new) = &diff.new else {
            return Err(format!("{} was deleted", diff.path().display()));
        };
        let checkout = ready.target.root().to_path_buf();
        let path = checkout.join(&new.path);
        if !path.exists() {
            return Err(format!(
                "{} is no longer in the checkout",
                new.path.display()
            ));
        }
        Ok(Place {
            checkout,
            file: path,
            line,
        })
    }

    /// Shows why opening the file in an editor failed, until the person does something else.
    pub fn show_open_error(&mut self, error: String) {
        self.open_error = Some(error);
    }

    pub fn act(&mut self, action: DiffAction) {
        self.open_error = None;
        match action {
            DiffAction::NextRange => self.step_range(1),
            DiffAction::PreviousRange => self.step_range(-1),
            DiffAction::SelectRange(index) => self.select_range(index),
            _ => {
                let viewport = self.viewport;
                if let Some(shown) = self.shown_mut() {
                    shown.act(action, viewport);
                }
            }
        }
    }

    /// Shows the diff read again after changes in place of the one showing, keeping the
    /// person's place: the file they were on, the line at the top of the screen as near as
    /// the new rows allow, the files they collapsed or expanded, and the column.
    fn show_refreshed(
        &mut self,
        target: Arc<Target>,
        commits: Vec<Commit>,
        range: usize,
        files: Vec<FileDiff>,
    ) {
        let layout = self.shown_layout();
        let viewport = self.viewport;
        let worker = self.refresh.take().map(|refresh| refresh.worker);
        self.refresh_error = None;
        let old = self.shown();
        let path_of = |shown: &Shown, file: usize| {
            shown
                .files
                .get(file)
                .map(|file| file.diff.path().to_path_buf())
        };
        let collapsed = old
            .map(|old| {
                old.files
                    .iter()
                    .map(|file| (file.diff.path().to_path_buf(), file.collapsed))
                    .collect::<std::collections::HashMap<_, _>>()
            })
            .unwrap_or_default();
        let current = old.and_then(|old| path_of(old, old.current_file));
        // The file at the top of the screen, the line there if it is one, and how far into
        // the file's rows the screen starts.
        let top = old.and_then(|old| {
            let row = *old.rows.get(old.offset)?;
            let file = row.file();
            let header = old.file_row(file)?;
            Some((
                path_of(old, file)?,
                old.line_number(row),
                old.offset - header,
            ))
        });
        let column = old.map_or(0, |old| old.column);
        let mut shown = Shown {
            worker: (!files.is_empty()).then_some(worker).flatten(),
            files: files
                .into_iter()
                .map(|file| {
                    let mut file = ShownFile::new(file);
                    if let Some(collapsed) = collapsed.get(file.diff.path()) {
                        file.collapsed = *collapsed;
                    }
                    file
                })
                .collect(),
            rows: Vec::new(),
            offset: 0,
            current_file: 0,
            column: 0,
            widest: 0,
            layout,
        };
        shown.lay_out();
        let index_of = |path: &Path| shown.files.iter().position(|file| file.diff.path() == path);
        shown.current_file = current.and_then(|path| index_of(&path)).unwrap_or(0);
        if let Some((path, number, into)) = top
            && let Some(file) = index_of(&path)
            && let Some(header) = shown.file_row(file)
        {
            let end = shown
                .rows
                .iter()
                .rposition(|row| row.file() == file)
                .unwrap_or(header);
            // The same line by its number, which earlier hunks changing doesn't move, or the
            // first after it; else as far into the file as before.
            let same_line = number.and_then(|number| {
                (header..=end).find(|&row| {
                    shown
                        .line_number(shown.rows[row])
                        .is_some_and(|found| found.0 == number.0 && found.1 >= number.1)
                })
            });
            shown.offset = same_line
                .unwrap_or(header + into)
                .min(end)
                .min(max_offset(shown.rows.len(), viewport));
        }
        shown.column = column.min(shown.widest.saturating_sub(MIN_COLUMNS_SHOWN));
        let ranges = target.ranges(&commits);
        self.state = DiffState::Ready(Box::new(Ready {
            target,
            commits,
            ranges,
            range,
            content: RangeContent::Shown(shown),
        }));
    }

    fn step_range(&mut self, by: isize) {
        let DiffState::Ready(ready) = &self.state else {
            return;
        };
        let count = ready.ranges.len().cast_signed();
        let index = (ready.range.cast_signed() + by).clamp(0, count - 1);
        self.select_range(index.cast_unsigned());
    }

    fn select_range(&mut self, index: usize) {
        let DiffState::Ready(ready) = &mut self.state else {
            return;
        };
        if index == ready.range || index >= ready.ranges.len() {
            return;
        }
        ready.range = index;
        // A refresh would load the range the person just left; what it was for is taken up
        // again once the new range shows.
        if let Some(refresh) = self.refresh.take() {
            Changes::add(&mut self.changes, refresh.paths);
        }
        // Replacing the content drops the range's worker, which stops it.
        ready.content = RangeContent::Loading(Worker::range(
            Arc::clone(&ready.target),
            ready.ranges[index].clone(),
            Arc::clone(&self.turns),
        ));
    }
}

/// Scrolled right as far as it goes, the widest line still shows this many columns.
const MIN_COLUMNS_SHOWN: usize = 20;

/// Where line `index` of `hunk` is in the new file: its own number, or for a removed line,
/// that of the line now in its place.
fn new_position(hunk: &Hunk, index: usize) -> u32 {
    if let Some(number) = hunk.lines[index].new {
        return number;
    }
    let before = hunk.lines[..index]
        .iter()
        .filter(|line| line.new.is_some())
        .count();
    // A hunk with no new lines starts after the line before it.
    let first = if hunk.new_len == 0 {
        hunk.new_start.saturating_add(1)
    } else {
        hunk.new_start
    };
    first
        .saturating_add(u32::try_from(before).unwrap_or(u32::MAX))
        .max(1)
}

fn max_offset(rows: usize, viewport: usize) -> usize {
    rows.saturating_sub(viewport)
}

impl ShownFile {
    fn new(diff: FileDiff) -> Self {
        let (mut added, mut removed) = (0, 0);
        if let FileContent::Text { hunks } = &diff.content {
            for line in hunks.iter().flat_map(|hunk| &hunk.lines) {
                match line.kind {
                    LineKind::Added => added += 1,
                    LineKind::Removed => removed += 1,
                    LineKind::Context => {}
                }
            }
        }
        let generated = diff
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| GENERATED.contains(&name));
        let collapse_reason = if generated {
            Some(CollapseReason::Generated)
        } else if added + removed > COLLAPSE_LINES {
            Some(CollapseReason::Large)
        } else {
            None
        };
        Self {
            collapsed: collapse_reason.is_some(),
            collapse_reason,
            diff,
            highlighted: None,
            added,
            removed,
            changes: None,
        }
    }
}

impl Shown {
    /// Lays the files out as rows, keeping what was at the top of the screen there: the same
    /// line, whichever layout it is now in, or else the same hunk or file.
    fn lay_out(&mut self) {
        let anchor = self.rows.get(self.offset).copied();
        self.rows.clear();
        self.widest = 0;
        for (index, file) in self.files.iter().enumerate() {
            self.rows.push(Row::File(index));
            let hunks = match &file.diff.content {
                FileContent::Binary => {
                    self.rows.push(Row::Note(index, Note::Binary));
                    continue;
                }
                FileContent::Text { hunks } => hunks,
            };
            if hunks.is_empty() {
                self.rows.push(Row::Note(index, Note::NoContentChange));
                continue;
            }
            if file.collapsed {
                self.rows.push(Row::Note(index, Note::Collapsed));
                continue;
            }
            for (hunk_index, hunk) in hunks.iter().enumerate() {
                self.rows.push(Row::Hunk {
                    file: index,
                    hunk: hunk_index,
                });
                for line in &hunk.lines {
                    self.widest = self
                        .widest
                        .max(crate::display::code_chars(&line.text).0.len());
                }
                let missing_newline =
                    |line: Option<usize>| line.is_some_and(|line| hunk.lines[line].missing_newline);
                match self.layout {
                    DiffLayout::Unified => {
                        for line in 0..hunk.lines.len() {
                            self.rows.push(Row::Line {
                                file: index,
                                hunk: hunk_index,
                                line,
                            });
                            if missing_newline(Some(line)) {
                                self.rows.push(Row::Note(index, Note::NoNewline));
                            }
                        }
                    }
                    DiffLayout::SideBySide => {
                        for (old, new) in pairs::side_by_side(hunk) {
                            self.rows.push(Row::Pair {
                                file: index,
                                hunk: hunk_index,
                                old,
                                new,
                            });
                            if missing_newline(old) || missing_newline(new) {
                                self.rows.push(Row::Note(index, Note::NoNewline));
                            }
                        }
                    }
                }
            }
        }
        if let Some(anchor) = anchor {
            self.offset = self
                .rows
                .iter()
                .position(|row| row.shows(anchor))
                .or_else(|| self.file_row(anchor.file()))
                .unwrap_or(0);
        }
    }

    /// The line `row` shows by its number in the file: in the new file, or the old one for a
    /// removed line. `true` with a number in the new file.
    fn line_number(&self, row: Row) -> Option<(bool, u32)> {
        let line = |file: usize, hunk: usize, line: usize| {
            let FileContent::Text { hunks } = &self.files[file].diff.content else {
                return None;
            };
            let line = &hunks[hunk].lines[line];
            line.new
                .map(|number| (true, number))
                .or(line.old.map(|number| (false, number)))
        };
        match row {
            Row::Line {
                file,
                hunk,
                line: index,
            } => line(file, hunk, index),
            Row::Pair {
                file,
                hunk,
                old,
                new,
            } => line(file, hunk, new.or(old)?),
            Row::File(_) | Row::Note(..) | Row::Hunk { .. } => None,
        }
    }

    /// The current file and the line in it, by its number in the new file, that the person is
    /// looking at: the first changed line on screen, or else the first line on screen, or
    /// else its first change. `None` without files.
    fn place(&self, viewport: usize) -> Option<(usize, u32)> {
        let file = self.current_file;
        let FileContent::Text { hunks } = &self.files.get(file)?.diff.content else {
            return Some((file, 1));
        };
        let end = self.offset.saturating_add(viewport).min(self.rows.len());
        let on_screen: Vec<(usize, usize)> = self.rows[self.offset.min(end)..end]
            .iter()
            .filter_map(|row| match *row {
                Row::Line {
                    file: of,
                    hunk,
                    line,
                } if of == file => Some((hunk, line)),
                Row::Pair {
                    file: of,
                    hunk,
                    old,
                    new,
                } if of == file => Some((hunk, new.or(old)?)),
                _ => None,
            })
            .collect();
        let changed =
            |&(hunk, line): &(usize, usize)| hunks[hunk].lines[line].kind != LineKind::Context;
        let everywhere = || {
            hunks
                .iter()
                .enumerate()
                .flat_map(|(index, hunk)| (0..hunk.lines.len()).map(move |line| (index, line)))
        };
        let line = on_screen
            .iter()
            .copied()
            .find(changed)
            .or_else(|| on_screen.first().copied())
            .or_else(|| everywhere().find(changed))
            .map_or(1, |(hunk, line)| new_position(&hunks[hunk], line));
        Some((file, line))
    }

    /// The file whose rows include row `row`.
    pub fn file_at(&self, row: usize) -> Option<usize> {
        self.rows.get(row).map(|row| row.file())
    }

    fn file_row(&self, file: usize) -> Option<usize> {
        self.rows.iter().position(|row| *row == Row::File(file))
    }

    fn act(&mut self, action: DiffAction, viewport: usize) {
        let last = max_offset(self.rows.len(), viewport);
        let scroll_to = |shown: &mut Self, offset: usize| {
            shown.offset = offset.min(last);
            if let Some(file) = shown.file_at(shown.offset) {
                shown.current_file = file;
            }
        };
        match action {
            DiffAction::Scroll(by) => scroll_to(self, self.offset.saturating_add_signed(by)),
            DiffAction::ScrollColumns(by) => {
                // Some of the widest line stays in view.
                let last = self.widest.saturating_sub(MIN_COLUMNS_SHOWN);
                self.column = self.column.saturating_add_signed(by).min(last);
            }
            DiffAction::Page(pages) => {
                let page = viewport.saturating_sub(1).max(1).cast_signed();
                scroll_to(self, self.offset.saturating_add_signed(pages * page));
            }
            DiffAction::Top => scroll_to(self, 0),
            DiffAction::Bottom => {
                scroll_to(self, last);
                self.current_file = self.files.len().saturating_sub(1);
            }
            DiffAction::NextFile => {
                self.show_file(
                    (self.current_file + 1).min(self.files.len().saturating_sub(1)),
                    viewport,
                );
            }
            DiffAction::PreviousFile => {
                // From below its header, back to the current file's header; from its header,
                // to the file before.
                let header = self.file_row(self.current_file);
                let file = if header.is_some_and(|header| header < self.offset) {
                    self.current_file
                } else {
                    self.current_file.saturating_sub(1)
                };
                self.show_file(file, viewport);
            }
            DiffAction::ShowFile(file) => self.show_file(file, viewport),
            DiffAction::ToggleFile(file) => {
                let index = file.unwrap_or(self.current_file);
                let Some(file) = self.files.get_mut(index) else {
                    return;
                };
                if !matches!(&file.diff.content, FileContent::Text { hunks } if !hunks.is_empty()) {
                    return;
                }
                file.collapsed = !file.collapsed;
                self.lay_out();
                self.show_file(index, viewport);
            }
            DiffAction::NextRange | DiffAction::PreviousRange | DiffAction::SelectRange(_) => {
                unreachable!("the tab changes range")
            }
        }
    }

    /// Makes `file` current, scrolling its header to the top unless all of it is on screen.
    fn show_file(&mut self, file: usize, viewport: usize) {
        let Some(header) = self.file_row(file) else {
            return;
        };
        self.current_file = file;
        let end = self
            .rows
            .iter()
            .rposition(|row| row.file() == file)
            .map_or(header, |last| last + 1);
        let on_screen = header >= self.offset && end <= self.offset + viewport;
        if !on_screen {
            self.offset = header.min(max_offset(self.rows.len(), viewport));
        }
    }
}

#[cfg(test)]
mod tests;
