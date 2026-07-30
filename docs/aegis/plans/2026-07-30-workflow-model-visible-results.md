# Neo Workflow Model-Visible Results Implementation Plan

Date: `2026-07-30`

Status: `approved design; ready for implementation`

## Goal

Make the existing `Workflow` and `TaskOutput` tools usable by a limited model on
the first attempt: action inputs describe their true required fields, every next
decision can be made from `ToolResult.content`, empty-input workflows need no
schema boilerplate, and ordinary host-operation failures return script-visible
outcomes without terminating the run.

## Architecture

Keep every landed owner and repair only its projection or local behavior:

```text
WorkflowTool
  -> WorkflowDefinitionRegistry / WorkflowLaunchCoordinator
  -> WorkflowRuntime -> LuaWorkflowRunner
  -> workflow output page -> TaskOutput
  -> ToolResult.content -> next model request
```

- `WorkflowTool` owns action-specific input and result JSON.
- workflow definition resolution owns the no-argument input schema default.
- `LuaWorkflowRunner` owns script-visible host outcomes.
- workflow output owns bounded `TaskOutput` JSON.
- the turn loop remains unchanged and continues to append only
  `ToolResult.content` to the next model request.

No new tool, result channel, registry, runtime, wait path, setting, retry, alias,
or fuzzy name matching is introduced.

## Tech Stack

- Rust 2024, minimum Rust `1.96.1`;
- existing `serde`, `serde_json`, `schemars`, `jsonschema`, `mlua`, `tokio`;
- existing `FakeModelClient`, workflow runtime, journal, and task adapters;
- no new dependency.

## Baseline And Authority Refs

- approved design:
  `docs/aegis/specs/2026-07-30-workflow-model-visible-results-design.md`;
- current product decision:
  `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`;
- current landed baseline:
  `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`;
- user approval in the current task;
- current source owners in `tools/workflow.rs`, `workflow/definition.rs`,
  `workflow/lua.rs`, `workflow/output.rs`, and `runtime/turn_loop.rs`.

## Compatibility Boundary

- preserve the seven `Workflow` action names and the existing `TaskOutput` views;
- preserve typed `details` for current UI and event consumers;
- preserve journal, task, artifact, replay, completion, and TUI ownership;
- preserve `output_schema` as required and keep final result validation;
- preserve `WaitDelegate` as delegate/swarm-only;
- preserve terminal behavior for `neo.fail`, uncaught script errors, resource
  exhaustion, cancellation, and invalid final results;
- remove the old summary-only model text without a fallback;
- omit `input_schema` only means strict no-argument input, never unchecked input.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: post-change focused regressions plus one production-chain acceptance
- Reason: neither the user nor project requested strict test-first development;
  current tests and the usability report already provide diagnostic evidence.
- Verification: one exact package/target/test command for every behavior slice.

## Verification

Focused local evidence must prove:

1. action-discriminated Workflow schema and exact field matrix;
2. no-argument default and strict rejection of non-empty arguments;
3. model-visible Workflow list/show/error/launch JSON;
4. model-visible TaskOutput result/journal/pending data under the complete byte cap;
5. failed outcomes for verify, command verification, unknown tools, and forbidden tools;
6. terminal `neo.fail` behavior;
7. the production Workflow launch and next-model message path;
8. removal of the old summary formatter and failure-raising wrappers.

Local focused tests do not prove provider-backed live execution or native Windows
behavior. Those remain release-level residual checks.

## Scope Check

### Aegis Visibility

Planning is useful because the model-visible source, typed UI projection, script
failure boundary, and production runtime chain cross several owners while the
old summary path must be retired rather than retained.

### Baseline Usage Draft

- Required baseline refs: approved design, ADR-0008, landed 2026-07-28 baseline.
- Delivered context refs: all required refs and current source owners.
- Acknowledged before plan refs: all required refs.
- Cited in plan refs: all required refs.
- Missing refs: none.
- Decision: `continue`.

### Requirement Ready Check

- Requirement source refs: approved design sections 3-16.
- Goals and scope refs: design sections 1, 4, 13, and 14.
- User / scenario refs: usability report and current user approval.
- Requirement item refs: model JSON, strict no-input default, failure classes,
  production parity, unchanged wait boundary.
- Acceptance / verification criteria refs: design section 15.
- Open blocker questions: none.
- Decision: `ready`.

### Change Necessity

