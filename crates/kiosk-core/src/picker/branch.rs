use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{FilterItem, FilterRequest, FilterWorker, TickChanges};
use crate::{
    backend::{Backend, KnownCheckout},
    event::{AppEvent, BranchOperationFailure, FilterKey, FilterTarget},
    git::{GitProvider, Repo},
    spawn::{
        EventSender, spawn_branch_loading, spawn_open_branch, spawn_open_checkouts,
        spawn_open_remote_branch,
    },
    state::{AppState, BranchEntry, Mode, SearchableList, ToastKind},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchContext {
    pub repo_path: PathBuf,
    pub repo_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BranchId {
    Local(String),
    Remote { remote: String, name: String },
}

impl BranchId {
    pub fn display_name(&self) -> String {
        match self {
            Self::Local(name) => name.clone(),
            Self::Remote { remote, name } => format!("{remote}/{name}"),
        }
    }
}

impl From<&str> for BranchId {
    fn from(name: &str) -> Self {
        Self::Local(name.to_string())
    }
}

/// How far the branch listing for the current generation has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchLoad {
    /// Waiting for the listing. The entries are empty on entering a repository, or the
    /// previous listing while it is refreshed. Once it arrives, `reselect` is selected if
    /// it is listed.
    Loading { reselect: Option<BranchId> },
    /// Listed. The remotes are fetched afterwards, and their branches merged in as each
    /// one finishes.
    Ready { fetching_remotes: bool },
}

/// Which of the repository's checkouts the backend is showing, as of the current
/// generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenCheckouts {
    Loading,
    Loaded(Vec<PathBuf>),
    Failed,
}

/// The branch list of one repository. Each listing is a generation: starting one with
/// [`Self::begin_load`] resets what the previous one knew, and background results for
/// any other generation are ignored.
#[derive(Debug)]
pub struct BranchViewState {
    pub entries: Vec<BranchEntry>,
    pub list: SearchableList,
    pub filter_generation: u64,
    generation: u64,
    load: BranchLoad,
    open_checkouts: OpenCheckouts,
    remote_branches: BTreeMap<String, Vec<BranchEntry>>,
    fetch_warning_remotes: HashSet<String>,
    unsupported_ref_warning_generation: Option<u64>,
}

impl Default for BranchViewState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            list: SearchableList::new(0),
            filter_generation: 0,
            generation: 0,
            load: BranchLoad::Ready {
                fetching_remotes: false,
            },
            open_checkouts: OpenCheckouts::Loading,
            remote_branches: BTreeMap::new(),
            fetch_warning_remotes: HashSet::new(),
            unsupported_ref_warning_generation: None,
        }
    }
}

impl BranchViewState {
    pub fn selected(&self) -> Option<&BranchEntry> {
        let selected = self.list.selected?;
        let index = self.list.filtered.get(selected)?.0;
        self.entries.get(index)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn load(&self) -> &BranchLoad {
        &self.load
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.load, BranchLoad::Loading { .. })
    }

    pub fn is_fetching_remotes(&self) -> bool {
        matches!(
            self.load,
            BranchLoad::Ready {
                fetching_remotes: true
            }
        )
    }

    pub fn open_checkouts(&self) -> &OpenCheckouts {
        &self.open_checkouts
    }

