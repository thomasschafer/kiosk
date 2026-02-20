/// Every user interaction produces an Action. UI never directly calls git/tmux.
#[derive(Debug, Clone)]
pub enum Action {
    // Navigation
    OpenRepo,
    EnterRepo,
    OpenBranch,
    GoBack,
    Quit,

    // Search
    SearchPush(char),
    SearchPop,
    MoveSelection(i32),

    // UI
    StartNewBranchFlow,
    DeleteWorktree,
    ConfirmDeleteWorktree,
    CancelDeleteWorktree,
    ShowError(String),
    ClearError,
}
