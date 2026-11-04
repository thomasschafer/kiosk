#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    MoveSelection(i32),
    /// Selects the item at this position of the list in front, as a pointer does.
    Select(usize),
    /// Scrolls the list in front by `delta` rows without moving the selection, unless it
    /// would leave the `viewport_rows` rows on screen.
    Scroll {
        delta: i32,
        viewport_rows: usize,
    },
    Insert(char),
    Backspace,
    DeleteWord,
    CursorLeft,
    CursorRight,
    ClearQuery,
    OpenRepo,
    OpenBranches,
    OpenBranch,
    StartNewBranch,
    CreateNewBranch,
    ResolveRemoteBranch,
    DeleteWorktree,
    ConfirmDeleteWorktree,
    CancelOverlay,
    BackToRepos,
    DismissToast,
    ShowHelp,
    ShowScanWarnings,
    CloseOverlay,
    Noop,
}
