# Handoff Prompt: Implement Workflow AI Usability Repair

> **Entire handoff superseded on 2026-08-03.** Do not execute this handoff. Use
> `docs/aegis/handoffs/2026-08-03-workflow-output-reliability.md` and its
> associated spec and plan. In particular, do not implement mandatory child
> schema failure, hidden repair, or fail-closed built-in behavior.

Copy everything below the separator into the implementation task unchanged.

---

You are implementing the approved Neo Workflow AI usability repair in:

```text
/Users/chenyuanhao/Workspace/neo
```

The design is approved and closed. This is an execution task, not a request for
another product review. Follow the implementation plan in order, commit every
repository task separately, stop after the evidence handoff, and return control
to the original reviewer for final review.

## 1. Read Authority In This Order

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-08-01-workflow-ai-usability-repair-design.md`
5. `docs/aegis/plans/2026-08-01-workflow-ai-usability-repair.md`
6. this handoff
7. `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
8. `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md`
9. `docs/aegis/baseline/2026-07-26-workflow-platform-contract.md`

The fixed design and plan commit is:

```text
a51c4df docs(workflow): design AI usability repair
```

Confirm it without changing branches:

```bash
rtk git show --stat --oneline a51c4df
rtk git status --short
rtk proxy icm recall-context "workflow AI usability response_format failed child schema repair create-workflow" --limit 5
```

Authority rules:

- the approved design owns behavior, preserved strict decisions, and non-goals;
- the plan owns task order, file boundaries, tests, retirement, and commits;
- this handoff owns execution discipline, stop conditions, and evidence format;
- current source is evidence for locating the named owners, not permission to
  choose a different design;
- report 004 is black-box evidence, not permission to treat every reported
  label as a product defect;
- historical designs remain in force only where the approved design says they
  are preserved.

Do not reread all historical reports, survey all providers, browse reference
projects, or repeat the whole-repository analysis. Use CodeGraph or `cx` for the
named symbol and its direct callers before each edit. Use bounded `rg` only for
literal text, test names, and retirement searches.

## 2. Root Cause Already Proven

Do not spend time rediscovering this chain:

1. `RequestOptions.response_format` is a provider-neutral optional hint.
2. `OpenAiCompatibleClient::request_body` serializes it as Chat Completions
   `response_format` for every provider with `type = "openai"`.
3. Report 004 showed four compatible endpoint families rejecting that field
   with HTTP 400 before model execution: zero tokens and zero tool calls.
4. The failed child snapshot is then passed to structured-output parsing as if
   it were successful assistant text.
5. Neo starts a repair turn, that request fails the same way, and the original
   protocol error is replaced by `strict_json_failed`.
6. Swarm retains detailed child errors but its model-visible summary reports
   only item and finished counts.
7. Several intentional strict Lua and Workflow behaviors are correct but are
   not taught clearly enough before authoring, so AI reports repeatedly propose
   weakening them.

The smallest complete repair is fixed: omit the optional native field only on
the ambiguous compatible wire, gate schema acceptance on a successful child
turn, improve existing bounded errors, and strengthen the existing canonical
authoring skill.

## 3. Closed Decisions

Implement all of these exactly:

1. `openai_response` continues mapping the exact schema to native
   `text.format` JSON Schema.
2. `openai` compatible requests omit native `response_format` deterministically.
3. Initial and repair child prompts still carry the exact schema and JSON-only
   instructions.
4. Host parsing and JSON Schema validation remain strict.
5. A successful child turn with invalid structured text receives at most one
   tools-disabled content-repair turn.
6. A child turn that failed at provider, authentication, rate-limit,
   cancellation, or runtime level never enters schema parsing or repair.
7. Child success means lifecycle `Completed` and no error terminal outcome. A
   completed snapshot with no terminal outcome is not automatically a failure.
8. Foreground Delegate and direct workflow swarm paths use the same success
   rule before schema acceptance.
9. Failed swarm summaries expose failed count, total count, and the first
   bounded child error while preserving ordered details.
10. Final-result schema failures expose instance path and a Unicode-safe,
    160-character maximum preview of the failing node.
11. The existing `create-workflow` skill remains the only detailed model-facing
    workflow authoring guide.
12. The Chinese and English guides mirror the same behavior.
13. Only `/Users/chenyuanhao/.neo/workflows/echo-test.workflow.toml` is repaired;
    its Lua source and `source_sha256` remain unchanged.
14. Implementation stops after Task 7 and returns evidence for independent
    final review.

## 4. Preserved Strict Behavior

The following report items are intentional. Teach them; do not “fix” them with
runtime aliases or permissive behavior:

