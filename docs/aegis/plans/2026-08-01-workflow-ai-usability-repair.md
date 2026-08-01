# Neo Workflow AI Usability Repair Implementation Plan

Date: `2026-08-01`

Status: `approved design; ready for external implementation and final review`

## Implementer Directive

This plan is the execution path, not a request for another design pass. Read the
approved design, then execute Tasks 1-7 in order. Do not repeat the report-004
experiments, conduct a whole-repository survey, reopen the thirteen item
classifications, or substitute a capability setting, fallback, alias, second
repair loop, or broader provider redesign.

Before editing each task, use CodeGraph or `cx` only for the named symbols and
their direct callers. If a symbol moved, resolve its current name with a bounded
search and keep the same owner and behavior. If the approved behavior cannot be
implemented within the named files, stop and return the exact conflict; do not
silently widen scope.

Every task is independently reviewable. View and stage only its listed paths,
run its exact focused checks, commit it with the stated message, and preserve all
unrelated dirty paths. Task 6 is the sole authorized user-state edit and creates
no repository commit. After Task 7, stop and return the required evidence to the
original reviewer.

## Goal

Restore workflow Delegate and swarm usability on OpenAI-compatible endpoints,
preserve original child failures, make failure summaries actionable, repair the
known `echo-test` definition, and teach every intentional strict behavior
before a model authors a workflow.

## Architecture

Keep the existing request, child, validation, result, and guidance owners:

```text
create-workflow skill
  -> Workflow tool with explicit schemas
  -> MultiAgentRuntime child prompt + RequestOptions
  -> provider wire selects whether the optional native hint is expressible
  -> child runtime outcome
  -> successful turn only: strict JSON parse + host schema validation + one repair
  -> WorkflowRuntime bounded direct/swarm outcome
  -> TaskOutput model-visible result
```

`OpenAiCompatibleClient` owns the ambiguous `openai` wire body and must omit the
optional hint. `OpenAiResponsesClient` remains the known native owner.
`WorkflowRuntime` remains the only workflow journal/result owner. The existing
`create-workflow` skill remains the only detailed model-facing authoring owner.

## Tech Stack

- Rust 2024, minimum Rust `1.96.1`;
- existing `RequestOptions`, `ResponseFormat`, `ChildRunOutput`,
  `WorkflowInvocationOutcome`, `SchemaValidationError`, and `bounded_summary`;
- existing `serde_json::Value::pointer` and standard-library character
  iteration;
- existing built-in skill loader and focused nextest targets;
- no new dependency, setting, provider probe, parser, retry, or prompt surface.

## Baseline And Authority Refs

- approved design:
  `docs/aegis/specs/2026-08-01-workflow-ai-usability-repair-design.md`;
- prior reliability design, preserved except where selectively superseded:
  `docs/aegis/specs/2026-07-30-dynamic-workflow-ai-reliability-design.md`;
- current landed behavior:
  `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md`;
- current workflow platform boundary:
  `docs/aegis/baseline/2026-07-26-workflow-platform-contract.md`;
- current architecture decision:
  `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`;
- black-box evidence:
  `.tmp/workflow-ai-usability-report-004.md`;
- user approval on 2026-08-01, including stronger model guidance for preserved
  strict behaviors.

## Compatibility Boundary

- preserve internal `RequestOptions.response_format` on initial and repair
  child requests;
- preserve native `openai_response` mapping;
- omit the hint only from the ambiguous `openai` compatible wire;
- preserve exact child prompt schema, strict host validation, and one repair;
- preserve explicit inline input/output schemas and historical saved-definition
  readability;
- preserve Lua API shapes and terminal semantics;
- preserve TaskOutput as the workflow task read/wait path;
- preserve session, workflow, journal, artifact, and definition formats;
- preserve every Delegate-family card and transcript behavior;
- edit only the exact `echo-test` user definition outside the repository;
- do not push, switch branches, add worktrees, or modify unrelated files.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: post-change focused regression
- Reason: strict test-first work was not requested; report 004 and the traced
  request/result path already provide a deterministic diagnostic baseline.
- Verification: each Rust command names one package, one target selector, and
  one exact test-name filter.

## Verification

The implementation is not ready for final review until it has:

