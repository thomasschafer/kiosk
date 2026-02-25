use std::path::PathBuf;

/// Expand a leading `~` to the user's home directory.
///
/// Returns `None` when the path starts with `~` but the home directory
/// cannot be determined (e.g. sandboxed environments). Non-tilde paths
/// are always returned as-is.
pub fn expand_tilde(path: &str) -> Option<PathBuf> {
    if path == "~" {
        dirs::home_dir()
    } else if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir().map(|home| home.join(rest))
    } else {
        Some(PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_path_unchanged() {
        assert_eq!(
            expand_tilde("/absolute/path"),
            Some(PathBuf::from("/absolute/path"))
        );
    }

    #[test]
    fn relative_path_unchanged() {
        assert_eq!(expand_tilde("relative"), Some(PathBuf::from("relative")));
    }

    #[test]
    fn tilde_alone_expands_to_home() {
        let result = expand_tilde("~").expect("home dir should exist in test env");
        assert!(!result.to_string_lossy().contains('~'));
    }

    #[test]
    fn tilde_with_rest_expands() {
        let result = expand_tilde("~/test").expect("home dir should exist in test env");
        assert!(result.to_string_lossy().ends_with("test"));
        assert!(!result.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn tilde_in_middle_not_expanded() {
        assert_eq!(
            expand_tilde("/some/~/path"),
            Some(PathBuf::from("/some/~/path"))
        );
    }
}
