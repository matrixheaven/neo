# Workflow Model-Visible Results And Recoverable Failures Design

Status: `approved`
Date: `2026-07-30`

## 1. Purpose

Make the existing `Workflow` tool usable by an imperfect model on the first
attempt. The model must be able to choose a saved workflow, correct invalid
input, read the final result, page useful journal summaries, and recover from
ordinary workflow-host failures without reading Neo source or guessing hidden
`ToolResult.details` fields.

This design changes no workflow lifecycle owner and adds no tool.

## 2. Current Defect

The next model request receives `ToolResult.content`, not
`ToolResult.details`. The current Workflow paths place most actionable data in
`details` while `content` contains only a human summary:

- `Workflow(list)` hides entries and the next cursor;
- `Workflow(show)` hides the definition and schemas;
- input errors hide `expected_shape` and `next_actions`;
- `TaskOutput(view="result")` says only whether a result exists;
- `TaskOutput(view="journal")` says only how many records exist.

The runtime and journal already hold the correct data. The defect is the
model-visible projection.

A second defect is production-path parity. Focused `LuaWorkflowRunner` tests
prove that `neo.verify(false)` and a denied `neo.tool` call are recoverable,
while a real `Workflow(run_inline)` evaluation reports that both can terminate
the run. Unit runner coverage therefore does not prove the production path.

## 3. Approved Direction

Use the existing owners and repair their outputs:

1. `Workflow` returns compact, action-specific JSON in `ToolResult.content`.
2. `TaskOutput` returns the requested bounded payload in
   `ToolResult.content`.
3. `ToolResult.details` remains the typed UI and event projection.
4. Expected host-operation failures return immutable outcome values.
5. Omitted `input_schema` means the workflow accepts no arguments.
6. `output_schema` remains required.
7. Workflow tasks continue to use `TaskOutput`; `WaitDelegate` remains limited
   to delegates and swarms.

## 4. Model-Visible Output Invariant

Every field needed for the model's next decision MUST be present in
`ToolResult.content`.

`ToolResult.details` MAY contain richer typed presentation data, but the model
must never need it to:

- choose or run a workflow;
- correct a failed call;
- read a final result;
- continue journal paging;
- answer a pending workflow question;
- choose the next documented tool call.

The runtime MUST NOT globally append arbitrary `details` to model context.
Each Workflow-owned projection selects only the data needed for that action.

## 5. Workflow Action Results

`Workflow` content is one compact JSON object. It uses the existing top-level
shape and removes null fields before serialization:

```json
{
  "ok": true,
  "action": "list",
  "status": "listed",
  "items": {},
  "next_actions": []
}
```

Required model-visible fields by action:

| Action | Content fields |
| --- | --- |
| `list` | `ok`, `action`, `status`, `items.entries`, `items.cursor`, `items.total`, `next_actions` |
| `show` | `ok`, `action`, `status`, `workflow`, `next_actions` |
| `validate_inline` | `ok`, `action`, `status`, `validation`, `workflow`, `next_actions` |
| `validate_saved` | `ok`, `action`, `status`, `validation`, `workflow`, `next_actions` |
| `save` | `ok`, `action`, `status`, `workflow`, `next_actions` |
| `run_inline` | `ok`, `action`, `status`, `task`, `next_actions` |
| `run_saved` | `ok`, `action`, `status`, `task`, `next_actions` |

The model-visible `workflow` object includes only information needed to use or
edit the definition:

- name and display name;
- description and phases;
- input and output schemas;
- script only for `show` and explicit inline validation results.

It excludes revision hashes, source hashes, storage paths, internal origins,
and other implementation identifiers.

Errors use one compact JSON shape:

```json
{
  "ok": false,
  "action": "run_inline",
  "status": "error",
  "error": {
    "code": "workflow_input_invalid",
    "message": "output_schema is required for run_inline",
    "field": "output_schema",
    "expected_shape": {
      "required": ["action", "name", "description", "phases", "script", "output_schema"],
      "optional": ["input_schema", "args"]
    },
    "side_effect_occurred": false
  },
  "next_actions": []
}
```

The human-readable summary remains a UI concern. It is not prepended to the
model JSON. Existing Workflow tool cards continue to render their current
human summary from typed arguments and details; they neither display nor parse
the compact model JSON.

## 6. TaskOutput Views

`TaskOutput` keeps the existing views, cursors, byte limits, and journal
scanner. Only the model-visible serialization changes.

### 6.1 Summary

Return status, invocation count, failure count, pending request data, and the
exact next action when one exists.

