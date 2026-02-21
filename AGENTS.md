# Agent instructions

## Dev environment
Use the Nix dev shell for all project tooling commands unless explicitly told otherwise.
This includes build, test, lint, formatting, and any `cargo`/Rust-related command.
This prevents failures due to missing toolchain/system binaries.
Examples: `nix develop -c cargo test`, `nix develop -c cargo run`, `nix develop -c cargo clippy`.

## tmux runtime validation (safe workflow)
To fully test kiosk end to end you can build and run in tmux, and capture output:

1. Build in dev shell: `nix develop -c cargo build -p kiosk`.
2. Snapshot sessions before: `tmux list-sessions -F '#{session_name}' | sort -u > "$BEFORE"`.
3. Use unique temp names only (`harness-kiosk-agent-<ts>-<pid>`) and a temp repo + temp config.
4. Launch detached:
   `tmux new-session -ds "$HARNESS" "cd <repo_root> && ./target/debug/kiosk --config <temp_config>"`.
5. Drive with `tmux send-keys` (repo name, then `Enter`).
6. Do not assume harness survives `Enter`; re-discover sessions.
7. Snapshot after and diff:
   `tmux list-sessions -F '#{session_name}' | sort -u > "$AFTER"`
   `comm -13 "$BEFORE" "$AFTER"`.
8. Verify panes and output markers:
   `tmux list-panes -t "<session>" -F '#{pane_id} cmd=#{pane_current_command} dead=#{pane_dead}'`
   `tmux send-keys -t "<pane_id>" 'echo AGENT_TMUX_MARKER' Enter`
   `tmux capture-pane -ep -t "<pane_id>" -S -2000 | rg 'AGENT_TMUX_MARKER|KIOSK_SPLIT_OK'`.
9. Cleanup only sessions from diff: `tmux kill-session -t "$s"`.

Never target fixed names like `kiosk`. Optional: use `tmux -L <temp-sock> ...` for full isolation.
Safety preflight: never use fixed names, wildcards, or prefix matches for cleanup; kill only exact names from the diff list.
Reliability: after launching kiosk and after pressing `Enter`, wait briefly (`sleep 1-2`) before the next tmux query/send.
Output caveat: TUI capture may be sparse; confirm behavior via created sessions/panes plus explicit marker commands.
Permission note: in constrained sandboxes, tmux socket or Nix daemon access may require elevated command permissions.

### TODO
- Add a debug mode (for example `KIOSK_NO_ALT_SCREEN=1`) that disables alternate-screen rendering so `tmux capture-pane` output is readable and reliable for agent/CI validation.