    /// Starts a new generation, whose listing is loaded next. Until it arrives the entries
    /// stay as they are, and `reselect` is selected once it does.
    pub(crate) fn begin_load(&mut self, reselect: Option<BranchId>) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("branch view generation overflow");
        self.load = BranchLoad::Loading { reselect };
        self.open_checkouts = OpenCheckouts::Loading;
        self.remote_branches.clear();
        self.fetch_warning_remotes.clear();
    }

    /// Takes the generation's listing, returning the branch to select. Its remotes are
    /// fetched next.
    pub(crate) fn finish_load(&mut self, branches: Vec<BranchEntry>) -> Option<BranchId> {
        self.entries = branches;
        self.remote_branches.clear();
        self.apply_open_indicators();
        match std::mem::replace(
            &mut self.load,
            BranchLoad::Ready {
                fetching_remotes: true,
            },
        ) {
            BranchLoad::Loading { reselect } => reselect,
            BranchLoad::Ready { .. } => None,
        }
    }

    /// Marks the generation's remotes as fetched.
    pub(crate) fn finish_remote_fetches(&mut self) {
        if let BranchLoad::Ready { fetching_remotes } = &mut self.load {
            *fetching_remotes = false;
        }
    }

    pub(crate) fn set_open_checkouts(&mut self, open_checkouts: OpenCheckouts) {
        self.open_checkouts = open_checkouts;
        self.apply_open_indicators();
    }

    pub fn merge_remote_snapshot(&mut self, remote: String, incoming: Vec<BranchEntry>) {
        let bucket = self.remote_branches.entry(remote).or_default();
        let mut known: HashSet<BranchId> = bucket.iter().map(BranchEntry::id).collect();
        bucket.extend(
            incoming
                .into_iter()
                .filter(|entry| known.insert(entry.id())),
        );
        BranchEntry::sort(bucket);

        let mut merged: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.remote.is_none())
            .cloned()
            .collect();
        let local_names: HashSet<String> = merged.iter().map(|entry| entry.name.clone()).collect();
        for entries in self.remote_branches.values() {
            merged.extend(
                entries
                    .iter()
                    .filter(|entry| !local_names.contains(&entry.name))
                    .cloned(),
            );
        }
        BranchEntry::sort(&mut merged);
        self.entries = merged;
    }

    pub fn promote_remote_to_local(&mut self, branch_name: &str) {
        for entries in self.remote_branches.values_mut() {
            entries.retain(|entry| entry.name != branch_name);
        }
        self.entries
            .retain(|entry| entry.remote.is_none() || entry.name != branch_name);
        if !self
            .entries
            .iter()
            .any(|entry| entry.remote.is_none() && entry.name == branch_name)
        {
            self.entries.push(BranchEntry::local(branch_name));
        }
        BranchEntry::sort(&mut self.entries);
    }

    pub fn apply_open_indicators(&mut self) {
        let open: &[PathBuf] = match &self.open_checkouts {
            OpenCheckouts::Loaded(open) => open,
            OpenCheckouts::Loading | OpenCheckouts::Failed => &[],
        };
        for branch in &mut self.entries {
            branch.apply_open_checkouts(open);
        }
    }

    pub fn record_fetch_warning(&mut self, remote: String) -> bool {
        self.fetch_warning_remotes.insert(remote)
    }

    pub fn should_warn_unsupported_refs(&mut self, generation: u64, skipped: bool) -> bool {
        if skipped && self.unsupported_ref_warning_generation != Some(generation) {
            self.unsupported_ref_warning_generation = Some(generation);
            true
        } else {
            false
        }
    }

    /// Puts the listing at `generation` straight into `load`, as tests set up a branch list
    /// without loading one.
    #[cfg(test)]
    pub(crate) fn set_load(&mut self, generation: u64, load: BranchLoad) {
        self.generation = generation;
        self.load = load;
    }

    #[cfg(test)]
    pub(crate) fn remote_snapshots_are_empty(&self) -> bool {
        self.remote_branches.is_empty()
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn handle_event(
    event: AppEvent,
    state: &mut AppState,
    changes: &mut TickChanges,
) -> Option<AppEvent> {
    match event {
        AppEvent::FilterCompleted {
            target: FilterTarget::Branches,
            generation,
            matches,
            selected,
        } => {
            if generation == state.branch_view.filter_generation {
                apply_filter_result(state, &matches, selected.as_ref());
            }
        }
        AppEvent::BranchesLoaded {
            repo_path,
            generation,
            branches,
            worktrees,
            skipped_unsupported_refs,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                if let Some(entry) = state
                    .repo_view
                    .entries
                    .iter_mut()
                    .find(|entry| entry.repo.path == repo_path)
                {
                    entry.repo.worktrees = worktrees;
                }
                if let Some(selection) = state.branch_view.finish_load(branches) {
                    changes.pinned_branch_selection = Some(selection);
                }
                super::delete::reconcile_pending(state, &repo_path);
                changes.start_remote_loading = Some((
                    repo_path.clone(),
                    generation,
                    state
                        .branch_view
                        .entries
                        .iter()
                        .map(|branch| branch.name.clone())
                        .collect(),
                ));
                changes.branches_changed = true;
                changes.resume_pending_deletes =
                    matches!(state.branch_view.open_checkouts(), OpenCheckouts::Loaded(_));
                warn_unsupported_refs(state, generation, skipped_unsupported_refs);
            }
        }
        AppEvent::RemoteBranchesLoaded {
            repo_path,
            generation,
            remote,
            branches,
            skipped_unsupported_refs,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                merge_remote_snapshot(state, changes, remote, branches);
                state.branch_view.apply_open_indicators();
                changes.branches_changed = true;
                super::new_branch::refresh_remote_prompt(state);
                warn_unsupported_refs(state, generation, skipped_unsupported_refs);
            }
        }
        AppEvent::RemoteBranchLoadFailed {
            repo_path,
            generation,
            message,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                state.push_toast(ToastKind::Warning, message);
            }
        }
        AppEvent::GitFetchCompleted {
            remote,
            branches,
            repo_path,
            generation,
            error,
            skipped_unsupported_refs,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                merge_remote_snapshot(state, changes, remote.clone(), branches);
                state.branch_view.apply_open_indicators();
                changes.branches_changed = true;
                if let Some(error) = error
                    && state.branch_view.record_fetch_warning(remote.clone())
                {
                    state.push_toast(
                        ToastKind::Warning,
                        format!("could not fetch remote {remote}: {error}"),
                    );
                }
                super::new_branch::refresh_remote_prompt(state);
                warn_unsupported_refs(state, generation, skipped_unsupported_refs);
            }
        }
        AppEvent::GitFetchesFinished {
            repo_path,
            generation,
            error,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                if let Some(error) = error {
                    state.push_toast(ToastKind::Warning, error);
                }
                state.branch_view.finish_remote_fetches();
                super::new_branch::refresh_remote_prompt(state);
            }
        }
        AppEvent::BranchLoadFailed {
            repo_path,
            generation,
            message,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                state.mode = Mode::RepoSelect;
                state.push_toast(ToastKind::Error, message);
            }
        }
        AppEvent::OpenCheckoutsLoaded {
            repo_path,
            generation,
            open,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                state
                    .branch_view
                    .set_open_checkouts(OpenCheckouts::Loaded(open));
                super::delete::refresh_open_state(state);
                changes.resume_pending_deletes = true;
            }
        }
        AppEvent::OpenCheckoutsFailed {
            repo_path,
            generation,
            message,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                state.branch_view.set_open_checkouts(OpenCheckouts::Failed);
                super::delete::refresh_open_state(state);
                state.push_toast(ToastKind::Error, message);
            }
        }
        AppEvent::BranchOperationFailed { repo_path, failure } => {
            if branch_context_matches(state, &repo_path)
                && matches!(
                    state.mode,
                    Mode::Loading {
                        branch: Some(_),
                        ..
                    }
                )
            {
                let context = state.branch_context().cloned().unwrap();
                state.mode = Mode::BranchSelect(context);
                let message = match failure {
                    BranchOperationFailure::Failed(message) => message,
                    BranchOperationFailure::LocalBranchAvailable {
                        branch_name,
                        tracking_created,
                        message,
                    } => {
                        let selection = BranchId::Local(branch_name.clone());
                        state.branch_view.promote_remote_to_local(&branch_name);
                        state.branch_view.list.input.clear();
                        changes.pinned_branch_selection = Some(selection.clone());
                        changes.branches_changed = true;
                        state.branch_view.begin_load(Some(selection));
                        changes.refresh_branch = state
                            .repo_view
                            .entries
                            .iter()
                            .find(|entry| entry.repo.path == repo_path)
                            .map(|entry| entry.repo.clone());
                        if tracking_created {
                            format!(
                                "Tracking branch {branch_name} was created, but its checkout could not be opened: {message}"
                            )
                        } else {
                            format!(
                                "Local branch {branch_name} now exists, but its checkout could not be opened: {message}"
                            )
                        }
                    }
                };
                state.push_toast(ToastKind::Error, message);
            }
        }
        event => return Some(event),
    }
    None
}

