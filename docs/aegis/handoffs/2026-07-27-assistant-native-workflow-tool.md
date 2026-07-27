# Handoff Prompt: Finish Assistant-Native Workflow Tool Tasks 2-7

Copy the prompt below into the implementation task unchanged.

---

You are continuing the approved assistant-native workflow redesign in
`/Users/chenyuanhao/Workspace/neo`.

Task 1 is complete and committed. Your job is to execute Tasks 2-7 exactly,
commit each verified task, and stop with an implementation/evidence handoff for
the original Codex agent to perform the final review. Do not redesign the
feature, repeat the Neo/Grok comparison, or start unrelated workflow-platform
expansion.

## 1. Authority: Read In This Order

1. `AGENTS.md`, `~/.codex/RTK.md`, and `~/.codex/CX.md`.
2. `docs/aegis/specs/2026-07-27-assistant-native-workflow-tool-design.md`.
3. `docs/aegis/plans/2026-07-27-assistant-native-workflow-tool.md`.
4. `docs/aegis/baseline/2026-07-26-workflow-platform-contract.md`.
5. `docs/aegis/adr/ADR-0006-local-workflow-platform.md`.
6. `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/20-checkpoint.md`.
7. `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/90-evidence.md`.

Known commits:

- `b2bc08c7 docs: design assistant-native workflow tool`
- `536c678f docs: plan assistant-native workflow tool`
- `74cc07c3 feat(workflow): add unified workflow tool adapter`
- `7d48c08 fix(workflow): preserve workflow failure semantics`

The spec is the product/contract authority. The plan is the execution
authority. The baseline and ADR remain authoritative only for durable runtime,
registry, task, persistence, lineage, and operator boundaries. Their old
model-launch capability contract is superseded by the new spec.

Run this recall before code work:

```bash
icm recall-context "assistant-native Workflow Tasks 2-7 permission capability retirement registry slash create-workflow black-box" --limit 5
```

Then inspect only the current state:

```bash
git status --short
git log -8 --oneline
```

At handoff time, the only unrelated dirty file was:

```text
 M .gitignore
```

The `.gitignore` modification belongs to the user. Never stage, edit, revert,
restore, stash, clean, or otherwise touch it.

## 2. Mission And Stop Boundary

Finish Tasks 2-7 so a top-level Neo assistant can discover, inspect, validate,
save, and launch workflows through one first-party model tool without invoking
Neo's CLI and without requiring a slash command to mint hidden launch state.

Your implementation boundary ends after:

1. Tasks 2-7 are implemented in order.
2. Each task has its focused verification and conventional commit.
3. The retirement scans and three-session black-box acceptance are recorded.
4. The superseding ADR/baseline record is created only after the runtime proof.
5. You produce a final evidence summary for the original Codex reviewer.

Do not push. Do not merge. Do not perform the final independent review on
behalf of the original Codex agent. The user will return the completed work to
that agent for final review.

## 3. Closed Model Tool Contract

The sole model-visible workflow tool is exactly `Workflow`.

It has exactly these seven serialized actions:

1. `list`
2. `show`
3. `validate_inline`
4. `validate_saved`
5. `save`
6. `run_inline`
7. `run_saved`

The input is one flat object with an `action` discriminator. The existing Task
1 parser owns the action-field matrix, null-as-absent behavior, missing-field
errors, saved/inline conflict rejection, and zero-side-effect parse failures.
Do not duplicate or bypass this parser in permission code.

Object-valued `args`, `input_schema`, and `output_schema` remain model-visible
objects. Results remain structured and include stable action/status/error data
and `next_actions`. Run actions return task metadata and direct the assistant to
`TaskOutput`; do not duplicate task output/control actions inside `Workflow`.

There must be no model-visible `RunWorkflow`, `SaveWorkflow`, alias, hidden
compatibility tool, fuzzy action matching, nested `oneOf` contract, or CLI
fallback.

## 4. Task 1 Completed State

Task 1 already provides:

