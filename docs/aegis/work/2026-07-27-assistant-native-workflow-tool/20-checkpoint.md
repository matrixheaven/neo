# Assistant-native unified Workflow tool - Checkpoint

- Task ID: `2026-07-27-assistant-native-workflow-tool`
- Current todo: implementation and follow-on remediation complete.
- Active slice: completion candidate after Tasks 1-7 and review of commit
  `9004d14f`; focused verification is green.
- Blocked on: none for implementation; the approved three-consecutive-session
  model acceptance remains uncollected.
- Next step: collect the still-pending three consecutive strict model sessions
  when release-grade model-routing acceptance is required.

## Completed Todos

- Approved design: `b2bc08c7`.
- Approved implementation plan: `536c678f`.
- Task 1 unified adapter and failure semantics: `74cc07c3`, `7d48c08`.
- Task 2 permission routing: `69455260`.
- Task 3 capability retirement: `0c8f326e`.
- Task 4 production registry wiring: `782f5957`.
- Task 5 slash and headless CLI correction: `6757fdf2`.
- Task 6 skill routing: `442af376`.
- Task 7 integration and follow-on binding: `9004d14f`.
- Follow-on remediation now covers model-allowed `TaskAnswer`, actionable
  `TaskOutput.pending_user`, root-only policy, mixed-batch zero execution,
  one-off route correction, saved-workflow exceptions, direct `neo.swarm`,
  canonical `details.structured_output`, and immutable JSON markers.

## Evidence Refs

- `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/90-evidence.md`
- Strict-entry audited session:
  `session_d99ac5cf-a4be-480f-8827-9b29507375b6`.
- Zero-side-effect route correction session:
  `session_b2025de1-1871-427a-bfe6-6724f9ecfb08`.
- Focused tests listed in `90-evidence.md`.

## ResumeStateHint

- Preserve the user's unrelated `.gitignore` modification.
- The sole model-visible lifecycle tool is `Workflow`; `TaskAnswer` is the
  model answer path only when runtime `answer_policy` allows it.
- Both tools are root-only. Do not add a child alias, CLI fallback, capability,
  nonce, or second workflow state owner.
- A known saved workflow may use `list`/`show`/`run_saved` directly. Inline
  authoring/new definitions/one-off evaluation activate `create-workflow`
  unless it is already active.
- Do not generalize from model-generated `.tmp` reports; canonical evidence is
  the tool wire plus workflow journal.
- Do not claim the approved three-session acceptance until three fresh strict
  sessions exist.

## DriftCheckDraft

- Original intent: aligned; assistants can validate, save, launch, inspect, and
  answer model-allowed gates without slash or CLI prerequisites.
- Scope fence: aligned; no engine rewrite, persistence migration, nested
  workflow, task-card redesign, predictive governance, or child-count cap.
- Baseline lock: aligned; registry, coordinator, runtime, task, lineage, and
  actual-usage owners remain canonical.
- Compatibility boundary: saved definitions, journals, tasks, named slash, and
  human/script CLI remain preserved.
- Retirement boundary: aligned; capability/nonce/authorization-mode and
  `RunWorkflow` remain absent with no alias or fallback.
- Evidence status: implementation and focused regression evidence are current;
  one strict real session plus one zero-effect correction session are audited;
  three-consecutive-session model acceptance is pending.
- Advisory decision: implementation `done`; model-consistency acceptance
  `needs-verification` and remains explicit residual risk.

Method Pack records are advisory evidence, not completion authority.
