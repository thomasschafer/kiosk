# kiosk

Tmux session manager that manages worktrees for you.

Search for the repo you want, and optionally select a branch: if a session already exists you jump straight in. If one doesn't, a new session is created, with a new worktree if needed.

## What it does

TODO

## Config

TODO: auto-generate

`~/.config/kiosk/config.toml`:

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
bind-key F popup -xC -yC -w90% -h90% -E "kiosk"
```

Then `<prefix> F` opens the switcher in a popup.

## Keybindings

TODO
