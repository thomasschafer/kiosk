use std::path::PathBuf;

/// Every user interaction produces an Action. UI never directly calls git/tmux.
#[derive(Debug, Clone)]
pub enum Action {
    // Navigation
    SelectRepo(usize),
    EnterRepo,
    SelectBranch(usize),
    OpenBranch,
    GoBack,
    Quit,

    // Search
    SearchPush(char),
    SearchPop,
    MoveSelection(i32),

    // Git
    CreateWorktree { branch: String },
    CreateBranchAndWorktree { new_branch: String, base: String },

    // Tmux
    OpenSession { path: PathBuf },

    // UI
    StartNewBranchFlow,
    ShowError(String),
    ClearError,
}
