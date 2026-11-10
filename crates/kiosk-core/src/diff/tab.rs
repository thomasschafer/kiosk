//! The diff tab's state: what it diffs, the range showing, the files and rows of that range,
//! and where it is scrolled to. Git work runs on a worker thread owned by the state that
//! waits for it, so leaving that state cancels it.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    Binary,
    Collapsed,
    /// Its name or mode changed, and nothing else.
    NoContentChange,
}

/// What the person does in the diff tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffAction {
    Scroll(isize),
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
    fn spawn(work: impl FnOnce(&dyn Fn(Event) -> bool) + Send + 'static) -> Self {
        let (sender, events) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        thread::spawn(move || {
            work(&|event| !stop.load(Ordering::Relaxed) && sender.send(event).is_ok());
        });
        Self { events, cancelled }
    }

    /// Finds what to diff from `cwd`, then loads its first range.
    fn resolve(cwd: PathBuf) -> Self {
        Self::spawn(move |send| {
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

    /// Diffs `range`, then highlights its files one by one.
    fn range(target: Arc<Target>, range: Range) -> Self {
        Self::spawn(move |send| {
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
}

impl DiffTab {
    /// A diff tab for the checkout `cwd` is in, which reads nothing until [`Self::start`].
    pub fn new(cwd: Option<PathBuf>) -> Self {
        Self {
            cwd,
            state: DiffState::Unstarted,
            viewport: 20,
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
                let content =
                    RangeContent::Loading(Worker::range(Arc::clone(&target), ranges[0].clone()));
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
        ));
    }
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
                self.rows
                    .extend((0..hunk.lines.len()).map(|line| Row::Line {
                        file: index,
                        hunk: hunk_index,
                        line,
                    }));
            }
        }
        if let Some(file) = top_file {
            self.offset = self.file_row(file).unwrap_or(0);
        }
    }

    /// The file whose rows include row `row`.
    pub fn file_at(&self, row: usize) -> Option<usize> {
        self.rows.get(row).map(|row| match *row {
            Row::File(file)
            | Row::Note(file, _)
            | Row::Hunk { file, .. }
            | Row::Line { file, .. } => file,
        })
    }

    fn file_row(&self, file: usize) -> Option<usize> {
        self.rows.iter().position(|row| *row == Row::File(file))
    }

    fn act(&mut self, action: DiffAction, viewport: usize) {
        let last = max_offset(self.rows.len(), viewport);
        let scroll = |offset: usize, by: isize| offset.saturating_add_signed(by).min(last);
        match action {
            DiffAction::Scroll(by) => self.offset = scroll(self.offset, by),
            DiffAction::Page(pages) => {
                let page = viewport.saturating_sub(1).max(1).cast_signed();
                self.offset = scroll(self.offset, pages * page);
            }
            DiffAction::Top => self.offset = 0,
            DiffAction::Bottom => self.offset = last,
            DiffAction::NextFile | DiffAction::PreviousFile => {
                let Some(current) = self.file_at(self.offset) else {
                    return;
                };
                let at_header = self.file_row(current) == Some(self.offset);
                let target = match action {
                    DiffAction::NextFile => current + 1,
                    // From inside a file, back to its header; from its header, the one before.
                    _ if at_header => current.saturating_sub(1),
                    _ => current,
                };
                if let Some(row) = self.file_row(target.min(self.files.len().saturating_sub(1))) {
                    self.offset = row.min(last);
                }
            }
            DiffAction::ShowFile(file) => {
                if let Some(row) = self.file_row(file) {
                    self.offset = row.min(last);
                }
            }
            DiffAction::ToggleFile(file) => {
                let Some(index) = file.or_else(|| self.file_at(self.offset)) else {
                    return;
                };
                let Some(file) = self.files.get_mut(index) else {
                    return;
                };
                if !matches!(&file.diff.content, FileContent::Text { hunks } if !hunks.is_empty()) {
                    return;
                }
                file.collapsed = !file.collapsed;
                self.offset = self.file_row(index).unwrap_or(self.offset);
                self.lay_out();
                self.offset = self.offset.min(max_offset(self.rows.len(), viewport));
            }
            DiffAction::NextRange | DiffAction::PreviousRange | DiffAction::SelectRange(_) => {
                unreachable!("the tab changes range")
            }
        }
    }
}

#[cfg(test)]
mod tests;
