# Dynamic Workflow AI Reliability Design

## Status

Proposed design for user review. The direction was approved in conversation on
2026-07-30. Implementation must not begin until this written design is approved.

This design supersedes the omitted-`input_schema` default in
`2026-07-30-workflow-model-visible-results-design.md`. It preserves the rest of
that design and the current workflow runtime, persistence, and task boundaries.

## Problem

The current dynamic workflow path can spend substantial model usage, obtain a
useful child answer, and still publish a misleading result:

1. A child model turn finishes, so the agent lifecycle is `completed`.
2. The child emits JSON inside a Markdown fence. Strict host parsing rejects it.
3. The single repair turn can repeat the same fenced form.
4. `Delegate` correctly returns an error while retaining the real completed
   agent lifecycle in its typed details.
5. Workflow dispatch incorrectly treats that two-layer state as corrupt and
   replaces the real schema error with `error result cannot be completed`.
6. Built-in workflows ignore failed outcome values, synthesize placeholder
   findings, and can report success after every required child failed.
7. The TUI shows `Failed Delegate` and a bare `status: completed`, which exposes
   the two internal states without explaining their different meanings.

Production child requests also fail to use Neo's existing provider-neutral
response-format support. The helper and provider mappings exist, but normal and
repair child turns do not attach the required JSON Schema to the model request.
The runtime therefore relies primarily on prompt compliance.

A separate authoring defect exists. Omitted `input_schema` currently becomes a
strict no-argument schema. A model can therefore save a script that reads
`neo.args.text` while silently declaring that the workflow accepts no input.
The saved `echo-test` observed in the usability run demonstrates this failure.

## Goals

- Make schema-constrained child output reliable on the first model turn when
  the active provider supports structured response formats.
- Keep exactly one host validation path for all providers.
- Preserve the difference between agent lifecycle and invocation result without
  displaying them as a contradiction.
- Preserve the original schema failure, actual usage, and child references in
  the workflow journal and model-visible result.
- Prevent built-in workflows from returning placeholder success after required
  children fail.
- Make inline workflow authoring explicit enough that an imperfect model cannot
  silently save an input-incompatible definition.
- Strengthen existing model guidance without adding tools, aliases, hidden
  retries, or another workflow authoring path.

## Non-Goals

- Changing `Delegate`, `DelegateGroup`, or `DelegateSwarm` card layout,
  expansion, ordering, or child activity presentation.
- Accepting prose, multiple JSON values, or Markdown-fenced JSON as canonical
  structured output.
- Adding another schema repair attempt.
- Making `output_schema` optional or synthesizing a permissive schema.
- Adding top-level `neo.swarm.output_schema`, flat `neo.tool` arguments,
  `WaitDelegate` workflow identifiers, or `neo.json_array(nil)` normalization.
- Changing `neo.verify` into a terminal operation or making `neo.fail`
  catchable.
- Parsing Lua source to infer input or output schemas.
- Mutating, deleting, or migrating existing user workflow definitions,
  sessions, journals, or artifacts.
- Redesigning built-in workflow concurrency in this change.

## First-Principles Decision

- Required outcome: a workflow either returns validated useful data or fails
  with the original actionable reason.
- Non-negotiable constraints: one workflow runtime owner, strict host
  validation, one repair turn, unchanged Delegate-family cards, and no aliases.
- Assumptions removed: a completed model turn is not necessarily a successful
  delegated result; prompt text alone is not a reliable structured-output
  mechanism; omitted authoring schemas are not safe defaults.
- Smallest sufficient path: connect the existing response-format capability,
  correct result mapping at workflow dispatch, delete placeholder success, and
  make authoring schemas explicit.

## State Semantics

Agent lifecycle and requested result acceptance are separate facts.

| Fact | Meaning | Schema failure after repair |
| --- | --- | --- |
| Agent lifecycle | Whether the model turn and child runtime settled | `completed` |
| Delegate result | Whether the requested delegated result was delivered | `failed` |
| Workflow invocation | Whether the host invocation satisfied its requirement | `failed` |
| Workflow child record | Whether the child contribution is usable by the workflow | `failed` |

Workflow dispatch MUST accept the combination of an error `ToolResult` and a
completed agent lifecycle. It MUST map the invocation to `failed`, preserve the
typed child details, actual usage, and child references, and use the original
schema error as its summary.

