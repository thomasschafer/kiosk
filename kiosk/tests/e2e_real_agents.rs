//! Agent status for real Claude Code and Codex sessions. These need both agents installed
//! and logged in, and each run makes a few small API calls, so they are ignored by default
//! and refuse to run without `KIOSK_E2E_REAL_AGENTS=1`. `scripts/nightly-e2e.sh` runs them.
//!
//! Each agent runs in a throwaway repo with the narrowest permissions that still let it ask
//! to run one shell command. The test follows it from idle, through running and waiting for
//! permission, back to idle once the command has run, checking what `kiosk agents` reports
//! at each step.
#![cfg(unix)]

mod support;

use std::{
    fmt::Write as _,
    fs,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use kiosk_core::git::test_fixtures::repo_fixture;
use support::{TestEnv, agent_in, agents, tracked_server, wait_for};

/// The session the agent runs in.
const SESSION: &str = "agent";
const STARTUP: Duration = Duration::from_secs(90);
/// How long the agent must stay idle after starting before the test trusts it is ready.
const SETTLE: Duration = Duration::from_secs(3);
/// How long the agent may take to answer a prompt or finish a command.
const TURN: Duration = Duration::from_secs(120);
/// How long kiosk may take to notice the agent move on after a key press.
const REACTION: Duration = Duration::from_secs(30);
/// The file the agent's command creates.
const FILE: &str = "finished.txt";
/// The agent's final reply, which the prompt spells out without containing, so the prompt's
/// echo on screen is not mistaken for the reply.
const REPLY: &str = "KIOSK-OK";
/// Writing a file needs permission from both agents as they are run here; the sleep keeps
/// the agent running long enough for kiosk to see it.
const PROMPT: &str = "Run the shell command `sleep 6 && touch finished.txt`, then reply with \
                      only the words KIOSK and OK joined by a hyphen.";

struct Agent {
    /// The command, which is also the name kiosk reports.
    name: &'static str,
    args: &'static [&'static str],
    /// Screens the agent may show before its prompt is ready.
    startup: &'static [StartupScreen],
    /// Keys that allow the command once.
    approve: &'static [&'static str],
}

struct StartupScreen {
    /// Text only this screen shows.
    marker: &'static str,
    keys: &'static [&'static str],
    /// The state kiosk should report while the screen shows, when it recognises it.
    reported: Option<&'static str>,
}

const CLAUDE: Agent = Agent {
    name: "claude",
    args: &[
        // Asks before anything that is not read-only, whatever the user's default mode is.
        "--permission-mode",
        "manual",
        // Leaves out the user's settings, so their allow rules and hooks do not apply.
        "--setting-sources",
        "project,local",
        "--tools",
        "Bash",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--model",
        "haiku",
        "--effort",
        "low",
    ],
    startup: &[StartupScreen {
        marker: "Yes, I trust this folder",
        keys: &["Down", "Enter"],
        // Kiosk does not recognise Claude Code's folder trust prompt.
        reported: None,
    }],
    approve: &["Enter"],
};

const CODEX: Agent = Agent {
    name: "codex",
    args: &[
        "--no-daemon",
        "--sandbox",
        "read-only",
        "--ask-for-approval",
        "on-request",
        // Sends approvals to the user rather than to an automatic reviewer.
        "-c",
        "approvals_reviewer=user",
        "-c",
        "notify=[]",
        "-c",
        "model_reasoning_effort=low",
    ],
    startup: &[
        StartupScreen {
            marker: "Trust this folder?",
            keys: &["Enter"],
            reported: Some("waiting"),
        },
        // Shown when the user has Codex hooks; they are not needed here.
        StartupScreen {
            marker: "Hooks need review",
            keys: &["3"],
            reported: Some("waiting"),
        },
    ],
    approve: &["y"],
};

#[test]
#[ignore = "runs real agents; see scripts/nightly-e2e.sh"]
fn claude_code_through_a_permission_prompt() {
    follow(&CLAUDE);
}

#[test]
#[ignore = "runs real agents; see scripts/nightly-e2e.sh"]
fn codex_through_a_permission_prompt() {
    follow(&CODEX);
}

