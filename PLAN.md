# Agent Session Switcher — Implementation Plan

Branch: `feat/agent-session-switcher` (based on `feat/agent-status`)

## Overview

Three features that work together to make kiosk an effective agent session manager:

1. **`kiosk next`** — CLI command to jump to the next session needing attention
2. **Multi-status detection** — show all agent statuses per session, not just the highest
3. **Sessions view** — TUI mode showing all active sessions sorted by agent status

## Implementation Order

1. `kiosk next` — immediately useful, self-contained
2. Multi-status detection — core change needed for sessions view display
3. Sessions TUI view — largest piece, builds on the above

---

## Part 1: `kiosk next` CLI Command

A subcommand that finds the next session needing attention and switches to it.

### Behaviour

1. List all tmux sessions that kiosk manages (same logic as `cmd_sessions`)
2. Run batched agent detection on all of them
3. Filter to sessions with at least one agent in **Waiting** or **Idle** state (Idle includes agents that may have asked a question — see #33)
4. Group by priority: Waiting first, then Idle
5. Within each group, pick the session with the **oldest `session_activity` timestamp** (least recently visited — avoids bouncing between the same sessions)
6. Skip the current tmux session — only switch to a *different* eligible session
7. If no other eligible session exists: show a message and exit code 1
8. If eligible: `tmux switch-client -t <session>`

### Why Oldest Activity Works

Tmux updates `session_activity` when you interact with a session. By always picking the least-recently-active session in the highest priority group, you get natural round-robin without any state file. Switch to A → A's timestamp updates → next call picks B → etc.

### Prerequisite

Add `TmuxProvider::current_session_name() -> Option<String>` to the trait (needed to know what to skip).

### CLI Interface

```
kiosk next [--json]
```

Output (non-JSON): `Switched to: kiosk--feat-agent-status (Waiting)`
Output (no match): `No other agent sessions need attention` (exit 1)
Must be inside tmux (same requirement as `open` without `--no-switch`).

JSON output:
```json
{
  "switched": true,
  "session": "kiosk--feat-agent-status",
  "agent_state": "Waiting"
}
```

### tmux Keybinding Usage

For instant switching (background, no visible output):
```
bind-key N run-shell "kiosk next 2>/dev/null"
```

With a brief popup showing the result:
```
bind-key N display-popup -w 60 -h 3 -E 'kiosk next 2>&1 || true; sleep 0.8'
```

The popup approach is nice because it shows "Switched to: ..." or "No other agent sessions need attention" for ~1 second then disappears. The background approach is faster but silent.

---

## Part 2: Multi-Status Detection

Currently `detect_for_session` returns only the highest-priority agent status per session. We need all detected agents so the TUI and CLI can show e.g. `[WAITING] [IDLE]` on a single row.

### Changes

- **`kiosk-core/src/agent/mod.rs`**:
  - Add `detect_all_for_session` returning `Vec<DetectionResult>` (collect all instead of keeping only `best`)
  - Add `detect_all_for_sessions_batched` counterpart
  - Keep existing `detect_for_session` as convenience wrapper (`.first()` by priority) for code that only needs the top status (e.g. `kiosk next`, `wait`)
- **`kiosk-core/src/state.rs`**: Change `BranchEntry.agent_status: Option<AgentStatus>` → `agent_statuses: Vec<AgentStatus>`
- **TUI branch picker**: Render multiple badges when present
- **CLI formatters**: Show all statuses in table output

### Display Format

Multiple agents: `[WAITING] [RUNNING]`
Single agent: `[IDLE]` (identical to today)
No agents: no badge (identical to today)

---

## Part 3: Sessions View in TUI

A new view mode showing all active sessions as a flat list, sorted by agent status.

### Sort Order

1. **Waiting** agents first
2. **Idle** agents (includes agents that may have asked a question)
3. **Running** agents
4. Sessions without agents (existing repo ordering)

Within the same status tier, sort by oldest `session_activity` (least recently visited first — consistent with `kiosk next` philosophy).

### Display

Each row shows:
```
repo-name/branch-name    [WAITING] [RUNNING]    /path/to/worktree
```

### Architecture

- **New component**: `kiosk-tui/src/components/sessions_view.rs` — new standalone component using the same `SearchableList` pattern as repo_list/branch_picker (not shoehorned into branch picker, which is inherently single-repo scoped)
- **New `Mode` variant**: Add `Sessions` to the `Mode` enum
- **Toggle keybinding**: Configurable key (e.g. `Ctrl+S`) to switch between repo view and sessions view, added to `[keys]` config
- **CLI flag**: `kiosk --sessions` or `kiosk -s` to open directly into sessions view
- **Data**: Fetched similarly to `cmd_sessions` using batched detection
- **Polling**: Cross-repo agent poller (current poller is scoped to single repo's branches — sessions view needs broader scope). Cancel branch-scoped poller when entering sessions view, start session-scoped one.

### Navigation

- **Enter**: Switch to the selected session
- **Search**: Filter by repo name, branch name, or session name
- **Esc**: Back to repo view (or quit if opened with `--sessions`)

---

## Resolved Decisions

- **Sort order**: Waiting > Idle > Running > No agent (Idle above Running because Idle includes potential questions — see #33)
- **`kiosk next` includes Idle**: Yes, because Idle includes agents that may have asked questions
- **Round-robin strategy**: Stateless, using tmux `session_activity` timestamps (oldest first within priority group)
- **Skip current session**: `kiosk next` only switches to a *different* session
- **Sessions view architecture**: New standalone component (not branch picker reuse)
- **`kiosk next` requires tmux**: Yes, like `open` without `--no-switch`
- **Non-kiosk sessions**: Not shown in sessions view
- **`Unknown` state**: Not eligible for `kiosk next`

---

## Related

- Issue #33: Add 'Asking' agent state to distinguish idle-with-question from idle-done