The current `error result cannot be completed` guards for `Delegate` and
`DelegateSwarm` MUST be deleted. No compatibility branch retains that behavior.
Malformed canonical details, empty identifiers, non-terminal foreground
results, and invalid modes remain rejected.

`Delegate` MUST use the schema error as error content when schema acceptance
fails. It MUST NOT use the child's otherwise successful prose body as the error
summary.

## Structured Response Delivery

### Provider Request

Every initial child turn with `output_schema` MUST attach Neo's existing
provider-neutral response format to `RequestOptions`:

- a stable internal name;
- the exact child JSON Schema;
- strict mode enabled.

The single tools-disabled repair turn MUST attach the same response format.
This reuses the existing `ResponseFormat` type, provider mappings, and
`attach_response_format_hint` helper. It does not add a provider-specific
workflow path.

Providers that do not map response formats continue to receive prompt guidance.
Their returned assistant text still passes through the same strict host parser
and schema validator. Unsupported provider behavior must fail clearly; Neo does
not silently loosen parsing.

### Host Validation

The host remains authoritative:

1. Parse exactly one JSON value from assistant text.
2. Validate it against the compiled child schema.
3. On failure, run at most one tools-disabled repair turn.
4. Parse and validate the repaired value through the same functions.
5. Preserve the first raw value, repaired raw value, error, and actual usage in
   bounded typed details.

Provider response-format support improves production accuracy but never replaces
host validation.

### Child Prompt

The initial child prompt MUST include the actual compact output schema, not only
a generic statement that a schema exists. It MUST say, adjacent to the schema:

- return exactly one JSON value;
- do not use Markdown fences or surrounding prose;
- include every required field;
- do not call a tool to format the final response.

The repair prompt keeps the same rules and includes the validation error and
schema. Prompt text is guidance, not the safety mechanism.

## Workflow Authoring Inputs

`input_schema` and `output_schema` MUST be explicit for every inline-definition
action:

- `validate_inline`;
- `save`;
- `run_inline`.

The three actions use one definition shape. There is no action-specific default
or compatibility alias.

A workflow with no arguments uses this explicit schema:

```json
{"type":"object","additionalProperties":false}
```

A workflow that reads `neo.args` declares the corresponding properties and
required fields. Neo does not infer schema from `args`, examples, or Lua source.

Existing saved definitions remain readable and runnable exactly as stored. This
change affects new model tool calls only and performs no persistent migration.

## Model Guidance

Guidance changes use existing canonical surfaces and avoid duplicate prose.

### Workflow Tool Description

The tool description MUST state near its opening:

- every call begins with an explicit `action`;
- inline definition actions require both schemas;
- known saved workflows use `run_saved` and do not resend their definition;
- workflow task identifiers are read with `TaskOutput`.

The input schema remains the enforcement mechanism. The description is a short
first-call checklist, not a second specification.

### Create-Workflow Skill

The existing `create-workflow` skill MUST consolidate its guidance around one
canonical authoring checklist:

1. choose the action;
2. declare input and output schemas;
3. make Lua return exactly the declared result;
4. check every outcome's `ok` field;
5. use `neo.fail` when a required outcome failed;
6. use `{name, input}` for `neo.tool`;
7. put `output_schema` on every heterogeneous swarm item;
8. use `neo.json_array` and `neo.json_object` only as Lua table type markers.

The skill MUST include one no-argument schema example and one argument-bearing
schema example. Existing repeated or contradictory guidance MUST be replaced,
not appended as another long section.

### Runtime Child Guidance

Normal and repair child prompts share the same concise output rules. The runtime
MUST NOT inject workflow authoring instructions into ordinary children; it only
supplies the result schema and final-response requirements relevant to that
child.

## Built-In Workflows

`code-review`, `deep-research`, and `large-refactor` MUST explicitly branch on
every required child or verification outcome.

When a required outcome has `ok = false`, the script MUST call `neo.fail` with
the preserved outcome summary. It MUST NOT continue by manufacturing placeholder
findings, fallback summaries, merge decisions, or successful final results.

Obsolete placeholder-producing helpers and text MUST be deleted. Partial success
may be designed later only with an explicit user-visible partial-result state;
it is not introduced here.

## TUI Presentation

The existing Delegate-family cards remain unchanged.

