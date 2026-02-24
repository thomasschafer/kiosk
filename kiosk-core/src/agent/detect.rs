use super::{AgentKind, AgentState};

/// Detect the kind of agent from tmux pane command or child process arguments
pub fn detect_agent_kind(pane_command: &str, child_process_args: Option<&str>) -> AgentKind {
    // Check pane command first
    let cmd_lower = pane_command.to_lowercase();
    if cmd_lower.contains("claude") {
        return AgentKind::ClaudeCode;
    }
    if cmd_lower.contains("codex") {
        return AgentKind::Codex;
    }

    // Check child process args if available
    if let Some(args) = child_process_args {
        let args_lower = args.to_lowercase();
        if args_lower.contains("claude") {
            return AgentKind::ClaudeCode;
        }
        if args_lower.contains("codex") {
            return AgentKind::Codex;
        }
    }

    AgentKind::Unknown
}

// ---------------------------------------------------------------------------
// Pattern constants
// ---------------------------------------------------------------------------

const BRAILLE_SPINNERS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const CLAUDE_RUNNING_PATTERNS: &[&str] = &["esc to interrupt", "ctrl+c to interrupt"];

const CLAUDE_WAITING_PATTERNS: &[&str] = &[
    "yes, allow",
    "yes, and always allow",
    "yes, and don't ask again",
    "allow once",
    "allow always",
    "(y/n)",
    "[y/n]",
    "enter to select",
    "esc to cancel",
    "❯ 1.",
    "do you trust the files",
];

const CODEX_RUNNING_PATTERNS: &[&str] = &["esc to interrupt", "working", "thinking"];

const CODEX_WAITING_PATTERNS: &[&str] = &[
    "yes, proceed",
    "press enter to confirm",
    "(y/n)",
    "[y/n]",
    "approve",
    "allow",
    "❯ 1.",
    "enter to select",
    "esc to cancel",
];

// ---------------------------------------------------------------------------
// State detection
// ---------------------------------------------------------------------------

/// Detect agent state from terminal content, dispatching to agent-specific detectors.
/// Content is ANSI-stripped and lowercased once here; per-agent functions receive clean input.
pub fn detect_state(content: &str, kind: AgentKind) -> AgentState {
    let clean = strip_ansi_codes(content);
    let last_lines = get_last_non_empty_lines(&clean, 30);
    let lowered = last_lines.to_lowercase();

    match kind {
        AgentKind::ClaudeCode => {
            detect_agent_state(&lowered, CLAUDE_RUNNING_PATTERNS, CLAUDE_WAITING_PATTERNS)
        }
        AgentKind::Codex => {
            detect_agent_state(&lowered, CODEX_RUNNING_PATTERNS, CODEX_WAITING_PATTERNS)
        }
        AgentKind::Unknown => AgentState::Idle,
    }
}

/// Generic agent state detection: checks running patterns (+ braille spinners),
/// then waiting patterns, then defaults to Idle.
fn detect_agent_state(
    content: &str,
    running_patterns: &[&str],
    waiting_patterns: &[&str],
) -> AgentState {
    if matches_any(content, running_patterns) || contains_braille_spinner(content) {
        return AgentState::Running;
    }
    if matches_any(content, waiting_patterns) {
        return AgentState::Waiting;
    }
    AgentState::Idle
}

fn matches_any(content: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| content.contains(p))
}

