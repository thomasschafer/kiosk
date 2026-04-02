use std::borrow::Cow;
use std::sync::LazyLock;

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
    ("@cursor/agent", AgentKind::CursorAgent),
    ("cursor/agent", AgentKind::CursorAgent),
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
    let first_token = cmd_lower.split_whitespace().next().unwrap_or_default();

    // Cursor CLI often appears as bare `agent` in pane command.
    if first_token == "agent" {
        return Some(AgentKind::CursorAgent);
    }

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

/// Detect the kind of agent from tmux pane title text.
///
/// Titles are useful for wrapper processes where `pane_current_command` may
/// be ambiguous (for example, a version string). Keep patterns conservative.
pub fn detect_agent_kind_from_title(title: &str) -> Option<AgentKind> {
    let lower = title.to_lowercase();
    if lower.contains("claude code") {
        return Some(AgentKind::ClaudeCode);
    }
    if lower.contains("openai codex") {
        return Some(AgentKind::Codex);
    }
    if lower.contains("cursor agent") {
        return Some(AgentKind::CursorAgent);
    }
    if lower.contains("opencode") {
        return Some(AgentKind::OpenCode);
    }
    if lower.contains("gemini") {
        return Some(AgentKind::Gemini);
    }
    None
}

/// Fallback kind detection from captured pane content.
///
/// Used when process-based detection is inconclusive for host commands
/// (`zsh`, `node`, etc.). Keep patterns narrow to avoid false positives.
pub fn detect_agent_kind_from_content(content: &str) -> Option<AgentKind> {
    detect_agent_kind_from_content_impl(content, false)
}

/// Fallback kind detection from captured pane content for wrapper-style
/// commands where `pane_current_command` may be a version string (for
/// example `2.1.63` in some Claude Code setups).
///
/// This includes Claude-specific content markers in addition to the standard
/// content fallback markers.
pub fn detect_agent_kind_from_wrapper_content(content: &str) -> Option<AgentKind> {
    detect_agent_kind_from_content_impl(content, true)
}

fn detect_agent_kind_from_content_impl(
    content: &str,
    include_claude_markers: bool,
) -> Option<AgentKind> {
    let clean = strip_ansi_codes(content);
    let lower = clean.to_lowercase();

    // Codex-specific UI markers seen in real captures.
    // Examples:
    // - `>_ OpenAI Codex (v0.105.0)`
    // - `gpt-5.3-codex high · 100% left · ~/path`
    if lower.contains("openai codex")
        || (lower.contains("codex") && lower.contains(" left") && clean.contains('·'))
    {
        return Some(AgentKind::Codex);
    }

    if include_claude_markers && lower.contains("claude code v") {
        return Some(AgentKind::ClaudeCode);
    }

    // Gemini CLI shows its banner in the content.
    if lower.contains("gemini code assist") || lower.contains("/model auto (gemini") {
        return Some(AgentKind::Gemini);
    }

    // Cursor Agent header line, e.g. `Cursor Agent v2026.02.27-e7d2ef6`
    if lower.contains("cursor agent v") {
        return Some(AgentKind::CursorAgent);
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
        "❯ 1.",
        "do you trust the files",
    ],
    // `? for shortcuts` is the canonical idle indicator but is NOT reliably
    // captured by `tmux capture-pane` in Claude Code >= v2.1 (rendered as a
    // status-bar element outside the normal text flow). We fall back to
    // detecting the input prompt character `❯` in `detect_claude_state`.
    idle_tail: &[
        "? for shortcuts",
        // Config overlay: unique filter prompt shown in /config screen
        "type to filter · enter",
        // Status overlay: shown in /status screen with session info
        "esc to cancel",
    ],
};

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
    "burrowing",
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
    "dilly-dallying",
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
    "orbiting",
    "philosophising",
    "pondering",
    "pontificating",
    "processing",
    "puttering",
    "puzzling",
    "reticulating",
    "ruminating",
    "scampering",
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
    "swooping",
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

/// Pre-computed suffixed thinking words to avoid allocations during detection.
/// Each word is stored with both `…` and `...` suffixes.
///
/// Built once on first access via `LazyLock`.
static THINKING_SUFFIXED: LazyLock<Vec<String>> = LazyLock::new(|| {
    CLAUDE_THINKING_WORDS
        .iter()
        .flat_map(|word| [format!("{word}…"), format!("{word}...")])
        .collect()
});

/// Glyphs that Claude Code prefixes to active thinking status lines.
///
/// Examples: `✦ Noodling…`, `✽ Burrowing… (1m 7s · ↓ 3.0k tokens)`.
/// Completed thinking replaces the ellipsis with a duration (`✻ Crunched for 37s`),
/// so checking for both the glyph AND an ellipsis on the same line distinguishes
/// active from completed thinking.
///
/// Note: `·` (middle dot) was observed as a rare thinking glyph (`· Orbiting…`)
/// but is excluded because it also appears extensively in normal UI elements
/// (e.g. `Opus 4.6 · Claude Max ·`). A line starting with `·` that contains
/// `...` in response text would false-match. When `·` is the thinking glyph,
/// `esc to interrupt` is always present as a primary running indicator.
const THINKING_GLYPHS: &[char] = &['✦', '✻', '✽', '✶', '✢'];

// -- Codex --------------------------------------------------------------------

const CODEX_PATTERNS: AgentPatterns = AgentPatterns {
    running: &["esc to interrupt", "tab to queue message"],
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
    // Inspired by AoE's `detect_cursor_status` which delegates to Claude detection.
    // Hexagonal markers (⬢/⬡) appear during active generation/thinking/editing.
    running: &[
        "esc to interrupt",
        "ctrl+c to interrupt",
        "ctrl+c to stop",
        "⬢ thinking",
        "⬡ thinking",
        "⬢ generating",
        "⬡ generating",
        "⬢ editing",
        "⬡ editing",
    ],
    waiting: &[
        "do you trust",
        "trust this workspace",
        "waiting for approval",
        "run this command?",
        "not in allowlist:",
        "run (once)",
        "run everything",
        "skip (esc or n)",
        // Cursor shows file operation approval dialogs
        "delete this file?",
        "delete (y)",
        "keep (n)",
        "overwrite (y)",
        "create this file?",
    ],
    idle_tail: &["/ commands"],
};

// -- OpenCode -----------------------------------------------------------------

const OPENCODE_PATTERNS: AgentPatterns = AgentPatterns {
    // The `esc interrupt` text appears in the footer alongside the block
    // spinner `⬝■` during active work. `esc again` covers the variant
    // phrasing `esc again to interrupt`.
    running: &["esc interrupt", "esc again"],
    // OpenCode shows a permission dialog for bash commands, file edits,
    // writes, and fetch operations. Two dialog styles exist:
    //   Old style: "Permission Required" + "Allow (a)" / "Deny (d)"
    //   New style: "△ Permission required" + "Allow once" / "Allow always" / "Reject"
    //              with footer "ctrl+f fullscreen  ⇆ select  enter confirm"
    waiting: &[
        "permission required",
        "allow (a)",
        "deny (d)",
        "allow once",
        "allow always",
        "enter confirm",
    ],
    // OpenCode's idle footer shows `ctrl+p commands` when at the input prompt.
    idle_tail: &["ctrl+p commands", "ctrl+t variants", "tab agents"],
};

// -- Gemini CLI ---------------------------------------------------------------

const GEMINI_PATTERNS: AgentPatterns = AgentPatterns {
    // Gemini CLI running indicators.
    // Patterns drawn from Agent of Empires' `detect_gemini_status`
    // (<https://github.com/njbrake/agent-of-empires>).
    running: &["esc to interrupt", "ctrl+c to interrupt", "esc to cancel"],
    // Gemini CLI approval prompts. Use specific patterns to avoid false
    // positives from agent output containing common words like "approve".
    waiting: &[
        "(y/n)",
        "[y/n]",
        "allow?",
        "approve?",
        "execute?",
        "enter to select",
        "esc to cancel",
        // Gemini shows an "Action Required" dialog with allow/deny options
        "action required",
        "allow once",
        "allow for this session",
        "allow execution",
    ],
    // `? for shortcuts` can be 7+ lines from the bottom (Gemini's footer is
    // 6+ lines deep). `type your message` is reliably visible in the last 5.
    idle_tail: &["? for shortcuts", "type your message"],
};

// ===========================================================================
// Braille spinners (shared across all agents)
// ===========================================================================

const BRAILLE_SPINNERS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ===========================================================================
// State detection — content preparation
// ===========================================================================

/// Pre-processed terminal content for state detection.
///
/// Strips ANSI codes, collects non-empty lines, and computes lowercased tail
/// windows (last 30, 5, and 3 lines) in a single pass. Both [`detect_state`]
/// and [`detect_state_rule`] use this to avoid duplicating the setup logic.
struct PreparedContent {
    /// Lowercased tail window of the last 30 non-empty lines (content-level).
    content: String,
    /// Lowercased tail window of the last 5 non-empty lines (tail-level).
    tail: String,
    /// Lowercased tail window of the last 3 non-empty lines (prompt-level).
    prompt_tail: String,
}