pub(crate) fn matches_repo(state: &AppState, repo_path: &Path) -> bool {
    matches!(&state.mode, Mode::BranchSelect(context) if context.repo_path == repo_path)
}

fn branch_context_matches(state: &AppState, repo_path: &Path) -> bool {
    state
        .branch_context()
        .is_some_and(|context| context.repo_path == repo_path)
}

/// Whether a background result for `repo_path`'s listing `generation` is for the branch
/// list in front. It is whichever dialog the branch list has open.
fn branch_context_generation_matches(state: &AppState, repo_path: &Path, generation: u64) -> bool {
    branch_context_matches(state, repo_path) && state.branch_view.generation() == generation
}

fn pin_selection(state: &AppState, changes: &mut TickChanges) {
    if changes.pinned_branch_selection.is_none() {
        changes.pinned_branch_selection = state.selected_branch().map(BranchEntry::id);
    }
}

fn merge_remote_snapshot(
    state: &mut AppState,
    changes: &mut TickChanges,
    remote: String,
    branches: Vec<BranchEntry>,
) {
    let selected = state.selected_branch().map(BranchEntry::id);
    pin_selection(state, changes);
    let visible = state
        .branch_view
        .list
        .filtered
        .iter()
        .filter_map(|(index, score)| {
            state
                .branch_view
                .entries
                .get(*index)
                .map(|branch| (branch.id(), *score))
        })
        .collect::<Vec<_>>();

    state.branch_view.merge_remote_snapshot(remote, branches);
    let indices = state
        .branch_view
        .entries
        .iter()
        .enumerate()
        .map(|(index, branch)| (branch.id(), index))
        .collect::<std::collections::HashMap<_, _>>();
    state.branch_view.list.filtered = visible
        .iter()
        .filter_map(|(id, score)| indices.get(id).map(|index| (*index, *score)))
        .collect();
    state.branch_view.list.selected = selected
        .as_ref()
        .and_then(|id| {
            state
                .branch_view
                .list
                .filtered
                .iter()
                .position(|(index, _)| state.branch_view.entries[*index].id() == *id)
        })
        .or_else(|| (!state.branch_view.list.filtered.is_empty()).then_some(0));
}

