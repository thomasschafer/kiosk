# Agent Session Switcher — Implementation Plan

Branch: `feat/agent-session-switcher` (based on `feat/agent-status`)

## Overview

Three features that work together to make kiosk an effective agent session manager:

1. **`kiosk next`** — CLI command to jump to the next session needing attention
2. **Multi-status detection** — show all agent statuses per session, not just the highest
3. **Sessions view** — TUI mode showing all active sessions sorted by agent status

## Status

### ✅ Part 1: `kiosk next` CLI Command (Done)

- Attempts to choose session (other than current) as follows (round robin with Waiting > Idle > Running > Unknown):
  - If there is a Waiting session, jump to the oldest by `session_activity`
  - Else, if there is an Idle session, jump to the oldest by `session_activity`
  - Else, if there is a Running session, jump to the oldest by `session_activity`
  - Else, if there is an Unknown session (detected agent, unclassified), jump to the oldest by `session_activity`
  - Else, don't change, show an error or message
- Never switches to the current session if there is another session that is either Waiting or Idle
- `--json` output supported

### ✅ Part 2: Sessions TUI View

- A full view of all sessions
- Sort by recency, like the `choose-tree -s -O time -Z` command
- Re-sort live as statuses and session activity change, while preserving the selected session when possible
- Enter to switch, search/filter supported

### Part 3: Session preview

#### Part 3.1: Monochrome preview

- Split the sessions view in half:
  - Left side: existing session list
  - Right side: preview content for the selected session
- Preview content source: tmux pane capture (plain text / monochrome)
- Refresh behavior:
  - Refresh immediately when selection changes (up/down, search selection change)
  - Refresh the selected session preview every 1 second while staying on the same row
- Config:
  - Add `session_preview.poll_interval_ms` (default `1000`)
  - Allow any positive value (no clamping)
  - Treat `0` (or invalid/non-positive values) as config errors

#### Part 3.2: Full-color preview

- Preserve ANSI styling from tmux capture output
- Render ANSI colors/styles in the TUI preview pane
- Keep behavior from 3.1 (selection-triggered refresh + periodic polling), adding color support only

## Still To Do

- [ ] Manual QA with real agent sessions (multiple agents running across worktrees)
- [ ] Session preview (part 3) in a later PR

## Resolved Decisions

- **Sort order**: Waiting > Idle > Running > No agent (Idle above Running because Idle includes potential questions — see issue #33)
- **Sessions view current row**: Always pin the current session to the top of the sessions list
- **`kiosk next` includes Idle**: Idle includes agents that may have asked questions
- **Round-robin strategy**: Stateless, using tmux `session_activity` timestamps (oldest first within priority group)
- **Skip current session**: `kiosk next` only switches to a *different* session, if one exists, otherwise shows message saying no session to jump to
- **Sessions view architecture**: New standalone component (not branch picker reuse)
- **`Running` and `Unknown` states**: Both eligible for `kiosk next` as lower-priority fallbacks (Running before Unknown)
- **Session preview rollout**: Ship monochrome first (3.1), then full-color ANSI rendering (3.2)
- **Session preview polling**: Refresh on selection changes and poll selected session every 1 second by default
- **Preview poll config policy**: configurable interval in milliseconds; any value `> 0` is accepted; no clamping

## Related

- Issue #33: Add 'Asking' agent state to distinguish idle-with-question from idle-done
