# kiosk

Tmux session manager that handles worktrees for you.

Search for the repo you want, and optionally select a branch: if a session already exists you jump straight in. If one doesn't, a new session is created, with a new worktree if needed.

## What it does

1. Scans your configured directories for git repos
2. Lets you fuzzy-search for a repo
3. Press Tab to pick a branch (with fuzzy search)
4. Opens or creates a tmux session in the right directory
5. Automatically creates/reuses worktrees for non-default branches

## Configuration

By default, kiosk looks for a TOML configuration file at:

- Linux or macOS: `~/.config/kiosk/config.toml`
- Windows: `%AppData%\kiosk\config.toml`

The following options can be set in your configuration file:

<!-- CONFIG START -->
#### `search_dirs`

Directories to scan for git repositories. Each directory is scanned one level deep.
Supports `~` for the home directory. For example:
```toml
search_dirs = ["~/Development", "~/Work"]
```

### `[session]` section

Layout when creating a new tmux session.

#### `split_command`

Command to run in a split pane when creating a new session. For example, to open
Helix in a vertical split:
```toml
[session]
split_command = "hx"
```

<!-- CONFIG END -->

### Example

```toml
search_dirs = ["~/Development", "~/Work"]

[session]
split_command = "hx"
```

## Usage with tmux

Add to your `tmux.conf`:

```tmux
bind-key F popup -xC -yC -w90% -h90% -E "kiosk"
```

Then `<prefix> F` opens the switcher in a popup.


## Installing

### crates.io

Ensure you have the Rust toolchain installed, then run:

```sh
cargo install kiosk
```

### Building from source

Ensure you have the Rust toolchain installed, then pull down the repo and run:

```sh
cargo install --path kiosk
```

## Keybindings

| Key | Action |
|-----|--------|
| Type | Filter repos/branches |
| Enter | Select repo / open session |
| Tab | Enter branch picker |
| Esc | Back / quit |
