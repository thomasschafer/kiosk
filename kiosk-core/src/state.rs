use crate::git::Repo;
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
    ConfirmDelete(String),
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
    let worktree_root = parent.join(".kiosk_worktrees");
    let safe_branch = branch.replace('/', "-");
    let base = format!("{}--{safe_branch}", repo.name);
    let candidate = worktree_root.join(&base);
    if !candidate.exists() {
        return Ok(candidate);
    }
    for i in 2..1000 {
        let candidate = worktree_root.join(format!("{base}-{i}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("Could not find an available worktree directory name after 1000 attempts")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repo;
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
            tmp.path().join(".kiosk_worktrees").join("myrepo--main")
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
                .join(".kiosk_worktrees")
                .join("repo--feat-awesome")
        );
    }

    #[test]
    fn test_worktree_dir_dedup() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let first = tmp.path().join(".kiosk_worktrees").join("repo--main");
        fs::create_dir_all(&first).unwrap();
        let result = worktree_dir(&repo, "main").unwrap();
        assert_eq!(
            result,
            tmp.path().join(".kiosk_worktrees").join("repo--main-2")
        );
    }

    #[test]
    fn test_worktree_dir_bounded_error() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "repo");
        let wt_root = tmp.path().join(".kiosk_worktrees");
        // Create the base and 2..999 suffixed dirs to exhaust the loop
        fs::create_dir_all(wt_root.join("repo--main")).unwrap();
        for i in 2..1000 {
            fs::create_dir_all(wt_root.join(format!("repo--main-{i}"))).unwrap();
        }
        let result = worktree_dir(&repo, "main");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("1000 attempts"));
    }

    #[test]
    fn test_worktree_dir_in_kiosk_worktrees_subdir() {
        let tmp = tempdir().unwrap();
        let repo = make_repo(tmp.path(), "myrepo");
        let result = worktree_dir(&repo, "dev").unwrap();
        assert!(result.to_string_lossy().contains(".kiosk_worktrees"));
    }
}
