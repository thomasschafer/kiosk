use super::{AgentKind, AgentState};
use regex::Regex;

/// Detect the kind of agent from tmux pane command or child process arguments
pub fn detect_agent_kind(pane_command: &str, child_process_args: Option<&str>) -> AgentKind {
    // Check pane command first
    if pane_command.to_lowercase().contains("claude") {
        return AgentKind::ClaudeCode;
    }
    if pane_command.to_lowercase().contains("codex") {
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

/// Detect agent state from terminal content, dispatching to agent-specific detectors
pub fn detect_state(content: &str, kind: AgentKind) -> AgentState {
    let clean_content = strip_ansi_codes(content);
    let last_lines = get_last_non_empty_lines(&clean_content, 30);

    match kind {
        AgentKind::Codex => detect_codex_state(&last_lines),
        AgentKind::ClaudeCode | AgentKind::Unknown => detect_claude_code_state(&last_lines), // Fallback to Claude Code
    }
}

/// Detect Claude Code agent state from terminal content
pub fn detect_claude_code_state(content: &str) -> AgentState {
    let content_lower = content.to_lowercase();

    // Check for Running patterns
    if contains_running_patterns(&content_lower) {
        return AgentState::Running;
    }

    // Check for Waiting patterns
    if contains_waiting_patterns(&content_lower) {
        return AgentState::Waiting;
    }

    // Default to Idle
    AgentState::Idle
}

/// Detect Codex agent state from terminal content
pub fn detect_codex_state(content: &str) -> AgentState {
    let content_lower = content.to_lowercase();

    // Check for Running patterns
    if contains_codex_running_patterns(&content_lower) {
        return AgentState::Running;
    }

    // Check for Waiting patterns
    if contains_codex_waiting_patterns(&content_lower) {
        return AgentState::Waiting;
    }

    // Default to Idle
    AgentState::Idle
}

fn contains_running_patterns(content: &str) -> bool {
    let running_patterns = ["esc to interrupt", "ctrl+c to interrupt"];

    // Check for text patterns
    for pattern in &running_patterns {
        if content.contains(pattern) {
            return true;
        }
    }

    // Check for braille spinner characters
    let braille_spinners = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    for spinner in &braille_spinners {
        if content.contains(*spinner) {
            return true;
        }
    }

    false
}

fn contains_waiting_patterns(content: &str) -> bool {
    let waiting_patterns = [
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

    for pattern in &waiting_patterns {
        if content.contains(pattern) {
            return true;
        }
    }

    false
}

fn contains_codex_running_patterns(content: &str) -> bool {
    let running_patterns = ["esc to interrupt", "working", "thinking"];

    // Check for text patterns
    for pattern in &running_patterns {
        if content.contains(pattern) {
            return true;
        }
    }

    // Check for braille spinner characters
    let braille_spinners = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    for spinner in &braille_spinners {
        if content.contains(*spinner) {
            return true;
        }
    }

    false
}

fn contains_codex_waiting_patterns(content: &str) -> bool {
    let waiting_patterns = [
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

    for pattern in &waiting_patterns {
        if content.contains(pattern) {
            return true;
        }
    }

    false
}

/// Strip ANSI escape codes from terminal content
fn strip_ansi_codes(content: &str) -> String {
    // Simple regex to match common ANSI escape sequences
    let re = Regex::new(r"\x1B\[[0-9;]*[mGKHfJABCDnsu]").unwrap();
    re.replace_all(content, "").to_string()
}

/// Get the last N non-empty lines from content
fn get_last_non_empty_lines(content: &str, count: usize) -> String {
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    let start_idx = if lines.len() > count {
        lines.len() - count
    } else {
        0
    };

    lines[start_idx..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_claude_code_running_state() {
        assert_eq!(
            detect_claude_code_state("Processing... esc to interrupt"),
            AgentState::Running
        );
        assert_eq!(
            detect_claude_code_state("Working hard ⠋ please wait"),
            AgentState::Running
        );
        assert_eq!(
            detect_claude_code_state("Press ctrl+c to interrupt the process"),
            AgentState::Running
        );
    }

    #[test]
    fn test_claude_code_waiting_state() {
        assert_eq!(
            detect_claude_code_state("Do you want to proceed? (Y/n)"),
            AgentState::Waiting
        );
        assert_eq!(
            detect_claude_code_state("Yes, allow this action\nNo, cancel"),
            AgentState::Waiting
        );
        assert_eq!(
            detect_claude_code_state("❯ 1. Option A\n  2. Option B\nEnter to select"),
            AgentState::Waiting
        );
        assert_eq!(
            detect_claude_code_state("Do you trust the files in this directory?"),
            AgentState::Waiting
        );
    }

    #[test]
    fn test_claude_code_idle_state() {
        assert_eq!(detect_claude_code_state("$ "), AgentState::Idle);
        assert_eq!(
            detect_claude_code_state("Welcome to Claude Code\n> "),
            AgentState::Idle
        );
        assert_eq!(detect_claude_code_state(""), AgentState::Idle);
    }

    #[test]
    fn test_codex_running_state() {
        assert_eq!(
            detect_codex_state("Codex is working on your request... esc to interrupt"),
            AgentState::Running
        );
        assert_eq!(
            detect_codex_state("Thinking ⠙ about your question"),
            AgentState::Running
        );
        assert_eq!(
            detect_codex_state("Processing files\nworking..."),
            AgentState::Running
        );
    }

    #[test]
    fn test_codex_waiting_state() {
        assert_eq!(
            detect_codex_state("Do you want to proceed? Yes, proceed / No"),
            AgentState::Waiting
        );
        assert_eq!(
            detect_codex_state("Press enter to confirm your choice"),
            AgentState::Waiting
        );
        assert_eq!(
            detect_codex_state("Please approve this action: [y/n]"),
            AgentState::Waiting
        );
    }

    #[test]
    fn test_codex_idle_state() {
        assert_eq!(detect_codex_state("> "), AgentState::Idle);
        assert_eq!(detect_codex_state("Codex ready\n> "), AgentState::Idle);
    }

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
    fn test_get_last_non_empty_lines() {
        let content = "Line 1\n\nLine 3\n\nLine 5\nLine 6\n\n";
        assert_eq!(get_last_non_empty_lines(content, 2), "Line 5\nLine 6");
        assert_eq!(
            get_last_non_empty_lines(content, 10), // More than available
            "Line 1\nLine 3\nLine 5\nLine 6"
        );
    }

    #[test]
    fn test_detect_state_with_ansi_codes() {
        let content_with_ansi = "\x1B[32mProcessing...\x1B[0m esc to interrupt";
        assert_eq!(
            detect_state(content_with_ansi, AgentKind::ClaudeCode),
            AgentState::Running
        );
    }

    #[test]
    fn test_detect_state_unknown_fallback() {
        let content = "Do you want to proceed? (Y/n)";
        assert_eq!(
            detect_state(content, AgentKind::Unknown),
            AgentState::Waiting
        );
    }

    #[test]
    fn test_braille_spinner_detection() {
        for spinner in ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'] {
            let content = format!("Loading {} please wait", spinner);
            assert_eq!(
                detect_claude_code_state(&content),
                AgentState::Running,
                "Failed to detect running state for spinner: {}",
                spinner
            );
        }
    }

    #[test]
    fn test_case_insensitive_detection() {
        assert_eq!(
            detect_claude_code_state("ESC TO INTERRUPT"),
            AgentState::Running
        );
        assert_eq!(detect_claude_code_state("Yes, Allow"), AgentState::Waiting);
    }

    #[test]
    fn test_empty_content() {
        assert_eq!(detect_claude_code_state(""), AgentState::Idle);
        assert_eq!(detect_codex_state(""), AgentState::Idle);
        assert_eq!(detect_state("", AgentKind::Unknown), AgentState::Idle);
    }

    #[test]
    fn test_only_whitespace_content() {
        assert_eq!(detect_claude_code_state("   \n\n  \t  "), AgentState::Idle);
    }
}
