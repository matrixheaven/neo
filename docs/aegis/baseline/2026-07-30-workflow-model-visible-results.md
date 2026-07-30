# Workflow Model-Visible Results — Landed Baseline

Status: `recorded-from-work`
Date: `2026-07-30`
Updated: `2026-07-31`
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
