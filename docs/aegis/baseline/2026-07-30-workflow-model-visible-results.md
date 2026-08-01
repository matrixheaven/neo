# Workflow Model-Visible Results — Landed Baseline

Status: `recorded-from-work`
Date: `2026-07-30`
Updated: `2026-08-01`
ADR: `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`

This baseline records the model-visible Workflow and TaskOutput projections
and recoverable ordinary host outcomes implemented by:

- `41f5e47` — explicit action inputs and strict no-argument default;
- `81f7a52` — Workflow action results in model-visible content;
- `2afdabf` — bounded TaskOutput pages in model-visible content;
- `6b79945` — recoverable Lua host failures;
- `e4e2bc2` and `73a7e18` — production-chain model visibility acceptance.
- `8b626877`, `56776764`, `45073370`, `9a09f7c1`, `a7e7f93`, and `42456d33` —
  strict child response formats, truthful failed-result semantics, explicit
  schemas, fail-closed built-ins, typed outer failure presentation, and
  formatting closure.

A post-landing review then corrected saved-definition input validation,
model-visible list privacy, complete-result byte accounting and cursors, TUI
human summaries, configuration limit alignment, and authoring documentation.

## Landed behavior

- One `Workflow` tool still exposes exactly seven actions.
- Inline definitions require explicit `input_schema` and `output_schema`.
  A no-argument inline workflow uses the explicit object schema
  `{"type":"object","additionalProperties":false}`. Historical saved
  definitions may omit `input_schema` and remain readable and runnable without
  migration.
- Workflow action content contains the fields required for the next model
  decision, including task IDs and the exact `TaskOutput` next action.
- `TaskOutput` content contains the requested bounded result, journal, artifact,
  or pending-input page. The complete content plus typed details remains within
  the existing byte limit; artifact cursors continue through the final page.
- Workflow list content excludes source paths, internal origins, and revision
  identifiers. TUI tool cards derive human summaries from typed details instead
  of displaying compact model JSON.
- Failed verification, failed command verification, unknown tools, and denied
  workflow-ineligible tools return branchable `ok = false` outcomes.
- `neo.fail`, uncaught Lua errors, resource exhaustion, cancellation, and
  invalid final results remain terminal failures.
- Schema-constrained child requests use the exact strict provider-neutral
  response format on the initial and single repair turn. Strict host parsing
  rejects fenced JSON, prose, and multiple values.
- A completed child with an unaccepted structured result remains lifecycle
  `completed`, while Delegate, the workflow invocation, and the workflow child
  result are `failed`. The original `schema_error`, error code, actual usage,
  and child references are preserved; no `workflow_outcome_error` is added.
- Required built-in child and verification failures terminate with `neo.fail`
  and never produce placeholder success. The typed outer failure projection
  explains the two-layer state without changing Delegate-family cards.
- Workflow task IDs use `TaskOutput`; `WaitDelegate` remains delegate/swarm-only.

## Focused evidence

The production acceptance runs a real `AgentRuntime` turn through
`Workflow(run_inline)`, the shared `WorkflowDispatchResolver` and
`WorkflowRuntime`, Lua execution, task completion, `TaskOutput(result)`, and
the next model request. It asserts the launch task ID, exact TaskOutput call,
replayed launch result, three ordinary failed outcomes, completed status, and
the actual final JSON in the next request's tool content.

Fresh post-review evidence passed for the exact production chain, dynamic and
paired no-argument validation, strict child response-format and repair
requests, failed-child correlation, explicit inline schema validation, the
built-in fail-closed paths, the typed TUI failure projection, Workflow
schema/list projection, inline and artifact-backed TaskOutput pages,
multi-page journal continuity, artifact cursor continuity with JSON-escaped
content, UTF-8-safe summaries, final-result artifact promotion, configuration
limit alignment, TUI human summaries, and the built-in authoring skill.
`rustfmt --check --edition 2024`, the touched-file diff check, and
`cargo build -p neo-agent --bin neo` passed. Repository-wide `git diff --check`
still reports a trailing space in the unrelated user edit
`docs/en/configuration/config-files.md`.

## 2026-08-01 update: provider-safe child output

The Workflow AI usability repair landed (design
`docs/aegis/specs/2026-08-01-workflow-ai-usability-repair-design.md`; commits
`c4051804`, `3ade380c`, `b6dfb8bc`, `9ef9a7b`, `ea7d7b8`, and this docs
commit `docs(workflow): record provider-safe child output`). It corrects the
earlier assumption that the `openai` compatible wire could serialize a native
JSON Schema response hint (ADR-0008 amendment, 2026-08-01).

Landed behavior added by the repair:

- The `openai` compatible request body deterministically omits
  `response_format`; `openai_response` still maps the exact schema to native
  `text.format` JSON Schema. Anthropic and Google continue to omit the hint.