impl PreparedContent {
    fn from_raw(raw: &str) -> Self {
        let clean = strip_ansi_codes(raw);
        let non_empty: Vec<&str> = clean
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let len = non_empty.len();
        Self {
            content: join_tail(&non_empty, len, 30).to_lowercase(),
            tail: join_tail(&non_empty, len, 5).to_lowercase(),
            prompt_tail: join_tail(&non_empty, len, 3).to_lowercase(),
        }
    }
}

// ===========================================================================
// State detection — public entry point
// ===========================================================================

/// Detect agent state from terminal content, dispatching to agent-specific
/// detectors.
pub fn detect_state(content: &str, kind: AgentKind) -> AgentState {
    let prep = PreparedContent::from_raw(content);

    match kind {
        AgentKind::ClaudeCode => detect_claude_state(&prep.content, &prep.tail, &prep.prompt_tail),
        AgentKind::Codex => detect_codex_state(&prep.content, &prep.tail),
        AgentKind::CursorAgent => detect_cursor_state(&prep.content, &prep.tail),
        AgentKind::OpenCode => detect_opencode_state(&prep.content, &prep.tail),
        AgentKind::Gemini => detect_gemini_state(&prep.content, &prep.tail),
    }
}

/// Return the high-level rule that produced `state` for the given content.
///
/// This is intended for diagnostics (for example `kiosk status --debug-agent`),
/// not for core state logic.
pub fn detect_state_rule(content: &str, kind: AgentKind, state: AgentState) -> &'static str {
    let prep = PreparedContent::from_raw(content);

    match kind {
        AgentKind::ClaudeCode => match state {
            AgentState::Running => {
                if matches_any(&prep.tail, CLAUDE_PATTERNS.running)
                    || contains_braille_spinner(&prep.tail)
                {
                    "claude.running.tail_marker"
                } else if contains_thinking_status(&prep.content) {
                    "claude.running.thinking_glyph"
                } else if contains_thinking_word(&prep.content) {
                    "claude.running.thinking_word"
                } else {
                    "claude.running.content_marker"
                }
            }
            AgentState::Waiting => "claude.waiting.pattern",
            AgentState::Idle => {
                if matches_any(&prep.tail, CLAUDE_PATTERNS.idle_tail) {
                    "claude.idle.footer"
                } else if prep
                    .prompt_tail
                    .lines()
                    .any(|line| is_idle_claude_prompt(line.trim_start()))
                {
                    "claude.idle.prompt_fallback"
                } else {
                    "claude.idle.other"
                }
            }
            AgentState::Unknown => "claude.unknown.no_match",
        },
        AgentKind::Codex => match state {
            AgentState::Running => "codex.running.pattern",
            AgentState::Waiting => "codex.waiting.tail_pattern",
            AgentState::Idle => {
                if matches_any(&prep.tail, CODEX_PATTERNS.idle_tail) {
                    "codex.idle.footer"
                } else if looks_like_codex_idle_prompt(&prep.tail) {
                    "codex.idle.prompt_and_footer"
                } else if looks_like_codex_plain_idle_prompt(&prep.tail) {
                    "codex.idle.plain_prompt"
                } else {
                    "codex.idle.other"
                }
            }
            AgentState::Unknown => "codex.unknown.no_match",
        },
        AgentKind::CursorAgent => match state {
            AgentState::Running => "cursor.running.pattern",
            AgentState::Waiting => "cursor.waiting.pattern",
            AgentState::Idle => {
                if matches_any(&prep.tail, CURSOR_PATTERNS.idle_tail) {
                    "cursor.idle.footer"
                } else {
                    "cursor.idle.prompt"
                }
            }
            AgentState::Unknown => "cursor.unknown.no_match",
        },
        AgentKind::OpenCode => match state {
            AgentState::Running => "opencode.running.pattern",
            AgentState::Waiting => "opencode.waiting.pattern",
            AgentState::Idle => {
                if matches_any(&prep.tail, OPENCODE_PATTERNS.idle_tail) {
                    "opencode.idle.footer"
                } else if prep
                    .tail
                    .lines()
                    .any(|line| line.contains('┃') || line.contains('╹'))
                {
                    "opencode.idle.prompt_bar"
                } else {
                    "opencode.idle.other"
                }
            }
            AgentState::Unknown => "opencode.unknown.no_match",
        },
        AgentKind::Gemini => match state {
            AgentState::Running => "gemini.running.pattern_or_spinner",
            AgentState::Waiting => {
                if prep.content.contains("waiting for auth") {
                    "gemini.waiting.auth"
                } else {
                    "gemini.waiting.pattern"
                }
            }
            AgentState::Idle => "gemini.idle.footer",
            AgentState::Unknown => "gemini.unknown.no_match",
        },
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
    // Claude priority differs from generic active-state detection because
    // some waiting markers (for example "(y/n)") are too broad unless paired
    // with an approval prompt. Keep waiting logic in a dedicated helper.
    if matches_any(tail, CLAUDE_PATTERNS.running) || contains_braille_spinner(tail) {
        return AgentState::Running;
    }
    if detect_claude_waiting(tail) || detect_claude_waiting(content) {
        return AgentState::Waiting;
    }
    // Glyph-based thinking status (checked on full content to catch thinking
    // lines pushed out of the tail by queued messages).
    if contains_thinking_status(content) {
        return AgentState::Running;
    }
    if matches_any(tail, CLAUDE_PATTERNS.idle_tail) {
        return AgentState::Idle;
    }

    // Prompt fallback: check if any line in a tight window (last 3 lines)
    // starts with `❯`. Using 3 lines instead of the broader tail reduces
    // false Idle during transitions.
    //
    // CRITICAL: the bare presence of `❯` is NOT enough — during API calls
    // the user's query `❯ <question text>` remains visible and would
    // false-match. Only return Idle when the prompt line looks genuinely
    // idle: bare (`❯` / `❯ `), or showing Claude's suggestion (`Try "…"`).
    //
    // This is checked BEFORE content-level running markers because Claude's
    // response text may contain keywords like "esc to interrupt" as prose
    // (e.g. when discussing agent detection). The strict bare-prompt check
    // prevents false Idle during actual processing (where the prompt line
    // contains the user's query text).
    if prompt_tail.lines().any(|line| {
        let trimmed = line.trim_start();
        is_idle_claude_prompt(trimmed)
    }) {
        return AgentState::Idle;
    }

    // Content-level running signals: explicit markers, braille spinners,
    // and whimsical "thinking" words (e.g. `noodling…`).
    if matches_any(content, CLAUDE_PATTERNS.running)
        || contains_braille_spinner(content)
        || contains_thinking_word(content)
    {
        return AgentState::Running;
    }

    AgentState::Unknown
}

fn detect_claude_waiting(content: &str) -> bool {
    if matches_any(content, CLAUDE_PATTERNS.waiting) {
        return true;
    }
    if content.contains("do you want to proceed?") {
        return content.contains("(y/n)")
            || content.contains("[y/n]")
            || content.contains("❯ 1.")
            || content.contains("enter to select")
            || content.contains("yes, allow")
            || content.contains("allow once")
            || content.contains("allow always");
    }
    false
}

/// Codex-specific state detection.
///
/// Codex does NOT use the alternate screen, so old content persists in the
/// scrollback. This means stale waiting/running text from earlier prompts
/// can cause false positives if checked against the full content window.
/// All pattern checks are therefore tail-only (last 5 lines).
///
/// Idle detection uses a three-step fallback chain because the primary
/// footer signal (`? for shortcuts`) isn't always visible in tmux captures:
///
/// 1. **`matches_any(tail, idle_tail)`** — the `? for shortcuts` footer.
///    Most reliable: only present when Codex is truly at the input prompt.
///    (Exception: if a running indicator also appears, Codex is actively
///    working — it shows both simultaneously during tool execution.)
///
/// 2. **[`looks_like_codex_idle_prompt`]** — requires BOTH a non-menu
///    prompt line (`› ...`) AND a model/footer status line (e.g.
///    `gpt-5.3-codex high · 100% left · ~/…`). Covers newer Codex renders
///    where `? for shortcuts` is absent but the footer status line is
///    present. Requiring both signals avoids false idle from stale text.
///
/// 3. **[`looks_like_codex_plain_idle_prompt`]** — bare prompt (`›` or
///    `› type a message`) on the last non-empty line, with no footer at
///    all. Most conservative: excludes tails containing `context left`
///    which commonly appear mid-processing.
fn detect_codex_state(_content: &str, tail: &str) -> AgentState {
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
    // IMPORTANT: Only check waiting patterns in the **tail** to avoid stale
    // scrollback false positives (e.g. old trust prompts, update dialogs
    // that remain in the buffer after being dismissed).
    if matches_any(tail, CODEX_PATTERNS.waiting) {
        return AgentState::Waiting;
    }
    // Fallback for newer Codex prompt/footer renders where `? for shortcuts`
    // is not visible in tmux capture. Require both a non-menu prompt line
    // and the model/footer status line to avoid false idle from stale text.
    if looks_like_codex_idle_prompt(tail) {
        return AgentState::Idle;
    }
    // Additional fallback: plain prompt line at the end without footer.
    // Keep this conservative by requiring prompt on the last non-empty line
    // and excluding "context left" tails which commonly appear mid-processing.
    if looks_like_codex_plain_idle_prompt(tail) {
        return AgentState::Idle;
    }
    AgentState::Unknown
}

