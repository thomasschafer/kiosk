//! The picker's behaviour: how events from background work and user actions change
//! [`AppState`] and which work they start. Drawing is left to the UI.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

use crate::{
    backend::Backend,
    config::{ResolvedSearchDir, keys::KeysConfig},
    event::{AppEvent, FilterKey, FilterTarget},
    git::{GitProvider, Repo},
    keyboard::Action,
    spawn::{
        EventSender, FetchDeduplicator, spawn_git_fetch, spawn_open_folders, spawn_open_repos,
        spawn_remote_branch_loading, spawn_repo_discovery,
    },
    state::{AppState, BranchId, HelpOverlayState, Mode, SearchableList},
};

pub mod branch;
pub mod delete;
pub mod new_branch;
pub mod repo;

/// At most this many background events are applied between two redraws.
pub const MAX_EVENTS_PER_TICK: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Quit,
    Opened,
}

/// A running picker: its background work and the channel that work reports on.
pub struct Picker<B: Backend> {
    git: Arc<dyn GitProvider>,
    backend: Option<Arc<B>>,
    keys: KeysConfig,
    sender: EventSender,
    receiver: mpsc::Receiver<AppEvent>,
    filter_worker: FilterWorker,
    fetch_deduplicator: FetchDeduplicator,
}

/// What one [`Picker::tick`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    /// Whether any background event arrived, so the screen may have changed.
    pub changed: bool,
    pub outcome: Option<RunOutcome>,
}

impl<B: Backend> Picker<B> {
    /// Starts scanning `search_dirs` and asks the backend what is open. Without a backend
    /// nothing can be opened, but the picker can still be browsed.
    pub fn start(
        git: Arc<dyn GitProvider>,
        backend: Option<Arc<B>>,
        keys: KeysConfig,
        search_dirs: Vec<ResolvedSearchDir>,
    ) -> Self {
        let (tx, receiver) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        let filter_worker = FilterWorker::spawn(sender.clone());
        spawn_repo_discovery(&git, &sender, search_dirs);
        if let Some(backend) = &backend {
            spawn_open_repos(backend, &sender);
        }
        Self {
            git,
            backend,
            keys,
            sender,
            receiver,
            filter_worker,
            fetch_deduplicator: FetchDeduplicator::default(),
        }
    }

    pub fn keys(&self) -> &KeysConfig {
        &self.keys
    }

    /// Applies the background events that have arrived and starts the work they call for.
    pub fn tick(&self, state: &mut AppState) -> Tick {
        let mut changes = TickChanges::default();
        let mut received = false;
        for event in self.receiver.try_iter().take(MAX_EVENTS_PER_TICK) {
            received = true;
            process_app_event(event, state, &mut changes);
        }
        load_open_folder_indicators(&mut changes, self.backend.as_ref(), &self.sender);
        if let Some(outcome) = apply_exit_effects(&mut changes, self.backend.as_deref(), state) {
            return Tick {
                changed: received,
                outcome: Some(outcome),
            };
        }
        self.apply_changes(state, changes);
        Tick {
            changed: received,
            outcome: None,
        }
    }

    fn apply_changes(&self, state: &mut AppState, mut changes: TickChanges) {
        repo::apply_changes(state, &mut changes, &self.filter_worker);
        if changes.branches_changed {
            branch::queue_filter(
                state,
                &self.filter_worker,
                changes.pinned_branch_selection.take(),
            );
        }
        if let Some((repo_path, generation, local_names)) = changes.start_remote_loading.take() {
            spawn_remote_branch_loading(
                &self.git,
                &self.sender,
                repo_path.clone(),
                local_names.clone(),
                generation,
            );
            spawn_git_fetch(
                &self.git,
                &self.sender,
                &self.fetch_deduplicator,
                repo_path,
                local_names,
                generation,
            );
        }
        if let Some(repo) = changes.refresh_branch.take() {
            branch::refresh(state, &self.git, self.backend.as_ref(), &self.sender, repo);
        }
        if changes.resume_pending_deletes {
            delete::resume(state, &self.git, self.backend.as_ref(), &self.sender);
        }
    }

