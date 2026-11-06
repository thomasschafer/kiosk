//! What the app's `kiosk` mod asks for on the command line, and finding the programs kiosk
//! drives.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, de::DeserializeOwned};

use crate::app::Space;

/// The command the mod runs.
pub const SERVE_COMMAND: &str = "serve";

const USAGE: &str = "usage: herdr-app-kiosk serve [--app-home <folder>] [--context <json>] \
     [--spaces <json>] | --version";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Serve the picker for the mod to draw.
    Serve(ServeArgs),
    /// Print the version, for the mod's installer to check.
    Version,
}

/// What the mod passes the serve command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeArgs {
    /// The app's home (`HERDR_APP_HOME`), which selects the window `herdr-app` drives.
    pub app_home: Option<PathBuf>,
    /// Where the person is in the app.
    pub context: AppContext,
    /// The app's spaces when the mod started kiosk, if it passed them.
    pub spaces: Option<Vec<Space>>,
}

/// Where the person is in the app when they open kiosk.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct AppContext {
    pub focused_pane_cwd: Option<PathBuf>,
    pub workspace_cwd: Option<PathBuf>,
}

impl AppContext {
    /// The focused terminal's folder, or else the space's.
    pub fn current_cwd(&self) -> Option<&Path> {
        self.focused_pane_cwd
            .as_deref()
            .filter(|path| !path.as_os_str().is_empty())
            .or(self.workspace_cwd.as_deref())
            .filter(|path| !path.as_os_str().is_empty())
    }
}

pub fn invocation() -> Result<Invocation> {
    invocation_with(std::env::args_os().skip(1))
}

pub fn invocation_with(args: impl IntoIterator<Item = OsString>) -> Result<Invocation> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        bail!("{USAGE}");
    };
    match command.to_str() {
        Some("--version" | "-V") => return Ok(Invocation::Version),
        Some(SERVE_COMMAND) => {}
        _ => bail!("unknown command {}; {USAGE}", command.to_string_lossy()),
    }
    let mut serve = ServeArgs::default();
    while let Some(flag) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .with_context(|| format!("{flag} needs a value; {USAGE}"))
        };
        match flag.to_str() {
            Some("--app-home") => {
                let path = PathBuf::from(value("--app-home")?);
                if !path.is_absolute() {
                    bail!("--app-home must be an absolute path: {}", path.display());
                }
                serve.app_home = Some(path);
            }
            Some("--context") => serve.context = json_value("--context", &value("--context")?)?,
            Some("--spaces") => serve.spaces = Some(json_value("--spaces", &value("--spaces")?)?),
            _ => bail!("unknown option {}; {USAGE}", flag.to_string_lossy()),
        }
    }
    Ok(Invocation::Serve(serve))
}

fn json_value<T: DeserializeOwned>(flag: &str, json: &OsString) -> Result<T> {
    let Some(json) = json.to_str() else {
        bail!("{flag} is not valid UTF-8");
    };
    serde_json::from_str(json).map_err(|error| anyhow::anyhow!("invalid {flag}: {error}"))
}

/// The program `name` on `PATH`, as a shell would find it.
pub fn find_program(name: &str) -> Result<PathBuf> {
    find_program_in(name, std::env::var_os("PATH"), std::env::var_os("PATHEXT"))
}

fn find_program_in(
    name: &str,
    path: Option<OsString>,
    pathext: Option<OsString>,
) -> Result<PathBuf> {
    let extensions: Vec<String> = if cfg!(windows) {
        pathext
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path.unwrap_or_default())
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| {
            extensions
                .iter()
                .map(move |extension| dir.join(format!("{name}{extension}")))
        })
        .find(|candidate| is_executable(candidate))
        .with_context(|| format!("{name} is not on PATH"))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoke(args: &[&str]) -> Result<Invocation> {
        invocation_with(args.iter().map(OsString::from))
    }

    #[cfg(not(windows))]
    const APP_HOME: &str = "/home/me/.config/herdr-app";
    #[cfg(windows)]
    const APP_HOME: &str = r"C:\Users\me\.config\herdr-app";

    #[test]
    fn serve_reads_the_app_home_and_context() {
        let invocation = invoke(&[
            SERVE_COMMAND,
            "--app-home",
            APP_HOME,
            "--context",
            r#"{"focused_pane_cwd":"/repo/src","workspace_cwd":"/repo"}"#,
        ])
        .unwrap();
        let Invocation::Serve(serve) = invocation else {
            panic!("{invocation:?}");
        };
        assert_eq!(serve.app_home.as_deref(), Some(Path::new(APP_HOME)));
        assert_eq!(serve.context.current_cwd(), Some(Path::new("/repo/src")));
        assert_eq!(
            invoke(&[SERVE_COMMAND]).unwrap(),
            Invocation::Serve(ServeArgs::default())
        );
    }

    #[test]
    fn serve_reads_the_spaces_the_mod_saw() {
        let invocation = invoke(&[
            SERVE_COMMAND,
            "--spaces",
            r#"[{"id":"sp1","folders":["/repo",null]},{"id":"sp2","folders":[]}]"#,
        ])
        .unwrap();
        let Invocation::Serve(serve) = invocation else {
            panic!("{invocation:?}");
        };
        assert_eq!(
            serve.spaces,
            Some(vec![
                Space {
                    id: "sp1".into(),
                    folders: vec![Some("/repo".into()), None],
                },
                Space {
                    id: "sp2".into(),
                    folders: vec![],
                },
            ])
        );
    }

    #[test]
    fn the_space_folder_is_the_fallback_and_unknown_context_fields_are_ignored() {
        let context: AppContext = serde_json::from_str(
            r#"{"focused_pane_cwd":"","workspace_cwd":"/repo","future_field":true}"#,
        )
        .unwrap();
        assert_eq!(context.current_cwd(), Some(Path::new("/repo")));
        assert_eq!(AppContext::default().current_cwd(), None);
    }

    #[test]
    fn version_is_asked_for_with_a_flag() {
        assert_eq!(invoke(&["--version"]).unwrap(), Invocation::Version);
        assert_eq!(invoke(&["-V"]).unwrap(), Invocation::Version);
    }

    #[test]
    fn bad_arguments_are_refused() {
        for (args, expected) in [
            (&[][..], "usage"),
            (&["app"], "unknown command"),
            (&[SERVE_COMMAND, "--verbose"], "unknown option"),
            (&[SERVE_COMMAND, "--app-home"], "needs a value"),
            (&[SERVE_COMMAND, "--app-home", "relative"], "absolute"),
            (&[SERVE_COMMAND, "--context", "not json"], "--context"),
            (
                &[SERVE_COMMAND, "--spaces", r#"[{"id":"sp1"}]"#],
                "--spaces",
            ),
            (&[SERVE_COMMAND, "--spaces"], "needs a value"),
        ] {
            let error = invoke(args).unwrap_err().to_string();
            assert!(error.contains(expected), "{args:?}: {error}");
        }
    }

    #[test]
    fn programs_are_found_on_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let name = "kiosk-test-program";
        let file_name = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let program = second.path().join(&file_name);
        std::fs::write(&program, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let not_executable = first.path().join(&file_name);
            std::fs::write(&not_executable, "").unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();
        let found = find_program_in(name, Some(path.clone()), None).unwrap();
        assert!(
            kiosk_core::path::equivalent(&found, &program),
            "found {} rather than {}",
            found.display(),
            program.display()
        );
        let error = find_program_in("not-a-kiosk-program", Some(path), None).unwrap_err();
        assert!(error.to_string().contains("not on PATH"), "{error}");
    }
}
