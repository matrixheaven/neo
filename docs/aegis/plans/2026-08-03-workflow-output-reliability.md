# Workflow Output Reliability Implementation Plan

Date: `2026-08-03`

Status: `ready for implementation after spec review`

This plan executes
`docs/aegis/specs/2026-08-03-workflow-output-reliability-design.md`. It
supersedes the output-enforcement tasks in
`docs/aegis/plans/2026-08-01-workflow-ai-usability-repair.md`.

## Implementer Directive

Use this as the implementation route. Do not reopen the schema-versus-prompt
debate, rewrite Lua as Rhai, add provider probes, or preserve the retired
repair path under a new name. Use the existing `WorkflowRuntime`, Lua host,
journal, and child result channels.

The shared worktree is dirty. Preserve all unrelated changes, especially:

- `.gitignore`;
- `crates/neo-agent/src/modes/interactive/input.rs`;
- `crates/neo-agent/src/modes/interactive/tests.rs`;
- `crates/neo-tui/src/transcript/presentation.rs`;
- `crates/neo-tui/src/transcript/progressive.rs`;
- `crates/neo-tui/src/transcript/store.rs`.

Do not modify the `Ctrl+O` fix or Delegate-family card presentation in this
task. Work only in the files listed by each task and add no compatibility
runtime, second parser, or new schema owner.

## Goal And Stop Condition

Make completed child work survive output-shape disagreement, remove hidden
repair spending, separate execution status from business data, and make the
built-in research/review Workflows produce useful partial results.

Stop when every acceptance criterion has focused evidence, the retired active
paths have zero matches, and any platform/provider limitation is reported as a
residual risk rather than hidden behind a green local test.

## Facts, Assumptions, Unknowns

### Facts

- The current child path is centered in
  `accept_child_structured_output_with_repair` in
  `crates/neo-agent-core/src/workflow/runtime.rs`.
- Foreground Delegate calls the path from
  `crates/neo-agent-core/src/tools/delegate.rs`.
- Direct Workflow swarm calls the same logical path from
  `crates/neo-agent-core/src/workflow/runtime.rs`.
- Child schema requirements are enforced while lowering Lua requests in
  `crates/neo-agent-core/src/workflow/lua.rs`.
- Final Lua output is gated by `accept_final_lua_result`.
- Built-ins currently read host `.ok` and use `neo.fail` for ordinary result
  disagreement.

### Assumptions

- Existing child output/event records already contain the original text and
  usage needed to preserve them without a second result store.
- Old journal records can remain deserializable while new code stops emitting
  repair events.

### Unknowns

- Exact current persisted sessions containing an open schema-repair record.
- Live provider availability for end-to-end confirmation.

Unknowns constrain verification only; they do not justify adding a fallback
parser or restoring the repair turn.

## Change Necessity

- **User-visible need:** completed Workflow work must not be lost after a
  child's structured projection differs from a prompt schema.
- **No-change option:** changing only prompts cannot prevent host code from
  converting projection failure into execution failure.
- **Minimum code boundary:** child result projection, outcome status, final
  result persistence, and the three affected built-ins.
- **Decision:** code-change.

## Canonical Owners

| Concern | Owner | Required change |
| --- | --- | --- |
| Child lifecycle and usage | existing `MultiAgentRuntime` result | preserve unchanged; expose to projection |
| Optional structured projection | existing workflow/delegate result path | one parse/validation attempt, no repair |
| Workflow execution state | `WorkflowInvocationOutcome.status` | retire top-level `ok` as host verdict |
| Final Lua persistence | `WorkflowRuntime::persist_canonical_final_result` | persist regardless of output mismatch |
| Business truth | each Workflow's `details`/final data | use `verified`, `supported`, `partial`, and gaps |
| Authoring guidance | existing `create-workflow` skill | document optional projection semantics |

No new owner is needed.

## TDD Route

- **Mode:** `off`
- **Decision:** `skipped`
- **Strict authority:** `not applicable`
- **Test posture:** diagnostic reproduction plus post-change focused regression
- **Reason:** strict test-first work was not requested; the session evidence
  and current tests already identify the failure path.
- **Verification:** every command below names one package, one target selector,
  and one test-name filter.

## Tasks

### Task 1: Separate host execution status from business result data

Files:

- `crates/neo-agent-core/src/workflow/state.rs`
- `crates/neo-agent-core/src/workflow/runtime_support.rs`
- `crates/neo-agent-core/src/workflow/runtime.rs`
- `crates/neo-agent-core/src/workflow/lua.rs`
- `crates/neo-agent-core/src/workflow/journal.rs`
- `crates/neo-agent-core/src/workflow/mod.rs`
- `crates/neo-agent-core/src/runtime/workflow_dispatch.rs`
- `crates/neo-agent-core/src/runtime/workflow_recovery_dispatch.rs`
- direct tests constructing `WorkflowInvocationOutcome`

