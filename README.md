# kiosk

Kiosk is a Git-aware tmux session manager, which shows the status of your AI agents (waiting for permissions, idle or running).

Search for the repo you want, and optionally select a branch or create a new one. If a session already exists, you jump straight in - if it doesn't, a new session is created, with a new worktree if needed.

![kiosk preview](media/preview.png)

Worktrees are created in `.kiosk_worktrees/` in the parent directory of the given repository. For instance, if you set `search_dirs = ["~/Development"]`, then worktrees are created at `~/Development/.kiosk_worktrees/`.


## Usage

### TUI

Kiosk has two primary modes, explained below.

#### Repos and branches view

Add a keybinding to your `tmux.conf` to run `kiosk` in a popup (the following uses `<prefix> f`, but change as appropriate):

```tmux
bind-key f popup -xC -yC -w90% -h90% -E "kiosk"
```

You'll start in the repo view, which shows all repos in the folders defined in your config:
- Start typing to fuzzy search across repos
- Enter opens the repo with the primary checkout
- Tab opens the branch view for that repo

From the branch view, you can again fuzzy match across branches:
- Enter opens a session in a worktree on that branch, either attaching to an existing session if one exists, or creating a new one otherwise

#### Sessions view

Add a keybinding to your `tmux.conf` to run `kiosk --sessions` in a popup (the following uses `<prefix> g`, but change as appropriate):

```tmux
bind-key g popup -xC -yC -w90% -h90% -E "kiosk --sessions"
```

This view shows all running tmux sessions, along with the status of any agents.

#### Config

