# wts — Worktree Switcher

A TUI for navigating git repos and worktrees with tmux session management.

## What it does

- Scans configured directories for git repos
- Shows each repo with its branch and any git worktrees
- Fuzzy search across everything
- Enter opens/attaches a tmux session for that directory
- `Ctrl+W` creates a new worktree from a branch picker
- Shows which entries have active tmux sessions (green dot)

## Config

`~/.config/wts/config.toml`:

```toml
# Directories to scan for git repos (1 level deep)
search_dirs = ["~/Development", "~/Work"]

[session]
# Optional: command to run in a split pane for new sessions
split_command = "hx"
```

## Usage with tmux

Add to your `tmux.conf`:

```tmux
bind-key F popup -xC -yC -w90% -h90% -E "wts"
```

Then `<prefix> F` opens the switcher in a popup.

## Keybindings

| Key | Action |
|-----|--------|
| Type | Fuzzy search |
| ↑/↓ | Navigate |
| Enter | Open/attach session |
| Ctrl+W | New worktree (branch picker) |
| Esc | Quit / close popup |

## Building

```sh
nix develop -c cargo build --release
# or just: cargo build --release (if you have a C linker)
```

Binary will be at `target/release/wts`.
