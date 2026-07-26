# Handoff Prompt: Neo Workflow Platform P0-P2

Copy the prompt below into the implementation task unchanged.

---

You are implementing the approved Neo workflow platform in
`/Users/chenyuanhao/Workspace/neo`.

This is a large, high-risk Rust architecture migration. The design and plan are
complete. Your job is execution, evidence, retirement, and exact commits. Do
not redesign the product, compare Neo with Grok again, or reopen closed choices.

## Authority: Read In This Order

1. `AGENTS.md`, `RTK.md`, and `CX.md`.
2. `docs/aegis/specs/2026-07-25-workflow-platform-design.md`.
3. `docs/aegis/plans/2026-07-25-workflow-platform.md`.
4. `docs/aegis/adr/ADR-0004-durable-runworkflow-runtime.md`.
5. `docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md`.

Known authority commits:

- spec: `ecaff36e` (`docs: specify workflow platform expansion`)
- plan: `39a5dee5` (`docs: plan workflow platform implementation`)

Run one focused memory recall before code work:

```bash
icm recall-context "approved workflow platform P0 P1 P2 implementation plan WorkflowRuntime Lua registry recovery schema swarm" --limit 5
```

Then inspect only:

```bash
git status --short
git branch --show-current
git log -5 --oneline
```

At handoff time the worktree had one unrelated user change:

```text
 M .gitignore
```

The `.gitignore` change adds `.grok/`. It is not part of this project task. Do
not stage, edit, revert, restore, stash, clean, or otherwise touch it.

The approved spec and plan are the requirement and execution authorities. The
comparison report at `.tmp/grok_vs_neo_workflow_report.md` is historical idea
input only. Do not translate it line by line or use it to override the spec.

## Closed Decisions: Never Re-Ask Or Reopen

1. Lua is the only workflow engine. Do not add Rhai, dual engines, feature
   flags, engine traits, engine factories, compatibility shims, or a future-
   engine abstraction.
2. `WorkflowRuntime` is the only durable owner of lifecycle, state, journal,
   replay, recovery, lineage, final result, and durable controls.
3. `WorkflowDefinitionRegistry` owns only definition discovery, precedence,
   validation, revision, and save. It never owns run state.
4. `WorkflowLaunchCoordinator` is stateless normalization/orchestration. It
   cannot write `run.json`, journal records, capability state, admission state,
   or task state.
5. Named `/workflow <name> [JSON_OBJECT]` is host-direct and performs zero model
   calls before workflow execution. Bare `/workflow` remains the dynamic model-
   authoring capability.
6. Registry precedence is exactly builtin < user < trusted project. Same-scope
   duplicates invalidate the name; invalid higher scope never falls back.
7. Definitions are paired `<name>.lua` and `<name>.workflow.toml`. Revision is
   the exact SHA-256 framing in the spec. Path/mtime are not hash inputs.
8. Final `output_schema` is required. Every Delegate/Swarm child
   `output_schema` is required.
9. Child schema failure gets exactly one same-session, tools-disabled corrective
   model continuation. It is journaled and visible. It never repeats original
   child/tool effects and never runs a second repair.
10. Generic `neo.tool` uses the canonical ToolRegistry and one semantic deny
    classifier. Do not copy schemas/descriptions or create an allowlist registry.
11. `neo.swarm` supports heterogeneous child specs and has no arbitrary total
    child count cap. Remove `MAX_SWARM_CHILDREN = 8`; do not replace it with 128,
    1024, or another constant. Actual machine bytes/storage/permits remain valid.
12. `AwaitingUser` is a durable independent state. The runtime owns requests
    and answers; the TUI is not the answer owner.
13. Terminal/retry execution always creates a linked run. Only a paused V2 run
    with unchanged source/args resumes under the same ID.
14. V1 runs are read-only. Explicit linked V2 upgrade creates a new run and
    leaves source V1 files byte-for-byte unchanged. There is no V1 writer.