### 6.2 Result

Return the actual final JSON result in `content`, not only `has_result`.

If the result cannot fit within the existing complete result limit, return the
existing bounded resource error with an artifact read instruction. Never claim
that the model received a result when it received only metadata.

### 6.3 Journal

Return the existing bounded journal summary records, `has_more`, and
`next_cursor`. Do not return the full journal and do not introduce an unbounded
text view.

### 6.4 Artifacts

Return the existing bounded artifact entries or content page needed by the
requested view. Internal content hashes remain identifiers only where the
next artifact read requires them.

### 6.5 Byte Accounting

`content` uses compact JSON. Existing complete `ToolResult` accounting remains
the safety ceiling, including both `content` and `details`. Page shrinking
continues to remove trailing journal or artifact entries until the complete
result fits.

No new output-size setting is added.

## 7. Input And Output Schemas

### 7.1 No-Argument Default

For `validate_inline`, `save`, and `run_inline`, omitted `input_schema` is
normalized by the definition owner to:

```json
{"type":"object","additionalProperties":false}
```

Omitted `args` remains `{}`. A non-empty `args` object fails against the
normalized schema. Neo never infers a schema from example arguments.

This is the sole meaning of a missing input schema for current inline and saved
definitions. No compatibility branch preserves the old unvalidated meaning.

### 7.2 Required Output Schema

`output_schema` remains required for inline validation, save, and run. Neo does
not synthesize a permissive schema and does not disable final result
validation.

A workflow with no meaningful result uses an explicit null schema:

```json
{"type":"null"}
```

### 7.3 First-Attempt Tool Shape

The model-visible `Workflow` input schema becomes an action-discriminated
schema. It remains one tool with the same seven action names, but each branch
declares its own required and optional fields.

Provider adapters must preserve the action branches. If a provider cannot
accept the canonical schema, registration fails clearly; Neo does not replace
it with a loose fallback schema.

## 8. Script Failure Semantics

The Workflow host APIs use three failure classes.

### 8.1 Outcome Values

Expected operation failures return immutable outcome values and do not abort
the workflow:

- `neo.verify(false, message)`;
- an eligible tool returning an error;
- an unknown tool name;
- a tool denied for workflow use;
- delegate or swarm child failure;
- command verification failure.

The common shape is:

```json
{
  "ok": false,
  "status": "failed",
  "summary": "...",
  "details": {"code": "..."}
}
```

Scripts branch on `outcome.ok`. `pcall` is not required for expected failures.

### 8.2 Catchable Script Errors

Malformed host API input remains a Lua error catchable by `pcall`, including:

- missing or empty required fields;
- a non-object tool input;
- invalid argument types;
- attempts to mutate immutable host values.

Catching these errors does not convert them into successful outcomes.

### 8.3 Terminal Run Failures

The following remain terminal:

- explicit `neo.fail`;
- Lua compile failure;
- uncaught Lua error;
- final result schema failure;
- resource limit exhaustion;
- cancellation;
- unrecoverable journal or host integrity failure.

`neo.fail` remains terminal even when wrapped in `pcall`.

## 9. Production-Path Parity

Failure semantics must be proven through the production chain, not only a
direct runner fixture:

```text
Workflow(run_inline)
-> WorkflowLaunchCoordinator
-> WorkflowRuntime
-> LuaWorkflowRunner
-> Workflow task completion
-> TaskOutput
-> next model request
```

The acceptance test must use the production workflow dispatch resolver and the
same tool-result message projection used by a normal model turn.

## 10. Waiting Boundary

`TaskOutput` remains the only workflow task read and wait tool.

- automatic completion remains the default;
- `TaskOutput(block=true)` is the explicit bounded wait;
- `WaitDelegate` accepts only delegate and swarm identifiers;
- no identifier compatibility layer or unified wait alias is added.

Workflow launch content includes the exact optional `TaskOutput` call, so the
model does not need to infer the waiting tool from the task identifier.

## 11. Unknown Tool Claim

The usability report observed one `unknown tool` result for `Read`, but did not
include the raw workflow input, journal record, session registry state, or
resolver refresh sequence. This is insufficient evidence for a registry
redesign.

Implementation work may add a focused production-path reproduction using the
existing registry and resolver. It must not add retries, fuzzy names, aliases,
or a second registry owner. A registry change is allowed only after that
reproduction identifies a root cause.

## 12. Options Considered

### Option A: Documentation And Defaults Only

Rejected. It reduces some first-call errors but leaves list, show, errors,
results, and journals invisible to the model.

