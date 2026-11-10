#!/usr/bin/env bash
# The kiosk mod end to end in the app sandbox: real key presses and clicks on the released
# Herdr app, checked against what kiosk showed (its message trace) and what the app did
# (`herdr-app ls`, the checkouts on disk). Run through `run.sh e2e`.
# lib.sh, sourced below and not followed without -x, defines `rows` and the like.
# shellcheck disable=SC1091,SC2154
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
source "$here/lib.sh"
shots=${E2E_SHOTS:-/src/target/app-e2e}
mkdir -p "$shots"

step=0
on_fail() {
  sb shot "$shots/failure.png" >/dev/null 2>&1 || true
  echo "at check $step" >&2
}
check() {
  step=$((step + 1))
  echo "- $1"
}

sb down >/dev/null 2>&1 || true
EXTRA_REPOS=16 sb fixtures
sb up
sb install >/dev/null

check "a picker the popup closed before reaching gives up and leaves no log"
# The picker is held back from starting until the popup has closed, so it cannot connect.
# The app reloads the mod when its folder changes, so each change is given time to settle.
bin=$HOME/sb/home/mods/kiosk/bin
release=/tmp/kiosk-e2e-release
rm -f "$release"
cp "$bin/herdr-app-kiosk" "$bin/held-herdr-app-kiosk"
printf '#!/bin/sh\nuntil [ -e %s ]; do sleep 0.05; done\nexec "%s" "$@"\n' "$release" "$bin/held-herdr-app-kiosk" >"$bin/.held.new"
chmod +x "$bin/.held.new"
mv -f "$bin/.held.new" "$bin/herdr-app-kiosk"
sleep 2
rm -f /tmp/herdr-app-kiosk-serve-*.log
sb open
for _ in $(seq 100); do
  [[ " $(open_overlays) " == *" kiosk "* ]] && break
  sleep 0.05
done
sb "do" overlay.close >/dev/null
wait_closed
touch "$release"
for _ in $(seq 50); do
  pgrep -f "^$bin/held-herdr-app-kiosk serve" >/dev/null && break
  sleep 0.1
done
pgrep -f "^$bin/held-herdr-app-kiosk serve" >/dev/null || fail "the held picker never started"
# It waits 15 s for a connection.
for _ in $(seq 200); do
  pgrep -f "^$bin/held-herdr-app-kiosk serve" >/dev/null || break
  sleep 0.1
done
if pgrep -f "^$bin/held-herdr-app-kiosk serve" >/dev/null; then fail "the picker kept waiting"; fi
if compgen -G '/tmp/herdr-app-kiosk-serve-*.log' >/dev/null; then
  fail "the picker left its log: $(cat /tmp/herdr-app-kiosk-serve-*.log)"
fi
[[ -z $(sb trace) ]] || fail "the picker was reached after all"
rm -f "$release"
mv -f "$bin/held-herdr-app-kiosk" "$bin/herdr-app-kiosk"
sleep 2

check "the picker lists the repositories"
open_kiosk
wait_view '.main.state == "list" and (.main.total == 23)'
[[ $(last_view | jq -r "$rows | .[0:3] | join(\",\")") == "api,cli,design-system" ]] || fail "unexpected order"
sb shot "$shots/repos.png" >/dev/null

check "without experimental_tabs there is no tab bar, and the tab keys are left to the app"
wait_view '.bar == []'
hello=$(grep '^< {"t":"hello"' "$trace" | tail -1 | cut -c3-)
[[ $(jq '[.chords[] | select(. == "ctrl-2" or . == "ctrl-tab")] | length' <<<"$hello") == 0 ]] ||
  fail "kiosk claimed tab keys: $hello"

check "with experimental_tabs, tab keys and the tab bar move between tabs, and the picker keeps its search"
config=$HOME/.config/herdr-app-kiosk/config.toml
sb key Escape
wait_closed
printf 'search_dirs = ["~/code"]\nexperimental_tabs = true\n' >"$config"
open_kiosk
wait_view '.main.state == "list" and (.bar | length) == 2'
sb shot "$shots/tabs.png" >/dev/null
sb type infr
wait_view ".main.query.before == \"infr\""
sb key ctrl+2
wait_view '.screen == "diff" and ([.bar[] | select(.active) | .id] == ["diff"])'
sb shot "$shots/diff-tab.png" >/dev/null
sb key ctrl+Tab
wait_view '.screen == "picker" and .main.query.before == "infr"'
sb key ctrl+Next
wait_view '.screen == "diff"'
sb click 580 105
wait_view '.screen == "picker"'
sb key ctrl+u
wait_view '.main.query.before == ""'
sb key Escape
wait_closed

