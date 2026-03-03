# Agent instructions

## Text
Please use sentence case unless some other casing e.g. title case is absolutely necessary.

## Code style
- Code should be as DRY as reasonably possible. This doesn't just apply to exact copies of code: if there are repeated patterns, we should extract these out for re-use when reasonably possible.
- We should aim to use Rust's features to simplify code - better to generate something with a macro or similar than risk it going out of sync. For instance, often when we enumerate over all variants of an enum we could instead use a macro, attributes on the struct fields or similar.

## Dev environment
Use the Nix dev shell for all project tooling commands unless explicitly told otherwise.
This includes build, test, lint, formatting, and any `cargo`/Rust-related command.
This prevents failures due to missing toolchain/system binaries.
Examples: `nix develop -c cargo test`, `nix develop -c cargo run`, `nix develop -c cargo clippy`.

## Agent status debugging
When status is surprising (`UNKNOWN`, `IDLE`, stale `WAITING`), gather these first:

1. `kiosk status <repo> <branch> --json --debug-agent | jq '{agent_status, agent_debug}'`
2. `kiosk panes <repo> <branch> --json`
3. `tmux list-panes -t "<session>" -F '#{pane_id} cmd=#{pane_current_command} title=#{pane_title} dead=#{pane_dead}'`
4. `tmux capture-pane -ep -t "<pane_id>" -S -2000 | tail -n 120`

## tmux runtime validation (safe workflow)
Use manual tmux navigation for exploratory checks. Keep E2E for repeatable automation.

1. Build in dev shell: `nix develop -c cargo build -p kiosk`.
2. Preflight safety: use unique temp names and snapshot sessions:
   `(tmux list-sessions -F '#{session_name}' 2>/dev/null || true) | sort -u > "$BEFORE"`.
3. Launch in detached harness with capture mode:
   `tmux new-session -ds "$HARNESS" "cd <repo_root> && KIOSK_NO_ALT_SCREEN=1 ./target/debug/kiosk --config <temp_config>"`.
4. Drive the TUI with `tmux send-keys`, then re-discover sessions (never assume harness survives `Enter`):
   `(tmux list-sessions -F '#{session_name}' 2>/dev/null || true) | sort -u > "$AFTER"` and `comm -13 "$BEFORE" "$AFTER"`.
5. Verify with `list-panes` + `capture-pane`, then clean up only exact sessions from the diff.

Rules:
- Never target fixed names like `kiosk`.
- Never use wildcard/prefix cleanup logic.
- Never run `tmux kill-session` unless the target came from the pre/post diff computed in the current run.
- Wait briefly (`sleep 1-2`) after launch and after major key actions before querying/capturing.
- `KIOSK_NO_ALT_SCREEN=1` should be the default for agent-driven capture runs.
- Optional full isolation: run all commands on a dedicated tmux socket (`tmux -L <temp-sock> ...`).

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
