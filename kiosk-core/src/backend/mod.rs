//! The contract between the picker and the multiplexer it opens checkouts in.
//!
//! A backend shows checkouts and folders in sessions of its own kind (herdr workspaces,
//! tmux sessions, Rex windows, ...). Every backend reports what is open and can open and
//! focus things. Worktree lifecycle is where backends differ, so it is chosen by type
//! through [`Backend::worktrees`]: a backend either manages checkouts natively
//! ([`NativeWorktrees`]) or lets kiosk manage them with git ([`ManagedWorktrees`]), in
//! which case it only has to say where new checkouts go and how to close a session.

use std::{
    fmt,
    path::{Path, PathBuf},
};

pub mod annotation;
pub mod identity;
#[cfg(any(test, feature = "test-support"))]
pub mod mock;
pub mod placement;
pub(crate) mod worktree;

pub use annotation::{Annotations, Annotator, Label, Tone};
pub use placement::WorktreePlacement;

pub trait Backend: Send + Sync + 'static {
    /// How the backend identifies a session that shows a checkout or folder.
    type SessionId: Clone + PartialEq + fmt::Debug + Send + Sync + 'static;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Main checkouts of the repositories that have something open.
    fn open_repos(&self) -> Result<Vec<PathBuf>, Self::Error>;
    /// Folders that sessions are currently in, for marking plain folders as open.
    fn open_folders(&self) -> Result<Vec<PathBuf>, Self::Error>;
    /// Every checkout of `repo` the backend knows about, with the session showing it.
    fn checkouts(&self, repo: &Path) -> Result<Vec<CheckoutState<Self::SessionId>>, Self::Error>;
    /// Focuses the session showing `checkout`, a checkout of `repo`, or opens one.
    fn open_checkout(&self, repo: &Path, checkout: &Path) -> Result<Opened, Self::Error>;
    /// Focuses the session in `folder`, a folder outside any repository, or opens one.
    fn open_folder(&self, folder: &Path) -> Result<Opened, Self::Error>;
    /// Tells the user something after the picker has closed.
    fn notify(&self, title: &str, body: &str) -> Result<(), Self::Error>;
    fn worktrees(&self) -> Worktrees<'_, Self>;
    /// Labels for open checkouts and folders, if the backend has any to show.
    fn annotator(&self) -> Option<&dyn Annotator<Self>> {
        None
    }
}

/// How a backend's worktrees are created and removed.
pub enum Worktrees<'a, B: Backend + ?Sized> {
    /// The backend creates, opens and removes worktrees itself.
    Native(&'a dyn NativeWorktrees<B>),
    /// Kiosk creates and removes worktrees with git, and asks the backend to show and
    /// close them.
    Managed(&'a dyn ManagedWorktrees<B>),
}

/// Worktree operations of a backend that manages checkouts itself.
pub trait NativeWorktrees<B: Backend + ?Sized>: Send + Sync {
    /// Opens an existing local branch, creating its checkout if it has none. `known` is what
    /// the picker last saw, which can be stale.
    fn open_branch(
        &self,
        repo: &Path,
        branch: &str,
        known: KnownCheckout,
    ) -> Result<Opened, B::Error>;
    /// Creates `branch` from `base`, a full ref such as `refs/heads/main`, in a new checkout
    /// and opens it.
    fn create_branch(&self, repo: &Path, branch: &str, base: &str) -> Result<Opened, B::Error>;
    /// Removes the checkout `session` shows and closes the session.
    fn remove(&self, session: &B::SessionId, force: bool)
    -> Result<Removed, RemoveError<B::Error>>;
}

/// What a backend provides so kiosk can manage its checkouts with git.
pub trait ManagedWorktrees<B: Backend + ?Sized>: Send + Sync {
    /// Where new checkouts go.
    fn placement(&self) -> &WorktreePlacement;
    /// Closes a session whose checkout kiosk has removed.
    fn close(&self, session: &B::SessionId) -> Result<(), B::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutState<S> {
    pub path: PathBuf,
    /// The session showing the checkout, if it is open.
    pub session: Option<S>,
}

/// Whether the picker's last listing showed a checkout for a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownCheckout {
    Exists,
    Missing,
}

/// A checkout or folder is showing. `warning` reports anything that went wrong on the way
/// that did not stop it opening.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opened {
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removed {
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveError<E> {
    /// The checkout has uncommitted changes; removing it needs `force`.
    DirtyRequiresForce(E),
    Failed(E),
}