check "a launcher opens kiosk on its tab, and closes kiosk when that tab already shows"
launch_diff() { sb "do" mod.open '{"name":"kiosk-diff"}' >/dev/null; }
# The launcher closes once kiosk acknowledges it, just after kiosk shows the tab.
# `ignore` names an overlay that may also be open, such as the app's toast.
only_kiosk_open() {
  local ignore=${1:-}
  for _ in $(seq 50); do
    [[ $(open_overlays | tr ' ' '\n' | grep -vx "${ignore:-^$}" | xargs) == kiosk ]] && return
    sleep 0.05
  done
  fail "the launcher left overlays: $(open_overlays)"
}
: >"$trace"
launch_diff
wait_view '.screen == "diff"'
only_kiosk_open
sleep 0.7
launch_diff
wait_closed

check "a launcher switches an open kiosk to its tab and gives the keyboard back"
open_kiosk
wait_view '.screen == "picker"'
launch_diff
wait_view '.screen == "diff"'
only_kiosk_open
sb key ctrl+1
wait_view '.screen == "picker"'

check "a launcher still opens kiosk after a crash left it marked open"
pkill -9 -x herdr-app
for _ in $(seq 100); do pgrep -x herdr-app >/dev/null || break; sleep 0.05; done
! pgrep -x herdr-app >/dev/null || fail "the app outlived kill -9"
pkill -x herdr-app-kiosk || true
jq -e '."kiosk.open" != null' "$HOME/sb/home/shared.json" >/dev/null || fail "the crash left kiosk unmarked, so this tests nothing"
sb up >/dev/null
: >"$trace"
launch_diff
wait_view '.screen == "diff"'
# The launcher's request went nowhere before it opened kiosk, and the app says so in a toast.
only_kiosk_open toast
sb key Escape
wait_closed
printf 'search_dirs = ["~/code"]\n' >"$config"
open_kiosk
wait_view '.main.state == "list" and .main.total == 23'

check "typing filters, alt+backspace (a picker chord) clears the word"
sb type infr
wait_view ".main.query.before == \"infr\" and ($rows == [\"infra\"])"
sb key alt+BackSpace
wait_view '.main.query.before == "" and .main.total == 23'

check "space is typed into the search rather than closing the popup"
sb type 'service 0'
wait_view '.main.query.before == "service 0"'
sleep 0.5
[[ " $(open_overlays) " == *" kiosk "* ]] || fail "space closed the popup"
sb key ctrl+u
wait_view '.main.query.before == ""'

check "the wheel scrolls the list and the selection stays on screen"
sb move 600 400
sb wheel down 6
wait_view '.main.offset > 0 and .main.selected != null'

check "help opens over the picker and Esc closes only the help"
sb key ctrl+h
wait_view '.overlay.kind == "help"'
sb shot "$shots/help.png" >/dev/null

check "down in help moves to another command each time, and help keeps its length"
selected='.overlay.entries[.overlay.selected].command'
length=$(last_view | jq '.overlay.entries | length')
for _ in $(seq 10); do
  command=$(last_view | jq -r "$selected")
  sb key Down
  wait_view "$selected != \"$command\" and (.overlay.entries | length) == $length"
done
sb shot "$shots/help-scrolled.png" >/dev/null
sb key Escape
wait_view '.overlay == null'
[[ " $(open_overlays) " == *" kiosk "* ]] || fail "Esc closed the popup"

check "tab browses a repository's branches, the back button returns, ctrl+u clears the search"
sb type api
wait_view "$rows == [\"api\"]"
sb key Tab
wait_view '.main.kind == "branches" and .main.title == "api — select branch"'
wait_view '[.main.rows[] | select(.name.text == "feat/auth") | .actions.delete] == [true]'
wait_view '[.main.rows[] | select(.name.text == "main") | .actions.delete] == [false]'
sb shot "$shots/branches.png" >/dev/null
sb click 355 108
wait_view '.main.kind == "repos" and .main.query.before == "api"'
sb key ctrl+u
wait_view '.main.query.before == ""'

check "a new branch gets a checkout and a space"
sb type api
wait_view "$rows == [\"api\"]"
sb key Tab
wait_view '.main.kind == "branches"'
sb type feat/e2e
wait_view '.main.rows == [] and (.main.empty | test("creates feat/e2e"))'
sb key Return
wait_view '.dialog.kind == "base_branch"'
sb shot "$shots/new-branch.png" >/dev/null