- root registration of `WorkflowTool`;
- the seven-action flat parser and model schema;
- registry-backed `list`, `show`, and `save`;
- canonical preflight for inline/saved validation;
- coordinator-backed inline/saved launch;
- structured results and action-specific `next_actions`;
- save-only replace recovery;
- pair rollback when manifest save fails;
- typed post-create launch failure classification;
- focused adapter, registry, and launch tests.

Do not rewrite Task 1. Make only the minimum Task 2-7 changes required by the
approved contract and compiler feedback.

Fresh Task 1 evidence at this handoff:

```text
workflow::registry::tests: 2 passed
tools::workflow::workflow_tests: 13 passed
workflow_launch::compile_schema_and_storage_failure_preserve_reusable_capability: 1 passed
workflow_launch::all_launch_adapters_reach_one_coordinator: 1 passed
workflow_registry::save_is_no_clobber_and_pair_atomic: 1 passed
cargo fmt --all --check: exit 0
git diff --check: exit 0
```

## 5. Temporary Task 1 Bridges: Must Be Retired

The following are deliberate compile/wiring bridges, not approved final
architecture:

1. `runtime/permission.rs` still contains the old `RunWorkflow` capability
   branch and missing-capability error. Task 2 replaces it with action-aware
   normal permission preparation.
2. `tools/workflow.rs` still contains `PreparedWorkflowLaunch` only so the old
   permission branch compiles. Task 2 removes this bridge after permission code
   uses the canonical action parser.
3. `WorkflowTool` currently launches with `LaunchAuthorizationMode::Headless`
   and passes `ctx.workflow_capability`. Tasks 2-3 remove this authorization
   mode and capability host requirement.
4. `workflow/capability.rs`, capability reservations, nonces, bind/consume/
   rollback state, config fields, and old launch error codes still exist.
   Tasks 3-5 delete them canonically.
5. `ToolContext::new()` currently uses
   `WorkflowDefinitionRegistry::empty()`. This is not production registry
   wiring. Task 4 must inject the existing session-shared, trust-aware registry
   at real root-agent construction sites. Do not create a second registry or
   silently leave production list/show/run_saved empty.
6. Old `RunWorkflow` strings remain in permission tests, policy tests, launch
   comments, CLI/slash code, agent fixtures, and capability tests. They are
   expected residue before Tasks 2-5, not evidence for compatibility retention.

Do not remove these bridges out of dependency order. Do not preserve them after
their owning task is complete.

## 6. Non-Negotiable Architecture Boundaries

1. `WorkflowDefinitionRegistry` remains the sole trusted definition
   list/resolve/save owner.
2. Canonical definition resolution plus coordinator preflight remain the sole
   validation path.
3. `WorkflowLaunchCoordinator` remains the sole launch ordering/rollback owner.
4. `WorkflowRuntime` remains the sole durable run, journal, replay, recovery,
   lineage, result, and actual-usage owner.
5. `BackgroundTaskManager` remains a task projection/control adapter, not a
   second run state.
6. Normal permission preparation and typed approval are the sole interactive
   human authorization owner.
7. `Workflow` is root-only. Child, restricted, and schema-repair tool sets must
   not receive it.
8. Named `/workflow <name> [JSON_OBJECT]` remains host-direct and zero-model-
   turn before launch.
9. Exact bare `/workflow` becomes a normal authoring turn with manual
   `create-workflow` activation. It creates no grant, nonce, or entitlement.
10. Headless `neo workflow` remains supported for explicit CLI use, but the
    assistant must not invoke it for normal workflow feature use.
11. Persisted definition pairs, runtime journals, linked runs, task controls,
    actual-usage admission, and child-effect authorization remain compatible.

Explicit non-goals for this work:

- no Lua-to-Rhai rewrite or engine abstraction;
- no second script engine;
- no persistence or journal migration;
- no saved-definition format migration;
- no nested workflow launch;
- no delete/rename workflow action;
- no duplicate TaskOutput/TaskPause/TaskResume/TaskStop/TaskAnswer/TaskList;
- no arbitrary total child cap;
- no predictive token/cost/time/agent governance;
- no hosted service, marketplace, or sync;
- no TUI task-card redesign.

