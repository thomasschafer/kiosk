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

check "the picker lists the repositories"
open_kiosk
wait_view '.main.state == "list" and (.main.total == 23)'
[[ $(last_view | jq -r "$rows | .[0:3] | join(\",\")") == "api,cli,design-system" ]] || fail "unexpected order"
sb shot "$shots/repos.png" >/dev/null

check "typing filters, alt+backspace (a picker chord) clears the word"
sb type infr
wait_view ".main.query.before == \"infr\" and ($rows == [\"infra\"])"
sb key alt+BackSpace
wait_view '.main.query.before == "" and .main.total == 23'

check "the wheel scrolls the list and the selection stays on screen"
sb move 600 400
sb wheel down 6
wait_view '.main.offset > 0 and .main.selected != null'

check "help opens over the picker and Esc closes only the help"
sb key ctrl+h
wait_view '.overlay.kind == "help"'
sb shot "$shots/help.png" >/dev/null
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
