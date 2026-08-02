# Workflow Output Reliability Design

Date: `2026-08-03`

Status: `approved direction; implementation pending`

This design supersedes the output-execution rules in:

- `docs/aegis/specs/2026-08-01-workflow-ai-usability-repair-design.md`
- `docs/aegis/plans/2026-08-01-workflow-ai-usability-repair.md`
- `docs/aegis/handoffs/2026-08-01-workflow-ai-usability-repair.md`

The older documents remain useful as historical evidence, but their strict
child-output gate, hidden repair turn, and fail-closed built-in behavior are
retired by this document.

## Decision In One Sentence

Workflow execution is judged by whether the child actually ran; a declared
output schema is only a best-effort projection of the returned data and can
never turn a completed child into an execution failure.

## Problem

Neo currently combines three unrelated operations:

1. asking a model to return a particular shape through prompt guidance;
2. mechanically parsing and validating the returned text against JSON Schema;
3. deciding whether the child or the whole Workflow executed successfully.

The model may ignore or extend the requested shape. That is normal model
behavior, not proof that the provider call failed. The current implementation
turns a projection mismatch into `schema_invalid`, spends another model turn
trying to repair it, and can then call `neo.fail`. A Workflow can therefore
consume a large child run and still be discarded solely because a field such as
`claim_note` was not listed in a prompt-owned schema.

The same ambiguity exists at the top level. `WorkflowInvocationOutcome.ok`
currently carries both host execution state and business judgments such as
whether evidence was verified. Built-in scripts then treat a false business
answer as a terminal host failure.

## Why A Schema Exists, And Why It Must Not Be The Judge

JSON Schema still has one useful job: when a caller wants structured data, the
host can attempt a deterministic projection so later code does not have to
guess field names. The schema itself is host data, while the model-facing
instruction generated from it is only guidance. Neither can make an AI output
reliable by force.

The schema therefore has this boundary:

- malformed schema documents are local definition/input errors and are reported
  before an expensive provider run when the definition is checked;
- a valid schema that does not match model text means the structured projection
  is unavailable;
- the original text, child lifecycle, observed usage, and provider error remain
  the real execution record;
- only execution failures, cancellation, resource limits, permission failures,
  host-channel failures, and explicit script failures can fail the Workflow.

This keeps a useful parser without pretending that mechanical field checking is
semantic understanding.

## Goals

- Never discard a completed child because its optional structured projection is
  missing, malformed, or contains extra fields.
- Never start a hidden model repair turn for a projection mismatch.
- Preserve the original child text/result, lifecycle, provider error, and actual
  token usage.
- Make `WorkflowInvocationOutcome.status` the only host execution state.
- Keep business judgments such as `verified`, `supported`, `contradictions`,
  `gaps`, and `partial` inside result data.
- Let research and review Workflows produce deterministic partial results when
  evidence is incomplete or a projection is unavailable.
- Keep the existing Lua `WorkflowRuntime`, journal, recovery, task reading,
  and Delegate-family presentation owners.
- Keep input and definition validation where it protects a host boundary.

## Non-Goals

- No rewrite from Lua to Rhai.
- No provider capability probing, response-format retry, endpoint allowlist, or
  error-text guessing.
- No fuzzy JSON extraction, regular-expression replacement for semantic
  decisions, or second parser.
- No automatic schema repair prompt, repair retry, or extra model call.
- No new Workflow runtime, result channel, persistence format, or transcript
  card type.
- No change to `Ctrl+O`; its alternate-screen return fix is already present in
  the current worktree and is outside this Workflow output change.
- No change to non-Workflow Delegate, DelegateGroup, or DelegateSwarm cards.

## First-Principles Decision

- **Irreducible outcome:** a Workflow must retain useful work after a child has
  actually run.
- **Non-negotiable constraints:** preserve raw evidence and usage, keep host
  failures visible, avoid extra provider cost, and keep one execution owner.
- **Assumption deleted:** prompt compliance is not a dependable transport
  protocol for an AI model.
- **Smallest sufficient path:** separate execution status from result
  projection, delete the repair path, and make built-ins degrade to explicit
  partial data.
- **Falsifier:** if a provider or host failure can be silently reported as a
  completed child, this design is wrong and must be stopped before release.

## Runtime Semantics

### Child execution

Every child has one execution result first. A child is execution-successful
when its lifecycle reaches `Completed` without a provider/runtime error. The
following remain execution failures:

- provider, authentication, rate-limit, or transport failure;
- cancellation or resource exhaustion;
- permission denial or child runtime failure;
- host channel/journal failure;
- explicit `neo.fail` or an uncaught Workflow script error.

The child result is never reclassified because its text does not match a
declared output schema.

### Optional child projection

`output_schema` on `neo.delegate` and `neo.swarm` children becomes optional.

