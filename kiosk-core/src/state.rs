use crate::{
    config::keys::{Command, FlattenedKeybindingRow},
    constants::{WORKTREE_DIR_DEDUP_MAX_ATTEMPTS, WORKTREE_DIR_NAME, WORKTREE_NAME_SEPARATOR},
    git::Repo,
    pending_delete::PendingWorktreeDelete,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use unicode_segmentation::UnicodeSegmentation;

/// Shared state for any searchable, filterable list.
/// Eliminates the mode-dispatch triplication for search/cursor/movement.
#[derive(Debug, Clone)]
pub struct SearchableList {
    pub search: String,
    pub cursor: usize,
    /// Index-score pairs, sorted by score descending
    pub filtered: Vec<(usize, i64)>,
    pub selected: Option<usize>,
    pub scroll_offset: usize,
}

#[derive(Clone, Copy)]
struct GraphemeSpan {
    start: usize,
    end: usize,
    is_whitespace: bool,
}

impl SearchableList {
    fn grapheme_spans(&self) -> Vec<GraphemeSpan> {
        self.search
            .grapheme_indices(true)
            .map(|(start, grapheme)| GraphemeSpan {
                start,
                end: start + grapheme.len(),
                is_whitespace: grapheme.chars().all(char::is_whitespace),
            })
            .collect()
    }

    fn grapheme_boundaries(&self) -> Vec<usize> {
        let mut boundaries: Vec<usize> =
            self.search.grapheme_indices(true).map(|(i, _)| i).collect();
        boundaries.push(self.search.len());
        boundaries
    }

    fn boundaries_from_spans(spans: &[GraphemeSpan], text_len: usize) -> Vec<usize> {
        let mut boundaries = Vec::with_capacity(spans.len().saturating_add(1));
        for span in spans {
            boundaries.push(span.start);
        }
        boundaries.push(text_len);
        boundaries
    }

    fn boundary_index_at_or_before(boundaries: &[usize], cursor: usize) -> usize {
        match boundaries.binary_search(&cursor) {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        }
    }

    fn clamp_cursor_to_boundary(&mut self, boundaries: &[usize]) -> usize {
        let cursor = self.cursor.min(self.search.len());
        let idx = Self::boundary_index_at_or_before(boundaries, cursor);
        self.cursor = boundaries.get(idx).copied().unwrap_or(0);
        idx
    }

    fn prev_word_boundary(&self, from: usize) -> usize {
        let spans = self.grapheme_spans();
        if spans.is_empty() {
            return 0;
        }
        let boundaries = Self::boundaries_from_spans(&spans, self.search.len());
        let cursor = from.min(self.search.len());
        let mut grapheme_idx =
            Self::boundary_index_at_or_before(&boundaries, cursor).saturating_sub(1);

        while let Some(span) = spans.get(grapheme_idx) {
            if !span.is_whitespace {
                break;
            }
            if grapheme_idx == 0 {
                return 0;
            }
            grapheme_idx -= 1;
        }

        while let Some(span) = spans.get(grapheme_idx) {
            if span.is_whitespace {
                return span.end;
            }
            if grapheme_idx == 0 {
                return 0;
            }
            grapheme_idx -= 1;
        }

        0
    }

    fn next_word_boundary(&self, from: usize) -> usize {
        let spans = self.grapheme_spans();
        if spans.is_empty() {
            return 0;
        }
        let boundaries = Self::boundaries_from_spans(&spans, self.search.len());
        let cursor = from.min(self.search.len());
        let mut grapheme_idx = Self::boundary_index_at_or_before(&boundaries, cursor);

        while let Some(span) = spans.get(grapheme_idx) {
            if !span.is_whitespace {
                break;
            }
            grapheme_idx += 1;
        }

        while let Some(span) = spans.get(grapheme_idx) {
            if span.is_whitespace {
                return span.start;
            }
            grapheme_idx += 1;
        }

        self.search.len()
    }

    pub fn new(item_count: usize) -> Self {
        Self {
            search: String::new(),
            cursor: 0,
            filtered: (0..item_count).map(|i| (i, 0)).collect(),
            selected: if item_count > 0 { Some(0) } else { None },
            scroll_offset: 0,
        }
    }

    pub fn reset(&mut self, item_count: usize) {
        self.search.clear();
        self.cursor = 0;
        self.filtered = (0..item_count).map(|i| (i, 0)).collect();
        self.selected = if item_count > 0 { Some(0) } else { None };
        self.scroll_offset = 0;
    }

    /// Move selection by delta, clamping to bounds
    pub fn move_selection(&mut self, delta: i32) {
        let len = self.filtered.len();
        if len == 0 {
            return;
        }
        let current = self.selected.unwrap_or(0);
        if delta > 0 {
            self.selected = Some(
                current
                    .saturating_add(delta.unsigned_abs() as usize)
                    .min(len - 1),
            );
        } else {
            self.selected = Some(current.saturating_sub(delta.unsigned_abs() as usize));
        }
    }

    pub fn move_to_top(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = Some(0);
        }
    }

    pub fn move_to_bottom(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = Some(self.filtered.len() - 1);
        }
    }

    pub fn update_scroll_offset_for_selection(&mut self, viewport_rows: usize) {
        let len = self.filtered.len();
        if len == 0 {
            self.scroll_offset = 0;
            return;
        }

        let viewport_rows = viewport_rows.max(1);
        let max_offset = len.saturating_sub(viewport_rows);
        let selected = self.selected.unwrap_or(0).min(len - 1);
        let anchor_top = usize::from(viewport_rows > 2);
        let anchor_bottom = viewport_rows.saturating_sub(2);

        let top_bound = self.scroll_offset.saturating_add(anchor_top);
        let bottom_bound = self.scroll_offset.saturating_add(anchor_bottom);

        if selected < top_bound {
            self.scroll_offset = selected.saturating_sub(anchor_top);
        } else if selected > bottom_bound {
            self.scroll_offset = selected.saturating_sub(anchor_bottom);
        }

        self.scroll_offset = self.scroll_offset.min(max_offset);
    }

    /// Move cursor left by one grapheme cluster (UTF-8 safe)
    pub fn cursor_left(&mut self) {
        let boundaries = self.grapheme_boundaries();
        let idx = self.clamp_cursor_to_boundary(&boundaries);
        if idx > 0 {
            self.cursor = boundaries[idx - 1];
        }
    }

    /// Move cursor right by one grapheme cluster (UTF-8 safe)
    pub fn cursor_right(&mut self) {
        let boundaries = self.grapheme_boundaries();
        let idx = self.clamp_cursor_to_boundary(&boundaries);
        if idx + 1 < boundaries.len() {
            self.cursor = boundaries[idx + 1];
        }
    }

    pub fn cursor_start(&mut self) {
        self.cursor = 0;
    }

    pub fn cursor_end(&mut self) {
        self.cursor = self.search.len();
    }

    pub fn cursor_word_left(&mut self) {
        let boundaries = self.grapheme_boundaries();
        self.clamp_cursor_to_boundary(&boundaries);
        self.cursor = self.prev_word_boundary(self.cursor);
    }

    pub fn cursor_word_right(&mut self) {
        let boundaries = self.grapheme_boundaries();
        self.clamp_cursor_to_boundary(&boundaries);
        self.cursor = self.next_word_boundary(self.cursor);
    }

    /// Insert a character at the current cursor position
    pub fn insert_char(&mut self, c: char) {
        let boundaries = self.grapheme_boundaries();
        self.clamp_cursor_to_boundary(&boundaries);
        self.search.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Remove the grapheme cluster before the cursor (UTF-8 safe)
    pub fn backspace(&mut self) -> bool {
        let boundaries = self.grapheme_boundaries();
        let idx = self.clamp_cursor_to_boundary(&boundaries);
        if idx == 0 {
            return false;
        }
        let prev = boundaries[idx - 1];
        self.search.drain(prev..self.cursor);
        self.cursor = prev;
        true
    }

    /// Remove the grapheme cluster at cursor position (UTF-8 safe)
    pub fn delete_forward_char(&mut self) -> bool {
        let boundaries = self.grapheme_boundaries();
        let idx = self.clamp_cursor_to_boundary(&boundaries);
        if idx + 1 >= boundaries.len() {
            return false;
        }
        let end = boundaries[idx + 1];
        self.search.drain(self.cursor..end);
        true
    }

    /// Delete word backwards from cursor position
    pub fn delete_word(&mut self) {
        if self.search.is_empty() || self.cursor == 0 {
            return;
        }
        let boundaries = self.grapheme_boundaries();
        self.clamp_cursor_to_boundary(&boundaries);
        let new_cursor = self.prev_word_boundary(self.cursor);

        self.search.drain(new_cursor..self.cursor);
        self.cursor = new_cursor;
    }

    /// Delete word forwards from cursor position
    pub fn delete_word_forward(&mut self) {
        if self.search.is_empty() || self.cursor >= self.search.len() {
            return;
        }
        let boundaries = self.grapheme_boundaries();
        self.clamp_cursor_to_boundary(&boundaries);
        let end = self.next_word_boundary(self.cursor);
        self.search.drain(self.cursor..end);
    }

    pub fn delete_to_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let boundaries = self.grapheme_boundaries();
        self.clamp_cursor_to_boundary(&boundaries);
        self.search.drain(..self.cursor);
        self.cursor = 0;
    }

    pub fn delete_to_end(&mut self) {
        if self.cursor >= self.search.len() {
            return;
        }
        let boundaries = self.grapheme_boundaries();
        self.clamp_cursor_to_boundary(&boundaries);
        self.search.truncate(self.cursor);
    }
}