fn follow(agent: &Agent) {
    assert!(
        std::env::var("KIOSK_E2E_REAL_AGENTS").is_ok_and(|value| value == "1"),
        "these tests run real agents, which costs API calls; set KIOSK_E2E_REAL_AGENTS=1 to run \
         them"
    );
    let installed = Command::new("sh")
        .args(["-c", &format!("command -v {}", agent.name)])
        .output()
        .unwrap();
    assert!(installed.status.success(), "{} is not on PATH", agent.name);

    // Declared first so it is removed after the server, and the agents with it, are gone.
    let repo = Repo::new(agent.name);
    let env = TestEnv::new("");
    tracked_server(&env);
    env.session_running(
        SESSION,
        &repo.path,
        // Keeps the pane after the agent exits, so a failure shows why it did.
        &format!("{}; sleep 600", launch_command(agent)),
    );
    wait_for("a collector on the agent's pane", || {
        pane_format(&env, "#{pane_pipe}:#{@kiosk_collector}").len() > 2
    });
    let mut watch = Watch {
        env: &env,
        agent,
        seen: Vec::new(),
    };

    watch.start();
    env.type_text_to(SESSION, PROMPT);
    env.keys_to(SESSION, &["Enter"]);
    watch.until(
        "the permission prompt",
        TURN,
        &["idle", "running"],
        Expected::Permission,
    );
    assert!(
        !repo.path.join(FILE).exists(),
        "{} ran the command without permission",
        agent.name
    );

    env.keys_to(SESSION, agent.approve);
    watch.until(
        "the command to run",
        REACTION,
        &["waiting"],
        Expected::Running,
    );
    watch.until("the reply", TURN, &["running"], Expected::Finished);
    assert!(
        env.screen_of(SESSION).contains(REPLY),
        "{} went idle before replying",
        agent.name
    );
    assert!(repo.path.join(FILE).exists());
    eprintln!("{}: kiosk reported {}", agent.name, watch.seen.join(" -> "));
}

/// The agent's command line, without the variables of any agent session running the tests,
/// which would otherwise make the agent think it is nested in that one.
fn launch_command(agent: &Agent) -> String {
    let mut command = String::from("env");
    for (name, _) in std::env::vars() {
        if name.starts_with("CLAUDE") || name.starts_with("CODEX") {
            write!(command, " -u {name}").unwrap();
        }
    }
    write!(command, " {}", agent.name).unwrap();
    for arg in agent.args {
        write!(command, " '{}'", arg.replace('\'', "'\\''")).unwrap();
    }
    command
}

fn pane_format(env: &TestEnv, format: &str) -> String {
    env.tmux_ok(&["display-message", "-p", "-t", SESSION, format])
        .trim()
        .to_owned()
}

/// A repo for one agent, in the same place every run so the folder trust the agent saves
/// for it is one entry rather than one per run. It lives in the user's cache rather than
/// the temporary directory, which the Nix dev shell moves on every run.
struct Repo {
    path: PathBuf,
}

