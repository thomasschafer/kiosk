//! Opening, creating and removing branch checkouts through whichever worktree support the
//! backend has. For [`Worktrees::Managed`] backends this is where kiosk does the git work.

use std::{fmt, path::Path};

use super::{Backend, KnownCheckout, Opened, RemoveError, Worktrees};
use crate::{
    event::WorktreeRemovalOutcome,
    git::{GitProvider, is_dirty_worktree_requires_force, local_branch_ref},
};

#[derive(Debug)]
pub enum OperationError<E> {
    Backend(E),
    Git(anyhow::Error),
    Placement(String),
}

impl<E: fmt::Display> fmt::Display for OperationError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(error) => write!(formatter, "{error}"),
            Self::Git(error) => write!(formatter, "{error:#}"),
            Self::Placement(message) => formatter.write_str(message),
        }
    }
}

/// Opens an existing local branch, creating a checkout for it if it has none.
pub(crate) fn open_branch<B: Backend>(
    git: &dyn GitProvider,
    backend: &B,
    repo: &Path,
    branch: &str,
    known: KnownCheckout,
) -> Result<Opened, OperationError<B::Error>> {
    match backend.worktrees() {
        Worktrees::Native(native) => native
            .open_branch(repo, branch, known)
            .map_err(OperationError::Backend),
        Worktrees::Managed(managed) => {
            // A fresh listing rather than `known`: the checkout may have been made or
            // removed since the picker listed branches.
            let worktrees = git.list_worktrees(repo).map_err(OperationError::Git)?;
            let path = if let Some(existing) = worktrees
                .iter()
                .find(|worktree| worktree.branch.as_deref() == Some(branch))
            {
                existing.path.clone()
            } else {
                let path = managed
                    .placement()
                    .checkout_path(repo, branch)
                    .map_err(OperationError::Placement)?;
                git.add_worktree(repo, &path, branch, None)
                    .map_err(OperationError::Git)?;
                path
            };
            backend
                .open_checkout(repo, &path)
                .map_err(OperationError::Backend)
        }
    }
}

/// Creates `branch` from local branch `base` in a new checkout and opens it.
pub(crate) fn create_branch<B: Backend>(
    git: &dyn GitProvider,
    backend: &B,
    repo: &Path,
    branch: &str,
    base: &str,
) -> Result<Opened, OperationError<B::Error>> {
    let base = &local_branch_ref(base);
    match backend.worktrees() {
        Worktrees::Native(native) => native
            .create_branch(repo, branch, base)
            .map_err(OperationError::Backend),
        Worktrees::Managed(managed) => {
            let path = managed
                .placement()
                .checkout_path(repo, branch)
                .map_err(OperationError::Placement)?;
            git.add_worktree(repo, &path, branch, Some(base))
                .map_err(OperationError::Git)?;
            backend
                .open_checkout(repo, &path)
                .map_err(OperationError::Backend)
        }
    }
}

/// Removes `checkout` of `repo`, closing the session showing it if there is one. The open
/// state is read again first, as the picker's copy can be stale.
///
/// When kiosk removes the checkout with git and then has a managed backend close its
/// session, `before_close` runs between the two. Nothing is left to resume once git has
/// removed the checkout, and closing may end the picker, so this is where the picker
/// forgets the pending record. If it cannot, `before_close` says why and the session
/// stays open. A native backend removes and closes in one call, so the record stays
/// until that call has returned.
pub(crate) fn remove_checkout<B: Backend>(
    git: &dyn GitProvider,
    backend: &B,
    backend_name: &str,
    repo: &Path,
    checkout: &Path,
    force: bool,
    before_close: impl FnOnce() -> Result<(), String>,
) -> WorktreeRemovalOutcome {
    let canonical_checkout = crate::path::canonical_or_original(checkout);
    let session = match backend.checkouts(repo) {
        Ok(checkouts) => checkouts
            .into_iter()
            .find(|state| {
                crate::path::equivalent(
                    &crate::path::canonical_or_original(&state.path),
                    &canonical_checkout,
                )
            })
            .map(|state| state.session),
        Err(error) => {
            return WorktreeRemovalOutcome::Failed(format!(
                "could not refresh open checkout state before removal: {error}"
            ));
        }
    };
    let Some(session) = session else {
        return WorktreeRemovalOutcome::Failed(format!(
            "could not confirm checkout state before removal because {} was not returned by {backend_name}",
            crate::path::display(checkout)
        ));
    };
    let Some(session) = session else {
        return match git.remove_worktree(repo, checkout, force) {
            Ok(()) => WorktreeRemovalOutcome::Removed {
                warning: git.prune_worktrees(repo).err().map(|error| {
                    format!("checkout was removed, but git worktree prune failed: {error:#}")
                }),
            },
            Err(error) => git_removal_failure(&error, force),
        };
    };
    match backend.worktrees() {
        Worktrees::Native(native) => match native.remove(&session, force) {
            Ok(removed) => WorktreeRemovalOutcome::Removed {
                warning: removed.warning,
            },
            Err(RemoveError::DirtyRequiresForce(_)) if !force => {
                WorktreeRemovalOutcome::DirtyRequiresForce
            }
            Err(RemoveError::DirtyRequiresForce(error) | RemoveError::Failed(error)) => {
                WorktreeRemovalOutcome::Failed(error.to_string())
            }
        },
        // The checkout goes first, so a refusal (uncommitted changes) leaves the session
        // as it was.
        Worktrees::Managed(managed) => match git.remove_worktree(repo, checkout, force) {
            Ok(()) => WorktreeRemovalOutcome::Removed {
                warning: match before_close() {
                    Ok(()) => managed.close(&session).err().map(|error| {
                        format!(
                            "checkout was removed, but it could not be closed in {backend_name}: {error}"
                        )
                    }),
                    Err(reason) => Some(format!(
                        "checkout was removed, but it was left open in {backend_name} because {reason}"
                    )),
                },
            },
            Err(error) => git_removal_failure(&error, force),
        },
    }
}

fn git_removal_failure(error: &anyhow::Error, force: bool) -> WorktreeRemovalOutcome {
    if !force && is_dirty_worktree_requires_force(error) {
        WorktreeRemovalOutcome::DirtyRequiresForce
    } else {
        WorktreeRemovalOutcome::Failed(format!("{error:#}"))
    }
}

#[cfg(test)]
mod tests;
