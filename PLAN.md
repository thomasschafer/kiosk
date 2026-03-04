# Agent Session Switcher — Implementation Plan

Branch: `feat/agent-session-switcher` (based on `feat/agent-status`)

## Overview

Two features that work together to make kiosk an effective agent session manager:

1. **`kiosk next`** — CLI command to jump to the next session needing attention
2. **Sessions view** — TUI mode showing all active sessions sorted by agent status

---

## Part 1: Multi-Status Detection

Currently `detect_for_session` returns only the highest-priority agent status per session. We need all detected agents per session so the TUI can show e.g. `W R` (one waiting, one running) on a single row.

### Changes

- **`kiosk-core/src/agent/mod.rs`**: Add `detect_all_for_session` (or rename existing) that returns `Vec<DetectionResult>` instead of `Option<DetectionResult>`
- Keep the existing `detect_for_session` as a convenience wrapper (returns `best()` from the vec) for backwards compat in CLI commands that only need the top status
- **`kiosk-core/src/state.rs`**: Change `BranchEntry.agent_status: Option<AgentStatus>` to `agent_statuses: Vec<AgentStatus>` (or keep both — single for priority logic, vec for display)
- **TUI branch picker**: Update rendering to show multiple status badges when present
- **CLI formatters**: Show all statuses in table output

### Display Format

For a session with a Waiting and a Running agent:
- TUI badge: `[W] [R]` (colored per status)
- CLI table: `[WAITING] [RUNNING]`

Single-agent sessions look identical to today — no visual change.

---

## Part 2: `kiosk next` CLI Command

A subcommand that finds the next session needing attention and switches to it.

### Behaviour

1. List all tmux sessions that kiosk manages (same logic as `cmd_sessions`)
2. Run batched agent detection on all of them
3. Filter to sessions with at least one agent in **Waiting** or **Idle** state
4. Sort: Waiting first, then Idle (within same priority, use stable session name order)
5. Find the current tmux session in the filtered list
6. Pick the **next** session after current (wrapping around)
7. If no eligible sessions: print message to stderr + exit code 1
8. If eligible: `tmux switch-client -t <session>`

### Round-Robin (Stateless)

The "next after current" approach gives natural round-robin:
- If you're on session A (waiting) and B (idle) and C (waiting) exist → switches to next in sorted order
- Calling `kiosk next` repeatedly cycles through all eligible sessions
- If current session isn't in the eligible list, picks the first eligible one

### CLI Interface

```
kiosk next [--json]
```

Output (non-JSON): `Switched to: kiosk--feat-agent-status (Waiting)`
Output (no match): `No agent sessions need attention` (exit 1)

JSON output:
```json
{
  "switched": true,
  "session": "kiosk--feat-agent-status",
  "agent_state": "Waiting"
}
```

### tmux Keybinding

User adds to `.tmux.conf`:
```
bind-key N run-shell "kiosk next"
```

Or with a popup for feedback:
```
bind-key N display-popup -d '#{pane_current_path}' -w 50 -h 3 'kiosk next 2>&1; sleep 0.5'
```

---

## Part 3: Sessions View in TUI

A new view mode showing all active sessions as a flat list, sorted by agent status.

### Sort Order

1. **Waiting** agents first
2. **Idle** agents
3. **Running** agents  
4. Sessions without agents (sorted by existing repo ordering: recency then alphabetical)

Within the same status tier, sort by session name for stability.

### Display

Each row shows:
```
repo-name/branch-name    [WAITING] [RUNNING]    /path/to/worktree
```

Or for single-agent:
```
repo-name/branch-name    [IDLE]                  /path/to/worktree
```

Sessions without agents:
```
repo-name/branch-name                            /path/to/worktree
```

### Navigation

- **Enter**: Switch to the selected session (same as current repo list → branch picker → Enter)
- **Search**: Filter by repo name, branch name, or session name
- **Toggle**: Keybinding to switch between repo view and sessions view (e.g. `Ctrl+S` or a configurable key)
- **CLI flag**: `kiosk --sessions` or `kiosk -s` to open directly into sessions view

### Architecture

- New `SessionsView` component in `kiosk-tui/src/components/`
- New `ViewMode` enum: `Repos` | `Sessions` in `AppState`
- Sessions view fetches data similarly to `cmd_sessions` but uses batched detection
- Agent status polling reuses existing infrastructure (poll interval from config)

---

## Implementation Order

1. **Multi-status detection** — core change that both features depend on
2. **`kiosk next`** — simpler, immediately useful with a tmux keybinding
3. **Sessions TUI view** — larger UI work, builds on the same data

---

## Open Questions

- Should `kiosk next` require being inside tmux? (Probably yes — it calls `tmux switch-client`)
- Should the sessions view show non-kiosk tmux sessions? (Probably no — stick to sessions kiosk knows about)
- Config key for the view toggle keybinding? (Add to `[keys]` section)
- Should `kiosk next` also work with `Unknown` state agents? (Probably no — Unknown means we can't determine state, not necessarily needs attention)

---

## Related

- Issue #33: Add 'Asking' agent state (future improvement to idle detection)
