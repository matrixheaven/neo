# ADR-0008 - Workflow Product Surface Contract

Status: `recorded-from-work`
Date: `2026-07-28`
Updated: `2026-08-01`

## Source Evidence

- Approved redesign: `docs/aegis/specs/2026-07-27-workflow-product-surface-redesign.md`.
- Implementation plan and handoff: `docs/aegis/plans/2026-07-28-workflow-product-surface-redesign.md` and `docs/aegis/handoffs/2026-07-28-workflow-product-surface-redesign.md`.
- Landed baseline: `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`.
- Provider-safe child-output amendment: `docs/aegis/specs/2026-08-01-workflow-ai-usability-repair-design.md` and its linked plan, handoff, and evidence.

## Context

ADR-0007 established the assistant-native `Workflow` tool and deleted the
capability/nonce authorization system. Implementation of the Workflow product
surface redesign expanded the assistant contract with self-contained mutation
actions, canonical child lifecycle records, Operator projection, automatic
retention, and a narrowed four-command CLI.

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

### Model-visible action results

`Workflow` returns one compact JSON object in its model-visible result content.
The object contains the action-specific fields needed for the next decision:
available definitions, definition schemas, validation errors, task IDs, and the
exact `TaskOutput` next action. `TaskOutput` likewise returns the requested
bounded summary, journal, result, artifact, or artifact-content page in its
content. Typed `details` remains the richer UI and event projection; the model
does not need it to choose the next action. Historical saved definitions may
omit `input_schema` to represent no arguments, while `output_schema` remains
required.

Inline workflow definitions must declare both `input_schema` and
`output_schema`. A no-argument inline workflow declares the explicit object
schema `{"type":"object","additionalProperties":false}`. Historical saved
definitions may still omit `input_schema` and remain readable and runnable
without migration.

Schema-constrained child requests carry the exact output schema through the
provider-neutral strict response format on the initial turn and on the one
tools-disabled repair turn. Host acceptance stays strict: one JSON value, no
fence, no prose, and no formatting tool call. A completed child lifecycle and
an accepted requested result are separate facts. When the child completes but
its structured result fails validation, Delegate and the workflow invocation
are failed while `schema_error`, its error code, actual usage, and child
references remain available. This state does not create a
`workflow_outcome_error`.

Required built-in child and verification outcomes call `neo.fail` on failure;
they do not manufacture placeholder success. The outer TUI renders this
two-layer status from typed details, while Delegate, DelegateGroup, and
DelegateSwarm cards, grouping, activity, expansion, and placement remain
unchanged.

### Ordinary failures and terminal failures

Ordinary host operations return immutable `ok = false` outcomes so Lua can
branch directly on `outcome.ok`. This includes failed verification, failed
command verification, unknown tools, and centrally denied orchestration or control tools.
These outcomes do not require `pcall` and do not terminate the run. Explicit
`neo.fail`, uncaught Lua errors, resource exhaustion, cancellation, and invalid
final results remain terminal workflow failures. `TaskOutput` is the workflow
task read/wait path; `WaitDelegate` remains limited to delegate and swarm IDs.

### Definition And Runtime Ownership

`WorkflowDefinitionRegistry` is the sole owner of stored workflow definitions
and their resolution. `WorkflowRuntime` is the sole durable owner of run
lifecycle, state, journal, replay, recovery, lineage, artifact references, and
aggregate results. Launch coordination, background tasks, session events, and
the TUI are adapters or projections; none writes workflow truth.

### Canonical Journal

Every run uses one `JournalEnvelope` and typed `JournalPayload` stream. New
runs, resume, output paging, retention, the harness, and child projection all
read the same journal through the canonical scanner and recovery path. Direct
delegates and swarm items share the generic `ChildQueued`/`ChildStarted`/
`ChildFinished` lifecycle. Live activity enriches non-terminal projections but
does not create another durable record stream.

### Operator Projection

The `/tasks` Workflow Operator produces immutable snapshots with stable cursors
for paging. Live activity from `MultiAgentRuntime` enriches non-terminal rows.

### Automatic Retention

Trigger at 90% global storage, reclaim to 80%. Minimum eligible age: 30 days.
Only terminal runs older than the configured minimum age are eligible.

## Alternatives Considered

- Keep broad nine-command CLI family. Rejected: exposes backend mechanics.
- Keep mandatory validate-before-run. Rejected: runtime owns safety, not prompt
  choreography.
- Keep swarm-specific child records. Rejected: direct delegates and swarm items
  need one lifecycle contract and one projection path.
- Keep multiple journal readers or writers. Rejected: duplicate contracts make
  durable ownership and recovery ambiguous.
- Treat a completed child lifecycle as a successful requested result. Rejected:
  lifecycle completion does not prove that the declared structured result was
  accepted.

## Amendment (2026-08-01): deterministic provider-safe child output

Report 004 and the Workflow AI usability repair implementation disproved the
earlier assumption that the `openai` compatible wire could serialize a native
JSON Schema response hint. The provider type cannot distinguish official Chat
Completions JSON Schema support from arbitrary compatible endpoints, so the
ambiguous wire now has one deterministic request shape:

