# kiosk

Git-aware tmux session manager.

![kiosk preview](media/preview.png)

Search for the repo you want, and optionally select a branch: if a session already exists you jump straight in. If one doesn't, a new session is created, with a new worktree if needed.

Worktrees are created in `.kiosk_worktrees/` in the parent directory of the given repository. For instance, if you set `search_dirs = ["~/Development"]`, then worktrees are created at `~/Development/.kiosk_worktrees/`.


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

## Usage with tmux

Add to your `tmux.conf`:

```tmux
bind-key F popup -xC -yC -w90% -h90% -E "kiosk"
```

Then `<prefix> F` opens the switcher in a popup.


## Configuration

You'll need a config file to get started. By default, kiosk looks for a TOML configuration file at:

- Linux or macOS: `~/.config/kiosk/config.toml`
- Windows: `%AppData%\kiosk\config.toml`

Here's a basic example:

```toml
search_dirs = ["~/Development", "~/Work"]

[session]
split_command = "hx"
```

### Config options

The following options can be set in your configuration file:

<!-- CONFIG START -->
#### `search_dirs`

Directories to scan for git repositories. Each directory can be scanned to a specified depth, with a default of 1 (i.e. just the top level).
Supports `~` for the home directory. For example:
```toml
search_dirs = ["~/Development", { path = "~/Work", depth = 2 }]
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

### `[theme]` section

Color theme configuration.

#### `accent`

Primary accent color (default: "magenta").

#### `secondary`

Secondary accent color (default: "cyan").

#### `success`

Success/positive color (default: "green").

### `[keys]` section

Key binding configuration.

Defaults are shown below.

```toml
[keys.general]
"C-c" = "quit"
"C-h" = "show_help"

[keys.text_edit]
"C-w" = "delete_backward_word"
"backspace" = "delete_backward_char"
"end" = "move_cursor_end"
"home" = "move_cursor_start"
"left" = "move_cursor_left"
"right" = "move_cursor_right"

[keys.list_navigation]
"A-G" = "move_bottom"
"A-g" = "move_top"
"C-d" = "half_page_down"
"C-n" = "move_down"
"C-p" = "move_up"
"C-u" = "half_page_up"
"down" = "move_down"
"pagedown" = "page_down"
"pageup" = "page_up"
"up" = "move_up"

[keys.confirm_cancel]
"enter" = "confirm"
"esc" = "cancel"

[keys.repo_select]
"enter" = "open_repo"
"esc" = "quit"
"tab" = "enter_repo"

[keys.branch_select]
"C-o" = "new_branch"
"C-x" = "delete_worktree"
"enter" = "open_branch"
"esc" = "go_back"

```

<!-- CONFIG END -->
