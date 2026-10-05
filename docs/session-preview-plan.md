# Session Preview Pane — Implementation Plan

## Product Vision

The sessions view (`Mode::Sessions`) currently shows a flat list of tmux sessions. This feature adds a **live preview** of the selected session's tmux window below the session list, giving users an at-a-glance view of what's happening in each session without switching to it.

### Layout

Always vertical — session list on top, preview on bottom. The preview gets the full terminal width, which means the tmux window content fits naturally without horizontal clipping (kiosk and the tmux sessions share the same terminal width). The only clipping is vertical (height), which is acceptable.

```
┌─ sessions ──────────────────────────┐
│ ▸ kiosk/main                        │
│   scooter/plugin-refactor            │
│   helix/main                         │
└──────────────────────────────────────┘
┌─ preview ────────────────────────────┐
│ ┌──────────────────┬─────────────────┐│
│ │ ❯ cargo test     │ README.md       ││
│ │                  │ config.toml     ││
│ │ running 42 tests │   1  # Dirs... ││
│ ├──────────────────┤   2  search... ││
│ │ ❯ git status     │   3            ││
│ │ On branch main   │   4  [session] ││
│ └──────────────────┴─────────────────┘│
└──────────────────────────────────────┘
```

### Height Split

The session list gets a fixed allocation: 3 rows for the search bar + enough rows for the list. The preview gets all remaining height, maximising the preview area while keeping the list usable.

### Preview Content

A composite "screenshot" of the entire tmux window — all panes rendered at their correct positions with pane border lines drawn between them. If a session has a left editor pane and right shell pane, you see both in the preview, laid out exactly as tmux renders them.

### Refresh

Preview content refreshes on the same polling interval as agent status detection (the existing `agent_poll_interval`). The preview is cached in state so we don't re-capture on every draw tick.

### Empty States

- No session selected → "Select a session to preview"
- Session has no active window → "No active window"
- Loading → "Loading preview..."
- Failed → "Preview unavailable"

---

## Architecture Decisions

These decisions were reached through a collaborative review between two AI agents, grounded in the existing codebase architecture.

### 1. Preview State Lives on `SessionsViewState`, Not Per-`SessionEntry`

Unlike metadata and agent status (which are small, per-session properties enriching every list row), preview data is:

- **Large** — potentially tens of KB of raw text per window capture (multiple panes × full screen content)
- **Selection-scoped** — only the currently selected session needs a preview
- **Expensive to clone** — `SessionEntry` is frequently cloned through events, reconciliation, and state updates. Adding heavy preview data to every entry would degrade performance for no benefit.

Preview invalidation is simple and orthogonal to session reconciliation: clear on selection change, clear when the selected session disappears from `SessionsDiscovered`.

However, to avoid invalid states (e.g. having a capture but no session name, or vice versa), the preview is modelled as a **typed state machine** rather than loose `Option` fields:

```rust
enum SessionPreviewState {
    Idle,
    Loading { session_name: String },
    Ready { session_name: String, capture: WindowCapture },
    NoActiveWindow { session_name: String },
    Failed { session_name: String, error: String },
}
```

The `NoActiveWindow` variant is separate from `Failed` to allow distinct empty-state rendering ("No active window" vs "Preview unavailable").

### 2. Dedicated Preview Worker, Not Piggybacking on the Sessions Poller

The existing sessions poller owns membership refresh and agent-status refresh. It doesn't receive live selection updates — it tracks `known_session_names` internally and runs on a long-lived background thread.

