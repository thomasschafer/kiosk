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

#### `keys`

Key binding configuration.

<!-- CONFIG END -->


## Keybindings

<!-- KEYS START -->
### General

| Key | Action |
|-----|--------|
| C-c | Quit the application |
| C-h | Show help |

### Repository Selection

| Key | Action |
|-----|--------|
| C-G | Move to bottom |
| C-d | Half page down |
| C-g | Move to top |
| C-n | Move down |
| C-p | Move up |
| C-u | Half page up |
| C-w | Delete word |
| backspace | Delete search character |
| down | Move down |
| end | Cursor to end |
| enter | Open repository |
| esc | Quit the application |
| home | Cursor to start |
| left | Cursor left |
| pagedown | Page down |
| pageup | Page up |
| right | Cursor right |
| tab | Enter repository |
| up | Move up |

### Branch Selection

| Key | Action |
|-----|--------|
| C-G | Move to bottom |
| C-d | Half page down |
| C-g | Move to top |
| C-n | Move down |
| C-o | New branch |
| C-p | Move up |
| C-u | Half page up |
| C-w | Delete word |
| C-x | Delete worktree |
| backspace | Delete search character |
| down | Move down |
| end | Cursor to end |
| enter | Open branch |
| esc | Go back |
| home | Cursor to start |
| left | Cursor left |
| pagedown | Page down |
| pageup | Page up |
| right | Cursor right |
| up | Move up |

### New Branch Base Selection

| Key | Action |
|-----|--------|
| C-G | Move to bottom |
| C-d | Half page down |
| C-g | Move to top |
| C-n | Move down |
| C-p | Move up |
| C-u | Half page up |
| C-w | Delete word |
| backspace | Delete search character |
| down | Move down |
| end | Cursor to end |
| enter | Open branch |
| esc | Go back |
| home | Cursor to start |
| left | Cursor left |
| pagedown | Page down |
| pageup | Page up |
| right | Cursor right |
| up | Move up |

### Confirmation

| Key | Action |
|-----|--------|
| N | Cancel |
| enter | Confirm |
| esc | Cancel |
| n | Cancel |
| y | Confirm |


### Search

In list modes (Repository/Branch Selection), any printable character will start or continue search filtering.
<!-- KEYS END -->
