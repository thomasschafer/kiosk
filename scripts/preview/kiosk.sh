#!/usr/bin/env bash
#
# Regenerates media/preview.png: the branch picker for the fictional repositories from
# fixture.sh, with open sessions whose agents report their status over OSC 7501.
# Everything runs in a temporary directory and throwaway tmux servers.
#
# Usage, from anywhere in the repository:
#   nix develop -c scripts/preview/kiosk.sh

set -euo pipefail

REPO_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
readonly REPO_DIR
readonly OUTPUT="$REPO_DIR/media/preview.png"

for tool in cargo nix tmux; do
    command -v "$tool" >/dev/null || {
        printf '%s is not on PATH; run this in the dev shell: nix develop -c %s\n' "$tool" "$0" >&2
        exit 1
    }
done

TEMP=$(mktemp -d "${TMPDIR:-/tmp}/kiosk-preview.XXXXXX")
TEMP=$(cd "$TEMP" && pwd -P)
readonly TEMP ROOT="$TEMP/fixture"
readonly SOCKET="kiosk-test-preview-$$"
fixture_tmux() {
    tmux -L "$SOCKET" -f /dev/null "$@"
}
cleanup() {
    fixture_tmux kill-server 2>/dev/null || true
    rm -rf -- "$TEMP"
}
trap cleanup EXIT

# A shell loop standing in for an agent, reporting its state over OSC 7501 until the
# server is killed. It repeats the report so kiosk's collector, which starts with the
# picker, sees it.
agent_session() {
    local name=$1 folder=$2 app=$3 state=$4
    fixture_tmux new-session -d -s "$name" -c "$folder" \
        "while :; do printf '\\033]7501;state=$state:app=$app\\007'; sleep 0.3; done"
}

printf 'Building kiosk...\n'
cargo build --quiet --release -p kiosk --manifest-path "$REPO_DIR/Cargo.toml"

nix run "$REPO_DIR#preview-fixture" -- "$ROOT"
readonly TIDEPOOL="$ROOT/repos/tidepool" WORKTREES="$ROOT/worktrees/tidepool"

mkdir "$TEMP/config" "$TEMP/state"
cat >"$TEMP/config/config.toml" <<EOF
search_dirs = ["$ROOT/repos"]

[worktrees]
directory = "$ROOT/worktrees"
EOF

agent_session tidepool "$TIDEPOOL" claude-code idle
agent_session tidepool-offline-sync "$WORKTREES/feat-offline-sync" claude-code working
agent_session tidepool-timezone-rounding "$WORKTREES/fix-timezone-rounding" codex blocked:kind=permission
agent_session tidepool-csv-export "$WORKTREES/feat-csv-export" claude-code "done"

printf 'Rendering the picker...\n'
nix run "$REPO_DIR#preview-render" -- \
    --cols 108 --rows 30 --output "$OUTPUT" --cwd "$TIDEPOOL" \
    --wait "select repo" --send Tab --wait "select branch" \
    --wait origin/refactor/sync-queue --wait running --wait waiting --wait "done" --wait idle \
    --send Down --send Down \
    -- env \
    KIOSK_TMUX_SOCKET="$SOCKET" \
    KIOSK_CONFIG_DIR="$TEMP/config" \
    KIOSK_STATE_DIR="$TEMP/state" \
    "$REPO_DIR/target/release/kiosk"
