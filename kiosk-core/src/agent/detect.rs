use super::{AgentKind, AgentState};

/// Detect the kind of agent from tmux pane command or child process arguments.
///
/// Order matters: more specific patterns are checked first to avoid false positives
/// (e.g. "cursor-agent" before "agent", "codex" before generic patterns).
pub fn detect_agent_kind(pane_command: &str, child_process_args: Option<&str>) -> AgentKind {
    // Check pane command first
    let cmd_lower = pane_command.to_lowercase();
    if cmd_lower.contains("claude") {
        return AgentKind::ClaudeCode;
    }
    if cmd_lower.contains("codex") {
        return AgentKind::Codex;
    }
    if cmd_lower.contains("cursor-agent") {
        return AgentKind::CursorAgent;
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
        if args_lower.contains("cursor-agent") {
            return AgentKind::CursorAgent;
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

/// Claude Code uses alt-screen so stale content is not an issue, but
/// `? for shortcuts` reliably indicates the idle prompt.
const CLAUDE_IDLE_TAIL_PATTERNS: &[&str] = &["? for shortcuts"];

const CODEX_RUNNING_PATTERNS: &[&str] = &["esc to interrupt"];

const CODEX_WAITING_PATTERNS: &[&str] = &[
    "yes, proceed",
    "yes, continue",
    "press enter to confirm",
    "press enter to continue",
    "(y/n)",
    "[y/n]",
    "approve command",
    "allow once",
    "allow always",
    "❯ 1.",
    "› 1.",
    "enter to select",
    "esc to cancel",
];

/// When Codex is idle, "? for shortcuts" appears at the bottom of the prompt.
/// Checking this against the tail prevents stale waiting/running text from
/// earlier in the buffer from causing false positives.
const CODEX_IDLE_TAIL_PATTERNS: &[&str] = &["? for shortcuts"];

const CURSOR_RUNNING_PATTERNS: &[&str] = &["esc to interrupt", "ctrl+c to interrupt"];

const CURSOR_WAITING_PATTERNS: &[&str] = &[
    "do you trust",
    "trust this workspace",
    "enter to select",
    "(y/n)",
    "[y/n]",
    "esc to cancel",
];

const CURSOR_IDLE_TAIL_PATTERNS: &[&str] = &[];

// ---------------------------------------------------------------------------
// State detection
// ---------------------------------------------------------------------------

/// Detect agent state from terminal content, dispatching to agent-specific detectors.
/// Content is ANSI-stripped and lowercased once here; per-agent functions receive clean input.
pub fn detect_state(content: &str, kind: AgentKind) -> AgentState {
    let clean = strip_ansi_codes(content);
    let last_30 = get_last_non_empty_lines(&clean, 30);
    let last_5 = get_last_non_empty_lines(&clean, 5);
    let content_lowered = last_30.to_lowercase();
    let tail_lowered = last_5.to_lowercase();

    match kind {
        AgentKind::ClaudeCode => detect_claude_state(&content_lowered, &tail_lowered),
        AgentKind::Codex => detect_codex_state(&content_lowered, &tail_lowered),
        AgentKind::CursorAgent => detect_agent_state(
            &content_lowered,
            &tail_lowered,
            CURSOR_RUNNING_PATTERNS,
            CURSOR_WAITING_PATTERNS,
            CURSOR_IDLE_TAIL_PATTERNS,
        ),
        AgentKind::Unknown => AgentState::Idle,
    }
}

/// Claude Code-specific state detection.
///
/// Claude Code uses the alternate screen, so there is no scrollback — the
/// captured pane content IS what the user sees. When Claude is processing,
/// the pane shows the user's prompt and empty lines but NO running indicators
/// (no "esc to interrupt", no spinners). The `? for shortcuts` footer is only
/// visible when Claude is idle at the input prompt.
fn detect_claude_state(content: &str, tail: &str) -> AgentState {
    detect_active_agent_state(
        content,
        tail,
        CLAUDE_RUNNING_PATTERNS,
        CLAUDE_WAITING_PATTERNS,
        CLAUDE_IDLE_TAIL_PATTERNS,
    )
}

/// Codex-specific state detection.
///
/// Codex does NOT use the alternate screen, so old content persists in the
/// scrollback. However, `? for shortcuts` reliably appears at the very bottom
/// when idle. During API calls the footer disappears before `• Working` appears
/// — a brief window (~1s) with no indicators at all.
fn detect_codex_state(content: &str, tail: &str) -> AgentState {
    detect_active_agent_state(
        content,
        tail,
        CODEX_RUNNING_PATTERNS,
        CODEX_WAITING_PATTERNS,
        CODEX_IDLE_TAIL_PATTERNS,
    )
}

/// State detection for agents where absence of the idle footer means "processing".
///
/// Both Claude Code and Codex share this trait: when they are actively working
/// (API call, tool execution), the idle footer (`? for shortcuts`) disappears.
/// During the initial API round-trip, no explicit running indicators are shown
/// either. So the strongest signal is: if the idle footer is gone and nothing
/// else matches → the agent is Running.
///
/// Detection priority:
/// 1. Running patterns in the **tail** → Running (takes precedence over idle
///    because agents like Codex show both `esc to interrupt` and `? for shortcuts`
///    simultaneously during active work)
/// 2. Idle tail patterns (e.g. `? for shortcuts`) → Idle
/// 3. Running patterns in full content + braille spinners → Running
/// 4. Waiting patterns → Waiting
/// 5. Default → **Running** (not Idle)
fn detect_active_agent_state(
    content: &str,
    tail: &str,
    running_patterns: &[&str],
    waiting_patterns: &[&str],
    idle_tail_patterns: &[&str],
) -> AgentState {
    // Running indicators in the tail override idle — the agent is actively working
    // even if the idle footer is also visible (Codex keeps it during tool execution).
    if matches_any(tail, running_patterns) || contains_braille_spinner(tail) {
        return AgentState::Running;
    }
    if matches_any(tail, idle_tail_patterns) {
        return AgentState::Idle;
    }
    if matches_any(content, running_patterns) || contains_braille_spinner(content) {
        return AgentState::Running;
    }
    if matches_any(content, waiting_patterns) {
        return AgentState::Waiting;
    }
    AgentState::Running
}

/// Generic agent state detection.
///
/// 1. Check `idle_tail_patterns` against the tail (last ~5 lines) — if found,
///    return Idle immediately. This prevents stale waiting/running text from
///    earlier in the terminal buffer from causing false positives.
/// 2. Check running patterns + braille spinners against the full content window.
/// 3. Check waiting patterns against the full content window.
/// 4. Default to Idle.
fn detect_agent_state(
    content: &str,
    tail: &str,
    running_patterns: &[&str],
    waiting_patterns: &[&str],
    idle_tail_patterns: &[&str],
) -> AgentState {
    if matches_any(tail, idle_tail_patterns) {
        return AgentState::Idle;
    }
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
    fn test_detect_agent_kind_cursor_agent_in_command() {
        assert_eq!(
            detect_agent_kind("cursor-agent", None),
            AgentKind::CursorAgent
        );
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
        assert_eq!(
            detect_agent_kind(
                "node",
                Some("/home/user/.cursor-agent/versions/0.1.0/index.js")
            ),
            AgentKind::CursorAgent
        );
    }

    #[test]
    fn test_detect_agent_kind_unknown() {
        assert_eq!(detect_agent_kind("bash", None), AgentKind::Unknown);
        assert_eq!(
            detect_agent_kind("vim", Some("vim file.txt")),
            AgentKind::Unknown
        );
        // "agent" alone is too generic — should not match
        assert_eq!(detect_agent_kind("agent", None), AgentKind::Unknown);
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
            detect_state("❯ \n? for shortcuts", AgentKind::ClaudeCode),
            AgentState::Idle
        );
        assert_eq!(
            detect_state(
                "Welcome to Claude Code\n❯ Try \"fix errors\"\n? for shortcuts",
                AgentKind::ClaudeCode
            ),
            AgentState::Idle
        );
    }

    #[test]
    fn test_claude_processing_no_indicators() {
        // When Claude is processing on alt-screen, no idle footer is visible.
        // This should be detected as Running, not Idle.
        assert_eq!(
            detect_state(
                "❯ what files are in this directory?\n\n",
                AgentKind::ClaudeCode
            ),
            AgentState::Running
        );
        // Empty pane (just started up, loading) should also be Running
        assert_eq!(detect_state("", AgentKind::ClaudeCode), AgentState::Running);
        assert_eq!(
            detect_state("$ ", AgentKind::ClaudeCode),
            AgentState::Running
        );
    }

    #[test]
    fn test_codex_running() {
        assert_eq!(
            detect_state("⠋ Searching codebase\nesc to interrupt", AgentKind::Codex),
            AgentState::Running
        );
        assert_eq!(
            detect_state("⠙ Processing your question", AgentKind::Codex),
            AgentState::Running
        );
    }

    #[test]
    fn test_codex_waiting() {
        assert_eq!(
            detect_state(
                "Would you like to run the following command?\n$ touch test.txt\n› 1. Yes, proceed (y)",
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
    fn test_codex_trust_prompt() {
        // Real Codex trust prompt shown on first launch in an untrusted directory
        let content = "\
> You are in /tmp

  Do you trust the contents of this directory? Working with untrusted contents comes with higher risk of prompt
  injection.

› 1. Yes, continue
  2. No, quit

  Press enter to continue";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Waiting);
    }

    #[test]
    fn test_codex_idle() {
        assert_eq!(
            detect_state("› Type a message\n\n  ? for shortcuts", AgentKind::Codex),
            AgentState::Idle
        );
        assert_eq!(
            detect_state(
                "Codex ready\n› Type a message\n  ? for shortcuts",
                AgentKind::Codex
            ),
            AgentState::Idle
        );
    }

    #[test]
    fn test_codex_processing_no_indicators() {
        // When Codex is waiting for an API response, no idle footer is visible.
        // This should be detected as Running, not Idle.
        assert_eq!(
            detect_state(
                "› Review main.py and find all the bugs\n\n  100% context left",
                AgentKind::Codex
            ),
            AgentState::Running
        );
        // Just "> " with no "? for shortcuts" → Running
        assert_eq!(detect_state("> ", AgentKind::Codex), AgentState::Running);
    }

    #[test]
    fn test_codex_working_indicator() {
        // Real Codex "Working" indicator observed during tool execution.
        // Both "esc to interrupt" and "? for shortcuts" are visible simultaneously.
        // Running patterns in the tail take priority over idle patterns.
        let content = "\
› hi

• Working (2s • esc to interrupt)

› Use /skills to list available skills

  ? for shortcuts                                                                                    100% context left";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Running);
    }

    #[test]
    fn test_codex_real_idle_with_context_left() {
        // Real Codex idle screen as captured from tmux
        let content = "\
╭─────────────────────────────────────────────────────╮
│ >_ OpenAI Codex (v0.104.0)                          │
│                                                     │
│ model:     gpt-5.3-codex   /model to change         │
│ directory: /tmp/test-workspace                      │
╰─────────────────────────────────────────────────────╯

  Tip: New 2x rate limits until April 2nd.

› Write tests for @filename

  ? for shortcuts                                                                                    100% context left";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn test_codex_real_api_call_phase() {
        // Real Codex screen during initial API call — no indicators at all,
        // just the query text and "100% context left"
        let content = "\
╭─────────────────────────────────────────────────────╮
│ >_ OpenAI Codex (v0.104.0)                          │
│                                                     │
│ model:     gpt-5.3-codex   /model to change         │
│ directory: /tmp/test-workspace                      │
╰─────────────────────────────────────────────────────╯

  Tip: New 2x rate limits until April 2nd.

› Review main.py and find all the bugs

                                                                                                     100% context left";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Running);
    }

    #[test]
    fn test_codex_update_prompt() {
        // Real Codex update prompt
        let content = "\
✨ Update available! 0.104.0 -> 0.105.0

  Release notes: https://github.com/openai/codex/releases/latest

› 1. Update now (runs `npm install -g @openai/codex`)
  2. Skip
  3. Skip until next version

  Press enter to continue";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Waiting);
    }

    #[test]
    fn test_codex_idle_tail_overrides_stale_waiting() {
        // After answering a permission prompt, stale "Yes, proceed" / "Press enter to confirm"
        // text remains in the buffer. The idle tail pattern should override it.
        let content = "\
Would you like to run the following command?
$ touch test.txt
› 1. Yes, proceed (y)
  2. Yes, and don't ask again (p)
  3. No (esc)

  Press enter to confirm or esc to cancel
╭──────────────────────────────╮
│ >_ OpenAI Codex (v0.104.0)   │
╰──────────────────────────────╯

› Type a message

  ? for shortcuts";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn test_codex_idle_tail_overrides_stale_running() {
        // After a task completes, stale "esc to interrupt" text may remain in
        // the scrollback but scrolled above the tail window. The idle tail
        // pattern should override it.
        let content = "\
• Working (5s • esc to interrupt)

• Ran rm hello.py
  └ (no output)

• Completed.
  - Deleted hello.py

› Type a message

  ? for shortcuts";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn test_codex_current_working_directory_not_running() {
        // "current working directory" contains "working" — should NOT trigger Running
        // now that "working" has been removed from CODEX_RUNNING_PATTERNS.
        let content = "current working directory: /home/user/project\n\n› Type a message\n\n  ? for shortcuts";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    // -- Cursor Agent state detection ----------------------------------------

    #[test]
    fn test_cursor_running() {
        assert_eq!(
            detect_state(
                "⠋ Editing file src/main.rs\nesc to interrupt",
                AgentKind::CursorAgent
            ),
            AgentState::Running
        );
        assert_eq!(
            detect_state("Processing... ctrl+c to interrupt", AgentKind::CursorAgent),
            AgentState::Running
        );
    }

    #[test]
    fn test_cursor_waiting() {
        assert_eq!(
            detect_state(
                "Do you trust the contents of this directory?\n\n▶ [a] Trust this workspace",
                AgentKind::CursorAgent
            ),
            AgentState::Waiting
        );
        assert_eq!(
            detect_state(
                "Use arrow keys to navigate, Enter to select\nDo you trust this workspace?",
                AgentKind::CursorAgent
            ),
            AgentState::Waiting
        );
    }

    #[test]
    fn test_cursor_idle() {
        assert_eq!(detect_state("> ", AgentKind::CursorAgent), AgentState::Idle);
    }

    // -- Unknown kind returns Idle -------------------------------------------

    #[test]
    fn test_unknown_always_idle() {
        assert_eq!(
            detect_state("esc to interrupt", AgentKind::Unknown),
            AgentState::Idle
        );
        assert_eq!(detect_state("(Y/n)", AgentKind::Unknown), AgentState::Idle);
        assert_eq!(detect_state("", AgentKind::Unknown), AgentState::Idle);
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
        // Claude and Codex default to Running when no patterns match
        assert_eq!(detect_state("", AgentKind::ClaudeCode), AgentState::Running);
        assert_eq!(detect_state("", AgentKind::Codex), AgentState::Running);
        assert_eq!(detect_state("", AgentKind::Unknown), AgentState::Idle);
    }

    #[test]
    fn test_only_whitespace_content() {
        // Claude defaults to Running on empty/whitespace content (alt-screen)
        assert_eq!(
            detect_state("   \n\n  \t  ", AgentKind::ClaudeCode),
            AgentState::Running
        );
    }
}
