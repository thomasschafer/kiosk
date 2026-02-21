use crate::{
    constants::{WORKTREE_DIR_DEDUP_MAX_ATTEMPTS, WORKTREE_DIR_NAME, WORKTREE_NAME_SEPARATOR},
    git::Repo,
};
use std::path::PathBuf;

/// Shared state for any searchable, filterable list.
/// Eliminates the mode-dispatch triplication for search/cursor/movement.
#[derive(Debug, Clone)]
pub struct SearchableList {
    pub search: String,
    pub cursor: usize,
    /// Index-score pairs, sorted by score descending
    pub filtered: Vec<(usize, i64)>,
    pub selected: Option<usize>,
}

impl SearchableList {
    pub fn new(item_count: usize) -> Self {
        Self {
            search: String::new(),
            cursor: 0,
            filtered: (0..item_count).map(|i| (i, 0)).collect(),
            selected: if item_count > 0 { Some(0) } else { None },
        }
    }

    pub fn reset(&mut self, item_count: usize) {
        self.search.clear();
        self.cursor = 0;
        self.filtered = (0..item_count).map(|i| (i, 0)).collect();
        self.selected = if item_count > 0 { Some(0) } else { None };
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

    /// Move cursor left by one char (UTF-8 safe)
    pub fn cursor_left(&mut self) {
        self.cursor = self.search[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
    }

    /// Move cursor right by one char (UTF-8 safe)
    pub fn cursor_right(&mut self) {
        if self.cursor < self.search.len() {
            self.cursor = self.search[self.cursor..]
                .char_indices()
                .nth(1)
                .map_or(self.search.len(), |(i, _)| self.cursor + i);
        }
    }

    pub fn cursor_start(&mut self) {
        self.cursor = 0;
    }

    pub fn cursor_end(&mut self) {
        self.cursor = self.search.len();
    }

    /// Insert a character at the current cursor position
    pub fn insert_char(&mut self, c: char) {
        self.search.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Remove the character before the cursor (UTF-8 safe)
    pub fn backspace(&mut self) -> bool {
        if self.cursor > 0 {
            let prev = self.search[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i);
            self.search.drain(prev..self.cursor);
            self.cursor = prev;
            true
        } else {
            false
        }
    }

    /// Delete word backwards from cursor position
    pub fn delete_word(&mut self) {
        if self.search.is_empty() || self.cursor == 0 {
            return;
        }
        let bytes = self.search.as_bytes();
        let mut new_cursor = self.cursor.min(bytes.len());

        // Skip whitespace
        while new_cursor > 0 && bytes[new_cursor - 1].is_ascii_whitespace() {
            new_cursor -= 1;
        }
        // Delete non-whitespace
        while new_cursor > 0 && !bytes[new_cursor - 1].is_ascii_whitespace() {
            new_cursor -= 1;
        }

        self.search.drain(new_cursor..self.cursor);
        self.cursor = new_cursor;
    }
}

/// Rich branch entry with worktree and session metadata
#[derive(Debug, Clone)]
pub struct BranchEntry {
    pub name: String,
    /// If a worktree already exists for this branch
    pub worktree_path: Option<PathBuf>,
    pub has_session: bool,
    pub is_current: bool,
    /// Remote-only branch (no local tracking branch)
    pub is_remote: bool,
}

impl BranchEntry {
    /// Build sorted branch entries from a repo's branches, worktrees, and active tmux sessions.
    ///
    /// Sorted by: sessions first, then worktrees, then alphabetical.
    pub fn build_sorted(
        repo: &crate::git::Repo,
        branch_names: &[String],
        active_sessions: &[String],
    ) -> Vec<Self> {
        use std::collections::HashMap;

        let wt_by_branch: HashMap<&str, &crate::git::Worktree> = repo
            .worktrees
            .iter()
            .filter_map(|wt| wt.branch.as_deref().map(|b| (b, wt)))
            .collect();

        let current_branch = repo.worktrees.first().and_then(|wt| wt.branch.as_deref());

        let mut entries: Vec<Self> = branch_names
            .iter()
            .map(|name| {
                let worktree_path = wt_by_branch.get(name.as_str()).map(|wt| wt.path.clone());
                let has_session = worktree_path
                    .as_ref()
                    .is_some_and(|p| active_sessions.contains(&repo.tmux_session_name(p)));
                let is_current = current_branch == Some(name.as_str());

                Self {
                    name: name.clone(),
                    worktree_path,
                    has_session,
                    is_current,
                    is_remote: false,
                }
            })
            .collect();

        Self::sort_entries(&mut entries);
        entries
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
                is_remote: true,
            })
            .collect()
    }

    pub(crate) fn sort_entries(entries: &mut [Self]) {
        entries.sort_by(|a, b| {
            // Remote branches always sort after local
            a.is_remote
                .cmp(&b.is_remote)
                .then(b.has_session.cmp(&a.has_session))
                .then(b.worktree_path.is_some().cmp(&a.worktree_path.is_some()))
                .then(a.name.cmp(&b.name))
        });
    }
}

/// What mode the app is in
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    RepoSelect,
    BranchSelect,
    NewBranchBase,
    /// Blocking loading state — shows spinner, no input except Ctrl+C
    Loading(String),
    /// Confirmation dialog for worktree deletion
    ConfirmDelete {
        branch_name: String,
        has_session: bool,
    },
    /// Help overlay showing key bindings
    Help {
        previous: Box<Mode>,
    },
}