/// Cursor Agent-specific state detection.
///
/// Cursor shows `/ commands` in the footer during BOTH idle and running
/// states, so we must check running indicators BEFORE idle patterns.
/// This requires a custom priority order: content-level running checks
/// (hexagonal markers ⬢/⬡ for Thinking/Generating/Editing) must come
/// before idle-tail checks, which differs from [`detect_active_state`]'s
/// order.
fn detect_cursor_state(content: &str, tail: &str) -> AgentState {
    // Check tail-level running first (ctrl+c to stop, esc to interrupt).
    if matches_any(tail, CURSOR_PATTERNS.running) || contains_braille_spinner(tail) {
        return AgentState::Running;
    }
    // Check waiting before idle — permission dialogs need human attention.
    if matches_any(content, CURSOR_PATTERNS.waiting) {
        return AgentState::Waiting;
    }
    // Content-level running indicators (⬢/⬡ hexagonal markers for
    // Thinking/Generating/Editing) must be checked BEFORE idle_tail
    // because Cursor shows `/ commands` in both idle and running states.
    if matches_any(content, CURSOR_PATTERNS.running) || contains_braille_spinner(content) {
        return AgentState::Running;
    }
    // Idle footer: `/ commands` only counts as idle when no running
    // indicators are present (checked above).
    if matches_any(tail, CURSOR_PATTERNS.idle_tail) {
        return AgentState::Idle;
    }
    // Fallback: bare prompt "> " at the end of output means idle.
    if tail.lines().any(|line| line.trim() == ">") {
        return AgentState::Idle;
    }
    AgentState::Unknown
}
/// OpenCode-specific state detection.
///
/// `OpenCode` uses the alternate screen (TUI app). It shows `esc interrupt`
/// during active work and `ctrl+p commands` in the idle footer. Like Claude,
/// it may not always capture the footer text reliably via tmux, so we fall
/// back to detecting the input prompt bar (`┃`/`╹`) in the tail when no
/// other indicators match.
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

fn detect_gemini_state(content: &str, _tail: &str) -> AgentState {
    // 1. Waiting — needs human attention urgently, checked first.
    if content.contains("waiting for auth") {
        return AgentState::Waiting;
    }
    if matches_any(
        content,
        &["action required", "allow once", "allow for this session"],
    ) {
        return AgentState::Waiting;
    }
    if matches_any(
        content,
        &[
            "run this command?",
            "approve?",
            "execute?",
            "allow execution",
        ],
    ) {
        return AgentState::Waiting;
    }
    // "(y/n)" is too broad on its own; require nearby approval context.
    if (content.contains("(y/n)") || content.contains("[y/n]"))
        && (content.contains("approve")
            || content.contains("allow")
            || content.contains("execute")
            || content.contains("action required")
            || content.contains("run this command"))
    {
        return AgentState::Waiting;
    }

    // 2. Running — content-level indicators (may not appear in tail).
    if matches_any(content, GEMINI_PATTERNS.running) || contains_braille_spinner(content) {
        return AgentState::Running;
    }

    // 3. Idle — checked against full content (not just tail) because Gemini's
    // footer can push `? for shortcuts` 7+ lines from the bottom. This is safe
    // because Gemini uses the alt screen (no stale scrollback) and running is
    // checked first.
    if matches_any(content, GEMINI_PATTERNS.idle_tail) {
        return AgentState::Idle;
    }

    AgentState::Unknown
}

// ===========================================================================
// Shared detection logic
// ===========================================================================

/// Shared state detection helper — currently used only by **`OpenCode`**.
///
/// This helper applies the standard priority for agents whose idle footer
/// reliably signals the prompt, and whose running indicators don't overlap
/// with idle-tail patterns.
///
/// Other agents have custom implementations because this priority order
/// doesn't fit their capture semantics:
/// - **Claude Code**: uses the alternate screen (no scrollback), needs a
///   tighter prompt-fallback window (last 3 lines), and has complex
///   waiting-pattern logic (approval prompts paired with y/n).
/// - **Codex**: no alternate screen, so stale scrollback can cause false
///   positives. Idle-footer-first priority is critical to override lingering
///   running/waiting text from earlier prompts.
/// - **Cursor**: shows `/ commands` in both idle AND running states, so
///   content-level running indicators must be checked before idle-tail —
///   the opposite of this helper's order.
/// - **Gemini**: uses content-level (not tail-level) pattern matching for
///   waiting/running, and has a distinct `(y/n)` approval detection that
///   must exclude false positives from content mentioning `(y/n)`.
///
/// Detection priority:
/// 1. Running patterns in the **tail** → Running
/// 2. Waiting patterns in full content → Waiting
/// 3. Idle tail patterns (e.g. `? for shortcuts`) → Idle
/// 4. Running patterns in full content + braille spinners → Running
/// 5. Default → **Unknown** (no recognisable pattern matched)
fn detect_active_state(content: &str, tail: &str, patterns: &AgentPatterns) -> AgentState {
    if matches_any(tail, patterns.running) || contains_braille_spinner(tail) {
        return AgentState::Running;
    }
    // Check waiting before idle_tail: permission/approval dialogs (e.g.
    // OpenCode's "Permission Required") can appear alongside the idle footer
    // in the same tmux capture, and waiting needs human attention urgently.
    if matches_any(content, patterns.waiting) {
        return AgentState::Waiting;
    }
    if matches_any(tail, patterns.idle_tail) {
        return AgentState::Idle;
    }
    if matches_any(content, patterns.running) || contains_braille_spinner(content) {
        return AgentState::Running;
    }
    AgentState::Unknown
}

// ===========================================================================
// Pattern matchers
// ===========================================================================

/// Check if a trimmed, lowercased line looks like Claude Code's idle prompt.
///
/// Returns `true` for:
/// - Bare prompt: `❯` or `❯ ` (just the chevron, possibly with trailing space)
/// - Suggestion: `❯ try "…"` (Claude's placeholder suggestion text)
///
/// Returns `false` for lines that contain a user's query after `❯`, e.g.
/// `❯ what is 2+2?` — this appears during API calls when Claude is actually
/// processing, not idle.
fn is_idle_claude_prompt(trimmed: &str) -> bool {
    // Must start with the prompt character
    let Some(after) = trimmed.strip_prefix('❯') else {
        return false;
    };
    // Bare prompt (nothing after ❯)
    if after.trim().is_empty() {
        return true;
    }
    // Claude's suggestion text: `Try "refactor cli.rs"` etc.
    // Lowercased input, so check for lowercase "try".
    if after.trim().starts_with("try \"") || after.trim().starts_with("try \u{201c}") {
        return true;
    }
    // The Gravy cat mascot is rendered to the right of the prompt on the same
    // terminal row. tmux capture-pane -J joins the row into one line, producing
    // `❯        (  ω  )` with many spaces before the cat art. User queries
    // follow `❯` with a single space (`❯ some query`), so 5+ leading spaces
    // reliably indicate the cat graphic rather than typed input.
    if after.starts_with("     ") {
        return true;
    }
    false
}

/// Step 2 of the Codex idle fallback chain.
///
/// Matches when the tail contains both a non-menu prompt line (`› ...`)
/// and a model/footer status line (e.g. `gpt-5.3-codex high · 100% left`).
/// This covers newer Codex renders where `? for shortcuts` is absent but
/// the footer status line is still present.
fn looks_like_codex_idle_prompt(tail: &str) -> bool {
    // Strong idle fallback: when Codex footer is present, allow any
    // non-menu prompt line (`› ...`). This matches real idle captures where
    // the last prompt may include user text.
    let has_prompt = tail.lines().any(is_non_menu_codex_prompt_line);

    // Newer Codex footer styles, e.g.
    // `gpt-5.3-codex high · 100% left · ~/Development/kiosk`
    // `gpt-5.4 default · 79% left · ~/Development/kiosk`
    //
    // Keep this narrow so simple `100% context left` does not imply idle.
    let has_footer = tail.lines().any(looks_like_codex_footer_line);

    has_prompt && has_footer
}

fn is_non_menu_codex_prompt_line(line: &str) -> bool {
    let Some(after_prompt) = codex_prompt_payload(line) else {
        return false;
    };
    !is_menu_codex_prompt_payload(after_prompt)
}

fn is_bare_non_menu_codex_prompt_line(line: &str) -> bool {
    let Some(after_prompt) = codex_prompt_payload(line) else {
        return false;
    };
    after_prompt.is_empty() || after_prompt.eq_ignore_ascii_case("type a message")
}

fn codex_prompt_payload(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let after_prompt = trimmed.strip_prefix('›')?;
    Some(after_prompt.trim())
}

fn is_menu_codex_prompt_payload(after_prompt: &str) -> bool {
    let Some((prefix, suffix)) = after_prompt.split_once('.') else {
        return false;
    };
    !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) && suffix.starts_with(' ')
}

/// Step 3 of the Codex idle fallback chain.
///
/// Matches a bare prompt (`›` or `› type a message`) on the last non-empty
/// line with no footer present at all. Most conservative of the three idle
/// checks: excludes tails containing `context left` which commonly appear
/// mid-processing.
fn looks_like_codex_plain_idle_prompt(tail: &str) -> bool {
    // `tail` is already lowercased by PreparedContent::from_raw.
    let mut lines = tail.lines().map(str::trim).filter(|line| !line.is_empty());
    let Some(last) = lines.next_back() else {
        return false;
    };
    // Keep this strict: only a bare prompt line should be treated as idle.
    if !is_bare_non_menu_codex_prompt_line(last) {
        return false;
    }
    // During processing Codex often shows "context left" with the user prompt.
    // Keep that state as Unknown to avoid false Idle.
    !tail.contains("context left")
}

