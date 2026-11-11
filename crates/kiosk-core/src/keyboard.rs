use crate::state::TextEdit;

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
    /// Edits the search in front, or moves its cursor.
    Edit(TextEdit),
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
    /// Keeps what was typed into the field in front and leaves it.
    Accept,
    Noop,
}