/// Rich branch entry with worktree and session metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct BranchEntry {
    pub name: String,
    /// If a worktree already exists for this branch
    pub worktree_path: Option<PathBuf>,
    pub has_session: bool,
    pub is_current: bool,
    /// Whether this is the default branch (main/master)
    pub is_default: bool,
    /// Remote-only branch (no local tracking branch)
    pub is_remote: bool,
    /// Last activity timestamp for the session (if any)
    pub session_activity_ts: Option<u64>,
}

impl BranchEntry {
    /// Build branch entries from a repo's branches, worktrees, and active tmux sessions
    /// (unsorted).
    pub fn build(
        repo: &crate::git::Repo,
        branch_names: &[String],
        active_sessions: &[String],
    ) -> Vec<Self> {
        Self::build_entries(
            repo,
            branch_names,
            active_sessions,
            None,
            &HashMap::new(),
            None,
        )
    }

    /// Build sorted branch entries from a repo's branches, worktrees, and active tmux sessions.
    ///
    /// Sorted by: sessions first, then worktrees, then alphabetical.
    pub fn build_sorted(
        repo: &crate::git::Repo,
        branch_names: &[String],
        active_sessions: &[String],
    ) -> Vec<Self> {
        let mut entries = Self::build(repo, branch_names, active_sessions);
        Self::sort_entries(&mut entries);
        entries
    }

    /// Build sorted branch entries with activity timestamps and default branch info.
    ///
    /// `cwd` is the user's current working directory (resolved to a repo/worktree root).
    /// When it matches a worktree path, that worktree's branch is marked as current.
    /// Falls back to the main worktree's branch when `cwd` is `None` or doesn't match.
    pub fn build_sorted_with_activity(
        repo: &crate::git::Repo,
        branch_names: &[String],
        active_sessions: &[String],
        default_branch: Option<&str>,
        session_activity: &HashMap<String, u64>,
        cwd: Option<&Path>,
    ) -> Vec<Self> {
        let mut entries = Self::build_entries(
            repo,
            branch_names,
            active_sessions,
            default_branch,
            session_activity,
            cwd,
        );
        Self::sort_entries(&mut entries);
        entries
    }

    fn build_entries(
        repo: &crate::git::Repo,
        branch_names: &[String],
        active_sessions: &[String],
        default_branch: Option<&str>,
        session_activity: &HashMap<String, u64>,
        cwd: Option<&Path>,
    ) -> Vec<Self> {
        let wt_by_branch: HashMap<&str, &crate::git::Worktree> = repo
            .worktrees
            .iter()
            .filter_map(|wt| wt.branch.as_deref().map(|b| (b, wt)))
            .collect();

        let current_branch = cwd
            .and_then(|p| repo.worktrees.iter().find(|wt| wt.path == p))
            .or_else(|| repo.worktrees.first())
            .and_then(|wt| wt.branch.as_deref());

        branch_names
            .iter()
            .map(|name| {
                let worktree_path = wt_by_branch.get(name.as_str()).map(|wt| wt.path.clone());
                let session_name = worktree_path.as_ref().map(|p| repo.tmux_session_name(p));
                let has_session = session_name
                    .as_ref()
                    .is_some_and(|sn| active_sessions.contains(sn));
                let is_current = current_branch == Some(name.as_str());
                let is_default = default_branch == Some(name.as_str());
                let session_activity_ts = session_name
                    .as_ref()
                    .and_then(|sn| session_activity.get(sn).copied());

                Self {
                    name: name.clone(),
                    worktree_path,
                    has_session,
                    is_current,
                    is_default,
                    is_remote: false,
                    session_activity_ts,
                }
            })
            .collect()
    }

