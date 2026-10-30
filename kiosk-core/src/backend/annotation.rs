//! Short labels a backend shows beside open checkouts and folders, such as what a program
//! running in them is doing. A backend that has nothing to say leaves
//! [`super::Backend::annotator`] at its default, and the picker shows nothing extra.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use super::Backend;

/// One label, such as `claude waiting`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub text: String,
    pub tone: Tone,
}

/// How a label is coloured, by what it means rather than by colour, so the picker's theme
/// decides how it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Progress,
    Attention,
    Success,
    Failure,
    Muted,
}

/// Labels by where the picker shows them. Paths need not be canonical.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Annotations {
    /// Rows of the repository view, by a repository's main checkout or by a plain folder.
    pub repos: Vec<(PathBuf, Vec<Label>)>,
    /// Rows of the branch view, by checkout.
    pub checkouts: Vec<(PathBuf, Vec<Label>)>,
}

/// What a backend provides to label open checkouts and folders. The picker asks again
/// every [`Annotator::interval`] while it is open.
pub trait Annotator<B: Backend + ?Sized>: Send + Sync {
    fn interval(&self) -> Duration;
    fn annotations(&self) -> Result<Annotations, B::Error>;
}

/// The latest labels, keyed by canonical path for lookup while drawing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnotationIndex {
    repos: HashMap<PathBuf, Vec<Label>>,
    checkouts: HashMap<PathBuf, Vec<Label>>,
}

impl AnnotationIndex {
    /// Canonicalizes every path, which touches the filesystem, so build it off the UI
    /// thread.
    pub fn new(annotations: Annotations) -> Self {
        let index = |entries: Vec<(PathBuf, Vec<Label>)>| {
            let mut map: HashMap<PathBuf, Vec<Label>> = HashMap::new();
            for (path, labels) in entries {
                if labels.is_empty() {
                    continue;
                }
                map.entry(crate::path::canonical_or_original(&path))
                    .or_default()
                    .extend(labels);
            }
            map
        };
        Self {
            repos: index(annotations.repos),
            checkouts: index(annotations.checkouts),
        }
    }

    pub fn repo(&self, path: &Path) -> &[Label] {
        lookup(&self.repos, path)
    }

    pub fn checkout(&self, path: &Path) -> &[Label] {
        lookup(&self.checkouts, path)
    }

    pub fn is_empty(&self) -> bool {
        self.repos.is_empty() && self.checkouts.is_empty()
    }
}

fn lookup<'a>(map: &'a HashMap<PathBuf, Vec<Label>>, path: &Path) -> &'a [Label] {
    if map.is_empty() {
        return &[];
    }
    map.get(path)
        .or_else(|| map.get(&crate::path::canonical_or_original(path)))
        .map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(text: &str) -> Label {
        Label {
            text: text.into(),
            tone: Tone::Neutral,
        }
    }

    #[test]
    fn labels_are_found_by_equivalent_paths_and_merged_per_path() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let index = AnnotationIndex::new(Annotations {
            repos: vec![(dir.path().into(), vec![])],
            checkouts: vec![
                (checkout.join("."), vec![label("a")]),
                (checkout.clone(), vec![label("b")]),
            ],
        });
        assert_eq!(index.checkout(&checkout), [label("a"), label("b")]);
        assert_eq!(index.repo(dir.path()), []);
        assert_eq!(index.checkout(&dir.path().join("missing")), []);
    }
}
