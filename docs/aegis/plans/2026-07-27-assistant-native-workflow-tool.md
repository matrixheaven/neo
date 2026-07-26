# Assistant-Native Workflow Tool Implementation Plan

Date: `2026-07-27`

Status: `approved spec; user authorized direct implementation`

## Goal

Replace the model-facing `RunWorkflow` plus `WorkflowCapability` contract with
one assistant-native `Workflow` tool. The top-level model must list, show,
validate, save, and run workflows through first-party tools, without using the
Neo CLI or requiring a slash command.

## Architecture

`Workflow` is the sole model adapter. It has seven flat actions: `list`,
`show`, `validate_inline`, `validate_saved`, `save`, `run_inline`, and
`run_saved`.

It reuses existing owners:

- `WorkflowDefinitionRegistry` owns trusted list/resolve/save;
- canonical definition resolution and coordinator preflight own validation;
- `WorkflowLaunchCoordinator` owns launch ordering and rollback;
- `WorkflowRuntime` owns durable runs, replay, lineage, and actual usage;
- `BackgroundTaskManager` owns task projection/control;
- normal permission preparation and typed approvals are the sole human
  authorization owner.

No runtime, registry, scheduler, persistence, task, or permission owner is
created.

## Authority And Compatibility

Primary authority:
`docs/aegis/specs/2026-07-27-assistant-native-workflow-tool-design.md`.

Historical runtime evidence:
`docs/aegis/baseline/2026-07-26-workflow-platform-contract.md` and
`docs/aegis/adr/ADR-0006-local-workflow-platform.md`. Their model capability
launch contract is superseded; their durable runtime/registry/task boundaries
remain authoritative.

Preserve paired definitions, SHA-256 revision framing, durable journals, task
controls, named slash launch, headless CLI, linked-run lineage, actual-usage
admission, and child-effect authorization. Intentionally remove `RunWorkflow`
and `WorkflowCapability` with no alias or compatibility branch.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: targeted post-change regression and black-box acceptance
- Reason: this is an integrated contract replacement; focused regressions prove
  the relevant boundaries without inventing a strict RED/GREEN requirement.

## Readiness And Boundaries

Change necessity: documents/skill text alone cannot make the existing tool
launch or remove the duplicate authorization owner. The minimum code boundary
is the adapter, permission path, capability retirement, coordinator/linked-run,
registry exposure, root/child tool registries, slash adapter, and skill.

Existence check: `Workflow` replaces the existing adapter and delegates to
existing owners. It is a justified new public tool contract, not a new durable
owner.

Retirement decision: `delete-first`. `WorkflowCapability` is internal code and
has no persistent state or verified external dependency.

Execution lock:

- Do not add a script engine, persistence migration, nested workflow launch,
  delete/rename action, task-control duplicate, arbitrary child cap, or prompt
  fallback.
- `Workflow` is root-only. Child tool registries and `neo.tool` keep workflow
  launch unavailable.
- Any discovered external consumer requiring old APIs pauses for a user
  compatibility decision; unknown use is not evidence.

## Task 1: Implement The Unified `Workflow` Adapter

Files:

- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent-core/src/tools/workflow_tests.rs`
- `crates/neo-agent-core/src/tools/mod.rs`

Change:

1. Retire `RunWorkflowInput`, `RunWorkflowPhaseInput`, `RunWorkflowTool`, old
   validation helpers, approval presentation helper, and old result wording.
2. Add a `WorkflowAction` enum with exactly the seven approved serialized
   action names.
3. Add a flat `WorkflowInput` with `#[serde(deny_unknown_fields)]` and optional
   `name`, `description`, `phases`, `script`, `input_schema`, `output_schema`,
   `args`, `scope`, `replace`, `cursor`, and `limit` fields.
4. Parse the input once to a private `PreparedWorkflowAction`. This parser owns
   the action-field matrix, null-as-absent behavior, exact missing-field
   diagnostics, forbidden saved/inline mixtures, and zero-side-effect errors.
5. Add `WorkflowTool` with a model-visible description that says to use it for
   list/show/validate/save/run/use/test/evaluate, to activate
   `create-workflow` for inline authoring, to use `TaskOutput` after run, and
   not to inspect source/Cargo/call `neo workflow` for assistant-native use.
6. Implement `list` through the registry result with only result-page slicing;
   no second index. Implement `show` through registry resolve.
7. Implement `validate_inline` through canonical dynamic resolution plus
   coordinator preflight and `validate_saved` through registry resolve plus the
   same preflight. Neither may create a file/run/task/effect.
8. Implement `save` as a thin translation to `WorkflowDefinitionRegistry::save`.
   The host computes the hash, creates the manifest, atomically writes, and
   refreshes the registry.
9. Implement run actions from resolved definitions via the coordinator, with
   launch sources `model:Workflow(run_inline)` and
   `model:Workflow(run_saved)`.
10. Return stable structured details: `ok`, `action`, `status`, workflow
    metadata, validation/items/task fields, error field, and `next_actions`.
11. Register `WorkflowTool` only at the root; remove root `RunWorkflowTool`.
12. Add action-shape, zero-effect, host-save, task-result, and structured-output
    tests.