- User-visible need: the next model must receive real Workflow and TaskOutput data
  and scripts must branch on ordinary failed outcomes.
- No-change / non-code option: documentation cannot make hidden `details` fields
  enter model context or alter Lua behavior.
- Why code change is necessary: current code serializes human summaries into
  `content`, requires `input_schema`, and raises failed verify outcomes.
- Minimum change boundary: the four existing owners and focused tests listed in
  the file map.
- Decision: `code-change`.

### Existence Check

- Proposed new product surface: none.
- Existing owner / reuse candidate: `WorkflowTool`, definition resolution,
  `LuaWorkflowRunner`, workflow output, and the turn loop.
- Why existing surface is insufficient: behavior inside these owners is wrong;
  the owners themselves are sufficient.
- Creation proof: only one isolated production acceptance test file may be added
  because no current test crosses launch, TaskOutput, and next-model projection.
- Entropy / retirement impact: delete two Lua wrappers and one TaskOutput summary
  formatter; do not add production abstractions.
- Decision: `reuse-existing`.

### Architecture Integrity Lens

- Invariant: every next-decision field is in `content`; richer UI data may remain
  in `details`.
- Canonical owner: action projection in `WorkflowTool`, bounded page projection in
  workflow output, host outcome semantics in `LuaWorkflowRunner`.
- Responsibility overlap: none added; `details` remains presentation data.
- Higher-level simplification: serialize the existing structured values instead
  of copying fields into a second model type.
- Retirement / falsifier: zero active references to the summary formatter or
  failure-raising verify wrappers.
- Verdict: proceed.

### Anti-Entropy Declaration

- Deletion class: internal code retirement.
- Old path: summary-only Workflow/TaskOutput model text and verify wrappers that
  turn failed outcomes back into Lua errors.
- New canonical owner: compact JSON serialized from existing structured results.
- Expected preserved behavior: typed UI details, byte caps, terminal failures.
- Expected retired behavior: hidden actionable data and `pcall`-required ordinary
  failures.
- External boundary touched: yes, model-visible tool schema and results.
- Source-of-truth data risk: none.
- User confirmation required: no.

Retirement decision: `delete-first`. There is no active external compatibility
evidence requiring both old and new model result shapes.

### Complexity Budget

- Artifact class: existing core owners and adapters.
- Target files / artifacts: four maintained source files, focused tests, skill,
  user docs, ADR, and landed baseline.
- Current pressure: `workflow.rs` and `output.rs` are large but already own the
  affected projections.
- Projected post-change pressure: small; deletion offsets most helper growth.
- Budget result: `within-budget`.
- Planned governance: edit in place, reuse existing JSON values, add no new
  production module.

### Plan-Time Complexity Check

- Owner fit: strong for all four source files.
- Add-in-place risk: low because changes replace existing functions or defaults.
- Better file boundary: only the end-to-end acceptance test deserves a separate
  integration target.
- Recommendation: edit in place; add one acceptance test file only if existing
  harness reuse cannot express the full chain in a current target.

### Plan Pressure Test

- Owner / retirement: fixed and delete-first.
- Architecture integrity / higher-level path: existing structured result is the
  higher-level path.
- Verification scope: exact owner tests plus one production-chain test.
- Task executability: each task names files, symbols, expected assertions, and
  commands.
- Pressure result: `proceed`.

## File Map

| File | Change |
| --- | --- |
| `crates/neo-agent-core/src/tools/workflow.rs` | action-discriminated schema; compact success/error JSON in `content`; remove summary input from `result` |
| `crates/neo-agent-core/src/tools/workflow_tests.rs` | exact schema, list/show/error/launch content regressions |
| `crates/neo-agent-core/src/workflow/definition.rs` | normalize absent input schema to a strict empty object schema |
| `crates/neo-agent-core/tests/workflow_registry.rs` | definition-level no-argument normalization regression |
| `crates/neo-agent-core/src/workflow/output.rs` | compact page JSON in `content`; delete `format_page_content`; retain whole-result byte accounting and shrinking |
| `crates/neo-agent-core/tests/workflow_output.rs` | result and journal payload plus byte-cap regressions |
| `crates/neo-agent-core/tests/workflow_user_input.rs` | actionable pending request remains present in model JSON |
| `crates/neo-agent-core/src/workflow/lua.rs` | return failed host outcomes directly; durably return unknown/forbidden tool failures; keep malformed input catchable and `neo.fail` terminal |
| `crates/neo-agent-core/tests/workflow_lua.rs` | outcome-value and terminal-error regressions |
| `crates/neo-agent-core/tests/workflow_model_visibility.rs` | production launch, task completion, TaskOutput, and next-model projection acceptance |
| `crates/neo-agent-core/src/skills/builtin/create-workflow.md` | teach no-input default, direct outcome branching, actual TaskOutput payloads |
| `docs/workflows.md` | update user-facing authoring/result behavior without internal details |
| `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md` | record model-visible result and host-failure decisions |
| `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md` | landed behavior and exact evidence |
| `docs/aegis/INDEX.md` | index plan, amended decision, and landed baseline |

