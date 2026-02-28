use super::{AgentKind, AgentState};

// ===========================================================================
// Agent kind detection
// ===========================================================================

/// Detect the kind of agent from tmux pane command or child process arguments.
///
/// Order matters: more specific patterns are checked first to avoid false positives
/// (e.g. "cursor-agent" before "agent", "codex" before generic patterns).
const AGENT_PATTERNS: &[(&str, AgentKind)] = &[
    ("cursor-agent", AgentKind::CursorAgent),
    ("opencode", AgentKind::OpenCode),
    ("claude", AgentKind::ClaudeCode),
    ("codex", AgentKind::Codex),
    ("gemini", AgentKind::Gemini),
];

pub fn detect_agent_kind(
    pane_command: &str,
    child_process_args: Option<&str>,
) -> Option<AgentKind> {
    let cmd_lower = pane_command.to_lowercase();
    for &(pattern, kind) in AGENT_PATTERNS {
        if cmd_lower.contains(pattern) {
            return Some(kind);
        }
    }

    if let Some(args) = child_process_args {
        let args_lower = args.to_lowercase();
        for &(pattern, kind) in AGENT_PATTERNS {
            if args_lower.contains(pattern) {
                return Some(kind);
            }
        }
    }

    None
}

// ===========================================================================
// Pattern definitions — per-agent
//
// Organised as structs to keep each agent's patterns self-contained and make
// it easy to add new agents. Inspired by Agent of Empires' `AgentDef` registry
// (<https://github.com/njbrake/agent-of-empires>).
// ===========================================================================

struct AgentPatterns {
    running: &'static [&'static str],
    waiting: &'static [&'static str],
    idle_tail: &'static [&'static str],
}

// -- Claude Code --------------------------------------------------------------

const CLAUDE_PATTERNS: AgentPatterns = AgentPatterns {
    running: &["esc to interrupt", "ctrl+c to interrupt"],
    waiting: &[
        "yes, allow",
        "yes, and always allow",
        "yes, and don't ask again",
        "allow once",
        "allow always",
        "(y/n)",
        "[y/n]",
        "enter to select",
        "enter to confirm",
        "esc to cancel",
        "esc to exit",
        "❯ 1.",
        "do you trust the files",
    ],
    // `? for shortcuts` is the canonical idle indicator but is NOT reliably
    // captured by `tmux capture-pane` in Claude Code >= v2.1 (rendered as a
    // status-bar element outside the normal text flow). We fall back to
    // detecting the input prompt character `❯` in `detect_claude_state`.
    idle_tail: &["? for shortcuts"],
};

/// Fallback idle patterns for Claude Code: the input prompt `❯` appears on
/// its own line when idle. Checked against only the last 3 non-empty lines
/// to reduce false Idle during the brief processing transition.
const CLAUDE_IDLE_PROMPT_PATTERNS: &[&str] = &["❯"];

/// Claude Code's whimsical "thinking" words shown during processing.
///
/// When Claude is working, it shows messages like `✦ Noodling… 42 tokens`
/// with rotating verbs. Matching these provides a secondary Running signal
/// that doesn't rely on "esc to interrupt" appearing.
///
/// Inspired by agent-os's `WHIMSICAL_WORDS` detection
/// (<https://github.com/saadnvd1/agent-os>).
const CLAUDE_THINKING_WORDS: &[&str] = &[
    "accomplishing",
    "actioning",
    "actualizing",
    "baking",
    "booping",
    "brewing",
    "calculating",
    "cerebrating",
    "channelling",
    "churning",
    "clauding",
    "coalescing",
    "cogitating",
    "combobulating",
    "computing",
    "concocting",
    "conjuring",
    "considering",
    "contemplating",
    "cooking",
    "crafting",
    "creating",
    "crunching",
    "deciphering",
    "deliberating",
    "determining",
    "discombulating",
    "divining",
    "doing",
    "effecting",
    "elucidating",
    "enchanting",
    "envisioning",
    "finagling",
    "flibbertigibbeting",
    "forging",
    "forming",
    "frolicking",
    "generating",
    "germinating",
    "hatching",
    "herding",
    "honking",
    "hustling",
    "ideating",
    "imagining",
    "incubating",
    "inferring",
    "jiving",
    "manifesting",
    "marinating",
    "meandering",
    "moseying",
    "mulling",
    "mustering",
    "musing",
    "noodling",
    "percolating",
    "perusing",
    "philosophising",
    "pondering",
    "pontificating",
    "processing",
    "puttering",
    "puzzling",
    "reticulating",
    "ruminating",
    "scheming",
    "schlepping",
    "shimmying",
    "shucking",
    "simmering",
    "smooshing",
    "spelunking",
    "spinning",
    "stewing",
    "sussing",
    "synthesizing",
    "thinking",
    "tinkering",
    "transmuting",
    "unfurling",
    "unravelling",
    "vibing",
    "wandering",
    "whirring",
    "wibbling",
    "wizarding",
    "working",
    "wrangling",
];

