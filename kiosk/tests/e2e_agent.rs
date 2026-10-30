//! Agent status in a real tmux server, from OSC 7501 reports and from fake agents' screens.
#![cfg(unix)]

mod support;

use serde_json::Value;
use support::{KIOSK, TestEnv, wait_for};

/// A shell command printing an OSC 7501 report with `body`.
fn report(body: &str) -> String {
    format!("printf '\\033]7501;{body}\\033\\\\'")
}

fn agents(env: &TestEnv) -> Vec<Value> {
    serde_json::from_str::<Value>(&env.kiosk_ok(&["agents", "--json"]))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn agent_in<'a>(agents: &'a [Value], session: &str) -> Option<&'a Value> {
    agents.iter().find(|agent| agent["session_name"] == session)
}

fn hooks(env: &TestEnv) -> usize {
    env.tmux_ok(&["show-hooks", "-g"])
        .lines()
        .filter(|line| line.contains("[7501]"))
        .count()
}

/// Starts the server and lets kiosk set its hooks, so new panes get collectors.
fn tracked_server(env: &TestEnv) {
    env.start(&env.path("projects"), "sleep 600");
    env.kiosk_ok(&["agents"]);
    assert_eq!(hooks(env), 3);
}

#[test]
fn osc7501_reports_become_agent_status_until_the_next_prompt() {
    let env = TestEnv::new("");
    let repo = env.repo("alpha");
    tracked_server(&env);
    env.session_running(
        "worker",
        &repo,
        &format!(
            "sleep 1; {}; sleep 2; printf '\\033]133;A\\007'; sleep 600",
            report("state=blocked:kind=permission:app=deploy")
        ),
    );
    wait_for("the report", || {
        agent_in(&agents(&env), "worker").is_some_and(|agent| {
            agent["state"] == "waiting"
                && agent["agent"] == "deploy"
                && agent["detail"] == "permission"
                && agent["source"] == "osc7501"
        })
    });
    wait_for("the prompt to clear it", || {
        agent_in(&agents(&env), "worker").is_none()
    });
}

#[test]
fn the_picker_labels_checkouts_with_their_agents() {
    let env = TestEnv::new("");
    let repo = env.repo("alpha");
    tracked_server(&env);
    env.session_running(
        "worker",
        &repo,
        &format!(
            "sleep 1; {}; sleep 600",
            report("state=working:app=builder")
        ),
    );
    wait_for("the report", || agent_in(&agents(&env), "worker").is_some());
    env.restart_picker(&repo);
    env.wait_for_screen("the repo label", |screen| {
        screen.contains("alpha") && screen.contains("builder running  ● open")
    });
    env.keys(&["Tab"]);
    env.wait_for_screen("the branch label", |screen| {
        screen.contains("select branch") && screen.contains("builder running  ● open")
    });
}

/// Writes a fake agent named `name` that prints `screen` and waits.
fn fake_agent(env: &TestEnv, name: &str, screen: &str) {
    env.script(
        name,
        &format!("printf '%b' '{}'\nsleep 600", screen.replace('\'', "'\\''")),
    );
}

#[test]
fn agents_without_osc7501_are_recognised_by_their_screens() {
    let env = TestEnv::new("");
    let repo = env.repo("alpha");
    tracked_server(&env);
    for (name, agent, screen, state) in [
        ("claude-idle", "claude", "❯ \\n? for shortcuts", "idle"),
        (
            "claude-waiting",
            "claude",
            "Allow write to src/main.rs?\\n  Yes, allow\\n  No, deny",
            "waiting",
        ),
        (
            "codex-running",
            "codex",
            "⠋ Searching codebase\\nesc to interrupt",
            "running",
        ),
    ] {
        fake_agent(&env, agent, screen);
        // A shell running the agent, as when a user starts one.
        env.session_running(name, &repo, &format!("sh -c '{agent}; true'"));
        wait_for(name, || {
            agent_in(&agents(&env), name).is_some_and(|found| {
                found["agent"] == agent
                    && found["state"] == state
                    && found["source"] == "heuristics"
            })
        });
    }
}

#[test]
fn a_pane_the_user_pipes_elsewhere_is_left_alone() {
    let env = TestEnv::new("");
    let repo = env.repo("alpha");
    env.start(&env.path("projects"), "sleep 600");
    fake_agent(&env, "claude", "❯ \\n? for shortcuts");
    env.session_running("mine", &repo, "sh -c 'claude; true'");
    let log = env.path("pipe.log");
    env.tmux_ok(&[
        "pipe-pane",
        "-t",
        "mine",
        &format!("cat > '{}'", log.display()),
    ]);
    agents(&env);
    let pipes = env.tmux_ok(&[
        "list-panes",
        "-t",
        "mine",
        "-F",
        "#{pane_pipe} [#{@kiosk_collector}]",
    ]);
    assert_eq!(pipes.trim(), "1 []");
    wait_for("the screen fallback", || {
        agent_in(&agents(&env), "mine").is_some_and(|agent| agent["source"] == "heuristics")
    });
}

#[test]
fn turning_osc7501_off_removes_hooks_and_collectors() {
    let env = TestEnv::new("");
    tracked_server(&env);
    let collected = |env: &TestEnv| {
        env.tmux_ok(&["list-panes", "-a", "-F", "#{pane_pipe}#{@kiosk_collector}"])
            .lines()
            .filter(|line| line.len() > 1)
            .count()
    };
    wait_for("a collector", || collected(&env) == 1);
    env.write_config("[agent]\nosc7501 = false\n");
    env.kiosk_ok(&["agents"]);
    assert_eq!(hooks(&env), 0);
    wait_for("the collectors to stop", || collected(&env) == 0);
}

#[test]
fn next_switches_to_the_session_whose_agent_waits() {
    let env = TestEnv::new("");
    let repo = env.repo("alpha");
    tracked_server(&env);
    env.session_running(
        "busy",
        &repo,
        &format!("sleep 1; {}; sleep 600", report("state=working")),
    );
    env.session_running(
        "asking",
        &repo,
        &format!("sleep 1; {}; sleep 600", report("state=blocked:app=agent")),
    );
    wait_for("both reports", || {
        let found = agents(&env);
        agent_in(&found, "busy").is_some() && agent_in(&found, "asking").is_some()
    });
    env.respawn(&repo, &format!("{KIOSK} next"));
    wait_for("the switch", || env.client_session() == "asking");
    assert!(env.screen().contains("Switched to asking (agent waiting)"));
}
