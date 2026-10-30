use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::PathBuf,
    sync::Arc,
};

use super::{FilterItem, FilterRequest, FilterWorker, TickChanges};
use crate::{
    backend::Backend,
    event::{AppEvent, FilterKey, FilterTarget},
    git::Repo,
    spawn::{EventSender, spawn_open_folder, spawn_open_repo},
    state::{AppState, Mode, SearchableList, ToastKind},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEntry {
    pub repo: Repo,
    pub disambiguator: Option<String>,
    pub is_open: bool,
    /// `repo.path` with symlinks resolved, for comparing against paths from git and the
    /// backend.
    canonical_path: PathBuf,
}

impl RepoEntry {
    pub fn new(repo: Repo) -> Self {
        let canonical_path = crate::path::canonical_or_original(&repo.path);
        Self {
            repo,
            disambiguator: None,
            is_open: false,
            canonical_path,
        }
    }

    pub fn display_name(&self) -> String {
        self.disambiguator.as_ref().map_or_else(
            || self.repo.name.clone(),
            |suffix| format!("{} ({suffix})", self.repo.name),
        )
    }
}

#[derive(Debug)]
pub struct RepoViewState {
    pub entries: Vec<RepoEntry>,
    pub list: SearchableList,
    pub loading: bool,
    pub selection_touched: bool,
    /// Optional search directories skipped at startup because they do not exist.
    pub missing_optional_dirs: usize,
    seen_paths: HashSet<PathBuf>,
    open_roots: HashSet<PathBuf>,
    open_folder_roots: HashSet<PathBuf>,
    folder_indicators_requested: bool,
    pub(crate) current_cwd: Option<PathBuf>,
    /// The main checkout of the repository `current_cwd` is in, once git has resolved it.
    /// It identifies the repository even from a linked worktree outside the search
    /// directories.
    current_main_checkout: Option<PathBuf>,
    pub(crate) filter_generation: u64,
}

impl RepoViewState {
    pub fn new(current_cwd: Option<PathBuf>) -> Self {
        Self {
            entries: Vec::new(),
            list: SearchableList::new(0),
            loading: true,
            selection_touched: false,
            missing_optional_dirs: 0,
            seen_paths: HashSet::new(),
            open_roots: HashSet::new(),
            open_folder_roots: HashSet::new(),
            folder_indicators_requested: false,
            current_cwd,
            current_main_checkout: None,
            filter_generation: 0,
        }
    }

    pub fn selected(&self) -> Option<&RepoEntry> {
        let selected = self.list.selected?;
        let index = self.list.filtered.get(selected)?.0;
        self.entries.get(index)
    }

    fn canonical_sort(&mut self) {
        let selected_path = self.selected().map(|entry| entry.repo.path.clone());
        let filtered_paths = self
            .list
            .filtered
            .iter()
            .filter_map(|(index, score)| {
                self.entries
                    .get(*index)
                    .map(|entry| (entry.repo.path.clone(), *score))
            })
            .collect::<Vec<_>>();
        self.entries.sort_by(|left, right| {
            left.repo
                .name
                .to_lowercase()
                .cmp(&right.repo.name.to_lowercase())
                .then(left.repo.name.cmp(&right.repo.name))
                .then(left.repo.path.cmp(&right.repo.path))
        });
        if self.list.input.text.is_empty() {
            self.list.filtered = (0..self.entries.len()).map(|index| (index, 0)).collect();
            self.list.selected = selected_path
                .and_then(|path| {
                    self.entries
                        .iter()
                        .position(|entry| entry.repo.path == path)
                })
                .or_else(|| (!self.entries.is_empty()).then_some(0));
        } else {
            let indices: HashMap<_, _> = self
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.repo.path.as_path(), index))
                .collect();
            self.list.filtered = filtered_paths
                .iter()
                .filter_map(|(path, score)| {
                    indices.get(path.as_path()).map(|index| (*index, *score))
                })
                .collect();
            self.list.selected = selected_path.and_then(|path| {
                self.list
                    .filtered
                    .iter()
                    .position(|(index, _)| self.entries[*index].repo.path == path)
            });
        }
    }

    /// Preselects the repository the user is in, unless they have moved the selection:
    /// the one git resolves as `current_cwd`'s main checkout, or else the deepest entry
    /// containing `current_cwd`, which also covers folders that are not repositories.
    pub(crate) fn apply_current_selection(&mut self) {
        if self.selection_touched {
            return;
        }
        let Some(cwd) = self.current_cwd.as_deref() else {
            return;
        };
        let entries = &self.entries;
        let filtered = || {
            self.list
                .filtered
                .iter()
                .enumerate()
                .map(|(position, (index, _))| (position, &entries[*index]))
        };
        let main_checkout = self.current_main_checkout.as_deref().and_then(|main| {
            filtered()
                .find(|(_, entry)| crate::path::equivalent(&entry.canonical_path, main))
                .map(|(position, _)| position)
        });
        let best = main_checkout.or_else(|| {
            filtered()
                .filter(|(_, entry)| crate::path::starts_with(cwd, &entry.canonical_path))
                .max_by_key(|(_, entry)| entry.canonical_path.components().count())
                .map(|(position, _)| position)
        });
        if best.is_some() {
            self.list.selected = best;
        }
    }
}

