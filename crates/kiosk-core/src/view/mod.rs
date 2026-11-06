//! What the picker shows, independent of how it is drawn: the text, the rows on screen and
//! what each part means. Every frontend draws from this, so the wording, the hints and the
//! rules for what appears when are the same in a terminal and in a window.
//!
//! Colours are named by meaning ([`Tone`]) and left to the frontend's theme. Text is already
//! sanitized for display.

use std::path::Path;

use serde::Serialize;

use crate::{
    backend::{Label, annotation::AnnotationIndex},
    config::keys::{BindingMode, Command, KeyChord, KeysConfig},
    display::{sanitize, sanitized_positions},
    keymap::SCAN_WARNINGS_BINDING_MODE,
    picker::{delete::DeleteFlowState, match_positions, new_branch::BaseBranchSelection},
    state::{
        AppState, BranchEntry, BranchLoad, HelpOverlayState, Mode, Overlay, RemoteBranchChoice,
        RemoteBranchPrompt, RepoEntry, SearchableList, TextInput, Toast, ToastKind, scan_summary,
    },
};

pub mod setup;

/// What a piece of text means, for the frontend's theme to colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Normal,
    Muted,
    /// Quieter than muted: remote-only branches.
    Faint,
    /// The repository view's colour.
    Accent,
    /// The branch view's and its dialogs' colour.
    Secondary,
    /// The help overlay's colour.
    Tertiary,
    Warning,
    Error,
    Open,
    Hint,
    /// A backend label's own tones.
    Neutral,
    Progress,
    Attention,
    Success,
    Failure,
}

impl From<crate::backend::Tone> for Tone {
    fn from(tone: crate::backend::Tone) -> Self {
        use crate::backend::Tone as Label;
        match tone {
            Label::Neutral => Self::Neutral,
            Label::Progress => Self::Progress,
            Label::Attention => Self::Attention,
            Label::Success => Self::Success,
            Label::Failure => Self::Failure,
            Label::Muted => Self::Muted,
        }
    }
}

/// A run of text in one tone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Segment {
    pub text: String,
    pub tone: Tone,
}

impl Segment {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// A key and what it does, such as `enter` and `open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hint {
    pub key: String,
    pub label: &'static str,
}

impl Hint {
    pub fn new(key: impl Into<String>, label: &'static str) -> Self {
        Self {
            key: key.into(),
            label,
        }
    }

    pub fn for_key(key: KeyChord, label: &'static str) -> Self {
        Self::new(key.to_string(), label)
    }
}

/// Query text split at the cursor, so a frontend can draw the cursor without measuring
/// byte offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Query {
    pub before: String,
    pub after: String,
}

impl Query {
    pub fn new(input: &TextInput) -> Self {
        Self::from_parts(&input.text, input.cursor)
    }

    /// `cursor` is a byte offset into `text`; one inside a character is moved before it.
    pub fn from_parts(text: &str, cursor: usize) -> Self {
        let mut cursor = cursor.min(text.len());
        while !text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        Self {
            before: sanitize(&text[..cursor]).into_owned(),
            after: sanitize(&text[cursor..]).into_owned(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }
}

/// Which of the picker's lists a [`ListView`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    Repos,
    Branches,
    Bases,
}

impl ListKind {
    pub const fn tone(self) -> Tone {
        match self {
            Self::Repos => Tone::Accent,
            Self::Branches | Self::Bases => Tone::Secondary,
        }
    }
}

/// A searchable list: its search field and the rows in view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListView {
    pub kind: ListKind,
    pub title: String,
    /// What the list is of, for a frontend that titles it its own way: the repository for
    /// its branches, the new branch for its bases.
    pub subject: Option<String>,
    pub placeholder: &'static str,
    pub query: Query,
    /// Such as `3 of 40 repos`.
    pub summary: String,
    /// Background work still adding to the list, such as `scanning`, drawn with a spinner.
    pub activity: Option<&'static str>,
    /// The rows in view, top to bottom.
    pub rows: Vec<Row>,
    /// Which of `rows` is selected.
    pub selected: Option<usize>,
    /// The position of `rows[0]` among all matches, and how many match.
    pub offset: usize,
    pub total: usize,
    /// Shown in place of rows when there are none.
    pub empty: Option<String>,
}

/// One row of a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    /// What the row shows, the same however the list is sorted, filtered or scrolled (see
    /// [`item_id`]). A click names it alongside the position.
    pub id: String,
    /// The row's position among all matches, which pointer selection names.
    pub position: usize,
    pub name: Segment,
    /// Which characters of `name` the search matched (character indices, ascending), for a
    /// frontend to mark.
    pub matches: Vec<usize>,
    /// What kind of item it is beyond its name, in the order a terminal writes them.
    pub badges: Vec<Badge>,
    /// The backend's labels, drawn at the end of the row.
    pub labels: Vec<Segment>,
    /// Whether the backend is showing it.
    pub open: bool,
    /// What else can be done to the row beyond opening it.
    pub actions: RowActions,
}

/// Something a row says about its item besides the name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Badge {
    /// A plain folder rather than a repository.
    Folder,
    /// A branch only this remote has.
    Remote { name: String },
    /// The branch is checked out in the repository's own checkout.
    MainCheckout,
    /// The branch has a checkout of its own, a linked worktree.
    Worktree,
    /// The branch is checked out where the user is.
    Current,
    /// The repository's default branch.
    Default,
}

