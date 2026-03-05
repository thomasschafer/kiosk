# Agent Session Switcher — Implementation Plan

Branch: `feat/agent-session-switcher` (based on `feat/agent-status`)

## Overview

Three features that work together to make kiosk an effective agent session manager:

1. **`kiosk next`** — CLI command to jump to the next session needing attention
2. **Multi-status detection** — show all agent statuses per session, not just the highest
3. **Sessions view** — TUI mode showing all active sessions sorted by agent status

## Status

All three parts are **implemented and passing** (38 tests, clippy clean, fmt clean).

### ✅ Part 1: `kiosk next` CLI Command (Done)

- Attemps to choose session (other than current) as follows (essentially round robin with Waiting > Idle, ignore other statuses):
  - If there is a Waiting session, jump to the oldest by `session_activity`
  - Else, if there is an Idle session, jump to the oldest by `session_activity`
  - Else, if there is a Running session, jump to the oldest by `session_activity`
  - Else, don't change, show an error or message
- Never switches to the current session if there is another session that is either Waiting or Idle
- `--json` output supported

### ✅ Part 2: Sessions TUI View (Done)

- A full ordered view of all sessions
- Sort: Waiting > Idle > Running > No agent, then by activity recency within groups (newest first, unlike `kiosk next` which filters for oldest first within groups)
- Enter to switch, Esc to go back, search/filter supported

## Still To Do

- [ ] Manual QA with real agent sessions (multiple agents running across worktrees)
- [ ] Update README with sessions view documentation and keybinding examples

## Resolved Decisions

- **Sort order**: Waiting > Idle > Running > No agent (Idle above Running because Idle includes potential questions — see issue #33)
- **`kiosk next` includes Idle**: Idle includes agents that may have asked questions
- **Round-robin strategy**: Stateless, using tmux `session_activity` timestamps (oldest first within priority group)
- **Skip current session**: `kiosk next` only switches to a *different* session, if one exists, otherwise shows message saying no session to jump to
- **Sessions view architecture**: New standalone component (not branch picker reuse)
- **`Running` and `Unknown` states**: Not eligible for `kiosk next`

## Related

- Issue #33: Add 'Asking' agent state to distinguish idle-with-question from idle-done