/// Detect a Codex model/status footer line (e.g. `gpt-5.3-codex high · 100% left · ~/…`).
///
/// Used by [`looks_like_codex_idle_prompt`] to confirm the footer is present
/// alongside a prompt line. Matches lines containing `·`-separated segments
/// where the first segment looks like a model name.
fn looks_like_codex_footer_line(line: &str) -> bool {
    // `line` is a trimmed slice of an already-lowercased tail string
    // (PreparedContent::from_raw lowercases before windowing).
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.contains("context left") || !trimmed.contains('·') {
        return false;
    }

    let parts: Vec<&str> = trimmed.split('·').map(str::trim).collect();
    if parts.len() < 2 {
        return false;
    }

    let model_segment = parts[0];
    let looks_like_model = model_segment.contains("codex")
        || model_segment.starts_with("gpt-")
        // Broad 'o' prefix catches the o-series family (o1, o3, o4-mini, …)
        // without enumerating every variant; the surrounding guards keep
        // false-positive risk low.
        || model_segment.starts_with('o');
    let has_left_metric = parts[1].contains("left");
    let has_path_segment = parts
        .get(2)
        .is_none_or(|segment| segment.contains('/') || segment.starts_with('~'));

    looks_like_model && has_left_metric && has_path_segment
}

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
/// English text that might contain words like "working..." or "processing...".
///
/// Only matches when the thinking word appears near the start of a line
/// (after optional whitespace), to avoid false positives from prose text
/// like "the system is working...". The glyph-based [`contains_thinking_status`]
/// check handles the canonical `✽ Burrowing…` format; this is a fallback for
/// cases where the glyph is stripped or missing.
///
/// Uses pre-computed suffixed strings ([`THINKING_SUFFIXED`]) to avoid
/// allocations on every poll cycle.
///
/// Inspired by agent-os (<https://github.com/saadnvd1/agent-os>).
fn contains_thinking_word(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        THINKING_SUFFIXED
            .iter()
            .any(|s| trimmed.starts_with(s.as_str()))
    })
}

/// Glyph-based thinking status detector for Claude Code.
///
/// Claude prefixes active thinking with one of [`THINKING_GLYPHS`] at the start
/// of the line, followed by a verb and ellipsis (e.g. `✽ Burrowing…`). Completed
/// thinking replaces the ellipsis with a duration (e.g. `✻ Crunched for 37s`).
///
/// We require the glyph to be the first non-whitespace character AND an ellipsis
/// on the same line. This avoids false matches from lines where a glyph appears
/// in normal text (e.g. tool output or user-typed messages).
fn contains_thinking_status(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed
            .chars()
            .next()
            .is_some_and(|c| THINKING_GLYPHS.contains(&c))
            && (trimmed.contains('…') || trimmed.contains("..."))
    })
}

// ===========================================================================
// Text helpers
// ===========================================================================

/// Strip ANSI escape codes from terminal content without regex.
/// Handles CSI (`ESC [`) sequences, OSC (`ESC ]`) sequences (terminated by
/// BEL `\x07` or ST `ESC \`), and unknown two-byte `ESC X` sequences.
///
/// Returns the input unchanged (no allocation) when no escape codes are
/// present — a common case for content already processed by tmux.
fn strip_ansi_codes(content: &str) -> Cow<'_, str> {
    // Fast path: no allocation when no escape codes present
    if !content.contains('\x1B') {
        return Cow::Borrowed(content);
    }

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
    Cow::Owned(out)
}