    /// Build remote-only branch entries, skipping branches that already exist locally.
    pub fn build_remote(remote_names: &[String], local_names: &[String]) -> Vec<Self> {
        let local_set: std::collections::HashSet<&str> =
            local_names.iter().map(String::as_str).collect();

        remote_names
            .iter()
            .filter(|name| !local_set.contains(name.as_str()))
            .map(|name| Self {
                name: name.clone(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_default: false,
                is_remote: true,
                session_activity_ts: None,
            })
            .collect()
    }

    pub fn sort_entries(entries: &mut [Self]) {
        entries.sort_by(|a, b| {
            // Remote branches always sort after local
            a.is_remote
                .cmp(&b.is_remote)
                // Current branch first
                .then(b.is_current.cmp(&a.is_current))
                // Default branch second
                .then(b.is_default.cmp(&a.is_default))
                // Branches with sessions, ordered by recency (most recent first)
                .then(cmp_optional_recency(
                    a.session_activity_ts,
                    b.session_activity_ts,
                ))
                // Branches with sessions (even without activity timestamps) before those without
                .then(b.has_session.cmp(&a.has_session))
                // Branches with worktrees before those without
                .then(b.worktree_path.is_some().cmp(&a.worktree_path.is_some()))
                .then(a.name.cmp(&b.name))
        });
    }
}

/// Compare two optional timestamps for recency-based sorting (most recent first).
/// `Some` sorts before `None`; when both are `Some`, the higher timestamp sorts first.
fn cmp_optional_recency(a: Option<u64>, b: Option<u64>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Sort repos by: current repo first, then repos with sessions by recency, then alphabetically.
#[allow(clippy::implicit_hasher)]
pub fn sort_repos(
    repos: &mut [Repo],
    current_repo_path: Option<&Path>,
    session_activity: &HashMap<String, u64>,
) {
    let current_repo_path = current_repo_path
        .and_then(|path| std::fs::canonicalize(path).ok())
        .or_else(|| current_repo_path.map(ToOwned::to_owned));
    let mut canonical_by_path = HashMap::with_capacity(repos.len());
    for repo in repos.iter() {
        let canonical = std::fs::canonicalize(&repo.path).unwrap_or_else(|_| repo.path.clone());
        canonical_by_path.insert(repo.path.clone(), canonical);
    }
    repos.sort_by(|a, b| {
        let a_path = canonical_by_path.get(&a.path).unwrap_or(&a.path);
        let b_path = canonical_by_path.get(&b.path).unwrap_or(&b.path);
        let a_is_current = current_repo_path.as_ref().is_some_and(|p| a_path == p);
        let b_is_current = current_repo_path.as_ref().is_some_and(|p| b_path == p);

        // Current repo first
        b_is_current
            .cmp(&a_is_current)
            .then_with(|| {
                let a_activity = repo_max_activity(a, session_activity);
                let b_activity = repo_max_activity(b, session_activity);
                cmp_optional_recency(a_activity, b_activity)
            })
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Get the most recent session activity for a repo (across all its worktrees).
fn repo_max_activity(repo: &Repo, session_activity: &HashMap<String, u64>) -> Option<u64> {
    let main_session = std::iter::once(repo.tmux_session_name(&repo.path));
    let wt_sessions = repo
        .worktrees
        .iter()
        .map(|wt| repo.tmux_session_name(&wt.path));
    main_session
        .chain(wt_sessions)
        .filter_map(|name| session_activity.get(&name).copied())
        .max()
}

/// What mode the app is in
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    RepoSelect,
    BranchSelect,
    SelectBaseBranch,
    /// Blocking loading state — shows spinner, no input except Ctrl+C
    Loading(String),
    /// Confirmation dialog for worktree deletion
    ConfirmWorktreeDelete {
        branch_name: String,
        has_session: bool,
    },
    /// Help overlay showing key bindings
    Help {
        previous: Box<Mode>,
    },
}

impl Mode {
    /// Commands to show in the footer bar, in display order.
    pub fn footer_commands(&self) -> &'static [Command] {
        match self {
            Mode::RepoSelect => &[
                Command::OpenRepo,
                Command::EnterRepo,
                Command::ShowHelp,
                Command::Quit,
            ],
            Mode::BranchSelect => &[
                Command::GoBack,
                Command::NewBranch,
                Command::DeleteWorktree,
                Command::ShowHelp,
                Command::Quit,
            ],
            Mode::SelectBaseBranch => &[
                Command::Cancel,
                Command::Confirm,
                Command::ShowHelp,
                Command::Quit,
            ],
            Mode::ConfirmWorktreeDelete { .. } => &[
                Command::Confirm,
                Command::Cancel,
                Command::ShowHelp,
                Command::Quit,
            ],
            Mode::Loading(_) | Mode::Help { .. } => &[],
        }
    }

    pub(crate) fn supports_text_edit(&self) -> bool {
        matches!(
            self,
            Mode::RepoSelect | Mode::BranchSelect | Mode::SelectBaseBranch | Mode::Help { .. }
        )
    }

    pub(crate) fn supports_list_navigation(&self) -> bool {
        matches!(
            self,
            Mode::RepoSelect | Mode::BranchSelect | Mode::SelectBaseBranch | Mode::Help { .. }
        )
    }

    pub(crate) fn supports_modal_actions(&self) -> bool {
        matches!(
            self,
            Mode::SelectBaseBranch | Mode::ConfirmWorktreeDelete { .. }
        )
    }

    pub(crate) fn supports_repo_select_actions(&self) -> bool {
        matches!(self, Mode::RepoSelect)
    }

    pub(crate) fn supports_branch_select_actions(&self) -> bool {
        matches!(self, Mode::BranchSelect)
    }
}

/// The new-branch flow state
#[derive(Debug, Clone)]
pub struct BaseBranchSelection {
    /// The new branch name (what the user typed)
    pub new_name: String,
    /// Base branches to pick from
    pub bases: Vec<String>,
    pub list: SearchableList,
}

#[derive(Debug, Clone)]
pub struct HelpOverlayState {
    pub list: SearchableList,
    pub rows: Vec<FlattenedKeybindingRow>,
}

/// Central application state. Components read from this, actions modify it.
#[derive(Debug, Clone)]
pub struct AppState {
    pub repos: Vec<Repo>,
    pub repo_list: SearchableList,
    pub loading_repos: bool,

    pub selected_repo_idx: Option<usize>,
    pub branches: Vec<BranchEntry>,
    pub branch_list: SearchableList,

    pub base_branch_selection: Option<BaseBranchSelection>,
    pub help_overlay: Option<HelpOverlayState>,

    pub split_command: Option<String>,
    pub mode: Mode,
    pub loading_branches: bool,
    pub error: Option<String>,
    active_list_page_rows: usize,
    pub pending_worktree_deletes: Vec<PendingWorktreeDelete>,
    pub session_activity: HashMap<String, u64>,
    /// Main repo root path from CWD (for repo ordering)
    pub current_repo_path: Option<PathBuf>,
    /// CWD resolved to repo/worktree root (for branch current detection)
    pub cwd_worktree_path: Option<PathBuf>,
}

impl AppState {
    pub fn new(repos: Vec<Repo>, split_command: Option<String>) -> Self {
        let repo_list = SearchableList::new(repos.len());
        Self {
            repos,
            repo_list,
            loading_repos: false,
            selected_repo_idx: None,
            branches: Vec::new(),
            branch_list: SearchableList::new(0),
            base_branch_selection: None,
            help_overlay: None,
            split_command,
            mode: Mode::RepoSelect,
            loading_branches: false,
            error: None,
            active_list_page_rows: 10,
            pending_worktree_deletes: Vec::new(),
            session_activity: HashMap::new(),
            current_repo_path: None,
            cwd_worktree_path: None,
        }
    }

    pub fn new_loading(loading_message: &str, split_command: Option<String>) -> Self {
        Self {
            repos: Vec::new(),
            repo_list: SearchableList::new(0),
            loading_repos: false,
            selected_repo_idx: None,
            branches: Vec::new(),
            branch_list: SearchableList::new(0),
            base_branch_selection: None,
            help_overlay: None,
            split_command,
            mode: Mode::Loading(loading_message.to_string()),
            loading_branches: false,
            error: None,
            active_list_page_rows: 10,
            pending_worktree_deletes: Vec::new(),
            session_activity: HashMap::new(),
            current_repo_path: None,
            cwd_worktree_path: None,
        }
    }

    /// Get the active searchable list for the current mode (mutable)
    pub fn active_list_mut(&mut self) -> Option<&mut SearchableList> {
        match self.mode {
            Mode::RepoSelect => Some(&mut self.repo_list),
            Mode::BranchSelect => Some(&mut self.branch_list),
            Mode::SelectBaseBranch => self.base_branch_selection.as_mut().map(|f| &mut f.list),
            Mode::Help { .. } => self.active_help_list_mut(),
            _ => None,
        }
    }

    /// Get the active searchable list for the current mode (immutable)
    pub fn active_list(&self) -> Option<&SearchableList> {
        match self.mode {
            Mode::RepoSelect => Some(&self.repo_list),
            Mode::BranchSelect => Some(&self.branch_list),
            Mode::SelectBaseBranch => self.base_branch_selection.as_ref().map(|f| &f.list),
            Mode::Help { .. } => self.active_help_list(),
            _ => None,
        }
    }

    pub fn active_help_list_mut(&mut self) -> Option<&mut SearchableList> {
        self.help_overlay.as_mut().map(|overlay| &mut overlay.list)
    }

    pub fn active_help_list(&self) -> Option<&SearchableList> {
        self.help_overlay.as_ref().map(|overlay| &overlay.list)
    }

    pub fn is_branch_pending_delete(&self, repo_path: &Path, branch_name: &str) -> bool {
        self.pending_worktree_deletes
            .iter()
            .any(|pending| pending.repo_path == repo_path && pending.branch_name == branch_name)
    }

    pub fn set_active_list_page_rows(&mut self, rows: usize) {
        self.active_list_page_rows = rows.max(1);
    }

    pub fn active_list_page_rows(&self) -> usize {
        self.active_list_page_rows.max(1)
    }

    pub fn mark_pending_worktree_delete(&mut self, pending: PendingWorktreeDelete) {
        self.pending_worktree_deletes.retain(|entry| {
            !(entry.repo_path == pending.repo_path && entry.branch_name == pending.branch_name)
        });
        self.pending_worktree_deletes.push(pending);
    }

    pub fn clear_pending_worktree_delete_by_path(&mut self, worktree_path: &Path) -> bool {
        let before = self.pending_worktree_deletes.len();
        self.pending_worktree_deletes
            .retain(|pending| pending.worktree_path != worktree_path);
        before != self.pending_worktree_deletes.len()
    }

    pub fn clear_pending_worktree_delete_by_branch(
        &mut self,
        repo_path: &Path,
        branch_name: &str,
    ) -> bool {
        let before = self.pending_worktree_deletes.len();
        self.pending_worktree_deletes.retain(|pending| {
            !(pending.repo_path == repo_path && pending.branch_name == branch_name)
        });
        before != self.pending_worktree_deletes.len()
    }

    /// Drop stale pending delete entries that no longer correspond to an existing worktree.
    pub fn reconcile_pending_worktree_deletes(&mut self) -> bool {
        let active_worktree_paths: HashSet<&Path> = self
            .repos
            .iter()
            .flat_map(|repo| repo.worktrees.iter().map(|wt| wt.path.as_path()))
            .collect();

        let before = self.pending_worktree_deletes.len();
        self.pending_worktree_deletes.retain(|pending| {
            !pending.is_expired() && active_worktree_paths.contains(pending.worktree_path.as_path())
        });
        before != self.pending_worktree_deletes.len()
    }
}

/// Determine where to put a new worktree for a branch, avoiding collisions.
///
/// Worktrees are placed in `.kiosk_worktrees/` inside the repo's parent directory:
/// ```text
/// ~/Development/.kiosk_worktrees/kiosk--feat-awesome/
/// ~/Development/.kiosk_worktrees/scooter--fix-bug/
/// ```
pub fn worktree_dir(repo: &Repo, branch: &str) -> anyhow::Result<PathBuf> {
    let parent = repo.path.parent().unwrap_or(&repo.path);
    let worktree_root = parent.join(WORKTREE_DIR_NAME);
    let safe_branch = branch.replace('/', "-");
    let base = format!("{}{WORKTREE_NAME_SEPARATOR}{safe_branch}", repo.name);
    let candidate = worktree_root.join(&base);
    if !candidate.exists() {
        return Ok(candidate);
    }
    for i in 2..WORKTREE_DIR_DEDUP_MAX_ATTEMPTS {
        let candidate = worktree_root.join(format!("{base}-{i}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "Could not find an available worktree directory name after {WORKTREE_DIR_DEDUP_MAX_ATTEMPTS} attempts"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Repo, Worktree};
    use crate::pending_delete::PendingWorktreeDelete;
    use std::fs;
    use tempfile::tempdir;

    fn make_repo(dir: &std::path::Path, name: &str) -> Repo {
        Repo {
            name: name.to_string(),
            session_name: name.to_string(),
            path: dir.join(name),
            worktrees: vec![],
        }
    }

    #[test]
    fn test_cursor_grapheme_combining_mark() {
        let mut list = SearchableList::new(0);
        list.search = "e\u{0301}".to_string();
        list.cursor_end();

        list.cursor_left();
        assert_eq!(list.cursor, 0);

        list.cursor_right();
        assert_eq!(list.cursor, list.search.len());

        list.cursor_end();
        assert!(list.backspace());
        assert_eq!(list.search, "");
        assert_eq!(list.cursor, 0);
    }

    #[test]
    fn test_cursor_grapheme_zwj_sequence() {
        let emoji = "👩‍💻";
        let mut list = SearchableList::new(0);
        list.search = format!("{emoji}a");

        list.cursor_start();
        list.cursor_right();
        assert_eq!(list.cursor, emoji.len());

        list.cursor_right();
        assert_eq!(list.cursor, list.search.len());
    }

    #[test]
    fn test_cursor_clamps_inside_grapheme() {
        let mut list = SearchableList::new(0);
        list.search = "café".to_string();
        list.cursor = 4;

        list.cursor_left();
        assert_eq!(list.cursor, 2);

        list.cursor = 4;
        list.cursor_right();
        assert_eq!(list.cursor, 5);
    }

    #[test]
    fn test_delete_forward_grapheme() {
        let emoji = "👩‍💻";
        let mut list = SearchableList::new(0);
        list.search = format!("{emoji}a");
        list.cursor = 0;

        assert!(list.delete_forward_char());
        assert_eq!(list.search, "a");
        assert_eq!(list.cursor, 0);
    }

    #[test]
    fn test_word_boundaries_unicode_whitespace() {
        let text = "alpha\u{00A0}\u{00A0}beta";
        let mut list = SearchableList::new(0);
        list.search = text.to_string();
        let beta_idx = text.find('b').unwrap();
        let alpha_end = text.find('\u{00A0}').unwrap();

        list.cursor_end();
        list.cursor_word_left();
        assert_eq!(list.cursor, beta_idx);

        list.cursor_start();
        list.cursor_word_right();
        assert_eq!(list.cursor, alpha_end);
    }

    #[test]
    fn test_delete_word_respects_whitespace() {
        let text = "alpha  beta";
        let mut list = SearchableList::new(0);
        list.search = text.to_string();
        list.cursor_end();

        list.delete_word();
        assert_eq!(list.search, "alpha  ");
        assert_eq!(list.cursor, "alpha  ".len());
    }

    #[test]
    fn test_delete_word_forward_respects_whitespace() {
        let text = "alpha  beta";
        let mut list = SearchableList::new(0);
        list.search = text.to_string();
        list.cursor_start();

        list.delete_word_forward();
        assert_eq!(list.search, "  beta");
        assert_eq!(list.cursor, 0);
    }

    #[test]
    fn test_cursor_word_from_whitespace() {
        let text = "alpha   beta";
        let mut list = SearchableList::new(0);
        list.search = text.to_string();
        list.cursor = 6;

        list.cursor_word_left();
        assert_eq!(list.cursor, 0);

        list.cursor = 5;
        list.cursor_word_right();
        assert_eq!(list.cursor, text.len());
    }

    #[test]
    fn test_delete_word_forward_from_whitespace() {
        let text = "alpha   beta";
        let mut list = SearchableList::new(0);
        list.search = text.to_string();
        list.cursor = 5;

        list.delete_word_forward();
        assert_eq!(list.search, "alpha");
        assert_eq!(list.cursor, 5);
    }

    #[test]
    fn test_delete_to_start_clamps_cursor() {
        let mut list = SearchableList::new(0);
        list.search = "café".to_string();
        list.cursor = 4;

        list.delete_to_start();
        assert_eq!(list.search, "é");
        assert_eq!(list.cursor, 0);
    }

    #[test]
    fn test_delete_to_end_clamps_cursor() {
        let mut list = SearchableList::new(0);
        list.search = "café".to_string();
        list.cursor = 4;

        list.delete_to_end();
        assert_eq!(list.search, "caf");
        assert_eq!(list.cursor, 3);
    }

    #[cfg(unix)]
    #[test]
    fn test_sort_repos_prefers_current_with_symlinked_paths() {
        use std::os::unix::fs::symlink;

        let tmp = tempdir().unwrap();
        let repo_dir = tmp.path().join("repo");
        let other_dir = tmp.path().join("other");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::create_dir_all(&other_dir).unwrap();

        let link_dir = tmp.path().join("repo-link");
        symlink(&repo_dir, &link_dir).unwrap();

        let mut repos = vec![
            Repo {
                name: "repo-link".to_string(),
                session_name: "repo-link".to_string(),
                path: link_dir.clone(),
                worktrees: vec![],
            },
            Repo {
                name: "other".to_string(),
                session_name: "other".to_string(),
                path: other_dir.clone(),
                worktrees: vec![],
            },
        ];

        sort_repos(&mut repos, Some(&repo_dir), &HashMap::new());
        assert_eq!(repos[0].path, link_dir);
    }

    #[test]
    fn test_worktree_dir_basic() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "myrepo");
        let result = worktree_dir(&repo, "main").unwrap();
        assert_eq!(
            result,
            tmp.path()
                .join(WORKTREE_DIR_NAME)
                .join(format!("myrepo{WORKTREE_NAME_SEPARATOR}main"))
        );
    }

    #[test]
    fn test_worktree_dir_slash_in_branch() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let result = worktree_dir(&repo, "feat/awesome").unwrap();
        assert_eq!(
            result,
            tmp.path()
                .join(WORKTREE_DIR_NAME)
                .join(format!("repo{WORKTREE_NAME_SEPARATOR}feat-awesome"))
        );
    }

    #[test]
    fn test_worktree_dir_dedup() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let first = tmp
            .path()
            .join(WORKTREE_DIR_NAME)
            .join(format!("repo{WORKTREE_NAME_SEPARATOR}main"));
        fs::create_dir_all(&first).unwrap();
        let result = worktree_dir(&repo, "main").unwrap();
        assert_eq!(
            result,
            tmp.path()
                .join(WORKTREE_DIR_NAME)
                .join(format!("repo{WORKTREE_NAME_SEPARATOR}main-2"))
        );
    }

    #[test]
    fn test_worktree_dir_bounded_error() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let wt_root = tmp.path().join(WORKTREE_DIR_NAME);
        // Create the base and 2..999 suffixed dirs to exhaust the loop
        let base = format!("repo{WORKTREE_NAME_SEPARATOR}main");
        fs::create_dir_all(wt_root.join(&base)).unwrap();
        for i in 2..WORKTREE_DIR_DEDUP_MAX_ATTEMPTS {
            fs::create_dir_all(wt_root.join(format!("{base}-{i}"))).unwrap();
        }
        let result = worktree_dir(&repo, "main");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains(&format!("{WORKTREE_DIR_DEDUP_MAX_ATTEMPTS} attempts"))
        );
    }

