/// Every user interaction produces an Action. UI never directly calls git/tmux.
#[derive(Debug, Clone)]
pub enum Action {
    // Navigation
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
    ShowError(String),
    ClearError,
}
