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
    SearchDeleteWord,

    // Movement
    MoveSelection(i32),
    HalfPageUp,
    HalfPageDown,
    PageUp,
    PageDown,
    MoveTop,
    MoveBottom,

    // Cursor movement (for search input)
    CursorLeft,
    CursorRight,
    CursorStart,
    CursorEnd,

    // UI
    StartNewBranchFlow,
    DeleteWorktree,
    ConfirmDeleteWorktree,
    CancelDeleteWorktree,
    ShowHelp,
}