impl Repo {
    fn new(agent: &str) -> Self {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .filter(|cache| !cache.is_empty())
            .map(PathBuf::from)
            .or_else(|| kiosk_core::path::home_dir().map(|home| home.join(".cache")))
            .expect("no home directory for the agents' repos");
        fs::create_dir_all(&cache).unwrap();
        let parent = kiosk_core::path::canonical_or_original(&cache).join("kiosk-e2e-real-agents");
        let path = parent.join(agent);
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        fs::create_dir_all(&parent).unwrap();
        Self {
            path: repo_fixture(&parent, agent),
        }
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A state the test waits for. Screen heuristics cannot tell a finished agent from an idle
/// one, but an agent reporting over OSC 7501 says it is done, and why it waits.
#[derive(Clone, Copy, Debug)]
enum Expected {
    Idle,
    Permission,
    Running,
    Finished,
}

impl Expected {
    fn matches(self, report: &Report) -> bool {
        let osc = report.source == Source::Osc7501;
        match self {
            Self::Idle => report.state == "idle",
            Self::Permission => {
                report.state == "waiting"
                    && (!osc || report.detail.as_deref() == Some("permission"))
            }
            Self::Running => report.state == "running",
            Self::Finished => report.state == if osc { "done" } else { "idle" },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    Osc7501,
    Heuristics,
}

/// What kiosk reports for the agent's pane.
struct Report {
    state: String,
    detail: Option<String>,
    source: Source,
}

/// Polls kiosk for the agent's status, checking each report and keeping the states seen.
struct Watch<'a> {
    env: &'a TestEnv,
    agent: &'a Agent,
    seen: Vec<String>,
}

impl Watch<'_> {
    /// What kiosk reports for the agent, after checking the report names the agent and
    /// comes from OSC 7501 exactly when the pane has sent a report.
    fn poll(&mut self) -> Option<Report> {
        let found = agent_in(&agents(self.env), SESSION).cloned();
        let reported = !pane_format(self.env, "#{@kiosk_status}").is_empty();
        let found = found?;
        let source = match found["source"].as_str() {
            Some("osc7501") => Source::Osc7501,
            Some("heuristics") => Source::Heuristics,
            other => panic!("unexpected source {other:?} in {found}"),
        };
        assert_eq!(
            source == Source::Osc7501,
            reported,
            "kiosk's source does not match whether the pane sent an OSC 7501 report: {found}"
        );
        // OSC 7501 reports may leave the app out.
        if source == Source::Heuristics || !found["agent"].is_null() {
            assert_eq!(found["agent"], self.agent.name, "{found}");
        }
        let report = Report {
            state: found["state"].as_str().unwrap().to_owned(),
            detail: found["detail"].as_str().map(str::to_owned),
            source,
        };
        let decided_by = found["rule"].as_str().unwrap_or("osc7501");
        let label = match &report.detail {
            Some(detail) => format!("{} ({detail}; {decided_by})", report.state),
            None => format!("{} ({decided_by})", report.state),
        };
        if self.seen.last() != Some(&label) {
            self.seen.push(label);
        }
        Some(report)
    }

    /// Answers startup screens until kiosk reports the agent idle.
    fn start(&mut self) {
        let deadline = Instant::now() + STARTUP;
        let mut idle_since = None;
        loop {
            let screen = self.env.screen_of(SESSION);
            if let Some(startup) = self
                .agent
                .startup
                .iter()
                .find(|startup| screen.contains(startup.marker))
            {
                if let Some(state) = startup.reported {
                    let found = self.poll().unwrap_or_else(|| {
                        self.fail(&format!("no status while {:?} shows", startup.marker))
                    });
                    if found.state != state {
                        self.fail(&format!(
                            "{} rather than {state} while {:?} shows",
                            found.state, startup.marker
                        ));
                    }
                }
                self.env.keys_to(SESSION, startup.keys);
                let gone = Instant::now() + REACTION;
                while self.env.screen_of(SESSION).contains(startup.marker) {
                    if Instant::now() > gone {
                        self.fail(&format!(
                            "{:?} stayed after {:?}",
                            startup.marker, startup.keys
                        ));
                    }
                    thread::sleep(Duration::from_millis(250));
                }
                idle_since = None;
                continue;
            }
            // Agents can look ready for a moment before a startup screen appears.
            if self
                .poll()
                .is_some_and(|report| Expected::Idle.matches(&report))
            {
                let idle = idle_since.get_or_insert_with(Instant::now);
                if idle.elapsed() >= SETTLE {
                    return;
                }
            } else {
                idle_since = None;
            }
            if Instant::now() > deadline {
                self.fail("the agent to start");
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    /// Polls until kiosk reports `expected`, failing on any state but those `allowed` on
    /// the way.
    fn until(&mut self, what: &str, timeout: Duration, allowed: &[&str], expected: Expected) {
        let deadline = Instant::now() + timeout;
        loop {
            match self.poll() {
                Some(report) if expected.matches(&report) => return,
                Some(report) if !allowed.contains(&report.state.as_str()) => {
                    self.fail(&format!("{} while waiting for {what}", report.state));
                }
                Some(_) => {}
                None => self.fail(&format!("no status while waiting for {what}")),
            }
            if Instant::now() > deadline {
                self.fail(&format!("timed out waiting for {what}"));
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    fn fail(&self, problem: &str) -> ! {
        panic!(
            "{}: {problem}\nkiosk reported: {}\nscreen:\n{}",
            self.agent.name,
            self.seen.join(" -> "),
            self.env.screen_of(SESSION)
        );
    }
}