pub(crate) fn queue_filter(
    state: &mut AppState,
    worker: &FilterWorker,
    selected_id: Option<BranchId>,
) {
    state.branch_view.filter_generation = state.branch_view.filter_generation.wrapping_add(1);
    if state.branch_view.list.input.text.is_empty() {
        state.branch_view.list.filtered = (0..state.branch_view.entries.len())
            .map(|index| (index, 0))
            .collect();
        if state.branch_view.entries.is_empty() {
            state.branch_view.list.selected = None;
        } else {
            state.branch_view.list.selected = selected_id
                .as_ref()
                .and_then(|id| {
                    state
                        .branch_view
                        .entries
                        .iter()
                        .position(|branch| branch.id() == *id)
                })
                .or(Some(0));
        }
        state.branch_view.list.scroll_offset = 0;
        return;
    }
    worker.request(FilterRequest {
        target: FilterTarget::Branches,
        generation: state.branch_view.filter_generation,
        query: state.branch_view.list.input.text.clone(),
        items: state
            .branch_view
            .entries
            .iter()
            .map(|branch| FilterItem {
                key: FilterKey::Branch(branch.id()),
                text: branch.display_name(),
            })
            .collect(),
        selected: selected_id.map(FilterKey::Branch),
    });
}

fn apply_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let current = state.selected_branch().map(BranchEntry::id);
    let indices = state
        .branch_view
        .entries
        .iter()
        .enumerate()
        .map(|(index, branch)| (branch.id(), index))
        .collect::<std::collections::HashMap<_, _>>();
    state.branch_view.list.filtered = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Branch(id) => indices.get(id).map(|index| (*index, *score)),
            FilterKey::Repo(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
        })
        .collect();
    let requested = selected.and_then(|key| match key {
        FilterKey::Branch(id) => Some(id.clone()),
        FilterKey::Repo(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
    });
    state.branch_view.list.selected = current
        .or(requested)
        .as_ref()
        .and_then(|id| {
            state
                .branch_view
                .list
                .filtered
                .iter()
                .position(|(index, _)| state.branch_view.entries[*index].id() == *id)
        })
        .or_else(|| (!state.branch_view.list.filtered.is_empty()).then_some(0));
    state.branch_view.list.scroll_offset = 0;
}

