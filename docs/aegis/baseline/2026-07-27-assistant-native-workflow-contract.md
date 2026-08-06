# Local Workflow Platform — Assistant-Native Contract Baseline

Status: `historical-superseded`
Date: `2026-07-27`
ADR: `docs/aegis/adr/ADR-0007-assistant-native-workflow-contract.md`
Superseded by: `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`

This baseline records the **landed assistant-native workflow contract** plus
focused test evidence from Tasks 1-7. The exact fresh three-session black-box
acceptance remains pending.

This snapshot is retained as historical evidence. Use the superseding 2026-07-28
baseline for current Workflow product-surface decisions; use this file only for
unchanged assistant-tool and retirement history.

## Product / Requirement Baseline

- `Workflow` is the sole model-visible workflow tool (root-only, seven flat actions).
- No model-visible `RunWorkflow`, alias, or fuzzy action matching.
- No model tool, skill, or permission path requires a slash command, capability, nonce, or grant.
- `create-workflow` is required for inline authoring, new definitions, and
  one-off evaluation unless already active; known saved workflows may use
  `list`/`show`/`run_saved` directly.
- One-off evaluation is `Skill(create-workflow) -> Workflow(validate_inline) ->
  Workflow(run_inline) -> TaskOutput`, without assistant
  CLI/hash/manifest/Cargo/source exploration.
- `TaskOutput.pending_user` exposes the typed request and `next_action`;
  `TaskAnswer` is used only for runtime model-allowed gates.
- Named `/workflow <name> [JSON_OBJECT]` remains host-direct, zero-model-turn.
- Exact bare `/workflow` activates `create-workflow` via canonical manual skill path + visible model turn.
- Headless CLI remains human/script surface only.

## Architecture / Runtime Boundary Baseline

- `WorkflowDefinitionRegistry` is sole trusted definition owner, session-shared, injected into production AgentConfig/ToolContext.
- `WorkflowLaunchCoordinator` is stateless; no authorization mode or capability host.
- `WorkflowRuntime` remains sole durable owner; `create_linked_run` no longer accepts capability reservations.
- Permission/approval layer owns typed `WorkflowSave` and `WorkflowLaunch` reviews.
- `Workflow` and `TaskAnswer` are root-registry-only. Child, restricted,
  schema-repair, and workflow-script tool sets prohibit both; runtime
  `human`/`human_or_model` policy remains the answer authority.
- Capability types, nonces, authorization modes, grants/bind/consume/revoke are deleted with no alias.

## Verification

Focused test evidence is recorded in the linked work record. Stale-owner scan
confirms zero active capability/nonce/authorization-mode references.

One audited session strictly begins `Skill -> validate_inline -> run_inline ->
TaskOutput` and completes a real `TaskAnswer` gate. A second session proves
zero-side-effect route correction, but later uses the CLI. The exact fresh
three-consecutive-session acceptance remains pending; these traces and focused
tests are not a substitute for that gate.

## Residual Risk

- The fresh three-session black-box acceptance is pending, including its model
  routing evidence.
- No native Windows/Linux model-behavior sessions exist; the platform tool
  contract is independent but model behavior remains unproven there.
- Unrelated `.gitignore` modification preserved.