// -- Codex --------------------------------------------------------------------

const CODEX_PATTERNS: AgentPatterns = AgentPatterns {
    running: &["esc to interrupt"],
    waiting: &[
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
    ],
    // When Codex is idle, `? for shortcuts` appears at the bottom of the prompt.
    idle_tail: &["? for shortcuts"],
};

// -- Cursor Agent -------------------------------------------------------------

const CURSOR_PATTERNS: AgentPatterns = AgentPatterns {
    // Cursor CLI is built on Claude Code, so shares the same running signals.
    // Inspired by `AoE`'s `detect_cursor_status` which delegates to Claude detection.
    running: &["esc to interrupt", "ctrl+c to interrupt"],
    waiting: &[
        "do you trust",
        "trust this workspace",
        "enter to select",
        "(y/n)",
        "[y/n]",
        "esc to cancel",
    ],
    idle_tail: &[],
};

// -- OpenCode -----------------------------------------------------------------

const OPENCODE_PATTERNS: AgentPatterns = AgentPatterns {
    // The `esc interrupt` text appears in the footer alongside the block
    // spinner `⬝■` during active work.
    running: &["esc interrupt"],
    // OpenCode currently auto-approves in Build mode and Plan mode is
    // read-only, so there are no user-facing approval prompts.
    waiting: &[],
    // OpenCode's idle footer shows `ctrl+p commands` when at the input prompt.
    idle_tail: &["ctrl+p commands", "ctrl+t variants", "tab agents"],
};

// -- Gemini CLI ---------------------------------------------------------------

const GEMINI_PATTERNS: AgentPatterns = AgentPatterns {
    // Gemini CLI running indicators.
    // Patterns drawn from Agent of Empires' `detect_gemini_status`
    // (<https://github.com/njbrake/agent-of-empires>).
    running: &["esc to interrupt", "ctrl+c to interrupt"],
    // Gemini CLI approval prompts.
    // Based on `AoE`'s detection + Gemini CLI docs.
    waiting: &[
        "(y/n)",
        "[y/n]",
        "allow",
        "approve",
        "execute?",
        "enter to select",
        "esc to cancel",
    ],
    idle_tail: &[],
};

// ===========================================================================
// Braille spinners (shared across all agents)
// ===========================================================================

const BRAILLE_SPINNERS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ===========================================================================
// State detection — public entry point
// ===========================================================================

