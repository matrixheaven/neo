# Neo Workflow AI Usability Repair Design

Date: `2026-08-01`

Status: `approved by the user on 2026-08-01`

> **Entire document superseded on 2026-08-03.** See
> `docs/aegis/specs/2026-08-03-workflow-output-reliability-design.md`.
> Do not execute any output-enforcement rule below. Its mandatory child output
> schema, one hidden repair turn, fail-closed projection behavior, and mixed
> host/business `ok` semantics are retired.

This file is retained only as historical evidence for the 2026-08-01
investigation. It is not a current implementation authority.

## Problem

The fourth AI usability report groups thirteen labels under "unfixed", but the
group mixes three different classes:

1. real product defects that prevent workflow children from running or hide the
   real failure;
2. model-guidance gaps that make correct strict behavior look accidental;
3. approved strict behavior that must remain unchanged.

The primary product failure is deterministic. Provider type `openai` represents
both OpenAI Chat Completions and arbitrary compatible endpoints. The shared
wire client currently serializes a JSON Schema `response_format` whenever a
child has `output_schema`. Four configured compatible endpoints reject that
field before model execution, so every tested child reports zero tokens and
zero tools.

The failure is then misclassified. A failed child turn is passed to the
structured-output validator as though it were assistant output. The validator
starts a second model turn, that request fails for the same protocol reason,
and the original HTTP error is replaced by `strict_json_failed`. Swarm summary
projection then reports only item counts, so the first actionable child error
is available only in the journal.

The remaining first-use failures are guidance failures. The existing
`create-workflow` skill already owns authoring guidance, but several closed
behaviors are either implicit, too far from the checklist, or contradicted by
the standalone-file documentation.

## Goals

- Let schema-constrained workflow children run on arbitrary `type = "openai"`
  compatible endpoints without sending an unproven native field.
- Keep native JSON Schema response formatting for the known
  `openai_response` wire.
- Keep one strict host parser, one JSON Schema validator, and at most one
  tools-disabled content-repair turn.
- Never start schema repair when the child turn itself failed.
- Preserve the original provider, authentication, rate-limit, cancellation, or
  runtime error as the workflow outcome summary.
- Make a failed swarm summary expose failure count and the first bounded child
  error without loading the journal.
- Make final-result schema failures identify the failing path and a bounded
  preview of the failing value.
- Teach every intentional strict behavior before a model authors a workflow.
- Repair the known `echo-test` user definition without changing its Lua source
  or any other user workflow.

## Non-Goals

- No provider capability setting, feature probe, endpoint allowlist, error-text
  matching, retry-on-HTTP-400, or automatic protocol fallback.
- No permissive or optional `input_schema` or `output_schema` for inline
  definitions.
- No schema inference from Lua, arguments, examples, or model output.
- No flat `neo.tool` alias, top-level swarm schema, `neo.json_array(nil)`
  normalization, catchable `neo.fail`, or workflow support in `WaitDelegate`.
- No second parser, validator, repair loop, request builder, workflow runtime,
  result channel, prompt owner, or documentation surface.
- No fenced-JSON extraction, prose scanning, fuzzy parsing, or more than one
  repair turn.
- No changes to Delegate, DelegateGroup, or DelegateSwarm card layout,
  expansion, activity, ordering, grouping, or transcript placement.
- No session, journal, artifact, or saved-definition migration.
- No automatic deletion of `~/.neo/workflows/echo-test` or any other user data.
- No broad provider-capability redesign.

## First-Principles Decision

- Required outcome: a workflow child either produces host-validated JSON or
  fails once with the original actionable reason.
- Non-negotiable constraints: provider-neutral workflow behavior, strict host
  validation, one repair turn, one authoring-guidance owner, and unchanged
  persistence and Delegate-family presentation.
- Assumption removed: an OpenAI-compatible endpoint does not necessarily
  support OpenAI's JSON Schema response format.
- Smallest sufficient path: stop serializing the optional hint on the ambiguous
  compatible wire, gate schema validation on a successful child turn, improve
  the existing bounded summaries, and strengthen the existing authoring skill.

## Issue Classification

| Report item | Decision | Required action |
| --- | --- | --- |
| `output_schema` required | preserve | Teach that it is intentional and show the exact no-argument schema. |
| inline `input_schema` required | preserve | Distinguish inline tool calls from historical paired files. |
| broken `echo-test` | repair user definition | Add required string field `text`; keep Lua and source hash unchanged. |
| `neo.json_array(nil)` fails | preserve | Teach that markers require a table and return a marked table. |
| flat `neo.tool` input fails | preserve | Teach the sole `{ name, input }` shape and decode-failure boundary. |
| `neo.fail` cannot be caught | preserve | State explicitly that `pcall` cannot recover the run. |
| child requests fail before tokens | repair source | Omit the native hint on the ambiguous compatible wire. |
| `neo.await_user` return unclear | repair guidance | State that it returns the raw read-only answer value. |
| `neo.report` return unclear | repair guidance | State that it returns no value and is statement-only. |
| final schema error lacks context | repair diagnostics | Include instance path and bounded failing-value preview. |
| tool decode failure unclear | repair guidance | Separate call-shape failure from branchable execution failure. |
| `WaitDelegate` rejects workflow IDs | preserve | Keep `TaskOutput` as the only workflow read/wait path. |
| workflow task waiting path | preserve | Teach `TaskOutput`; add no alias. |