check "a click beside a dialog, on the dimmed list, only closes the dialog"
sb click 200 400
wait_view '.dialog == null and .main.kind == "branches"'
[[ ! -d $HOME/.herdr/worktrees/api/feat-e2e ]] || fail "the click behind the dialog created the branch"
sb key Return
wait_view '.dialog.kind == "base_branch"'
sb key Return
wait_closed
[[ -d $HOME/.herdr/worktrees/api/feat-e2e ]] || fail "no checkout for feat/e2e"
space_folders | grep -qx "$HOME/.herdr/worktrees/api/feat-e2e" || fail "no space for feat/e2e"

check "the new space shows as open, and removing its checkout asks first"
open_kiosk
wait_view '[.main.rows[] | select(.name.text == "api") | .open] == [true]'
sb shot "$shots/open.png" >/dev/null
sb type api
wait_view "$rows == [\"api\"]"
sb key Tab
sb type e2e
wait_view "$rows == [\"feat/e2e\"]"
sb key ctrl+x
wait_view '.dialog.kind == "confirm_delete" and (.dialog.lines[0] | test("feat-e2e"))'
sb shot "$shots/remove.png" >/dev/null
sb key Return
wait_view '.dialog == null'
for _ in $(seq 50); do [[ -d $HOME/.herdr/worktrees/api/feat-e2e ]] || break; sleep 0.1; done
[[ ! -d $HOME/.herdr/worktrees/api/feat-e2e ]] || fail "the checkout is still there"

check "Esc goes straight back with a search typed, and a branch only the remote has opens as a tracking checkout"
sb key Escape
wait_view '.main.kind == "repos" and .main.query.before == "api"'
sb key ctrl+u
wait_view '.main.query.before == ""'
sb type web
wait_view "$rows == [\"web\"]"
sb key Tab
wait_view '[.main.rows[] | select(.name.text == "feat/from-review") | .badges] == [[{"kind": "remote", "name": "origin"}]]'
sb type from-review
wait_view "$rows == [\"feat/from-review\"]"
sb key Return
wait_closed
git -C "$HOME/code/web" rev-parse --verify -q feat/from-review >/dev/null || fail "no local tracking branch"

check "clicking a row opens that repository, with its [on_open] pane beside it"
marker=$HOME/sb/on-open
rm -f "$marker"
printf 'search_dirs = ["~/code"]\n\n[on_open]\npanes = [{ command = "pwd > %s", direction = "right" }]\n' \
  "$marker" >"$HOME/.config/herdr-app-kiosk/config.toml"
open_kiosk
wait_view '.main.total == 23'
sb type cli
wait_view "$rows == [\"cli\"]"
sb click 500 161
wait_closed
space_folders | grep -qx "$HOME/code/cli" || fail "no space for cli"
for _ in $(seq 100); do [[ -s $marker ]] && break; sleep 0.1; done
[[ $(cat "$marker" 2>/dev/null) == "$HOME/code/cli" ]] || fail "the [on_open] command did not run in the checkout"
terminals=$(sb ls | jq --arg folder "$HOME/code/cli" '[.spaces[] | select(.folder == $folder) | .tabs[].terminals[]] | length')
[[ $terminals == 2 ]] || fail "the cli space has $terminals terminals, not the shell and its [on_open] pane"
sb shot "$shots/on-open.png" >/dev/null
printf 'search_dirs = ["~/code"]\n' >"$HOME/.config/herdr-app-kiosk/config.toml"

check "Esc quits with a search typed, and the picker exits with the popup"
open_kiosk
wait_view '.main.state == "list"'
sb type qu
wait_view '.main.query.before == "qu"'
sb key Escape
wait_closed
for _ in $(seq 50); do pgrep -x herdr-app-kiosk >/dev/null || break; sleep 0.1; done
! pgrep -x herdr-app-kiosk >/dev/null || fail "a picker is still running"

check "closing the popup from outside ends the picker too"
open_kiosk
wait_view '.main.state == "list"'
sb "do" overlay.close >/dev/null
wait_closed
for _ in $(seq 50); do pgrep -x herdr-app-kiosk >/dev/null || break; sleep 0.1; done
! pgrep -x herdr-app-kiosk >/dev/null || fail "a picker outlived its popup"

check "a click below the panel, in the popup's empty part, closes it like one beside it"
open_kiosk
wait_view '.main.state == "list"'
sb type docs
wait_view "$rows == [\"docs\"]"
sb click 640 650
wait_closed
for _ in $(seq 50); do pgrep -x herdr-app-kiosk >/dev/null || break; sleep 0.1; done
! pgrep -x herdr-app-kiosk >/dev/null || fail "a picker outlived its popup"