fn contains_braille_spinner(content: &str) -> bool {
    content.chars().any(|c| BRAILLE_SPINNERS.contains(&c))
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// Strip ANSI escape codes from terminal content without regex.
/// Scans for ESC[ sequences and skips to the terminating byte.
fn strip_ansi_codes(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut chars = content.chars();
    while let Some(c) = chars.next() {
        if c == '\x1B' {
            // Check for CSI sequence (ESC + '[')
            if let Some('[') = chars.next() {
                // Skip parameter bytes and intermediate bytes until final byte (0x40-0x7E)
                for c in chars.by_ref() {
                    if c.is_ascii() && (0x40..=0x7E).contains(&(c as u8)) {
                        break;
                    }
                }
            }
            // else: lone ESC or other escape — drop both bytes
        } else {
            out.push(c);
        }
    }
    out
}

/// Get the last N non-empty lines from content
fn get_last_non_empty_lines(content: &str, count: usize) -> String {
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    let start_idx = lines.len().saturating_sub(count);
    lines[start_idx..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- detect_agent_kind ---------------------------------------------------

    #[test]
    fn test_detect_agent_kind_claude_in_command() {
        assert_eq!(detect_agent_kind("claude", None), AgentKind::ClaudeCode);
        assert_eq!(
            detect_agent_kind("Claude Code", None),
            AgentKind::ClaudeCode
        );
    }

    #[test]
    fn test_detect_agent_kind_codex_in_command() {
        assert_eq!(detect_agent_kind("codex", None), AgentKind::Codex);
        assert_eq!(detect_agent_kind("some-codex-tool", None), AgentKind::Codex);
    }

    #[test]
    fn test_detect_agent_kind_child_process() {
        assert_eq!(
            detect_agent_kind("bash", Some("python claude_main.py")),
            AgentKind::ClaudeCode
        );
        assert_eq!(
            detect_agent_kind("node", Some("/usr/bin/codex --version")),
            AgentKind::Codex
        );
    }

    #[test]
    fn test_detect_agent_kind_unknown() {
        assert_eq!(detect_agent_kind("bash", None), AgentKind::Unknown);
        assert_eq!(
            detect_agent_kind("vim", Some("vim file.txt")),
            AgentKind::Unknown
        );
    }

    // -- detect_state (full pipeline: ANSI strip + lowercase + detect) -------

    #[test]
    fn test_claude_running() {
        assert_eq!(
            detect_state("Processing... esc to interrupt", AgentKind::ClaudeCode),
            AgentState::Running
        );
        assert_eq!(
            detect_state("Working hard ⠋ please wait", AgentKind::ClaudeCode),
            AgentState::Running
        );
        assert_eq!(
            detect_state(
                "Press ctrl+c to interrupt the process",
                AgentKind::ClaudeCode
            ),
            AgentState::Running
        );
    }

    #[test]
    fn test_claude_waiting() {
        assert_eq!(
            detect_state("Do you want to proceed? (Y/n)", AgentKind::ClaudeCode),
            AgentState::Waiting
        );
        assert_eq!(
            detect_state("Yes, allow this action\nNo, cancel", AgentKind::ClaudeCode),
            AgentState::Waiting
        );
        assert_eq!(
            detect_state(
                "❯ 1. Option A\n  2. Option B\nEnter to select",
                AgentKind::ClaudeCode
            ),
            AgentState::Waiting
        );
        assert_eq!(
            detect_state(
                "Do you trust the files in this directory?",
                AgentKind::ClaudeCode
            ),
            AgentState::Waiting
        );
    }

    #[test]
    fn test_claude_idle() {
        assert_eq!(
            detect_state("$ ", AgentKind::ClaudeCode),
            AgentState::Idle
        );
        assert_eq!(
            detect_state("Welcome to Claude Code\n> ", AgentKind::ClaudeCode),
            AgentState::Idle
        );
        assert_eq!(
            detect_state("", AgentKind::ClaudeCode),
            AgentState::Idle
        );
    }

    #[test]
    fn test_codex_running() {
        assert_eq!(
            detect_state(
                "Codex is working on your request... esc to interrupt",
                AgentKind::Codex
            ),
            AgentState::Running
        );
        assert_eq!(
            detect_state("Thinking ⠙ about your question", AgentKind::Codex),
            AgentState::Running
        );
        assert_eq!(
            detect_state("Processing files\nworking...", AgentKind::Codex),
            AgentState::Running
        );
    }

    #[test]
    fn test_codex_waiting() {
        assert_eq!(
            detect_state(
                "Do you want to proceed? Yes, proceed / No",
                AgentKind::Codex
            ),
            AgentState::Waiting
        );
        assert_eq!(
            detect_state("Press enter to confirm your choice", AgentKind::Codex),
            AgentState::Waiting
        );
        assert_eq!(
            detect_state("Please approve this action: [y/n]", AgentKind::Codex),
            AgentState::Waiting
        );
    }

    #[test]
    fn test_codex_idle() {
        assert_eq!(
            detect_state("> ", AgentKind::Codex),
            AgentState::Idle
        );
        assert_eq!(
            detect_state("Codex ready\n> ", AgentKind::Codex),
            AgentState::Idle
        );
    }

    // -- Unknown kind returns Idle -------------------------------------------

    #[test]
    fn test_unknown_always_idle() {
        assert_eq!(
            detect_state("esc to interrupt", AgentKind::Unknown),
            AgentState::Idle
        );
        assert_eq!(
            detect_state("(Y/n)", AgentKind::Unknown),
            AgentState::Idle
        );
        assert_eq!(
            detect_state("", AgentKind::Unknown),
            AgentState::Idle
        );
    }

    // -- ANSI stripping ------------------------------------------------------

    #[test]
    fn test_strip_ansi_codes() {
        assert_eq!(strip_ansi_codes("\x1B[32mGreen text\x1B[0m"), "Green text");
        assert_eq!(strip_ansi_codes("Normal text"), "Normal text");
        assert_eq!(
            strip_ansi_codes("\x1B[1;31mBold red\x1B[0m and normal"),
            "Bold red and normal"
        );
    }

    #[test]
    fn test_detect_state_with_ansi_codes() {
        assert_eq!(
            detect_state(
                "\x1B[32mProcessing...\x1B[0m esc to interrupt",
                AgentKind::ClaudeCode
            ),
            AgentState::Running
        );
    }

    #[test]
    fn test_mixed_case_ansi_pipeline() {
        assert_eq!(
            detect_state(
                "\x1B[1mYES, ALLOW\x1B[0m this action",
                AgentKind::ClaudeCode
            ),
            AgentState::Waiting
        );
    }

    // -- Helpers -------------------------------------------------------------

    #[test]
    fn test_get_last_non_empty_lines() {
        let content = "Line 1\n\nLine 3\n\nLine 5\nLine 6\n\n";
        assert_eq!(get_last_non_empty_lines(content, 2), "Line 5\nLine 6");
        assert_eq!(
            get_last_non_empty_lines(content, 10),
            "Line 1\nLine 3\nLine 5\nLine 6"
        );
    }

    #[test]
    fn test_braille_spinner_detection() {
        for spinner in BRAILLE_SPINNERS {
            let content = format!("Loading {spinner} please wait");
            assert_eq!(
                detect_state(&content, AgentKind::ClaudeCode),
                AgentState::Running,
                "Failed for spinner: {spinner}",
            );
        }
    }

    #[test]
    fn test_case_insensitive_detection() {
        assert_eq!(
            detect_state("ESC TO INTERRUPT", AgentKind::ClaudeCode),
            AgentState::Running
        );
        assert_eq!(
            detect_state("Yes, Allow", AgentKind::ClaudeCode),
            AgentState::Waiting
        );
    }

    #[test]
    fn test_empty_content() {
        assert_eq!(detect_state("", AgentKind::ClaudeCode), AgentState::Idle);
        assert_eq!(detect_state("", AgentKind::Codex), AgentState::Idle);
        assert_eq!(detect_state("", AgentKind::Unknown), AgentState::Idle);
    }

    #[test]
    fn test_only_whitespace_content() {
        assert_eq!(
            detect_state("   \n\n  \t  ", AgentKind::ClaudeCode),
            AgentState::Idle
        );
    }
}
