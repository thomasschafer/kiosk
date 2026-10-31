#!/usr/bin/env bash
#
# Builds the fictional repositories that README previews show, the same every run:
#
#   ROOT/repos/       a few repositories, one of them tidepool, cloned from ROOT/tidepool.git
#                     with branches only on its remote
#   ROOT/worktrees/tidepool/feat-csv-export, feat-offline-sync, fix-timezone-rounding,
#                     refactor-storage-layer
#                     worktrees of tidepool branches
#
# Use it through the flake, which provides Git:
#
#   nix run github:thomasschafer/kiosk#preview-fixture -- ROOT
#
# ROOT must not exist yet.

set -euo pipefail

[ $# -eq 1 ] || {
    printf 'usage: preview-fixture ROOT\n' >&2
    exit 1
}
[ ! -e "$1" ] || {
    printf 'preview-fixture: %s already exists\n' "$1" >&2
    exit 1
}
mkdir -p "$1"
ROOT=$(cd "$1" && pwd -P)
readonly ROOT

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Demo GIT_AUTHOR_EMAIL=demo@example.invalid
export GIT_COMMITTER_NAME=Demo GIT_COMMITTER_EMAIL=demo@example.invalid
export GIT_AUTHOR_DATE=2026-01-01T12:00:00Z GIT_COMMITTER_DATE=2026-01-01T12:00:00Z

commit() {
    local checkout=$1 message=$2
    printf '%s\n' "$message" >>"$checkout/CHANGELOG"
    git -C "$checkout" add CHANGELOG
    git -C "$checkout" commit -qm "$message"
}

new_repo() {
    git init -q -b main "$1"
    commit "$1" "Initial commit"
}

mkdir "$ROOT/repos" "$ROOT/worktrees" "$ROOT/worktrees/tidepool"
for repo in aurora-api compass-cli ledger lighthouse night-market orbit-ui paper-planes; do
    new_repo "$ROOT/repos/$repo"
done

readonly ORIGIN="$ROOT/tidepool.git"
readonly SEED="$ROOT/seed"
git init -q --bare -b main "$ORIGIN"
new_repo "$SEED"
for branch in \
    chore/ci-cache \
    chore/release-0.9 \
    dependabot/cargo/serde_json-1.0.140 \
    dependabot/cargo/tokio-1.47.1 \
    docs/api-reference \
    feat/calendar-view \
    feat/home-screen-widget \
    feat/push-notifications \
    feat/recurring-tasks \
    feat/shared-lists \
    fix/flaky-sync-test \
    fix/ios-keyboard-overlap \
    fix/locale-date-parsing \
    perf/lazy-thumbnails \
    refactor/sync-queue; do
    git -C "$SEED" switch -q -c "$branch" main
    commit "$SEED" "Work on $branch"
done
git -C "$SEED" push -q --all "$ORIGIN"
rm -rf -- "$SEED"

readonly TIDEPOOL="$ROOT/repos/tidepool"
git clone -q "$ORIGIN" "$TIDEPOOL"
for branch in chore/bump-deps docs/getting-started; do
    git -C "$TIDEPOOL" branch "$branch" main
done
for branch in feat/csv-export feat/offline-sync fix/timezone-rounding refactor/storage-layer; do
    worktree="$ROOT/worktrees/tidepool/${branch//\//-}"
    git -C "$TIDEPOOL" worktree add -q -b "$branch" "$worktree" main
    commit "$worktree" "Work on $branch"
done
commit "$TIDEPOOL" "Merge the sync engine"