check "kiosk opens with the repository it is in centred, then scrolls as usual"
# service-08 is the 14th of 23 repositories, with 10 rows in view: offset 8 centres it,
# where scrolling it just into view would leave it on the bottom row (offset 4).
# The app tells mods a terminal's folder once its shell reports it (OSC 7), which the
# sandbox's sh doesn't do by itself, so the new space's shell reports it here.
# The marker is printed as osc7-2 but typed as osc7-$((1 + 1)), so only the output matches.
folder=$HOME/code/service-08
sb "do" space.new "$(jq -cn --arg folder "$folder" '{folder: $folder}')" >/dev/null
terminal=
for _ in $(seq 100); do
  terminal=$(sb hc ls | jq -r --arg folder "$folder" '[.terminals[] | select(.cwd == $folder) | .address] | last // empty')
  [[ -n $terminal ]] && break
  sleep 0.1
done
[[ -n $terminal ]] || fail "no terminal in $folder"
# shellcheck disable=SC2016
sb hc send "$terminal" 'printf "\033]7;file://%s%s\033\\" "$(hostname)" "$PWD"; echo "osc7-$((1 + 1))"' >/dev/null
reported=false
for _ in $(seq 100); do
  if sb hc read "$terminal" | grep -qx 'osc7-2'; then reported=true; break; fi
  sleep 0.1
done
$reported || fail "the shell in $folder never reported its folder"
open_kiosk
wait_view '.main.total == 23 and .main.offset == 8 and .main.rows[.main.selected].name.text == "service-08"'
sb shot "$shots/centred.png" >/dev/null
sb key Down
wait_view '.main.rows[.main.selected].name.text == "service-09" and .main.offset == 8'
sb key Escape
wait_closed

check "the diff tab shows the focused checkout's changes, highlighted, and moves through them"
# The space in service-08 is still focused. Its fixture has one empty commit on main, so the
# diff is what's uncommitted: a modified file, a new one and an untracked one.
config=$HOME/.config/herdr-app-kiosk/config.toml
printf 'search_dirs = ["~/code"]\nexperimental_tabs = true\n' >"$config"
printf 'fn main() {\n    // a comment\n    let s = "text";\n}\n' >"$folder/main.rs"
git -C "$folder" add main.rs
git -C "$folder" commit -q -m "add main"
printf 'fn main() {\n    // a comment\n    let s = "changed";\n}\n' >"$folder/main.rs"
printf 'untracked\n' >"$folder/notes.txt"
: >"$trace"
sb "do" mod.open '{"name":"kiosk-diff"}' >/dev/null
wait_view '.screen == "diff" and (.body.rows | length) > 0'
[[ $(last_view | jq -c '[.files[].path]') == '["main.rs","notes.txt"]' ]] || fail "unexpected files"
[[ $(last_view | jq -r '.ranges[0].label') == "All changes" ]] || fail "the first range isn't all changes"
# The changed line is highlighted: its string is in the string tone.
string=$(last_view | jq '.tones | index("string")')
last_view | jq -e --argjson string "$string" \
  '[.body.rows[] | select(.[0] == "line" and .[1] == "+") | .[5] | select(. != null) | [range(1; length; 2) as $i | .[$i]] | index($string)] | any(. != null)' \
  >/dev/null || fail "the added line isn't highlighted"
sleep 0.7
sb shot "$shots/diff.png" >/dev/null
sb key n
wait_view '.current_file == 1'
sb key bracketright
wait_view '.range == 1 and .ranges[1].label == "Uncommitted"'
sb key Escape
wait_closed
printf 'search_dirs = ["~/code"]\n' >"$config"

check "without a config, the setup wizard writes one and the picker starts"
rm "$HOME/.config/herdr-app-kiosk/config.toml"
open_kiosk
wait_view '.screen == "setup" and .step == "welcome"'
sb shot "$shots/setup.png" >/dev/null
sb key Return
wait_view '.step == "directories"'
# The wizard is given the path as a person types it.
# shellcheck disable=SC2088
sb type '~/code'
wait_view '.path.query.before == "~/code"'
sb shot "$shots/setup-directories.png" >/dev/null
sb key Return
wait_view '.depth.query.before == "1"'
sb key Return
wait_view '.step == "folder_mode"'
sb key Down
wait_view '.options[1].selected == true'
sb shot "$shots/setup-folders.png" >/dev/null
sb key Return
wait_view '.step == "directories" and (.added | length) == 1'
sb key Return
wait_view '.step == "confirm"'
sb shot "$shots/setup-confirm.png" >/dev/null
sb key Return
wait_view '.screen == "picker" and .main.state == "list" and .main.total > 0'
grep -q 'include_non_git = true' "$HOME/.config/herdr-app-kiosk/config.toml" || fail "the config does not include plain folders"
sb key ctrl+c
wait_closed

echo "all $step checks passed; screenshots in $shots"
