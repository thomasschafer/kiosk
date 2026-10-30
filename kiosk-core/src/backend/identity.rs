//! Matching sessions to checkouts and folders by the folder each session is in, for
//! backends that have no identity of their own to match by.

use std::path::{Path, PathBuf};

use crate::path::{canonical_or_original, equivalent, starts_with};

/// A session and the folder it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFolder<S> {
    pub session: S,
    pub folder: PathBuf,
}

/// The first session whose folder is `folder` itself.
pub fn session_in_folder<'a, S>(live: &'a [LiveFolder<S>], folder: &Path) -> Option<&'a S> {
    let folder = canonical_or_original(folder);
    live.iter()
        .find(|entry| equivalent(&canonical_or_original(&entry.folder), &folder))
        .map(|entry| &entry.session)
}

/// The first session showing `checkout`: its folder is inside the checkout and not inside
/// another of the repository's `checkouts` nested below it.
pub fn session_showing_checkout<'a, S>(
    live: &'a [LiveFolder<S>],
    checkout: &Path,
    checkouts: &[PathBuf],
) -> Option<&'a S> {
    let checkout = canonical_or_original(checkout);
    let deeper: Vec<PathBuf> = checkouts
        .iter()
        .map(|path| canonical_or_original(path))
        .filter(|path| !equivalent(path, &checkout) && starts_with(path, &checkout))
        .collect();
    live.iter()
        .find(|entry| {
            let folder = canonical_or_original(&entry.folder);
            starts_with(&folder, &checkout) && !deeper.iter().any(|path| starts_with(&folder, path))
        })
        .map(|entry| &entry.session)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(entries: &[(&str, &str)]) -> Vec<LiveFolder<String>> {
        entries
            .iter()
            .map(|(session, folder)| LiveFolder {
                session: (*session).into(),
                folder: (*folder).into(),
            })
            .collect()
    }

    #[test]
    fn folder_sessions_match_the_folder_exactly() {
        let live = live(&[("a", "/notes/sub"), ("b", "/notes")]);
        assert_eq!(
            session_in_folder(&live, Path::new("/notes")),
            Some(&"b".into())
        );
        assert_eq!(session_in_folder(&live, Path::new("/other")), None);
    }

    #[test]
    fn a_checkout_is_shown_by_a_session_inside_it_but_not_inside_a_nested_checkout() {
        let checkouts = [PathBuf::from("/repo"), PathBuf::from("/repo/.worktrees/x")];
        let nested_only = live(&[("x", "/repo/.worktrees/x/src")]);
        assert_eq!(
            session_showing_checkout(&nested_only, Path::new("/repo"), &checkouts),
            None
        );
        assert_eq!(
            session_showing_checkout(&nested_only, Path::new("/repo/.worktrees/x"), &checkouts),
            Some(&"x".into())
        );
        let main = live(&[("x", "/repo/.worktrees/x"), ("m", "/repo/src")]);
        assert_eq!(
            session_showing_checkout(&main, Path::new("/repo"), &checkouts),
            Some(&"m".into())
        );
    }
}
