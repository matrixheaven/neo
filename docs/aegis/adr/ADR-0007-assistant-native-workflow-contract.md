# ADR-0007 - Assistant-native Workflow contract

Status: `recorded-from-work`
Date: `2026-07-27`

## Source Evidence

- Approved design: `docs/aegis/specs/2026-07-27-assistant-native-workflow-tool-design.md` (`b2bc08c7`).
- Implementation plan: `docs/aegis/plans/2026-07-27-assistant-native-workflow-tool.md` (`536c678f`).
- Focused test evidence and the pending fresh acceptance record: `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/90-evidence.md`.

## Context

ADR-0006 shipped a completed P0-P2 workflow platform with a model launch
contract centered on `RunWorkflow` + `WorkflowCapability`. A real Neo session
demonstrated that the contract was human-command-first instead of
assistant-native: the model inspected repository sources, invoked the CLI
through Bash because the authoring skill taught CLI paths, and eventually hit a
capability gate that required a slash command only the user could issue.

## Decision

Replace the model-facing `RunWorkflow` plus `WorkflowCapability` contract with
one assistant-native `Workflow` tool. The model sees exactly one canonical
workflow tool with seven flat actions (`list`, `show`, `validate_inline`,
`validate_saved`, `save`, `run_inline`, `run_saved`). Every workflow lifecycle
step is discoverable and executable through first-party model tools, without
CLI or slash-command prerequisites.

The durable runtime owners — `WorkflowRuntime`, `WorkflowDefinitionRegistry`,
`WorkflowLaunchCoordinator`, `BackgroundTaskManager` — remain their original
ADR-0006 boundaries. What changes is the model-facing adapter layer:

1. `Workflow` replaces `RunWorkflow` as the sole model-visible workflow tool.
2. Interactive human authorization moves entirely to the existing
   permission/approval layer (typed Save/Launch/Revise/Cancel reviews).
3. The `WorkflowCapability` nonce/grants/bind/consume/reserve system is
   deleted without a compatibility branch.
4. `LaunchAuthorizationMode` and capability-nonce-carrying launch bindings are
   removed; the coordinator holds no authorization state.
5. Linked-run forks and independent launches no longer contend on a global
   one-shot capability lock.
6. `Workflow` and `TaskAnswer` are present only in the root model tool registry.
   Child, restricted, schema-repair, and workflow-script tool sets cannot reach
   workflow launch or answer a workflow gate. `TaskAnswer` remains governed by
   the durable request's runtime `human`/`human_or_model` policy.
7. Named `/workflow <name> [JSON_OBJECT]` remains host-direct and
   zero-model-turn, but exact bare `/workflow` now activates `create-workflow`
   through the canonical manual skill path and starts a normal model turn.
8. The `create-workflow` skill is the first action for inline authoring, new
   definitions, and one-off evaluation unless already active. Known saved
   workflows may use `list`/`show`/`run_saved` directly. Every lifecycle step
   routes through `Workflow`, with no CLI, hash, manifest, Cargo, or
   source-inspection fallback.
9. Headless `neo workflow ...` CLI is supported for humans and scripts; the
   assistant never uses it as a workflow product path.

## Alternatives Considered

- Patch the capability contract with a tool that could auto-grant. Rejected:
  duplicate authorization ownership of the same feature is architecturally
  unsound.
- Keep `RunWorkflow` as a compatibility alias. Rejected: doubling the model
  surface with no benefit and a stale authorization model.
- Teach the model to call the CLI through Bash when `Workflow` is registered.
  Rejected: first-party tool preference is a first-principles invariant.

## Consequences

- The model tool surface for workflow lifecycle is one tool with an explicit
  action discriminator. The model never needs a slash capability, a nonce, or
  CLI access.
- One-off evaluation follows `Skill(create-workflow) ->
  Workflow(validate_inline) -> Workflow(run_inline) -> TaskOutput`.
- `TaskOutput.pending_user` is the actionable answer contract. The model calls
  `TaskAnswer` only when `next_action` permits it; human-only gates wait for the
  user.
- Ask-mode save and launch reviews show typed presentations with target-pair
  paths (save) or source/phases/args (launch). Revise and Cancel create no
  files, runs, or tasks.
- Plan mode permits read/validate actions and denies save/launch mutation.
- Existing paired `.lua` + `.workflow.toml` definitions, V2 journals,
  artifacts, lineage, and headless CLI remain compatible and require no
  migration.
- The exact fresh three-session black-box acceptance remains pending. Until it
  is recorded, this ADR makes no model-behavior or business-trace completion
  claim beyond the focused test evidence.

## Compatibility Boundary

Preserve: paired definitions, SHA-256 revision framing, V2 journals,
artifacts, lineage, linked-run durability, task controls, actual-usage
admission, named slash launch, headless CLI, and child-effect authorization.
Delegate/DelegateGroup/DelegateSwarm card designs, and Bash/Terminal admission
semantics are unchanged.

## Retirement Impact

Retire without fallback as active code: `workflow/capability.rs`,
`WorkflowCapability`, `WorkflowCapabilityReservation`,
`LaunchAuthorizationMode`, capability fields in config/ToolContext/context,
launch nonce fields, bare-slash grant behavior, and `RunWorkflow`.
Historical specs, plans, and ADR-0006 retain those terms as evidence of the
retired design. Source-owners retirement scan confirms zero active
authorization-mode, nonce, or capability-type references in product source.

## Baseline Sync

- Needed: needed
- Target: `docs/aegis/baseline/2026-07-27-assistant-native-workflow-contract.md`
- Action: create superseding snapshot
- Reason: The assistant-native contract replaces the capability-based launch
  baseline from ADR-0006; fresh three-session acceptance remains pending.

## Evidence References

- `docs/aegis/specs/2026-07-27-assistant-native-workflow-tool-design.md`
- `docs/aegis/plans/2026-07-27-assistant-native-workflow-tool.md`
- `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/20-checkpoint.md`
- `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/90-evidence.md`
- Fresh three-session black-box acceptance (pending; no session traces claimed)

## Supersedes

- ADR: `docs/aegis/adr/ADR-0006-local-workflow-platform.md`
- Reason: ADR-0006 recorded the original model launch contract with
  `RunWorkflow` + `WorkflowCapability`. The assistant-native correction lands
  `Workflow` as canonical and deletes capability authorization; this ADR is the
  current authority for the model tool, permission, slash, and child-tool-policy
  boundaries. ADR-0006 remains historical for runtime, registry, journal,
  lineage, and platform durable boundaries.

## Boundary

This ADR is an advisory Aegis Method Pack record. It does not grant completion
authority or replace project-authoritative architecture sources.