- Omitted schema: return the ordinary child result without structured output.
- Present valid schema: parse the returned assistant text once and attach
  `details.structured_output` only when it matches.
- Parse or validation mismatch: keep the child `Completed`, set a bounded
  projection status/error in details, and continue with no second model turn.
- Provider-native structured hints remain optional optimizations of the wire;
  they do not replace host retention of the ordinary result.

The structured projection is not copied into the ordinary parent summary when
it would inflate model-visible output. The existing child result and journal
remain the source for the full text and actual usage.

### Final Workflow result

The final Lua return value is persisted exactly as returned. A declared final
`output_schema` may be retained as definition metadata and used for an optional
projection/diagnostic, but a mismatch never prevents persistence or changes a
completed Workflow into `schema_invalid`.

Existing definitions remain readable. The field may remain in persisted
manifests for that reason, but no new execution path may treat it as a required
AI-output promise.

### Execution status and business data

Retire `WorkflowInvocationOutcome.ok` as the host verdict. `status` carries
execution state only. The Lua outcome table exposes `status`, `summary`,
`details`, usage, and child references; it does not expose a second host
boolean with overlapping meaning.

`neo.verify(condition, message)` returns a completed host outcome whose details
contain the business result, for example `verified = false` and the message.
It does not abort the script. `neo.fail(message)` remains an explicit terminal
script decision and is reserved for actual required execution failures or
explicit user/script policy, not for missing structured projection.

## Built-in Workflow Behavior

The built-ins remain ordinary Lua definitions and keep their current tools,
roles, phases, and artifacts.

- `deep-research` treats child execution failures as failures, but treats
  unavailable structured findings, contradictions, gaps, and verifier
  uncertainty as `partial` data.
- `code-review` and `large-refactor` use `status` to detect child execution
  failure. Missing structured child data produces a bounded partial result and
  does not call `neo.fail` merely because a schema projection was unavailable.
- Verification booleans live in result data as `verified`, `supported`, or the
  existing domain field. They are never read from the host execution status.
- Every built-in has a deterministic fallback result/report that names the
  missing evidence or projection and preserves completed work.

The final result may include a domain-level `status` such as `verified`,
`partial`, or `failed`; this field is data owned by the Workflow and is not the
host execution status.

## Compatibility Boundary

- Keep the existing `WorkflowRuntime`, Lua host API, journal, session replay,
  task IDs, and Delegate-family presentation.
- Keep input schema validation and malformed definition rejection at the local
  boundary; they happen before provider work.
- Keep old persisted `ok` and schema-repair journal fields readable if required
  by existing session replay, but never write or dispatch the retired behavior.
- Keep output schema fields readable in saved definitions; they no longer gate
  child or final execution.
- Do not add a second result model or compatibility runtime.
- Update current built-in fixtures and authoring guidance to the new semantics.

## Explicit Retirements

The implementation must delete or stop using these paths:

1. mandatory child `output_schema` checks in `agent` and `swarm` lowering;
2. the hidden one-turn `run_schema_repair_turn` flow and its new journal writes;
3. schema projection failure mapped to `WorkflowInvocationOutcome.status = Failed`;
4. built-in `neo.fail` calls caused only by projection/schema mismatch;
5. final Lua return validation as a terminal execution gate;
6. top-level `ok` as a mixture of host execution and business truth;
7. the old usability-repair guidance that tells models these rules are strict
   child execution requirements.

## Acceptance Criteria

1. A child returning valid JSON plus an undeclared `claim_note` remains
   `Completed`, consumes exactly one provider turn, preserves usage, and keeps
   its ordinary text/result available.
2. A child returning prose or invalid JSON remains `Completed` when the child
   turn itself completed; only the structured projection is unavailable.
3. A provider/runtime error remains a failed child with its original error and
   observed usage, with no projection attempt and no repair request.
4. A swarm with mixed successful, failed, and projection-unavailable children
   preserves per-item state and reports a bounded partial result.
5. A final Lua value that does not match the declared output schema is still
   persisted and the Workflow remains completed.
6. `neo.verify(false, ...)` returns completed execution data with
   `details.verified = false` and does not abort Lua.
7. `deep-research` produces a deterministic partial report when verification
   returns contradictions, gaps, or no structured projection, and only fails
   for an actual child/host failure.
8. No new repair request or `SchemaRepairStarted` write occurs on a projection
   mismatch.
9. Historical sessions containing old `ok` or repair records remain readable,
   while new runs do not create the retired behavior.
10. The `Ctrl+O` alternate-screen behavior and all non-Workflow Delegate-family
    cards remain unchanged.

## Verification Boundary

Focused local tests must cover the exact child, swarm, final-result, Lua
verification, and built-in fallback paths. Formatting and whitespace checks are
required. These checks prove the named macOS worktree paths only; they do not
prove remote CI, every provider, or native Windows/Linux behavior.
