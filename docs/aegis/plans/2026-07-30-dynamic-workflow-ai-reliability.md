# Neo Dynamic Workflow AI Reliability Implementation Plan

Date: `2026-07-30`

Status: `approved design; ready for implementation`

## Goal

Make dynamic workflows obtain schema-valid child results reliably, preserve
truthful failure semantics when validation still fails, and guide imperfect
models toward the one valid Workflow authoring path.

## Architecture

Keep the current owners:

```text
Workflow tool / create-workflow skill
  -> MultiAgentRuntime child request
  -> provider-neutral ResponseFormat
  -> strict host validation
  -> Delegate ToolResult
  -> WorkflowDispatchResolver outcome
  -> WorkflowRuntime journal
  -> outer TUI tool-result projection
```

`WorkflowRuntime` remains the sole persistent workflow owner. Agent lifecycle
and accepted workflow result remain separate facts. Delegate-family cards are
unchanged.

## Tech Stack

- Rust 2024, minimum Rust `1.96.1`;
- existing `ResponseFormat`, `RequestOptions`, `FakeHarness`, JSON Schema, Lua
  runtime, and transcript renderer;
- no new dependency, tool, parser, retry, alias, or persistence format.

## Baseline And Authority Refs

- approved design:
  `docs/aegis/specs/2026-07-30-dynamic-workflow-ai-reliability-design.md`;
- current decision: `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`;
- landed baseline:
  `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md`;
- runtime baseline:
  `docs/aegis/baseline/2026-07-26-workflow-platform-contract.md`;
- user approval in the current task.

## Compatibility Boundary

- preserve strict parsing of exactly one JSON value;
- preserve exactly one tools-disabled repair turn;
- preserve saved workflows, sessions, journals, task identifiers, and
  historical records without migration;
- preserve `output_schema` as required;
- preserve Delegate, DelegateGroup, and DelegateSwarm card layout, content,
  expansion, ordering, and activity;
- preserve malformed-detail and non-terminal-result rejection;
- remove the completed-plus-error rejection and omitted inline input-schema
  default without compatibility branches.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: post-change focused regression
- Reason: the user did not request strict test-first work and the production log
  already provides the diagnostic reproduction.
- Verification: one exact package, one exact target selector, and one focused
  test filter for each behavior slice.

## Verification

Focused checks must prove:

1. initial and repair child requests carry the exact strict response format;
2. both child prompts include the actual schema and identical JSON-only rules;
3. strict host parsing still rejects fences, prose, and multiple values;
4. completed child lifecycle plus schema failure maps to failed workflow result
   while preserving the original error, usage, and child references;
5. all inline definition actions require both schemas;
6. tool and skill guidance expose one schema-first path;
7. every required built-in child failure terminates without placeholder success;
8. the outer failed Delegate display distinguishes lifecycle from result while
   Delegate-family cards remain unchanged;
9. one configured live-provider run succeeds without repair or secondary error.

## Scope Check

### Aegis Visibility

One visible failure crosses provider request projection, validation, workflow
status mapping, authoring guidance, built-ins, and transcript projection. The
plan keeps each edit with its existing owner.

### Baseline Usage Draft

- Required baseline refs: approved design, ADR-0008, landed workflow baseline,
  and runtime baseline.
- Acknowledged before plan refs: all required refs.
- Cited in plan refs: all required refs.
- Missing refs: none.
- Decision: `continue`.

### Requirement Ready Check

- Requirement source refs: approved design and current user approval.
- Goals and scope refs: design Goals, Non-Goals, and Acceptance Criteria.
- User / scenario refs: usability report and captured workflow journal.
- Acceptance refs: design Verification and Acceptance Criteria.
- Open blocker questions: none.
- Decision: `ready`.

### Change Necessity

- User-visible need: useful child work becomes a misleading failed Workflow
  result, and the model lacks the strongest available schema guidance.
- No-change option: prompt edits alone cannot attach provider response format,
  preserve typed failure data, or stop built-in placeholder success.
- Minimum boundary: existing request, Delegate, workflow tool/runtime, built-in,
  and outer transcript owners.
- Decision: `code-change`.

### Existence And Architecture Check

