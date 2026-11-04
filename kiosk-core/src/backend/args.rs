//! Checks on what a backend passes to the program it drives, each failing with a message
//! the backend wraps in its own error.

use std::path::Path;

/// `path`, which must be absolute; `name` says what it is in the message.
pub fn absolute(name: &str, path: &Path) -> Result<(), String> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(format!("{name} must be absolute: {}", path.display()))
    }
}

/// `value`, which must have more than whitespace in it.
pub fn nonempty(name: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{name} must not be empty"))
    } else {
        Ok(())
    }
}

/// `path` as text for a program's command line, which needs it to be valid UTF-8.
pub fn utf8_path(name: &str, path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} is not valid UTF-8 and cannot be passed on"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_checked_with_their_name_in_the_message() {
        assert!(absolute("folder", &std::env::temp_dir()).is_ok());
        assert_eq!(
            absolute("folder", Path::new("repo")).unwrap_err(),
            "folder must be absolute: repo"
        );
        assert!(nonempty("command", "hx").is_ok());
        assert_eq!(
            nonempty("command", "  ").unwrap_err(),
            "command must not be empty"
        );
        assert_eq!(utf8_path("cwd", Path::new("/repo")).unwrap(), "/repo");
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/repo-\xff"));
        assert!(
            utf8_path("cwd", path)
                .unwrap_err()
                .contains("not valid UTF-8")
        );
    }
}