Unrelated interactive input and terminal files remain untouched.

## Task 1: Make Workflow Inputs First-Attempt Correct

**Files:** modify `tools/workflow.rs`, `workflow/definition.rs`,
`tools/workflow_tests.rs`, and `tests/workflow_registry.rs`.

**Why:** the model schema currently advertises one flat shape while execution
uses seven different required-field sets, and empty-input workflows require
unnecessary boilerplate.

**Change Necessity:** documentation cannot alter provider-visible JSON Schema or
definition validation. The minimum boundary is the existing tool schema and
definition resolver.

**Impact/Compatibility:** keep one `Workflow` tool and all seven action names.
Reject missing `output_schema` before side effects. Missing `input_schema`
normalizes to exactly:

```rust
serde_json::json!({
    "type": "object",
    "additionalProperties": false,
})
```

**Steps:**

1. Replace the derived flat `WorkflowInput` schema returned by
   `WorkflowTool::input_schema` with a hand-built `oneOf` whose branches use a
   `const` action and the exact `expected_shape` required/optional lists. Keep
   `additionalProperties: false` in every branch and keep object types for
   `args`, `input_schema`, and `output_schema`.
2. Change `inline_definition` so `input_schema` is copied as `Option<Value>`;
   keep `output_schema` required through `required_object`.
3. In `resolve_dynamic_definition`, replace the `None => None` branch with the
   strict empty object schema, validate its byte limit, and compile it through
   the existing `finish_resolved` path.
4. Update `workflow_schema_is_flat_and_exact` into
   `workflow_schema_declares_action_specific_required_fields`; assert all seven
   action branches and especially required `output_schema` with optional
   `input_schema` for inline actions.
5. Add one registry regression proving `{}` validates when the input schema was
   omitted and `{"unexpected": true}` fails against the normalized schema.
6. Run:

```bash
rtk cargo test --package neo-agent-core --lib -- tools::workflow_tests::workflow_schema_declares_action_specific_required_fields --exact --nocapture
rtk cargo test --package neo-agent-core --test workflow_registry -- omitted_input_schema_is_strictly_no_arguments --exact --nocapture
```

7. Commit only this task:

```bash
rtk git add crates/neo-agent-core/src/tools/workflow.rs crates/neo-agent-core/src/tools/workflow_tests.rs crates/neo-agent-core/src/workflow/definition.rs crates/neo-agent-core/tests/workflow_registry.rs
rtk git commit -m "fix(workflow): make action inputs explicit"
```

## Task 2: Put Workflow Action Data In Model Content

**Files:** modify `tools/workflow.rs` and `tools/workflow_tests.rs`.

**Why:** list, show, validation errors, and launch results currently hide the
data the next model turn needs in `details`.

**Change Necessity:** only `WorkflowTool` can select safe action-specific fields;
globally appending all details would expose internal identifiers and waste
tokens.

**Impact/Compatibility:** `details` remains the same typed object for the UI.
The exact same compact object is serialized into `content`; null top-level
fields are omitted. Internal hashes, origins, and paths remain absent from the
model `workflow` object.

**Repair Track:**

- root cause: `result` accepts caller-written human text and stores the real
  object only in `details`;
- repair: build one `serde_json::Value`, serialize it with
  `serde_json::to_string`, and attach that same value as details;
- verification: parse `result.content` and assert action-specific data.

**Retirement Track:** delete the `content` parameter from `result` and every
summary-only caller string. Do not keep both shapes.

**Steps:**

1. Change `result` to accept only action, status, and `WorkflowResultDetails`.
   Build one object, remove null optional fields, serialize compactly, and use
   it for both `ToolResult::ok(content)` and `.with_details(value)`.
