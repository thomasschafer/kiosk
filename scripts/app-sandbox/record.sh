#!/usr/bin/env bash
# Records the README's preview of the kiosk mod in the app sandbox: frames the app renders
# itself, taken while a script types, then turned into media/preview.gif. Run through
# `run.sh record`; look at the result before committing it.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
sb() { "$here/sandbox" "$@"; }
out=${RECORD_OUT:-/src/media/preview.gif}
still=${RECORD_STILL:-/src/preview.png}
fps=12
frames=$(mktemp -d)
# A frame takes about as long as a keystroke at 2x, so every letter typed shows.
export TYPE_DELAY=160

sb down >/dev/null 2>&1 || true
sb fixtures
# At 2x, as a high-density screen draws it, so the preview stays sharp when scaled down.
SANDBOX_THEME=${SANDBOX_THEME:-catppuccin-macchiato} SANDBOX_SCALE=${SANDBOX_SCALE:-2} sb up
sb install >/dev/null
sb "do" space.new "{\"folder\":\"$HOME/code/api\"}" >/dev/null
sleep 2
sb move 1275 815

# Takes a frame every 1/fps s, or as often as the app can save one if that's slower, and
# writes the rate it kept to $frames/rate for the GIF to play at.
capture() {
  local n=0 next start now
  start=$(date +%s%N)
  next=$start
  while [[ ! -f $frames/stop ]]; do
    sb shot "$(printf '%s/%05d.png' "$frames" "$n")" >/dev/null
    n=$((n + 1))
    next=$((next + 1000000000 / fps))
    now=$(date +%s%N)
    if ((next > now)); then
      sleep "$(printf '0.%09d' $((next - now)))"
    else
      next=$now
    fi
  done
  now=$(date +%s%N)
  echo "$n $(((now - start) / 1000000))" | awk '{ printf "%.3f", $1 * 1000 / $2 }' >"$frames/rate"
}
capture &
capturing=$!

pause() { sleep "$1"; }
pause 1
sb open
pause 1.4
sb type web
pause 0.9
sb key Tab
pause 1.4
sb shot "$still" >/dev/null
sb type dark
pause 1.1
sb key Return
pause 2.2
sb open
pause 1.2
sb type api
pause 0.7
sb key Tab
pause 1.2
sb type feat/login
pause 1.3
sb key Return
pause 1.6
sb key Return
pause 2.4

touch "$frames/stop"
wait "$capturing"
scale="scale=1000:-2:flags=lanczos"
rate=$(cat "$frames/rate")
ffmpeg -hide_banner -loglevel error -y -framerate "$rate" -i "$frames/%05d.png" \
  -vf "$scale,palettegen=max_colors=192:stats_mode=full" "$frames/palette.png"
ffmpeg -hide_banner -loglevel error -y -framerate "$rate" -i "$frames/%05d.png" -i "$frames/palette.png" \
  -lavfi "${scale}[frames];[frames][1:v]paletteuse=dither=sierra2_4a" -loop 0 "$out"
[[ -n ${RECORD_KEEP:-} ]] && cp -r "$frames" "$RECORD_KEEP"
rm -rf "$frames"
echo "$out ($(du -h "$out" | cut -f1), $rate fps), $still"