- inline `validate_inline`, `save`, and `run_inline` require explicit
  `input_schema` and `output_schema`;
- no-argument inline definitions use the explicit closed object schema;
- `neo.tool` accepts only `{ name, input }`;
- a call-shape decode error is distinct from an executed tool returning
  `ok = false`;
- `neo.json_array` and `neo.json_object` require tables and return marked tables;
  `nil` is invalid;
- `neo.fail` is terminal and cannot be undone by `pcall`;
- `neo.await_user` returns the raw read-only answer value;
- `neo.report` returns no value and is statement-only;
- workflow task IDs are read and waited through `TaskOutput`, never
  `WaitDelegate`;
- historical paired saved definitions without `input_schema` remain readable;
  this does not make inline schemas optional.

The exact canonical guidance block and test anchors are already written in Task
5 of the plan. Use them. Replace contradictory or repeated wording instead of
adding another long section. Do not add this material to the global system
prompt.

## 5. Prohibited Changes

Do not add or change any of the following:

- provider capability settings, probes, endpoint allowlists, error-text
  matching, HTTP-400 retry, or automatic protocol fallback;
- a second request builder, validator, parser, repair loop, workflow runtime,
  result channel, prompt owner, or documentation tutorial;
- schema inference, optional inline schemas, flat tool aliases, nil
  normalization, catchable terminal failure, or workflow support in
  `WaitDelegate`;
- fenced-JSON extraction, prose scanning, fuzzy parsing, or more than one
  content-repair turn;
- session, journal, artifact, saved-definition, or workflow migration;
- any Delegate, DelegateGroup, or DelegateSwarm card layout, expansion,
  activity, ordering, grouping, or transcript behavior;
- any new dependency;
- any file outside the plan merely because it is nearby;
- any user workflow other than the exact `echo-test.workflow.toml` file;
- push, branch switch, worktree mutation, merge, amend, stash, reset, restore,
  clean, rebase, or unrelated cleanup.

If implementation appears to require one of these, stop and report the exact
source conflict and acceptance criterion. Do not improvise around the design.

## 6. Worktree And Command Discipline

The worktree is shared. Pre-existing modified or untracked files belong to the
user or another task. Never revert, overwrite, stage, or commit them.

All shell commands must use `rtk`. Use `rtk proxy` when RTK has no dedicated
subcommand. Do not replace the plan's focused checks with broad `cargo test`,
package-wide nextest, whole-workspace Clippy, or “make sure” test expansion.

Before every task:

1. view `rtk git status --short`;
2. use CodeGraph or `rtk proxy cx` for only the named owner and direct callers;
3. confirm no overlapping user changes in the task files;
4. make only the planned edit.

After every repository task:

1. view only that task's diff;
2. run every exact focused check listed in the plan;
3. run `rtk git diff --check`;
4. stage only the task's files with `rtk git add`;
5. commit with the plan's exact message using `rtk git commit`;
6. view `rtk git status --short` and preserve unrelated paths.

If an existing test name moved, use a bounded literal search, keep the same
package and target selector, and record the resolved test name. If unrelated
dirty work blocks a check, report the exact path and compiler error. Never
revert the blocker.

## 7. Execute Tasks 1-7 In Order

### Task 1: compatible provider wire

- File: `crates/neo-ai/src/providers/openai/compatible.rs`
- Owner: `OpenAiCompatibleClient::request_body`
- Delete the compatible-wire `response_format` serialization branch.
- Replace its positive mapping test with omission proof.
- Run both the compatible omission regression and the existing Responses native
  mapping regression.
- Commit: `fix(ai): omit unsupported structured output hints`

Gate: compatible body has no native field; Responses mapping still passes; no
configuration or endpoint branch exists.

### Task 2: failed child gate

- Files: `crates/neo-agent-core/src/tools/delegate.rs`,
  `crates/neo-agent-core/src/workflow/runtime.rs`,
  `crates/neo-agent-core/tests/workflow_schema.rs`
- Owners: `apply_child_output_schema`, `child_run_to_outcome_with_schema`
- Add the success precondition before schema compilation/acceptance.
- Add two regressions through the real consumers: foreground Delegate and
  direct workflow swarm.
- Do not test only `accept_child_structured_output_with_repair`; that bypasses
  the new guards.
- Keep the successful-invalid-text, exactly-one-repair regression green.
- Commit: `fix(workflow): preserve failed child errors`

Gate: each failed-child test sends one request, writes no schema-repair event,
retains original error/usage/details, and contains no replacement schema error.

### Task 3: swarm failure summary

- Files: `crates/neo-agent-core/src/workflow/runtime.rs`,
  `crates/neo-agent-core/tests/workflow_lua.rs`