Changes:

1. Remove active use of `WorkflowInvocationOutcome.ok` as a host verdict and
   make status comparisons use `WorkflowOutcomeStatus`.
2. Keep old serialized `ok` data readable if the current serde shape requires
   it, but do not write it as a new execution decision.
3. Remove `ok` from the Lua outcome table. Expose `status`, `summary`,
   `details`, usage, and child references.
4. Make `neo.verify(false, message)` return a completed host outcome with
   `details.verified = false` and the message. Keep explicit `neo.fail`
   terminal.
5. Update log, phase, report, recovery, swarm aggregation, and tool-result
   code to use execution status only.

Check:

```bash
cargo nextest run -p neo-agent-core --test workflow_lua verify_false_is_completed_data
cargo nextest run -p neo-agent-core --test workflow_runtime invocation_status_uses_execution_state
```

Expected result: a false verification is visible as data and does not abort;
real failed/cancelled outcomes still remain terminal.

### Task 2: Delete child repair and make projection optional

Files:

- `crates/neo-agent-core/src/workflow/runtime.rs`
- `crates/neo-agent-core/src/workflow/journal_scan.rs`
- `crates/neo-agent-core/src/workflow/output.rs`
- `crates/neo-agent-core/src/workflow/error.rs`
- `crates/neo-agent-core/src/tools/delegate.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/multi_agent/mod.rs`
- `crates/neo-agent-core/src/workflow/lua.rs`
- `crates/neo-agent-core/src/workflow/schema.rs`
- `crates/neo-agent-core/src/workflow/recovery.rs`
- `crates/neo-agent-core/src/workflow/journal.rs`
- `crates/neo-agent-core/src/workflow/harness.rs`
- `crates/neo-agent-core/tests/workflow_schema.rs`
- `crates/neo-agent-core/tests/workflow_lua.rs`

Changes:

1. Remove mandatory child `output_schema` checks from `agent` and `swarm`
   lowering. Keep schema compilation for explicitly supplied valid definitions
   at the local boundary.
2. Replace the repair-owned acceptance path with one existing-owner projection
   step: parse/validate once when a schema is supplied, attach structured data
   only on success, and attach a bounded projection-unavailable diagnostic on
   mismatch.
3. Do not call `run_schema_repair_turn`, do not append new
   `SchemaRepairStarted`/`SchemaRepairFinished` records, and do not aggregate a
   repair usage amount.
4. Preserve child lifecycle, original text/result, provider/runtime error, and
   all observed usage on every path.
5. Keep old repair journal variants readable only as historical input if replay
   requires them; they must have no active dispatch or recovery behavior.

Check:

```bash
cargo nextest run -p neo-agent-core --test workflow_schema child_projection_mismatch_keeps_completed_status
cargo nextest run -p neo-agent-core --test workflow_schema child_projection_mismatch_does_not_start_repair
cargo nextest run -p neo-agent-core --test workflow_schema failed_child_preserves_original_error_without_projection
cargo nextest run -p neo-agent-core --test workflow_schema swarm_mixed_results_preserve_usage_and_partial_state
```

Expected result: an extra `claim_note`, prose, or invalid JSON can make only
the projection unavailable; a provider/runtime failure remains the original
failure and creates no projection or repair request.

### Task 3: Stop final output schema from vetoing persistence

Files:

- `crates/neo-agent-core/src/workflow/runtime.rs`
- `crates/neo-agent-core/src/workflow/lua.rs`
- `crates/neo-agent-core/src/workflow/schema.rs`
- `crates/neo-agent-core/tests/workflow_schema.rs`
- `crates/neo-agent-core/tests/workflow_lua.rs`

Changes:

1. Keep `persist_canonical_final_result` as the one persistence path.
2. Change `accept_final_lua_result` so a declared output schema can produce a
   projection diagnostic but cannot reject or fail to persist the returned Lua
   value.
3. Keep malformed definition/schema checks local and before provider work; do
   not turn a model/result mismatch into `schema_invalid_final_result`.
4. Preserve artifact limits, final-result journal records, usage, terminal
   reasons, and replay behavior.

Check:

```bash
cargo nextest run -p neo-agent-core --test workflow_schema final_lua_result_schema_mismatch_is_persisted
cargo nextest run -p neo-agent-core --test workflow_lua final_return_preserves_value_when_projection_is_unavailable
```

Expected result: the exact returned value is durable and the Workflow remains
completed even when it does not match the declared output shape.