On first launch (when no config file exists), you'll see a setup wizard to create your config file. If you'd rather do this manually, see the [configuration](#configuration) section.

### CLI

You can also use Kiosk as a CLI, which is particularly useful for AI agents. Below are some example commands, but see `kiosk --help` for a complete list of commands and options.

<details>

#### Examples

```bash
# List repos
kiosk list --json

# List branches with metadata
kiosk branches my-project --json

# Create a new branch, worktree, and tmux session (without attaching)
kiosk open my-project --new-branch feat/thing --base main --no-switch --json

# Launch a command in the session (the command is typed and Enter is sent automatically)
kiosk open my-project feat/thing --no-switch --run "your-command-here" --log --json

# Send a follow-up command to an existing session
kiosk send my-project feat/thing --command "another-command" --json

# Send raw tmux keys (e.g. for TUI interaction — no Enter appended)
kiosk send my-project feat/thing --keys "C-c" --json
kiosk send my-project feat/thing --keys "Escape" --json

# Send literal text without appending Enter
kiosk send my-project feat/thing --text "y" --json

# Target a specific pane (default: 0)
kiosk send my-project feat/thing --command "ls" --pane 1 --json
kiosk status my-project feat/thing --pane 1 --json

# List panes in a session
kiosk panes my-project feat/thing --json

# Check session status
kiosk status my-project feat/thing --json

# Include agent detection debug metadata (kind/state source + matched rules)
kiosk status my-project feat/thing --json --debug-agent

# List active kiosk sessions (includes last_activity, pane_count, current_command)
kiosk sessions --json

# Switch to the oldest other tmux session whose detected agent is idle, waiting, or unknown
kiosk next --json

# Read session logs
kiosk log my-project feat/thing --tail 100 --json

# Show resolved configuration
kiosk config show --json

# Non-interactive cleanup of orphaned worktrees
kiosk clean --yes --json

# Delete a specific worktree and session when done
kiosk delete my-project feat/thing --force --json
```

#### Waiting for completion

Use `--wait` on `open` to block until the command finishes:

```bash
# Launch, wait for completion, then read output — all in one
kiosk open my-project feat/thing --no-switch --run "cargo test" --wait --wait-timeout 300 --log --json
kiosk log my-project feat/thing --tail 200 --json
```

Or use the standalone `wait` command for commands sent later:

```bash
kiosk send my-project feat/thing --command "cargo test"
kiosk wait my-project feat/thing --timeout 300 --json
```

#### Session naming

Kiosk names tmux sessions deterministically:
- Main checkout: `<repo-name>` (dots replaced with `_`)
- Branch worktree: `<repo-name>--<branch>` (with `/` replaced by `-`, `.` replaced by `_`)

The `open --json` response includes the exact session name in the `session` field.

#### Agent status troubleshooting

When agent state looks wrong (`UNKNOWN` vs `IDLE`/`RUNNING`), capture both Kiosk's debug metadata and raw pane tail:

```bash
# Show detected kind/state and which rule matched
kiosk status my-project feat/thing --json --debug-agent | jq '{agent_status, agent_debug}'

# Confirm pane command/title and target pane ids
kiosk panes my-project feat/thing --json
tmux list-panes -t my-project--feat-thing -F '#{pane_id} cmd=#{pane_current_command} title=#{pane_title}'

# Inspect recent pane content (strip ANSI, large enough tail)
tmux capture-pane -ep -t my-project--feat-thing:0.0 -S -2000 | tail -n 80
```

</details>

## Installing

### Homebrew

```sh
brew install thomasschafer/tap/kiosk
```

### Cargo

Ensure you have the Rust toolchain installed, then run:

```sh
cargo install kiosk
```

### Prebuilt binaries

Download the appropriate binary for your system from the [releases page](https://github.com/thomasschafer/kiosk/releases/latest):

| Platform | Architecture | Download file |
|-|-|-|
| Linux | Intel/AMD | `*-x86_64-unknown-linux-musl.tar.gz` |
| Linux | ARM64 | `*-aarch64-unknown-linux-musl.tar.gz` |
| macOS | Apple Silicon| `*-aarch64-apple-darwin.tar.gz` |
| macOS | Intel | `*-x86_64-apple-darwin.tar.gz` |
| Windows | x64 | `*-x86_64-pc-windows-msvc.zip` |

After downloading, extract the binary and move it to a directory in your `PATH`.

### Building from source

Ensure you have the Rust toolchain installed, then pull down the repo and run:

```sh
cargo install --path kiosk
```


## Configuration

By default, Kiosk looks for a TOML configuration file at:

- Linux or macOS: `~/.config/kiosk/config.toml`
- Windows: `%AppData%\kiosk\config.toml`

Here's a minimal example that contains all required keys (change values as appropriate):

```toml
search_dirs = ["~/Development"]
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

Colors can be a named color (`black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `gray`, `dark_gray`) or a hex value (`#rrggbb`).

Defaults:

```toml
[theme]
accent = "magenta"
secondary = "cyan"
tertiary = "green"
success = "green"
error = "red"
warning = "yellow"
muted = "dark_gray"
border = "dark_gray"
hint = "blue"
highlight_fg = "black"

[theme.status]
running = "green"
waiting = "yellow"
idle = "cyan"
unknown = "blue"
```

### `[keys]` section

Key binding configuration.
To unbind an inherited key mapping, assign it to `noop`.

Defaults are shown below.

```toml
[keys.general]
"C-c" = "quit"
"esc" = "quit"
"C-h" = "show_help"
"C-s" = "toggle_sessions"

[keys.text_edit]
"backspace" = "delete_backward_char"
"del" = "delete_forward_char"
"C-d" = "delete_forward_char"
"C-w" = "delete_backward_word"
"A-backspace" = "delete_backward_word"
"A-d" = "delete_forward_word"
"C-u" = "delete_to_start"
"C-k" = "delete_to_end"
"left" = "move_cursor_left"
"right" = "move_cursor_right"
"A-b" = "move_cursor_word_left"
"A-left" = "move_cursor_word_left"
"A-f" = "move_cursor_word_right"
"A-right" = "move_cursor_word_right"
"home" = "move_cursor_start"
"C-a" = "move_cursor_start"
"end" = "move_cursor_end"
"C-e" = "move_cursor_end"

[keys.list_navigation]
"up" = "move_up"
"down" = "move_down"
"C-p" = "move_up"
"C-n" = "move_down"
"A-j" = "half_page_down"
"A-k" = "half_page_up"
"pageup" = "page_up"
"pagedown" = "page_down"
"C-v" = "page_down"
"A-v" = "page_up"
"A-g" = "move_top"
"A-G" = "move_bottom"

[keys.repo_select]
"enter" = "open_repo"
"tab" = "enter_repo"

[keys.branch_select]
"enter" = "open_branch"
"esc" = "go_back"
"C-o" = "new_branch"
"C-x" = "delete_worktree"

[keys.sessions_select]
"enter" = "switch_to_session"
"tab" = "jump_to_next_agent_session"
"S-tab" = "jump_to_previous_agent_session"

[keys.modal]
"enter" = "confirm"
"esc" = "cancel"
"tab" = "tab_complete"

```

### `[agent]` section

Agent detection configuration.

#### `enabled`

Whether agent status detection is enabled.
Set to `false` to completely disable agent polling and status display.

Default: `true`

#### `poll_interval_ms`

Interval in milliseconds between agent status polls.

Default: `500`

### `[agent.labels]` section

Label text for each agent state shown in the branch picker.

#### `running`

Default: `"[RUNNING]"`

#### `waiting`

Default: `"[WAITING]"`

#### `idle`

Default: `"[IDLE]"`

#### `unknown`

Default: `"[UNKNOWN]"`

<!-- CONFIG END -->