- Owner: `WorkflowRuntime::run_swarm_batch_effect`
- Failed summary: `failed <failed>/<total>: <first bounded error>`.
- Preserve successful summary, ordered item details, usage, and child records.
- Commit: `fix(workflow): surface swarm child failures`

Gate: model-visible summary is actionable; no per-child transcript expansion or
Delegate-family card edit exists.

### Task 4: final schema diagnostic

- Files: `crates/neo-agent-core/src/workflow/schema.rs`,
  `crates/neo-agent-core/tests/workflow_schema.rs`
- Owner: `validate_final_lua_result`
- Use `instance_path`, `Value::pointer`, `serde_json::to_string`, and a private
  character-safe preview.
- Keep at most 160 characters; on truncation use 159 characters plus `…`.
- Preserve error code and terminal behavior; never start model repair.
- Commit: `fix(workflow): clarify final schema failures`

Gate: nested path, short value, long Unicode value, and no-full-root leakage are
all proved by the exact regression.

### Task 5: model guidance and guides

- Files: the built-in `create-workflow.md`, its loader test, and both language
  workflow guides listed in the plan.
- Add the plan's exact closed-behavior block near the authoring checklist.
- Correct existing API tables and contradictory paired-file/effect-outcome
  prose.
- Extend the existing built-in guidance regression with all plan anchors and
  negative assertions.
- Commit: `docs(workflow): teach strict authoring behavior`

Gate: one detailed prompt owner, no global prompt addition, no duplicate long
tutorial, and Chinese/English behavior matches.

### Task 6: exact user-state repair and live acceptance

- Build `neo-agent` first so live commands use the implemented binary.
- Record pre-edit hashes for `echo-test.lua`, its manifest, and all sibling
  workflow files.
- Use `apply_patch` to add only the approved `input_schema` to
  `echo-test.workflow.toml`.
- Prove the Lua hash still matches `source_sha256`.
- Run the exact check and `{"text":"hello"}` execution commands from the plan.
- Run one minimal child schema workflow for each report-004 endpoint and one
  shipped `code-review` workflow on a small read-only scope.
- Create no repository commit for this task.

Do not mutate the user's persistent default model/provider for acceptance. Use
an already selected session or an isolated temporary configuration that refers
to credential environment variables without copying inline secrets. If safe
selection, credentials, network, or provider availability is missing, record
that provider's exact blocker. A blocker never authorizes fallback code.

Gate: exact `echo-test` succeeds, sibling hashes are unchanged, and every live
table row contains evidence or a precise blocker.

### Task 7: decision and landed baseline

- Files: `ADR-0008-workflow-product-surface-contract.md` and
  `2026-07-30-workflow-model-visible-results.md`
- Record the deterministic wire mapping, failed-child gate, rejected
  alternatives, exact commits/tests, live evidence/blockers, and `echo-test`
  result.
- Preserve historical partial-acceptance wording.
- Run the plan's retirement searches and final scoped checks.
- Commit: `docs(workflow): record provider-safe child output`

Gate: active compatible serialization and forbidden fallback/config paths are
absent; the records describe only landed and actually verified behavior.

Then stop. Do not perform another repair pass, push, merge, or claim final
acceptance.

## 8. Live Evidence Rules

For each provider record:

| Provider | Resolved type | Reached model | Repair count | Valid result | Exact blocker |
| --- | --- | --- | --- | --- | --- |
| custom | required | required | required | required | required if blocked |
| zhipuai-coding-plan | required | required | required | required | required if blocked |
| kimi-for-coding | required | required | required | required | required if blocked |
| opencode-go | required | required | required | required | required if blocked |

“Reached model” requires provider usage or another direct signal, not absence of
HTTP 400 alone. Record whether valid JSON arrived on the first turn or after the
single content repair. One provider's success proves only that provider and
request. It does not prove remote CI or native Windows/Linux behavior.

## 9. Final Evidence Required

Return the implementation report in Chinese, conclusion first, with:

1. Task 1-5 and Task 7 commits in order;
2. every exact focused command and pass/fail result;
3. the complete live provider table with evidence or exact blockers;
4. `echo-test` Lua and manifest before/after hashes plus final JSON result;
5. retirement-search results;
6. changed paths grouped by task;
7. unrelated dirty paths that were preserved;
8. explicit confirmation that no Delegate-family card file changed;
9. remaining provider, remote CI, and native Windows/Linux risk;
10. any planned command/test name that had to be resolved, with reason;
11. a direct request for the original reviewer to perform final review.

Before responding, store the required concise ICM completion record. Do not say
the feature is fully accepted; say implementation evidence is ready for the
original review.
