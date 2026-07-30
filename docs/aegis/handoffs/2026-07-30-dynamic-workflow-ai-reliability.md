# Handoff Prompt: Implement Dynamic Workflow AI Reliability

Copy everything below the separator into the implementation task unchanged.

---

You are implementing the approved Neo dynamic-workflow AI reliability work in:

```text
/Users/chenyuanhao/Workspace/neo
```

The design is approved and closed. Execute the implementation plan in order.
Do not restart product discovery, repeat a whole-repository survey, reopen the
decisions, or substitute a different architecture. Read the listed authority
and only the targeted current source needed for each task.

## 1. Read Authority In This Order

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-07-30-dynamic-workflow-ai-reliability-design.md`
5. `docs/aegis/plans/2026-07-30-dynamic-workflow-ai-reliability.md`
6. this handoff
7. `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
8. `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md`
9. `docs/aegis/baseline/2026-07-26-workflow-platform-contract.md`

Authority rules:

- approved design owns behavior and non-goals;
- implementation plan owns order, files, tests, retirement, and commits;
- this handoff owns execution discipline and final evidence;
- current code is evidence, not permission to weaken approved behavior;
- historical reports and `.references/` are not implementation authority.

Known design commit:

```text
efc528b1 docs(workflow): design AI reliability improvements
```

Confirm the planning commit with `git log -5 --oneline`; do not guess it.

Before source edits:

```bash
icm recall-context "dynamic workflow ResponseFormat Delegate completed failed schema prompt reliability" --limit 5
git status --short
git log -5 --oneline
```

The worktree is shared. Every pre-existing dirty or untracked path belongs to
the user or another task. Never revert, restore, stash, clean, overwrite, stage,
or commit unrelated paths. Forbidden Git operations include `reset`,
`checkout --`, `restore`, `stash`, `clean`, `rebase`, `rm`, amend, force push,
branch switching, and worktree mutation. Ordinary `git add` and `git commit`
are required only for the exact task files after focused verification.

## 2. Root Cause Already Proven

Do not spend tokens rediscovering this chain:

1. The child model turn completed, so agent lifecycle `completed` is true.
2. Initial and repair outputs were Markdown-fenced JSON, so strict host parsing
   rejected both.
3. Production child requests did not carry Neo's existing provider-neutral
   response format.
4. Delegate returned an error with truthful completed agent details.
5. Workflow dispatch incorrectly rejected that valid two-layer state, replaced
   the original schema error with `error result cannot be completed`, and lost
   usage and child references.
6. Built-in workflows ignored failed outcomes and manufactured placeholder
   success.
7. The outer tool result displayed `Failed Delegate` beside a bare completed
   status, while the child card correctly showed the lifecycle.

Decisive current owners:

- `crates/neo-agent-core/src/workflow/schema.rs:190`:
  `attach_response_format_hint` already exists;
