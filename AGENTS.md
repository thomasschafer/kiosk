# Agent instructions

## Text
Please use sentence case unless some other casing e.g. title case is absolutely necessary.

## Code style
- Code should be as DRY as reasonably possible. This doesn't just apply to exact copies of code: if there are repeated patterns, we should extract these out for re-use when reasonably possible.
- We should aim to use Rust's features to simplify code - better to generate something with a macro or similar than risk it going out of sync. For instance, often when we enumerate over all variants of an enum we could instead use a macro, attributes on the struct fields or similar.

## State modeling goal
- Prefer type-driven state machines over loose flags.
- Invalid states should be unrepresentable wherever practical.
- Group mode-specific state into typed structs (for example, sessions-only fields in a `SessionsViewState`) rather than keeping them as top-level `AppState` fields.
- Represent lifecycle explicitly with enums (for example, `Loading` vs `Ready`) rather than booleans.
- Prefer typed transitions and helper methods that enforce invariants rather than ad-hoc field mutation spread across files.
- Favor ownership patterns that make cleanup automatic (for example, mode-owned poller handles that cancel on drop).

## Creating PRs
In PR descriptions:
- Don't be too verbose - keep things concise but still include all relevant information for a reviewer.
- Don't add pointless verification sections (e.g. detailing the commands you ran to test).
- Include instructions on how to manually test the changes if possible - if not possible explain briefly why it can't be tested manually and give alternatives to verify the changes.

## Dev environment
Use the Nix dev shell for all project tooling commands unless explicitly told otherwise.
This includes build, test, lint, formatting, and any `cargo`/Rust-related command.
This prevents failures due to missing toolchain/system binaries.
Examples: `nix develop -c cargo test`, `nix develop -c cargo run`, `nix develop -c cargo clippy`.

## Agent status debugging
When status is surprising (`unknown`, `idle`, stale `waiting`), gather these first:

1. `kiosk agents --json`: each pane's status, its source (`osc7501` or `heuristics`) and, for heuristics, the screen rule that matched.
2. `tmux list-panes -a -F '#{pane_id} cmd=#{pane_current_command} title=#{pane_title} dead=#{pane_dead} pipe=#{pane_pipe} collector=#{@kiosk_collector} status=#{@kiosk_status}'`
3. `tmux show-hooks -g | grep 7501` to check kiosk's collector hooks.
4. `tmux capture-pane -ep -t "<pane_id>" -S -2000 | tail -n 120`

## tmux runtime validation (safe workflow)
Use manual tmux navigation for exploratory checks. Keep E2E for repeatable automation.

Always use a dedicated socket (`tmux -L kiosk-test-<unique>`), never the user's own server. Kiosk sets global hooks and pipes panes to collectors in whichever server it runs against, so running it against the user's server changes their setup.

1. Build in dev shell: `nix develop -c cargo build -p kiosk` (the dev shell provides tmux).
2. Start a server on the dedicated socket with a session for the picker, pointing kiosk at a temporary config:
   `tmux -L "$SOCK" -f /dev/null new-session -ds harness -x 120 -y 30 "KIOSK_CONFIG_DIR=<temp_config_dir> KIOSK_STATE_DIR=<temp_state_dir> ./target/debug/kiosk; sleep 30"`.
3. Opening switches the attached client, so attach one from a second dedicated socket:
   `tmux -L "$SOCK-client" -f /dev/null new-session -d "env -u TMUX tmux -L $SOCK attach -t harness"`.
4. Drive the TUI with `tmux -L "$SOCK" send-keys`, then verify with `list-sessions`, `list-panes` and `capture-pane` on the same socket. Wait briefly (`sleep 1-2`) after launch and after major key actions.
5. Clean up with `tmux -L "$SOCK" kill-server` and `tmux -L "$SOCK-client" kill-server`. Never run `kill-server` or `kill-session` without `-L` and a socket you created in this run.

To run kiosk commands from outside the test server, set `KIOSK_TMUX_SOCKET="$SOCK"` and unset `TMUX`.

## Sending keys to agent TUIs in tmux
Agent TUIs use raw terminal input modes. The critical rule: **text and Enter must be separate `tmux send-keys` calls with a brief delay (~300ms+)**. Sending them in the same invocation drops the Enter in most agents.

```bash
# Wrong — Enter is dropped when combined with text:
tmux send-keys -t mysession "hello" Enter

# Correct — separate calls with delay:
tmux send-keys -t mysession "hello"
sleep 0.3
tmux send-keys -t mysession Enter
```

**Alternative for Claude Code specifically:** `tmux send-keys -H 0d` (raw CR byte) also works even in the same call, but the separate-call pattern is more universal across all agents.

Other keys that work normally: `BSpace`, `Escape`, `C-c`, `C-d`, regular text.
