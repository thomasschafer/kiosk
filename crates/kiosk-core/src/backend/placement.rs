use std::path::{Path, PathBuf};

const DEFAULT_SLUG: &str = "worktree";
/// How many numbered folders `checkout_path` tries before giving up.
const MAX_SUFFIX: u32 = 100;

/// Where kiosk puts the checkouts it creates: `<root>/<repo folder name>/<branch slug>`,
/// the layout herdr uses for its own worktrees, with a numbered suffix when that folder is
/// taken.
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

    /// A free path for a new checkout of `branch` of the repository whose main checkout is
    /// `repo`. Different branches can share a slug (`feat/x` and `feat-x`), as can
    /// repositories with the same folder name, so a taken folder gets the first free
    /// numbered suffix (`feat-x-2`). Callers reuse a branch's existing checkout rather than
    /// asking for a path.
    pub fn checkout_path(&self, repo: &Path, branch: &str) -> Result<PathBuf, String> {
        let repo_name = repo.file_name().ok_or_else(|| {
            format!(
                "repository {} has no folder name",
                crate::path::display(repo)
            )
        })?;
        let parent = self.root.join(repo_name);
        let slug = branch_slug(branch);
        for suffix in 1..=MAX_SUFFIX {
            let candidate = if suffix == 1 {
                parent.join(&slug)
            } else {
                parent.join(format!("{slug}-{suffix}"))
            };
            match candidate.symlink_metadata() {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(candidate);
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(format!(
                        "could not check whether {} is free: {error}",
                        crate::path::display(&candidate)
                    ));
                }
            }
        }
        Err(format!(
            "no free checkout folder for {branch} in {} after {MAX_SUFFIX} tries",
            crate::path::display(&parent)
        ))
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
    fn taken_checkout_folders_get_a_numbered_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let placement = WorktreePlacement::new(temp.path().join("trees")).unwrap();
        let repo = temp.path().join("one/app");
        let first = placement.checkout_path(&repo, "feat/x").unwrap();
        std::fs::create_dir_all(&first).unwrap();

        // `feat-x` slugs like `feat/x`, and another repo named `app` shares its folder.
        let second = placement.checkout_path(&repo, "feat-x").unwrap();
        assert_eq!(second, temp.path().join("trees/app/feat-x-2"));
        std::fs::create_dir_all(&second).unwrap();
        assert_eq!(
            placement
                .checkout_path(&temp.path().join("two/app"), "feat/x")
                .unwrap(),
            temp.path().join("trees/app/feat-x-3")
        );
    }

    #[test]
    fn relative_roots_are_refused() {
        assert!(WorktreePlacement::new("relative".into()).is_err());
    }
}
