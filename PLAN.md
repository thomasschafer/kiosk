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

Commit: `9c848ed`, refined in `c2334fb`

- Lists all kiosk-managed tmux sessions, runs batched agent detection
- Filters to Waiting/Idle agents, picks oldest `session_activity` (stateless round-robin)
- Skips current session, `--json` output supported
- `TmuxProvider::current_session_name()` added

### ✅ Part 2: Multi-Status Detection (Done)

Commit: `bdc0d38`

- `detect_all_for_session` / `detect_all_for_sessions_batched` in `agent/mod.rs`
- `BranchEntry.agent_statuses: Vec<AgentStatus>` (was `Option<AgentStatus>`)
- CLI formatters and TUI branch picker updated for multiple badges

### ✅ Part 3: Sessions TUI View (Done)

Commit: `6fd1704`

- New `sessions_view.rs` component (197 lines) with `SearchableList` pattern
- `Mode::Sessions` variant, toggled via configurable `toggle_sessions` key
- `kiosk --sessions` / `kiosk -s` flag to open directly into sessions view
- Cross-repo agent poller (`spawn_sessions_agent_poller`) with cancellation token
- Sort: Waiting > Idle > Running > No agent, then by oldest activity
- Enter to switch, Esc to go back, search/filter supported

## Still To Do

- [ ] Manual QA with real agent sessions (multiple agents running across worktrees)
- [ ] Update README with sessions view documentation and keybinding examples
- [ ] Consider `kiosk next` tmux keybinding recipe in README
- [ ] Merge `feat/agent-status` into `main` first (this branch depends on it)
- [ ] Readme generation check (`nix develop -c cargo run -p xtask -- readme --check`)

## Resolved Decisions

- **Sort order**: Waiting > Idle > Running > No agent (Idle above Running because Idle includes potential questions — see #33)
- **`kiosk next` includes Idle**: Yes, because Idle includes agents that may have asked questions
- **Round-robin strategy**: Stateless, using tmux `session_activity` timestamps (oldest first within priority group)
- **Skip current session**: `kiosk next` only switches to a *different* session
- **Sessions view architecture**: New standalone component (not branch picker reuse)
- **`kiosk next` requires tmux**: Yes, like `open` without `--no-switch`
- **Non-kiosk sessions**: Not shown in sessions view
- **`Unknown` state**: Not eligible for `kiosk next`

## Related

- Issue #33: Add 'Asking' agent state to distinguish idle-with-question from idle-done
