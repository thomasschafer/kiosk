# Nightly Real-Agent E2E Tests

Runs kiosk's agent detection E2E tests against **real, locally-installed coding
agents** to catch regressions when agents update their TUI output.

## Setup Status (nix-server)

| Agent       | Installed | Authed | Notes                                    |
|-------------|-----------|--------|------------------------------------------|
| Claude Code | ✅        | ✅     | v2.1.63, `~/.local/bin/claude`           |
| Codex       | ✅        | ✅     | v0.104.0, `~/.local/bin/codex`           |
| OpenCode    | ✅        | ❓     | v1.2.15, `~/.local/bin/opencode` — config exists but no provider set |
| Gemini CLI  | ❌        | ❌     | Not installed                            |
| Cursor CLI  | ❌        | ❌     | Not installed                            |

**Important:** `~/.local/bin` is NOT on PATH in non-interactive shells.
The nightly script handles this, but interactive setup needs
`export PATH="$HOME/.local/bin:$PATH"` first.

## TODOs

Before this is operational, the following manual steps are needed:

- [x] **Auth Claude Code** — already authed via `claude.ai` OAuth
- [x] **Install & auth Codex** — already installed and authed
- [ ] **Auth OpenCode** — installed but needs provider auth. SSH in, run
      `export PATH="$HOME/.local/bin:$PATH" && opencode`, then use
      `/connect → OpenAI → ChatGPT Plus/Pro` to auth via browser OAuth
- [ ] **Install Gemini CLI** — run
      `npm install -g @google/gemini-cli --prefix ~/.local`
      then run `gemini` and complete Google OAuth (free tier via
      Google AI Studio is sufficient)
- [ ] **Install Cursor CLI** — run
      `curl -fsSL https://cursor.com/install | bash`
      then run `cursor` and complete browser auth (Hobby tier, free)
- [ ] **Set up OpenClaw cron job** — after all agents are authed, configure
      a nightly cron (e.g. 3am London time) that runs
      `./scripts/nightly-e2e.sh --update-agents` and reports results via
      Telegram
- [ ] **Verify** — run `./scripts/nightly-e2e.sh` manually once to confirm
      everything works end-to-end

## How It Works

- GitHub CI continues to run **fake-agent** E2E tests on every PR (fast,
  deterministic, no auth needed)
- This server runs **real-agent** E2E tests nightly, catching breakage from
  agent updates that change TUI output
- Tests use `KIOSK_E2E_REAL_AGENTS=1` which launches actual agent binaries
  instead of fake scripts

## Auth & Cost

All agents use **flat-rate subscription auth** — no API keys, no variable costs:

| Agent       | Auth                          | Subscription       |
|-------------|-------------------------------|---------------------|
| Claude Code | `claude auth login` (OAuth)   | Claude Pro/Max      |
| Codex       | First-run OAuth               | ChatGPT Plus/Pro    |
| OpenCode    | `/connect → OpenAI` (OAuth)   | ChatGPT Plus/Pro    |
| Cursor CLI  | Browser OAuth                 | Hobby (free)        |
| Gemini CLI  | `gemini` first-run OAuth      | Google AI Studio (free) |

Tokens auto-refresh. If one expires, the relevant tests will fail with an auth
error — just re-auth interactively.

## Usage

```bash
# Run tests against currently installed agent versions
./scripts/nightly-e2e.sh

# Update all agents to latest, then test
./scripts/nightly-e2e.sh --update-agents

# Test a specific branch
./scripts/nightly-e2e.sh --branch main

# Both
./scripts/nightly-e2e.sh --update-agents --branch feat/agent-status
```

Logs are written to `logs/nightly-e2e/` (gitignored, last 30 days retained).
The test run has a 10-minute timeout to prevent hung agents from blocking the
cron job.

## Test Design

Tests run with `--test-threads=1` because real agents share tmux and can't
run in parallel. Each test:

1. Launches a real agent with a trivial task in a tmux session
2. Waits for the agent to reach a specific state (running, waiting, idle)
3. Asserts kiosk detects the state correctly via CLI
4. Cleans up the tmux session

The `--update-agents` flag installs the latest version of each agent before
running, which is the primary value — detecting when an agent update changes
TUI output and breaks kiosk's detection patterns.