Lua versus Rhai is a separate future capability decision. Do not use it to
delay or expand this assistant-routing correction.

## 7. Discovery And Editing Discipline

The design and plan are approved. Do not perform another full-repository audit
or inspect `.references/grok-build`.

For each task:

1. Use CodeGraph first when `.codegraph/` exists.
2. Trace direct callers/consumers of the named owner.
3. Use focused `rg` for literals, configs, tests, and stale scans.
4. Stop discovery when the task's file lease and call path are confirmed.
5. Preserve unrelated dirty files and concurrent work.
6. Use `apply_patch` for manual edits.
7. Run the narrowest task-specific tests from the plan.
8. Stage exact task files only, inspect cached diff/check, and commit.

If using subagents, obey `AGENTS.md`: use at least three only when work is
genuinely independent, assign disjoint file leases, and let the root agent own
integration, fresh verification, staging, and commits. Subagents must perform
no Git mutation and must never share an active file lease.

The dependency order below is serial. Parallelize only test discovery or
strictly disjoint analysis, not shared implementation owners.

## 8. Task 2: Route `Workflow` Through Normal Permissions

File lease:

- `crates/neo-agent-core/src/runtime/permission.rs`
- `crates/neo-agent-core/src/permissions.rs`
- `crates/neo-agent-core/src/approval.rs`
- `crates/neo-agent-core/src/tools/workflow.rs` only for parser/bridge exposure
- focused permission and approval tests

Required changes:

1. Replace the early `RunWorkflow` capability branch with action-aware
   preparation based on the existing typed Workflow parser.
2. Parse once. Permission preparation and execution must not independently
   interpret raw action fields.
3. Let `list`, `show`, `validate_inline`, and `validate_saved` execute directly
   in Ask/Auto/Yolo and plan mode after zero-effect validation.
4. Route `save` through typed Workflow Save review in Ask mode. The review must
   show scope, target pair paths, create/replace, source, phases, and schemas,
   with `Save`, `Revise`, and `Cancel` outcomes.
5. Route both run actions through the existing typed Workflow Launch review in
   Ask mode. Preserve source, phases, args, child-effect warning, and
   `Launch`, `Revise`, and `Cancel` outcomes.
6. Auto/Yolo use the normal permission semantics; do not mint hidden workflow
   authorization.
7. Reject `save`, `run_inline`, and `run_saved` in plan mode. Do not reject
   read/validate actions.
8. `Revise` and `Cancel` must create no files, runs, tasks, capabilities,
   nonces, or cleanup side effects.
9. Delete `PreparedWorkflowLaunch` and the old missing-capability message when
   no caller remains.

Verification:

```bash
cargo test --package neo-agent-core --lib runtime::permission::tests -- --nocapture
cargo nextest run -p neo-agent-core --test workflow_launch -- --nocapture
cargo fmt --all --check
git diff --check
```

Commit exactly:

```text
feat(workflow): route workflow actions through normal permissions
```

Stop Task 2 if normal permission preparation cannot consume the canonical
Workflow parser without creating a second action model. Fix the owner boundary;
do not add parallel parsing.

## 9. Task 3: Retire Capability State From Launch And Linked Runs

File lease:

- delete `crates/neo-agent-core/src/workflow/capability.rs`
- `crates/neo-agent-core/src/workflow/mod.rs`
- `crates/neo-agent-core/src/workflow/launch.rs`
- `crates/neo-agent-core/src/workflow/runtime.rs`
- `crates/neo-agent-core/src/workflow/error.rs`
- `crates/neo-agent-core/src/tools/background_tasks.rs`
- `crates/neo-agent-core/tests/workflow_launch.rs`
- `crates/neo-agent-core/tests/workflow_lineage.rs`

Required changes:

1. Delete capability types/exports and authorization-specific errors.
2. Remove nonce/capability data from `WorkflowLaunchBinding` while retaining
   session, workspace, actor, permission, lineage, and compiled-schema data.
3. Delete intent digest, authorization modes including `Headless`, bind,
   consume, reserve, rollback, and capability host fields.