- New product surface: none.
- Reuse: `AgentConfig`, `chat_request`, `attach_response_format_hint`,
  `MultiAgentRuntime`, `WorkflowTool`, `WorkflowDispatchResolver`, built-ins,
  and tool-result renderer.
- Invariant: provider hints improve accuracy; strict host validation decides
  acceptance.
- Ownership: lifecycle in `MultiAgentRuntime`, tool result in `Delegate`,
  invocation result in `WorkflowDispatchResolver`, persistence in
  `WorkflowRuntime`.
- Retirement: no active completed-plus-error guard, optional inline
  `input_schema`, or placeholder-success path remains.
- Decision: `reuse-existing`; proceed.

### Complexity Check

- Current pressure: `runtime.rs` and `workflow.rs` are large but already own the
  behavior.
- Add-in-place risk: moderate only if duplicate helpers or prompt text are added.
- Better boundary: one turn request field and one shared child guidance helper;
  no new module.
- Recommendation: `edit-in-place`.

## File Map

- `crates/neo-agent-core/src/runtime/config.rs`: turn response format.
- `crates/neo-agent-core/src/runtime/chat_request.rs`: request projection.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`: initial/repair wiring and
  child guidance.
- `crates/neo-agent-core/src/tools/delegate.rs`: schema error content.
- `crates/neo-agent-core/src/runtime/workflow_dispatch.rs`: result mapping.
- `crates/neo-agent-core/src/tools/workflow.rs`: explicit schemas and checklist.
- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`: authoring guidance.
- three Lua files under `crates/neo-agent-core/src/workflow/builtins/`:
  fail required outcomes and remove placeholders.
- `crates/neo-tui/src/transcript/tool_renderers.rs`: typed outer failure body.
- existing focused test files named below.
- ADR-0008 and the landed model-visible-results baseline: final decision sync.

Do not modify provider adapters unless a focused test proves their existing
`ResponseFormat` mapping is broken.

## Execution Readiness View

- Intent Lock: implement the approved reliability design, including model guidance.
- Scope Fence: only the file map and evidence-driven adjacent tests.
- Baseline Lock: approved design and listed workflow baselines.
- Owner Constraints: no second runtime, state model, parser, or provider path.
- Compatibility Boundary: stored data and Delegate-family cards unchanged.
- Retirement Boundary: delete wrong rejection, omitted-schema, contradictory
  guidance, and placeholder-success paths.
- Task Batches: six tasks below, in order.
- Drift Rule: if a change needs persistence migration, another retry, alias,
  parser mode, provider-specific branch, or card redesign, stop and report.
- Completion Evidence: focused tests, `git diff --check`, commits, live result
  or exact blocker, and residual risk.

## Task 1: Attach Strict Response Format And Strong Child Guidance

### Files

- `crates/neo-agent-core/src/runtime/config.rs`
- `crates/neo-agent-core/src/runtime/chat_request.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/tests/workflow_schema.rs`

### Steps

1. Add a skipped/non-user-config `Option<ResponseFormat>` to `AgentConfig`,
   initialize it to `None`, and copy it into `RequestOptions`.
2. In `MultiAgentRuntime`, use one private helper and existing
   `attach_response_format_hint` to attach stable name `child_output`, exact
   schema, and `strict = true`.
3. Apply it to ordinary Delegate children, swarm children, and the tools-disabled
   repair turn. Children without schemas retain `None`.
4. Change `child_prompt` to accept the schema. Serialize it compactly beside
   these rules: exactly one JSON value, no fence or prose, every required field,
   and no formatting tool call.
5. Keep `schema_repair_correction_prompt` aligned while including the validation
   error and exact schema.
6. Extend `child_schema_invalid_output_gets_exactly_one_tools_disabled_repair`
   to assert both captured requests have identical response format/schema and
   both prompts carry the rules. Keep strict-parser assertions.

### Verification

```bash
cargo nextest run -p neo-agent-core --test workflow_schema child_schema_invalid_output_gets_exactly_one_tools_disabled_repair
cargo nextest run -p neo-agent-core --lib structured_child_prompt_requires_json_only_output
```

### Commit

```bash
git add crates/neo-agent-core/src/runtime/config.rs crates/neo-agent-core/src/runtime/chat_request.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/tests/workflow_schema.rs
git commit -m "fix(workflow): constrain child model responses"
```