/// Detect agent state from terminal content, dispatching to agent-specific detectors.
/// Content is ANSI-stripped and lowercased once here; per-agent functions receive clean input.
pub fn detect_state(content: &str, kind: AgentKind) -> AgentState {
    let clean = strip_ansi_codes(content);
    let last_30 = get_last_non_empty_lines(&clean, 30);
    let last_5 = get_last_non_empty_lines(&clean, 5);
    let content_lowered = last_30.to_lowercase();
    let tail_lowered = last_5.to_lowercase();

    match kind {
        AgentKind::ClaudeCode => {
            // Claude prompt fallback uses a tighter window (last 3 lines) to
            // reduce false Idle during the brief processing transition when
            // the user's question line (containing ❯) is still near the bottom.
            let prompt_tail = get_last_non_empty_lines(&clean, 3).to_lowercase();
            detect_claude_state(&content_lowered, &tail_lowered, &prompt_tail)
        }
        AgentKind::Codex => detect_codex_state(&content_lowered, &tail_lowered),
        AgentKind::CursorAgent => {
            detect_generic_state(&content_lowered, &tail_lowered, &CURSOR_PATTERNS)
        }
        AgentKind::OpenCode => detect_opencode_state(&content_lowered, &tail_lowered),
        AgentKind::Gemini => {
            detect_generic_state(&content_lowered, &tail_lowered, &GEMINI_PATTERNS)
        }
    }
}

// ===========================================================================
// Per-agent state detection
// ===========================================================================

/// Claude Code-specific state detection.
///
/// Claude Code uses the alternate screen, so there is no scrollback — the
/// captured pane content IS what the user sees. When Claude is processing,
/// the pane shows the user's prompt and empty lines but NO running indicators
/// (no "esc to interrupt", no spinners).
///
/// `? for shortcuts` is the ideal idle signal but isn't reliably captured by
/// tmux. As a fallback, the input prompt `❯` on the last non-empty line
/// (with no running/waiting indicators elsewhere) indicates idle.
fn detect_claude_state(content: &str, tail: &str, prompt_tail: &str) -> AgentState {
    // First try the standard detection (handles running, waiting, and
    // the `? for shortcuts` idle pattern if tmux happens to capture it).
    let state = detect_active_state(content, tail, &CLAUDE_PATTERNS);
    if state != AgentState::Unknown {
        return state;
    }

    // Secondary running signal: Claude's whimsical "thinking" words.
    // These appear as `✦ Noodling… 42 tokens` during processing.
    // Inspired by agent-os (https://github.com/saadnvd1/agent-os).
    if contains_thinking_word(tail) {
        return AgentState::Running;
    }

    // Fallback: check if any line in a tight prompt window (last 3 lines)
    // starts with the input prompt character `❯`. Using 3 lines instead of
    // the broader tail (5 lines) reduces false Idle during the brief
    // processing transition when the user's question (containing `❯`)
    // hasn't scrolled out of the tail yet.
    if prompt_tail.lines().any(|line| {
        let trimmed = line.trim_start();
        CLAUDE_IDLE_PROMPT_PATTERNS
            .iter()
            .any(|p| trimmed.starts_with(p))
    }) {
        return AgentState::Idle;
    }

    AgentState::Unknown
}

/// Codex-specific state detection.
///
/// Codex does NOT use the alternate screen, so old content persists in the
/// scrollback. This means stale waiting/running text from earlier prompts
/// can cause false positives if checked against the full content window.
///
/// Key insight: waiting patterns are only checked against the **tail** (last
/// 5 lines), not the full 30-line window, to avoid matching stale prompts
/// like "Press enter to continue" from trust dialogs that are still in the
/// scrollback above. Running patterns in the tail are reliable regardless.
fn detect_codex_state(content: &str, tail: &str) -> AgentState {
    // For Codex (no alt-screen), the idle footer `? for shortcuts` is the
    // single most reliable signal — it's only visible when Codex is truly
    // at the input prompt. Check it FIRST to override any stale running/
    // waiting text that may linger in the scrollback or even the tail.
    if matches_any(tail, CODEX_PATTERNS.idle_tail) {
        // Exception: if there's also a running indicator in the tail,
        // Codex is actively working (it shows both simultaneously during
        // tool execution). Running + idle footer = Running.
        if matches_any(tail, CODEX_PATTERNS.running) || contains_braille_spinner(tail) {
            return AgentState::Running;
        }
        return AgentState::Idle;
    }
    // No idle footer → check running indicators
    if matches_any(tail, CODEX_PATTERNS.running) || contains_braille_spinner(tail) {
        return AgentState::Running;
    }
    if matches_any(content, CODEX_PATTERNS.running) || contains_braille_spinner(content) {
        return AgentState::Running;
    }
    // IMPORTANT: Only check waiting patterns in the **tail** to avoid stale
    // scrollback false positives (e.g. old trust prompts, update dialogs
    // that remain in the buffer after being dismissed).
    if matches_any(tail, CODEX_PATTERNS.waiting) {
        return AgentState::Waiting;
    }
    AgentState::Unknown
}