1. one exact provider serialization regression;
2. one exact native Responses serialization regression;
3. exact failed-child/no-repair regressions for both foreground Delegate and
   direct workflow swarm paths;
4. one exact swarm-summary regression;
5. one exact final-schema diagnostic regression;
6. one exact built-in skill guidance regression;
7. formatting checks for touched Rust files;
8. `git diff --check` after every task;
9. cross-provider live evidence or exact per-provider blockers;
10. one representative built-in workflow live run or exact blocker;
11. exact `echo-test` validation and successful run;
12. decision-record and landed-baseline sync;
13. a clean retirement search for removed compatible-wire mapping and forbidden
    fallback/config additions.

## Scope Check

### Aegis Visibility

Planning is required because the repair corrects an approved provider
assumption, changes shared child failure classification, touches model-facing
guidance, and includes one explicitly scoped user-state repair.

### Facts, Assumptions, And Unknowns

- Fact: report 004 recorded four endpoint families failing at zero tokens and
  zero tool calls with the same unsupported `response_format` error.
- Fact: `OpenAiCompatibleClient` currently serializes the hint whenever present.
- Fact: Anthropic and Google already omit the hint; OpenAI Responses maps it.
- Fact: initial and repair child prompts already include the exact schema and
  JSON-only rules.
- Fact: `accept_child_structured_output_with_repair` currently parses every
  child output without first requiring a successful child turn.
- Fact: workflow swarm summary currently contains only total and finished
  counts.
- Fact: `SchemaValidationError` already carries `instance_path`.
- Fact: the built-in skill already owns canonical workflow authoring guidance.
- Fact: `echo-test.lua` reads `args.text` while its paired manifest lacks an
  input schema.
- Assumption: all four report endpoints still use provider type `openai`; live
  acceptance must record the resolved provider type before each run.
- Unknown: current network and credential availability for each endpoint. A
  blocker limits live evidence but does not justify a source fallback.

### BaselineUsageDraft

- Required baseline refs: approved design, workflow model-visible-results
  baseline, workflow platform baseline, ADR-0008.
- Acknowledged before plan refs: all required refs.
- Cited in plan refs: all required refs.
- Missing refs: none.
- Decision: `continue`.

### Requirement Ready Check

- Requirement source refs: approved design and user approval.
- Goals and scope refs: design Goals, Non-Goals, Selected Runtime Design,
  Selected Model Guidance, User Definition Repair, and Acceptance Criteria.
- User/scenario refs: report-004 child failures and repeated AI confusion over
  intentional strict behavior.
- Acceptance refs: the twelve verification obligations above.
- Open blocker questions: none.
- Decision: `ready`.

### Ripple Signal Triage

- Removing compatible-wire serialization affects every current caller of
  `RequestOptions.response_format`, which is presently the child schema path.
- Failed-child gating has two consumers: foreground Delegate handling and direct
  workflow swarm handling.
- Swarm summary changes model-visible workflow content but not stored item
  details or TUI DelegateSwarm cards.
- Final schema diagnostics change terminal reason text but not error code or
  state.
- Skill guidance changes extracted built-ins and must preserve frontmatter and
  discovery behavior.
- Decision: verify each boundary separately; do not run broad workspace tests.

### Change Necessity

- User-visible need: child workflows must run on configured endpoints and fail
  with the real reason when the model turn does not run.
- No-change option: documentation cannot remove the rejected wire field, stop
  the second request, or restore the overwritten provider error.
- Why code change is necessary: the failures are deterministic request
  serialization and result-routing behavior.
- Minimum change boundary: compatible wire serializer, two existing child
  acceptance call sites, existing workflow summary, existing schema error
  mapper, and canonical guidance/docs.
- Decision: `code-change`.

### Existence Check

- Proposed new surface: none.
- Existing reuse candidates: provider wire mapping, child outcome, strict host
  validator, `bounded_summary`, built-in skill, language guides.
- Why existing surfaces are sufficient: each required behavior already has an
  owner; the defect is an incorrect assumption or missing guard/text.
- Entropy impact: delete one wire branch; add no runtime branch except
  successful-child preconditions and state-aware summary formatting.
- Decision: `reuse-existing`.

### Architecture Integrity Lens

- Invariant: provider wire failure and schema-invalid successful output are
  distinct failure classes.