/// The new-branch flow state
#[derive(Debug, Clone)]
pub struct NewBranchFlow {
    /// The new branch name (what the user typed)
    pub new_name: String,
    /// Base branches to pick from
    pub bases: Vec<String>,
    pub list: SearchableList,
}

/// Central application state. Components read from this, actions modify it.
#[derive(Debug, Clone)]
pub struct AppState {
    pub repos: Vec<Repo>,
    pub repo_list: SearchableList,

    pub selected_repo_idx: Option<usize>,
    pub branches: Vec<BranchEntry>,
    pub branch_list: SearchableList,

    pub new_branch_base: Option<NewBranchFlow>,

    pub split_command: Option<String>,
    pub mode: Mode,
    pub error: Option<String>,
}

impl AppState {
    pub fn new(repos: Vec<Repo>, split_command: Option<String>) -> Self {
        let repo_list = SearchableList::new(repos.len());
        Self {
            repos,
            repo_list,
            selected_repo_idx: None,
            branches: Vec::new(),
            branch_list: SearchableList::new(0),
            new_branch_base: None,
            split_command,
            mode: Mode::RepoSelect,
            error: None,
        }
    }

    pub fn new_loading(loading_message: &str, split_command: Option<String>) -> Self {
        Self {
            repos: Vec::new(),
            repo_list: SearchableList::new(0),
            selected_repo_idx: None,
            branches: Vec::new(),
            branch_list: SearchableList::new(0),
            new_branch_base: None,
            split_command,
            mode: Mode::Loading(loading_message.to_string()),
            error: None,
        }
    }

    /// Get the active searchable list for the current mode (mutable)
    pub fn active_list_mut(&mut self) -> Option<&mut SearchableList> {
        match self.mode {
            Mode::RepoSelect => Some(&mut self.repo_list),
            Mode::BranchSelect => Some(&mut self.branch_list),
            Mode::NewBranchBase => self.new_branch_base.as_mut().map(|f| &mut f.list),
            _ => None,
        }
    }

    /// Get the active searchable list for the current mode (immutable)
    pub fn active_list(&self) -> Option<&SearchableList> {
        match self.mode {
            Mode::RepoSelect => Some(&self.repo_list),
            Mode::BranchSelect => Some(&self.branch_list),
            Mode::NewBranchBase => self.new_branch_base.as_ref().map(|f| &f.list),
            _ => None,
        }
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
        assert!(result.unwrap_err().to_string().contains(&format!(
            "{WORKTREE_DIR_DEDUP_MAX_ATTEMPTS} attempts"
        )));
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

        // dev has session → first
        assert_eq!(entries[0].name, "dev");
        assert!(entries[0].has_session);
        assert!(entries[0].worktree_path.is_some());

        // main has worktree but no session → second
        assert_eq!(entries[1].name, "main");
        assert!(!entries[1].has_session);
        assert!(entries[1].worktree_path.is_some());
        assert!(entries[1].is_current);

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
        assert!(!entries[0].is_remote); // dev or main
        assert!(!entries[1].is_remote);
        assert!(entries[2].is_remote); // feature-a
        assert!(entries[3].is_remote); // feature-b
    }
}