/// OpenCode-specific state detection.
///
/// `OpenCode` uses the alternate screen (TUI app). It shows `esc interrupt`
/// during active work and `ctrl+p commands` in the idle footer. Like Claude,
/// it may not always capture the footer text reliably via tmux, so we fall
/// back to detecting the input prompt bar (`┃`) with the agent label
/// (e.g. `Build`) in the tail when no other indicators match.
fn detect_opencode_state(content: &str, tail: &str) -> AgentState {
    let state = detect_active_state(content, tail, &OPENCODE_PATTERNS);
    if state != AgentState::Unknown {
        return state;
    }

    // Fallback: OpenCode's input prompt shows a vertical bar `┃` followed
    // by the agent mode label (e.g. "Build", "Plan"). If we see this in the
    // tail without running indicators, the agent is idle at the prompt.
    if tail
        .lines()
        .any(|line| line.contains('┃') || line.contains('╹'))
    {
        return AgentState::Idle;
    }

    AgentState::Unknown
}

// ===========================================================================
// Shared detection logic
// ===========================================================================

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
/// 5. Default → **Unknown** (no recognisable pattern matched)
fn detect_active_state(content: &str, tail: &str, patterns: &AgentPatterns) -> AgentState {
    if matches_any(tail, patterns.running) || contains_braille_spinner(tail) {
        return AgentState::Running;
    }
    if matches_any(tail, patterns.idle_tail) {
        return AgentState::Idle;
    }
    if matches_any(content, patterns.running) || contains_braille_spinner(content) {
        return AgentState::Running;
    }
    if matches_any(content, patterns.waiting) {
        return AgentState::Waiting;
    }
    AgentState::Unknown
}

/// Generic agent state detection (for agents like Cursor, Gemini where we
/// don't have a strong "absence of idle = running" signal).
///
/// 1. Check `idle_tail` patterns against the tail — if found, Idle.
/// 2. Check running patterns + braille spinners against full content.
/// 3. Check waiting patterns against full content.
/// 4. Default to Idle.
fn detect_generic_state(content: &str, tail: &str, patterns: &AgentPatterns) -> AgentState {
    if matches_any(tail, patterns.idle_tail) {
        return AgentState::Idle;
    }
    if matches_any(content, patterns.running) || contains_braille_spinner(content) {
        return AgentState::Running;
    }
    if matches_any(content, patterns.waiting) {
        return AgentState::Waiting;
    }
    AgentState::Idle
}

// ===========================================================================
// Pattern matchers
// ===========================================================================

fn matches_any(content: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| content.contains(p))
}

fn contains_braille_spinner(content: &str) -> bool {
    content.chars().any(|c| BRAILLE_SPINNERS.contains(&c))
}

/// Check if content contains a Claude "thinking" word followed by `…` or `...`.
///
/// Claude shows status like `✦ Noodling… 42 tokens` during processing.
/// We look for `<word>…` or `<word>...` to avoid false positives on normal
/// English text that might contain words like "working" or "processing".
///
/// Inspired by agent-os (<https://github.com/saadnvd1/agent-os>).
fn contains_thinking_word(content: &str) -> bool {
    CLAUDE_THINKING_WORDS.iter().any(|word| {
        content.contains(&format!("{word}…")) || content.contains(&format!("{word}..."))
    })
}

// ===========================================================================
// Text helpers
// ===========================================================================