4. Preserve coordinator preflight, durable create, task registration, start
   event, worker start, and the existing rollback order.
5. Preserve typed post-create failure semantics. Do not regress Task 1's
   distinction between pre-create zero-effect errors and post-create failures.
6. Simplify every coordinator caller to the new intent/host shape.
7. Remove `WorkflowCapabilityReservation` from linked-run creation.
8. Remove self-grant/reserve behavior from linked fork and CLI fork paths.
   Independent forks must not contend on a global one-shot lock.
9. Rewrite tests for direct Ask/Auto/Yolo launch, multiple independent
   launches, invalid preflight with zero create, Revise/Cancel with zero run,
   coordinator ordering, and concurrent independent linked runs.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_launch -- --nocapture
cargo nextest run -p neo-agent-core --test workflow_lineage -- --nocapture
cargo test --package neo-agent-core --lib workflow::launch::tests -- --nocapture
cargo fmt --all --check
git diff --check
```

Commit exactly:

```text
refactor(workflow): remove launch capability authorization
```

Stop if a linked-run regression appears. Diagnose the existing runtime owner;
never restore a global capability lock as a workaround.

## 10. Task 4: Remove Capability Carriers And Wire The Real Root Registry

File lease:

- `crates/neo-agent-core/src/runtime/config.rs`
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent/src/config/mod.rs`
- `crates/neo-agent/src/config/loader.rs`
- `crates/neo-agent/src/config/mutations.rs`
- `crates/neo-agent/src/config/mcp_ops.rs`
- compiler-reported root run/session/interactive construction sites
- fixtures that construct `ToolContext` or runtime config
- `crates/neo-agent-core/tests/workflow_tool_policy.rs`

Required changes:

1. Delete `workflow_capability` fields, defaults, builders, debug output,
   propagation, and lifecycle wiring from config, controller/session, context,
   and fixtures.
2. Remove ToolContext capability injection and all stale constructors. Do not
   add compatibility defaults.
3. Locate the existing session-shared, trust-aware
   `WorkflowDefinitionRegistry` owner used by slash/CLI/session wiring and
   inject that exact registry into the production root `ToolContext`.
4. `ToolContext::new()` may remain a minimal test/default constructor only if
   every production root path explicitly supplies the canonical registry and a
   focused test proves saved definitions are visible through `Workflow`.
5. Do not construct a fresh second registry per tool call or maintain a second
   index. Registry refresh/list/resolve/save remain owned by
   `WorkflowDefinitionRegistry`.
6. Keep `WorkflowTool` in the root built-in registry only.
7. Do not register `Workflow` in `with_builtin_child_tools`, restricted child
   registries, or schema-repair registries.
8. Keep Workflow launch denied from workflow scripts through the canonical
   semantic deny classifier.
9. Update policy tests so root contains `Workflow`, child/restricted/repair sets
   do not, and active product source contains no model `RunWorkflow` tool.

Verification:

```bash
cargo test --package neo-agent-core --lib tools::tests -- --nocapture
cargo nextest run -p neo-agent-core --test workflow_tool_policy -- --nocapture
cargo fmt --all --check
git diff --check
```

Add one focused production-construction regression that fails if the root
Workflow tool receives an empty registry while a saved definition exists.

Commit exactly:

```text
refactor(workflow): remove capability plumbing and child launch
```

Stop if registry ownership is ambiguous. Trace the canonical session registry;
do not solve ambiguity by creating another owner.

## 11. Task 5: Correct Named And Bare Slash Plus Headless CLI

File lease:

- `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- compiler-required stale lifecycle wiring in interactive input/controller/
  session code
- `crates/neo-agent/src/modes/workflow.rs`
- focused interactive and CLI workflow tests

Required changes:

1. Exact bare `/workflow` manually activates `create-workflow` and submits a
   normal visible model turn for workflow authoring.
2. Bare `/workflow` creates no grant, nonce, capability, reservation, or hidden
   status.
3. Preserve exact `/workflow <name> [JSON_OBJECT]` as registry-backed,
   host-direct, zero-model-turn launch with normal Ask review.
4. Delete named-launch grant/revoke/unbind handling. `Revise`/`Cancel` only
   resolve the review and emit normal feedback/status.
5. Use the simplified coordinator for named slash and headless CLI run/fork.
6. Preserve `/workflowish` and other prefix boundaries as non-matches.
7. Preserve explicit headless CLI behavior; do not teach the model to use it.

Verification:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::named_workflow_slash_launches_without_model_call --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests -- --nocapture
cargo test --package neo-agent --bin neo -- modes::workflow::tests -- --nocapture
cargo fmt --all --check
git diff --check
```

