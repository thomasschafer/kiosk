#!/usr/bin/env bash
# Runs the app sandbox on this machine through Docker: `run.sh up`, then any `sandbox`
# command (`run.sh open`, `run.sh key ctrl+h`, ...), `run.sh e2e`, `run.sh shots`, `run.sh record`, and
# `run.sh rm` to delete the container. See README.md beside this file.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
name=${KIOSK_SANDBOX:-herdr-app-kiosk-sandbox}
image=herdr-app-kiosk-sandbox

ensure() {
  docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" "$here"
  if [[ -z $(docker ps -q -f "name=^$name$") ]]; then
    docker rm -f "$name" >/dev/null 2>&1 || true
    # KIOSK_SANDBOX_MOUNTS: extra `-v` arguments, such as a local kiosk checkout.
    # shellcheck disable=SC2086
    # The app's update checks resolve nowhere, so a newer release's Restart note never covers
    # what is being looked at.
    docker run -d --init --name "$name" --shm-size=1g --add-host releases.herdr.dev:127.0.0.1 -v "$repo:/src" ${KIOSK_SANDBOX_MOUNTS:-} "$image" sleep infinity >/dev/null
  fi
}

case ${1:-} in
  rm) docker rm -f "$name" >/dev/null; echo "removed $name" ;;
  e2e | record | shots) ensure; exec docker exec -e SANDBOX_APPEARANCE -e SANDBOX_THEME -e SANDBOX_SCALE -e KIOSK_MOD -e E2E_SHOTS -e SHOTS_OUT "$name" bash "/src/scripts/app-sandbox/$1.sh" "${@:2}" ;;
  *) ensure; exec docker exec -e SANDBOX_APPEARANCE -e SANDBOX_THEME -e SANDBOX_SCALE "$name" /src/scripts/app-sandbox/sandbox "$@" ;;
esac