    pub fn handle_action(&self, state: &mut AppState, action: Action) -> Option<RunOutcome> {
        process_action(
            action,
            state,
            &self.git,
            self.backend.as_ref(),
            &self.sender,
            &self.filter_worker,
            &self.keys,
        )
    }
}

impl<B: Backend> Drop for Picker<B> {
    fn drop(&mut self) {
        self.sender.cancel();
    }
}

#[derive(Clone)]
pub(crate) struct FilterItem {
    pub(crate) key: FilterKey,
    pub(crate) text: String,
}

pub(crate) struct FilterRequest {
    pub(crate) target: FilterTarget,
    pub(crate) generation: u64,
    pub(crate) query: String,
    pub(crate) items: Vec<FilterItem>,
    pub(crate) selected: Option<FilterKey>,
}

/// Fuzzy-filters lists off the UI thread. Only the latest request is kept, so typing
/// faster than filtering skips stale queries.
pub(crate) struct FilterWorker {
    pending: Arc<(Mutex<Option<FilterRequest>>, Condvar)>,
    cancel: Arc<AtomicBool>,
}

impl FilterWorker {
    pub(crate) fn spawn(sender: EventSender) -> Self {
        let pending = Arc::new((Mutex::new(None::<FilterRequest>), Condvar::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_pending = Arc::clone(&pending);
        let thread_cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            let matcher = SkimMatcherV2::default();
            loop {
                let request = {
                    let (lock, condition) = &*thread_pending;
                    let mut request = lock.lock().unwrap();
                    while request.is_none() && !thread_cancel.load(Ordering::Relaxed) {
                        request = condition.wait(request).unwrap();
                    }
                    if thread_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    request.take().unwrap()
                };
                let filtered = fuzzy_filter(&request.query, &request.items, &matcher);
                sender.send(AppEvent::FilterCompleted {
                    target: request.target,
                    generation: request.generation,
                    matches: filtered,
                    selected: request.selected,
                });
            }
        });
        Self { pending, cancel }
    }

    pub(crate) fn request(&self, request: FilterRequest) {
        let (lock, condition) = &*self.pending;
        *lock.lock().unwrap() = Some(request);
        condition.notify_one();
    }
}

impl Drop for FilterWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.pending.1.notify_one();
    }
}

fn fuzzy_filter(
    query: &str,
    items: &[FilterItem],
    matcher: &SkimMatcherV2,
) -> Vec<(FilterKey, i64)> {
    if query.is_empty() {
        return items.iter().map(|item| (item.key.clone(), 0)).collect();
    }
    let mut scored: Vec<_> = items
        .iter()
        .filter_map(|item| {
            matcher
                .fuzzy_match(&item.text, query)
                .map(|score| (item, score))
        })
        .collect();
    scored.sort_by(|(left, left_score), (right, right_score)| {
        right_score
            .cmp(left_score)
            .then(left.text.len().cmp(&right.text.len()))
            .then(left.text.cmp(&right.text))
    });
    scored
        .into_iter()
        .map(|(item, score)| (item.key.clone(), score))
        .collect()
}

