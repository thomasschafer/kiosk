#!/usr/bin/env bash
#
# Renders a terminal program's screen to a PNG for a README preview.
#
# Runs the command in a throwaway tmux server, follows the steps in order (wait for
# text to appear, send a key), waits for the screen to settle, then draws the screen
# with Catppuccin Macchiato colours in JetBrainsMono Nerd Font. Use it through the
# flake, which provides every tool it needs and the font:
#
#   nix run github:thomasschafer/kiosk#preview-render -- \
#     --cols 108 --rows 30 --output media/preview.png [--cwd DIR] \
#     [--wait TEXT | --send KEY]... -- COMMAND [ARG]...
#
# KEY is a tmux key name, such as Tab, Down or C-c. The command runs without TMUX set,
# so it can be pointed at another tmux server.

set -euo pipefail

: "${KIOSK_PREVIEW_FONT_DIR:?must name the directory holding JetBrainsMono Nerd Font; run through the flake}"

readonly USAGE="usage: preview-render --cols N --rows N --output FILE.png [--cwd DIR] [--wait TEXT | --send KEY]... -- COMMAND [ARG]..."
readonly FONT_FAMILY="JetBrainsMono Nerd Font Mono"
readonly FONT_SIZE=45
readonly LINE_HEIGHT=1.3
# Background, foreground, then ANSI colours 0 to 15.
readonly THEME=24273a,cad3f5,494d64,ed8796,a6da95,eed49f,8aadf4,f5bde6,8bd5ca,b8c0e0,5b6078,ed8796,a6da95,eed49f,8aadf4,f5bde6,8bd5ca,a5adcb
readonly TIMEOUT_TENTHS=300

fail() {
    printf 'preview-render: %s\n' "$1" >&2
    exit 1
}

cols=""
rows=""
output=""
cwd=$PWD
steps=()
while [ $# -gt 0 ]; do
    case $1 in
        --cols | --rows | --output | --cwd | --wait | --send)
            [ $# -ge 2 ] || fail "$1 needs a value"$'\n'"$USAGE"
            case $1 in
                --cols) cols=$2 ;;
                --rows) rows=$2 ;;
                --output) output=$2 ;;
                --cwd) cwd=$2 ;;
                --wait) steps+=("wait" "$2") ;;
                --send) steps+=("send" "$2") ;;
            esac
            shift 2
            ;;
        --)
            shift
            break
            ;;
        *) fail "unexpected argument '$1'"$'\n'"$USAGE" ;;
    esac
done
[ $# -gt 0 ] || fail "no command given"$'\n'"$USAGE"
[[ $cols =~ ^[1-9][0-9]*$ && $rows =~ ^[1-9][0-9]*$ ]] || fail "--cols and --rows must be positive integers"$'\n'"$USAGE"
[[ $output == *.png ]] || fail "--output must name a .png file"$'\n'"$USAGE"
[ -d "$cwd" ] || fail "--cwd is not a directory: $cwd"
[ -d "$(dirname "$output")" ] || fail "the directory for --output does not exist: $output"

work=$(mktemp -d "${TMPDIR:-/tmp}/kiosk-preview-render.XXXXXX")
socket="kiosk-test-preview-render-$$"
t() {
    tmux -L "$socket" -f "$work/tmux.conf" "$@"
}
cleanup() {
    t kill-server 2>/dev/null || true
    rm -rf -- "$work"
}
trap cleanup EXIT

cat >"$work/tmux.conf" <<'EOF'
set -g status off
set -g remain-on-exit on
set -g default-terminal tmux-256color
EOF

screen() {
    t capture-pane -p -t preview "$@"
}

fail_with_screen() {
    printf 'preview-render: %s; the screen was:\n' "$1" >&2
    screen >&2 || true
    exit 1
}

check_running() {
    if [ "$(t display -p -t preview '#{pane_dead}')" = 1 ]; then
        fail_with_screen "the command exited"
    fi
}

wait_for() {
    local tenths
    for ((tenths = 0; tenths < TIMEOUT_TENTHS; tenths++)); do
        check_running
        if screen | grep -Fq -- "$1"; then
            return 0
        fi
        sleep 0.1
    done
    fail_with_screen "timed out waiting for '$1'"
}

wait_until_settled() {
    local previous current tenths
    previous=$(screen -e)
    for ((tenths = 0; tenths < TIMEOUT_TENTHS; tenths += 5)); do
        sleep 0.5
        check_running
        current=$(screen -e)
        if [ "$current" = "$previous" ]; then
            return 0
        fi
        previous=$current
    done
    fail_with_screen "the screen kept changing"
}

t new-session -d -s preview -x "$cols" -y "$rows" -c "$cwd" env -u TMUX "$@"
for ((index = 0; index < ${#steps[@]}; index += 2)); do
    case ${steps[index]} in
        wait) wait_for "${steps[index + 1]}" ;;
        send) t send-keys -t preview "${steps[index + 1]}" ;;
    esac
done
wait_until_settled

screen -e >"$work/screen.ansi"
captured_rows=$(wc -l <"$work/screen.ansi")
[ "$captured_rows" -eq "$rows" ] || fail "captured $captured_rows rows, expected $rows"

# A one-frame asciicast: each captured row drawn at its own line, with the cursor hidden.
jq -Rsc --argjson cols "$cols" --argjson rows "$rows" '
    {version: 2, width: $cols, height: $rows},
    [0, "o", ((rtrimstr("\n") | split("\n") | to_entries
        | map("\u001b[\(.key + 1);1H\(.value)") | join("")) + "\u001b[?25l")]
' "$work/screen.ansi" >"$work/screen.cast"

agg --quiet \
    --font-dir "$KIOSK_PREVIEW_FONT_DIR" \
    --font-family "$FONT_FAMILY" \
    --font-size "$FONT_SIZE" \
    --line-height "$LINE_HEIGHT" \
    --theme "$THEME" \
    "$work/screen.cast" "$work/screen.gif"
ffmpeg -loglevel error -y -i "$work/screen.gif" -frames:v 1 -map_metadata -1 "$work/screen.png"
mv -- "$work/screen.png" "$output"
printf 'Wrote %s\n' "$output"