Verification:

```bash
cargo test --package neo-agent-core --lib tools::workflow::workflow_tests -- --nocapture
cargo nextest run -p neo-agent-core --test workflow_registry save_is_no_clobber_and_pair_atomic
```

Commit: `feat(workflow): add unified workflow tool adapter`

## Task 2: Route `Workflow` Through Normal Permissions

Files:

- `crates/neo-agent-core/src/runtime/permission.rs`
- `crates/neo-agent-core/src/permissions.rs`
- `crates/neo-agent-core/src/approval.rs`
- focused permission/approval tests

Change:

1. Replace the `RunWorkflow` capability early branch with action-aware
   preparation based on the same typed workflow parser.
2. Let list/show/validate execute directly in Ask/Auto/Yolo and plan mode after
   zero-effect validation.
3. Add/reuse a typed Workflow Save review: scope, pair paths, create/replace,
   source, phases, schemas, and `Save/Revise/Cancel` actions.
4. Retain the existing typed Workflow Launch review for both run actions:
   source, phases, args, child-effect warning, and `Launch/Revise/Cancel`.
5. Reject save/run in plan mode; do not reject read/validate.
6. Ensure Revise/Cancel return feedback/cancellation without file, run, task,
   nonce, or cleanup side effects.

Verification:

```bash
cargo test --package neo-agent-core --lib runtime::permission::tests -- --nocapture
cargo nextest run -p neo-agent-core --test workflow_launch -- --nocapture
```

Commit: `feat(workflow): route workflow actions through normal permissions`

## Task 3: Retire Capability State From Launch And Linked Runs

Files:

- Delete `crates/neo-agent-core/src/workflow/capability.rs`
- `crates/neo-agent-core/src/workflow/mod.rs`
- `crates/neo-agent-core/src/workflow/launch.rs`
- `crates/neo-agent-core/src/workflow/runtime.rs`
- `crates/neo-agent-core/src/workflow/error.rs`
- `crates/neo-agent-core/src/tools/background_tasks.rs`
- `crates/neo-agent-core/tests/workflow_launch.rs`
- `crates/neo-agent-core/tests/workflow_lineage.rs`

Change:

1. Delete capability types/exports and authorization-specific error codes.
2. Remove nonce from `WorkflowLaunchBinding`; retain session/workspace/actor,
   permission, lineage, and compiled-schema fields.
3. Delete intent digest, authorization modes, bind/consume/rollback paths, and
   capability host field. Preserve coordinator preflight, durable create, task
   registration, start event, worker start, and existing rollback order.
4. Change all coordinator callers to the simplified intent/hosts shape.
5. Remove `WorkflowCapabilityReservation` from
   `WorkflowRuntime::create_linked_run`. Its durable lineage/admission/rollback
   rules remain owned by the runtime.
6. Remove self-grant/reserve from linked fork and CLI fork paths. Independent
   forks must not contend on a global one-shot lock.
7. Rewrite tests for direct Ask/Auto/Yolo launch, multiple independent launch,
   invalid-preflight zero create, Revise/Cancel zero run, coordinator order, and
   independent concurrent linked runs.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_launch -- --nocapture
cargo nextest run -p neo-agent-core --test workflow_lineage -- --nocapture
cargo test --package neo-agent-core --lib workflow::launch::tests -- --nocapture
```

Commit: `refactor(workflow): remove launch capability authorization`

## Task 4: Remove Capability Carriers And Make Workflow Root-Only

Files:

- `crates/neo-agent-core/src/runtime/config.rs`
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent/src/config/mod.rs`, `config/loader.rs`,
  `config/mutations.rs`, `mcp_ops.rs`
- run/session/interactive construction sites and fixtures reported by compiler
- `crates/neo-agent-core/tests/workflow_tool_policy.rs`

Change:

1. Delete `workflow_capability` fields, defaults, builders, debug output,
   propagation, and lifecycle code from config, ToolContext, controller/session,
   and fixture constructors.
2. Remove ToolContext capability injection from default tool context.
3. Do not register `WorkflowTool` in `with_builtin_child_tools`; retain it in
   the workflow-script deny classifier.
4. Update role/policy tests: root has `Workflow`; child/restricted/schema-repair
   tool sets do not; no active `RunWorkflow` remains.
5. Fix stale constructors only; do not add compatibility defaults.

Verification:

```bash
cargo test --package neo-agent-core --lib tools::tests -- --nocapture
cargo nextest run -p neo-agent-core --test workflow_tool_policy -- --nocapture
```

Commit: `refactor(workflow): remove capability plumbing and child launch`

## Task 5: Correct Named And Bare Slash Plus Headless CLI

Files:

