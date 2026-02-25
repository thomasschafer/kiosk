use std::{fs, path::PathBuf};

/// Expand `~` to home directory for filesystem operations
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    } else if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Split input into (`parent_dir`, prefix).
/// e.g. `~/Dev` → `("~/", "Dev")`, `test` → `("./", "test")`
pub fn split_input(input: &str) -> (String, String) {
    if let Some(last_slash) = input.rfind('/') {
        let parent = &input[..=last_slash];
        let prefix = &input[last_slash + 1..];
        (parent.to_string(), prefix.to_string())
    } else {
        ("./".to_string(), input.to_string())
    }
}

/// Join parent directory with a name, preserving `~/` display.
fn join_path(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// Generate filesystem completions for the given input.
/// Directories only, prefix-matched (case-insensitive), hidden dirs skipped
/// unless prefix starts with `.`.
pub fn complete(input: &str) -> Vec<String> {
    if input.is_empty() {
        return Vec::new();
    }

    let (parent_dir, prefix) = split_input(input);
    let expanded_parent = expand_tilde(&parent_dir);

    let Ok(entries) = fs::read_dir(&expanded_parent) else {
        return Vec::new();
    };

    let prefix_lower = prefix.to_lowercase();
    let mut completions: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && !prefix.starts_with('.') {
                return None;
            }
            if name.to_lowercase().starts_with(&prefix_lower) {
                Some(join_path(&parent_dir, &name))
            } else {
                None
            }
        })
        .collect();

    completions.sort();
    completions
}

/// Find the longest common prefix of all completions.
pub fn common_prefix(completions: &[String]) -> String {
    if completions.is_empty() {
        return String::new();
    }
    if completions.len() == 1 {
        return completions[0].clone();
    }

    let first = &completions[0];
    let mut prefix_chars = first.chars().count();
    for other in &completions[1..] {
        prefix_chars = prefix_chars.min(other.chars().count());
        for (i, (a, b)) in first.chars().zip(other.chars()).enumerate() {
            if a != b {
                prefix_chars = prefix_chars.min(i);
                break;
            }
        }
    }
    first.chars().take(prefix_chars).collect()
}

/// Check if a path exists (expanding `~` if needed).
pub fn path_exists(path: &str) -> bool {
    expand_tilde(path).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_input_tilde() {
        assert_eq!(split_input("~/Dev"), ("~/".to_string(), "Dev".to_string()));
    }

    #[test]
    fn test_split_input_tilde_slash() {
        assert_eq!(split_input("~/"), ("~/".to_string(), String::new()));
    }

    #[test]
    fn test_split_input_bare() {
        assert_eq!(split_input("test"), ("./".to_string(), "test".to_string()));
    }

    #[test]
    fn test_split_input_absolute() {
        assert_eq!(
            split_input("/usr/local/bin"),
            ("/usr/local/".to_string(), "bin".to_string())
        );
    }

    #[test]
    fn test_expand_tilde_absolute() {
        assert_eq!(
            expand_tilde("/absolute/path"),
            PathBuf::from("/absolute/path")
        );
    }

    #[test]
    fn test_expand_tilde_with_rest() {
        let expanded = expand_tilde("~/test");
        assert!(expanded.to_string_lossy().contains("test"));
        assert!(!expanded.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn test_common_prefix_empty() {
        assert_eq!(common_prefix(&[]), "");
    }

    #[test]
    fn test_common_prefix_single() {
        assert_eq!(common_prefix(&["~/Projects".to_string()]), "~/Projects");
    }

    #[test]
    fn test_common_prefix_shared() {
        assert_eq!(
            common_prefix(&["~/Dev".to_string(), "~/Development".to_string()]),
            "~/Dev"
        );
    }

    #[test]
    fn test_common_prefix_divergent() {
        assert_eq!(
            common_prefix(&["~/Dev".to_string(), "~/Work".to_string()]),
            "~/"
        );
    }

    #[test]
    fn test_common_prefix_multibyte() {
        assert_eq!(
            common_prefix(&["~/Üntersuchung".to_string(), "~/Über".to_string()]),
            "~/Ü"
        );
    }

    #[test]
    fn test_split_input_empty() {
        assert_eq!(split_input(""), ("./".to_string(), String::new()));
    }

    #[test]
    fn test_complete_empty_input() {
        assert!(complete("").is_empty());
    }

    #[test]
    fn test_join_path_trailing_slash() {
        assert_eq!(join_path("~/", "Dev"), "~/Dev");
    }

    #[test]
    fn test_join_path_no_trailing_slash() {
        assert_eq!(join_path("~", "Dev"), "~/Dev");
    }

    #[test]
    fn test_complete_nonexistent_parent() {
        let result = complete("/nonexistent_path_xyz_12345/foo");
        assert!(result.is_empty());
    }

    #[test]
    fn test_complete_tmp() {
        // /tmp should exist on any unix system
        let result = complete("/tmp/");
        // We can't assert specific contents but it shouldn't panic
        for item in &result {
            assert!(item.starts_with("/tmp/"));
        }
    }

    #[test]
    fn test_complete_with_temp_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("Desktop")).unwrap();
        std::fs::create_dir(tmp.path().join("Development")).unwrap();
        std::fs::create_dir(tmp.path().join("Documents")).unwrap();

        let input = format!("{}/De", tmp.path().display());
        let results = complete(&input);

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.contains("Desktop")));
        assert!(results.iter().any(|r| r.contains("Development")));
    }

    #[test]
    fn test_complete_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("MyDir")).unwrap();

        let input = format!("{}/my", tmp.path().display());
        let results = complete(&input);
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("MyDir"));
    }

    #[test]
    fn test_complete_hides_dotfiles_unless_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".hidden")).unwrap();
        std::fs::create_dir(tmp.path().join("visible")).unwrap();

        let no_dot = complete(&format!("{}/", tmp.path().display()));
        assert_eq!(no_dot.len(), 1);
        assert!(no_dot[0].contains("visible"));

        let with_dot = complete(&format!("{}/.h", tmp.path().display()));
        assert_eq!(with_dot.len(), 1);
        assert!(with_dot[0].contains(".hidden"));
    }

    #[test]
    fn test_complete_only_dirs_not_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("subdir")).unwrap();
        std::fs::write(tmp.path().join("subfile.txt"), "data").unwrap();

        let results = complete(&format!("{}/sub", tmp.path().display()));
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("subdir"));
    }

    #[cfg(unix)]
    #[test]
    fn test_complete_follows_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("real_dir");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, tmp.path().join("link_dir")).unwrap();

        let results = complete(&format!("{}/", tmp.path().display()));
        assert!(
            results.iter().any(|r| r.contains("link_dir")),
            "Symlinked directory should appear in completions: {results:?}"
        );
        assert!(results.iter().any(|r| r.contains("real_dir")));
    }

    #[cfg(unix)]
    #[test]
    fn test_complete_ignores_broken_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/nonexistent_target_xyz", tmp.path().join("broken")).unwrap();
        std::fs::create_dir(tmp.path().join("valid")).unwrap();

        let results = complete(&format!("{}/", tmp.path().display()));
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("valid"));
    }

    #[test]
    fn test_complete_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("charlie")).unwrap();
        std::fs::create_dir(tmp.path().join("alpha")).unwrap();
        std::fs::create_dir(tmp.path().join("bravo")).unwrap();

        let results = complete(&format!("{}/", tmp.path().display()));
        assert_eq!(results.len(), 3);
        assert!(results[0].contains("alpha"));
        assert!(results[1].contains("bravo"));
        assert!(results[2].contains("charlie"));
    }
}