2. Apply the same rule to `workflow_error_result_with_context`: one compact error
   object in both `content` and `details`, including `field`, `expected_shape`,
   `side_effect_occurred`, and `next_actions`.
3. Keep `workflow_details(include_source)` as the safety filter: source only for
   `show` and explicit inline validation; no hash, origin, or storage path.
4. Update existing saved-action and run-inline tests to parse `content`. Add
   focused assertions for list entries/cursor/total, show definition/schema,
   error expected shape, and exact TaskOutput next action.
5. Run:

```bash
rtk cargo test --package neo-agent-core --lib -- tools::workflow_tests::saved_actions_list_show_validate_run_and_recover_from_conflict --exact --nocapture
rtk cargo test --package neo-agent-core --lib -- tools::workflow_tests::run_inline_returns_registered_task_and_task_output_next_action --exact --nocapture
rtk cargo test --package neo-agent-core --lib -- tools::workflow_tests::action_matrix_rejects_saved_inline_mixtures_without_side_effects --exact --nocapture
```

6. Commit only this task:

```bash
rtk git add crates/neo-agent-core/src/tools/workflow.rs crates/neo-agent-core/src/tools/workflow_tests.rs
rtk git commit -m "fix(workflow): expose action results to models"
```

## Task 3: Put Bounded TaskOutput Pages In Model Content

**Files:** modify `workflow/output.rs`, `tests/workflow_output.rs`, and
`tests/workflow_user_input.rs`.

**Why:** result and journal views currently disclose only counts, so the model
cannot verify output or continue paging.

**Change Necessity:** the existing page already contains the correct bounded
data. The minimum fix is its serializer and byte-accounting loop.

**Impact/Compatibility:** keep every view, cursor, page shrink rule, artifact
range read, and complete-result cap. Count both compact `content` and typed
`details`, even though they contain the same selected page fields.

**Repair Track:** serialize `TaskOutputPage` compactly into `content`; reuse the
existing details value and shrink trailing journal/artifact entries until the
complete result fits.

**Retirement Track:** delete `format_page_content` and its `std::fmt::Write`
dependency. No summary fallback remains.

**Steps:**

1. Add a small private serializer that returns `(String, Value)` from one
   `TaskOutputPage`; use `serde_json::to_string` and `serde_json::to_value` with
   the existing typed error mapping.
2. Reuse that serializer in `page_to_tool_result` and
   `shrink_page_to_tool_result_cap`; preserve `measure_tool_result_bytes`.
3. Delete `format_page_content` completely.
4. Extend `multi_gigabyte_logical_journal_pages_under_tool_result_cap` to parse
   every content page, assert journal records and cursors match details, and
   retain the whole-result byte cap assertion.
5. Add a result-page assertion that the actual final JSON is present in parsed
   content. Update the pending-input test to assert request id, prompt, schema,
   and next action from parsed content.
6. Run:

```bash
rtk cargo test --package neo-agent-core --test workflow_output -- multi_gigabyte_logical_journal_pages_under_tool_result_cap --exact --nocapture
rtk cargo test --package neo-agent-core --test workflow_output -- result_content_contains_actual_final_json --exact --nocapture
rtk cargo test --package neo-agent-core --test workflow_user_input -- task_output_exposes_actionable_pending_request_without_journal_view --exact --nocapture
```

7. Commit only this task:

```bash
rtk git add crates/neo-agent-core/src/workflow/output.rs crates/neo-agent-core/tests/workflow_output.rs crates/neo-agent-core/tests/workflow_user_input.rs
rtk git commit -m "fix(workflow): expose bounded task output pages"
```

## Task 4: Return Ordinary Host Failures As Outcomes

**Files:** modify `workflow/lua.rs` and `tests/workflow_lua.rs`.

**Why:** ordinary failed operations should be branchable data, not errors that
require `pcall` or can terminate a run.

**Change Necessity:** the host already constructs immutable
`WorkflowInvocationOutcome` values. Two wrappers and two early error branches
undo that design.

**Impact/Compatibility:** malformed host input remains a catchable Lua error.
`neo.fail`, uncaught Lua errors, resource exhaustion, cancellation, and final
schema failure remain terminal.

**Repair Track:**

- remove `VERIFY_WRAPPER` and `VERIFY_COMMAND_WRAPPER`;
- register their host functions directly so both success and failure return the
  immutable outcome table;
- for unknown and forbidden tool names, call `invoke_local` with
  `WorkflowInvocationKind::Tool`, `failed_outcome`, and a stable details code,
  then return `immutable_outcome`;
