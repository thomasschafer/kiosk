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
};

use super::{Commit, FileContent, FileDiff, Highlighted, LineKind, Range, Target};

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
}

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
}

impl Row {
    /// The file the row belongs to.
    pub const fn file(self) -> usize {
        match self {
            Self::File(file)
            | Self::Note(file, _)
            | Self::Hunk { file, .. }
            | Self::Line { file, .. } => file,
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
    Resolved {
        target: Arc<Target>,
        commits: Vec<Commit>,
    },
    NoTarget(String),
    Diffed(Vec<FileDiff>),
    Highlighted(usize, Highlighted),
    Failed(String),
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
            if !send(Event::Diffed(files)) {
                return;
            }
            for (index, file) in to_highlight.iter().enumerate() {
                // A file that can't be highlighted shows plain; the diff itself is fine.
                let highlighted = target.highlight(file).unwrap_or_default();
                if !send(Event::Highlighted(index, highlighted)) {
                    return;
                }
            }
        })
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
}

impl DiffTab {
    /// A diff tab for the checkout `cwd` is in, which reads nothing until [`Self::start`].
    pub fn new(cwd: Option<PathBuf>) -> Self {
        Self {
            cwd,
            state: DiffState::Unstarted,
            viewport: 20,
            turns: Arc::default(),
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

    /// Applies what the workers have reported. Returns whether anything changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        loop {
            let event = match &self.state {
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
            };
            let Some(event) = event else {
                return changed;
            };
            changed = true;
            self.apply(event);
        }
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
            Event::Failed(message) => match &mut self.state {
                DiffState::Ready(ready) => ready.content = RangeContent::Failed(message),
                _ => self.state = DiffState::Failed(message),
            },
            Event::Diffed(files) => {
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
                };
                shown.lay_out();
                ready.content = RangeContent::Shown(shown);
            }
            Event::Highlighted(index, highlighted) => {
                if let Some(shown) = self.shown_mut() {
                    if let Some(file) = shown.files.get_mut(index) {
                        file.highlighted = Some(highlighted);
                    }
                    if index + 1 == shown.files.len() {
                        shown.worker = None;
                    }
                }
            }
        }
    }

    pub fn act(&mut self, action: DiffAction) {
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

/// How many columns `text` takes, roughly: each tab as a tab stop's worth.
fn columns(text: &str) -> usize {
    text.chars()
        .map(|character| if character == '\t' { 4 } else { 1 })
        .sum()
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
        }
    }
}

impl Shown {
    /// Lays the files out as rows, keeping the file at the top of the screen there.
    fn lay_out(&mut self) {
        let top_file = self.file_at(self.offset);
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
                for (line_index, line) in hunk.lines.iter().enumerate() {
                    self.rows.push(Row::Line {
                        file: index,
                        hunk: hunk_index,
                        line: line_index,
                    });
                    if line.missing_newline {
                        self.rows.push(Row::Note(index, Note::NoNewline));
                    }
                    self.widest = self.widest.max(columns(&line.text));
                }
            }
        }
        if let Some(file) = top_file {
            self.offset = self.file_row(file).unwrap_or(0);
        }
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