| Provider type | Wire behavior |
| --- | --- |
| `openai_response` | Map the internal hint to native `text.format` JSON Schema. |
| `openai` | Omit the internal hint from the compatible request body. |
| `anthropic` / `google` | Omit the internal hint (unchanged). |

The child prompt still carries the exact schema and JSON-only rules, and all
returned text still passes the single strict host parser and JSON Schema
validator with at most one tools-disabled content-repair turn. A child turn
that failed at the provider, authentication, rate-limit, cancellation, or
runtime level never enters schema parsing or repair; the original actionable
error and observed usage are preserved. Failed swarm summaries expose
`failed <failed>/<total>` plus the first bounded child error. Final-result
schema failures include the instance path and a bounded Unicode-safe preview
of the failing node; validation remains terminal and never starts model
repair.

Rejected alternatives: error-text matching, HTTP-400 retry, provider capability
settings, endpoint allowlists, probes, automatic protocol fallback, optional
inline schemas, flat tool aliases, catchable terminal failure, and second
parsers/validators/prompt owners. None of these paths exist in the codebase.

## Amendment (2026-08-07): task browser responsive UI

The approved `/tasks` and Workflow Operator presentation redesign (design
`docs/aegis/specs/2026-08-07-task-browser-ui-design.md`, implementation plan
`docs/aegis/plans/2026-08-07-task-browser-ui-redesign.md`) narrows the visual
and responsive surface contract of the Operator without changing any runtime,
journal, task-control, persistence, or model-visible behavior:

- The general Task Browser uses a persistent inspector at `>= 100` content
  columns (task list clamped to `30-42`), one full-width page at `70-99`
  (list, Details, or Latest output), and the same single-page route below
  `70` with at most two stable task-row lines.
- The Workflow Operator uses a wide split (`>= 100`: Steps left, Agents right,
  lower selected-agent preview), a stacked layout (`70-99`: summary, Steps,
  Agents, compact preview above a fixed footer), and a small tabbed layout
  (`< 70`: stable header, `[STEPS]`/`[AGENTS]` selector, one active page).
- Issued Bash/Terminal commands wrap and scroll in Agent Details and are never
  silently cut; bounded output regions are visibly labelled `Latest output ·
  Preview` with a shown/total fraction.
- Pointer input routes through the same layout geometry as rendering: clicks
  select the row under the pointer; wheel moves the pane under the pointer;
  drag/release stay consumed while the browser is open.
- The former small-width sequential route (`Workflow summary -> Steps ->
  Agents -> Agent details`) is retired; the only small-width route is
  `Workflow header -> [Steps | Agents] -> Agent details`. No compatibility
  setting, alias, or fallback branch preserves the old sequence.
- Workflow runtime ownership, the canonical journal, child projection,
  `WorkflowChildRow` fields (including `generated_files`, now copied through
  to the TUI view), Delegate-family transcript cards, session JSONL, provider
  requests, and compaction input remain unchanged.

## Consequences

- CLI scripts using removed commands (`show`, `save`, `answer`, `fork`, `prune`)
  must update.
- Saved workflow runs, resume, output paging, retention, the harness, and child
  projection use the same journal contract.
- Automatic retention prevents unbounded storage growth without user-facing CLI.
- Strict provider-neutral child response formats improve first-turn schema
  adherence without weakening host validation. Provider capability and model
  authoring quality remain runtime verification concerns.

## Compatibility Boundary

Saved definitions, the seven assistant actions, canonical workflow journals,
resume and recovery, task controls, headless CLI, permission ownership, and
Delegate-family card presentation remain intact. ADR-0009 changes only the
interactive slash-entry route; it does not replace these product and runtime
boundaries.

## Retirement Impact

The broad nine-command CLI, mandatory validate-before-run choreography,
swarm-only child records, duplicate journal readers and writers, and the
assumption that lifecycle completion proves structured-result acceptance are
retired without active aliases.

## Baseline Sync

- Needed: `resolved`
- Target: `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`
- Action: current landed baseline exists
- Reason: product surface, journal, retention, and result ownership are recorded there.

## Evidence References

- `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`
- `docs/aegis/work/2026-07-28-workflow-product-surface-redesign/20-checkpoint.md`
- `docs/aegis/work/2026-07-28-workflow-product-surface-redesign/90-evidence.md`
- `docs/aegis/work/2026-07-28-workflow-final-review/final-review.md`

## Supersedes

Supersedes the human CLI, model action semantics, journal event model, and
operator-surface portions of ADR-0007. ADR-0006/0007 remain historical context
for the runtime, registry, and platform durability decisions recorded there;
this ADR is authoritative for the current Workflow product surface and journal
contract.

ADR-0009 later supersedes only the interactive slash-entry portion. The
remaining product surface, journal, task, result, and retention decisions here
remain current.

This ADR is an advisory Aegis Method Pack record. It does not grant completion authority or replace project-authoritative architecture sources.