    #[test]
    fn test_worktree_dir_in_kiosk_worktrees_subdir() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "myrepo");
        let result = worktree_dir(&repo, "dev").unwrap();
        assert!(result.to_string_lossy().contains(WORKTREE_DIR_NAME));
    }

    #[test]
    fn test_build_sorted_basic() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/tmp/myrepo"),
                    branch: Some("main".to_string()),
                    is_main: true,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo-dev"),
                    branch: Some("dev".to_string()),
                    is_main: false,
                },
            ],
        };

        let branches = vec!["main".into(), "dev".into(), "feature".into()];
        let sessions = vec!["myrepo-dev".to_string()];

        let entries = BranchEntry::build_sorted(&repo, &branches, &sessions);

        // main is current → first
        assert_eq!(entries[0].name, "main");
        assert!(entries[0].is_current);
        assert!(entries[0].worktree_path.is_some());

        // dev has session → second
        assert_eq!(entries[1].name, "dev");
        assert!(entries[1].has_session);
        assert!(entries[1].worktree_path.is_some());

        // feature has nothing → last
        assert_eq!(entries[2].name, "feature");
        assert!(!entries[2].has_session);
        assert!(entries[2].worktree_path.is_none());
    }

    #[test]
    fn test_build_remote_deduplication() {
        let remote = vec!["main".into(), "dev".into(), "remote-only".into()];
        let local = vec!["main".into(), "dev".into()];

        let entries = BranchEntry::build_remote(&remote, &local);

        // Only "remote-only" should appear (main and dev are local)
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "remote-only");
        assert!(entries[0].is_remote);
    }

    #[test]
    fn test_build_remote_empty_when_all_local() {
        let remote = vec!["main".into(), "dev".into()];
        let local = vec!["main".into(), "dev".into()];

        let entries = BranchEntry::build_remote(&remote, &local);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_sort_remote_after_local() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/myrepo"),
                branch: Some("main".to_string()),
                is_main: true,
            }],
        };

        let local_names = vec!["main".into(), "dev".into()];
        let mut entries = BranchEntry::build_sorted(&repo, &local_names, &[]);

        // Add remote branches
        let remote_names = vec!["feature-a".into(), "feature-b".into()];
        let remote = BranchEntry::build_remote(&remote_names, &local_names);
        entries.extend(remote);
        BranchEntry::sort_entries(&mut entries);

        // Local branches should come before remote
        assert!(!entries[0].is_remote); // main (current)
        assert!(!entries[1].is_remote); // dev
        assert!(entries[2].is_remote); // feature-a
        assert!(entries[3].is_remote); // feature-b
    }

    #[test]
    fn test_pending_delete_mark_and_clear() {
        let mut state = AppState::new(vec![make_repo(std::path::Path::new("/tmp"), "repo")], None);
        let repo_path = PathBuf::from("/tmp/repo");
        let worktree_path = PathBuf::from("/tmp/repo-dev");
        let pending =
            PendingWorktreeDelete::new(repo_path.clone(), "dev".to_string(), worktree_path.clone());
        state.mark_pending_worktree_delete(pending);
        assert!(state.is_branch_pending_delete(&repo_path, "dev"));

        assert!(state.clear_pending_worktree_delete_by_path(&worktree_path));
        assert!(!state.is_branch_pending_delete(&repo_path, "dev"));
    }

    #[test]
    fn test_scroll_anchor_behavior_down_then_up() {
        let mut list = SearchableList::new(100);
        let viewport_rows = 20;

        // Move down into the middle: selection should be anchored one row above bottom.
        for _ in 0..25 {
            list.move_selection(1);
            list.update_scroll_offset_for_selection(viewport_rows);
        }
        let selected = list.selected.unwrap_or(0);
        assert_eq!(selected - list.scroll_offset, 18);

        // Move to bottom: selection may reach the actual bottom row.
        for _ in 0..200 {
            list.move_selection(1);
            list.update_scroll_offset_for_selection(viewport_rows);
        }
        let selected = list.selected.unwrap_or(0);
        assert_eq!(selected, 99);
        assert_eq!(selected - list.scroll_offset, 19);

        // Move up: keep viewport stationary first, then anchor one below top.
        list.move_selection(-1);
        list.update_scroll_offset_for_selection(viewport_rows);
        let selected = list.selected.unwrap_or(0);
        assert_eq!(selected, 98);
        assert_eq!(selected - list.scroll_offset, 18);

        for _ in 0..17 {
            list.move_selection(-1);
            list.update_scroll_offset_for_selection(viewport_rows);
        }
        let selected = list.selected.unwrap_or(0);
        assert_eq!(selected, 81);
        assert_eq!(selected - list.scroll_offset, 1);

        list.move_selection(-1);
        list.update_scroll_offset_for_selection(viewport_rows);
        let selected = list.selected.unwrap_or(0);
        assert_eq!(selected, 80);
        assert_eq!(selected - list.scroll_offset, 1);
    }

    #[test]
    fn test_scroll_down_starts_before_last_viewport_row() {
        let mut list = SearchableList::new(100);
        let viewport_rows = 20;

        for _ in 0..18 {
            list.move_selection(1);
            list.update_scroll_offset_for_selection(viewport_rows);
        }
        assert_eq!(list.selected, Some(18));
        assert_eq!(list.scroll_offset, 0);

        list.move_selection(1);
        list.update_scroll_offset_for_selection(viewport_rows);
        assert_eq!(list.selected, Some(19));
        assert_eq!(list.scroll_offset, 1);
    }

    #[test]
    fn test_scroll_up_from_bottom_keeps_offset_until_top_anchor_hit() {
        let mut list = SearchableList::new(100);
        let viewport_rows = 20;

        for _ in 0..200 {
            list.move_selection(1);
            list.update_scroll_offset_for_selection(viewport_rows);
        }
        let offset_at_bottom = list.scroll_offset;
        assert_eq!(list.selected, Some(99));

        for expected_selected in (81..=98).rev() {
            list.move_selection(-1);
            list.update_scroll_offset_for_selection(viewport_rows);
            assert_eq!(list.selected, Some(expected_selected));
            assert_eq!(list.scroll_offset, offset_at_bottom);
        }
    }

    #[test]
    fn test_scroll_reversing_direction_near_bottom_does_not_move_offset() {
        let mut list = SearchableList::new(100);
        let viewport_rows = 20;
        for _ in 0..200 {
            list.move_selection(1);
            list.update_scroll_offset_for_selection(viewport_rows);
        }

        let offset_before = list.scroll_offset;
        list.move_selection(-1);
        list.update_scroll_offset_for_selection(viewport_rows);
        let offset_after_up = list.scroll_offset;
        list.move_selection(1);
        list.update_scroll_offset_for_selection(viewport_rows);
        let offset_after_down = list.scroll_offset;

        assert_eq!(offset_before, offset_after_up);
        assert_eq!(offset_after_up, offset_after_down);
    }

    #[test]
    fn test_first_up_from_bottom_does_not_change_offset_across_viewports() {
        for viewport_rows in 3..=40 {
            let mut list = SearchableList::new(35);
            for _ in 0..200 {
                list.move_selection(1);
                list.update_scroll_offset_for_selection(viewport_rows);
            }
            let offset_before = list.scroll_offset;
            list.move_selection(-1);
            list.update_scroll_offset_for_selection(viewport_rows);
            assert_eq!(
                list.scroll_offset, offset_before,
                "Offset changed for viewport_rows={viewport_rows}"
            );
        }
    }

    #[test]
    fn test_prev_word_boundary_edges() {
        let mut list = SearchableList::new(0);
        list.search = "alpha   beta".to_string();

        assert_eq!(list.prev_word_boundary(0), 0);
        assert_eq!(list.prev_word_boundary(list.search.len()), 8);
        assert_eq!(list.prev_word_boundary(7), 0);
        assert_eq!(list.prev_word_boundary(usize::MAX), 8);
    }

    #[test]
    fn test_next_word_boundary_edges() {
        let mut list = SearchableList::new(0);
        list.search = "alpha   beta".to_string();

        assert_eq!(list.next_word_boundary(0), 5);
        assert_eq!(list.next_word_boundary(5), 12);
        assert_eq!(
            list.next_word_boundary(list.search.len()),
            list.search.len()
        );
        assert_eq!(list.next_word_boundary(usize::MAX), list.search.len());
    }

    #[test]
    fn test_word_boundary_empty_and_spaces_only() {
        let empty = SearchableList::new(0);
        assert_eq!(empty.prev_word_boundary(3), 0);
        assert_eq!(empty.next_word_boundary(3), 0);

        let mut spaces = SearchableList::new(0);
        spaces.search = "   ".to_string();
        assert_eq!(spaces.prev_word_boundary(3), 0);
        assert_eq!(spaces.next_word_boundary(0), 3);
    }

    #[test]
    fn test_branch_sort_order_with_activity() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/tmp/myrepo"),
                    branch: Some("main".to_string()),
                    is_main: true,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--dev"),
                    branch: Some("dev".to_string()),
                    is_main: false,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--hotfix"),
                    branch: Some("hotfix".to_string()),
                    is_main: false,
                },
            ],
        };

        let branches = vec![
            "main".into(),
            "dev".into(),
            "hotfix".into(),
            "feature".into(),
        ];
        let sessions = vec!["myrepo--dev".to_string(), "myrepo--hotfix".to_string()];
        let mut activity = HashMap::new();
        activity.insert("myrepo--dev".to_string(), 100);
        activity.insert("myrepo--hotfix".to_string(), 200);

        let entries = BranchEntry::build_sorted_with_activity(
            &repo,
            &branches,
            &sessions,
            Some("main"),
            &activity,
            None,
        );

        // Order: current (main), default (main, but already current), sessions by recency, worktrees, rest
        assert_eq!(entries[0].name, "main"); // current + default
        assert!(entries[0].is_current);
        assert!(entries[0].is_default);
        assert_eq!(entries[1].name, "hotfix"); // session ts=200
        assert_eq!(entries[2].name, "dev"); // session ts=100
        assert_eq!(entries[3].name, "feature"); // no session, no worktree
    }

    #[test]
    fn test_branch_sort_default_after_current() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/myrepo"),
                branch: Some("dev".to_string()),
                is_main: true,
            }],
        };

        let branches = vec!["main".into(), "dev".into(), "feature".into()];
        let entries = BranchEntry::build_sorted_with_activity(
            &repo,
            &branches,
            &[],
            Some("main"),
            &HashMap::new(),
            None,
        );

        assert_eq!(entries[0].name, "dev"); // current (main worktree has dev checked out)
        assert_eq!(entries[1].name, "main"); // default
        assert_eq!(entries[2].name, "feature");
    }

    #[test]
    fn test_sort_repos_ordering() {
        let mut repos = vec![
            Repo {
                name: "zebra".to_string(),
                session_name: "zebra".to_string(),
                path: PathBuf::from("/tmp/zebra"),
                worktrees: vec![Worktree {
                    path: PathBuf::from("/tmp/zebra"),
                    branch: Some("main".to_string()),
                    is_main: true,
                }],
            },
            Repo {
                name: "alpha".to_string(),
                session_name: "alpha".to_string(),
                path: PathBuf::from("/tmp/alpha"),
                worktrees: vec![Worktree {
                    path: PathBuf::from("/tmp/alpha"),
                    branch: Some("main".to_string()),
                    is_main: true,
                }],
            },
            Repo {
                name: "current".to_string(),
                session_name: "current".to_string(),
                path: PathBuf::from("/tmp/current"),
                worktrees: vec![],
            },
        ];

        let mut activity = HashMap::new();
        activity.insert("zebra".to_string(), 500);

        sort_repos(&mut repos, Some(Path::new("/tmp/current")), &activity);

        assert_eq!(repos[0].name, "current"); // current repo
        assert_eq!(repos[1].name, "zebra"); // has session
        assert_eq!(repos[2].name, "alpha"); // alphabetical
    }

    #[test]
    fn test_reconcile_pending_deletes_removes_missing_worktree() {
        let repo = Repo {
            name: "repo".to_string(),
            session_name: "repo".to_string(),
            path: PathBuf::from("/tmp/repo"),
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/repo"),
                branch: Some("main".to_string()),
                is_main: true,
            }],
        };
        let mut state = AppState::new(vec![repo], None);
        state.mark_pending_worktree_delete(PendingWorktreeDelete::new(
            PathBuf::from("/tmp/repo"),
            "dev".to_string(),
            PathBuf::from("/tmp/repo-dev"),
        ));

        assert!(state.reconcile_pending_worktree_deletes());
        assert!(state.pending_worktree_deletes.is_empty());
    }

    #[test]
    fn test_sort_repos_no_current_repo() {
        let mut repos = vec![
            Repo {
                name: "zebra".to_string(),
                session_name: "zebra".to_string(),
                path: PathBuf::from("/tmp/zebra"),
                worktrees: vec![Worktree {
                    path: PathBuf::from("/tmp/zebra"),
                    branch: Some("main".to_string()),
                    is_main: true,
                }],
            },
            Repo {
                name: "alpha".to_string(),
                session_name: "alpha".to_string(),
                path: PathBuf::from("/tmp/alpha"),
                worktrees: vec![],
            },
            Repo {
                name: "mango".to_string(),
                session_name: "mango".to_string(),
                path: PathBuf::from("/tmp/mango"),
                worktrees: vec![Worktree {
                    path: PathBuf::from("/tmp/mango"),
                    branch: Some("main".to_string()),
                    is_main: true,
                }],
            },
        ];

        let mut activity = HashMap::new();
        activity.insert("mango".to_string(), 300);
        activity.insert("zebra".to_string(), 100);

        sort_repos(&mut repos, None, &activity);

        // Sessions by recency first, then alphabetical
        assert_eq!(repos[0].name, "mango"); // session ts=300
        assert_eq!(repos[1].name, "zebra"); // session ts=100
        assert_eq!(repos[2].name, "alpha"); // no session, alphabetical
    }

    #[test]
    fn test_sort_repos_multiple_worktree_sessions() {
        let mut repos = vec![
            Repo {
                name: "repo-a".to_string(),
                session_name: "repo-a".to_string(),
                path: PathBuf::from("/tmp/repo-a"),
                worktrees: vec![
                    Worktree {
                        path: PathBuf::from("/tmp/repo-a"),
                        branch: Some("main".to_string()),
                        is_main: true,
                    },
                    Worktree {
                        path: PathBuf::from("/tmp/repo-a--feat"),
                        branch: Some("feat".to_string()),
                        is_main: false,
                    },
                ],
            },
            Repo {
                name: "repo-b".to_string(),
                session_name: "repo-b".to_string(),
                path: PathBuf::from("/tmp/repo-b"),
                worktrees: vec![Worktree {
                    path: PathBuf::from("/tmp/repo-b"),
                    branch: Some("main".to_string()),
                    is_main: true,
                }],
            },
        ];

        let mut activity = HashMap::new();
        // repo-a has two worktree sessions: main at 50, feat at 500
        activity.insert("repo-a".to_string(), 50);
        activity.insert("repo-a--feat".to_string(), 500);
        // repo-b has one session at 200
        activity.insert("repo-b".to_string(), 200);

        sort_repos(&mut repos, None, &activity);

        // repo-a max activity is 500 > repo-b's 200
        assert_eq!(repos[0].name, "repo-a");
        assert_eq!(repos[1].name, "repo-b");
    }

    #[test]
    fn test_sort_repos_empty() {
        let mut repos: Vec<Repo> = vec![];
        sort_repos(&mut repos, None, &HashMap::new());
        assert!(repos.is_empty());
    }

    #[test]
    fn test_branch_sort_current_is_also_default() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/myrepo"),
                branch: Some("main".to_string()),
                is_main: true,
            }],
        };

        let branches = vec!["main".into(), "dev".into(), "feature".into()];
        let entries = BranchEntry::build_sorted_with_activity(
            &repo,
            &branches,
            &[],
            Some("main"),
            &HashMap::new(),
            None,
        );

        // main is both current and default — should appear exactly once at position 0
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "main");
        assert!(entries[0].is_current);
        assert!(entries[0].is_default);
        // No duplicate
        assert_eq!(
            entries.iter().filter(|e| e.name == "main").count(),
            1,
            "main should appear exactly once"
        );
    }

    #[test]
    fn test_branch_sort_session_without_activity_ts() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/tmp/myrepo"),
                    branch: Some("main".to_string()),
                    is_main: true,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--dev"),
                    branch: Some("dev".to_string()),
                    is_main: false,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--hotfix"),
                    branch: Some("hotfix".to_string()),
                    is_main: false,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--no-ts"),
                    branch: Some("no-ts".to_string()),
                    is_main: false,
                },
            ],
        };

        let branches = vec![
            "main".into(),
            "dev".into(),
            "hotfix".into(),
            "no-ts".into(),
            "plain".into(),
        ];
        // no-ts has a session but no activity timestamp
        let sessions = vec![
            "myrepo--dev".to_string(),
            "myrepo--hotfix".to_string(),
            "myrepo--no-ts".to_string(),
        ];
        let mut activity = HashMap::new();
        activity.insert("myrepo--dev".to_string(), 100);
        activity.insert("myrepo--hotfix".to_string(), 200);
        // no-ts intentionally missing from activity map

        let entries = BranchEntry::build_sorted_with_activity(
            &repo,
            &branches,
            &sessions,
            Some("main"),
            &activity,
            None,
        );

        assert_eq!(entries[0].name, "main"); // current + default
        assert_eq!(entries[1].name, "hotfix"); // session ts=200
        assert_eq!(entries[2].name, "dev"); // session ts=100
        // no-ts has session but no timestamp — has_session=true but session_activity_ts=None
        // It has a worktree, so it sorts among worktree branches
        // The sort_entries sorts by session_activity_ts first (Some before None),
        // then worktree presence. no-ts has no activity_ts so it falls to worktree tier.
        let no_ts_pos = entries.iter().position(|e| e.name == "no-ts").unwrap();
        let plain_pos = entries.iter().position(|e| e.name == "plain").unwrap();
        assert!(
            no_ts_pos < plain_pos,
            "no-ts (has worktree) should sort before plain (no worktree)"
        );
    }

    #[test]
    fn test_branch_sort_no_default_no_current() {
        // No default branch; CWD is None so fallback picks first worktree as current
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--alpha"),
                    branch: Some("alpha".to_string()),
                    is_main: false,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--beta"),
                    branch: Some("beta".to_string()),
                    is_main: false,
                },
            ],
        };

        let branches = vec![
            "alpha".into(),
            "beta".into(),
            "gamma".into(),
            "delta".into(),
        ];
        let sessions = vec!["myrepo--alpha".to_string()];
        let mut activity = HashMap::new();
        activity.insert("myrepo--alpha".to_string(), 999);

        let entries = BranchEntry::build_sorted_with_activity(
            &repo, &branches, &sessions, None, // no default
            &activity, None,
        );

        // alpha has session with ts → first
        assert_eq!(entries[0].name, "alpha");
        // beta has worktree but no session → next
        assert_eq!(entries[1].name, "beta");
        // gamma and delta are plain, alphabetical
        assert_eq!(entries[2].name, "delta");
        assert_eq!(entries[3].name, "gamma");
    }

    #[test]
    fn test_branch_sort_worktrees_before_plain() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/tmp/myrepo"),
                    branch: Some("main".to_string()),
                    is_main: true,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--wt-branch"),
                    branch: Some("wt-branch".to_string()),
                    is_main: false,
                },
            ],
        };

        let branches = vec![
            "main".into(),
            "aaa-plain".into(),
            "wt-branch".into(),
            "zzz-plain".into(),
        ];

        let entries = BranchEntry::build_sorted_with_activity(
            &repo,
            &branches,
            &[],
            None,
            &HashMap::new(),
            None,
        );

        assert_eq!(entries[0].name, "main"); // current
        assert_eq!(entries[1].name, "wt-branch"); // has worktree
        // plain branches alphabetical
        assert_eq!(entries[2].name, "aaa-plain");
        assert_eq!(entries[3].name, "zzz-plain");
    }

    #[test]
    fn test_branch_sort_remote_always_last() {
        let mut entries = vec![
            BranchEntry {
                name: "aaa-remote".to_string(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_default: false,
                is_remote: true,
                session_activity_ts: None,
            },
            BranchEntry {
                name: "zzz-local".to_string(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_default: false,
                is_remote: false,
                session_activity_ts: None,
            },
            BranchEntry {
                name: "mmm-local".to_string(),
                worktree_path: None,
                has_session: false,
                is_current: false,
                is_default: false,
                is_remote: false,
                session_activity_ts: None,
            },
        ];

        BranchEntry::sort_entries(&mut entries);

        // Local branches first (alphabetical), then remote
        assert_eq!(entries[0].name, "mmm-local");
        assert!(!entries[0].is_remote);
        assert_eq!(entries[1].name, "zzz-local");
        assert!(!entries[1].is_remote);
        assert_eq!(entries[2].name, "aaa-remote");
        assert!(entries[2].is_remote);
    }

    #[test]
    fn test_cwd_worktree_determines_current_branch() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/tmp/myrepo"),
                    branch: Some("main".to_string()),
                    is_main: true,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--feature"),
                    branch: Some("feature".to_string()),
                    is_main: false,
                },
            ],
        };

        let branches = vec!["main".into(), "feature".into(), "dev".into()];

        // CWD is in the feature worktree — feature should be current, not main
        let entries = BranchEntry::build_sorted_with_activity(
            &repo,
            &branches,
            &[],
            Some("main"),
            &HashMap::new(),
            Some(Path::new("/tmp/myrepo--feature")),
        );

        assert_eq!(entries[0].name, "feature"); // current (CWD worktree)
        assert!(entries[0].is_current);
        assert_eq!(entries[1].name, "main"); // default
        assert!(entries[1].is_default);
        assert!(!entries[1].is_current);
        assert_eq!(entries[2].name, "dev");
    }

    #[test]
    fn test_cwd_main_repo_marks_main_worktree_current() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/tmp/myrepo"),
                    branch: Some("main".to_string()),
                    is_main: true,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--feature"),
                    branch: Some("feature".to_string()),
                    is_main: false,
                },
            ],
        };

        let branches = vec!["main".into(), "feature".into()];

        // CWD is the main repo dir — main should be current
        let entries = BranchEntry::build_sorted_with_activity(
            &repo,
            &branches,
            &[],
            Some("main"),
            &HashMap::new(),
            Some(Path::new("/tmp/myrepo")),
        );

        assert_eq!(entries[0].name, "main"); // current + default
        assert!(entries[0].is_current);
        assert_eq!(entries[1].name, "feature");
        assert!(!entries[1].is_current);
    }

    #[test]
    fn test_cwd_unrelated_falls_back_to_main_worktree() {
        let repo = Repo {
            name: "myrepo".to_string(),
            session_name: "myrepo".to_string(),
            path: PathBuf::from("/tmp/myrepo"),
            worktrees: vec![
                Worktree {
                    path: PathBuf::from("/tmp/myrepo"),
                    branch: Some("main".to_string()),
                    is_main: true,
                },
                Worktree {
                    path: PathBuf::from("/tmp/myrepo--feature"),
                    branch: Some("feature".to_string()),
                    is_main: false,
                },
            ],
        };

        let branches = vec!["main".into(), "feature".into()];

        // CWD doesn't match any worktree — falls back to first (main)
        let entries = BranchEntry::build_sorted_with_activity(
            &repo,
            &branches,
            &[],
            Some("main"),
            &HashMap::new(),
            Some(Path::new("/tmp/unrelated-dir")),
        );

        assert_eq!(entries[0].name, "main"); // current (fallback to first worktree)
        assert!(entries[0].is_current);
    }

    #[test]
    fn test_build_remote_has_correct_defaults() {
        let remote = vec!["feat-x".into(), "feat-y".into()];
        let local: Vec<String> = vec![];

        let entries = BranchEntry::build_remote(&remote, &local);

        assert_eq!(entries.len(), 2);
        for entry in &entries {
            assert!(!entry.is_default, "remote entries should not be default");
            assert!(
                entry.session_activity_ts.is_none(),
                "remote entries should have no activity ts"
            );
            assert!(entry.is_remote, "remote entries should be marked remote");
            assert!(!entry.has_session);
            assert!(!entry.is_current);
            assert!(entry.worktree_path.is_none());
        }
    }

    #[test]
    fn test_active_list_points_to_help_overlay_in_help_mode() {
        let mut state = AppState::new(vec![make_repo(std::path::Path::new("/tmp"), "repo")], None);
        state.help_overlay = Some(HelpOverlayState {
            list: SearchableList::new(3),
            rows: Vec::new(),
        });
        state.mode = Mode::Help {
            previous: Box::new(Mode::RepoSelect),
        };

        assert!(state.active_list().is_some());
        assert_eq!(state.active_list().and_then(|list| list.selected), Some(0));

        if let Some(list) = state.active_list_mut() {
            list.move_selection(1);
        }
        assert_eq!(
            state
                .help_overlay
                .as_ref()
                .and_then(|overlay| overlay.list.selected),
            Some(1)
        );
    }

    #[test]
    fn test_branch_entry_serde_round_trip() {
        let entry = BranchEntry {
            name: "feat/test".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/repo-feat-test")),
            has_session: true,
            is_current: false,
            is_default: false,
            is_remote: false,
            session_activity_ts: Some(12345),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let decoded: BranchEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, entry);
    }
}