- Canonical owners: provider serializer for wire fields, child outcome for turn
  success, schema validator for successful text, WorkflowRuntime for summaries,
  built-in skill for authoring guidance.
- Responsibility overlap: none required; do not add provider detection to
  WorkflowRuntime or prompt logic to the provider client.
- Higher-level simplification: deterministic omission on the ambiguous wire
  fixes all compatible endpoints at one shared owner.
- Retirement: remove the compatible-wire mapping test and branch completely.
- Verdict: `proceed`.

### Plan Pressure Test

- Owner and retirement: exact existing owners; one obsolete mapping removed.
- Higher-level path: shared compatible serializer fixes the endpoint class.
- Verification scope: one exact regression per behavior plus bounded live runs.
- Task executability: file paths, symbols, commands, and commits are fixed.
- Pressure result: `proceed`.

### Plan-Time Complexity Check

- Target files: an 846-line provider client, 1,422-line Delegate tool, 4,461-line
  WorkflowRuntime, 213-line schema module, focused integration tests, and the
  existing 439-line authoring skill.
- Current pressure: WorkflowRuntime and Delegate are large but already contain
  the exact affected functions; moving the behavior would create duplicate
  ownership.
- Projected pressure: a short early return at each child consumer, one summary
  formatter branch, and one bounded preview helper.
- Better boundary: keep each edit in its current owner; do not extract a new
  module or general capability framework.
- Recommendation: `edit-in-place`.

## Execution Readiness View

- Intent Lock: restore child execution and first-use clarity without weakening
  strict workflow semantics.
- Scope Fence: only the files named by Tasks 1-7 and the exact `echo-test` pair.
- Baseline Lock: approved 2026-08-01 design selectively supersedes only the
  earlier provider mapping assumption.
- Approved Behavior: deterministic wire omission, successful-turn-only schema
  validation, bounded actionable summaries, stronger canonical guidance.
- Owner Constraints: provider fields stay in provider clients; workflow state
  stays in WorkflowRuntime; authoring guidance stays in `create-workflow`.
- Compatibility Boundary: no schema, persistence, Lua API, task, or
  Delegate-family presentation change.
- Retirement Boundary: delete the compatible-wire mapping branch/test; add no
  switch, retry, alias, or duplicate prompt.
- Task Batches: Tasks 1-5 source/docs, Task 6 live/user-state acceptance, Task 7
  decision/baseline sync.
- Test Obligations: exact commands in every task plus cross-provider evidence.
- Review Gates: task diff and focused verification before each commit; stop
  after Task 7 and return evidence for independent final review.
- Drift Rules: any need for config, probing, error matching, another repair,
  parser loosening, data migration, or card edit returns to design.
- Evidence Required: commits, exact test outputs, live provider table,
  `echo-test` result, retirement searches, clean scoped status.
- Advisory Boundary: this view guides execution and does not grant completion.

## Task 1: Make The Compatible Wire Provider-Safe

**Files**

- Modify: `crates/neo-ai/src/providers/openai/compatible.rs`

**Why**

Every configured `type = "openai"` endpoint currently receives an optional
JSON Schema field that the protocol type cannot promise.

**Change Necessity**

Documentation cannot change the serialized request. The minimum source change
is deletion of the `response_format` assignment in `request_body`.

**Repair Track**

- Root cause: ambiguous compatible wire assumed official optional capability.
- Owner: `OpenAiCompatibleClient::request_body`.
- Repair: remove the branch that writes `body["response_format"]`.
- Boundary: leave internal `RequestOptions`, prompts, and Responses mapping
  unchanged.

**Retirement Track**

- Delete `response_format_maps_json_schema_for_chat_completions`.
- Replace it with `response_format_is_omitted_for_compatible_chat_endpoints`.
- Do not keep the old mapping behind configuration or endpoint matching.

**Steps**

1. Delete this production branch:

   ```rust
   if let Some(response_format) = &request.options.response_format {
       body["response_format"] = response_format.to_openai_chat_response_format();
   }
   ```

2. Replace the positive mapping regression with a request that contains a
   `ResponseFormat` and asserts:

   ```rust
   assert!(body.get("response_format").is_none());
   assert!(body.pointer("/text/format").is_none());
   ```