/// Join the last `count` elements from `lines` into a single `\n`-separated
/// string. Avoids collecting into a temporary `Vec` — works directly with
/// the pre-collected slice.
fn join_tail(lines: &[&str], len: usize, count: usize) -> String {
    let start = len.saturating_sub(count);
    lines[start..].join("\n")
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
        assert_eq!(
            detect_agent_kind("agent", None),
            Some(AgentKind::CursorAgent)
        );
        assert_eq!(
            detect_agent_kind("agent --version", None),
            Some(AgentKind::CursorAgent)
        );
        assert_eq!(detect_agent_kind("cursor", None), None);
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
            detect_agent_kind(
                "node",
                Some("/home/user/.local/share/npm/node_modules/@cursor/agent/dist/index.js")
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
        assert_eq!(detect_agent_kind("python", Some("agent")), None);
    }

    #[test]
    fn detect_kind_from_title_claude() {
        assert_eq!(
            detect_agent_kind_from_title("✳ Claude Code"),
            Some(AgentKind::ClaudeCode)
        );
    }

    #[test]
    fn detect_kind_from_title_codex() {
        assert_eq!(
            detect_agent_kind_from_title(">_ OpenAI Codex"),
            Some(AgentKind::Codex)
        );
    }

    #[test]
    fn detect_kind_from_title_cursor_requires_agent_word() {
        assert_eq!(detect_agent_kind_from_title("Cursor"), None);
        assert_eq!(
            detect_agent_kind_from_title("Cursor Agent"),
            Some(AgentKind::CursorAgent)
        );
    }

    #[test]
    fn detect_kind_from_title_unknown() {
        assert_eq!(detect_agent_kind_from_title("Toms-MacBook-Pro.local"), None);
    }

    #[test]
    fn detect_kind_from_content_codex() {
        let content = "╭──────────────────────────────────────────────────╮\n\
                       │ >_ OpenAI Codex (v0.105.0)                       │\n\
                       ╰──────────────────────────────────────────────────╯\n\
                       \n\
                       › Write tests for @filename\n\
                         gpt-5.3-codex high · 100% left · ~/Development/kiosk";
        assert_eq!(
            detect_agent_kind_from_content(content),
            Some(AgentKind::Codex)
        );
    }

    #[test]
    fn detect_kind_from_content_gemini() {
        // Banner text from Gemini CLI startup
        let content = "Plan: Gemini Code Assist for individuals
                       ╭──────────────────────────────────────╮
                       │ ? for shortcuts";
        assert_eq!(
            detect_agent_kind_from_content(content),
            Some(AgentKind::Gemini)
        );
        // Footer with model selector
        let content2 = "/tmp/project (main)  no sandbox (see /docs)  /model Auto (Gemini 3)";
        assert_eq!(
            detect_agent_kind_from_content(content2),
            Some(AgentKind::Gemini)
        );
    }

    #[test]
    fn detect_kind_from_content_cursor() {
        let content = "  Cursor Agent v2026.02.27-e7d2ef6
                       
                       Write tests for the parser module
                       / commands";
        assert_eq!(
            detect_agent_kind_from_content(content),
            Some(AgentKind::CursorAgent)
        );
    }

    #[test]
    fn detect_kind_from_wrapper_content_claude() {
        let content = "▐▛███▜▌   Claude Code v2.1.63\n\
                       ▝▜█████▛▘  Opus 4.6 · Claude Max\n\
                       ❯ 1. Yes";
        assert_eq!(
            detect_agent_kind_from_wrapper_content(content),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(detect_agent_kind_from_content(content), None);
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
    fn claude_proceed_phrase_without_dialog_context_is_not_waiting() {
        let content = "I ask users: do you want to proceed? in docs\n? for shortcuts";
        assert_eq!(
            detect_state(content, AgentKind::ClaudeCode),
            AgentState::Idle
        );
    }

    #[test]
    fn claude_y_n_in_prose_is_not_waiting() {
        let content = "Guideline: use (y/n) to answer prompts in scripts\n? for shortcuts";
        assert_eq!(
            detect_state(content, AgentKind::ClaudeCode),
            AgentState::Idle
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
                " ▐▛███▜▌   Claude Code v2.1.59\n\
                 ▝▜█████▛▘  Opus 4.6 · Claude Max\n\
                   ▘▘ ▝▝    ~/Development/kiosk\n\
                 \n\
                 ────────────\n\
                 ❯ Try \"create a util logging.py that...\"\n\
                 ────────────\n\
                   PR #12",
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
    fn claude_idle_prompt_with_gravy_cat_on_same_line() {
        // tmux capture-pane -J joins the prompt row and the cat mascot column
        // into one line: `❯` at the left, cat art far to the right.
        let content = "────────────────────────────────────────────────────────────────────────────────────────────────────────────────  ( °   °)\n\
❯                                                                                                                  (  ω  )\n\
────────────────────────────────────────────────────────────────────────────────────────────────────────────────  (\")_(\")  ⏵⏵ bypass permissions on (shift+tab to cycle)  Gravy";
        assert_eq!(
            detect_state(content, AgentKind::ClaudeCode),
            AgentState::Idle
        );
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

    /// During API calls, Claude shows `❯ <user query>` with no running
    /// indicators. This should NOT be detected as Idle — the user's query
    /// text distinguishes it from the bare idle prompt `❯ `.
    ///
    /// Regression test for false Idle observed during manual testing.
    #[test]
    fn claude_api_call_not_false_idle() {
        let content = "\
 ▐▛███▜▌   Claude Code v2.1.63\n\
▝▜█████▛▘  Opus 4.6 · Claude Max\n\
  ▘▘ ▝▝    ~/Development/kiosk\n\
\n\
────────────────────────────────────────\n\
❯ what is 2+2? reply with just the number\n\
────────────────────────────────────────\n\
  ⏵⏵ bypass permissions on (shift+tab to cycle) · PR #12";
        assert_eq!(
            detect_state(content, AgentKind::ClaudeCode),
            AgentState::Unknown,
            "User query after ❯ should not be detected as Idle"
        );
    }

    #[test]
    fn claude_idle_suggestion_try() {
        // Claude's suggestion text starts with "Try" — this IS idle.
        let content = "\
────────────────────────────────────────\n\
❯ Try \"refactor cli.rs\"\n\
────────────────────────────────────────\n\
  ⏵⏵ bypass permissions on";
        assert_eq!(
            detect_state(content, AgentKind::ClaudeCode),
            AgentState::Idle,
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
    fn codex_idle_prompt_footer_fallback() {
        let content = "■ Conversation interrupted - tell the model what to do differently.\n\
            › \n\
              gpt-5.3-codex high · 100% left · ~/Development/kiosk";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn codex_prompt_with_context_left_stays_unknown() {
        let content = "› Review main.py and find all the bugs\n\
              100% context left";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Unknown);
    }

    #[test]
    fn codex_working_indicator() {
        // Both "esc to interrupt" and "? for shortcuts" visible — Running wins
        let content = "› hi\n\n\
            • Working (2s • esc to interrupt)\n\n\
              ? for shortcuts                     100% context left";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Running);
    }

    #[test]
    fn codex_idle_tail_overrides_stale_waiting() {
        let content = "› 1. Yes, proceed (y)\n  Press enter to confirm\n\n\
            › Type a message\n\n  ? for shortcuts";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn codex_idle_tail_overrides_stale_running() {
        // Enough lines between stale running text and idle footer to push
        // the stale content out of the tail (last 5 non-empty lines).
        let content = "• Working (5s • esc to interrupt)\n\n\
            • Ran rm hello.py\n  └ (no output)\n\n\
            • Completed.\n  - Deleted hello.py\n\n\
            › Type a message\n\n  ? for shortcuts";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn codex_stale_running_not_false_running_without_idle_tail() {
        // A stale running marker outside the tail should not force Running.
        // With a prompt + modern Codex footer, this should settle to Idle.
        let content = "• Working (8s • esc to interrupt)\n\n\
            • Completed task\n\
            • Updated summary\n\
            • Saved results\n\
            • Ready for next step\n\
            › Type a message\n\
              gpt-5.3-codex high · 100% left";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn codex_plain_prompt_without_footer_is_idle() {
        let content = "• Added regression tests for both core and TUI poller paths\n\
            • Built and validated changes\n\
            › ";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn codex_prompt_with_user_text_without_footer_stays_unknown() {
        let content = "• Added regression tests for both core and TUI poller paths\n\
            • Built and validated changes\n\
            › Nice! Okay next bug. You are currently idle. I'm seeing [UNKNOWN]";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Unknown);
    }

    #[test]
    fn codex_prompt_with_user_text_and_footer_is_idle() {
        let content = "■ Conversation interrupted - tell the model what to do differently.\n\
            › Write tests for @filename\n\
              gpt-5.3-codex high · 100% left · ~/Development/kiosk";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    #[test]
    fn codex_prompt_with_new_footer_style_is_idle() {
        let content = "• Updated tests\n\
            › Write tests for @filename\n\
              gpt-5.4 default · 79% left · ~/Development/kiosk";
        assert_eq!(detect_state(content, AgentKind::Codex), AgentState::Idle);
    }

    /// Stale trust prompt text should NOT cause false Waiting.
    ///
    /// Real scenario: After dismissing the trust prompt and update dialog,
    /// stale text like "› 1. Yes, continue" and "Press enter to continue"
    /// remains in the scrollback. When Codex is processing a query, these
    /// should not be matched.
    ///
    /// Regression test for a bug found during manual testing (2026-02-28).
    #[test]
    fn codex_stale_trust_prompt_not_false_waiting() {
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

    /// Stale update dialog should NOT cause false Waiting.
    #[test]
    fn codex_stale_update_dialog_not_false_waiting() {
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
    fn cursor_waiting_command_approval_prompt() {
        let content = "\
  $ sleep 10 Waiting for approval...

 ┌───────────────────────────────────────────────────────────────────────────────────────────────┐
 │ $  sleep 10 in .                                                                              │
 └───────────────────────────────────────────────────────────────────────────────────────────────┘
 ┌───────────────────────────────────────────────────────────────────────────────────────────────┐
 │ Run this command?                                                                             │
 │ Not in allowlist: sleep 10                                                                    │
 │  → Run (once) (y)                                                                             │
 │    Add Shell(sleep) to allowlist? (tab)                                                       │
 │    Run Everything (shift+tab)                                                                 │
 │    Skip (esc or n)                                                                            │
 └───────────────────────────────────────────────────────────────────────────────────────────────┘";
        assert_eq!(
            detect_state(content, AgentKind::CursorAgent),
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
    fn opencode_waiting_permission_required() {
        let content = "Permission Required\n\nTool: bash\nPath: /tmp\n\nCommand\n\n```bash\nrm test.txt\n```\n\n Allow (a)   Allow for session (s)   Deny (d)";
        assert_eq!(
            detect_state(content, AgentKind::OpenCode),
            AgentState::Waiting
        );
    }

    #[test]
    fn opencode_waiting_allow_button() {
        let content = "some output\nAllow (a)  Deny (d)  ctrl+p commands";
        assert_eq!(
            detect_state(content, AgentKind::OpenCode),
            AgentState::Waiting
        );
    }

    #[test]
    fn opencode_permission_in_prose_not_waiting() {
        // The word "permission" in normal output shouldn't trigger Waiting —
        // only the exact dialog text "Permission Required" should.
        let content = "You need permission to access this file\n  ctrl+p commands";
        // This contains "Permission Required"? No — just "permission".
        // Should be Idle (footer present), not Waiting.
        assert_eq!(detect_state(content, AgentKind::OpenCode), AgentState::Idle);
    }

    #[test]
    fn opencode_waiting_beats_idle_footer() {
        // OpenCode's permission dialog can appear alongside the idle footer
        // (ctrl+p commands) in the same tmux capture. Waiting should win.
        let content = "Permission Required\n\nTool: bash\nAllow (a)  Deny (d)\n  ctrl+p commands";
        assert_eq!(
            detect_state(content, AgentKind::OpenCode),
            AgentState::Waiting
        );
    }

    #[test]
    fn opencode_waiting_new_style_dialog() {
        // Newer OpenCode versions show a different permission dialog with
        // "Allow once" / "Allow always" / "Reject" buttons.
        let content = "  ┃\n  ┃  △ Permission required\n  ┃    # Deletes the specified temporary file\n  ┃\n  ┃  $ rm -f \"/tmp/test.txt\"\n  ┃\n  ┃\n  ┃   Allow once   Allow always   Reject          ctrl+f fullscreen  ⇆ select  enter confirm\n  ┃";
        assert_eq!(
            detect_state(content, AgentKind::OpenCode),
            AgentState::Waiting
        );
    }

    #[test]
    fn opencode_waiting_allow_once_button() {
        let content = "some output\nAllow once  Allow always  Reject";
        assert_eq!(
            detect_state(content, AgentKind::OpenCode),
            AgentState::Waiting
        );
    }

    #[test]
    fn opencode_waiting_new_style_beats_idle_footer() {
        // New-style permission dialog alongside the idle footer.
        let content = "△ Permission required\n$ rm test.txt\nAllow once  Allow always  Reject\n  ctrl+p commands";
        assert_eq!(
            detect_state(content, AgentKind::OpenCode),
            AgentState::Waiting
        );
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
            detect_state("approve? yes or no", AgentKind::Gemini),
            AgentState::Waiting,
        );
    }

    #[test]
    fn gemini_waiting_for_auth_is_waiting() {
        assert_eq!(
            detect_state(
                "⠴ Waiting for auth... (Press ESC to cancel)",
                AgentKind::Gemini
            ),
            AgentState::Waiting,
        );
    }

    #[test]
    fn gemini_y_n_in_generic_prose_is_not_waiting() {
        assert_eq!(
            detect_state("The docs mention (y/n) as an example", AgentKind::Gemini),
            AgentState::Unknown,
        );
    }

    #[test]
    fn gemini_unknown_no_patterns() {
        // Unrecognised content should be Unknown, not Idle — avoids false
        // Idle during API calls when no indicators are visible.
        assert_eq!(
            detect_state("some random output", AgentKind::Gemini),
            AgentState::Unknown,
        );
    }

    #[test]
    fn gemini_idle_with_shortcuts() {
        assert_eq!(
            detect_state("ready\n? for shortcuts", AgentKind::Gemini),
            AgentState::Idle,
        );
    }

    #[test]
    fn gemini_approve_in_prose_not_waiting() {
        // "approve" in normal prose should NOT trigger Waiting — we require
        // "approve?" with a trailing question mark. Without idle_tail
        // patterns, this returns Unknown (not Waiting).
        assert_eq!(
            detect_state(
                "I approve of this approach and will proceed",
                AgentKind::Gemini
            ),
            AgentState::Unknown,
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
    fn strip_ansi_codes_no_alloc_fast_path() {
        let input = "No escape codes here";
        let result = strip_ansi_codes(input);
        assert_eq!(result, input);
    }

    #[test]
    fn join_tail_basic() {
        let lines = vec!["a", "b", "c", "d", "e"];
        assert_eq!(join_tail(&lines, 5, 2), "d\ne");
        assert_eq!(join_tail(&lines, 5, 10), "a\nb\nc\nd\ne");
        assert_eq!(join_tail(&lines, 5, 0), "");
    }

    // -- PreparedContent ------------------------------------------------------

    #[test]
    fn prepared_content_basic_windows() {
        let prep = PreparedContent::from_raw("line1\nline2\nline3\nline4\nline5\nline6");
        // last_3 should contain lines 4-6
        assert_eq!(prep.prompt_tail, "line4\nline5\nline6");
        // last_5 should contain lines 2-6
        assert_eq!(prep.tail, "line2\nline3\nline4\nline5\nline6");
        // last_30 should contain all lines (fewer than 30)
        assert_eq!(prep.content, "line1\nline2\nline3\nline4\nline5\nline6");
    }

    #[test]
    fn prepared_content_strips_ansi() {
        let prep = PreparedContent::from_raw("\x1B[32mgreen\x1B[0m\nplain");
        assert_eq!(prep.tail, "green\nplain");
    }

    #[test]
    fn prepared_content_skips_empty_lines() {
        let prep = PreparedContent::from_raw("a\n\n  \nb\n\nc");
        assert_eq!(prep.prompt_tail, "a\nb\nc");
    }

    #[test]
    fn prepared_content_empty_input() {
        let prep = PreparedContent::from_raw("");
        assert_eq!(prep.content, "");
        assert_eq!(prep.tail, "");
        assert_eq!(prep.prompt_tail, "");
    }

    #[test]
    fn prepared_content_lowercases() {
        let prep = PreparedContent::from_raw("Hello WORLD");
        assert_eq!(prep.tail, "hello world");
    }

    #[test]
    fn prepared_content_over_30_boundary() {
        // 31 lines: content must contain only the last 30; tail the last 5;
        // prompt_tail the last 3.
        let lines: Vec<String> = (1..=31).map(|i| format!("line{i:02}")).collect();
        let raw = lines.join("\n");
        let prep = PreparedContent::from_raw(&raw);

        // content = lines 2..=31 (last 30)
        let expected_content = lines[1..].join("\n").to_lowercase();
        assert_eq!(
            prep.content, expected_content,
            "content should be last 30 lines"
        );

        // tail = lines 27..=31 (last 5)
        let expected_tail = lines[26..].join("\n").to_lowercase();
        assert_eq!(prep.tail, expected_tail, "tail should be last 5 lines");

        // prompt_tail = lines 29..=31 (last 3)
        let expected_prompt = lines[28..].join("\n").to_lowercase();
        assert_eq!(
            prep.prompt_tail, expected_prompt,
            "prompt_tail should be last 3 lines"
        );
    }

    #[test]
    fn prepared_content_exactly_30_lines() {
        // Exactly 30 lines: content should include all of them (no truncation).
        let lines: Vec<String> = (1..=30).map(|i| format!("line{i:02}")).collect();
        let raw = lines.join("\n");
        let prep = PreparedContent::from_raw(&raw);
        let expected = lines.join("\n").to_lowercase();
        assert_eq!(
            prep.content, expected,
            "exactly 30 lines should not be truncated"
        );
    }

    #[test]
    fn prepared_content_windows_are_nested() {
        // prompt_tail ⊆ tail ⊆ content — the suffix of each larger window should
        // end with the smaller window.
        let lines: Vec<String> = (1..=40).map(|i| format!("line{i:02}")).collect();
        let raw = lines.join("\n");
        let prep = PreparedContent::from_raw(&raw);
        assert!(
            prep.content.ends_with(&prep.tail),
            "content should end with tail"
        );
        assert!(
            prep.tail.ends_with(&prep.prompt_tail),
            "tail should end with prompt_tail"
        );
    }

    // -- detect_state_rule ---------------------------------------------------

    #[test]
    fn state_rule_claude_running_tail_marker() {
        let rule = detect_state_rule(
            "doing work\nesc to interrupt",
            AgentKind::ClaudeCode,
            AgentState::Running,
        );
        assert_eq!(rule, "claude.running.tail_marker");
    }

    #[test]
    fn state_rule_claude_running_thinking_glyph() {
        // Glyph + ellipsis but no "esc to interrupt" in last 5 lines.
        let content = "✦ Cogitating… 42 tokens\n\n\n\n\nsome other line";
        let rule = detect_state_rule(content, AgentKind::ClaudeCode, AgentState::Running);
        assert_eq!(rule, "claude.running.thinking_glyph");
    }

    #[test]
    fn state_rule_claude_running_thinking_word() {
        // Word+ellipsis at line start, glyph stripped.
        let content = "noodling…\n\n\n\n\nsome other line";
        let rule = detect_state_rule(content, AgentKind::ClaudeCode, AgentState::Running);
        assert_eq!(rule, "claude.running.thinking_word");
    }

    #[test]
    fn state_rule_claude_running_content_marker() {
        // "esc to interrupt" must be outside the tail (last 5 non-empty lines)
        // to avoid matching the tail_marker rule. Six non-empty lines after it
        // pushes it beyond the tail window.
        let content = "esc to interrupt\nline2\nline3\nline4\nline5\nsome other line";
        let rule = detect_state_rule(content, AgentKind::ClaudeCode, AgentState::Running);
        assert_eq!(rule, "claude.running.content_marker");
    }

    #[test]
    fn state_rule_claude_waiting() {
        let rule = detect_state_rule(
            "Yes, allow this action",
            AgentKind::ClaudeCode,
            AgentState::Waiting,
        );
        assert_eq!(rule, "claude.waiting.pattern");
    }

    #[test]
    fn state_rule_claude_idle_footer() {
        let rule = detect_state_rule(
            "some output\n? for shortcuts",
            AgentKind::ClaudeCode,
            AgentState::Idle,
        );
        assert_eq!(rule, "claude.idle.footer");
    }

    #[test]
    fn state_rule_claude_idle_prompt_fallback() {
        let rule = detect_state_rule(
            "some output\n❯ Try \"refactor cli.rs\"",
            AgentKind::ClaudeCode,
            AgentState::Idle,
        );
        assert_eq!(rule, "claude.idle.prompt_fallback");
    }

    #[test]
    fn state_rule_claude_unknown() {
        let rule = detect_state_rule("", AgentKind::ClaudeCode, AgentState::Unknown);
        assert_eq!(rule, "claude.unknown.no_match");
    }

    #[test]
    fn state_rule_codex_running() {
        let rule = detect_state_rule(
            "• Working (2s • esc to interrupt)",
            AgentKind::Codex,
            AgentState::Running,
        );
        assert_eq!(rule, "codex.running.pattern");
    }

    #[test]
    fn state_rule_codex_waiting() {
        let rule = detect_state_rule(
            "Run this command?\n› 1. Yes, proceed (y)",
            AgentKind::Codex,
            AgentState::Waiting,
        );
        assert_eq!(rule, "codex.waiting.tail_pattern");
    }

    #[test]
    fn state_rule_codex_idle_footer() {
        let rule = detect_state_rule(
            "› Type a message\n? for shortcuts",
            AgentKind::Codex,
            AgentState::Idle,
        );
        assert_eq!(rule, "codex.idle.footer");
    }

    #[test]
    fn state_rule_codex_idle_prompt_and_footer() {
        let rule = detect_state_rule(
            "› some text\ngpt-5.3-codex high · 100% left · ~/project",
            AgentKind::Codex,
            AgentState::Idle,
        );
        assert_eq!(rule, "codex.idle.prompt_and_footer");
    }

    #[test]
    fn state_rule_codex_idle_plain_prompt() {
        let rule = detect_state_rule("• Done\n› ", AgentKind::Codex, AgentState::Idle);
        assert_eq!(rule, "codex.idle.plain_prompt");
    }

    #[test]
    fn state_rule_codex_unknown() {
        let rule = detect_state_rule("", AgentKind::Codex, AgentState::Unknown);
        assert_eq!(rule, "codex.unknown.no_match");
    }

    #[test]
    fn state_rule_cursor_running() {
        let rule = detect_state_rule(
            "⠋ editing\nesc to interrupt",
            AgentKind::CursorAgent,
            AgentState::Running,
        );
        assert_eq!(rule, "cursor.running.pattern");
    }

    #[test]
    fn state_rule_cursor_waiting() {
        let rule = detect_state_rule(
            "run this command?\nnot in allowlist: sleep 10",
            AgentKind::CursorAgent,
            AgentState::Waiting,
        );
        assert_eq!(rule, "cursor.waiting.pattern");
    }

    #[test]
    fn state_rule_cursor_idle_footer() {
        let rule = detect_state_rule(
            "some output\n/ commands",
            AgentKind::CursorAgent,
            AgentState::Idle,
        );
        assert_eq!(rule, "cursor.idle.footer");
    }

    #[test]
    fn state_rule_cursor_idle_prompt() {
        // Bare "> " prompt with no "/ commands" footer → prompt fallback.
        let rule = detect_state_rule("> ", AgentKind::CursorAgent, AgentState::Idle);
        assert_eq!(rule, "cursor.idle.prompt");
    }

    #[test]
    fn state_rule_opencode_running() {
        let rule = detect_state_rule(
            "⬝⬝■■  esc interrupt",
            AgentKind::OpenCode,
            AgentState::Running,
        );
        assert_eq!(rule, "opencode.running.pattern");
    }

    #[test]
    fn state_rule_opencode_waiting() {
        let rule = detect_state_rule(
            "Permission Required\nAllow (a)  Deny (d)",
            AgentKind::OpenCode,
            AgentState::Waiting,
        );
        assert_eq!(rule, "opencode.waiting.pattern");
    }

    #[test]
    fn state_rule_opencode_idle_footer() {
        let rule = detect_state_rule(
            "some output\nctrl+p commands",
            AgentKind::OpenCode,
            AgentState::Idle,
        );
        assert_eq!(rule, "opencode.idle.footer");
    }

    #[test]
    fn state_rule_opencode_idle_prompt_bar() {
        let rule = detect_state_rule(
            "  ┃  Build  GPT-5.3\n  ╹▀▀▀▀",
            AgentKind::OpenCode,
            AgentState::Idle,
        );
        assert_eq!(rule, "opencode.idle.prompt_bar");
    }

    #[test]
    fn state_rule_gemini_running() {
        let rule = detect_state_rule(
            "working\nesc to interrupt",
            AgentKind::Gemini,
            AgentState::Running,
        );
        assert_eq!(rule, "gemini.running.pattern_or_spinner");
    }

    #[test]
    fn state_rule_gemini_waiting_auth() {
        let rule = detect_state_rule(
            "⠴ Waiting for auth...",
            AgentKind::Gemini,
            AgentState::Waiting,
        );
        assert_eq!(rule, "gemini.waiting.auth");
    }

    #[test]
    fn state_rule_gemini_waiting_pattern() {
        let rule = detect_state_rule(
            "action required\nallow once",
            AgentKind::Gemini,
            AgentState::Waiting,
        );
        assert_eq!(rule, "gemini.waiting.pattern");
    }

    #[test]
    fn state_rule_gemini_idle() {
        let rule = detect_state_rule(
            "some output\n? for shortcuts",
            AgentKind::Gemini,
            AgentState::Idle,
        );
        assert_eq!(rule, "gemini.idle.footer");
    }

    #[test]
    fn state_rule_gemini_unknown() {
        let rule = detect_state_rule("random output", AgentKind::Gemini, AgentState::Unknown);
        assert_eq!(rule, "gemini.unknown.no_match");
    }

    // -- Thinking word -------------------------------------------------------

    #[test]
    fn thinking_word_with_ellipsis() {
        // Bare word at line start (glyph stripped or missing)
        assert!(contains_thinking_word("noodling…"));
        assert!(contains_thinking_word("spelunking..."));
        // With leading whitespace
        assert!(contains_thinking_word("  noodling…"));
    }

    #[test]
    fn thinking_word_with_glyph_defers_to_thinking_status() {
        // When the glyph is present, contains_thinking_status() handles
        // detection. contains_thinking_word() is a fallback for when the
        // glyph is stripped, so it does NOT match glyph-prefixed lines.
        assert!(!contains_thinking_word("✦ cogitating… 42 tokens"));
    }

    #[test]
    fn thinking_word_without_ellipsis_no_match() {
        assert!(!contains_thinking_word("noodling"));
        assert!(!contains_thinking_word("I was thinking about it"));
    }

    #[test]
    fn thinking_word_in_prose_no_match() {
        // Words mid-sentence should NOT match (could be response text)
        assert!(!contains_thinking_word("The system is working… and done"));
        assert!(!contains_thinking_word("It was processing... the data"));
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    struct FixtureCase {
        file: &'static str,
        content: &'static str,
        kind: AgentKind,
        expected: AgentState,
    }

    macro_rules! fixtures {
        ( $( ($name:ident, $path:literal, $kind:expr, $expected:expr), )+ ) => {
            $( const $name: &str = include_str!(concat!("pane-captures/", $path)); )+

            fn fixture_cases() -> Vec<FixtureCase> {
                vec![
                    $( FixtureCase {
                        file: $path,
                        content: $name,
                        kind: $kind,
                        expected: $expected,
                    }, )+
                ]
            }
        };
    }

    fixtures! {
        // ── Claude Code ─────────────────────────────────────────────────
        (CLAUDE_IDLE_FRESH,               "claude-code/idle-fresh.txt",                             AgentKind::ClaudeCode,  AgentState::Idle),
        (CLAUDE_IDLE_AFTER_RESPONSE,      "claude-code/idle-after-response.txt",                    AgentKind::ClaudeCode,  AgentState::Idle),
        (CLAUDE_IDLE_AFTER_CANCELLED_EDIT,"claude-code/idle-after-cancelled-edit.txt",              AgentKind::ClaudeCode,  AgentState::Idle),
        // FIXME: should be Idle — user typed follow-up, Claude is idle but no `? for shortcuts` visible
        (CLAUDE_IDLE_QUEUED,              "claude-code/idle-with-queued-message.txt",               AgentKind::ClaudeCode,  AgentState::Unknown),
        // FIXME: should be Idle — thinking completed, user is typing, but no idle indicator captured
        (CLAUDE_IDLE_TYPING,              "claude-code/idle-typing-after-completed-thinking.txt",   AgentKind::ClaudeCode,  AgentState::Unknown),
        (CLAUDE_IDLE_RESPONSE_KEYWORDS,   "claude-code/idle-response-contains-running-keywords.txt",AgentKind::ClaudeCode,  AgentState::Idle),
        (CLAUDE_IDLE_INVALID_CMD,         "claude-code/idle-after-invalid-command.txt",             AgentKind::ClaudeCode,  AgentState::Idle),
        (CLAUDE_IDLE_CONFIG,              "claude-code/idle-config-screen.txt",                     AgentKind::ClaudeCode,  AgentState::Idle),
        (CLAUDE_IDLE_STATUS,              "claude-code/idle-status-screen.txt",                     AgentKind::ClaudeCode,  AgentState::Idle),
        (CLAUDE_IDLE_BYPASS,              "claude-code/idle-fresh-bypass-permissions.txt",          AgentKind::ClaudeCode,  AgentState::Idle),
        (CLAUDE_RUNNING_THINKING,         "claude-code/running-thinking-word.txt",                  AgentKind::ClaudeCode,  AgentState::Running),
        (CLAUDE_RUNNING_STREAMING,        "claude-code/running-streaming-response.txt",             AgentKind::ClaudeCode,  AgentState::Running),
        (CLAUDE_RUNNING_EXTENDED,         "claude-code/running-extended-thinking.txt",              AgentKind::ClaudeCode,  AgentState::Running),
        (CLAUDE_RUNNING_ESC,              "claude-code/running-with-esc-to-interrupt.txt",          AgentKind::ClaudeCode,  AgentState::Running),
        (CLAUDE_RUNNING_QUEUED,           "claude-code/running-with-queued-message.txt",            AgentKind::ClaudeCode,  AgentState::Running),
        (CLAUDE_RUNNING_BASH_TOOL,        "claude-code/running-bash-tool-execution.txt",            AgentKind::ClaudeCode,  AgentState::Running),
        (CLAUDE_RUNNING_BYPASS_BASH,      "claude-code/running-bypass-permissions-bash.txt",        AgentKind::ClaudeCode,  AgentState::Running),
        (CLAUDE_RUNNING_ORBITING,         "claude-code/running-orbiting-thinking.txt",              AgentKind::ClaudeCode,  AgentState::Running),
        (CLAUDE_WAITING_BASH,             "claude-code/waiting-bash-permission.txt",                AgentKind::ClaudeCode,  AgentState::Waiting),
        (CLAUDE_WAITING_EDIT,             "claude-code/waiting-edit-permission.txt",                AgentKind::ClaudeCode,  AgentState::Waiting),
        (CLAUDE_WAITING_WRITE,            "claude-code/waiting-write-permission.txt",               AgentKind::ClaudeCode,  AgentState::Waiting),
        (CLAUDE_CANCELLED,                "claude-code/cancelled.txt",                              AgentKind::ClaudeCode,  AgentState::Idle),
        (CLAUDE_CANCELLED_MID,            "claude-code/cancelled-mid-response.txt",                 AgentKind::ClaudeCode,  AgentState::Idle),

        // ── Codex ───────────────────────────────────────────────────────
        (CODEX_IDLE_FRESH,                "codex/idle-fresh.txt",                                   AgentKind::Codex,       AgentState::Idle),
        (CODEX_IDLE_AFTER_RESPONSE,       "codex/idle-after-response.txt",                          AgentKind::Codex,       AgentState::Idle),
        (CODEX_IDLE_AFTER_DENIED,         "codex/idle-after-denied-permission.txt",                 AgentKind::Codex,       AgentState::Idle),
        (CODEX_IDLE_NEW_FOOTER,           "codex/idle-new-footer.txt",                              AgentKind::Codex,       AgentState::Idle),
        // FIXME: should be Idle — user is mid-typing a prompt
        (CODEX_IDLE_TYPING,               "codex/idle-user-typing.txt",                             AgentKind::Codex,       AgentState::Unknown),
        (CODEX_RUNNING_SPINNER,           "codex/running-spinner.txt",                              AgentKind::Codex,       AgentState::Running),
        // FIXME: should be Running — Codex is actively streaming but `? for shortcuts` stays visible
        (CODEX_RUNNING_STREAMING,         "codex/running-streaming-with-stale-footer.txt",          AgentKind::Codex,       AgentState::Idle),
        // FIXME: should be Running — stale idle markers in scrollback cause false Idle
        (CODEX_RUNNING_STALE_IDLE,        "codex/running-with-stale-idle-markers.txt",              AgentKind::Codex,       AgentState::Idle),
        // FIXME: should be Running — same stale `? for shortcuts` footer issue as above
        (CODEX_RUNNING_STREAMING_LONG,    "codex/running-streaming-long-response.txt",              AgentKind::Codex,       AgentState::Idle),
        // FIXME: should be Running — "Working" indicator was transient, capture shows stale idle footer
        (CODEX_RUNNING_WORKING,           "codex/running-working-indicator.txt",                    AgentKind::Codex,       AgentState::Idle),
        (CODEX_RUNNING_QUEUED,            "codex/running-with-queued-message.txt",                  AgentKind::Codex,       AgentState::Running),
        (CODEX_WAITING_COMMAND,           "codex/waiting-command-permission.txt",                   AgentKind::Codex,       AgentState::Waiting),
        (CODEX_WAITING_TRUST,             "codex/waiting-trust-dialog.txt",                         AgentKind::Codex,       AgentState::Waiting),
        (CODEX_CANCELLED,                 "codex/cancelled.txt",                                    AgentKind::Codex,       AgentState::Idle),
        (CODEX_CANCELLED_MID,             "codex/cancelled-mid-streaming.txt",                      AgentKind::Codex,       AgentState::Idle),

        // ── OpenCode ────────────────────────────────────────────────────
        (OPENCODE_IDLE_FRESH,             "opencode/idle-fresh.txt",                                AgentKind::OpenCode,    AgentState::Idle),
        (OPENCODE_IDLE_AFTER_RESPONSE,    "opencode/idle-after-response.txt",                       AgentKind::OpenCode,    AgentState::Idle),
        (OPENCODE_IDLE_AFTER_REJECTED,    "opencode/idle-after-rejected-permission.txt",            AgentKind::OpenCode,    AgentState::Idle),
        (OPENCODE_RUNNING_SPINNER,        "opencode/running-block-spinner.txt",                     AgentKind::OpenCode,    AgentState::Running),
        (OPENCODE_RUNNING_ESC,            "opencode/running-esc-again-to-interrupt.txt",            AgentKind::OpenCode,    AgentState::Running),
        (OPENCODE_RUNNING_STREAMING_RESP, "opencode/running-streaming-response.txt",                AgentKind::OpenCode,    AgentState::Running),
        (OPENCODE_WAITING_BASH,           "opencode/waiting-bash-permission.txt",                   AgentKind::OpenCode,    AgentState::Waiting),
        (OPENCODE_WAITING_WRITE,          "opencode/waiting-write-permission.txt",                  AgentKind::OpenCode,    AgentState::Waiting),
        (OPENCODE_CANCELLED,              "opencode/cancelled.txt",                                 AgentKind::OpenCode,    AgentState::Idle),
        (OPENCODE_CANCELLED_MID,          "opencode/cancelled-mid-response.txt",                    AgentKind::OpenCode,    AgentState::Idle),

        // ── Cursor CLI ──────────────────────────────────────────────────
        (CURSOR_IDLE_FRESH,               "cursor-cli/idle-fresh.txt",                              AgentKind::CursorAgent, AgentState::Idle),
        (CURSOR_IDLE_AFTER_RESPONSE,      "cursor-cli/idle-after-response.txt",                     AgentKind::CursorAgent, AgentState::Idle),
        (CURSOR_IDLE_PENDING_EDITS,       "cursor-cli/idle-with-pending-edits.txt",                 AgentKind::CursorAgent, AgentState::Idle),
        (CURSOR_IDLE_AFTER_FILE_EDIT,     "cursor-cli/idle-after-file-edit.txt",                    AgentKind::CursorAgent, AgentState::Idle),
        (CURSOR_RUNNING_CTRL_C,           "cursor-cli/running-ctrl-c-to-stop.txt",                 AgentKind::CursorAgent, AgentState::Running),
        (CURSOR_RUNNING_GENERATING,       "cursor-cli/running-generating.txt",                      AgentKind::CursorAgent, AgentState::Running),
        // FIXME: should be Running — Cursor streams without running indicators, only idle footer visible
        (CURSOR_RUNNING_STREAMING,        "cursor-cli/running-streaming.txt",                       AgentKind::CursorAgent, AgentState::Idle),
        (CURSOR_RUNNING_THINKING,         "cursor-cli/running-thinking.txt",                        AgentKind::CursorAgent, AgentState::Running),
        (CURSOR_RUNNING_STREAM_TOKENS,    "cursor-cli/running-streaming-with-token-count.txt",      AgentKind::CursorAgent, AgentState::Running),
        (CURSOR_RUNNING_MESSY,            "cursor-cli/running-with-messy-history.txt",              AgentKind::CursorAgent, AgentState::Running),
        (CURSOR_RUNNING_QUEUED_MSG,       "cursor-cli/running-with-queued-message.txt",             AgentKind::CursorAgent, AgentState::Running),
        (CURSOR_WAITING_COMMAND,          "cursor-cli/waiting-command-approval.txt",                AgentKind::CursorAgent, AgentState::Waiting),
        (CURSOR_WAITING_FILE_DELETION,    "cursor-cli/waiting-file-deletion.txt",                   AgentKind::CursorAgent, AgentState::Waiting),
        (CURSOR_WAITING_TRUST,            "cursor-cli/waiting-trust-workspace.txt",                 AgentKind::CursorAgent, AgentState::Waiting),
        (CURSOR_CANCELLED,                "cursor-cli/cancelled.txt",                               AgentKind::CursorAgent, AgentState::Idle),
        (CURSOR_CANCELLED_MID,            "cursor-cli/cancelled-mid-streaming.txt",                 AgentKind::CursorAgent, AgentState::Idle),

        // ── Gemini CLI ──────────────────────────────────────────────────
        (GEMINI_IDLE_FRESH,               "gemini-cli/idle-fresh.txt",                              AgentKind::Gemini,      AgentState::Idle),
        (GEMINI_IDLE_AFTER_RESPONSE,      "gemini-cli/idle-after-response.txt",                     AgentKind::Gemini,      AgentState::Idle),
        (GEMINI_IDLE_USER_MSG,            "gemini-cli/idle-with-user-message.txt",                  AgentKind::Gemini,      AgentState::Idle),
        (GEMINI_RUNNING_BRAILLE,          "gemini-cli/running-braille-spinner.txt",                 AgentKind::Gemini,      AgentState::Running),
        (GEMINI_RUNNING_LONG,             "gemini-cli/running-long-response.txt",                   AgentKind::Gemini,      AgentState::Running),
        (GEMINI_RUNNING_GOOGLE,           "gemini-cli/running-google-search-tool.txt",              AgentKind::Gemini,      AgentState::Running),
        // FIXME: should be Running — Gemini streams without any running indicator, only idle footer visible
        (GEMINI_RUNNING_STREAMING_RESP,   "gemini-cli/running-streaming-response.txt",              AgentKind::Gemini,      AgentState::Idle),
        (GEMINI_RUNNING_THINKING_STATUS,  "gemini-cli/running-thinking-with-status.txt",            AgentKind::Gemini,      AgentState::Running),
        (GEMINI_WAITING_AUTH,             "gemini-cli/waiting-auth.txt",                            AgentKind::Gemini,      AgentState::Waiting),
        (GEMINI_WAITING_EDIT,             "gemini-cli/waiting-edit-permission.txt",                 AgentKind::Gemini,      AgentState::Waiting),
        (GEMINI_WAITING_SHELL,            "gemini-cli/waiting-shell-permission.txt",                AgentKind::Gemini,      AgentState::Waiting),
        (GEMINI_WAITING_WRITE,            "gemini-cli/waiting-write-permission.txt",                AgentKind::Gemini,      AgentState::Waiting),
        (GEMINI_CANCELLED,                "gemini-cli/cancelled.txt",                               AgentKind::Gemini,      AgentState::Idle),
        (GEMINI_CANCELLED_MID,            "gemini-cli/cancelled-mid-response.txt",                  AgentKind::Gemini,      AgentState::Idle),
    }
    #[test]
    fn all_fixture_captures() {
        let cases = fixture_cases();
        assert!(!cases.is_empty(), "no fixture cases defined");

        let mut failures = Vec::new();
        for case in &cases {
            let actual = detect_state(case.content, case.kind);
            if actual != case.expected {
                failures.push(format!(
                    "  {}: expected {:?}, got {:?}",
                    case.file, case.expected, actual
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "Fixture detection failures ({}/{} failed):\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }
}