15. Global admission limits actual VM/worker/executor/storage occupancy. Queue
    excess work as pending. Do not predict token, cost, agent, time, or task size.
16. `/tasks` is extended; no second dashboard/state owner. Existing Delegate,
    DelegateGroup, DelegateSwarm, Bash, Terminal, and workflow transcript card
    layouts and semantics remain unchanged.
17. Shell admission waits remain pending. Commands without explicit timeout or
    cancel remain unbounded.
18. Local-only, cross-platform, no hosted workflow service/marketplace/sync.

## Mandatory Execution Model

Use subagent-driven development with at least three independent subagents when
file leases do not overlap. The root agent is accountable for architecture,
integration, fresh verification, staging, and commits.

Subagent rules:

- Assign one plan task and one exact file lease at a time.
- Include the task's tests, residue scans, stop conditions, and required output.
- Subagents perform no Git mutation: no add, commit, reset, checkout, restore,
  stash, clean, rebase, push, rm, branch, switch, merge, cherry-pick, tag,
  worktree, apply, amend, force operation, or destructive command.
- Subagents do not edit `.references/`, `.gitignore`, user sessions, workflow
  journals, goals, or other persistent data.
- Subagents return changed files, exact command output/status, stale scans, and
  unresolved risks, then stop.
- Root rereads every diff, compares it to spec/plan, reruns fresh checks, stages
  exact files, and commits before the lease is reused.
- Never run two agents against the same file. Serialize shared owners exactly
  as described below.

Root Git rules:

- `git add` and `git commit` are expected after each verified logical task.
- One plan task equals one conventional commit unless the plan explicitly
  divides a task into separate built-in commits.
- Do not push without explicit user authorization.
- Never use reset/checkout/restore/stash/clean/rebase/rm/amend or force push.
- Never absorb unrelated dirty changes to make tests pass.

## Code Discovery Boundary

The plan already names the expected files, symbols, owner boundaries, tests,
and dependencies. Do not repeat a full-repository audit. Do not explore
`.references/` unless a task is genuinely blocked by a missing current-source
contract and the plan/spec cannot answer it.

For task-local source discovery:

1. Use `codegraph explore` first when `.codegraph/` exists.
2. Trace direct callers and consumers of the owner you will change.
3. Use focused `rg` for literals/config/docs and exact source windows only.
4. Stop discovery once the task's file lease and call path are confirmed.

If live source conflicts with the plan, do not silently improvise. Report the
exact task, current symbols/lines, conflict, and smallest plan correction.

## Task Order And Dependency Waves

Follow `docs/aegis/plans/2026-07-25-workflow-platform.md` for full files,
steps, commands, stop conditions, and commit messages. This summary does not
replace the plan.

### Wave 1: Durable substrate, serial

1. V1 fixtures and typed V2 identity/state/error contracts.
2. Versioned V2 journal and streaming validation.
3. Torn-tail recovery, quarantine, and V1 read-only behavior.
4. Transactional runtime creation, transition table, durable effect ordering,
   and final-result commit protocol.

Do not parallelize these tasks. They share `state.rs`, `journal.rs`,
`runtime.rs`, and durable wire contracts.

### Wave 2: Runtime services, parallel only with disjoint leases

5. Production recovery resolver and worker supervision.
6. Global admission, storage reservations, limits/config, retention preview,
   and deletion of `token_cap`.
7. Immutable artifact store and canonical final result transport.
8. Checkpoint digest, linked runs, and explicit V1-to-V2 upgrade.

Serialize any edit to `runtime.rs`, `journal.rs`, or `state.rs`. New bounded
module work may run in parallel, but root integrates one shared-owner task at a
time.

### Wave 3: Definitions and launch, serial contract

9. Typed manifest/source definition and canonical revision hashing.
10. Trusted builtin/user/project registry and no-clobber save.
11. Exact `WorkflowLaunchIntent`, stateless coordinator, authorization binding,
    and retirement of the direct tool-side launch transaction.
12. Canonical Lua return conversion, strict schemas, final return persistence,
    and complete host APIs.