/// Strip ANSI escape codes from terminal content without regex.
/// Handles CSI (`ESC [`) sequences, OSC (`ESC ]`) sequences (terminated by
/// BEL `\x07` or ST `ESC \`), and unknown two-byte `ESC X` sequences.
fn strip_ansi_codes(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1B' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if c.is_ascii() && (0x40..=0x7E).contains(&(c as u8)) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    loop {
                        match chars.next() {
                            None | Some('\x07') => break,
                            Some('\x1B') => {
                                if chars.peek() == Some(&'\\') {
                                    chars.next();
                                }
                                break;
                            }
                            Some(_) => {}
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Get the last N non-empty lines from content.
fn get_last_non_empty_lines(content: &str, count: usize) -> String {
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    let start_idx = lines.len().saturating_sub(count);
    lines[start_idx..].join("\n")
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- detect_agent_kind ---------------------------------------------------

    #[test]
    fn detect_kind_claude_in_command() {
        assert_eq!(
            detect_agent_kind("claude", None),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(
            detect_agent_kind("Claude Code", None),
            Some(AgentKind::ClaudeCode),
        );
    }

    #[test]
    fn detect_kind_codex_in_command() {
        assert_eq!(detect_agent_kind("codex", None), Some(AgentKind::Codex));
    }

    #[test]
    fn detect_kind_cursor_agent_in_command() {
        assert_eq!(
            detect_agent_kind("cursor-agent", None),
            Some(AgentKind::CursorAgent),
        );
    }

    #[test]
    fn detect_kind_opencode_in_command() {
        assert_eq!(
            detect_agent_kind("opencode", None),
            Some(AgentKind::OpenCode)
        );
    }

    #[test]
    fn detect_kind_gemini_in_command() {
        assert_eq!(detect_agent_kind("gemini", None), Some(AgentKind::Gemini));
        assert_eq!(
            detect_agent_kind("gemini-cli", None),
            Some(AgentKind::Gemini)
        );
    }

    #[test]
    fn detect_kind_child_process() {
        assert_eq!(
            detect_agent_kind("bash", Some("python claude_main.py")),
            Some(AgentKind::ClaudeCode),
        );
        assert_eq!(
            detect_agent_kind("node", Some("/usr/bin/codex --version")),
            Some(AgentKind::Codex),
        );
        assert_eq!(
            detect_agent_kind(
                "node",
                Some("/home/user/.cursor-agent/versions/0.1.0/index.js")
            ),
            Some(AgentKind::CursorAgent),
        );
        assert_eq!(
            detect_agent_kind("node", Some("/home/user/.local/bin/opencode")),
            Some(AgentKind::OpenCode),
        );
        assert_eq!(
            detect_agent_kind("node", Some("/usr/bin/gemini serve")),
            Some(AgentKind::Gemini),
        );
    }

    #[test]
    fn detect_kind_unknown() {
        assert_eq!(detect_agent_kind("bash", None), None);
        assert_eq!(detect_agent_kind("vim", Some("vim file.txt")), None);
        assert_eq!(detect_agent_kind("agent", None), None);
    }

    // -- Claude Code ---------------------------------------------------------

    #[test]
    fn claude_running() {
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
    fn claude_running_whimsical_words() {
        // Inspired by agent-os: Claude shows `✦ <word>… N tokens` during processing
        assert_eq!(
            detect_state("✦ Noodling… 42 tokens", AgentKind::ClaudeCode),
            AgentState::Running,
        );
        assert_eq!(
            detect_state("✦ Cogitating… 128 tokens", AgentKind::ClaudeCode),
            AgentState::Running,
        );
        assert_eq!(
            detect_state("✦ Spelunking... 7 tokens", AgentKind::ClaudeCode),
            AgentState::Running,
        );
        assert_eq!(
            detect_state("✦ Clauding… 3 tokens", AgentKind::ClaudeCode),
            AgentState::Running,
        );
    }

    #[test]
    fn claude_thinking_word_requires_ellipsis() {
        // Plain word without ellipsis should NOT trigger Running
        assert_ne!(
            detect_state("I was thinking about it", AgentKind::ClaudeCode),
            AgentState::Running,
        );
        assert_ne!(
            detect_state("Processing the request", AgentKind::ClaudeCode),
            AgentState::Running,
        );
    }

    #[test]
    fn claude_waiting() {
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
    fn claude_idle() {
        assert_eq!(
            detect_state("❯ \n? for shortcuts", AgentKind::ClaudeCode),
            AgentState::Idle
        );
    }

    #[test]
    fn claude_idle_prompt_fallback_without_shortcuts() {
        assert_eq!(
            detect_state(
                " ▐▛███▜▌   Claude Code v2.1.59\n                 ▝▜█████▛▘  Opus 4.6 · Claude Max\n                   ▘▘ ▝▝    ~/Development/kiosk\n                 \n                 ────────────\n                 ❯ Try \"create a util logging.py that...\"\n                 ────────────\n                   PR #12",
                AgentKind::ClaudeCode
            ),
            AgentState::Idle
        );
    }

    #[test]
    fn claude_idle_prompt_bare() {
        assert_eq!(detect_state("❯ ", AgentKind::ClaudeCode), AgentState::Idle);
    }

    #[test]
    fn claude_prompt_not_idle_when_running() {
        assert_eq!(
            detect_state(
                "❯ my question\nProcessing... esc to interrupt",
                AgentKind::ClaudeCode
            ),
            AgentState::Running
        );
    }

    #[test]
    fn claude_prompt_not_idle_when_waiting() {
        assert_eq!(
            detect_state(
                "❯ my question\nAllow write?\n  Yes, allow\n  No, deny",
                AgentKind::ClaudeCode
            ),
            AgentState::Waiting
        );
    }

    #[test]
    fn claude_idle_real_capture_after_response() {
        let content = "\
 ▐▛███▜▌   Claude Code v2.1.59\n\
▝▜█████▛▘  Opus 4.6 · Claude Max\n\
  ▘▘ ▝▝    ~/Development/kiosk\n\
\n\
❯ what is 2+2? reply with just the number\n\
\n\
● 4\n\
\n\
────────────────────────────────────────\n\
❯ \n\
────────────────────────────────────────\n\
  PR #12";
        assert_eq!(
            detect_state(content, AgentKind::ClaudeCode),
            AgentState::Idle
        );
    }

    #[test]
    fn claude_processing_no_indicators() {
        assert_eq!(detect_state("", AgentKind::ClaudeCode), AgentState::Unknown);
        assert_eq!(
            detect_state("$ ", AgentKind::ClaudeCode),
            AgentState::Unknown
        );
    }

    #[test]
    fn claude_processing_prompt_scrolled_out() {
        let content = "\
 ▐▛███▜▌   Claude Code v2.1.59\n\
▝▜█████▛▘  Opus 4.6 · Claude Max\n\
  ▘▘ ▝▝    ~/Development/kiosk\n\
\n\
❯ refactor the config module\n\
\n\
● I'll start by reading the current config module to understand\n\
  the current structure.\n\
\n\
  Read kiosk-core/src/config/mod.rs\n\
  Read kiosk-core/src/config/theme.rs\n\
  Read kiosk-core/src/config/keys.rs\n\
  Reading kiosk-core/src/agent/mod.rs";
        assert_eq!(
            detect_state(content, AgentKind::ClaudeCode),
            AgentState::Unknown,
        );
    }

    // -- Codex ---------------------------------------------------------------

    #[test]
    fn codex_running() {
        assert_eq!(
            detect_state("⠋ Searching codebase\nesc to interrupt", AgentKind::Codex),
            AgentState::Running
        );
    }

    #[test]
    fn codex_waiting() {
        assert_eq!(
            detect_state(
                "Would you like to run?\n$ touch test.txt\n› 1. Yes, proceed (y)",
                AgentKind::Codex
            ),
            AgentState::Waiting
        );
    }

    #[test]
    fn codex_trust_prompt() {
        let content =
            "> You are in /tmp\n\n› 1. Yes, continue\n  2. No, quit\n\n  Press enter to continue";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Waiting);
    }

    #[test]
    fn codex_idle() {
        assert_eq!(
            detect_state("› Type a message\n\n  ? for shortcuts", AgentKind::Codex),
            AgentState::Idle
        );
    }

    #[test]
    fn codex_processing_no_indicators() {
        assert_eq!(
            detect_state(
                "› Review main.py and find all the bugs\n\n  100% context left",
                AgentKind::Codex
            ),
            AgentState::Unknown
        );
    }

    #[test]
    fn codex_working_indicator() {
        let content = "› hi\n\n• Working (2s • esc to interrupt)\n\n  ? for shortcuts                                                                                    100% context left";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Running);
    }

    #[test]
    fn codex_idle_tail_overrides_stale_waiting() {
        let content = "› 1. Yes, proceed (y)\n  Press enter to confirm\n\n› Type a message\n\n  ? for shortcuts";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn codex_idle_tail_overrides_stale_running() {
        let content = "• Working (5s • esc to interrupt)\n\n• Ran rm hello.py\n  └ (no output)\n\n• Completed.\n  - Deleted hello.py\n\n› Type a message\n\n  ? for shortcuts";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    // -- Cursor Agent --------------------------------------------------------

    #[test]
    fn cursor_running() {
        assert_eq!(
            detect_state(
                "⠋ Editing file src/main.rs\nesc to interrupt",
                AgentKind::CursorAgent
            ),
            AgentState::Running
        );
    }

    #[test]
    fn cursor_waiting() {
        assert_eq!(
            detect_state(
                "Do you trust the contents of this directory?\n\n▶ [a] Trust this workspace",
                AgentKind::CursorAgent
            ),
            AgentState::Waiting
        );
    }

    #[test]
    fn cursor_idle() {
        assert_eq!(detect_state("> ", AgentKind::CursorAgent), AgentState::Idle);
    }

    // -- OpenCode ------------------------------------------------------------

    #[test]
    fn opencode_running() {
        let content = "  ┃  Build  GPT-5.3\n  ╹▀▀▀▀\n   ⬝⬝■■■■■■  esc interrupt  ctrl+p commands";
        assert_eq!(
            detect_state(content, AgentKind::OpenCode),
            AgentState::Running
        );
    }

    #[test]
    fn opencode_idle_with_footer() {
        let content =
            "  ┃  Build  GPT-5.3\n  ╹▀▀▀▀\n  ctrl+t variants  tab agents  ctrl+p commands";
        assert_eq!(detect_state(content, AgentKind::OpenCode), AgentState::Idle);
    }

    #[test]
    fn opencode_idle_fallback_prompt_bar() {
        let content = "  ┃\n  ┃  Build  GPT-5.3\n  ╹▀▀▀▀";
        assert_eq!(detect_state(content, AgentKind::OpenCode), AgentState::Idle);
    }

    #[test]
    fn opencode_unknown_empty() {
        assert_eq!(detect_state("", AgentKind::OpenCode), AgentState::Unknown);
    }

    // -- Gemini CLI ----------------------------------------------------------

    #[test]
    fn gemini_running() {
        assert_eq!(
            detect_state(
                "Working on your request\nesc to interrupt",
                AgentKind::Gemini
            ),
            AgentState::Running,
        );
        assert_eq!(
            detect_state("Processing ⠋", AgentKind::Gemini),
            AgentState::Running,
        );
    }

    #[test]
    fn gemini_waiting() {
        assert_eq!(
            detect_state("run this command? (y/n)", AgentKind::Gemini),
            AgentState::Waiting,
        );
        assert_eq!(
            detect_state("approve changes?", AgentKind::Gemini),
            AgentState::Waiting,
        );
    }

    #[test]
    fn gemini_idle() {
        assert_eq!(
            detect_state("some random output", AgentKind::Gemini),
            AgentState::Idle,
        );
    }

    // -- Cross-agent ---------------------------------------------------------

    #[test]
    fn braille_spinner_all_chars() {
        for spinner in BRAILLE_SPINNERS {
            let content = format!("Loading {spinner} please wait");
            assert_eq!(
                detect_state(&content, AgentKind::ClaudeCode),
                AgentState::Running,
            );
        }
    }

    #[test]
    fn case_insensitive() {
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
    fn empty_content_unknown() {
        assert_eq!(detect_state("", AgentKind::ClaudeCode), AgentState::Unknown);
        assert_eq!(detect_state("", AgentKind::Codex), AgentState::Unknown);
    }

    #[test]
    fn ansi_codes_stripped() {
        assert_eq!(
            detect_state(
                "\x1B[32mProcessing...\x1B[0m esc to interrupt",
                AgentKind::ClaudeCode
            ),
            AgentState::Running
        );
    }

    // -- Text helpers --------------------------------------------------------

    #[test]
    fn strip_ansi_codes_basic() {
        assert_eq!(strip_ansi_codes("\x1B[32mGreen\x1B[0m"), "Green");
        assert_eq!(strip_ansi_codes("Normal"), "Normal");
    }

    #[test]
    fn strip_ansi_codes_osc() {
        assert_eq!(strip_ansi_codes("\x1B]0;title\x07text"), "text");
        assert_eq!(strip_ansi_codes("\x1B]0;title\x1B\\text"), "text");
    }

    #[test]
    fn get_last_non_empty_lines_basic() {
        let content = "Line 1\n\nLine 3\n\nLine 5\nLine 6\n\n";
        assert_eq!(get_last_non_empty_lines(content, 2), "Line 5\nLine 6");
    }

    // -- Thinking word -------------------------------------------------------

    #[test]
    fn thinking_word_with_ellipsis() {
        assert!(contains_thinking_word("noodling…"));
        assert!(contains_thinking_word("✦ cogitating… 42 tokens"));
        assert!(contains_thinking_word("spelunking..."));
    }

    #[test]
    fn thinking_word_without_ellipsis_no_match() {
        assert!(!contains_thinking_word("noodling"));
        assert!(!contains_thinking_word("I was thinking about it"));
    }
}

// -- Codex stale content regression tests --------------------------------

#[test]
fn codex_stale_trust_prompt_not_false_waiting() {
    // Real scenario: After dismissing the trust prompt and update dialog,
    // stale text like "› 1. Yes, continue" and "Press enter to continue"
    // remains in the scrollback. When Codex is processing a query, these
    // should NOT cause a false Waiting state.
    //
    // This was a real bug found during manual testing (2026-02-28).
    let content = "\
> You are in /home/user/project\n\
\n\
  Do you trust the contents of this directory?\n\
\n\
› 1. Yes, continue\n\
  2. No, quit\n\
\n\
  Press enter to continue\n\
\n\
╭─────────────────────────────────────────────╮\n\
│ >_ OpenAI Codex (v0.104.0)                  │\n\
╰─────────────────────────────────────────────╯\n\
\n\
› what is 2+2?\n\
\n\
                                                  100% context left";
    assert_eq!(
        detect_state(content, AgentKind::Codex),
        AgentState::Unknown,
        "Stale trust prompt text should NOT cause false Waiting"
    );
}

#[test]
fn codex_stale_update_dialog_not_false_waiting() {
    // Stale update dialog with "Press enter to continue" in scrollback
    let content = "\
✨ Update available! 0.104.0 -> 0.106.0\n\
\n\
› 1. Update now\n\
  2. Skip\n\
  3. Skip until next version\n\
\n\
  Press enter to continue\n\
\n\
╭─────────────────────────────────────────────╮\n\
│ >_ OpenAI Codex (v0.104.0)                  │\n\
╰─────────────────────────────────────────────╯\n\
\n\
› Fix the bug\n\
\n\
                                                  100% context left";
    assert_eq!(
        detect_state(content, AgentKind::Codex),
        AgentState::Unknown,
        "Stale update dialog should NOT cause false Waiting"
    );
}
