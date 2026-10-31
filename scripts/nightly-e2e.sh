#!/usr/bin/env bash
#
# Nightly real-agent E2E test runner for kiosk.
#
# Runs the agent status E2E tests in kiosk/tests/e2e_real_agents.rs against the real,
# locally installed Claude Code and Codex. Designed to be invoked by a scheduler or
# manually; see scripts/nightly-e2e-runbook.md.
#
# Prerequisites:
#   - claude and codex installed and logged in
#   - Nix, for the dev shell that provides the Rust toolchain and tmux
#
# Usage:
#   ./scripts/nightly-e2e.sh [--update-agents] [--branch <branch>]
#
# Options:
#   --update-agents   Update both agents to their latest versions before testing
#   --branch <name>   Git branch to test (default: current branch)
#
# Exit codes:
#   0 - All tests passed
#   1 - One or more tests failed
#   2 - Infrastructure error (missing agent, build failure, timeout, etc.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
LOG_DIR="${REPO_DIR}/logs/nightly-e2e"
TIMESTAMP="$(date +%Y-%m-%d_%H%M%S)"
LOG_FILE="${LOG_DIR}/${TIMESTAMP}.log"
# The tests reuse one repo per agent, so runs must not overlap.
LOCK_DIR="${LOG_DIR}/.lock"

# Ensure ~/.local/bin is on PATH for agents installed by their own installers
export PATH="${HOME}/.local/bin:${HOME}/.cargo/bin:${PATH}"

AGENTS=(claude codex)
UPDATE_AGENTS=false
TARGET_BRANCH=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --update-agents) UPDATE_AGENTS=true; shift ;;
    --branch)
      if [[ $# -lt 2 || "$2" == --* ]]; then
        echo "Missing value for --branch" >&2
        exit 2
      fi
      TARGET_BRANCH="$2"
      shift 2
      ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$LOG_DIR"

log() {
  echo "[$(date '+%H:%M:%S')] $*" | tee -a "$LOG_FILE"
}

die() {
  log "FATAL: $*"
  exit 2
}

log_agent_versions() {
  for agent in "${AGENTS[@]}"; do
    "$agent" --version 2>&1 | tee -a "$LOG_FILE" || true
  done
}

if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  die "Another run holds ${LOCK_DIR}; remove it if no run is in progress"
fi
trap 'rmdir "$LOCK_DIR"' EXIT

log "=== Nightly E2E run started ==="
log "Repo: ${REPO_DIR}"

# --- Pre-flight checks ---

for tool in nix git; do
  command -v "$tool" &>/dev/null || die "${tool} not found on PATH"
done

for agent in "${AGENTS[@]}"; do
  command -v "$agent" &>/dev/null || die "${agent} not found on PATH"
done

# --- Update agents (optional) ---

if [[ "$UPDATE_AGENTS" == true ]]; then
  for agent in "${AGENTS[@]}"; do
    log "Updating ${agent}..."
    "$agent" update 2>&1 | tee -a "$LOG_FILE" || log "WARNING: ${agent} update failed"
  done
fi

# --- Git operations ---

cd "$REPO_DIR"

if [[ -n "$(git status --porcelain)" ]]; then
  die "Working tree is not clean; refusing to pull or switch branches"
fi

if [[ -n "$TARGET_BRANCH" ]]; then
  log "Switching to branch: ${TARGET_BRANCH}"
  git fetch origin 2>&1 | tee -a "$LOG_FILE" || die "Failed to fetch origin"
  git checkout "$TARGET_BRANCH" 2>&1 | tee -a "$LOG_FILE" || die "Failed to checkout ${TARGET_BRANCH}"
  git pull --ff-only origin "$TARGET_BRANCH" 2>&1 | tee -a "$LOG_FILE" || die "Failed to pull ${TARGET_BRANCH}"
else
  log "Pulling latest on current branch: $(git branch --show-current)"
  git pull --ff-only 2>&1 | tee -a "$LOG_FILE" || die "Failed to pull"
fi

log "HEAD: $(git log --oneline -1)"

# --- Build ---

TEST_ARGS=(--package kiosk --test e2e_real_agents)

log "Building kiosk and the real-agent tests..."
if ! nix develop -c cargo test "${TEST_ARGS[@]}" --no-run 2>&1 | tee -a "$LOG_FILE"; then
  die "Build failed"
fi

# --- Run tests ---

log "Running real-agent E2E tests..."
log "Agent versions:"
log_agent_versions

TEST_EXIT=0
# 10-minute timeout: real agents can hang if auth expires or network stalls. The dev shell
# provides timeout, which macOS lacks.
KIOSK_E2E_REAL_AGENTS=1 nix develop -c timeout 600 cargo test "${TEST_ARGS[@]}" \
  -- --ignored --nocapture \
  2>&1 | tee -a "$LOG_FILE" || TEST_EXIT=$?

# --- Results ---

if [[ $TEST_EXIT -eq 0 ]]; then
  log "=== ALL TESTS PASSED ==="
  FINAL_EXIT=0
elif [[ $TEST_EXIT -eq 124 || $TEST_EXIT -eq 137 ]]; then
  log "=== INFRASTRUCTURE FAILURE (timed out after 10 minutes) ==="
  FINAL_EXIT=2
elif [[ $TEST_EXIT -eq 101 ]]; then
  log "=== TESTS FAILED ==="
  FINAL_EXIT=1
else
  log "=== INFRASTRUCTURE FAILURE (exit code: ${TEST_EXIT}) ==="
  FINAL_EXIT=2
fi

# Keep last 30 days of logs
find "$LOG_DIR" -name "*.log" -mtime +30 -delete 2>/dev/null || true

exit $FINAL_EXIT