- `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- stale lifecycle wiring in `modes/interactive/input.rs` and controller/session
  code only as compiler requires
- `crates/neo-agent/src/modes/workflow.rs`
- interactive and CLI workflow tests

Change:

1. Replace bare `/workflow` grant/status behavior with canonical manual
   `create-workflow` activation and a normal visible model turn. It creates no
   hidden entitlement.
2. Preserve exact named `/workflow <name> [JSON_OBJECT]`, registry resolve,
   zero-model-turn behavior, and normal Ask review.
3. Delete named-launch grant/revoke/unbind handling. Revise/Cancel only resolve
   the review and emit feedback/status.
4. Use the simplified coordinator for named slash and headless CLI run/fork.
5. Replace capability lifecycle tests with bare authoring/no-grant, named Ask
   Launch/Revise/Cancel, named direct launch, headless run, and `/workflowish`
   boundary tests.

Verification:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::named_workflow_slash_launches_without_model_call --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests -- --nocapture
cargo test --package neo-agent --bin neo -- modes::workflow::tests -- --nocapture
```

Commit: `fix(workflow): make slash authoring assistant-native`

## Task 6: Rewrite `create-workflow` Routing

Files:

- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- `crates/neo-agent-core/src/skills/builtin/mod.rs`
- affected skill/discovery tests

Change:

1. Broaden discovery to create/save/run/use/test/evaluate/inspect workflows,
   including black-box evaluation inside the Neo repository.
2. Replace CLI-first procedure with the approved action table:
   create-only -> save -> offer run; create/test -> save -> run_saved ->
   TaskOutput; one-off evaluation -> validate_inline -> run_inline ->
   TaskOutput; known saved -> run_saved; discover -> list/show/run_saved.
3. Prohibit assistant CLI calls, manual hashes, source/Cargo inspection, and
   slash prerequisites when using workflows as a feature.
4. Keep canonical engine API documentation, but make host save the pair/hash
   owner. Update done criteria to require real structured outcomes.
5. Preserve the explicit implementation-debug escape hatch.

Verification:

```bash
cargo test --package neo-agent-core --lib skills::builtin::tests -- --nocapture
cargo test --package neo-agent-core --lib runtime::skill_dispatch::tests -- --nocapture
```

Commit: `feat(skills): route workflow requests through workflow tool`

## Task 7: Integrate And Prove The User Flow

Files:

- only contract-required tests/fixtures
- `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/` evidence/checkpoint
- a new ADR/baseline record only after all implementation proof

Change and verification:

1. Run all focused tests above, `cargo fmt --all --check`, and `git diff --check`.
2. Scan active sources for `WorkflowCapability`, reservation, authorization
   mode, nonce, missing-capability message, old model `RunWorkflow`, and skill
   CLI instructions. Historical documents are allowed matches.
3. In three fresh normal Neo sessions, run the exact approved Chinese request:

   ```text
   请你在.tmp/ 下，去全面测试我的dynamic workflow功能，调用相关 skill 和 tool，深度评测，给我结论和报告
   ```

   Required trace: `Skill(create-workflow) -> Workflow(validate_inline) ->
   Workflow(run_inline) -> TaskOutput -> report`; no repo/source/CLI/Cargo tool
   appears before the first `Workflow` call.
4. Run manual-skill, create-only, create-and-test, and implementation-debug
   scenarios from design Section 22.
5. Record exact native-platform evidence and distinguish it from unrun platform
   coverage.
6. Create a superseding ADR/baseline record only after fresh proof; do not edit
   historical docs in place.

Verification:

```bash
cargo fmt --all --check
rg -n "WorkflowCapability|WorkflowCapabilityReservation|LaunchAuthorizationMode|launch_nonce|Use the exact /workflow slash command first" crates/neo-agent-core crates/neo-agent
git diff --check
```

Commit: `docs: record assistant-native workflow contract` only after proof.

## Risks And Drift Rules

- A flat action schema that cannot represent the approved matrix without a
  nested union pauses for design amendment; do not invent a fallback schema.
- A linked-run regression pauses for runtime diagnosis; do not reintroduce a
  global one-shot lock.
- Black-box model routing failure after three diagnosed attempts pauses for
  contract review; do not pile on prompt branches.
- No full cross-platform claim without native evidence.
- Any new durable owner, alias, capability branch, nested launch, engine change,
  persistence migration, or arbitrary total child cap is scope drift.

## Execution Readiness View

- Intent Lock: one assistant-native top-level `Workflow` tool replaces model
  `RunWorkflow` and the capability ritual.
- Scope Fence: no engine/persistence/task-card/nested-workflow expansion.
- Baseline Lock: preserve runtime, registry, coordinator, task, actual-usage,
  and child-effect authorization ownership.
- Retirement Boundary: delete capability module, fields, nonce, authorization
  modes, errors, tests, lifecycle logic, and old tool registration.
- Test Obligations: all task-specific focused tests, formatter, retirement scan,
  three primary black-box sessions, and four secondary scenarios.
- Review Gates: each implementation slice receives spec-compliance then
  code-quality review before the next dependent slice.
- Evidence Required Before Completion: focused tests, exact traces, scoped diff,
  and post-proof ADR/baseline record.

## Self-Review

Every approved action, permission rule, retirement requirement, slash/child
boundary, model-routing rule, and acceptance row maps to Tasks 1-7. The plan
contains no compatibility fallback, no placeholder task, and no unowned runtime
responsibility.