- keep eligible tools on `handle.invoke` and the canonical dispatch resolver.

**Retirement Track:** delete the wrappers and tests that require `pcall` for
ordinary failure. Do not add a compatibility toggle.

**Steps:**

1. Delete both wrapper constants and `wrap_host_function` if no references
   remain.
2. Register `host_verify` and `host_verify_command` directly.
3. In `neo.tool`, perform structural input checks first. After a valid canonical
   input, allocate the invocation index once. Return a durable failed outcome
   with `details.code = "unknown_tool"` when the registry has no exact name, or
   `details.code = "tool_not_workflow_eligible"` for forbidden names. Dispatch
   eligible tools unchanged.
4. Rewrite the verify regression to assert `pcall` succeeds and the returned
   outcome has `ok=false`, `status="failed"`, unchanged summary/details, and
   immutable nested fields.
5. Rewrite the forbidden-tool regression without `pcall`; add one exact unknown
   tool case. Keep `neo_fail_is_terminal_even_when_pcall_catches_it` unchanged.
6. Run:

```bash
rtk cargo test --package neo-agent-core --test workflow_lua -- neo_verify_failure_returns_an_immutable_outcome --exact --nocapture
rtk cargo test --package neo-agent-core --test workflow_lua -- denied_neo_tool_returns_failed_outcome_without_aborting --exact --nocapture
rtk cargo test --package neo-agent-core --test workflow_lua -- unknown_neo_tool_returns_failed_outcome_without_aborting --exact --nocapture
rtk cargo test --package neo-agent-core --test workflow_lua -- neo_fail_is_terminal_even_when_pcall_catches_it --exact --nocapture
```

7. Commit only this task:

```bash
rtk git add crates/neo-agent-core/src/workflow/lua.rs crates/neo-agent-core/tests/workflow_lua.rs
rtk git commit -m "fix(workflow): return recoverable host outcomes"
```

## Task 5: Prove The Production Model Path

**Files:** create `tests/workflow_model_visibility.rs`; modify a production owner
only if the acceptance test exposes a concrete root cause.

**Why:** direct runner tests cannot prove the launched runtime and next model
request receive the same failure and result semantics.

**Change Necessity:** this is the one justified new test surface because the
existing tests stop at separate owners.

**Impact/Compatibility:** use only the real registry, launch coordinator,
workflow runtime, dispatch resolver, `TaskOutput`, turn loop, and fake model.
No test-only production branch.

**Steps:**

1. Build a temporary session/workspace fixture with `FakeModelClient`,
   `ToolRegistry::with_builtin_tools`, and one shared
   `WorkflowDispatchResolver` installed in the agent configuration used by both
   the root turn and workflow runner.
2. First fake model turn calls `Workflow(run_inline)` with a script that records
   failed `neo.verify(false)`, unknown-tool, and forbidden-tool outcomes, then
   returns them as final JSON.
3. Wait through the real task completion path and issue
   `TaskOutput(view="result", block=true)` from the next fake model turn.
4. In the following recorded request, parse the Workflow launch tool result and
   TaskOutput tool result from message content. Assert the launch contains the
   task id and exact TaskOutput call; assert the actual final JSON contains all
   three failed outcomes and that the workflow terminal status is completed.
5. If this test fails before assertions, trace the concrete failure to its
   existing owner and fix only that owner. Do not add retries, aliases, registry
   refreshes, or a second projection.
6. Run:

```bash
rtk cargo test --package neo-agent-core --test workflow_model_visibility -- production_workflow_result_reaches_the_next_model_request --exact --nocapture
```

7. Commit only this task:

```bash
rtk git add crates/neo-agent-core/tests/workflow_model_visibility.rs crates/neo-agent-core/src
rtk git commit -m "test(workflow): prove model-visible production results"
```

Before committing, stage only an additional source file when the failing
acceptance test proved that exact file required a repair.

## Task 6: Update Authoring Guidance And Architecture Records

**Files:** modify `skills/builtin/create-workflow.md`, `docs/workflows.md`,
ADR-0008, and INDEX; create the landed baseline.

**Why:** model and user guidance must teach the single landed behavior, and the
architecture record must preserve the changed model boundary and failure
classification.

**Change Necessity:** code alone leaves the built-in model guidance and current
decision record stale.

**Impact/Compatibility:** describe user goals and observable results. Do not
expose hashes, storage paths, internal scopes, or implementation fingerprints.