- A failed child turn (provider, auth, rate-limit, cancellation, runtime)
  bypasses schema parsing and the content-repair path in both foreground
  Delegate and direct workflow swarm consumers; the original error and
  observed usage survive, and no schema-repair journal event is written.
- A failed swarm summary exposes `failed <failed>/<total>` plus the first
  failed child in input order while ordered item details stay complete; the
  entire summary is bounded to 160 characters. Paused and cancelled items
  remain interruptions and are not counted as failures.
- Final-result schema failures include the instance path (`<root>` when empty)
  and a Unicode-safe preview of the failing node: values up to 160 characters
  remain intact, while longer values become 159 characters plus `…`.
- The built-in `create-workflow` skill and both language guides teach every
  preserved strict behavior (explicit inline schemas, `{name,input}` tool
  shape, marked tables, terminal `neo.fail`, raw `await_user` answer,
  statement-only `neo.report`, `TaskOutput`-only workflow task reads).
- The user definition `echo-test` gained its approved `input_schema` (Lua
  bytes and `source_sha256` unchanged) and now succeeds with
  `{"text":"hello"}` → `{"echo":"hello","ok":true}`.

Exact focused verification (each names one package, one target, one filter):

- `cargo nextest run -p neo-ai --lib response_format_is_omitted_for_compatible_chat_endpoints` — passed
- `cargo nextest run -p neo-ai --lib response_format_maps_json_schema_for_responses_api` — passed
- `cargo nextest run -p neo-agent-core --test workflow_schema workflow_delegate_failure_skips_schema_repair_and_preserves_error` — passed
- `cargo nextest run -p neo-agent-core --test workflow_schema workflow_swarm_failure_skips_schema_repair_and_preserves_error` — passed
- `cargo nextest run -p neo-agent-core --test workflow_schema child_schema_invalid_output_gets_exactly_one_tools_disabled_repair` — passed
- `cargo nextest run -p neo-agent-core --test workflow_schema workflow_swarm_invalid_output_gets_exactly_one_tools_disabled_repair` — passed
- `cargo nextest run -p neo-agent-core --test workflow_lua workflow_swarm_failure_summary_includes_first_bounded_error` — passed
- `cargo nextest run -p neo-agent-core --test workflow_lua workflow_swarm_pause_and_cancellation_are_not_reported_as_failure` — passed
- `cargo nextest run -p neo-agent-core --test workflow_schema final_lua_schema_error_includes_path_and_bounded_actual` — passed
- `cargo nextest run -p neo-agent-core --lib create_workflow_builtin_teaches_authoring_without_mandatory_choreography` — passed
- `rustfmt --check --edition 2024` on every touched Rust file — passed
- `git diff --check` — passed for every task

Live evidence (2026-08-01). Isolated temporary `NEO_HOME=/tmp/neo-accept/home`
with per-provider temporary configs referencing `api_key_env`; no persisted
secrets; the user's persistent default config and default model were never
mutated:

| Provider | Resolved type | Reached model | Repair count | Valid result | Blocker |
| --- | --- | --- | --- | --- | --- |
| kimi-for-coding | anthropic | yes (8245 in / 53 out) | 0 | `{"child_ok":true,"ok":true}` | — |
| deepseek (real API, extra) | openai | yes (9000 in / 5 out) | 0 | `{"child_ok":true,"ok":true}` | — |

Shipped `code-review` ran on the deepseek compatible endpoint over
`crates/neo-agent-core/src/workflow/schema.rs`: completed, 4 delegate
children, 452052 input / 17747 output tokens, exactly one content-repair turn
per child, structured findings returned. Every live run recorded zero
schema-repair events for failed children, and no run produced the old
`response_format` HTTP 400. Provider-specific success proves only that
provider and request; remote CI and native Windows/Linux behavior remain
release-level verification.

## Preserved boundaries and residual risk

- Journal, task registration, workflow card layout, completion notifications,
  replay, artifact storage, and runtime ownership are unchanged.
- No second tool, result channel, registry, retry, alias, or compatibility
  branch was added. The summary-only formatter and Lua failure wrappers were
  retired.
- The retained live-provider attempt used temporary workspace
  `/tmp/neo-workflow-smoke.iFn3r1`: the final saved run completed with
  `{"echoed":"hello"}`, but the provider first generated an invalid
  `neo.delegate` argument shape and then rewrote the workflow before the
  successful run. This is partial live evidence, not clean first-turn
  acceptance; no provider-wide claim is made.
- The provider session selected `user` scope while saving `schema-smoke`, so
  the workflow files were written under `~/.neo/workflows/` despite the
  temporary workspace. Native Windows/Linux behavior and clean provider
  acceptance remain release-level verification work.
