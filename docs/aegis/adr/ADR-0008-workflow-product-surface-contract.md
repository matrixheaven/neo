# ADR-0008 - Workflow Product Surface Contract

Status: `recorded-from-work`
Date: `2026-07-28`

## Context

ADR-0007 established the assistant-native `Workflow` tool and deleted the
capability/nonce authorization system. Implementation of the Workflow product
surface redesign expanded the assistant contract with self-contained mutation
actions, V3 journal child lifecycle, Operator projection, automatic retention,
and a narrowed four-command CLI.

## Decision

### Human CLI

`neo workflow` exposes exactly four same-level commands:

- `list` — show available definitions
- `run` — execute a saved workflow
- `check` — validate without running
- `test` — run against a fixture

Exit codes: `0` success, `1` user/workflow failure, `2` input invalid, `3`
non-interactive awaiting input, `4` host/runtime failure, `130` interruption.

### Assistant Tool

The `Workflow` tool has seven actions: `list`, `show`, `validate_inline`,
`validate_saved`, `save`, `run_inline`, `run_saved`. Each mutation action
performs complete preflight internally. No mandatory validation ordering.

### Journal V3

New runs write `JOURNAL_FORMAT_V3` with generic `ChildQueued`/`ChildStarted`/
`ChildFinished` lifecycle events. V1/V2 remain readable without migration.

### Operator Projection

The `/tasks` Workflow Operator produces immutable snapshots with stable cursors
for paging. Live activity from `MultiAgentRuntime` enriches non-terminal rows.

### Automatic Retention

Trigger at 90% global storage, reclaim to 80%. Minimum eligible age: 30 days.
Only terminal, unreferenced, unpinned runs are eligible.

## Alternatives Considered

- Keep broad nine-command CLI family. Rejected: exposes backend mechanics.
- Keep mandatory validate-before-run. Rejected: runtime owns safety, not prompt
  choreography.
- Keep V2 journal format. Rejected: unable to support generic child lifecycle
  without SwarmItem* backward-compatibility constraints.

## Consequences

- CLI scripts using removed commands (`show`, `save`, `answer`, `fork`, `prune`)
  must update.
- Saved workflow runs use V3 journal, readable across versions.
- Automatic retention prevents unbounded storage growth without user-facing CLI.

## Supersedes

Supersedes the human CLI, model action semantics, and operator-surface portions
of ADR-0007. ADR-0006/0007 remain historical for runtime, registry, journal,
and platform durable boundaries.