### Task 4: Repair built-in Workflow degradation paths

Files:

- `crates/neo-agent-core/src/workflow/builtins/deep-research.lua`
- `crates/neo-agent-core/src/workflow/builtins/code-review.lua`
- `crates/neo-agent-core/src/workflow/builtins/large-refactor.lua`
- their paired `.workflow.toml` definitions when result fields change
- `crates/neo-agent-core/tests/workflow_builtins.rs`

Changes:

1. Replace host `outcome.ok` checks with `outcome.status == "completed"` checks
   for actual child execution failures.
2. Treat missing `details.structured_output`, projection diagnostics,
   contradictions, gaps, unsupported findings, and verification false values
   as result data and set a deterministic `partial` result.
3. Reserve `neo.fail` for actual child/host failure or explicit user/script
   policy. Do not call it for a missing projection or a negative evidence
   judgment.
4. Return bounded deterministic fallback reports that identify the missing
   evidence and retain all completed findings.
5. Remove schemas that encode host `ok` as the Workflow's execution result;
   keep only useful domain fields for optional projection metadata.

Check:

```bash
cargo nextest run -p neo-agent-core --test workflow_builtins deep_research_projection_unavailable_is_partial
cargo nextest run -p neo-agent-core --test workflow_builtins deep_research_verification_false_is_partial
cargo nextest run -p neo-agent-core --test workflow_builtins code_review_child_projection_gap_is_partial
cargo nextest run -p neo-agent-core --test workflow_builtins large_refactor_child_projection_gap_is_partial
```

Expected result: the session example with a schema-forbidden `claim_note`
produces a useful partial Workflow instead of a terminal `schema_invalid`.

### Task 5: Align model guidance and visible result descriptions

Files:

- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- `crates/neo-agent-core/src/skills/builtin/mod.rs`
- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent-core/tests/workflow_tool_policy.rs`
- `crates/neo-agent-core/tests/workflow_model_visibility.rs`

Changes:

1. State plainly that output schemas guide an optional structured projection;
   they are not a guarantee that an AI will emit exactly those fields.
2. State that projection mismatch preserves the child and does not start a
   repair turn.
3. Explain that `status` is execution state and `verified`/`supported`/
   `partial` are Workflow data.
4. Keep input-schema and malformed-definition rules that protect local host
   boundaries. Remove statements claiming every child output schema is a
   mandatory execution requirement.
5. Keep the existing single authoring skill and single Workflow tool
   description; do not add a second guidance surface.

Check:

```bash
cargo nextest run -p neo-agent-core --test workflow_tool_policy create_workflow_guidance_describes_optional_output_projection
cargo nextest run -p neo-agent-core --test workflow_model_visibility workflow_result_exposes_status_and_business_data
```

Expected result: the model receives one coherent explanation instead of a
prompt/schema promise that the host later treats as an execution protocol.

### Task 6: Focused acceptance and retirement checks

Files:

- tests touched by Tasks 1-5;
- `docs/aegis/adr/` or `docs/aegis/baseline/` only if the implementation
  changes an existing decision record;
- no unrelated source files.

Checks:

```bash
rg -n "run_schema_repair_turn|accept_child_structured_output_with_repair|SchemaRepairStarted|SchemaRepairFinished" crates/neo-agent-core/src
rg -n "outcome\.ok|verification\.ok|primary\.ok|counterpoints\.ok|context_child\.ok" crates/neo-agent-core/src/workflow crates/neo-agent-core/src/tools/delegate.rs
cargo fmt --all --check
git diff --check
```

The first scan may retain historical journal deserialization symbols only when
needed for replay; it must show no new repair dispatch. The second scan must
show no active host-status use in the Workflow path. Any remaining `ok` inside
domain result JSON must be explained as data, not execution state.

## Verification And Residual Risk

Focused tests prove the named local paths only. They do not prove all live
providers, remote CI, or native Windows/Linux behavior. Live acceptance should
record the resolved provider type, one child with an extra output field, one
provider/runtime failure, one partial research run, and one completed final
result. Missing credentials or provider availability must be reported as a
blocker with the exact command and endpoint, not replaced with a claim of
success.

## Retirement

Delete the active repair execution path and old fail-closed consumers in the
same implementation. Do not add aliases or a second compatibility runtime.
Retain only the minimum historical deserialization needed to open existing
sessions. If that reader is no longer required after checking current replay
fixtures, delete it too and update its negative test.

## Commit Boundary

One verified commit for the complete logical change is preferred because Tasks
1-4 change one execution/result boundary. Do not stage the dirty unrelated
files listed above. Do not push, switch branches, create a worktree, or modify
user session data as part of this plan.