12A. Provider-neutral optional JSON Schema response format; host validation
     remains authoritative.

Task 12A may run independently in `neo-ai` after schema shape is stable. Do not
make `neo-ai` depend on `neo-agent-core`.

### Wave 4: Child/tool/input/isolation

13. Exactly one tools-disabled child schema repair continuation.
14. Durable `AwaitingUser`, typed answer control, and permit release.
15. Canonical concrete `ChildPlan`, heterogeneous durable swarm, and removal of
    arbitrary total child count.
16. Generic canonical `neo.tool`, centralized deny set, child capability
    ceiling, and typed workflow provenance.
17. Context policy and cross-platform isolated worktree manager when the
    pre-edit existence check confirms no current owner.

Serialize Tasks 13/15 on `multi_agent/runtime.rs` and `tools/delegate.rs`.
Serialize Tasks 12/13/14/16 on `workflow/lua.rs`, `runtime.rs`, and dispatch
owners. Never add a second child runtime or schema validator to avoid conflict.

### Wave 5: Operator and author surfaces

18. Paged TaskOutput views and thin BackgroundTaskManager adapter.
19. Complete headless `neo workflow` command family.
20. Named slash host-direct launch while preserving bare capability semantics.
21. `/tasks` stable pagination, filters, workflow detail, and canonical actions.
22. Pure definition `check` and deterministic real-runtime fixture harness.
23. Deep-research, code-review, and large-refactor built-ins as ordinary
    immutable registry definitions.
24. English/Chinese workflow guide, references, config/data locations, and
    VitePress navigation/build.
25. Native macOS/Linux/Windows verification, stale scans, superseding ADR,
    current baseline, and final retirement closure.

Task 21 waits for Task 18. Task 23 waits for Tasks 12-17 and 22. Docs describe
behavior only after the corresponding code evidence exists. Task 25 is last.

## Shared-File Serialization Map

```text
workflow/state.rs           -> V2 runtime owner only
workflow/journal.rs         -> V2 journal owner only
workflow/runtime.rs         -> runtime owner; all dependent patches serialized
workflow/lua.rs             -> one Lua host owner
runtime/workflow_dispatch.rs-> one dispatch/recovery/tool policy owner
multi_agent/runtime.rs      -> one ChildPlan/repair/swarm owner
tools/delegate.rs           -> same ChildPlan/repair/swarm owner
tools/background_tasks.rs   -> one TaskOutput/task projection owner
neo-agent interactive files -> one slash/tasks integration owner
neo-tui tasks_browser/*     -> one dashboard projection owner
Cargo.toml manifests        -> dependency changes land before consumers
```

If two active agents need one file, stop one lease and serialize. Do not resolve
the conflict by duplicating types, helpers, adapters, flags, or fallback paths.

## Required Owner/Retirement Checks Per Task

Before editing, answer in the task checkpoint:

- What existing owner is responsible?
- Why a code change is necessary?
- Which old path must be deleted in the same task?
- What compatibility boundary must remain?
- Which exact test falsifies the task?
- Which stale scan proves the old path died?

Default retirement policy:

- Internal code/contract carrier: delete-first after consumer migration.
- V1/user persistent files: read-only compatibility; no mutation/deletion.
- Rebuildable caches/orphan previews: inspect/recompute; actual deletion only
  through approved explicit prune behavior.

Never retain both old and new paths "for safety". Unknown dependency is not
evidence for a compatibility branch.

## Test Discipline

TDD route is `off / skipped`. Use focused post-change regressions. Do not write
strict RED/GREEN ceremony, redundant cosmetic tests, derived-trait round trips,
or tests for library behavior Neo does not own.

Every Rust test command must name:

- one package;
- exactly one target selector (`--lib`, `--bin neo`, or `--test <target>`);
- one precise test-name filter.

Use the exact commands in the plan. If a proposed test name changes during
implementation, replace it with the actual exact name in the task evidence and
handoff report; never hide it behind a broad substring or package-wide run.