Commit exactly:

```text
fix(workflow): make slash authoring assistant-native
```

## 12. Task 6: Rewrite `create-workflow` Routing

File lease:

- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- `crates/neo-agent-core/src/skills/builtin/mod.rs`
- focused skill discovery/dispatch tests

Required changes:

1. Broaden skill discovery to create, save, run, use, test, evaluate, inspect,
   and deeply assess workflows, including black-box evaluation while cwd is the
   Neo repository.
2. Teach this canonical action table:

```text
create only        -> Workflow(save) -> ask whether to run
create and test    -> Workflow(save) -> Workflow(run_saved) -> TaskOutput
one-off evaluation -> Workflow(validate_inline) -> Workflow(run_inline) -> TaskOutput
known saved        -> Workflow(run_saved) -> TaskOutput
discover           -> Workflow(list/show) -> Workflow(run_saved) -> TaskOutput
```

3. Prohibit Bash/Terminal calls to `neo workflow`, manual hash/manifest
   construction, repository source inspection, Cargo tests, and slash-command
   prerequisites when the user is using workflows as a product feature.
4. Keep the engine authoring API documentation needed to write correct Lua.
5. Make the host `Workflow(save)` action the sole pair/path/hash/manifest save
   owner.
6. Require structured validation/run/task outcomes before declaring success.
7. Preserve an explicit escape hatch only when the user asks to debug or test
   Neo's workflow implementation or CLI itself.

Verification:

```bash
cargo test --package neo-agent-core --lib skills::builtin::tests -- --nocapture
cargo test --package neo-agent-core --lib runtime::skill_dispatch::tests -- --nocapture
cargo fmt --all --check
git diff --check
```

Commit exactly:

```text
feat(skills): route workflow requests through workflow tool
```

Do not add prompt fallbacks or multiple near-duplicate routing paragraphs to
compensate for a failing tool contract. Diagnose first-call failures at the
skill description, tool schema, permission path, or registry wiring owner.

## 13. Task 7: Integrate And Prove The User Flow

File lease:

- contract-required tests/fixtures only;
- `docs/aegis/work/2026-07-27-assistant-native-workflow-tool/`;
- a new superseding ADR/baseline record only after all runtime proof passes.

First run every focused verification from Tasks 2-6, then:

```bash
cargo fmt --all --check
rg -n "WorkflowCapability|WorkflowCapabilityReservation|LaunchAuthorizationMode|launch_nonce|Use the exact /workflow slash command first" crates/neo-agent-core crates/neo-agent
git diff --check
```

Active product source must have no capability type, reservation, authorization
mode, launch nonce, missing-capability instruction, or model-visible
`RunWorkflow`. Historical specs/plans/ADRs/handoffs may retain those terms.

### Mandatory Three-Session Black-Box Acceptance

Build/run the current Neo product and start three fresh normal sessions. In
each session, use this exact Chinese request with no source-aware preface:

```text
请你在.tmp/ 下，去全面测试我的dynamic workflow功能，调用相关 skill 和 tool，深度评测，给我结论和报告
```

Each of the three sessions must independently show this business trace:

```text
Skill(create-workflow)
-> Workflow(validate_inline)
-> Workflow(run_inline)
-> TaskOutput
-> report
```

Before the first `Workflow` call, the assistant must not call Bash, Terminal,
Read, Grep, Find, Glob, Cargo, the Neo CLI, or repository implementation tests.
The assistant must not ask the user to run `/workflow` first. Record the exact
tool sequence, action arguments with secrets/redacted values omitted, task
terminal result, report path, and any retries.