fn load_open_folder_indicators<B: Backend>(
    changes: &mut TickChanges,
    backend: Option<&Arc<B>>,
    sender: &EventSender,
) {
    if !std::mem::take(&mut changes.load_open_folder_indicators) {
        return;
    }
    if let Some(backend) = backend {
        spawn_open_folders(backend, sender);
    }
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct TickChanges {
    pub(crate) repos_changed: bool,
    pub(crate) branches_changed: bool,
    pub(crate) collision_pass: bool,
    pub(crate) load_open_folder_indicators: bool,
    pub(crate) workspace_opened: bool,
    pub(crate) open_warning: Option<String>,
    pub(crate) pinned_branch_selection: Option<BranchId>,
    pub(crate) start_remote_loading: Option<(PathBuf, u64, Vec<String>)>,
    pub(crate) refresh_branch: Option<Repo>,
    pub(crate) resume_pending_deletes: bool,
}

pub(crate) fn process_app_event(event: AppEvent, state: &mut AppState, changes: &mut TickChanges) {
    let Some(event) = branch::handle_event(event, state, changes) else {
        return;
    };
    let Some(event) = delete::handle_event(event, state, changes) else {
        return;
    };
    let Some(event) = repo::handle_event(event, state, changes) else {
        return;
    };
    let Some(event) = new_branch::handle_event(event, state, changes) else {
        return;
    };
    match event {
        AppEvent::FilterCompleted {
            target,
            generation,
            matches,
            selected,
        } => match target {
            FilterTarget::Help if generation == state.help_filter_generation => {
                apply_help_filter_result(state, &matches, selected.as_ref());
            }
            FilterTarget::Repos
            | FilterTarget::Branches
            | FilterTarget::Bases
            | FilterTarget::Help => {}
        },
        AppEvent::RepoOpened { warning } => {
            changes.open_warning = warning;
            changes.workspace_opened = true;
        }
        _ => {}
    }
}

/// Once something has opened the picker is done. A warning from opening it is passed on as
/// a notification, as the picker is about to close.
fn apply_exit_effects<B: Backend>(
    changes: &mut TickChanges,
    backend: Option<&B>,
    state: &AppState,
) -> Option<RunOutcome> {
    if !changes.workspace_opened {
        return None;
    }
    if let Some(warning) = changes.open_warning.take() {
        let name = state.product.name;
        if let Some(backend) = backend {
            if let Err(error) = backend.notify(name, &warning) {
                eprintln!("{name}: {warning} (notification failed: {error})");
            }
        } else {
            eprintln!("{name}: {warning}");
        }
    }
    Some(RunOutcome::Opened)
}

pub(crate) fn process_action<B: Backend>(
    action: Action,
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    backend: Option<&Arc<B>>,
    sender: &EventSender,
    filter_worker: &FilterWorker,
    keys: &KeysConfig,
) -> Option<RunOutcome> {
    match action {
        Action::Quit => return Some(RunOutcome::Quit),
        Action::MoveSelection(delta) => {
            if let Some(overlay) = &mut state.help_overlay {
                overlay.list.move_selection(delta);
            } else if matches!(state.mode, Mode::RepoSelect) {
                repo::move_selection(state, delta);
            } else if matches!(state.mode, Mode::BranchSelect(_)) {
                branch::move_selection(state, delta);
            } else if matches!(
                state.mode,
                Mode::SelectBaseBranch { .. } | Mode::ConfirmRemoteBranch { .. }
            ) {
                new_branch::move_selection(state, delta);
            }
        }
        Action::Insert(character) => {
            edit_active_list(state, filter_worker, |list| {
                list.input.insert_char(character);
            });
        }
        Action::Backspace => {
            edit_active_list(state, filter_worker, |list| list.input.backspace());
        }
        Action::DeleteWord => {
            edit_active_list(state, filter_worker, |list| list.input.delete_word());
        }
        Action::CursorLeft => active_list_mut(state).input.cursor_left(),
        Action::CursorRight => active_list_mut(state).input.cursor_right(),
        Action::ClearQuery => {
            edit_active_list(state, filter_worker, |list| list.input.clear());
        }
        Action::OpenRepo => repo::open_selected(state, backend, sender),
        Action::OpenBranches => branch::enter(state, git, backend, sender),
        Action::OpenBranch => branch::open_selected(state, git, backend, sender),
        Action::StartNewBranch => new_branch::start(state, git, backend, sender),
        Action::CreateNewBranch => new_branch::create(state, git, backend, sender),
        Action::ResolveRemoteBranch => {
            new_branch::resolve_remote_prompt(state, git, backend, sender);
        }
        Action::DeleteWorktree => delete::begin(state),
        Action::ConfirmDeleteWorktree => {
            delete::confirm(state, git, backend, sender);
        }
        Action::CancelOverlay => cancel_overlay(state),
        Action::BackToRepos => {
            branch::leave(state);
            repo::queue_filter(state, filter_worker, true);
        }
        Action::DismissToast => {
            state.dismiss_toast();
        }
        Action::ShowHelp => {
            let binding_mode = KeysConfig::mode_for(&state.mode);
            state.help_overlay = Some(HelpOverlayState::for_mode(keys, binding_mode));
            state.help_filter_generation = state.help_filter_generation.wrapping_add(1);
        }
        Action::CloseHelp => state.help_overlay = None,
        Action::Noop => {}
    }
    None
}

fn active_list_mut(state: &mut AppState) -> &mut SearchableList {
    if let Some(overlay) = &mut state.help_overlay {
        return &mut overlay.list;
    }
    match &mut state.mode {
        Mode::BranchSelect(_) => &mut state.branch_view.list,
        Mode::SelectBaseBranch { flow, .. } => &mut flow.list,
        Mode::RepoSelect
        | Mode::Loading { .. }
        | Mode::ValidatingNewBranch { .. }
        | Mode::ConfirmRemoteBranch { .. }
        | Mode::ConfirmWorktreeDelete(_) => &mut state.repo_view.list,
    }
}

fn edit_active_list(
    state: &mut AppState,
    worker: &FilterWorker,
    edit: impl FnOnce(&mut SearchableList),
) {
    if let Some(overlay) = &mut state.help_overlay {
        edit(&mut overlay.list);
        queue_help_filter(state, worker, None);
        return;
    }
    match state.mode {
        Mode::RepoSelect => {
            repo::edit(state, worker, edit);
        }
        Mode::BranchSelect(_) => {
            branch::edit(state, worker, edit);
        }
        Mode::SelectBaseBranch { .. } => {
            new_branch::edit(state, worker, edit);
        }
        Mode::Loading { .. }
        | Mode::ValidatingNewBranch { .. }
        | Mode::ConfirmRemoteBranch { .. }
        | Mode::ConfirmWorktreeDelete(_) => {}
    }
}

fn queue_help_filter(state: &mut AppState, worker: &FilterWorker, selected_index: Option<usize>) {
    state.help_filter_generation = state.help_filter_generation.wrapping_add(1);
    let Some(overlay) = &mut state.help_overlay else {
        return;
    };
    if overlay.list.input.text.is_empty() {
        overlay.list.filtered = (0..overlay.rows.len()).map(|index| (index, 0)).collect();
        overlay.list.selected = (!overlay.rows.is_empty()).then_some(0);
        overlay.list.scroll_offset = 0;
        return;
    }
    worker.request(FilterRequest {
        target: FilterTarget::Help,
        generation: state.help_filter_generation,
        query: overlay.list.input.text.clone(),
        items: overlay
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| FilterItem {
                key: FilterKey::Help(index),
                text: row.search_text(),
            })
            .collect(),
        selected: selected_index.map(FilterKey::Help),
    });
}