3. Run:

   ```bash
   rtk cargo nextest run -p neo-ai --lib response_format_is_omitted_for_compatible_chat_endpoints
   rtk cargo nextest run -p neo-ai --lib response_format_maps_json_schema_for_responses_api
   rtk proxy rustfmt --check --edition 2024 crates/neo-ai/src/providers/openai/compatible.rs
   rtk git diff --check
   ```

4. View only this file's diff, stage it, and commit:

   ```bash
   rtk git diff -- crates/neo-ai/src/providers/openai/compatible.rs
   rtk git add crates/neo-ai/src/providers/openai/compatible.rs
   rtk git commit -m "fix(ai): omit unsupported structured output hints"
   ```

**Expected proof**

- The exact nextest test passes.
- `openai/compatible.rs` has no production write to `response_format`.
- `openai/responses.rs` is untouched.

## Task 2: Preserve Failed Child Outcomes And Skip Repair

**Files**

- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/src/workflow/runtime.rs`
- Modify: `crates/neo-agent-core/tests/workflow_schema.rs`

**Why**

A provider/runtime failure is currently consumed as invalid assistant JSON,
causing a second request and replacing the real reason.

**Change Necessity**

The minimum repair is an early return at each existing schema-acceptance
consumer. The schema validator remains unchanged and continues to own only
successful assistant text.

**Repair Track**

- Root cause: missing successful-turn precondition.
- Owners: `apply_child_output_schema` and
  `child_run_to_outcome_with_schema`.
- Repair: bypass compilation/validation/repair when the ordinary child outcome
  is not successful.
- Boundary: preserve actual usage, child details, state, and original summary.

**Retirement Track**

- A provider error must no longer appear in `first_raw` or `repair_raw` as a
  schema failure.
- No schema-repair journal event may be written for a failed model turn.
- Do not add a new error type or fallback.

**Steps**

1. In `apply_child_output_schema`, after enforcing the workflow-child
   `output_schema` requirement but before compiling the schema, apply the same
   success predicate used by `child_agent_to_outcome`: lifecycle must be
   `AgentLifecycleState::Completed`, and an optional terminal outcome must not
   be an error. A completed snapshot with `outcome = None` is not automatically
   a failure. For a failed turn, return the original output plus only observed
   actual usage:

   ```rust
   let child_succeeded = output.snapshot.state == AgentLifecycleState::Completed
       && !output
           .snapshot
           .outcome
           .as_ref()
           .is_some_and(|outcome| outcome.is_error);
   if !child_succeeded {
       let usage = accumulate_actual_usage(None, &output.events);
       let mut extra = json!({});
       if let Some(usage) = usage {
           extra["actual_usage"] = json!(usage);
       }
       return Ok((output, extra));
   }
   ```

2. In `child_run_to_outcome_with_schema`, immediately after
   `child_run_to_outcome(output)`, return the ordinary outcome when `!outcome.ok`.
   Do not compile the schema, write schema-repair journal events, or start a
   second model request.

3. Add integration regression
   `workflow_delegate_protocol_failure_skips_schema_repair_and_preserves_error`.
   Use `FakeHarness::from_result_turns` with `AiError::Protocol`, set
   `max_retries = 0`, and exercise the foreground Delegate path through its
   normal workflow dispatch. Assert:

   - exactly one child request;
   - workflow child outcome is failed;
   - summary contains the original protocol error;
   - no `SchemaRepairStarted` or `SchemaRepairFinished` journal record;
   - no `schema_error` or `strict_json_failed` replacement;
   - observed usage and child reference fields remain truthful.

4. Add integration regression
   `workflow_swarm_protocol_failure_skips_schema_repair_and_preserves_error`.
   Exercise `child_run_to_outcome_with_schema` through a direct workflow swarm
   with a required item `output_schema`. Assert the same one-request,
   original-error, zero-repair, usage, and child-detail behavior. Do not call
   `accept_child_structured_output_with_repair` directly in either regression;
   that would bypass the guards being proved.

5. Keep the existing successful-invalid-text repair regression unchanged except
   for provider-wire expectations owned by Task 1. It must still prove exactly
   two requests and one tools-disabled repair.

6. Run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --test workflow_schema workflow_delegate_protocol_failure_skips_schema_repair_and_preserves_error
   rtk cargo nextest run -p neo-agent-core --test workflow_schema workflow_swarm_protocol_failure_skips_schema_repair_and_preserves_error
   rtk cargo nextest run -p neo-agent-core --test workflow_schema child_schema_invalid_output_gets_exactly_one_tools_disabled_repair
   rtk proxy rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/workflow/runtime.rs crates/neo-agent-core/tests/workflow_schema.rs
   rtk git diff --check
   ```