If any session fails, diagnose the responsible owner and retry only after a
bounded fix. After three diagnosed attempts still fail, stop for contract
review. Do not pile on fuzzy matching, auto-retry, hidden capability state, or
prompt branches.

### Secondary Acceptance Scenarios

Record fresh evidence for:

1. manual `create-workflow` activation;
2. create-only -> save -> offer run, with no task created before consent;
3. create-and-test -> save -> run_saved -> TaskOutput;
4. known saved discovery via list/show/run_saved;
5. explicit implementation-debug request where source/Cargo/CLI inspection is
   allowed;
6. exact bare `/workflow` authoring turn;
7. named `/workflow <name> [JSON_OBJECT]` zero-model-turn launch;
8. headless CLI run/fork;
9. root versus child/restricted/schema-repair tool policy.

Record native platform evidence truthfully. Local macOS proof is not native
Windows/Linux proof. Use available VMs only under `AGENTS.md` memory and
shutdown rules; never claim an unrun platform passed.

Only after all required evidence exists, create a new ADR/baseline record that
supersedes the model-tool, capability-authorization, and bare-slash portions of
ADR-0006. Preserve ADR-0006 as historical truth. Do not edit old specs, plans,
handoffs, ADRs, or baselines in place.

Commit after proof:

```text
docs: record assistant-native workflow contract
```

## 14. Git And Verification Rules

- One plan task equals one commit using the exact messages above.
- `git add` and `git commit` are expected after each verified task.
- Stage exact task files only.
- Before every commit run `git diff --cached --check` and inspect
  `git diff --cached --stat`.
- Do not push without explicit user authorization.
- Never use reset, checkout/restore paths, stash, clean, rebase, amend,
  cherry-pick, merge, branch deletion, worktree mutation, `git rm`, or force
  operations.
- Never make unrelated failures pass by reverting another person's work.
- Focused local proof is not remote CI or full cross-platform proof.

If a focused test name in the plan is stale, use the actual narrow target and
record the exact replacement. Do not widen to package-wide `cargo test` as a
substitute for understanding the failing boundary.

Some plan blocks name a whole test target with no individual function filter.
Treat those blocks as required coverage inventories, not permission to violate
`AGENTS.md`: enumerate and run the relevant exact test filters one by one, then
record every command. A target-wide command may be additional diagnostic
evidence, but it is not the task's required narrow proof.

## 15. Mandatory Stop Conditions

Stop and report exact evidence if any of these occurs:

1. The flat seven-action contract cannot express a required field matrix
   without adding a second tool or nested union.
2. A verified external consumer requires the retired capability API. Unknown
   use is not evidence; do not add compatibility preemptively.
3. Removing capability state breaks linked-run durability or concurrency in a
   way the existing runtime contract cannot satisfy.
4. Production registry ownership is ambiguous after tracing current
   construction paths.
5. A child/restricted/repair registry receives `Workflow` and cannot be fixed
   without changing the approved root-only boundary.
6. Named slash can no longer remain host-direct and zero-model-turn.
7. Three diagnosed black-box attempts still fail first-call routing.
8. Required native-platform evidence is unavailable; report the gap instead of
   claiming cross-platform completion.
9. Unrelated dirty work overlaps a required file and cannot be safely
   preserved.
10. A proposed fix requires engine rewrite, persistence migration, nested
    workflow launch, a new durable owner, alias, fallback, or arbitrary child
    cap.

## 16. Final Handoff Back To The Original Reviewer

After Task 7, stop and provide:

1. commit list for Tasks 2-7;
2. exact changed files per commit;
3. exact focused command results and exit statuses;
4. stale-scan output with historical-only matches identified;
5. all three Chinese black-box traces;
6. secondary acceptance results;
7. native platform evidence versus unrun platforms;
8. preserved unrelated worktree changes;
9. residual risks and any stop-condition deviations;
10. ADR/baseline record paths and commit;
11. explicit statement that no push/merge was performed.

Do not call the whole redesign finally accepted. State only that Tasks 2-7 are
ready for the original Codex agent's final review.

---