### Option B: Repair Existing Model Projections And Failure Semantics

Selected. It fixes the complete defect class within the current owners and
adds no product surface.

### Option C: Split Workflow Into Multiple Tools Or Expand WaitDelegate

Rejected. It adds tool names and duplicate waiting paths without fixing the
hidden-result defect.

### Existence Check

- Proposed new product surface: none.
- Existing reuse path: `Workflow`, `TaskOutput`, Workflow definition
  resolution, `LuaWorkflowRunner`, and the current turn loop.
- New fallback or compatibility path: none.
- Decision: reuse existing owners.

## 13. Ownership And Impact

Canonical owners remain:

- `WorkflowTool`: action input and action result projection;
- workflow definition resolution: schema normalization and validation;
- `LuaWorkflowRunner`: script host failure semantics;
- workflow output module: bounded TaskOutput view projection;
- runtime turn loop: unchanged generic rule that model context receives
  `ToolResult.content`;
- TUI and workflow operator: unchanged typed consumers of runtime state and
  `ToolResult.details`.

No second runtime, registry, journal reader, task system, or completion queue is
introduced.

## 14. Non-Goals

This design does not:

- replace Lua;
- add or rename model tools;
- change the seven Workflow actions;
- add automatic retries or fuzzy tool matching;
- expose full journals;
- inject arbitrary `ToolResult.details` into model context;
- change workflow persistence or replay identity;
- change Delegate, DelegateGroup, or DelegateSwarm cards;
- change Bash or Terminal behavior;
- redesign `/tasks`;
- resolve the unproven `Read` registry claim without reproduction.

## 15. Acceptance Evidence

Focused tests must prove:

1. the Workflow model schema declares action-specific required fields;
2. omitted input schema accepts `{}` and rejects non-empty arguments;
3. missing output schema is rejected before side effects;
4. list content includes usable entries and paging data;
5. show content includes the usable definition without internal hashes;
6. input error content includes field, expected shape, and next actions;
7. launch content includes the task identifier and exact TaskOutput action;
8. result content includes the actual final JSON;
9. journal content includes bounded summary records and a valid next cursor;
10. denied and unknown `neo.tool` calls return failed outcomes without aborting;
11. `neo.verify(false)` returns a failed outcome without aborting;
12. `neo.fail` remains terminal;
13. a production-path run proves the final result reaches the next model
    request;
14. existing typed details remain available to current TUI consumers;
15. no Workflow identifier is accepted by `WaitDelegate`.

The existing fresh-session assistant acceptance remains required. Negative
validation cases are reported separately from first-attempt success; they are
not counted as failed positive runs.

## 16. Retirement And Baseline Sync

Implementation removes the old summary-only Workflow and TaskOutput model
projections. No fallback keeps both shapes.

After implementation and verification:

1. update the current workflow ADR for model-visible outputs and host failure
   semantics;
2. record a new landed baseline;
3. update the `create-workflow` skill and user documentation;
4. mark the affected result-projection and failure-semantics sections of older
   designs as historical evidence.

## 17. Working Artifacts

### Task Intent

- Outcome: an imperfect model can author, run, observe, and recover from a
  workflow without hidden result fields or an avoidable retry.
- Success evidence: the focused acceptance list above plus fresh production
  model sessions.
- Stop condition: every required model decision field is present in
  `ToolResult.content`, production failure behavior matches focused tests, and
  no duplicate interface is introduced.
- Primary risk: copying all typed details into model context would leak
  implementation data and inflate tokens.

### Baseline Read Set

- `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`
- `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- `docs/aegis/specs/2026-07-27-workflow-product-surface-redesign.md`
- current Workflow, output, Lua runner, and turn-loop owners

### Baseline Usage

- Product direction: preserve one Workflow tool and TaskOutput-based task
  handling.
- Runtime boundary: preserve WorkflowRuntime, registry, journal, background
  task, and TUI ownership.
- Result: current implementation drift in model-visible projection plus a
  production-path verification gap.
- Architecture alignment: the current owners remain correct; the projection
  and production acceptance evidence are incomplete.

### Impact Statement

- Affected: model-visible Workflow results, TaskOutput results, inline input
  normalization, Lua host failure values, and focused tests.
- Preserved: persistence, replay, task lifecycle, UI cards, operator state,
  shell tools, and delegate concurrency.
- Added product surfaces: none.
- Removed behavior: hidden actionable result payloads and fatal handling for
  expected host-operation failures.

## 18. Review Gate

This design requires user approval before an implementation plan or source
change. Approval selects Option B and the exact boundaries in this document.