## Implementation Control

This design is closed for implementation. The implementer must execute the
selected path rather than reopen provider capability discovery, reinterpret the
thirteen report labels, or replace preserved strict behavior with convenience
aliases.

The following rules control any ambiguity encountered during implementation:

1. Current source is evidence for locating the named owners; it does not
   authorize a different product decision.
2. A completed child turn is successful when its lifecycle is `Completed` and
   its optional terminal outcome is not an error. Absence of a terminal outcome
   alone is not a failure.
3. Both existing schema-acceptance consumers must apply that same success
   predicate before parsing or repair: foreground Delegate and direct workflow
   swarm children.
4. A source mismatch may justify a bounded symbol/caller trace and an updated
   line number. It does not justify a whole-repository survey or architecture
   redesign.
5. Any apparent need for configuration, endpoint probing, error-text matching,
   protocol retry, parser loosening, another guidance owner, persistence
   migration, or Delegate-family presentation changes is design drift. Stop and
   return the exact conflict to the original reviewer.
6. Do not edit a file outside the implementation plan merely because it is
   nearby. First prove that an acceptance criterion cannot be met at the named
   owner, then stop for review before widening scope.
7. Focused tests and individual provider runs prove only their named paths. They
   never establish remote CI, every endpoint, or native Windows/Linux support.

## Selected Runtime Design

### Provider Wire Mapping

`RequestOptions.response_format` remains the provider-neutral internal hint.
Child initial and repair requests continue to carry the exact schema in that
field so request capture, the OpenAI Responses wire, and future known native
wires can use it.

Wire behavior becomes:

| Provider type | Wire behavior |
| --- | --- |
| `openai_response` | Map the hint to `text.format` JSON Schema. |
| `openai` | Omit the hint from Chat Completions-compatible request bodies. |
| `anthropic` | Continue omitting the hint. |
| `google` | Continue omitting the hint. |

This is not a runtime fallback. Each provider wire has one deterministic
request shape. Compatible endpoints still receive the exact schema and JSON-only
rules in the child prompt, and all returned text still passes through the same
strict host validator.

The `openai` wire may include official Chat Completions endpoints that support
JSON Schema. They intentionally use the prompt plus host-validation path
because the provider type cannot distinguish that capability from arbitrary
compatible endpoints. Correct universal behavior is preferred over an
unverifiable optional optimization.

### Failed Child Gate

Structured-output acceptance applies only after the child model turn reached a
successful completed outcome.

For direct Delegate and direct workflow swarm children:

1. derive the ordinary child outcome first;
2. if that outcome is not successful, return it unchanged except for actual
   usage already observed;
3. do not parse the provider error as JSON;
4. do not append `SchemaRepairStarted`;
5. do not issue a repair request;
6. preserve the original child summary and typed child details.

Only a successful child turn whose assistant text is invalid JSON or fails the
declared schema may enter the existing one-repair path.

### Swarm Failure Summary

The workflow swarm outcome retains complete per-item details. Its bounded
summary becomes state-aware:

- all succeeded: retain total and finished counts;
- any failed: include failed count, total count, and the first failed item's
  bounded summary;
- interrupted without a failed item: include finished count and interrupted
  state.

The summary reuses the existing 160-character workflow summary bound. It does
not copy full child output or add per-item text to the normal model result.

### Final Result Schema Diagnostics

`SchemaValidationError` already retains `instance_path`. Final-result mapping
will add:

- `<root>` or the JSON Pointer instance path;
- a JSON serialization of the failing node when resolvable;
- a 160-character Unicode-safe preview with an explicit ellipsis when cut.

Example:

```text
schema_invalid_final_result at /name: "a" is shorter than 3 characters; actual="a"
```

The complete invalid result is never copied into the error. Validation remains
terminal and never starts a model repair turn.

## Selected Model Guidance

### Canonical Owner

`crates/neo-agent-core/src/skills/builtin/create-workflow.md` remains the sole
detailed model-facing authoring guide. The Workflow tool description continues
to route creation, change, adaptation, evaluation, and one-off authoring to
that skill. No global system-prompt section and no second authoring skill are
added.

This avoids paying specialized workflow guidance on unrelated turns while
still exposing it before authoring.

### Closed Decisions Block

Move the following compact rules next to the existing authoring checklist and
mirror them in the host API section:

1. Inline `validate_inline`, `save`, and `run_inline` always require explicit
   `input_schema` and `output_schema`; omission is not a retry strategy.
2. A no-argument inline workflow uses
   `{"type":"object","additionalProperties":false}`.
3. `neo.tool` accepts only `{ name = "ToolName", input = { ... } }`; call-shape
   decode errors abort unless ordinary Lua error handling catches them, while a
   tool execution failure returns `ok = false`.
