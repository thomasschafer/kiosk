#!/usr/bin/env bash
#
# Nightly real-agent E2E test runner for kiosk.
#
# Runs the agent detection E2E tests against real, locally-installed coding
# agents (Claude Code, Codex, OpenCode, Cursor CLI, Gemini CLI). Designed to be invoked
# by a cron job or manually.
#
# Prerequisites:
#   - All agents installed and authenticated (see README)
#   - Rust toolchain available
#   - tmux available
#
# Usage:
#   ./scripts/nightly-e2e.sh [--update-agents] [--branch <branch>]
#
# Options:
#   --update-agents   Update all agents to latest versions before testing
#   --branch <name>   Git branch to test (default: current branch)
#
# Exit codes:
#   0 - All tests passed
#   1 - One or more tests failed
#   2 - Infrastructure error (missing agent, build failure, etc.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
LOG_DIR="${REPO_DIR}/logs/nightly-e2e"
TIMESTAMP="$(date +%Y-%m-%d_%H%M%S)"
LOG_FILE="${LOG_DIR}/${TIMESTAMP}.log"

# Ensure ~/.local/bin is on PATH for agents installed via npm/curl
export PATH="${HOME}/.local/bin:${HOME}/.cargo/bin:${PATH}"

UPDATE_AGENTS=false
TARGET_BRANCH=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --update-agents) UPDATE_AGENTS=true; shift ;;
    --branch) TARGET_BRANCH="$2"; shift 2 ;;
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

# --- Pre-flight checks ---

log "=== Nightly E2E run started ==="
log "Repo: ${REPO_DIR}"

# Check required tools
for tool in cargo tmux git; do
  command -v "$tool" &>/dev/null || die "${tool} not found on PATH"
done

# Check agents are installed
AGENTS_MISSING=()
command -v claude &>/dev/null  || AGENTS_MISSING+=("claude")
command -v codex &>/dev/null   || AGENTS_MISSING+=("codex")
command -v opencode &>/dev/null || AGENTS_MISSING+=("opencode")
command -v cursor &>/dev/null  || AGENTS_MISSING+=("cursor")
command -v gemini &>/dev/null  || AGENTS_MISSING+=("gemini")

if [[ ${#AGENTS_MISSING[@]} -gt 0 ]]; then
  log "WARNING: Missing agents: ${AGENTS_MISSING[*]}"
  log "Tests for missing agents will be skipped or fail"
fi

# --- Update agents (optional) ---

if [[ "$UPDATE_AGENTS" == true ]]; then
  log "Updating agents to latest versions..."

  if command -v npm &>/dev/null; then
    log "Updating Codex..."
    npm update -g @openai/codex 2>&1 | tee -a "$LOG_FILE" || log "WARNING: Codex update failed"

    log "Updating OpenCode..."
    npm update -g opencode-ai 2>&1 | tee -a "$LOG_FILE" || log "WARNING: OpenCode update failed"
  fi

  if command -v claude &>/dev/null; then
    log "Updating Claude Code..."
    claude update --yes 2>&1 | tee -a "$LOG_FILE" || log "WARNING: Claude Code update failed"
  fi

  if command -v gemini &>/dev/null; then
    log "Updating Gemini CLI..."
    npm update -g @google/gemini-cli 2>&1 | tee -a "$LOG_FILE" || log "WARNING: Gemini CLI update failed"
  fi

  # Cursor CLI: re-run install script to get latest
  if command -v cursor &>/dev/null; then
    log "Updating Cursor CLI..."
    curl -fsSL https://cursor.com/install | bash 2>&1 | tee -a "$LOG_FILE" || log "WARNING: Cursor CLI update failed"
  fi

  log "Agent versions after update:"
  claude --version 2>&1 | tee -a "$LOG_FILE" || true
  codex --version 2>&1 | tee -a "$LOG_FILE" || true
  opencode --version 2>&1 | tee -a "$LOG_FILE" || true
  cursor --version 2>&1 | tee -a "$LOG_FILE" || true
  gemini --version 2>&1 | tee -a "$LOG_FILE" || true
fi

# --- Git operations ---

cd "$REPO_DIR"

if [[ -n "$TARGET_BRANCH" ]]; then
  log "Switching to branch: ${TARGET_BRANCH}"
  git fetch origin 2>&1 | tee -a "$LOG_FILE"
  git checkout "$TARGET_BRANCH" 2>&1 | tee -a "$LOG_FILE" || die "Failed to checkout ${TARGET_BRANCH}"
  git pull origin "$TARGET_BRANCH" 2>&1 | tee -a "$LOG_FILE" || die "Failed to pull ${TARGET_BRANCH}"
else
  log "Pulling latest on current branch: $(git branch --show-current)"
  git pull 2>&1 | tee -a "$LOG_FILE" || die "Failed to pull"
fi

log "HEAD: $(git log --oneline -1)"

# --- Build ---

log "Building kiosk..."
if ! cargo build --package kiosk 2>&1 | tee -a "$LOG_FILE"; then
  die "Build failed"
fi

# --- Run tests ---

log "Running real-agent E2E tests..."
log "Agent versions:"
claude --version 2>&1 | tee -a "$LOG_FILE" || true
codex --version 2>&1 | tee -a "$LOG_FILE" || true
opencode --version 2>&1 | tee -a "$LOG_FILE" || true
cursor --version 2>&1 | tee -a "$LOG_FILE" || true
gemini --version 2>&1 | tee -a "$LOG_FILE" || true

TEST_EXIT=0
# 10-minute timeout: real agents can hang if auth expires or network stalls
timeout 600 env KIOSK_E2E_REAL_AGENTS=1 cargo test \
  --package kiosk \
  --test e2e_agent \
  -- --test-threads=1 \
  2>&1 | tee -a "$LOG_FILE" || TEST_EXIT=$?

if [[ $TEST_EXIT -eq 124 ]]; then
  log "FATAL: Test run timed out after 10 minutes"
fi

# --- Results ---

if [[ $TEST_EXIT -eq 0 ]]; then
  log "=== ALL TESTS PASSED ==="
else
  log "=== TESTS FAILED (exit code: ${TEST_EXIT}) ==="
fi

# Keep last 30 days of logs
find "$LOG_DIR" -name "*.log" -mtime +30 -delete 2>/dev/null || true

exit $TEST_EXIT