Coupling preview capture into this poller would be racy (the poller can't know the current selection without a new shared-state channel) and would fight the codebase's preference for typed, mode-owned lifecycles with automatic cleanup.

Instead: a **dedicated preview worker** with its own `PollerHandle`:

- Spawns on selection change (and on entering sessions mode)
- Cancels the previous worker via its `PollerHandle`
- Does an **immediate first capture** (so preview appears instantly on navigation)
- Then refreshes periodically at the agent poll interval
- Stale results are dropped via cancellation token matching

**Important:** The preview poller handle must be stored separately from the agent poller (`SessionsViewState.preview_poller`), **not** installed via `install_sessions_poller()` / `active_agent_poller`. Otherwise selection changes would cancel the sessions agent poller and vice versa. Preview cleanup (cancel) must be handled explicitly on: leaving sessions mode, switching to a session, and entering sessions mode (in case of stale handles).

Note: `PollerHandle` does not cancel on drop — all cancel points must be explicit. Adding `Drop` semantics to the existing cloneable `PollerHandle` is not safe without auditing all current clone patterns.

### 3. Full Window Compositing From the Start

The product requirement is to show the full tmux window with all panes. Starting with active-pane-only would be throwaway work — users expect to see their complete window layout.

The compositor is straightforward (~50-80 lines):
1. tmux gives exact pane positions via `list-panes -F '#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}'`
2. Allocate a 2D grid of `window_width × window_height`
3. Place each pane's captured text at its `(left, top)` coordinates
4. Fill gap cells with simple border characters (`│` for vertical, `─` for horizontal)

No fancy junction detection for v1 — just basic gap fills.

### 4. Compositing in `kiosk-tui`, Not `kiosk-core`

The current crate split keeps `kiosk-core` UI-agnostic and state-focused. Compositing and rendering are UI concerns. So:

- **`kiosk-core`:** tmux capture types (`WindowCapture`, `PaneCapture`) and provider method only
- **`kiosk-tui`:** compositor, preview widget, and (later) ANSI style parsing

### 5. Plain Text First, Styled ANSI Deferred

Get the layout, compositing, and preview lifecycle right before adding ANSI colour parsing. `tmux capture-pane -e` gives ANSI escape sequences that can be parsed into ratatui styles, but that's a clear enhancement phase.

### 6. Explicit tmux Active-Window Targeting

The provider API is careful about exact targeting (e.g. `={session}` syntax). For multi-window sessions, we must explicitly target the active window rather than letting tmux pick.

**Coherent snapshot:** Use a single `list-panes -t =SESSION -F ...` call that includes `#{window_id}`, `#{window_width}`, and `#{window_height}` alongside pane geometry. This eliminates the race condition of separate calls for layout and window size. If multiple window IDs appear (shouldn't happen without `-s`), use the first consistently.

### 7. Centralised Selection-Change Detection

Selection changes can happen via many paths: direct movement, page movement, search edits, jump-to-agent actions, and async reconciliation after `SessionsDiscovered` / metadata / agent patches. Rather than wiring preview spawning into every action branch, add a single helper like `sync_session_preview_to_selection(...)` and call it after any action/event that may affect sessions selection or membership.

### 8. Correct Capture Flags for Visual Preview

The existing `capture_pane_content` uses `-J` (join wrapped lines) for agent detection. For visual preview, `-J` is **wrong** because joined lines no longer match pane row positions. Use `capture-pane -p` without `-J`, capturing the current visible screen only (`-S 0 -E -1` or equivalent). Clip/pad each line to `pane_width` and ensure exactly `pane_height` rows.

### 9. Defensive Horizontal Clipping

While kiosk and tmux sessions typically share the same terminal width, this isn't guaranteed — detached sessions, clients on other terminals, and `KIOSK_TMUX_SOCKET` scenarios can produce different widths. The renderer must clip horizontally if `window_width > preview_inner_width`, not assume only vertical clipping is needed.

---

## Implementation Plan

### Phase 1: tmux Capture Infrastructure (`kiosk-core`)

**New types:**

```rust
/// Position and content of a single pane within a window.
pub struct PaneCapture {
    pub pane_id: String,
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
    pub content: String,
    pub active: bool,
}

/// Complete snapshot of a tmux window's visual state.
pub struct WindowCapture {
    pub window_width: u32,
    pub window_height: u32,
    pub panes: Vec<PaneCapture>,
}
```

**New `TmuxProvider` method:**

```rust
fn capture_window(&self, session: &str) -> anyhow::Result<WindowCapture>;
```

Implementation in `CliTmuxProvider`:
1. Single `tmux list-panes -t =SESSION -F '#{window_id}|#{window_width}|#{window_height}|#{pane_id}|#{pane_left}|#{pane_top}|#{pane_width}|#{pane_height}|#{pane_active}'` — layout + window dimensions in one call (coherent snapshot, no race)
2. For each pane: `tmux capture-pane -t PANE_ID -p` (without `-J`) — raw visible content preserving row positions
3. Clip/pad each pane's content to exactly `pane_width` × `pane_height`
4. Assemble and return `WindowCapture`

Mock provider: add `window_captures: HashMap<String, WindowCapture>`.

### Phase 2: Preview State & Worker

**State (`kiosk-core/src/state.rs`):**

```rust
pub enum SessionPreviewState {
    Idle,
    Loading { session_name: String },
    Ready { session_name: String, capture: WindowCapture },
    Failed { session_name: String, error: String },
}
```

Add to `SessionsViewState`:
- `pub preview: SessionPreviewState`
- `pub preview_poller: Option<PollerHandle>` (or a dedicated handle type)

**Event (`kiosk-core/src/event.rs`):**

```rust
AppEvent::SessionPreviewReady {
    session_name: String,
    capture: WindowCapture,
}

AppEvent::SessionPreviewFailed {
    session_name: String,
    error: String,
}
```

**Worker (`kiosk-tui/src/app/spawn.rs`):**

New function `spawn_preview_worker`:
- Takes session name, tmux provider, event sender, cancel token, poll interval
- Immediately captures on spawn (no delay)
- Loops: sleep for poll interval, capture again, send event
- Checks cancel token before each iteration

**Integration (`kiosk-tui/src/app/mod.rs`):**

Add a centralised helper `sync_session_preview_to_selection(state, tmux, sender)` that:
- Reads the currently selected session name
- If it matches the current preview target, does nothing
- Otherwise: cancels existing preview worker, sets state to `Loading`, spawns new worker
- If no session is selected, cancels worker and sets `Idle`

Call this helper after any action/event that may affect sessions selection or membership (movement, search, page movement, jump-to-agent, `SessionsDiscovered`, metadata/agent patches, reconciliation).

Event handling:
- `SessionPreviewReady`: if `session_name` matches current `Loading`/`Ready` session, transition to `Ready`; otherwise discard
- `SessionPreviewFailed`: transition to `Failed` (same session name check)
- `SessionsDiscovered`: if the `Loading`/`Ready` session disappeared, reset to `Idle`

Preview cleanup (explicit cancel) on:
- Leaving sessions mode (back to repos)
- Switching to a session (entering it)
- Re-entering sessions mode (cancel any stale handle)

### Phase 3: Layout & Preview Widget (`kiosk-tui`)

**Layout change in `sessions_view.rs`:**

```rust
let chunks = Layout::vertical([
    Constraint::Length(3),           // search bar
    Constraint::Length(list_height), // session list
    Constraint::Min(8),             // preview
]).split(area);
```

`list_height`: adaptive — `min(filtered_count + 2, area.height / 3)` with a reasonable cap.

**Fix `active_list_page_rows`:** Update the calculation in `mod.rs` to account for the reduced list area when in sessions mode.

**New compositor (`kiosk-tui/src/components/session_preview.rs`):**

```rust
pub fn draw(
    f: &mut Frame,
    area: Rect,
    preview_state: &SessionPreviewState,
    theme: &Theme,
) { ... }
```

Rendering:
1. Draw a bordered block with title " preview "
2. For `Ready` state: composite the `WindowCapture` into a 2D character grid, render into the inner area (clipping vertically if needed)
3. For other states: render the appropriate empty-state message

Compositor logic:
1. Create a `Vec<Vec<char>>` grid of `window_width × window_height`
2. For each pane, split content into lines, place characters at `(pane.left + col, pane.top + row)`
3. Fill uncovered cells with border characters based on adjacency (vertical gap → `│`, horizontal gap → `─`)
4. Render visible rows into the ratatui buffer

### Phase 4: Edge Cases

- **Zoomed panes:** tmux reports the zoomed pane as filling the entire window. The compositor handles this naturally — one pane at full window dimensions.
- **Help overlay:** Already draws on top of the sessions view. No change needed.
- **Unresolved sessions:** Still tmux sessions — they get previews too.
- **Multi-window sessions:** We capture the active window only.
- **Session with no panes:** Show "No active window" state.

---

## Test Plan

1. **Preview state transitions:** Idle → Loading → Ready, Loading → Failed, Ready → Idle on selection change
2. **Stale result rejection:** Preview for session A arrives after user moved to session B → discarded
3. **Session disappearance:** Selected session removed from `SessionsDiscovered` → preview resets to Idle
4. **Compositor:** Multi-pane layouts produce correct grid (2-pane horizontal, 2-pane vertical, 3-pane mixed)
5. **Compositor edge cases:** Single pane (no borders), empty content, varying pane sizes
6. **List paging:** `active_list_page_rows` correctly computed with the new three-way layout
7. **Empty states:** No selection, loading, failed states render correctly
8. **Preview preserved across patches:** Metadata and agent-status patches for the same session don't clear the preview
9. **Preview poller independence:** Cancelling preview doesn't cancel sessions agent poller, and vice versa
10. **Preview cleanup on mode exit:** Preview worker is not running after leaving sessions mode
11. **Horizontal clipping:** Preview renders correctly when `window_width > preview_inner_width`

---

## Files to Create

- `kiosk-tui/src/components/session_preview.rs` — preview widget + compositor

## Files to Modify

- `kiosk-core/src/tmux/provider.rs` — add `capture_window` to trait, `WindowCapture`/`PaneCapture` types (re-export from `tmux/mod.rs`)
- `kiosk-core/src/tmux/cli.rs` — implement `capture_window` for `CliTmuxProvider`
- `kiosk-core/src/tmux/mock.rs` — mock implementation
- `kiosk-core/src/event.rs` — add `SessionPreviewReady`/`SessionPreviewFailed` events
- `kiosk-core/src/state.rs` — add `SessionPreviewState`, preview fields on `SessionsViewState`
- `kiosk-tui/src/components/sessions_view.rs` — three-way layout split, integrate preview
- `kiosk-tui/src/components/mod.rs` — register new component
- `kiosk-tui/src/app/mod.rs` — handle new events, preview lifecycle on selection change
- `kiosk-tui/src/app/spawn.rs` — `spawn_preview_worker` function

---

## Future Enhancements

- **ANSI styled preview:** Use `capture-pane -e` and parse escape sequences into ratatui styles
- **Preview scroll:** Allow scrolling the preview vertically to see clipped content
- **Preview toggle:** Keybinding to hide/show the preview pane
- **Border junction detection:** Proper `┼`/`├`/`┤`/`┬`/`┴` at border intersections