7. View only Task 2 paths, stage, and commit:

   ```bash
   rtk git diff -- crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/workflow/runtime.rs crates/neo-agent-core/tests/workflow_schema.rs
   rtk git add crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/workflow/runtime.rs crates/neo-agent-core/tests/workflow_schema.rs
   rtk git commit -m "fix(workflow): preserve failed child errors"
   ```

**Expected proof**

- Failed child: one request, original error, zero repairs.
- Successful invalid text: one repair, strict validation unchanged.

## Task 3: Make Swarm Failure Summary Actionable

**Files**

- Modify: `crates/neo-agent-core/src/workflow/runtime.rs`
- Modify: `crates/neo-agent-core/tests/workflow_lua.rs`

**Why**

The model-visible swarm summary currently reports only counts even when every
child failed. The first actionable error is hidden in the journal.

**Change Necessity**

The ordered item outcomes already contain the required reason. Only the
existing summary projection must change.

**Repair Track**

- Root cause: state-independent summary formatting.
- Owner: `WorkflowRuntime::run_swarm_batch_effect` final outcome construction.
- Repair: compute failed count and first failed bounded summary before moving
  ordered details into the outcome.
- Boundary: keep item order, details, usage, and child records unchanged.

**Retirement Track**

- Remove generic `items=N finished=N` summary for failed swarms.
- Retain it for successful swarms.
- Add no per-child transcript or result expansion.

**Steps**

1. Before final outcome construction, derive:

   ```rust
   let failed_count = item_outcomes
       .iter()
       .filter(|(_, outcome)| !outcome.ok)
       .count();
   let first_failure = item_outcomes
       .iter()
       .find_map(|(_, outcome)| (!outcome.ok).then(|| bounded_summary(&outcome.summary)));
   ```

2. Build one summary:

   - success: existing count summary;
   - failure: `swarm <id> failed <failed>/<total>: <first failure>`;
   - interrupted without failure: include finished and total counts plus
     `interrupted`.

3. Add regression
   `workflow_swarm_failure_summary_includes_first_bounded_error`. Assert:

   - outcome is failed;
   - summary includes `failed 2/2` and the first error;
   - summary contains at most the existing 160-character bounded error;
   - ordered item details still contain both full child outcomes.

4. Run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --test workflow_lua workflow_swarm_failure_summary_includes_first_bounded_error
   rtk proxy rustfmt --check --edition 2024 crates/neo-agent-core/src/workflow/runtime.rs crates/neo-agent-core/tests/workflow_lua.rs
   rtk git diff --check
   ```

5. Stage only the two files and commit:

   ```bash
   rtk git add crates/neo-agent-core/src/workflow/runtime.rs crates/neo-agent-core/tests/workflow_lua.rs
   rtk git commit -m "fix(workflow): surface swarm child failures"
   ```

## Task 4: Add Bounded Final-Schema Context

**Files**

- Modify: `crates/neo-agent-core/src/workflow/schema.rs`
- Modify: `crates/neo-agent-core/tests/workflow_schema.rs`

**Why**

Final schema failures identify the rule but may omit the result path and nearby
value needed to correct the Lua return. Copying the complete result would waste
model input and may expose unrelated data.

**Change Necessity**

`SchemaValidationError` already owns instance-path data. A small private preview
helper in the same module is the minimum sufficient diagnostic.

**Repair Track**

- Root cause: final-result mapper discards `instance_path` and local value.
- Owner: `validate_final_lua_result`.
- Repair: resolve `Value::pointer`, serialize that node, and append a bounded
  preview.
- Boundary: error code and terminal behavior remain unchanged; no repair turn.

**Retirement Track**

- Do not add full invalid result to workflow details or journal.
- Do not create an artifact for rejected results.

**Steps**

1. Add a private helper in `schema.rs` that uses `serde_json::to_string`, keeps
   at most 160 Unicode scalar values, and never slices UTF-8 bytes. If the
   serialized value exceeds the bound, collect the first 159 characters and
   append the single existing Neo ellipsis character `…`, yielding exactly 160
   characters. If serialization fails, use the JSON value's ordinary string
   rendering; do not add a second error path.

2. In `validate_final_lua_result`, before rewriting the message:

   - use `<root>` when `instance_path` is empty;
   - otherwise use the existing JSON Pointer;
   - resolve the failing node with `value.pointer(&err.instance_path)` and fall
     back to the root only if the pointer cannot resolve;
   - format
     `schema_invalid_final_result at <path>: <validator message>; actual=<preview>`.

3. Extend the existing final-result test or add
   `final_lua_schema_error_includes_path_and_bounded_actual`. Cover:

   - nested path `/name`;
   - exact short value preview;
   - a long Unicode value is cut safely and ends with an ellipsis;
   - message does not contain the complete long root object.

4. Run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --test workflow_schema final_lua_schema_error_includes_path_and_bounded_actual
   rtk proxy rustfmt --check --edition 2024 crates/neo-agent-core/src/workflow/schema.rs crates/neo-agent-core/tests/workflow_schema.rs
   rtk git diff --check
   ```