**Steps:**

1. Update `create-workflow.md`: omitted input schema means no arguments;
   `output_schema` is always required; branch directly on `outcome.ok`; use
   `TaskOutput` for actual result/journal/artifact data; never use
   `WaitDelegate` for workflow ids.
2. Update `docs/workflows.md` with the same behavior in novice-facing language.
3. Amend ADR-0008 with one decision section for compact model-visible action
   JSON and one for ordinary failed host outcomes versus terminal failures.
4. Create `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md`
   containing the landed commit ids, exact focused test evidence, preserved
   boundaries, and residual live/provider/platform risk.
5. Update `docs/aegis/INDEX.md` for this plan and baseline. Do not create a new
   ADR; amend the current Workflow product decision.
6. Run documentation checks:

```bash
rtk rg -n "pcall.*neo\.(verify|verify_command)|input_schema.*required|has_result:|records:" crates/neo-agent-core/src/skills/builtin/create-workflow.md docs/workflows.md crates/neo-agent-core/src/workflow crates/neo-agent-core/src/tools/workflow.rs
rtk git diff --check
```

Expected result: no active guidance says `pcall` is required for ordinary failed
outcomes, no summary-only formatter remains, and `git diff --check` is clean.

7. Commit only this task:

```bash
rtk git add crates/neo-agent-core/src/skills/builtin/create-workflow.md docs/workflows.md docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md docs/aegis/INDEX.md
rtk git commit -m "docs(workflow): record model-visible results"
```

## Final Scoped Verification

Run only the exact focused tests already named in Tasks 1-5, then:

```bash
rtk rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/workflow.rs crates/neo-agent-core/src/tools/workflow_tests.rs crates/neo-agent-core/src/workflow/definition.rs crates/neo-agent-core/src/workflow/output.rs crates/neo-agent-core/src/workflow/lua.rs crates/neo-agent-core/tests/workflow_registry.rs crates/neo-agent-core/tests/workflow_output.rs crates/neo-agent-core/tests/workflow_user_input.rs crates/neo-agent-core/tests/workflow_lua.rs crates/neo-agent-core/tests/workflow_model_visibility.rs
rtk git diff --check
rtk rg -n "VERIFY_WRAPPER|VERIFY_COMMAND_WRAPPER|format_page_content" crates/neo-agent-core/src
```

Expected results:

- all exact tests pass;
- formatting and whitespace checks pass;
- retirement search returns no active source hits;
- unrelated interactive input and terminal changes remain unstaged.

## Risks And Stop Conditions

- If a provider adapter rewrites or rejects the action branches, stop and fix the
  existing adapter only after an exact provider-schema test proves the issue.
- If doubling compact page data across `content` and `details` makes even a
  single bounded record impossible, preserve the whole-result cap and return the
  existing resource error; do not silently claim full delivery.
- If the production acceptance finds registry drift, prove the exact resolver
  lifecycle before changing it. The report alone does not authorize retries or
  aliases.
- Stop completion claims until the exact production-chain test and all retirement
  searches pass.

## Execution Readiness View

- Intent Lock: implement the approved model-visible results design only.
- Scope Fence: four source owners, focused tests, authoring docs, ADR amendment,
  and landed baseline; no TUI, Shell, Terminal, or delegate-card work.
- Baseline Lock: ADR-0008 and the 2026-07-28 landed baseline remain authoritative
  except where the approved design explicitly supersedes projection and failure
  semantics.
- Approved Behavior: compact JSON content, strict no-input default, required
  output schema, recoverable ordinary failures, unchanged terminal failures.
- Owner Constraints: reuse existing owners and the generic content-only turn-loop
  rule.
- Compatibility Boundary: seven actions and TaskOutput views remain; no fallback
  for old model summaries.
- Retirement Boundary: delete summary formatter and verify failure wrappers.
- Task Batches: inputs; Workflow projection; TaskOutput projection; host outcomes;
  production acceptance; docs and architecture records.
- Test Obligations: exact commands in each task.
- Review Gates: source review after Tasks 1-4; architecture and evidence review
  before Task 6 completion.
- Drift / Rewind Rules: evidence against a named existing owner may narrow or
  correct that task; any new product surface returns to design.
- Evidence Required Before Completion: exact tests, format check, diff check,
  retirement search, and clean staging boundary.
- Advisory Boundary: Aegis execution guidance only; not completion authority.

