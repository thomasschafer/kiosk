# shellcheck shell=bash
# What e2e.sh and shots.sh share: driving the sandbox and waiting on what kiosk shows.
# Sourced, not run.

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
sb() { "$here/sandbox" "$@"; }
trace=$HOME/sb/trace.jsonl

# Says what went wrong, with the last view kiosk sent, and stops. A script may define
# `on_fail` to save a screenshot first.
fail() {
  declare -F on_fail >/dev/null && on_fail
  echo "FAIL: $*" >&2
  echo "last view: $(last_view | head -c 2000)" >&2
  exit 1
}

# The latest view kiosk sent. A tab's body is lifted to the top, its tab as its `screen` and
# the tab bar beside it as `bar`, so checks read the picker's view the same in any tab.
last_view() {
  grep '^< {"t":"view"' "$trace" 2>/dev/null | tail -1 | cut -c3- |
    jq -c '.view | if .screen == "tabs" then .body + {screen: .body.tab, bar} else . end'
}

# Waits until the latest view satisfies the jq filter `$1`.
wait_view() {
  for _ in $(seq 100); do
    if [[ $(last_view | jq -r "$1" 2>/dev/null) == true ]]; then return; fi
    sleep 0.1
  done
  fail "the view never satisfied $1"
}

open_overlays() { sb status | jq -r '.overlays | join(" ")'; }

wait_closed() {
  for _ in $(seq 100); do
    [[ " $(open_overlays) " != *" kiosk "* ]] && return
    sleep 0.1
  done
  fail "the popup stayed open"
}

# The mod takes focus again 600 ms after it opens, as the app can take it back while the
# popup appears; a key pressed before then may go to the app instead.
open_kiosk() {
  sb open
  wait_view '.screen != null'
  sleep 0.7
}

# The names of the rows in view, for the scripts that source this.
# shellcheck disable=SC2034
rows='[.main.rows[].name.text]'
space_folders() { sb ls | jq -r '.spaces[].folder'; }
