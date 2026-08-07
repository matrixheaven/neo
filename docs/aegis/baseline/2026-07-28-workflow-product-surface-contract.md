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
- One journal format for all runs. Old journal formats are unsupported.
- Generic ChildQueued/ChildStarted/ChildFinished lifecycle events.
- Operator projection: WorkflowOperatorSnapshot, stable cursor paging.
- Automatic retention at 90% watermark, 80% target, 30-day minimum age.
- `/tasks` Workflow Operator view with Step/Agent/Details layouts.

## Architecture / Runtime Boundary Baseline

- WorkflowRuntime: sole durable lifecycle owner and journal writer.
- WorkflowLaunchCoordinator: stateless launch preflight.
- WorkflowDefinitionRegistry: sole trusted definition owner.
- BackgroundTaskManager: task lookup, control forwarding, durable/live join.
- TUI: selection, rendering, answer drafts only. No journal parsing.
- No second runtime, registry, scheduler, task system, or completion queue.

## Amendment (2026-08-07): task browser responsive UI

The approved `/tasks` and Workflow Operator presentation redesign adds a
narrow visual/responsive amendment to this contract (design
`docs/aegis/specs/2026-08-07-task-browser-ui-design.md`, verified by the
implementation plan `docs/aegis/plans/2026-08-07-task-browser-ui-redesign.md`):

- General Task Browser: persistent inspector at `>= 100` columns (list
  clamped `30-42`), one full-width page at `70-99` (list / Details / Latest
  output), single-page route below `70` with stable two-line task rows.
- Workflow Operator: wide split at `>= 100` (Steps | Agents + lower
  selected-agent preview), stacked at `70-99` (summary, Steps, Agents, compact
  preview, fixed footer), tabbed at `< 70` (stable header, `[Steps | Agents]`,
  one active page). `Enter` opens the full-width Agent Details page at any
  width; `Esc` returns through Details, Workflow, and tasks.
- Complete Bash/Terminal commands wrap and scroll in Agent Details (never
  silently cut); bounded output is visibly labelled `Latest output · Preview`.
- Pointer clicks select the row under the pointer; wheel moves the pane under
  the pointer; drag/release stay consumed while the browser is open.
- The former small-width sequential route (`summary -> Steps -> Agents ->
  details`) is retired with no compatibility setting or fallback branch.
- Runtime ownership, journal contract, child projection (including
  `generated_files` copy-through to the TUI view), Delegate-family cards,
  session JSONL, provider requests, and compaction input are unchanged.

## Verification

All focused test evidence recorded at `docs/aegis/work/2026-07-28-workflow-product-surface-redesign/90-evidence.md`.
Key results cover journal recovery and retention, child lifecycle projection,
dispatch, and the Workflow view in `/tasks`.

## Residual Risk

- Real `workflow run` verification requires a configured provider credential.