- `crates/neo-agent-core/src/multi_agent/runtime.rs:1906`: initial child turn;
- `crates/neo-agent-core/src/multi_agent/runtime.rs:1979`: repair turn;
- `crates/neo-agent-core/src/tools/delegate.rs:204`: schema result handling;
- `crates/neo-agent-core/src/runtime/workflow_dispatch.rs:811`: workflow outcome;
- `crates/neo-agent-core/src/tools/workflow.rs:279`: inline definition shape;
- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`: author guidance;
- three files under `crates/neo-agent-core/src/workflow/builtins/`;
- `crates/neo-tui/src/transcript/tool_renderers.rs:830`: outer tool body.

Use CodeGraph before text search for symbols. Use targeted `rg` only for
literals, tests, and retirement checks. Do not conduct another broad review.

## 3. Closed Required Behavior

Implement all of the following:

1. Every schema-constrained initial child request carries the exact JSON Schema
   in strict `ResponseFormat`.
2. The only repair turn carries the same format and advertises no tools.
3. Host parsing and validation remain strict. Fences, prose, and multiple JSON
   values remain invalid.
4. Initial and repair prompts include the actual schema and say: exactly one
   JSON value, no fence or prose, every required field, and no formatting tool.
5. A completed agent whose structured result failed acceptance retains lifecycle
   `completed`, but Delegate result, workflow invocation, and workflow child
   result are `failed`.
6. Preserve `schema_error`, `schema_error_code`, actual usage, and child
   references. Do not emit `workflow_outcome_error` for this valid state.
7. `validate_inline`, `save`, and `run_inline` require explicit `input_schema`
   and `output_schema`.
8. Existing saved definitions remain readable and runnable without migration.
9. Workflow tool description, `create-workflow` skill, normal prompt, and repair
   prompt teach one consistent schema-first path.
10. `code-review`, `deep-research`, and `large-refactor` terminate through
    `neo.fail` when a required child or verification outcome fails. They never
    manufacture placeholder success.
11. The outer failed tool presentation explains lifecycle versus requested
    result using typed details.
12. Delegate, DelegateGroup, and DelegateSwarm cards remain byte-for-byte and
    behaviorally unchanged.

## 4. Prompt Guidance Is Required

Do not treat prompt work as optional documentation polish.

The Workflow tool opening must tell the model:

- begin every call with explicit `action`;
- inline definitions require both schemas;
- known saved workflows use `run_saved` without resending definitions;
- returned workflow task IDs are read with `TaskOutput`.

The `create-workflow` skill must replace repeated guidance with one checklist:

1. choose the action;
2. declare input and output schemas;
3. make Lua return exactly the declared result;
4. check every outcome `ok` field;
5. call `neo.fail` for a failed required outcome;
6. call `neo.tool` with `{name, input}`;
7. put `output_schema` on every heterogeneous swarm item;
8. use `neo.json_array` and `neo.json_object` only as Lua table type markers.

Include one no-argument input schema and one required-argument schema. Delete
the retired statement that `input_schema` may be omitted. Replace contradictory
text instead of appending another long section.

Prompt text improves first-call accuracy; it is not the safety boundary.

## 5. Prohibitions

Do not add or change:

- fence stripping, prose extraction, fuzzy parsing, automatic repair loops, or
  more than one repair turn;
- provider-specific Workflow paths;
- a second request builder, runtime, state owner, schema owner, parser, or
  persistence format;
- optional/permissive `output_schema` or schema inference from Lua;
- action aliases, flat tool arguments, top-level swarm schema, wait-tool
  compatibility, empty-value normalization, or predictive limits;
- stored workflow/session/journal migration or rewriting;
- Delegate-family card files, layout, grouping, expansion, activity, ordering,
  or transcript placement;
- fixes for unrelated report items without proven root cause;
- new dependencies.

If implementation appears to require a prohibited item, stop and report the
specific conflict. Do not improvise around it.

## 6. Execute The Plan Exactly

Open and follow:

```text
docs/aegis/plans/2026-07-30-dynamic-workflow-ai-reliability.md
```

Execute Tasks 1 through 6 in order. After every task:

1. view only that task's diff;
2. run the named focused check;
3. run `git diff --check`;
4. stage only that task's paths;
5. commit with the plan's message;
6. run `git status --short` and preserve unrelated paths.

If a planned regression does not exist, create that exact test. If an existing
test name differs, resolve it with targeted `rg`, keep the same package and
target selector, and report the resolved name.

Do not replace focused checks with broad `cargo test`, package-wide nextest,
whole-workspace Clippy, or unrelated cleanup. If another task's dirty changes
block compilation, record the exact path/error and continue only where scoped
evidence remains valid. Never revert the blocker.

## 7. Live Acceptance

Run live acceptance only after deterministic checks pass:

- use a temporary workspace;
- use the configured live provider;
- save and run a one-child `schema-smoke` project workflow there;
- do not alter existing user workflows;
- verify first-turn schema acceptance, useful output, no repair,
  no `workflow_outcome_error`, and no placeholder;
- leave the temporary workspace path in evidence rather than recursively
  deleting it.

Provider-specific success is not proof for every provider or operating system.
If credentials, network, capability, or environment block the run, report the
blocker exactly and do not claim live acceptance.

## 8. Completion Standard

Do not claim completion until deterministic checks pass, retirement searches
are clean in active code, the decision record and baseline are updated, and
every task has its own commit.

Before the final report:

```bash
git diff --check
git status --short
git log --oneline -10
```

Final report, in Chinese, must contain:

- conclusion first;
- commits in task order;
- exact test commands and results;
- live evidence or exact blocker;
- changed files grouped by owner;
- original error/usage/child-reference preservation evidence;
- retirement-search result;
- explicit confirmation that Delegate-family card files were not modified;
- unrelated dirty paths preserved;
- residual provider and native-platform risk;
- no claim that focused local tests prove remote CI or all operating systems.

Before responding, store a concise ICM completion record as required by
`AGENTS.md`.
*** Update File: /Users/chenyuanhao/Workspace/neo/docs/aegis/INDEX.md
@@
 | Date | Kind | Path | Title |
 | --- | --- | --- | --- |
+| 2026-07-30 | handoff | docs/aegis/handoffs/2026-07-30-dynamic-workflow-ai-reliability.md | Implement Dynamic Workflow AI Reliability Handoff Prompt |
+| 2026-07-30 | plan | docs/aegis/plans/2026-07-30-dynamic-workflow-ai-reliability.md | Neo Dynamic Workflow AI Reliability Implementation Plan |
 | 2026-07-30 | plan | docs/aegis/plans/2026-07-30-workflow-model-visible-results.md | Neo Workflow Model-Visible Results Implementation Plan |