impl Badge {
    /// How a terminal writes it after the name.
    pub const fn suffix(&self) -> Option<&'static str> {
        match self {
            Self::Folder => Some(FOLDER_SUFFIX),
            Self::MainCheckout | Self::Worktree => Some(WORKTREE_SUFFIX),
            Self::Current => Some(CURRENT_SUFFIX),
            Self::Default => Some(DEFAULT_SUFFIX),
            Self::Remote { .. } => None,
        }
    }

    pub const fn tone(&self) -> Tone {
        match self {
            Self::Folder | Self::Default | Self::Remote { .. } => Tone::Muted,
            Self::MainCheckout | Self::Worktree | Self::Current => Tone::Secondary,
        }
    }
}

impl Row {
    /// The name and badges as a terminal writes them: `origin/feature`, `main (worktree) *`.
    pub fn segments(&self) -> Vec<Segment> {
        let name = self.badges.iter().find_map(|badge| match badge {
            Badge::Remote { name } => Some(format!("{name}/{}", self.name.text)),
            _ => None,
        });
        std::iter::once(Segment::new(
            name.unwrap_or_else(|| self.name.text.clone()),
            self.name.tone,
        ))
        .chain(self.badges.iter().filter_map(|badge| {
            badge
                .suffix()
                .map(|suffix| Segment::new(suffix, badge.tone()))
        }))
        .collect()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RowActions {
    /// The repository has branches to browse.
    pub branches: bool,
    /// The branch has a checkout that can be removed.
    pub delete: bool,
}

/// A choice in a dialog, such as tracking a remote branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub label: String,
    /// What the choice is (see [`item_id`]), which a click names alongside its position.
    pub id: String,
}

pub const OPEN_MARKER: &str = "● open";

/// How many rows each list has room for, which decides what is scrolled into view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewports {
    pub list: usize,
    pub bases: usize,
    pub help: usize,
    pub scan_warnings: usize,
}

/// Everything the picker shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PickerView {
    pub main: Main,
    pub dialog: Option<Dialog>,
    pub overlay: Option<OverlayView>,
    pub toast: Option<ToastView>,
    /// The footer's key hints.
    pub hints: Vec<Hint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Main {
    /// Work the picker waits for, such as opening a checkout.
    Loading {
        message: String,
        hint: Option<String>,
    },
    List(ListView),
}