After every task:

```bash
rustfmt --check --edition 2024 <exact touched Rust files>
git diff --check -- <exact task files>
```

After root stages exact files:

```bash
git diff --cached --check
git diff --cached --stat
```

Focused local proof does not mean workspace CI, remote CI, provider-backed live
execution, native Windows, or native Linux passed.

## Required Product Invariants

### Persistence and recovery

- Journal V2 uses versioned records, contiguous sequence, run ID, canonical
  hashes, and fail-closed ordering.
- Valid unterminated final JSON may normalize. Invalid EOF suffix is quarantined
  before truncation. Quarantine failure preserves the original bytes.
- Interior/newline-terminated corruption fails closed.
- External effects execute only after durable start and are never automatically
  retried when uncertain.
- `FinalResultRecorded` precedes `Completed`; Completed without a valid result
  is corruption.
- Runtime mutexes never cross blocking file I/O or external awaits.

### Definitions and launch

- Definition discovery does not execute Lua.
- Registry cache is rebuildable and never authorizes launch.
- Runs pin exact resolved source and revision; edits/deletes/shadows do not
  change existing runs.
- Named slash, dynamic tool, and headless CLI converge on the same intent and
  coordinator tests.
- Validation/storage failure creates no run and does not consume reusable
  authorization.

### Lua/schema/results

- Lua `nil` maps to JSON `null`; multiple returns fail.
- Empty array/object markers are explicit and immutable.
- Sparse/mixed/cyclic/non-finite/deep/oversized values fail deterministically.
- Strict JSON accepts exactly one value; no prose scan, fence unwrap, quote/
  comma repair, first-object extraction, or multiple-value selection.
- Provider-native structured output is only a hint; host validation is final.
- Final schema failure triggers no hidden model call.

### Child/tool/swarm/input

- Child repair is exactly one same-session tools-disabled model effect,
  journaled before dispatch and included in actual usage.
- `ChildPlan` is a concrete canonical internal type, not a trait/factory.
- `tool_allow` only reduces authority. Resume cannot override original child
  role/model/provider/context/worktree/ceiling.
- Existing DelegateSwarm and workflow swarm are authoring adapters over the
  same child lifecycle owner.
- Per-item queued/started/finished state prevents replay of completed siblings.
- `neo.tool` defaults ordinary tools open and uses the centralized semantic
  deny set. MCP remains eligible through ordinary permissions/instructions.
- Awaited answers are durable and not for credentials/secrets.

### Artifacts/lineage/output/UI

- Artifacts are immutable canonical text/JSON bytes, content-hashed, atomically
  published, and visible only after a journal commit.
- Linked runs import a verified completed prefix and artifact hashes. Mismatch
  fails before new effects; inherited usage is separate.
- TaskOutput never synchronously loads or serializes a complete journal or
  artifact; cursor is query-bound and the complete ToolResult is capped.
- `/tasks` pagination has no hidden 50-item cap; state and actions call
  canonical runtime APIs.
- Existing card golden output is unchanged.

## Built-In Workflow Requirements

Built-ins are ordinary paired manifest/Lua definitions embedded through the
registry. They use public `neo.*` APIs and receive no privileged host branch.

- Deep research: structured evidence/source findings, research-plan artifact,
  contradiction/gap verification, optional clarification, validated final
  report, actual usage.
- Code review: read-only capability ceilings, structured severity/path/line/
  evidence/test-gap findings, dedupe/challenge, findings-first output, no code
  mutation.
- Large refactor: approved spec/plan input, independent slices, isolated
  worktrees when supported, verification/review artifacts, human merge and
  retirement decisions, no auto-merge/delete.

Each built-in must pass the deterministic public harness.

## Cross-Platform And VM Protocol

- Use `Path`/`PathBuf`; no hard-coded separators, bare `sh -c`, Unix signals,
  executable-bit assumptions, or unguarded Unix-only file behavior.