## Task 2: Preserve Truthful Delegate And Workflow Failure Semantics

### Files

- `crates/neo-agent-core/src/tools/delegate.rs`
- `crates/neo-agent-core/src/runtime/workflow_dispatch.rs`
- `crates/neo-agent-core/tests/workflow_dispatch.rs`

### Repair And Retirement

- Schema failure uses `schema_error` as `ToolResult.content`.
- Completed lifecycle plus `is_error` maps to failed invocation while preserving
  details, usage, and child references.
- Delete both `error result cannot be completed` guards and old test premise.
- Preserve cancellation, interruption, resource limits, and malformed details.

### Steps

1. Make Delegate content prefer `schema_error` on schema failure,
   `structured_output` on success, and ordinary content otherwise.
2. In canonical Delegate and DelegateSwarm mapping, convert only
   `result_is_error && lifecycle_status == Completed` to workflow `Failed`.
3. Replace `child_error_result_cannot_claim_completed_status` with
   `completed_delegate_with_schema_error_maps_to_failed_and_preserves_correlation`.
   Assert original summary, no `workflow_outcome_error`, usage, and child ref.
4. Add `completed_swarm_error_maps_to_failed` as a narrow status assertion.
5. Add a private Delegate unit assertion for schema-error content without
   duplicating an end-to-end fixture.

### Verification

```bash
cargo nextest run -p neo-agent-core --test workflow_dispatch completed_delegate_with_schema_error_maps_to_failed_and_preserves_correlation
cargo nextest run -p neo-agent-core --test workflow_dispatch completed_swarm_error_maps_to_failed
cargo nextest run -p neo-agent-core --lib delegate_schema_error_content_uses_validation_reason
```

### Commit

```bash
git add crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/runtime/workflow_dispatch.rs crates/neo-agent-core/tests/workflow_dispatch.rs
git commit -m "fix(workflow): preserve failed child result semantics"
```

## Task 3: Require Explicit Schemas And Consolidate Model Guidance

### Files

- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent-core/src/tools/workflow_tests.rs`
- `crates/neo-agent-core/tests/workflow_tool_policy.rs`
- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- `crates/neo-agent-core/src/skills/builtin/mod.rs` only for assertions

### Steps

1. Require `input_schema` in `inline_definition`, like `output_schema`.
2. Move `input_schema` from optional to required for `validate_inline`, `save`,
   and `run_inline` in `expected_shape` and generated branches.
3. Keep stored-definition input schema optional for historical files. Do not migrate.
4. Put a short checklist at the Workflow description opening: explicit action,
   both schemas for inline actions, `run_saved` for known definitions, and
   `TaskOutput` for workflow task IDs.
5. Replace repeated skill guidance with the approved eight-step checklist.
6. Include one no-argument schema
   `{"type":"object","additionalProperties":false}` and one required-argument
   example. Delete contradictory text instead of appending.
7. Update schema/error/policy/embedded-skill tests. Add
   `inline_actions_reject_missing_input_schema_with_expected_shape`.

### Verification

```bash
cargo nextest run -p neo-agent-core --lib workflow_schema_declares_action_specific_required_fields
cargo nextest run -p neo-agent-core --lib inline_actions_reject_missing_input_schema_with_expected_shape
cargo nextest run -p neo-agent-core --test workflow_tool_policy workflow_tool_is_root_only_and_description_has_no_choreography
cargo nextest run -p neo-agent-core --lib create_workflow_builtin_teaches_authoring_without_mandatory_choreography
```

If an existing policy/skill test name differs, resolve the exact name with
targeted `rg`, keep the same package/target selector, and record it.

### Commit

```bash
git add crates/neo-agent-core/src/tools/workflow.rs crates/neo-agent-core/src/tools/workflow_tests.rs crates/neo-agent-core/tests/workflow_tool_policy.rs crates/neo-agent-core/src/skills/builtin/create-workflow.md crates/neo-agent-core/src/skills/builtin/mod.rs
git commit -m "fix(workflow): guide explicit schema authoring"
```

## Task 4: Make Built-In Workflows Fail Closed

### Files

- three Lua files under `crates/neo-agent-core/src/workflow/builtins/`
- `crates/neo-agent-core/tests/workflow_builtins.rs`

### Steps

1. Check every required host outcome immediately. Call `neo.fail` with its
   preserved summary when `ok = false`.
2. Replace ignored `neo.verify` calls and remove `pcall` blocks hiding unusable
   required results.
3. Delete code-review's synthetic non-empty finding and deep-research's
   summary/fallback findings. Valid empty findings remain an empty JSON array.
4. Deep-research also fails when its schema-valid verification result reports
   `ok = false`.
5. Keep large-refactor's human merge/retirement choice unchanged.
6. Add `required_child_failure_aborts_builtin_without_placeholder_result`.
   Retain source/fixture checks for the other built-ins.

### Verification

```bash
cargo nextest run -p neo-agent-core --test workflow_builtins required_child_failure_aborts_builtin_without_placeholder_result
cargo nextest run -p neo-agent-core --test workflow_builtins all_builtin_definitions_validate_through_public_registry
```

### Commit

```bash
git add crates/neo-agent-core/src/workflow/builtins/code-review.lua crates/neo-agent-core/src/workflow/builtins/deep-research.lua crates/neo-agent-core/src/workflow/builtins/large-refactor.lua crates/neo-agent-core/tests/workflow_builtins.rs
git commit -m "fix(workflow): fail closed on required child errors"
```

## Task 5: Clarify The Outer Failed Delegate Presentation

### Files

- `crates/neo-tui/src/transcript/tool_renderers.rs`
- `crates/neo-tui/tests/multi_agent_transcript.rs`

### Steps

1. Add one typed renderer before the generic result body. Activate only for a
   failed Delegate-family result carrying schema error details and completed lifecycle.
2. Render completed agent lifecycle, failed requested result, and plain
   format-mismatch reason. Expanded content may include precise `schema_error`.
3. Do not parse text and do not edit Delegate-family card, grouping, activity,
   absorption, or expansion code.
4. Add
   `option_b_delegate_absorption_distinguishes_completed_agent_from_failed_schema_result`.
   Assert the facts and absence of bare generic `status: completed`.
5. Keep existing Delegate card assertions unchanged.

### Verification

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_delegate_absorption_distinguishes_completed_agent_from_failed_schema_result
```

