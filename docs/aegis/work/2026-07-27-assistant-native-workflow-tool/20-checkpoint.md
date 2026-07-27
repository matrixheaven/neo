# Assistant-native unified Workflow tool - Checkpoint

- Task ID: `2026-07-27-assistant-native-workflow-tool`
- Current todo: Task 3 - retire capability state from launch and linked runs.
- Active slice: Task 2 committed as `69455260`; permission routing is now the
  sole interactive authorization owner for Workflow save/launch.
- Blocked on: none.
- Next step: delete `workflow/capability.rs` and strip nonce/authorization-mode
  state from the launch coordinator, linked runs, and fork paths (Task 3).

## Completed Todos

- Approved design committed as `b2bc08c7`.
- Implementation plan committed as `536c678f`.
- Unified root-only `Workflow` adapter committed as `74cc07c3`.
- Pair rollback and typed launch-stage failure semantics committed as
  `7d48c08`.
- Task 1 focused verification passed.
- Task 1 handoff prepared for Tasks 2-7.
- Task 2 normal-permission routing committed as `69455260` (typed save/launch
  reviews, plan-mode matrix, zero-effect preflight before approval, capability
  revoke logic removed from permission paths).

## Evidence Refs

- `commit:74cc07c3`
- `commit:7d48c08`
- `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/90-evidence.md`
- `docs/aegis/handoffs/2026-07-27-assistant-native-workflow-tool.md`

## ResumeStateHint

- Preserve the user's unrelated `.gitignore` modification.
- Read the spec, plan, runtime baseline, ADR-0006, this checkpoint, and the
  handoff before editing.
- Begin Task 2 only. Do not reopen design or repeat the Grok comparison.
- The sole model-visible workflow tool is `Workflow`; there is no
  `RunWorkflow` alias.
- `PreparedWorkflowLaunch`, `LaunchAuthorizationMode::Headless`, capability
  carriers, and the empty default registry are temporary bridges, not final
  architecture.
- Task 4 must inject the existing session-shared registry into production root
  tool contexts; do not create another registry owner.
- Tasks 2-5 retire capability state in the approved dependency order.
- Do not begin engine, persistence, nested-workflow, or task-card work.

## DriftCheckDraft

- Original intent: aligned for Task 1.
- Scope fence: aligned; no engine, persistence, nested-workflow, CLI removal,
  or task-card expansion occurred.
- Baseline lock: aligned; registry, coordinator, runtime, task, lineage, and
  actual-usage owners remain intact.
- Compatibility boundary: saved definitions, journals, tasks, named slash, and
  headless CLI remain preserved.
- Retirement boundary: explicit and pending Tasks 2-5; no alias was added.
- New risk signals:
  - production registry injection remains Task 4 work;
  - capability and Headless authorization bridges remain until Tasks 2-5.
- Evidence status: sufficient for Task 1 completion, not for the whole plan.
- Advisory decision: `continue` with Task 2.

Method Pack records are advisory evidence, not completion authority.
