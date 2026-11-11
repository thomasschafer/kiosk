//! The `[editor]` section: how a file is opened at a line, in a new pane beside the
//! checkout's.

use std::path::Path;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::on_open::OnOpenPaneDirection;

const FILE: &str = "{file}";
const LINE: &str = "{line}";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// The editor files open in from the diff tab, in a new pane beside the checkout's.
pub struct EditorConfig {
    /// Shell command typed into the new pane, which starts in the checkout. `{file}` is
    /// replaced with the file's path, quoted for the shell, and `{line}` with the line's
    /// number, such as `hx {file}:{line}` or `nvim +{line} {file}`. It must contain
    /// `{file}`.
    pub command: String,
    /// Split direction: `right` or `down`. Defaults to `right`.
    #[serde(default = "right")]
    pub direction: OnOpenPaneDirection,
}

const fn right() -> OnOpenPaneDirection {
    OnOpenPaneDirection::Right
}

impl EditorConfig {
    pub fn validate(&self) -> Result<()> {
        if !self.command.contains(FILE) {
            bail!("[editor] command must contain {FILE}, where the file's path goes");
        }
        if quoted_placeholder(&self.command) {
            // Inside the command's own quotes, the path's quoting would be part of the path,
            // and a shell would expand what the path holds, such as `$(...)`.
            bail!("[editor] command must leave {FILE} unquoted: kiosk quotes the path itself");
        }
        Ok(())
    }

    /// The command that opens `file` at `line`.
    pub fn command_for(&self, file: &Path, line: u32) -> Result<String> {
        let file = shell_word(file)?;
        // The line goes in first, so a `{line}` in the path stays as it is.
        Ok(self
            .command
            .replace(LINE, &line.to_string())
            .replace(FILE, &file))
    }
}

/// Whether `{file}` is inside quotes in `command`, as a POSIX shell, fish, cmd or PowerShell
/// would read them: within `'...'` or `"..."`, with a backslash escaping the next character
/// outside single quotes.
fn quoted_placeholder(command: &str) -> bool {
    let mut quote = None;
    let mut characters = command.char_indices();
    while let Some((index, character)) = characters.next() {
        if quote.is_some() && command[index..].starts_with(FILE) {
            return true;
        }
        match (quote, character) {
            (None | Some('"'), '\\') => {
                characters.next();
            }
            (None, '\'' | '"') => quote = Some(character),
            (Some(open), _) if open == character => quote = None,
            _ => {}
        }
    }
    false
}

/// `path` as one word for the shell the pane runs, quoted where it needs to be. Paths the
/// quoting can't carry the same way in every shell are refused rather than guessed at.
fn shell_word(path: &Path) -> Result<String> {
    let Some(text) = path.to_str() else {
        bail!("{} isn't valid UTF-8, so it can't be typed", path.display());
    };
    if text.chars().any(char::is_control) {
        bail!("{text:?} has control characters, so it can't be typed");
    }
    let plain = |character: char| {
        character.is_alphanumeric()
            || "/._-+:@=".contains(character)
            || (cfg!(windows) && character == '\\')
    };
    if text.chars().all(plain) {
        return Ok(text.to_owned());
    }
    // POSIX shells and fish take everything between single quotes as it is, except that
    // fish reads a backslash before a quote or another backslash as an escape. On Windows,
    // cmd and PowerShell both take double quotes, though PowerShell expands `$` and "`".
    let (quote, unquotable): (char, &[char]) = if cfg!(windows) {
        ('"', &['"', '$', '`', '%'])
    } else {
        ('\'', &['\'', '\\'])
    };
    if let Some(character) = text
        .chars()
        .find(|character| unquotable.contains(character))
    {
        bail!("{text:?} has {character:?} in it, which can't be quoted for every shell");
    }
    Ok(format!("{quote}{text}{quote}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(command: &str) -> EditorConfig {
        EditorConfig {
            command: command.into(),
            direction: OnOpenPaneDirection::Right,
        }
    }

    #[test]
    fn parses_with_the_direction_defaulting_to_right() {
        let config: EditorConfig = toml::from_str(r#"command = "hx {file}:{line}""#).unwrap();
        assert_eq!(config, editor("hx {file}:{line}"));
        let config: EditorConfig =
            toml::from_str("command = \"nvim {file}\"\ndirection = \"down\"").unwrap();
        assert_eq!(config.direction, OnOpenPaneDirection::Down);
    }

    #[test]
    fn the_command_must_say_where_the_file_goes_and_leave_it_unquoted() {
        for command in [
            "hx {file}",
            r#"sh -c 'e "$0"' {file}"#,
            r#"e "a\"b" {file}"#,
            r"e \' {file}",
        ] {
            editor(command)
                .validate()
                .unwrap_or_else(|error| panic!("{command}: {error}"));
        }
        let error = editor("hx").validate().unwrap_err().to_string();
        assert!(error.contains("{file}"), "{error}");
        for command in [
            r#"nvim "{file}""#,
            "nvim '{file}'",
            r#"e "+{line} {file}:x""#,
        ] {
            let error = editor(command).validate().unwrap_err().to_string();
            assert!(error.contains("unquoted"), "{command}: {error}");
        }
    }

    #[test]
    fn fills_in_the_file_and_line() {
        let config = editor("hx {file}:{line} # {line}");
        assert_eq!(
            config
                .command_for(Path::new("/repo/src/main.rs"), 12)
                .unwrap(),
            "hx /repo/src/main.rs:12 # 12"
        );
        assert_eq!(
            editor("${EDITOR} +{line} {file}")
                .command_for(Path::new("/repo/a"), 3)
                .unwrap(),
            "${EDITOR} +3 /repo/a",
            "braces that aren't placeholders stay"
        );
        let quote = if cfg!(windows) { '"' } else { '\'' };
        assert_eq!(
            editor("e {file}")
                .command_for(Path::new("/repo/{line}"), 3)
                .unwrap(),
            format!("e {quote}/repo/{{line}}{quote}")
        );
    }

    #[cfg(unix)]
    #[test]
    fn quotes_paths_the_shell_would_split_and_refuses_what_it_cannot_quote() {
        use std::os::unix::ffi::OsStrExt;
        let config = editor("e {file}");
        let command = |path: &str| config.command_for(Path::new(path), 1);
        assert_eq!(command("/my repo/$x*.rs").unwrap(), "e '/my repo/$x*.rs'");
        assert_eq!(command("/repo/naïve.rs").unwrap(), "e /repo/naïve.rs");
        for path in ["/repo/it's", "/repo/back\\slash", "/repo/new\nline"] {
            assert!(command(path).is_err(), "{path:?} was typed");
        }
        let invalid = std::ffi::OsStr::from_bytes(b"/repo/\xff");
        assert!(config.command_for(Path::new(invalid), 1).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn quotes_paths_the_shell_would_split_and_refuses_what_it_cannot_quote() {
        let config = editor("e {file}");
        let command = |path: &str| config.command_for(Path::new(path), 1);
        assert_eq!(command(r"C:\repo\main.rs").unwrap(), r"e C:\repo\main.rs");
        assert_eq!(
            command(r"C:\my repo\a.rs").unwrap(),
            r#"e "C:\my repo\a.rs""#
        );
        for path in [r"C:\repo\$x", r"C:\repo\100%", "C:\\repo\\new\nline"] {
            assert!(command(path).is_err(), "{path:?} was typed");
        }
    }
}