/// A dialog over the branch list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Dialog {
    ValidatingBranch {
        title: String,
        message: &'static str,
    },
    BaseBranch {
        title: String,
        list: ListView,
    },
    RemoteBranch {
        title: &'static str,
        lines: Vec<String>,
        choices: Vec<Choice>,
        selected: usize,
        hints: Vec<Hint>,
    },
    ConfirmDelete {
        title: &'static str,
        /// Said first and louder: the checkout has uncommitted changes.
        warning: Option<&'static str>,
        lines: Vec<String>,
        /// The checkout, for frontends that set it apart from `lines`.
        path: String,
        /// Whether it is removed despite uncommitted changes.
        force: bool,
        /// Where the checkout is also closed, if it is open there.
        closes_in: Option<String>,
        /// Shown in place of the hints while the removal runs.
        busy: Option<&'static str>,
        hints: Vec<Hint>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverlayView {
    Help(HelpView),
    ScanWarnings(ScanWarningsView),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HelpView {
    pub title: &'static str,
    pub placeholder: &'static str,
    pub query: Query,
    /// Explains that a notification is in front of the bindings listed.
    pub notice: Option<&'static str>,
    /// Such as `12 commands (esc: close)`.
    pub summary: String,
    /// The entries in view.
    pub entries: Vec<HelpEntry>,
    /// Which of `entries` is selected.
    pub selected: Option<usize>,
    /// The position of `entries[0]` among all of them, sections and blank lines included, and
    /// how many there are.
    pub offset: usize,
    pub total: usize,
    pub empty: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HelpEntry {
    Section {
        name: &'static str,
    },
    Blank,
    Binding {
        /// The binding's position among the matches, which pointer selection names.
        position: usize,
        /// Which binding it is (see [`item_id`]), which a click names alongside its position.
        id: String,
        /// Every key that runs `command` here, in the order to show them.
        keys: Vec<String>,
        description: &'static str,
        command: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanWarningsView {
    pub title: String,
    /// The warnings from the first in view on.
    pub warnings: Vec<ScanWarningView>,
    /// How many warnings are above the first in view.
    pub offset: usize,
    pub total: usize,
    pub hints: Vec<Hint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanWarningView {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToastView {
    pub level: ToastLevel,
    /// `Warning` or `Error`.
    pub label: &'static str,
    pub message: String,
    /// Such as `1/3` when more are queued behind it.
    pub counter: Option<String>,
    pub hints: Vec<Hint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastLevel {
    Warning,
    Error,
}

impl ToastLevel {
    pub const fn tone(self) -> Tone {
        match self {
            Self::Warning => Tone::Warning,
            Self::Error => Tone::Error,
        }
    }
}

/// The picker as it is now. Scrolls each list so its selection is in view in the rows
/// `viewports` gives it, as drawing it would.
pub fn picker(state: &mut AppState, keys: &KeysConfig, viewports: Viewports) -> PickerView {
    let main = match &state.mode {
        Mode::Loading { message, .. } => Main::Loading {
            message: sanitize(message).into_owned(),
            hint: loading_hint(keys),
        },
        Mode::RepoSelect => Main::List(repo_list(state, viewports.list)),
        Mode::BranchSelect(_)
        | Mode::ValidatingNewBranch { .. }
        | Mode::SelectBaseBranch { .. }
        | Mode::ConfirmRemoteBranch { .. }
        | Mode::ConfirmWorktreeDelete(_) => Main::List(branch_list(state, keys, viewports.list)),
    };
    let dialog = match &mut state.mode {
        Mode::ValidatingNewBranch { name, .. } => Some(Dialog::ValidatingBranch {
            title: new_branch_title(name),
            message: VALIDATING_BRANCH,
        }),
        Mode::SelectBaseBranch { context, flow } => Some(Dialog::BaseBranch {
            title: base_branch_title(&flow.new_name),
            list: base_list(&context.repo_path, flow, viewports.bases),
        }),
        Mode::ConfirmRemoteBranch { context, prompt } => {
            Some(remote_branch_dialog(&context.repo_path, prompt, keys))
        }
        Mode::ConfirmWorktreeDelete(flow) => {
            Some(delete_dialog(flow, state.product.backend_name, keys))
        }
        Mode::RepoSelect | Mode::BranchSelect(_) | Mode::Loading { .. } => None,
    };
    let toast = toast(state, keys);
    let toast_visible = toast.is_some();
    let warnings = state.scan_warnings().to_vec();
    let overlay = match &mut state.overlay {
        Some(Overlay::Help(help)) => Some(OverlayView::Help(help_view(
            help,
            toast_visible,
            viewports.help,
        ))),
        Some(Overlay::ScanWarnings(overlay)) => {
            let total = warnings.len();
            let viewport = viewports.scan_warnings.max(1);
            overlay.scroll_offset = overlay.scroll_offset.min(total.saturating_sub(viewport));
            Some(OverlayView::ScanWarnings(ScanWarningsView {
                title: scan_summary(total),
                warnings: warnings
                    .iter()
                    .skip(overlay.scroll_offset)
                    .take(viewport)
                    .map(|warning| ScanWarningView {
                        path: sanitize(&warning.path.display().to_string()).into_owned(),
                        message: sanitize(&warning.message).into_owned(),
                    })
                    .collect(),
                offset: overlay.scroll_offset,
                total,
                hints: scan_warnings_hints(keys),
            }))
        }
        None => None,
    };
    PickerView {
        main,
        dialog,
        overlay,
        toast,
        hints: footer_hints(keys, state),
    }
}

/// The notification in front, if any.
pub fn toast(state: &AppState, keys: &KeysConfig) -> Option<ToastView> {
    state.toasts.front().map(|toast| ToastView {
        level: match toast.kind {
            ToastKind::Warning => ToastLevel::Warning,
            ToastKind::Error => ToastLevel::Error,
        },
        label: toast_label(toast.kind),
        message: sanitize(&toast.message).into_owned(),
        counter: toast_counter(state.toasts.len()),
        hints: toast_hints(keys, toast),
    })
}

/// A list's rows in view, scrolled so the selection is among them.
fn rows_in_view<T>(
    list: &mut SearchableList,
    entries: &[T],
    viewport_rows: usize,
    row: impl Fn(&T) -> Row,
) -> (Vec<Row>, Option<usize>) {
    list.update_scroll_offset(viewport_rows);
    let visible = list.visible_items(viewport_rows);
    let selected = list.selected.and_then(|selected| {
        visible
            .iter()
            .position(|(position, _)| *position == selected)
    });
    let rows = visible
        .iter()
        .filter_map(|(position, index)| {
            entries.get(*index).map(|entry| Row {
                position: *position,
                ..row(entry)
            })
        })
        .collect();
    (rows, selected)
}

/// The repository list, scrolled so its selection is among its `viewport_rows` rows. Until
/// the person moves it, the selection is the repository they are in, which goes in the
/// middle of the view so the repositories either side of it show too.
pub fn repo_list(state: &mut AppState, viewport_rows: usize) -> ListView {
    let view = &mut state.repo_view;
    let annotations = &state.annotations;
    let query = view.list.input.text.clone();
    if !view.selection_touched {
        view.list.centre_scroll_offset(viewport_rows);
    }
    let (rows, selected) = rows_in_view(&mut view.list, &view.entries, viewport_rows, |entry| {
        repo_row(entry, annotations, &query)
    });
    let empty = if view.loading {
        rows.is_empty().then(|| DISCOVERING_REPOS.to_owned())
    } else if view.entries.is_empty() {
        Some(no_repos_message(view.missing_optional_dirs))
    } else {
        rows.is_empty().then(|| NO_MATCHING_REPOS.to_owned())
    };
    ListView {
        kind: ListKind::Repos,
        title: repo_list_title(state.product.name),
        subject: None,
        placeholder: REPO_PLACEHOLDER,
        query: Query::new(&view.list.input),
        summary: list_summary(view.list.filtered.len(), view.entries.len(), "repos"),
        activity: view.loading.then_some(SCANNING),
        offset: view.list.scroll_offset,
        total: view.list.filtered.len(),
        rows,
        selected,
        empty,
    }
}

fn repo_row(entry: &RepoEntry, annotations: &AnnotationIndex, query: &str) -> Row {
    let name = entry.display_name();
    Row {
        id: repo_id(entry),
        position: 0,
        name: Segment::new(sanitize(&name).into_owned(), Tone::Normal),
        matches: name_matches(query, "", &name),
        badges: if entry.repo.is_git {
            Vec::new()
        } else {
            vec![Badge::Folder]
        },
        labels: labels(annotations.repo(&entry.repo.path)),
        open: entry.is_open,
        actions: repo_actions(entry),
    }
}

fn repo_actions(entry: &RepoEntry) -> RowActions {
    RowActions {
        branches: entry.repo.is_git,
        delete: false,
    }
}

/// Where `query` matched `name`, which was searched as `prefix` and `name` together: the
/// matched characters' positions in `name` as `sanitize` shows it.
fn name_matches(query: &str, prefix: &str, name: &str) -> Vec<usize> {
    let skipped = prefix.chars().count();
    let in_name: Vec<usize> = match_positions(query, &format!("{prefix}{name}"))
        .into_iter()
        .filter_map(|position| position.checked_sub(skipped))
        .collect();
    sanitized_positions(name, &in_name)
}

/// The branch list, scrolled so its selection is among its `viewport_rows` rows.
pub fn branch_list(state: &mut AppState, keys: &KeysConfig, viewport_rows: usize) -> ListView {
    let context = state.branch_context();
    let repo_name = context.map_or_else(|| "?".to_owned(), |context| context.repo_name.clone());
    let repo_path = context
        .map(|context| context.repo_path.clone())
        .unwrap_or_default();
    let main_checkout = main_checkout(state);
    let creates = creates_branch(state, keys);
    let view = &mut state.branch_view;
    let annotations = &state.annotations;
    let search = view.list.input.text.clone();
    let (rows, selected) = rows_in_view(&mut view.list, &view.entries, viewport_rows, |branch| {
        branch_row(
            &repo_path,
            branch,
            annotations,
            main_checkout.as_deref(),
            &search,
        )
    });
    let query = Query::new(&view.list.input);
    let empty = if view.is_loading() {
        rows.is_empty().then(|| LOADING_BRANCHES.to_owned())
    } else {
        creates.map(|key| create_branch_message(&key, &search))
    };
    ListView {
        kind: ListKind::Branches,
        title: branch_list_title(&repo_name),
        subject: Some(sanitize(&repo_name).into_owned()),
        placeholder: BRANCH_PLACEHOLDER,
        query,
        summary: list_summary(view.list.filtered.len(), view.entries.len(), "branches"),
        activity: branch_activity(view.load()),
        offset: view.list.scroll_offset,
        total: view.list.filtered.len(),
        rows,
        selected,
        empty,
    }
}

/// The repository's own checkout in the branch list, which cannot be removed.
fn main_checkout(state: &AppState) -> Option<std::path::PathBuf> {
    state
        .branch_context()
        .map(|context| crate::path::canonical_or_original(&context.repo_path))
}

/// The open key's chord when no branch matches what is typed in the branch list, so opening
/// creates a branch of that name.
fn creates_branch(state: &AppState, keys: &KeysConfig) -> Option<KeyChord> {
    let view = &state.branch_view;
    (!view.is_loading() && view.list.filtered.is_empty() && !view.list.input.text.is_empty())
        .then(|| keys.first_key(BindingMode::Branch, Command::Open))
        .flatten()
}

fn in_main_checkout(branch: &BranchEntry, main_checkout: Option<&Path>) -> bool {
    branch
        .worktree_path()
        .is_some_and(|path| main_checkout.is_some_and(|main| crate::path::equivalent(path, main)))
}

/// `main_checkout` is the repository's own checkout, which cannot be removed.
fn branch_actions(branch: &BranchEntry, main_checkout: Option<&Path>) -> RowActions {
    RowActions {
        branches: false,
        delete: branch.remote.is_none()
            && branch.worktree_path().is_some()
            && !in_main_checkout(branch, main_checkout),
    }
}

/// `main_checkout` is the repository's own checkout, which cannot be removed.
fn branch_row(
    repo: &Path,
    branch: &BranchEntry,
    annotations: &AnnotationIndex,
    main_checkout: Option<&Path>,
    query: &str,
) -> Row {
    let branch_in_main_checkout = in_main_checkout(branch, main_checkout);
    let (name, badges) = if let Some(remote) = &branch.remote {
        (
            Segment::new(sanitize(&branch.name).into_owned(), Tone::Faint),
            vec![Badge::Remote {
                name: sanitize(remote).into_owned(),
            }],
        )
    } else {
        let badges = [
            (branch_in_main_checkout, Badge::MainCheckout),
            (
                branch.checkout.is_some() && !branch_in_main_checkout,
                Badge::Worktree,
            ),
            (branch.is_current, Badge::Current),
            (branch.is_default, Badge::Default),
        ]
        .into_iter()
        .filter_map(|(shown, badge)| shown.then_some(badge))
        .collect();
        (
            Segment::new(sanitize(&branch.name).into_owned(), Tone::Normal),
            badges,
        )
    };
    let labels = branch
        .worktree_path()
        .map_or(&[][..], |path| annotations.checkout(path));
    // The list is searched by the name a terminal shows, `origin/feature` for a remote's.
    let prefix = branch
        .remote
        .as_ref()
        .map_or_else(String::new, |remote| format!("{remote}/"));
    Row {
        id: branch_id(repo, branch),
        position: 0,
        name,
        matches: name_matches(query, &prefix, &branch.name),
        badges,
        labels: self::labels(labels),
        open: branch.is_open(),
        actions: branch_actions(branch, main_checkout),
    }
}

/// The new branch's base choices, scrolled so the selection is among `viewport_rows` rows.
pub fn base_list(repo: &Path, flow: &mut BaseBranchSelection, viewport_rows: usize) -> ListView {
    let search = flow.list.input.text.clone();
    let (rows, selected) = rows_in_view(&mut flow.list, &flow.bases, viewport_rows, |base| Row {
        id: base_id(repo, &flow.new_name, base),
        position: 0,
        name: Segment::new(sanitize(base).into_owned(), Tone::Normal),
        matches: name_matches(&search, "", base),
        badges: Vec::new(),
        labels: Vec::new(),
        open: false,
        actions: RowActions::default(),
    });
    ListView {
        kind: ListKind::Bases,
        title: BASE_BRANCH_TITLE.to_owned(),
        subject: Some(sanitize(&flow.new_name).into_owned()),
        placeholder: BASE_BRANCH_PLACEHOLDER,
        query: Query::new(&flow.list.input),
        summary: base_summary(flow.list.filtered.len()),
        activity: None,
        offset: flow.list.scroll_offset,
        total: flow.list.filtered.len(),
        empty: rows.is_empty().then(|| NO_MATCHING_BASES.to_owned()),
        rows,
        selected,
    }
}

fn labels(labels: &[Label]) -> Vec<Segment> {
    labels
        .iter()
        .map(|label| Segment::new(sanitize(&label.text).into_owned(), label.tone.into()))
        .collect()
}

/// What identifies the item at `position` of the list in front, the same however that list
/// is sorted, filtered or scrolled, and different in every other list: its kind and what it
/// is, such as a repository's path. A click names the item it was on, so a click on a row
/// that has since moved, or on a list that has since gone, acts on nothing.
pub fn item_id(state: &AppState, position: usize) -> Option<String> {
    let index = |list: &SearchableList| list.filtered.get(position).map(|(index, _)| *index);
    match &state.overlay {
        Some(Overlay::Help(help)) => {
            let row_index = index(&help.list)?;
            return help.rows.get(row_index).map(|row| help_id(row_index, row));
        }
        Some(Overlay::ScanWarnings(_)) => return None,
        None => {}
    }
    match &state.mode {
        Mode::RepoSelect => state
            .repo_view
            .entries
            .get(index(&state.repo_view.list)?)
            .map(repo_id),
        Mode::BranchSelect(context) => state
            .branch_view
            .entries
            .get(index(&state.branch_view.list)?)
            .map(|branch| branch_id(&context.repo_path, branch)),
        Mode::SelectBaseBranch { context, flow } => flow
            .bases
            .get(index(&flow.list)?)
            .map(|base| base_id(&context.repo_path, &flow.new_name, base)),
        Mode::ConfirmRemoteBranch { context, prompt } => prompt
            .choices()
            .get(position)
            .map(|choice| choice_id(&context.repo_path, prompt, choice)),
        Mode::ValidatingNewBranch { .. }
        | Mode::ConfirmWorktreeDelete(_)
        | Mode::Loading { .. } => None,
    }
}

/// What an [`item_id`] encodes: everything that tells one item from another, typed, so
/// that no two items share an id however their names look.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ItemId<'a> {
    Repo {
        path: PathKey<'a>,
    },
    Branch {
        repo: PathKey<'a>,
        remote: Option<&'a str>,
        name: &'a str,
    },
    Base {
        repo: PathKey<'a>,
        new_branch: &'a str,
        base: &'a str,
    },
    Track {
        repo: PathKey<'a>,
        new_branch: &'a str,
        remote: Option<&'a str>,
        name: &'a str,
    },
    Create {
        repo: PathKey<'a>,
        new_branch: &'a str,
    },
    Help {
        index: usize,
        command: &'a str,
    },
}

/// A path in an [`ItemId`], encoded exactly: paths need not be UTF-8, and two that are
/// not can display alike.
struct PathKey<'a>(&'a Path);

impl Serialize for PathKey<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use std::fmt::Write as _;
        let mut hex = String::new();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            for byte in self.0.as_os_str().as_bytes() {
                let _ = write!(hex, "{byte:02x}");
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            for unit in self.0.as_os_str().encode_wide() {
                let _ = write!(hex, "{unit:04x}");
            }
        }
        #[cfg(not(any(unix, windows)))]
        for byte in self.0.to_string_lossy().as_bytes() {
            let _ = write!(hex, "{byte:02x}");
        }
        serializer.serialize_str(&hex)
    }
}

impl ItemId<'_> {
    fn encode(&self) -> String {
        serde_json::to_string(self).expect("an item id holds only strings and numbers")
    }
}

fn repo_id(entry: &RepoEntry) -> String {
    ItemId::Repo {
        path: PathKey(&entry.repo.path),
    }
    .encode()
}

fn branch_id(repo: &Path, branch: &BranchEntry) -> String {
    ItemId::Branch {
        repo: PathKey(repo),
        remote: branch.remote.as_deref(),
        name: &branch.name,
    }
    .encode()
}

fn base_id(repo: &Path, new_branch: &str, base: &str) -> String {
    ItemId::Base {
        repo: PathKey(repo),
        new_branch,
        base,
    }
    .encode()
}

fn choice_id(repo: &Path, prompt: &RemoteBranchPrompt, choice: &RemoteBranchChoice) -> String {
    match choice {
        RemoteBranchChoice::Track(branch) => ItemId::Track {
            repo: PathKey(repo),
            new_branch: prompt.name(),
            remote: branch.remote.as_deref(),
            name: &branch.name,
        },
        RemoteBranchChoice::CreateNew => ItemId::Create {
            repo: PathKey(repo),
            new_branch: prompt.name(),
        },
    }
    .encode()
}

fn help_id(index: usize, row: &crate::state::HelpBindingRow) -> String {
    ItemId::Help {
        index,
        command: row.command_name,
    }
    .encode()
}

// The picker's wording, shared by every frontend.

pub const REPO_PLACEHOLDER: &str = "Type to search repos…";
pub const BRANCH_PLACEHOLDER: &str = "Type to search branches…";
pub const BASE_BRANCH_TITLE: &str = "Base branch";
pub const BASE_BRANCH_PLACEHOLDER: &str = "Type to filter local branches…";
pub const DISCOVERING_REPOS: &str = "Discovering repos…";
pub const LOADING_BRANCHES: &str = "Loading branches…";
pub const NO_MATCHING_REPOS: &str = "No matching repos";
pub const NO_MATCHING_BASES: &str = "No matching branches";
pub const VALIDATING_BRANCH: &str = "Validating branch name…";
pub const REMOVING_CHECKOUT: &str = "Removing checkout…";
pub const UNCOMMITTED_CHANGES: &str = "This checkout has uncommitted changes.";
pub const CONFIRM_DELETE_TITLE: &str = "Confirm delete";
pub const SCANNING: &str = "scanning";
pub const FOLDER_SUFFIX: &str = "  dir";
pub const WORKTREE_SUFFIX: &str = " (worktree)";
pub const CURRENT_SUFFIX: &str = " *";
pub const DEFAULT_SUFFIX: &str = " (default)";
pub const HELP_TITLE: &str = "Help — active key bindings";
pub const HELP_PLACEHOLDER: &str = "Filter by key, command, or description…";
pub const HELP_TOAST_NOTICE: &str =
    "After help closes, notification dismissal takes precedence over mode actions.";
pub const NO_MATCHING_BINDINGS: &str = "No matching bindings";

pub fn repo_list_title(product_name: &str) -> String {
    format!("{} — select repo", sanitize(product_name))
}

pub fn branch_list_title(repo_name: &str) -> String {
    format!("{} — select branch", sanitize(repo_name))
}

/// Such as `3 of 40 repos`.
pub fn list_summary(matching: usize, total: usize, noun: &str) -> String {
    format!("{matching} of {total} {noun}")
}

pub fn base_summary(matching: usize) -> String {
    format!("{matching} bases")
}

pub fn no_repos_message(missing_optional_dirs: usize) -> String {
    match missing_optional_dirs {
        0 => "No repos found in the configured search directories".into(),
        1 => "No repos found (1 optional search directory not found)".into(),
        count => format!("No repos found ({count} optional search directories not found)"),
    }
}

/// Shown when no branch matches the query, which the open key then creates.
pub fn create_branch_message(open_key: &KeyChord, name: &str) -> String {
    format!(
        "No matching branches. {open_key} creates {}",
        sanitize(name)
    )
}

/// What the branch list is still waiting for: its branches, then remote fetches.
pub const fn branch_activity(load: &BranchLoad) -> Option<&'static str> {
    match load {
        BranchLoad::Loading { .. } => Some("loading"),
        BranchLoad::Ready {
            fetching_remotes: true,
        } => Some("fetching"),
        BranchLoad::Ready {
            fetching_remotes: false,
        } => None,
    }
}

pub fn new_branch_title(name: &str) -> String {
    format!("New branch \"{}\"", sanitize(name))
}

pub fn base_branch_title(name: &str) -> String {
    format!("New branch \"{}\" — pick base", sanitize(name))
}

pub fn loading_hint(keys: &KeysConfig) -> Option<String> {
    keys.first_key(BindingMode::Modal, Command::Quit)
        .map(|key| format!("{key} to close (operation continues)"))
}

pub fn confirm_cancel_keys(keys: &KeysConfig) -> (Option<KeyChord>, Option<KeyChord>) {
    (
        keys.first_key(BindingMode::Modal, Command::Open),
        keys.first_key(BindingMode::Modal, Command::Back),
    )
}

/// A dialog's hints: its open key, labelled with what it does (`confirm`), and its back key.
pub fn confirm_cancel_hints(keys: &KeysConfig, confirm: &'static str) -> Vec<Hint> {
    let (confirm_key, cancel) = confirm_cancel_keys(keys);
    confirm_key
        .map(|key| Hint::for_key(key, confirm))
        .into_iter()
        .chain(cancel.map(|key| Hint::for_key(key, "cancel")))
        .collect()
}

/// Combines the up and down keys into one hint when both are bound.
pub fn move_hints(keys: &KeysConfig, mode: BindingMode) -> Vec<Hint> {
    move_hints_labelled(keys, mode, ["move", "move up", "move down"])
}

fn move_hints_labelled(
    keys: &KeysConfig,
    mode: BindingMode,
    [both, up_only, down_only]: [&'static str; 3],
) -> Vec<Hint> {
    match (
        keys.preferred_key(mode, Command::MoveUp),
        keys.preferred_key(mode, Command::MoveDown),
    ) {
        (Some(up), Some(down)) => vec![Hint::new(format!("{up}/{down}"), both)],
        (Some(up), None) => vec![Hint::for_key(up, up_only)],
        (None, Some(down)) => vec![Hint::for_key(down, down_only)],
        (None, None) => Vec::new(),
    }
}

pub fn scan_warnings_hints(keys: &KeysConfig) -> Vec<Hint> {
    let mut hints = move_hints_labelled(
        keys,
        SCAN_WARNINGS_BINDING_MODE,
        ["scroll", "scroll up", "scroll down"],
    );
    hints.push(Hint::new("esc", "close"));
    hints
}

/// `repo` is the repository the new branch is for.
pub fn remote_branch_dialog(repo: &Path, prompt: &RemoteBranchPrompt, keys: &KeysConfig) -> Dialog {
    let name = sanitize(prompt.name());
    let lines = match prompt.remote_count() {
        0 if prompt.remotes_pending() => vec![format!(
            "Remotes are still loading, so {name} may already exist on one."
        )],
        0 => vec![format!("No remote has a branch named {name}.")],
        count => {
            let mut lines = vec![if count == 1 {
                format!("{name} already exists on a remote.")
            } else {
                format!("{name} already exists on {count} remotes.")
            }];
            if prompt.remotes_pending() {
                lines.push("Other remotes are still loading.".into());
            }
            lines
        }
    };
    let choices = prompt
        .choices()
        .iter()
        .map(|choice| Choice {
            label: match choice {
                RemoteBranchChoice::Track(branch) => {
                    format!("Track {}", sanitize(&branch.display_name()))
                }
                RemoteBranchChoice::CreateNew => format!("Create new branch {name}"),
            },
            id: choice_id(repo, prompt, choice),
        })
        .collect();
    let title = match (prompt.remote_count(), prompt.remotes_pending()) {
        (0, true) => "Remotes still loading",
        (0, false) => "New branch",
        _ => "Branch exists on remote",
    };
    Dialog::RemoteBranch {
        title,
        lines,
        choices,
        selected: prompt.selected(),
        hints: [
            move_hints(keys, BindingMode::Choice),
            confirm_cancel_hints(keys, REMOTE_BRANCH_CONFIRM),
        ]
        .concat(),
    }
}

/// The confirmation for removing a checkout. `backend_name` names where an open checkout
/// is also closed.
pub fn delete_dialog(flow: &DeleteFlowState, backend_name: &str, keys: &KeysConfig) -> Dialog {
    let target = flow.target();
    let path = sanitize(&crate::path::display(target.worktree_path())).into_owned();
    let mut lines = vec![if target.force() {
        format!("Force-remove {path}?")
    } else {
        format!("Remove checkout {path}?")
    }];
    let closes_in = target
        .is_open()
        .then(|| sanitize(backend_name).into_owned());
    if let Some(backend) = &closes_in {
        lines.push(format!("It will also be closed in {backend}."));
    }
    Dialog::ConfirmDelete {
        title: CONFIRM_DELETE_TITLE,
        warning: target.force().then_some(UNCOMMITTED_CHANGES),
        lines,
        path,
        force: target.force(),
        closes_in,
        busy: target.in_progress().then_some(REMOVING_CHECKOUT),
        hints: confirm_cancel_hints(keys, DELETE_CONFIRM),
    }
}

pub const fn toast_label(kind: ToastKind) -> &'static str {
    match kind {
        ToastKind::Warning => "Warning",
        ToastKind::Error => "Error",
    }
}

/// Such as `1/3`, when more notifications wait behind the one shown.
pub fn toast_counter(queued: usize) -> Option<String> {
    (queued > 1).then(|| format!("1/{queued}"))
}

pub fn toast_hints(keys: &KeysConfig, toast: &Toast) -> Vec<Hint> {
    toast
        .is_scan_summary()
        .then(|| keys.scan_warnings_key())
        .flatten()
        .map(|key| Hint::for_key(key, "details"))
        .into_iter()
        .chain(
            keys.dismiss_toast_key()
                .map(|key| Hint::for_key(key, "dismiss")),
        )
        .collect()
}

/// The footer's hints for `mode`, in the order they are drawn.
/// What the open key is called in the remote-branch and remove dialogs, alike in their own
/// hints and the footer's.
const REMOTE_BRANCH_CONFIRM: &str = "confirm";
const DELETE_CONFIRM: &str = "remove";

/// What the open key acts on, which decides the hints offered for the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// A row, and what else can be done to it.
    Row(RowActions),
    /// No branch matches what is typed, so opening creates a branch of that name.
    NewBranch,
    /// Nothing matches, so there is nothing to open.
    Nothing,
}

impl Selection {
    pub fn of(state: &AppState, keys: &KeysConfig) -> Self {
        let row = match &state.mode {
            Mode::RepoSelect => state.selected_repo().map(repo_actions),
            Mode::BranchSelect(_) => {
                if creates_branch(state, keys).is_some() {
                    return Self::NewBranch;
                }
                // Delete is offered only when pressing it would start removing the checkout.
                state.selected_branch().map(|_| RowActions {
                    branches: false,
                    delete: crate::picker::delete::selected_target(state).is_ok(),
                })
            }
            Mode::SelectBaseBranch { flow, .. } => flow
                .list
                .selected
                .filter(|selected| *selected < flow.list.filtered.len())
                .map(|_| RowActions::default()),
            Mode::ValidatingNewBranch { .. }
            | Mode::ConfirmRemoteBranch { .. }
            | Mode::ConfirmWorktreeDelete(_)
            | Mode::Loading { .. } => Some(RowActions::default()),
        };
        row.map_or(Self::Nothing, Self::Row)
    }
}

pub fn footer_hints(keys: &KeysConfig, state: &AppState) -> Vec<Hint> {
    let mode = &state.mode;
    let selection = Selection::of(state, keys);
    let actions = match selection {
        Selection::Row(actions) => Some(actions),
        Selection::NewBranch | Selection::Nothing => None,
    };
    let binding_mode = KeysConfig::mode_for(mode);
    let mut hints = Vec::new();
    let mut add = |command, label: &'static str| {
        if let Some(key) = keys.preferred_key(binding_mode, command) {
            hints.push(Hint::for_key(key, label));
        }
    };
    if matches!(
        mode,
        Mode::ValidatingNewBranch { .. } | Mode::Loading { .. }
    ) {
        add(Command::Quit, "quit");
        return hints;
    }
    match selection {
        Selection::Nothing => {}
        Selection::NewBranch => add(Command::Open, "create"),
        Selection::Row(_) => add(
            Command::Open,
            match mode {
                Mode::SelectBaseBranch { .. } => "create",
                Mode::ConfirmWorktreeDelete(_) => DELETE_CONFIRM,
                Mode::ConfirmRemoteBranch { .. } => REMOTE_BRANCH_CONFIRM,
                Mode::RepoSelect
                | Mode::BranchSelect(_)
                | Mode::ValidatingNewBranch { .. }
                | Mode::Loading { .. } => "open",
            },
        ),
    }
    if matches!(mode, Mode::RepoSelect) && actions.is_some_and(|actions| actions.branches) {
        add(Command::BranchesView, "branches");
    }
    if matches!(mode, Mode::BranchSelect(_)) {
        add(Command::NewBranch, "new");
        if actions.is_some_and(|actions| actions.delete) {
            add(Command::Delete, "delete");
        }
    }
    if matches!(
        mode,
        Mode::BranchSelect(_)
            | Mode::SelectBaseBranch { .. }
            | Mode::ConfirmRemoteBranch { .. }
            | Mode::ConfirmWorktreeDelete(_)
    ) {
        add(Command::Back, "back");
    }
    add(Command::Help, "help");
    add(Command::Quit, "quit");
    hints
}