fn warn_unsupported_refs(state: &mut AppState, generation: u64, skipped: bool) {
    if state
        .branch_view
        .should_warn_unsupported_refs(generation, skipped)
    {
        state.push_toast(ToastKind::Warning, crate::git::UNSUPPORTED_REF_WARNING);
    }
}

pub(crate) fn enter<B: Backend>(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    backend: Option<&Arc<B>>,
    sender: &EventSender,
) {
    let Some(entry) = state.selected_repo() else {
        return;
    };
    if !entry.repo.is_git {
        state.push_toast(
            ToastKind::Warning,
            "Branches are only available for git repositories",
        );
        return;
    }
    let context = BranchContext {
        repo_path: entry.repo.path.clone(),
        repo_name: entry.repo.name.clone(),
    };
    let repo = entry.repo.clone();
    // The user chose this repo, so a late preselection must not move off it.
    state.repo_view.selection_touched = true;
    state.mode = Mode::BranchSelect(context);
    state.branch_view.entries.clear();
    state.branch_view.list = SearchableList::new(0);
    state.branch_view.filter_generation = state.branch_view.filter_generation.wrapping_add(1);
    state.branch_view.begin_load(None);
    load(state, git, backend, sender, repo);
}

/// Loads the current generation's listing of `repo` and its open checkouts.
pub(crate) fn load<B: Backend>(
    state: &AppState,
    git: &Arc<dyn GitProvider>,
    backend: Option<&Arc<B>>,
    sender: &EventSender,
    repo: Repo,
) {
    let repo_path = repo.path.clone();
    let generation = state.branch_view.generation();
    spawn_branch_loading(
        git,
        sender,
        repo,
        state.repo_view.current_cwd.clone(),
        generation,
    );
    if let Some(backend) = backend {
        spawn_open_checkouts(backend, sender, repo_path, generation);
    }
}

pub(crate) fn open_selected<B: Backend>(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    backend: Option<&Arc<B>>,
    sender: &EventSender,
) {
    let Some(branch) = state.selected_branch().cloned() else {
        return;
    };
    open(state, git, backend, sender, &branch);
}

pub(crate) fn open<B: Backend>(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    backend: Option<&Arc<B>>,
    sender: &EventSender,
    branch: &BranchEntry,
) {
    let Some(context) = state.branch_context().cloned() else {
        return;
    };
    let branch_name = branch.name.clone();
    let has_worktree = branch.checkout.is_some();
    let remote = branch.remote.clone();
    let Some(backend) = backend else {
        state.push_toast(ToastKind::Error, state.product.no_backend_message);
        return;
    };
    let verb = if remote.is_some() {
        format!("Checking out remote branch {branch_name}…")
    } else if has_worktree {
        format!("Opening {branch_name}…")
    } else {
        format!("Creating worktree for {branch_name}…")
    };
    state.mode = Mode::Loading {
        message: verb,
        branch: Some(context.clone()),
    };
    if let Some(remote) = remote {
        spawn_open_remote_branch(git, backend, sender, context.repo_path, branch_name, remote);
    } else {
        let known = if has_worktree {
            KnownCheckout::Exists
        } else {
            KnownCheckout::Missing
        };
        spawn_open_branch(git, backend, sender, context.repo_path, branch_name, known);
    }
}

pub(crate) fn move_selection(state: &mut AppState, delta: i32) {
    state.branch_view.list.move_selection(delta);
}

pub(crate) fn edit(
    state: &mut AppState,
    worker: &FilterWorker,
    edit: impl FnOnce(&mut SearchableList),
) {
    edit(&mut state.branch_view.list);
    queue_filter(state, worker, None);
}

pub(crate) fn leave(state: &mut AppState) {
    state.mode = Mode::RepoSelect;
}

#[cfg(test)]
mod tests;
