//! What each `kiosk` command does.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use kiosk_core::{
    config::{self, ConfigWarning, LoadedConfig},
    git::{CliGitProvider, GitProvider},
};
use serde::Serialize;

use crate::{
    PRODUCT,
    agent::{Detector, PaneStatus, PaneStatuses, labels, osc7501},
    args,
    config::{AgentConfig, Config},
    tmux::{
        CliTmux, PaneId, Server, Session, SessionId, Tmux, TmuxBackend, TmuxError,
        backend::Placement, sessions,
    },
};

fn load() -> Result<LoadedConfig<Config>> {
    config::load_config::<Config>(&PRODUCT)
}

fn tmux_available() -> bool {
    std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Starts or stops OSC 7501 collectors to match the config.
fn sync_collectors(tmux: &dyn Tmux, agent: &AgentConfig) -> Result<()> {
    let sessions = sessions(tmux)?;
    if agent.collects_osc7501() {
        let executable =
            std::env::current_exe().context("could not find kiosk's own executable")?;
        osc7501::tracking::enable(tmux, &executable, &sessions)?;
    } else {
        osc7501::tracking::disable(tmux, &sessions)?;
    }
    Ok(())
}

/// Runs the picker, then attaches to what it opened when kiosk runs outside tmux.
pub fn picker() -> Result<()> {
    let mut loaded = load()?;
    let git = Arc::new(CliGitProvider) as Arc<dyn GitProvider>;
    let current_cwd = std::env::current_dir().ok();
    if !tmux_available() {
        kiosk_tui::launch::run::<TmuxBackend, _>(
            &PRODUCT,
            loaded,
            git,
            None,
            current_cwd.as_deref(),
        )?;
        return Ok(());
    }
    let server = Server::from_env();
    let cli = CliTmux::new(server.clone());
    let tmux = Arc::new(cli.clone()) as Arc<dyn Tmux>;
    if let Err(error) = sync_collectors(tmux.as_ref(), &loaded.extension.agent) {
        loaded.warnings.push(ConfigWarning {
            message: format!("agent status: {error:#}"),
        });
    }
    let placement = loaded
        .extension
        .worktrees
        .placement(kiosk_core::path::home_dir().as_deref())?;
    let backend = Arc::new(TmuxBackend::new(
        tmux,
        Placement::of(&server),
        Arc::clone(&git),
        placement,
        loaded.extension.on_open.clone(),
        loaded.extension.agent.clone(),
    ));
    kiosk_tui::launch::run(
        &PRODUCT,
        loaded,
        git,
        Some(Arc::clone(&backend)),
        current_cwd.as_deref(),
    )?;
    for notice in backend.take_notices() {
        eprintln!("{notice}");
    }
    if let Some(session) = backend.take_attach() {
        attach(&cli, &session)?;
    }
    Ok(())
}

fn attach(tmux: &CliTmux, session: &SessionId) -> Result<()> {
    let mut command = tmux.command();
    command.args(["attach-session", "-t", session.as_str()]);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        Err(error).context("could not run tmux attach-session")
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .context("could not run tmux attach-session")?;
        if !status.success() {
            bail!("tmux attach-session exited with {status}");
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct PaneReport<'a> {
    session: &'a str,
    session_name: &'a str,
    pane: &'a str,
    #[serde(flatten)]
    status: &'a PaneStatus,
}

/// Every pane with an agent, by session.
fn detect(tmux: &dyn Tmux, agent: &AgentConfig) -> Result<Vec<(Session, PaneStatuses)>> {
    if !agent.enabled {
        bail!("agent status is turned off in the config (`agent.enabled = false`)");
    }
    sync_collectors(tmux, agent)?;
    let detector = Detector::new(agent, tmux);
    Ok(sessions(tmux)?
        .into_iter()
        .map(|session| {
            let statuses = detector.session(&session);
            (session, statuses)
        })
        .collect())
}

/// Prints the status of every agent kiosk can see.
pub fn agents(json: bool) -> Result<()> {
    let loaded = load()?;
    let tmux = CliTmux::new(Server::from_env());
    let detected = detect(&tmux, &loaded.extension.agent)?;
    let reports: Vec<PaneReport<'_>> = detected
        .iter()
        .flat_map(|(session, statuses)| {
            statuses.iter().map(move |(pane, status)| PaneReport {
                session: session.id.as_str(),
                session_name: &session.name,
                pane: pane.as_str(),
                status,
            })
        })
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }
    for report in &reports {
        let detail = report
            .status
            .detail
            .map_or_else(String::new, |detail| format!(" ({detail})"));
        let source = match report.status.source {
            crate::agent::Source::Osc7501 => "osc7501".to_string(),
            crate::agent::Source::Heuristics => {
                format!("screen: {}", report.status.rule.unwrap_or("unknown rule"))
            }
        };
        println!(
            "{}\t{}\t{}{detail}\t{source}",
            kiosk_core::display::sanitize(report.session_name),
            report.pane,
            kiosk_core::display::sanitize(&report.status.label().text),
        );
    }
    Ok(())
}

#[derive(Serialize)]
struct Switched<'a> {
    session: &'a str,
    session_name: &'a str,
    labels: Vec<String>,
}

/// Switches to the least recently active other session with an agent waiting on the user.
pub fn next(json: bool) -> Result<()> {
    let loaded = load()?;
    let server = Server::from_env();
    if !server.is_ambient_server() {
        bail!("kiosk next switches the tmux client it runs in, so it must run inside tmux");
    }
    let tmux = CliTmux::new(server);
    let current = current_session(&tmux)?;
    let mut detected = detect(&tmux, &loaded.extension.agent)?;
    detected.sort_by(|(left, _), (right, _)| {
        left.activity
            .cmp(&right.activity)
            .then(left.id.cmp(&right.id))
    });
    let Some((session, statuses)) = detected.iter().find(|(session, statuses)| {
        session.id != current && statuses.iter().any(|(_, status)| status.state.wants_user())
    }) else {
        bail!("no other session has an agent waiting, done, idle or in an unknown state");
    };
    tmux.run(args!["switch-client", "-t", session.id.as_str()])?;
    let switched = Switched {
        session: session.id.as_str(),
        session_name: &session.name,
        labels: labels(statuses)
            .into_iter()
            .map(|label| label.text)
            .collect(),
    };
    if json {
        println!("{}", serde_json::to_string(&switched)?);
    } else {
        println!(
            "Switched to {} ({})",
            kiosk_core::display::sanitize(switched.session_name),
            kiosk_core::display::sanitize(&switched.labels.join(", "))
        );
    }
    Ok(())
}

/// The session of the pane kiosk runs in, or of its client in a popup, which has no pane.
fn current_session(tmux: &dyn Tmux) -> Result<SessionId, TmuxError> {
    let mut command = args!["display-message", "-p"];
    if let Some(pane) = std::env::var("TMUX_PANE")
        .ok()
        .filter(|pane| !pane.is_empty())
    {
        command.extend(args!["-t", pane]);
    }
    command.push("#{session_id}".into());
    SessionId::parse(tmux.run(command)?.trim())
}

/// Collects one pane's OSC 7501 status from standard input; tmux runs this.
pub fn collect(socket: PathBuf, pane: &str) -> Result<()> {
    let tmux = CliTmux::new(Server::Path(socket));
    crate::agent::osc7501::collector::run(&tmux, &PaneId::parse(pane)?, std::io::stdin().lock())?;
    Ok(())
}