/// The help entries for every match, with a heading where a section starts.
pub fn help_entries(overlay: &HelpOverlayState) -> Vec<HelpEntry> {
    let mut entries = Vec::new();
    let mut current_section = None;
    for (position, (row_index, _)) in overlay.list.filtered.iter().enumerate() {
        let Some(row) = overlay.rows.get(*row_index) else {
            continue;
        };
        if current_section != Some(row.section_name) {
            if current_section.is_some() {
                entries.push(HelpEntry::Blank);
            }
            current_section = Some(row.section_name);
            entries.push(HelpEntry::Section {
                name: row.section_name,
            });
        }
        entries.push(HelpEntry::Binding {
            position,
            id: help_id(*row_index, row),
            keys: row.keys.clone(),
            description: row.description,
            command: row.command_name,
        });
    }
    entries
}

/// Scrolls the help overlay so its selection is among the `viewport_rows` entries in view,
/// headings and gaps included, and returns the index of the selected entry.
pub fn scroll_help(
    overlay: &mut HelpOverlayState,
    entries: &[HelpEntry],
    viewport_rows: usize,
) -> Option<usize> {
    if overlay.list.filtered.is_empty() {
        overlay.list.scroll_offset = 0;
        return None;
    }
    let binding_entries = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| matches!(entry, HelpEntry::Binding { .. }).then_some(index))
        .collect::<Vec<_>>();
    let selected = overlay
        .list
        .selected
        .unwrap_or_default()
        .min(overlay.list.filtered.len() - 1);
    let selected_entry = binding_entries.get(selected).copied().unwrap_or_default();
    let viewport_rows = viewport_rows.max(1);
    if selected_entry < overlay.list.scroll_offset {
        overlay.list.scroll_offset = selected_entry;
    } else if selected_entry >= overlay.list.scroll_offset.saturating_add(viewport_rows) {
        overlay.list.scroll_offset = selected_entry + 1 - viewport_rows;
    }
    overlay.list.scroll_offset = overlay
        .list
        .scroll_offset
        .min(entries.len().saturating_sub(viewport_rows));
    overlay
        .list
        .selected
        .and_then(|selected| binding_entries.get(selected))
        .copied()
}

pub fn help_summary(matching: usize) -> String {
    format!("{matching} commands (esc: close)")
}

fn help_view(
    overlay: &mut HelpOverlayState,
    toast_visible: bool,
    viewport_rows: usize,
) -> HelpView {
    let entries = help_entries(overlay);
    let selected = scroll_help(overlay, &entries, viewport_rows);
    let offset = overlay.list.scroll_offset;
    let total = entries.len();
    HelpView {
        offset,
        total,
        title: HELP_TITLE,
        placeholder: HELP_PLACEHOLDER,
        query: Query::new(&overlay.list.input),
        notice: toast_visible.then_some(HELP_TOAST_NOTICE),
        summary: help_summary(overlay.list.filtered.len()),
        empty: entries.is_empty().then_some(NO_MATCHING_BINDINGS),
        selected: selected.and_then(|selected| selected.checked_sub(offset)),
        entries: entries
            .into_iter()
            .skip(offset)
            .take(viewport_rows.max(1))
            .collect(),
    }
}

#[cfg(test)]
mod tests;
