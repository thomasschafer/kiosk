use std::path::{Path, PathBuf};

const DEFAULT_SLUG: &str = "worktree";

/// Where kiosk puts the checkouts it creates: `<root>/<repo folder name>/<branch slug>`,
/// the layout herdr uses for its own worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreePlacement {
    root: PathBuf,
}

impl WorktreePlacement {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        if root.is_absolute() {
            Ok(Self { root })
        } else {
            Err(format!(
                "the worktree directory must be absolute: {}",
                crate::path::display(&root)
            ))
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The checkout path for `branch` of the repository whose main checkout is `repo`.
    pub fn checkout_path(&self, repo: &Path, branch: &str) -> Result<PathBuf, String> {
        let repo_name = repo.file_name().ok_or_else(|| {
            format!(
                "repository {} has no folder name",
                crate::path::display(repo)
            )
        })?;
        Ok(self.root.join(repo_name).join(branch_slug(branch)))
    }
}

/// The folder name for a branch's checkout: lowercase ASCII letters and digits, with every
/// other run of characters replaced by one dash.
pub fn branch_slug(branch: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for character in branch.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        DEFAULT_SLUG.into()
    } else {
        slug.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_match_herdr_checkout_folder_names() {
        assert_eq!(branch_slug("feature/Login-Page"), "feature-login-page");
        assert_eq!(branch_slug("--fix__bug--"), "fix-bug");
        assert_eq!(branch_slug("日本"), DEFAULT_SLUG);
    }

    #[test]
    fn checkout_paths_nest_under_the_repository_name() {
        let root = std::env::temp_dir().join("worktrees");
        let placement = WorktreePlacement::new(root.clone()).unwrap();
        assert_eq!(
            placement
                .checkout_path(&std::env::temp_dir().join("repo"), "feat/x")
                .unwrap(),
            root.join("repo").join("feat-x")
        );
    }

    #[test]
    fn relative_roots_are_refused() {
        assert!(WorktreePlacement::new("relative".into()).is_err());
    }
}