fn apply_help_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let Some(overlay) = &mut state.help_overlay else {
        return;
    };
    let current = overlay
        .list
        .selected
        .and_then(|selected| overlay.list.filtered.get(selected))
        .map(|(index, _)| *index);
    let scores = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Help(index) if *index < overlay.rows.len() => Some((*index, *score)),
            FilterKey::Repo(_) | FilterKey::Branch(_) | FilterKey::Base(_) | FilterKey::Help(_) => {
                None
            }
        })
        .collect::<HashMap<_, _>>();
    overlay.list.filtered = (0..overlay.rows.len())
        .filter_map(|index| scores.get(&index).map(|score| (index, *score)))
        .collect();
    let requested = selected.and_then(|key| match key {
        FilterKey::Help(index) => Some(*index),
        FilterKey::Repo(_) | FilterKey::Branch(_) | FilterKey::Base(_) => None,
    });
    overlay.list.selected = current
        .or(requested)
        .and_then(|selected| {
            overlay
                .list
                .filtered
                .iter()
                .position(|(index, _)| *index == selected)
        })
        .or_else(|| (!overlay.list.filtered.is_empty()).then_some(0));
    overlay.list.scroll_offset = 0;
}

fn cancel_overlay(state: &mut AppState) {
    if delete::cancel(state) {
        return;
    }
    new_branch::cancel(state);
}

#[cfg(test)]
mod tests;
