# Nightly real-agent E2E runbook

This runbook is the operating contract for automated runs of `scripts/nightly-e2e.sh`.

## Purpose

Run real-agent end-to-end detection tests on a regular cadence against current agent versions, and classify failures as either test regressions or infrastructure issues.

## Canonical command

Run from the repository root:

```bash
flock -n /tmp/kiosk-nightly-e2e.lock ./scripts/nightly-e2e.sh --update-agents --branch main
```

Notes:
- `flock` prevents overlapping runs.
- `--update-agents` updates tools before testing.
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
2. For surprising agent status (`UNKNOWN`, `IDLE`, stale `WAITING`), gather:
   - `kiosk status <repo> <branch> --json --debug-agent | jq '{agent_status, agent_debug}'`
   - `kiosk panes <repo> <branch> --json`
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