pub(crate) fn handle_event(
    event: AppEvent,
    state: &mut AppState,
    changes: &mut TickChanges,
) -> Option<AppEvent> {
    match event {
        AppEvent::ReposFound { repo } => {
            if !repo.is_git && !state.repo_view.folder_indicators_requested {
                state.repo_view.folder_indicators_requested = true;
                changes.load_open_folder_indicators = true;
            }
            changes.repos_changed |= add(state, repo);
        }
        AppEvent::ScanComplete => {
            state.repo_view.loading = false;
            changes.collision_pass = true;
        }
        AppEvent::ScanWarning(_warning) => {
            state.push_scan_warning();
        }
        AppEvent::CurrentRepoResolved { main_checkout } => {
            state.repo_view.current_main_checkout =
                Some(crate::path::canonical_or_original(&main_checkout));
            state.repo_view.apply_current_selection();
        }
        AppEvent::OpenReposLoaded { roots } => {
            state.repo_view.open_roots = roots
                .iter()
                .map(|root| crate::path::canonical_or_original(root))
                .collect();
            apply_open_indicators(state);
        }
        AppEvent::OpenFoldersLoaded { folders } => {
            state.repo_view.open_folder_roots = folders
                .iter()
                .map(|folder| crate::path::canonical_or_original(folder))
                .collect();
            apply_open_indicators(state);
        }
        AppEvent::FilterCompleted {
            target: FilterTarget::Repos,
            generation,
            matches,
            selected,
        } if generation == state.repo_view.filter_generation => {
            apply_filter_result(state, &matches, selected.as_ref());
        }
        AppEvent::RepoOpenFailed(message)
            if matches!(state.mode, Mode::Loading { branch: None, .. }) =>
        {
            state.mode = Mode::RepoSelect;
            state.push_toast(ToastKind::Error, message);
        }
        AppEvent::CurrentRepoFailed(message)
        | AppEvent::OpenReposFailed(message)
        | AppEvent::OpenFoldersFailed(message) => {
            state.push_toast(ToastKind::Warning, message);
        }
        event => return Some(event),
    }
    None
}

pub(crate) fn apply_changes(
    state: &mut AppState,
    changes: &mut TickChanges,
    worker: &FilterWorker,
) {
    if changes.repos_changed {
        state.repo_view.canonical_sort();
        state.repo_view.apply_current_selection();
    }
    if changes.collision_pass {
        apply_collisions(state);
        state.repo_view.canonical_sort();
        state.repo_view.apply_current_selection();
        changes.repos_changed = true;
    }
    if changes.repos_changed && matches!(state.mode, Mode::RepoSelect) {
        queue_filter(state, worker, true);
    }
}

pub(crate) fn move_selection(state: &mut AppState, delta: i32) {
    state.repo_view.selection_touched = true;
    state.repo_view.list.move_selection(delta);
}

pub(crate) fn edit(
    state: &mut AppState,
    worker: &FilterWorker,
    edit: impl FnOnce(&mut SearchableList),
) {
    state.repo_view.selection_touched = true;
    edit(&mut state.repo_view.list);
    queue_filter(state, worker, false);
}

pub(crate) fn queue_filter(state: &mut AppState, worker: &FilterWorker, preserve_selection: bool) {
    state.repo_view.filter_generation = state.repo_view.filter_generation.wrapping_add(1);
    if state.repo_view.list.input.text.is_empty() {
        state.repo_view.canonical_sort();
        if !preserve_selection {
            state.repo_view.list.selected = (!state.repo_view.entries.is_empty()).then_some(0);
        }
        if preserve_selection {
            state.repo_view.apply_current_selection();
        }
        return;
    }
    let selected = preserve_selection
        .then(|| state.selected_repo().map(|entry| entry.repo.path.clone()))
        .flatten()
        .map(FilterKey::Repo);
    worker.request(FilterRequest {
        target: FilterTarget::Repos,
        generation: state.repo_view.filter_generation,
        query: state.repo_view.list.input.text.clone(),
        items: state
            .repo_view
            .entries
            .iter()
            .map(|entry| FilterItem {
                key: FilterKey::Repo(entry.repo.path.clone()),
                text: entry.display_name(),
            })
            .collect(),
        selected,
    });
}