5. Stage only Task 4 files and commit:

   ```bash
   rtk git add crates/neo-agent-core/src/workflow/schema.rs crates/neo-agent-core/tests/workflow_schema.rs
   rtk git commit -m "fix(workflow): clarify final schema failures"
   ```

## Task 5: Teach Preserved Strict Behavior Before Authoring

**Files**

- Modify: `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- Modify: `crates/neo-agent-core/src/skills/builtin/mod.rs`
- Modify: `docs/zh/guides/workflows.md`
- Modify: `docs/en/guides/workflows.md`

**Why**

Repeated AI reports classify intentional behavior as defects because the
correct mental model is implicit or split across sections. Prompt guidance is
part of first-call product reliability.

**Change Necessity**

Runtime aliases would weaken the approved API. The minimum change is to make
the existing canonical skill explicit and align the two guides.

**Repair Track**

- Root cause: incomplete and contradictory guidance near first use.
- Owner: `create-workflow` skill; human mirrors in both guides.
- Repair: one compact closed-decisions block near the authoring checklist and
  precise return/error semantics in existing API sections.
- Boundary: no global prompt, second skill, tool, or duplicated long tutorial.

**Retirement Track**

- Replace the paired-file sentence that can be read as an inline default.
- Replace "all effectful calls return one outcome shape" with accurate grouped
  semantics.
- Delete repeated wording made obsolete by the new compact block.

**Steps**

1. Add `## Closed behavior: do not retry by changing the API shape` immediately
   after the authoring checklist in `create-workflow.md`. Use this exact compact
   content, adjusting only surrounding heading depth if the current file moved:

   ```markdown
   ## Closed behavior: do not retry by changing the API shape

   - Inline `validate_inline`, `save`, and `run_inline` always require explicit
     `input_schema` and `output_schema`. For no arguments, use
     `{"type":"object","additionalProperties":false}`.
   - Call `neo.tool` only as
     `{ name = "ToolName", input = { ... } }`. A call-shape decode error aborts
     the host operation; an executed tool failure returns `ok = false` and may
     be branched on.
   - `neo.json_array(table)` and `neo.json_object(table)` return marked Lua
     tables. They do not serialize values, and `nil` is invalid.
   - `neo.fail(message)` is terminal. `pcall` cannot undo or recover that run
     decision.
   - `neo.await_user(...)` returns the raw read-only answer value, not an outcome
     table.
   - `neo.report(...)` records an intermediate report and returns no value. Use
     it only as a statement.
   - Read and wait for workflow task IDs with `TaskOutput`. Never pass a
     workflow ID to `WaitDelegate`.
   ```

2. Update the Host API tables so they state:

   - `neo.report`: returns no value; statement-only;
   - `neo.fail`: terminal and not recoverable through `pcall`;
   - `neo.await_user`: raw read-only answer;
   - `neo.tool`: only `{name,input}`; decode failure differs from `ok=false`;
   - JSON markers: table in, marked table out, `nil` invalid;
   - workflow task IDs: `TaskOutput`, not `WaitDelegate`.

