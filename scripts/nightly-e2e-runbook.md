# Nightly real-agent E2E runbook

This runbook is the operating contract for automated runs of `scripts/nightly-e2e.sh`.

## Purpose

Run real-agent end-to-end status tests on a regular cadence against current agent versions, and classify failures as either test regressions or infrastructure issues.

## What the tests cover

`kiosk/tests/e2e_real_agents.rs` starts Claude Code and Codex, each in a tmux server of its own on a `kiosk-test-*` socket, and checks what `kiosk agents --json` reports as the agent goes from idle, through running and waiting for permission to run a shell command, to finished. Each prompt is one short, cheap request. Where the agent reports its own status over OSC 7501 the test expects kiosk to use that, including `done` once the agent has finished; otherwise it expects the screen heuristics, which report a finished agent as `idle`. Each run logs the states kiosk reported, which shows when an agent starts or stops sending OSC 7501.

The agents run with the narrowest permissions that still reach a permission prompt: Claude Code in `manual` mode with only the Bash tool and without the user's settings files, and Codex with a read-only sandbox, approvals sent to the user, and its hooks left untrusted. Neither is given a bypass flag.

Each agent works in a repo at `~/.cache/kiosk-e2e-real-agents/<agent>` (or under `$XDG_CACHE_HOME`), which the tests recreate on every run. Keeping the path fixed means the folder trust each agent saves in its own config is one entry, not one per run. Beyond that, the agents write to their config dirs only what any session does, such as transcripts.

## Running by hand

The tests are ignored by default and refuse to run without `KIOSK_E2E_REAL_AGENTS=1`, so normal test runs and CI never start real agents. To run them without the script's git and update steps:

```bash
KIOSK_E2E_REAL_AGENTS=1 nix develop -c cargo test -p kiosk --test e2e_real_agents -- --ignored --nocapture
```

## Canonical command

Run from the repository root:

```bash
./scripts/nightly-e2e.sh --update-agents --branch main
```

Notes:
- Runs share the agents' repos, so each test locks its agent's repo and fails at once if another run, from any checkout, holds it.
- The Codex test passes `--no-daemon`, which older Codex releases reject.
- `--update-agents` updates both agents with their own `update` commands before testing.
- `--branch main` ensures a stable target branch for scheduled runs.

## Exit code contract

`scripts/nightly-e2e.sh` normalizes results to:

- `0`: all tests passed
- `1`: one or more tests failed (likely product/test regression)
- `2`: infrastructure failure (setup/tooling/timeout/preflight/build issues)

## Logs and artifacts

Each run writes one timestamped log:

- `logs/nightly-e2e/YYYY-MM-DD_HHMMSS.log`

The script keeps logs for 30 days.

Recommended report payload:
- exit code
- branch
- HEAD commit (`git log --oneline -1`)
- log file path
- agent versions printed by the run

## Failure triage flow

1. Read the run log and identify whether the failure is a test failure (`exit 1`) or infrastructure failure (`exit 2`).
2. For surprising agent status (`unknown`, `idle`, stale `waiting`), gather:
   - `kiosk agents --json`, which gives each pane's status, its source (`osc7501` or `heuristics`) and, for heuristics, the screen rule that matched
   - `tmux list-panes -t "<session>" -F '#{pane_id} cmd=#{pane_current_command} title=#{pane_title} dead=#{pane_dead}'`
   - `tmux capture-pane -ep -t "<pane_id>" -S -2000 | tail -n 120`
3. Include the relevant snippets and command outputs in the failure report.

## Safety boundaries for automation

Allowed:
- run scheduled command
- retry once on transient infra failures
- gather diagnostics and open an issue or PR with evidence

Not allowed:
- auto-merge code
- mutate auth/account state
- broad cleanup of tmux sessions not created by the current run

## Dry-run checklist

1. Confirm one successful run from scheduler context.
2. Confirm one failing run is captured and classified correctly.
3. Confirm diagnostics can be collected end-to-end from the same automation context.