pub(crate) fn open_selected<B: Backend>(
    state: &mut AppState,
    backend: Option<&Arc<B>>,
    sender: &EventSender,
) {
    let Some(entry) = state.selected_repo() else {
        return;
    };
    let repo_path = entry.repo.path.clone();
    let repo_name = entry.repo.name.clone();
    let is_git = entry.repo.is_git;
    let Some(backend) = backend else {
        state.push_toast(ToastKind::Error, state.product.no_backend_message);
        return;
    };
    state.mode = Mode::Loading {
        message: format!("Opening {repo_name}…"),
        branch: None,
    };
    if is_git {
        spawn_open_repo(backend, sender, repo_path);
    } else {
        spawn_open_folder(backend, sender, repo_path);
    }
}

fn add(state: &mut AppState, repo: Repo) -> bool {
    if !state.repo_view.seen_paths.insert(repo.path.clone()) {
        return false;
    }
    let mut entry = RepoEntry::new(repo);
    entry.is_open = if entry.repo.is_git {
        state.repo_view.open_roots.contains(&entry.canonical_path)
    } else {
        state
            .repo_view
            .open_folder_roots
            .contains(&entry.canonical_path)
    };
    state.repo_view.entries.push(entry);
    true
}

fn apply_collisions(state: &mut AppState) {
    let repos = state
        .repo_view
        .entries
        .iter()
        .map(|entry| entry.repo.clone())
        .collect::<Vec<_>>();
    let disambiguators = collision_disambiguators(&repos);
    for (entry, disambiguator) in state.repo_view.entries.iter_mut().zip(disambiguators) {
        entry.disambiguator = disambiguator;
    }
}

fn apply_open_indicators(state: &mut AppState) {
    for entry in &mut state.repo_view.entries {
        let open_paths = if entry.repo.is_git {
            &state.repo_view.open_roots
        } else {
            &state.repo_view.open_folder_roots
        };
        entry.is_open = open_paths
            .iter()
            .any(|open_path| crate::path::equivalent(open_path, &entry.canonical_path));
    }
}

fn apply_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let current = state.selected_repo().map(|entry| entry.repo.path.clone());
    let indices: HashMap<_, _> = state
        .repo_view
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.repo.path.as_path(), index))
        .collect();
    state.repo_view.list.filtered = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Repo(path) => indices.get(path.as_path()).map(|index| (*index, *score)),
            FilterKey::Branch(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
        })
        .collect();
    let requested = selected.and_then(|key| match key {
        FilterKey::Repo(path) => Some(path.clone()),
        FilterKey::Branch(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
    });
    state.repo_view.list.selected = current
        .or(requested)
        .as_ref()
        .and_then(|path| {
            state
                .repo_view
                .list
                .filtered
                .iter()
                .position(|(index, _)| state.repo_view.entries[*index].repo.path == *path)
        })
        .or_else(|| (!state.repo_view.list.filtered.is_empty()).then_some(0));
    state.repo_view.list.scroll_offset = 0;
}

fn collision_disambiguators(repos: &[Repo]) -> Vec<Option<String>> {
    collision_disambiguators_with_case(repos, cfg!(windows))
}

fn collision_disambiguators_with_case(
    repos: &[Repo],
    case_insensitive: bool,
) -> Vec<Option<String>> {
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, repo) in repos.iter().enumerate() {
        let key = if case_insensitive {
            repo.name.to_lowercase()
        } else {
            repo.name.clone()
        };
        groups.entry(key).or_default().push(index);
    }
    let parents: Vec<Vec<OsString>> = repos
        .iter()
        .map(|repo| {
            repo.path
                .parent()
                .map(|parent| {
                    parent
                        .components()
                        .filter_map(|component| match component {
                            std::path::Component::Normal(value) => Some(value.to_os_string()),
                            std::path::Component::Prefix(value) => {
                                Some(value.as_os_str().to_os_string())
                            }
                            std::path::Component::RootDir
                            | std::path::Component::CurDir
                            | std::path::Component::ParentDir => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    let mut result = vec![None; repos.len()];

    for indices in groups.values().filter(|indices| indices.len() > 1) {
        for &index in indices {
            let parent = &parents[index];
            let depth = (1..=parent.len())
                .find(|depth| {
                    let suffix = &parent[parent.len() - depth..];
                    indices.iter().all(|other| {
                        *other == index
                            || parents[*other].len() < *depth
                            || !path_components_equal(
                                &parents[*other][parents[*other].len() - depth..],
                                suffix,
                                case_insensitive,
                            )
                    })
                })
                .unwrap_or_else(|| {
                    panic!(
                        "duplicate repository path invariant violated for {}",
                        repos[index].path.display()
                    )
                });
            let suffix = parent[parent.len() - depth..]
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            result[index] = Some(if depth < parent.len() {
                format!("…/{suffix}")
            } else {
                suffix
            });
        }
    }
    result
}

fn path_components_equal(left: &[OsString], right: &[OsString], case_insensitive: bool) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left == right
                || case_insensitive
                    && left.to_string_lossy().to_lowercase()
                        == right.to_string_lossy().to_lowercase()
        })
}

#[cfg(test)]
#[path = "repo/tests.rs"]
mod tests;