3. In both language guides:

   - qualify omitted paired-file `input_schema` as a stored-definition behavior;
   - state that inline actions require the explicit no-argument schema;
   - replace the universal effect-outcome statement with three groups:
     outcome-table calls, raw-answer `await_user`, and no-return `report`;
   - add the terminal/decode/marker/wait boundaries without another tutorial.

4. Extend existing test
   `create_workflow_builtin_teaches_authoring_without_mandatory_choreography`.
   Assert the skill contains the distinctive anchors `always require explicit`,
   `additionalProperties`, `{ name = "ToolName", input = { ... } }`,
   `call-shape decode error`, `marked Lua tables`, `pcall`, `raw read-only answer
   value`, `returns no value`, `TaskOutput`, and `WaitDelegate`. Also assert it
   contains no retired optional-inline-schema, flat-tool, catchable-fail, or
   string-return marker guidance.

5. Run:

   ```bash
   rtk cargo nextest run -p neo-agent-core --lib create_workflow_builtin_teaches_authoring_without_mandatory_choreography
   rtk proxy rustfmt --check --edition 2024 crates/neo-agent-core/src/skills/builtin/mod.rs
   rtk git diff --check
   ```

6. Stage only Task 5 files and commit:

   ```bash
   rtk git add crates/neo-agent-core/src/skills/builtin/create-workflow.md crates/neo-agent-core/src/skills/builtin/mod.rs docs/zh/guides/workflows.md docs/en/guides/workflows.md
   rtk git commit -m "docs(workflow): teach strict authoring behavior"
   ```

## Task 6: Repair `echo-test` And Run Live Acceptance

**Files**

- Modify exact user state:
  `/Users/chenyuanhao/.neo/workflows/echo-test.workflow.toml`
- Do not modify:
  `/Users/chenyuanhao/.neo/workflows/echo-test.lua`
- Evidence only: temporary workspaces and existing workflow run directories.

**Why**

The known saved definition is internally inconsistent and remains a visible
false product failure after source repairs.

**Change Necessity**

The user approved exact repair of this definition. Repository migration code is
unnecessary and forbidden.

**Repair Track**

- Add the exact `input_schema` from the design.
- Preserve Lua bytes and `source_sha256`.
- Touch no other user workflow.

**Retirement Track**

- No deletion is authorized.
- No automatic migration or normalization is added.

**Steps**

1. Record hashes and status before the edit:

   ```bash
   rtk cargo build -p neo-agent --bin neo
   rtk proxy shasum -a 256 /Users/chenyuanhao/.neo/workflows/echo-test.lua
   rtk proxy shasum -a 256 /Users/chenyuanhao/.neo/workflows/echo-test.workflow.toml
   rtk grep -n '^source_sha256' /Users/chenyuanhao/.neo/workflows/echo-test.workflow.toml
   rtk proxy find /Users/chenyuanhao/.neo/workflows -maxdepth 1 -type f -exec shasum -a 256 '{}' +
   rtk git status --short
   ```

2. Use `apply_patch` to add only the approved TOML input schema. Do not use a
   shell redirection or rewrite the file wholesale.

3. Verify the Lua hash still equals the manifest `source_sha256` and validate:

   ```bash
   rtk proxy shasum -a 256 /Users/chenyuanhao/.neo/workflows/echo-test.lua
   rtk grep -n '^source_sha256' /Users/chenyuanhao/.neo/workflows/echo-test.workflow.toml
   rtk proxy ./target/debug/neo workflow check echo-test --json
   rtk proxy ./target/debug/neo workflow run echo-test --args '{"text":"hello"}' --output json
   rtk proxy find /Users/chenyuanhao/.neo/workflows -maxdepth 1 -type f -exec shasum -a 256 '{}' +
   ```

4. Record the successful final value and confirm no other file under
   `~/.neo/workflows` changed during this task.

5. Run one minimal inline or temporary saved one-child workflow with a required
   object schema on the active report-004 endpoint and on one representative
   OpenAI-compatible endpoint. Record:

   | Provider | Resolved type | Reached model | Repair count | Result | Blocker |
   | --- | --- | --- | --- | --- | --- |
   | kimi-for-coding | | | | | |
   | deepseek | | | | | |

   Empty cells are not acceptable in the final handoff; fill them with evidence
   or an exact blocker.

   Provider selection must not mutate the user's persistent default model or
   provider. Use an already selected session model, or an isolated temporary
   configuration that references existing credential environment variables
   without copying inline secrets. If neither is safely available, record that
   exact limitation as the provider's blocker. Never rewrite
   `~/.neo/config.toml` for acceptance.