4. `neo.json_array` and `neo.json_object` require tables, return marked tables,
   and do not serialize to strings. `nil` is invalid.
5. `neo.fail` is a terminal run decision. `pcall` cannot undo or recover it.
6. `neo.await_user` returns the raw read-only answer value, not an outcome
   table.
7. `neo.report` records an intermediate report and returns no value. Use it as
   a statement.
8. Workflow task IDs are read and waited through `TaskOutput`; never pass them
   to `WaitDelegate`.

The existing authoring example remains canonical. Repeated or contradictory
sentences are replaced, not followed by another competing section.

### Human Documentation

Both language guides mirror the same behavior. The paired-file manifest
section must state that omitted `input_schema` applies only to paired saved
definitions, while inline Workflow tool actions require an explicit schema.
The "effect outcomes" section must exclude `neo.await_user` and `neo.report`
from the common outcome-table statement.

## User Definition Repair

The known persisted definition is:

```text
~/.neo/workflows/echo-test.lua
~/.neo/workflows/echo-test.workflow.toml
```

The Lua source reads `neo.args.text`; the manifest has no `input_schema`. Add
only:

```toml
[input_schema]
type = "object"
additionalProperties = false
required = ["text"]

[input_schema.properties.text]
type = "string"
minLength = 1
```

The Lua bytes and `source_sha256` remain unchanged. No other user workflow may
be edited, deleted, moved, or normalized. This exact repair is user-approved
by approval of this design; deletion remains unapproved.

## Persistence And Compatibility

- No stored run, session, journal, artifact, or definition format changes.
- Historical definitions without `input_schema` remain readable and runnable.
- New inline authoring remains explicit-schema-only.
- `RequestOptions.response_format` remains available to known native wires.
- The `openai` compatible request body loses only an optional field that cannot
  be assumed supported.
- Child prompt and strict host validation remain identical across providers.
- Existing schema-repair journal records remain readable.
- No Delegate-family event or card shape changes.

## Retirement Boundary

Delete the `response_format` serialization branch and its positive mapping test
from `OpenAiCompatibleClient`. Replace the test with an omission assertion.

Do not retain:

- a configuration switch;
- an endpoint list;
- a protocol-error retry;
- the old compatible-wire mapping behind a branch;
- duplicated prompt guidance;
- any alias for the strict Lua APIs.

## Verification

Focused deterministic proof must show:

1. `openai` compatible request bodies omit `response_format` even when the
   internal request option is present.
2. `openai_response` request bodies still map the exact schema.
3. A failed child turn causes no repair request and no schema-repair journal
   event; its original error and actual usage survive.
4. Successful invalid child text still receives exactly one tools-disabled
   repair and then succeeds or fails schema validation truthfully.
5. A failed swarm summary contains failure count and first bounded error while
   retaining ordered item details.
6. A final-result schema error contains instance path and a bounded failing
   value, including Unicode-safe truncation.
7. The built-in authoring skill contains every closed decision and no retired
   guidance.
8. Both language guides agree on return shapes and inline-versus-paired schema
   behavior.
9. The repaired `echo-test` validates and succeeds with `{"text":"hello"}`.

After deterministic proof, run one minimal one-child workflow on the active
report-004 endpoint and on one representative OpenAI-compatible endpoint.
Evidence must distinguish:

- request reached the model (`token_count > 0` or equivalent provider usage);
- first-turn acceptance versus one content-repair turn;
- valid structured result;
- absence of protocol-field failure;
- exact blocker when credentials, network, or provider availability prevents a
  run.

One representative endpoint must also run the shipped `code-review` workflow
over a small read-only scope. Provider-specific success is not evidence for all
providers or for native Windows/Linux behavior.

## Acceptance Criteria

1. The active report-004 endpoint and a representative OpenAI-compatible
   endpoint no longer receive the unsupported `response_format` field from
   Neo's `openai` wire.
2. Provider or runtime child failure is never reclassified as a schema failure
   and never starts schema repair.
3. Invalid successful assistant text retains strict parsing and at most one
   tools-disabled repair.
4. Direct and swarm workflow outcomes expose the first actionable failure in a
   bounded model-visible summary.
5. Final-result schema failures expose path and bounded actual-node context
   without copying the complete result.
6. Models are explicitly taught all preserved strict behaviors before
   authoring through the existing `create-workflow` skill.
7. Inline schemas remain explicit; flat tool arguments, nil markers, catchable
   terminal failure, and wait-tool aliases remain rejected.
8. `echo-test` succeeds with a valid `text` argument and no unrelated user
   workflow changes.
9. No new dependency, setting, retry, parser, runtime, persistence format,
   authoring surface, or Delegate-family presentation change is introduced.
10. Implementation evidence is returned for an independent final review; the
    implementer does not claim that focused or provider-specific proof covers
    remote CI or every operating system.

## Decision And Baseline Follow-Up

This is an architecture-significant correction to an earlier provider
assumption. After implementation and verification:

- amend `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md` with the
  selected wire behavior and rejected alternatives;
- update `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md` with
  the landed behavior and exact evidence;
- do not rewrite the historical design or claim that its live-provider
  acceptance was stronger than recorded.
