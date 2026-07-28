# Workflow Product Surface Contract — Landed Baseline

Status: `recorded-from-adr`
Date: `2026-07-28`
ADR: `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
Supersedes (historical): `docs/aegis/baseline/2026-07-27-assistant-native-workflow-contract.md`

This baseline records the landed Workflow product surface redesign, verified
by focused test evidence at commit `b52c172a`.

## Product / Requirement Baseline

- Four same-level CLI commands: `list`, `run`, `check`, `test`. No hidden
  aliases or compatibility branches.
- Seven self-contained Workflow actions. `run_inline`, `run_saved`, and `save`
  perform complete validation preflight internally.
- `validate_inline` and `validate_saved` exist for explicit check-only intent.
- No prompt-keyword route gate, mandatory action ordering, or CLI/slash/Cargo
  prerequisite.
- `create-workflow` skill teaches authoring without being a launch grant.
- Journal format V3 for new runs; V1/V2 readable without migration.
- Generic ChildQueued/ChildStarted/ChildFinished V3 lifecycle events.
- V2 SwarmItem* events remain readable (projected to generic rows).
- Operator projection: WorkflowOperatorSnapshot, stable cursor paging.
- Automatic retention at 90% watermark, 80% target, 30-day minimum age.
- `/tasks` Workflow Operator view with Step/Agent/Details layouts.

## Architecture / Runtime Boundary Baseline

- WorkflowRuntime: sole durable lifecycle owner. Journal V3 writes.
- WorkflowLaunchCoordinator: stateless launch preflight.
- WorkflowDefinitionRegistry: sole trusted definition owner.
- BackgroundTaskManager: task lookup, control forwarding, durable/live join.
- TUI: selection, rendering, answer drafts only. No journal parsing.
- No second runtime, registry, scheduler, task system, or completion queue.

## Verification

All focused test evidence recorded at `docs/aegis/work/2026-07-28-workflow-product-surface-redesign/90-evidence.md`.
Key results: 657+ package tests pass, 5 operator tests, 5 V3 journal tests,
30 dispatch tests, 7 retention tests. TUI regressions (95 tests) unchanged.

## Residual Risk

- Three CLI integration tests require API credentials for `workflow run`.
- Native Windows/Linux/macOS terminal evidence is pending (Task 10).
- Unofficial docs (docs/en, docs/zh) reference retired CLI commands.