The outer failed tool presentation MUST distinguish the two facts in plain
language when typed schema-error details are present:

```text
Failed Delegate
agent: completed
result: failed
reason: child result did not match the required format
```

The collapsed presentation MUST NOT show a bare `status: completed` beneath a
`Failed Delegate` title. Expanded details may include the precise validation
error needed for diagnosis. The model-visible tool result retains the precise
error code and message.

This is a typed projection from `is_error`, agent lifecycle, and schema-error
details. It never parses display text.

## Persistence And Compatibility

`WorkflowRuntime` remains the sole persistent owner of workflow invocation and
child result state. Session events, background tasks, and the TUI remain
projections.

No journal field, run metadata file, session event, or stored definition format
is rewritten. Existing records that contain `workflow_outcome_error` remain
readable historical facts. New runs stop producing the retired secondary error
for valid two-layer states.

## Retirement Decision

- Deletion class: internal code and model-input schema retirement.
- Delete first: the completed-plus-error rejection guards, omitted input schema
  default for inline authoring, and placeholder-success paths.
- Preserve: stored definitions, sessions, journals, task identifiers, strict
  parsing, one repair attempt, and Delegate-family cards.
- External compatibility exception: none.
- Persistent data deletion: none.

## Verification

Implementation must leave the smallest checks that prove each boundary.

### Core Result Mapping

A focused workflow-dispatch test constructs an error `Delegate` result with:

- completed agent lifecycle;
- `schema_error_code` and `schema_error`;
- actual usage;
- an agent identifier.

It asserts a failed workflow invocation, original schema error, preserved usage
and child reference, and no `workflow_outcome_error`. The same status rule is
covered for `DelegateSwarm` without duplicating all assertions.

### Provider Request Projection

A focused fake-model test captures the first child and repair requests. Both
must contain the exact strict response format and schema. A provider path that
omits wire support still reaches strict host validation.

### Built-In Failure

One deterministic built-in workflow test proves a required child failure makes
the workflow terminally failed and no placeholder successful result is recorded.
Source-level checks cover the other built-ins only where they prevent duplicated
expensive fixtures.

### Authoring Shape

Workflow tool schema tests assert that all three inline-definition actions
require `action`, `input_schema`, and `output_schema`. A production-path model
test proves the model-visible error names the missing field and the exact
required shape.

### Model Guidance

Focused source checks prove the Workflow tool description and
`create-workflow` skill each present one canonical authoring checklist, with no
retained contradictory examples. Request-capture tests prove normal and repair
child prompts both include the exact schema and the same concise final-response
rules.

### TUI Projection

One focused transcript test proves the failed tool presentation distinguishes
agent completion from result failure while the existing Delegate card snapshot
remains byte-for-byte unchanged.

### Live Acceptance

After deterministic checks pass, run one small saved workflow with one
schema-constrained child through the configured live provider. Evidence must
show first-turn schema acceptance, useful final output, no secondary canonical
error, and no placeholder result. This is provider-specific evidence, not proof
for every provider or operating system.

## Acceptance Criteria

1. A schema-constrained child request uses the existing strict response format
   on both normal and repair turns.
2. A completed child with invalid structured output produces one clear failed
   workflow result with the original reason.
3. Actual usage and child references survive that failure.
4. The TUI never presents bare `Failed Delegate` plus `status: completed`.
5. Built-in workflows cannot complete successfully when a required child
   outcome failed.
6. All inline-definition actions reject omitted `input_schema` before launch or
   save.
7. `output_schema` remains required.
8. Existing stored workflows and historical runs remain readable without
   migration.
9. No new tool, alias, parser mode, retry, persistence owner, or card design is
   introduced.
10. The Workflow tool, `create-workflow` skill, and both child prompt paths give
    the model one consistent schema-first path with no contradictory legacy
    guidance.

## Authority And Follow-Up

- Product baseline: the approved conversation and the observed usability run.
- Runtime baseline: `2026-07-26-workflow-platform-contract.md` and
  `2026-07-30-workflow-model-visible-results.md`.
- Architecture alignment: implementation drift in response-format wiring and
  result mapping; design defect in omitted authoring schema and placeholder
  success.
- Architecture decision signal: yes. After implementation and verification,
  update ADR-0008 and the workflow model-visible-results baseline to record the
  explicit input schema and two-layer result semantics.