- Link/reparse, atomic replace, sync, registry, artifact, lineage, prune, and
  worktree behavior need native Windows/Linux/macOS evidence.
- Before any Parallels boot, inspect available Mac memory and `prlctl list`.
- Boot at most one VM. Never start both.
- Run only the exact task target on the VM.
- Verify the tested commit SHA matches the root evidence.
- Stop the VM after use and verify it is stopped.
- If dependency/toolchain resolution blocks before the test, report a blocker,
  not a pass.
- Remote machines are real machines. Do not delete or mutate broad paths.

## ICM And Long-Task Continuity

Use ICM actively:

- Recall once before work.
- Store immediately after resolving an error, making an architecture decision,
  discovering a preference, completing a significant task, or reaching about
  20 tool calls without a store.
- Store only durable summaries, not transient logs or existing AGENTS facts.

After every committed task, retain a compact checkpoint:

```text
Task / commit:
Files changed:
Owner repaired:
Old path retired:
Exact tests and exit status:
Stale scans:
Covered scope:
Uncovered risk:
Next dependency-ready tasks:
Dirty unrelated files preserved:
```

Resume from the latest checkpoint, spec, plan, baseline, current Git status,
and current diff. Never resume from model memory alone.

## Stop And Escalate

Stop the affected task instead of inventing a workaround when any of these
appear:

- persistent V1/session/workflow/artifact deletion or in-place migration;
- uncertain external-effect retry;
- second durable state/result/registry/schema/tool/child/answer owner;
- Rhai, dual engine, engine abstraction, compatibility flag, fallback parser,
  fuzzy JSON, or duplicate launch path;
- unsafe Neo code or dependency upgrade not approved by the plan;
- ad-hoc shell worktree management, auto-merge, or dirty worktree deletion;
- model/tool-supplied machine limits, predictive governance, or arbitrary total
  child cap;
- runtime lock held over I/O/await;
- hidden shell timeout or admission behavior change;
- Delegate/Group/Swarm/Bash/Terminal/workflow card redesign;
- English/Chinese contract drift;
- native platform behavior cannot be proven;
- unrelated dirty changes overlap the task.

Report the exact task, current code evidence, why the approved contract cannot
be met, and the smallest user decision needed. Continue only independent,
non-overlapping tasks.

## Final Retirement And Documentation Closure

After all code and native evidence:

1. Build bilingual VitePress docs.
2. Scan active code/docs for retired owners, count constants, `token_cap`,
   full-journal reads, fuzzy schema behavior, Rhai/engine abstraction, and
   duplicate launch paths.
3. Use the Aegis workspace helper to supersede ADR-0004 with the next available
   ADR number; do not overwrite historical ADR-0004.
4. Create a dated workflow-platform current baseline containing only landed
   behavior and actual evidence.
5. Update `docs/aegis/INDEX.md` through the helper or an exact reviewed edit.
6. Run Aegis workspace structure check. A structure pass is not evidence
   sufficiency.
7. Run staged diff check and commit the ADR/baseline closeout separately.

## Completion Report

Return a high-signal report with:

- task -> commit SHA -> exact test command -> result;
- owner repair and old-path retirement for each task;
- definition/hash/schema/journal/cursor golden evidence;
- stale scans and allowed historical/fixture hits;
- macOS, Linux, and Windows native evidence separated by OS and commit SHA;
- docs build result;
- confirmation that `.references/`, `.gitignore`, persistent V1/user data,
  existing card contracts, and shell admission/unbounded command behavior were
  untouched;
- VMs started/stopped;
- blocked or unverified surfaces and residual risk;
- explicit statement that remote CI/provider live execution passed only if it
  actually ran and reached a terminal success state.

Do not claim the whole platform complete from a subset of local tests. Confidence
must match the actual covered scope.

Begin now by reading the five authority files, running the focused ICM recall,
recording Git status, and dispatching at least three non-overlapping read-only
discovery/file-lease subagents for the first dependency-ready work. Then execute
Wave 1 serially with root review, exact verification, and one commit per task.

---