6. Run shipped `code-review` on one small read-only scope through a provider
   that passed the child smoke. Record useful structured output, token usage,
   repair count, and absence of the old HTTP 400.

7. This task does not create a repository commit. Do not stage user-state or
   temporary files.

**Expected proof**

- `echo-test` returns `ok=true` and `echo="hello"`.
- The active report-004 endpoint and the representative OpenAI-compatible
  endpoint reach the model.
- One built-in workflow returns useful structured data.

## Task 7: Sync The Decision And Prepare Final Review

**Files**

- Modify: `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- Modify: `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md`

**Why**

The previous recorded assumption said the compatible provider mapping was an
accuracy improvement. The new live evidence disproves that assumption.

**Change Necessity**

The architecture decision and landed baseline must describe the single current
path so a future maintainer does not reintroduce the removed branch.

**Repair Track**

- Amend ADR-0008 with selected deterministic wire mapping and rejected
  alternatives.
- Update the landed baseline with failed-child gating, swarm summary, guidance,
  exact tests, and live evidence.

**Retirement Track**

- State that compatible-wire native JSON Schema mapping is retired.
- State that no capability switch, probe, or retry remains.
- Preserve the historical partial-acceptance note rather than rewriting it.

**Steps**

1. Add a dated ADR amendment that records:

   - `openai_response` maps native JSON Schema;
   - `openai` compatible wire omits it;
   - prompt plus strict host validation plus one content repair is canonical;
   - failed child turns bypass schema repair;
   - error-text retry, configuration, probes, and aliases were rejected.

2. Update the baseline with exact commit hashes, exact test commands/results,
   cross-provider evidence or blockers, and `echo-test` result.

3. Run retirement searches:

   ```bash
   rtk proxy rg -n 'body\["response_format"\]' crates/neo-ai/src/providers/openai/compatible.rs
   rtk proxy rg -n 'structured_output.*(supported|capability)|response_format.*(retry|fallback|probe)' crates/neo-ai/src crates/neo-agent-core/src
   rtk proxy rg -n 'flat.*neo\.tool|neo\.json_array\(nil\)|WaitDelegate.*workflow' crates/neo-agent-core/src/skills/builtin/create-workflow.md docs/zh/guides/workflows.md docs/en/guides/workflows.md
   ```

   Expected: the first search has no matches; the latter searches show only
   explicit prohibitions or no matches, never an active alternate path.

4. Run final scoped checks:

   ```bash
   rtk git diff --check
   rtk git status --short
   rtk git log --oneline -12
   ```

5. Stage only the ADR and baseline and commit:

   ```bash
   rtk git add docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md
   rtk git commit -m "docs(workflow): record provider-safe child output"
   ```

6. Stop implementation. Return the evidence bundle to the user so the original
   reviewer can perform the requested independent final review. Do not make
   additional fixes, push, merge, amend, clean, or claim remote/native release
   completion.

## Final Evidence Required From The Implementer

Return in Chinese, conclusion first:

1. commits in Task 1-5 and Task 7 order;
2. exact focused command results;
3. live provider table with evidence or exact blockers;
4. `echo-test` before/after hashes and final result;
5. retirement-search output;
6. changed paths grouped by task;
7. unrelated dirty paths preserved;
8. explicit confirmation that no Delegate-family card file changed;
9. residual provider, remote CI, and native Windows/Linux risk;
10. a direct request for the original reviewer to conduct final review.

## Plan Self-Review

- Spec coverage: every approved requirement maps to Tasks 1-7.
- Placeholders: none; live evidence cells must be filled during Task 6.
- Type consistency: changes use existing request, outcome, schema-error, and
  summary types.
- Compatibility: strict schemas, one repair, persistence, tasks, and cards are
  explicitly preserved.
- Minimality: one obsolete wire branch is deleted; no new owner or setting.
- Verification: every source slice has an exact package, target, and test
  filter.
- Dual track: repair and retirement are explicit for every source task.
- Final authority: implementation stops before independent final review.
