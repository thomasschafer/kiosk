#!/usr/bin/env bash
# Screenshots of every screen kiosk draws, the same ones each run, for comparing how two
# versions of the mod look. Run through `run.sh shots`; they land in SHOTS_OUT (by default
# target/shots/<theme>). SANDBOX_THEME, SANDBOX_APPEARANCE and SANDBOX_SCALE pick the look
# (by default catppuccin-macchiato at 2x), and KIOSK_MOD another mod folder to draw them.
# lib.sh, sourced below and not followed without -x, defines `rows` and the like.
# shellcheck disable=SC1091,SC2154
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
source "$here/lib.sh"
export SANDBOX_THEME=${SANDBOX_THEME:-catppuccin-macchiato} SANDBOX_SCALE=${SANDBOX_SCALE:-2}
out=${SHOTS_OUT:-/src/target/shots/$SANDBOX_THEME}
rm -rf "$out"
mkdir -p "$out"
on_fail() { sb shot "$out/failure.png" >/dev/null 2>&1 || true; }
config=$HOME/.config/herdr-app-kiosk/config.toml

# Waits for animations to settle, with the pointer clear of the popup, then saves `$1`.
shot() {
  sb move 1275 815
  sleep 0.6
  sb shot "$out/$1.png" >/dev/null
  echo "- $1"
}

fresh() {
  if [[ " $(open_overlays) " == *" kiosk "* ]]; then
    sb "do" overlay.close >/dev/null
    wait_closed
  fi
  open_kiosk
}

sb down >/dev/null 2>&1 || true
EXTRA_REPOS=16 sb fixtures
sb up
sb install >/dev/null
sb "do" space.new "{\"folder\":\"$HOME/code/api\"}" >/dev/null
sleep 1

fresh
wait_view '.main.state == "list" and .main.total == 23 and ([.main.rows[] | select(.name.text == "api") | .open] == [true])'
shot 01-repos

sb type de
wait_view '.main.query.before == "de" and .main.total < 23'
shot 02-repos-filtered

sb key ctrl+u
sb type zzz
wait_view '.main.query.before == "zzz" and .main.rows == []'
shot 03-repos-no-match

fresh
sb key ctrl+h
wait_view '.overlay.kind == "help"'
shot 04-help

fresh
sb type api
wait_view "$rows == [\"api\"]"
sb key Tab
wait_view '.main.kind == "branches" and (.main.rows | length) > 3'
shot 05-branches

sb type feat/new-thing
wait_view '.main.rows == [] and (.main.empty | test("creates"))'
shot 06-branches-new-name

sb key Return
wait_view '.dialog.kind == "base_branch"'
shot 07-new-branch

sb key Escape
wait_view '.dialog == null'
sb key ctrl+u
sb type auth
wait_view "$rows == [\"feat/auth\"]"
sb key ctrl+x
wait_view '.dialog.kind == "confirm_delete"'
shot 08-remove

fresh
sb type web
wait_view "$rows == [\"web\"]"
sb key Tab
wait_view '[.main.rows[] | select(.name.text == "feat/from-review") | .badges[0].kind] == ["remote"]'
shot 09-branches-remote

# Folders the scan can't read, which it warns of without stopping.
locked=("$HOME/code/vendor/archive" "$HOME/code/vendor/mirror" "$HOME/code/vendor/old-clients")
mkdir -p "${locked[@]}"
chmod 000 "${locked[@]}"
printf 'search_dirs = [{ path = "~/code", depth = 3 }]\n' >"$config"
fresh
wait_view '.main.state == "list" and (.toast.message // "" | test("could not be scanned"))'
shot 10-scan-toast

sb key ctrl+s
wait_view '.overlay.kind == "scan_warnings"'
shot 11-scan-warnings

chmod 755 "${locked[@]}"
rm -rf "$HOME/code/vendor" "$config"
fresh
wait_view '.screen == "setup" and .step == "welcome"'
shot 12-setup-welcome

sb key Return
wait_view '.step == "directories"'
# shellcheck disable=SC2088
sb type '~/co'
wait_view '.path.query.before == "~/co" and (.completions | length) > 0'
shot 13-setup-directories

sb key ctrl+u
# shellcheck disable=SC2088
sb type '~/code'
wait_view '.path.query.before == "~/code"'
sb key Return
wait_view '.depth.query.before == "1"'
sb key Return
wait_view '.step == "folder_mode"'
shot 14-setup-folders

sb key Return
wait_view '.step == "directories" and (.added | length) == 1'
shot 15-setup-added

sb key Return
wait_view '.step == "confirm"'
shot 16-setup-confirm

sb "do" overlay.close >/dev/null
wait_closed
echo "screenshots in $out"
