# Agent instructions

## Dev environment
Use the Nix dev shell for all project tooling commands unless explicitly told otherwise.
This includes build, test, lint, formatting, and any `cargo`/Rust-related command.
This prevents failures due to missing toolchain/system binaries.
Examples: `nix develop -c cargo test`, `nix develop -c cargo run`, `nix develop -c cargo clippy`.

## tmux runtime validation (safe workflow)
Use manual tmux navigation for exploratory checks; this is the primary workflow.
E2E tests should cover repeatable automation checks.

1. Build in dev shell:
   `nix develop -c cargo build -p kiosk`.
2. Preflight safety:
   Use unique temp names only (for harness/repo), snapshot sessions before test:
   `(tmux list-sessions -F '#{session_name}' 2>/dev/null || true) | sort -u > "$BEFORE"`.
3. Launch kiosk in detached harness with capture-friendly mode:
   `tmux new-session -ds "$HARNESS" "cd <repo_root> && KIOSK_NO_ALT_SCREEN=1 ./target/debug/kiosk --config <temp_config>"`.
4. Drive the TUI:
   send keys via `tmux send-keys` (search text, `Enter`, `Tab`, `Esc`, etc.).
5. Re-discover sessions after interaction:
   never assume harness survives `Enter`.
   `(tmux list-sessions -F '#{session_name}' 2>/dev/null || true) | sort -u > "$AFTER"`
   `comm -13 "$BEFORE" "$AFTER"`.
6. Verify results:
   `tmux list-panes -t "<session>" -F '#{pane_id} cmd=#{pane_current_command} dead=#{pane_dead}'`
   `tmux send-keys -t "<pane_id>" 'echo AGENT_TMUX_MARKER' Enter`
   `tmux capture-pane -ep -t "<pane_id>" -S -2000 | rg 'AGENT_TMUX_MARKER|KIOSK_SPLIT_OK|kiosk|select repo|select branch'`.
7. Cleanup:
   kill only exact session names from the pre/post diff list:
   `tmux kill-session -t "$s"`.

Rules:
- Never target fixed names like `kiosk`.
- Never use wildcard/prefix cleanup logic.
- Never run `tmux kill-session` unless the target came from the pre/post diff computed in the current run.
- Wait briefly (`sleep 1-2`) after launch and after major key actions before querying/capturing.
- `KIOSK_NO_ALT_SCREEN=1` should be the default for agent-driven capture runs.
- Optional full isolation: run all commands on a dedicated tmux socket (`tmux -L <temp-sock> ...`).

Success checklist:
- `capture-pane -ep` shows kiosk UI text (for example `kiosk — select repo` or `select branch`).
- Pre/post session diff contains the expected app-created session.
- `list-panes` for the app session matches expected layout (main pane + split pane if configured).
- Marker command grep passes in at least one target pane (`AGENT_TMUX_MARKER` / `KIOSK_SPLIT_OK`).