### Commit

```bash
git add crates/neo-tui/src/transcript/tool_renderers.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "fix(tui): clarify failed delegate result status"
```

## Task 6: Final Verification, Live Acceptance, And Decision Sync

### Files

- `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md`

### Steps

1. Run every exact test from Tasks 1-5. Do not replace them with broad tests.
2. Run `rustfmt --check --edition 2024` on each touched Rust file and
   `git diff --check`.
3. Build once:

```bash
cargo build -p neo-agent --bin neo
```

4. Create a temporary workspace with `mktemp -d`. Use it as the execution tool's
   typed working directory and run:

```bash
/Users/chenyuanhao/Workspace/neo/target/debug/neo --yolo run --output json '<intent>'
```

Use this intent:

```text
Create a project workflow named schema-smoke with one Delegate child. It accepts
an explicit object with required string field text. The child returns an object
with required string field echoed and no other fields. Save it, run the saved
workflow with text set to hello, use TaskOutput until terminal, and return only
the final structured Workflow result.
```

Do not modify existing user workflows. Leave the temporary workspace path in
evidence instead of recursively deleting it.
5. Verify first-turn schema acceptance, useful echoed output, no repair event,
   no `workflow_outcome_error`, and no placeholder. If provider access is
   unavailable, report the blocker and do not claim live acceptance.
6. Amend ADR-0008 and the landed baseline with explicit inline schemas,
   response-format wiring, two-layer status, retired error guard, and unchanged
   card boundary. Do not create another ADR or baseline.
7. Confirm retirement:

```bash
rg -n "error result cannot be completed|omit it to accept no arguments|child summary|evidence = \"fallback\"|no structured findings emitted by children" crates/neo-agent-core crates/neo-tui
```

Expected: no active production hit.

### Commit

```bash
git add docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md
git commit -m "docs(workflow): record reliable structured child results"
```

## Completion Report

Return commits in order, exact test results, live evidence or blocker, changed
files by owner, retirement-search proof, unchanged Delegate-family card files,
preserved unrelated changes, and residual provider/platform risk.
