# Workflow Model-Visible Results — Landed Baseline

Status: `recorded-from-work`
Date: `2026-07-30`
ADR: `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`

This baseline records the model-visible Workflow and TaskOutput projections
and recoverable ordinary host outcomes implemented by:

- `41f5e47` — explicit action inputs and strict no-argument default;
- `81f7a52` — Workflow action results in model-visible content;
- `2afdabf` — bounded TaskOutput pages in model-visible content;
- `6b79945` — recoverable Lua host failures;
- `e4e2bc2` and `73a7e18` — production-chain model visibility acceptance.

A post-landing review then corrected saved-definition input validation,
model-visible list privacy, complete-result byte accounting and cursors, TUI
human summaries, configuration limit alignment, and authoring documentation.

## Landed behavior

- One `Workflow` tool still exposes exactly seven actions.
- Omitted `input_schema` means the workflow accepts no arguments; `output_schema`
  remains required. Dynamic and paired saved definitions share this behavior.
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
- Workflow task IDs use `TaskOutput`; `WaitDelegate` remains delegate/swarm-only.

## Focused evidence

The production acceptance runs a real `AgentRuntime` turn through
`Workflow(run_inline)`, the shared `WorkflowDispatchResolver` and
`WorkflowRuntime`, Lua execution, task completion, `TaskOutput(result)`, and
the next model request. It asserts the launch task ID, exact TaskOutput call,
replayed launch result, three ordinary failed outcomes, completed status, and
the actual final JSON in the next request's tool content.

Fresh post-review evidence passed for the exact production chain, dynamic and
paired no-argument validation, Workflow schema/list projection, inline and
artifact-backed TaskOutput pages, multi-page journal continuity, artifact
cursor continuity with JSON-escaped content, UTF-8-safe summaries, final-result
artifact promotion, configuration limit alignment, TUI human summaries, and
the built-in authoring skill. Formatting and `git diff --check` also passed.

## Preserved boundaries and residual risk

- Journal, task registration, workflow card layout, completion notifications,
  replay, artifact storage, and runtime ownership are unchanged.
- No second tool, result channel, registry, retry, alias, or compatibility
  branch was added. The summary-only formatter and Lua failure wrappers were
  retired.
- Focused local tests do not prove live provider acceptance of the action
  branches or native Windows/Linux behavior. Those remain release-level
  verification work.
