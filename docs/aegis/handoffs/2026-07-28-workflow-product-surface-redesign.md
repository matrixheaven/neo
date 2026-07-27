# Handoff Prompt: Implement Neo Workflow Product Surface Redesign

Copy everything below the separator into the implementation task unchanged.

---

You are implementing the approved Neo Workflow product-surface redesign in:

```text
/Users/chenyuanhao/Workspace/neo
```

The product design is closed. Your job is to execute the approved implementation
plan in order, preserve every approved capability, commit each verified task,
and return a precise implementation/evidence report for final review. Do not
restart product discovery, reopen the Neo/Grok comparison, or replace approved
decisions with your own preferred design.

## 1. Read Authority In This Order

Read these files before changing source code:

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-07-27-workflow-product-surface-redesign.md`
5. `docs/aegis/plans/2026-07-28-workflow-product-surface-redesign.md`
6. this handoff
7. `docs/aegis/adr/ADR-0006-local-workflow-platform.md`
8. `docs/aegis/adr/ADR-0007-assistant-native-workflow-contract.md`
9. `docs/aegis/baseline/2026-07-26-workflow-platform-contract.md`
10. `docs/aegis/baseline/2026-07-27-assistant-native-workflow-contract.md`

Authority order:

- The approved spec owns product behavior and acceptance.
- The implementation plan owns task order, file boundaries, verification, and
  commit boundaries.
- This handoff owns execution discipline and reporting.
- ADR-0006, ADR-0007, and the two older baselines remain authority only where
  the approved spec does not supersede them.
- Current code is evidence of the landed state, not permission to weaken the
  approved contract.
- Historical specs, plans, handoffs, evidence, and `.references/` are not
  implementation authority.

Known authority commit:

```text
bff4931b docs: approve workflow product surface redesign
```

The planning commit containing the plan and this handoff is the current commit
that adds both files. Confirm it with `git log -3 --oneline`; do not guess a SHA.

Before code work, run:

```bash
icm recall-context "workflow product surface redesign four CLI commands seven Workflow actions journal V3 Operator typed answer automatic retention" --limit 5
git status --short
git log -5 --oneline
```

At planning handoff, the unrelated dirty file is:

```text
 M .gitignore
```

It belongs to the user. Never edit, stage, revert, restore, stash, clean, or
otherwise alter it.

## 2. Mission

Implement all eleven plan tasks so that:

1. novice humans can run and follow workflows without understanding backend
   maintenance or opening a second terminal;
2. automation has exactly four same-level CLI commands;
3. the model keeps all seven Workflow actions and can call valid mutation
   actions directly without ritual prerequisites;
4. every workflow child has truthful durable lineage and live activity;
5. `/tasks` gives Workflow tasks a focused Steps/Agents/Details operating view
   while ordinary tasks retain the existing browser;
6. completion, human input, pause/resume/stop, contextual Save, storage safety,
   recovery, artifacts, and actual usage remain fully functional;
7. Windows, Linux, and macOS behavior is proven natively before completion.

Substantial refactoring is allowed. Functionality loss is not.

## 3. Closed Decisions: Do Not Reopen

### Human CLI

`neo workflow` exposes exactly these four same-level commands:

```text
neo workflow list [--json]
neo workflow run <name> [--args <JSON_OBJECT> | --args-file <PATH>]
                         [--output text|json|jsonl]
neo workflow check <name-or-path> [--json]
neo workflow test <name-or-path> --case <fixture-path> [--json]
```

Delete `show`, `save`, `answer`, `fork`, `prune`, `--detach`, and their retired
flags/helpers. Do not retain aliases, hidden commands, compatibility parsing, or
deprecated warnings.

`run` must use the real runtime and execute real Lua. TTY human input occurs in
the same process. Non-interactive awaiting-input returns structured output and
exit `3`. Exact exits are `0`, `1`, `2`, `3`, `4`, and `130` as defined by the
spec.

### Model Tool

The single model-visible tool remains `Workflow`, root-only, with exactly seven
actions:

1. `list`
2. `show`
3. `validate_inline`
4. `validate_saved`
5. `save`
6. `run_inline`
7. `run_saved`

Do not remove, merge, rename, or hide any action.

`run_inline`, `run_saved`, and `save` perform their complete validation and
permission preflight internally. `validate_inline` and `validate_saved` exist
only for explicit check-only intent. No transcript-keyword gate, mandatory
action ordering, CLI prerequisite, slash prerequisite, hidden entitlement,
fuzzy matching, automatic tool retry, or second model tool may be introduced.

Known saved workflows may be listed, shown, or run without skill activation.
Custom workflow authoring may activate `create-workflow`; that skill teaches
authoring and never grants runtime capability.

The model receives a started task handle and automatic terminal completion.
Polling through `TaskOutput` remains optional detail, not a required loop.

### `/tasks` Workflow View

There is no new slash command, dashboard, or author layer.

- `/tasks` first selects the most recently updated Workflow needing input;
- otherwise it selects the most recent active Workflow;
- otherwise it opens the ordinary Task Browser;
- selecting a normal task keeps the ordinary browser;
- Enter on a Workflow task opens the Workflow view inside the existing overlay;
- Esc returns to the prior browser selection and viewport.

The Workflow view is exactly Steps, Agents, and Details as specified. Do not add
raw event, journal, hash, revision, scope, checkpoint, maintenance, prediction,
manual refresh, output cycling, or transcript-jump surfaces.

The main agent roster does not show token usage. Details may show actual usage,
model, and provider when known. Never estimate usage, cost, time, or capacity.

Keep existing Workflow, Delegate, DelegateGroup, and DelegateSwarm transcript
cards unchanged in layout, content, expansion, timing, and placement.

### Durable Runtime

- `WorkflowRuntime` remains the lifecycle and durable state owner.
- The journal remains the durable child-fact owner.
- `MultiAgentRuntime` supplies live activity and actual live usage only.
- `BackgroundTaskManager` performs lookup, control forwarding, and the
  durable/live join.
- TUI owns selection, focus, scrolling, rendering, and answer drafts only.

New runs write journal V3 generic `ChildQueued`, `ChildStarted`, and
`ChildFinished` facts for direct delegates and swarm items. Do not dual-write
legacy swarm events.

V2 remains read-only and is projected without migration. A started child with
no durable finish after restart is `Recovering`, never falsely Working or
Completed. No arbitrary total child cap is allowed.

### Input, Controls, Save, And Retention

- Answers are built from the durable pending request and validated again by the
  runtime owner.
- Pause enters truthful `Pausing` behavior while approved work finishes and
  prevents new queued starts.
- Stop controls the parent Workflow through the existing runtime owner.
- Save appears only for inline unsaved runs and uses pinned exact metadata plus
  existing permission/registry owners.
- Public maintenance commands are removed only after automatic retention owns
  storage reclamation.
- Automatic retention uses the approved 90% trigger, 80% low watermark, and
  30-day minimum age, preserving every protected run.

## 4. Product And Architecture Prohibitions

Do not add:

- Rhai, a second script engine, or an engine abstraction;
- a second runtime, scheduler, registry, ToolRegistry, task manager, journal,
  completion queue, child state machine, or storage owner;
- a CLI-local Lua runner or synthetic headless runner;
- TUI journal parsing or TUI persistence;
- title/role/row-position matching for child identity;
- random durable child or step identities;
- V2 migration, rewrite, repair append, or reconciliation append;
- hidden CLI aliases or compatibility branches;
- single-agent stop/retry, automatic uncertain-effect retry, predictive
  governance, total child limits, hosted services, or web UI;
- user-facing backend maintenance, raw identifiers, or internal state-machine
  assembly.

Do not use the product term prohibited by ICM preference
`01KYJ3WSM7SBWJFAJ4KYJARS0Q` in new source, types, comments, docs, tests, commit
messages, or reports. Query the preference by ID if needed; do not quote the
term back into artifacts.

## 5. Discovery Boundary

The design and reference comparison are finished.

For each task:

1. Read only the task section in the plan plus its named owner files.
2. If `.codegraph/` exists, use CodeGraph before text search for call paths.
3. Use focused `rg` only for literals, configuration, tests, and retirement
   scans.
4. Trace direct callers and downstream consumers of the owner being changed.
5. Stop discovery as soon as the task's file boundary and call path are known.

Do not run a fresh whole-repository exploration. Do not reopen
`.references/grok-build` unless a named plan assertion is contradicted by
current source and the contradiction cannot be resolved from the approved spec.
If that happens, stop and report the exact contradiction before expanding scope.

The large files named in the plan receive only deletion, thin wiring, or local
replacement. Put new child projection, durable/live joining, and Workflow view
state in the bounded modules listed by the plan. Do not create abstractions for
future engines, future dashboards, or hypothetical consumers.

## 6. Execution Protocol

Execute Tasks 1 through 11 in order. The sequence is dependency-bearing, not a
menu.

Before source edits:

1. Create or resume
   `docs/aegis/work/2026-07-28-workflow-product-surface-redesign/` using the
   installed Aegis workspace helper when available.
2. Record intent, current checkpoint, baseline usage, and an empty evidence
   ledger.
3. Record the current worktree state and unrelated `.gitignore` modification.
4. Confirm that spec commit `bff4931b` is an ancestor of HEAD.

For every task:

1. Restate the task goal, files, explicit non-edits, and exact checks.
2. Work only inside that task's file boundary unless compiler evidence proves a
   directly required adjacent consumer.
3. If an adjacent file is required, record why in the checkpoint before editing.
4. Make the minimum coherent owner-level change. Delete the replaced path in
   the same task when the plan says it is retired.
5. Run every exact test command listed for that task. Do not substitute broad
   package tests.
6. Run `rtk git diff --check` on the task diff.
7. Update checkpoint/evidence/drift records with command, exit, and assertion.
8. Stage only that task's files and its work-record updates.
9. Review `git diff --cached --stat` and `git diff --cached --check`.
10. Commit with the exact task commit subject below.
11. Confirm the next task starts from the committed state.

If implementation uses subagents, keep the root implementer responsible for
integration, fresh verification, staging, and commits. Assign disjoint file
leases only. Subagents must not perform Git mutation. The runtime/journal/task
manager/TUI dependency chain is primarily serial; do not parallelize shared
owners merely to satisfy a concurrency target.

## 7. Ordered Tasks And Commit Boundaries

### Task 1: Model actions are self-contained

Delete the prompt/transcript choreography gate, preserve all seven actions,
clarify self-contained mutation preflight, and rewrite `create-workflow` as
authoring guidance rather than a capability grant.

Commit:

```text
fix(workflow): make workflow actions self-contained
```

Gate before Task 2: valid `save`, `run_inline`, and `run_saved` work as the
first Workflow business action; explicit validation remains side-effect free.

### Task 2: Pin exact product metadata

Persist backward-readable display name, input schema, definition origin, and
inline-unsaved metadata at durable creation. Preserve existing completion
delivery using only fields available at this stage.

Do not add generated files here. Task 5 owns the only terminal files aggregate
and later completion enrichment. Task 2 must be independently complete and
committable.

Commit:

```text
feat(workflow): pin product metadata for run delivery
```

Gate before Task 3: old metadata loads through one canonical fallback; new
metadata reproduces the exact launch/save definition; base completion remains
exactly once without polling.

### Task 3: Journal V3 and V2 read-only projection

Add V3 generic child facts, version-aware reading, durable child keys, step
occurrences, artifact references, Recovering projection, and strict V2
read-only compatibility.

Commit:

```text
feat(workflow): add journal v3 child lifecycle
```

Gate before Task 4: V3 round-trips, V2 bytes are unchanged, unresolved starts
recover truthfully, and unknown/torn data remains fail-closed.

### Task 4: One child lifecycle producer path

Make direct delegate and every swarm item emit one generic queued/started/
finished lifecycle through `WorkflowRuntime`. Use durable agent binding and
shared payloads. Delete all V3 writes of the legacy swarm events.

Commit:

```text
feat(workflow): journal direct and swarm children uniformly
```

Gate before Task 5: direct and swarm children replay exactly once, preserve
their stable identities, and aggregate actual usage without top-level duplicate
task rows.

### Task 5: Durable/live projection, paging, and terminal files

Build the immutable Workflow query result, durable/live join, stable cursor,
bounded per-step paging, 1,000/10,000-child proof, typed reason projection,
actual-usage details, and the sole runtime-owned generated-files aggregate.
Then enrich completion through thin queue wiring from that aggregate.

Commit:

```text
feat(workflow): project workflow operator state
```

Gate before Task 6: durable terminal facts win, live activity only enriches
non-terminal rows, cursors reject cross-query reuse, typed reasons remain
distinct, no total cap exists, and completion includes generated files without
a second projection.

### Task 6: Workflow view inside `/tasks`

Implement smart entry, Steps/Agents focus, keyed selection, Details, responsive
layouts, one-second refresh, and mouse behavior in the existing Task Browser
overlay. Preserve ordinary task browsing and every existing transcript card.

Commit:

```text
feat(tui): add workflow operator to tasks
```

Gate before Task 7: all required widths/heights fit, selection survives refresh,
the roster hides usage, ordinary task behavior passes, and transcript logical
content is unchanged.

### Task 7: Typed input, truthful controls, contextual Save

Add Pausing, pause/resume/stop forwarding, schema-driven answer forms,
same-request dismissal memory, authoritative revalidation, structured fallback,
and Save only for inline unsaved runs.

Commit:

```text
feat(workflow): add typed operator controls
```

Gate before Task 8: queued starts stop during Pausing, current work is labeled
truthfully, all required input shapes validate, dismiss/reopen works, and Save
uses canonical permission/registry owners without relaunching.

### Task 8: Automatic protected retention

Move reusable storage collection and deletion safety into the existing
retention owner. Wire startup, post-terminal, and pre-denial triggers. Only old,
terminal, unreferenced, unpinned runs are eligible.

This task must finish before public `prune` is removed in Task 9.

Commit:

```text
feat(workflow): automate safe run retention
```

Gate before Task 9: temp-root tests prove high/low watermarks, path containment,
protected-run preservation, one-directory deletion, and safe failure behavior.

### Task 9: Exactly four real CLI commands

Delete the retired CLI grammar and synthetic runner. Reuse the real preparation,
registry, coordinator, runtime, tool binding, and Lua execution path. Implement
streaming JSONL, stable JSON/text output, same-process TTY answers, exact exits,
and controlled Ctrl+C behavior.

Commit:

```text
feat(cli): replace workflow command surface
```

Gate before Task 10: help exposes only four commands, real Lua returns the real
result, side-effect-free commands remain clean, TTY/non-TTY behavior works, and
removed routes are unknown.

### Task 10: Integrated and native acceptance

Run the exact prior regressions, black-box model flows, Workflow view scenarios,
the combined 1,000/10,000-child scale test, transcript regressions, and native
macOS/Linux/Windows checks. Record actual platform, command, commit, exit, and
assertion.

Before native VMs, obey `AGENTS.md`: check host memory, boot no more than one VM
at once, and shut it down after use. A cross-target build is not native proof.

Obtain a fresh independent code review after Tasks 1-10. Resolve every confirmed
P0/P1 finding within the owning task boundary and record the repair commit and
re-run evidence before Task 11.

Commit:

```text
test(workflow): prove redesigned product surfaces
```

### Task 11: Publish landed docs and architecture record

Only after Tasks 1-10 and native evidence, update equivalent EN/ZH user docs,
create ADR-0008 and the landed baseline, finish checkpoint/evidence/reflection,
and update `docs/aegis/INDEX.md`.

Do not edit historical ADR-0006/0007, old baselines, old specs, old plans, or old
evidence. Supersede affected contracts through the new files only.

Commit:

```text
docs(workflow): publish redesigned product surfaces
```

## 8. Verification Rules

Every exact task command is written in the plan. Run all of them. Do not replace
them with:

- broad `cargo test`;
- package-wide `cargo nextest run`;
- a vague substring filter;
- compilation alone;
- a local check presented as another OS or remote CI result.

Every Rust test used as task evidence must name:

1. exactly one package;
2. exactly one target selector (`--lib`, `--bin`, or `--test`);
3. at least one exact test-name filter.

Resource-sensitive and destructive tests use isolated temp roots. Never point a
test or retention command at the user's real workflow/session directories.

At Task 10 also run:

```bash
cargo fmt --all --check
rtk git diff --check
```

Run the scoped stale-contract scans from Task 11 against active crates, current
EN/ZH docs, and README. Historical Aegis records are intentionally excluded.

The minimum final evidence set is:

- exact per-task test exits;
- negative proof for every retired writer/route/flag/key;
- black-box custom-authoring trace;
- direct saved-workflow trace without skill activation;
- explicit check-only zero-side-effect trace;
- Workflow view direct + swarm + multi-step + typed-input trace;
- 1,000/10,000-child bounded paging result;
- unchanged ordinary Task Browser and transcript regressions;
- native macOS/Linux/Windows terminal/runtime/UI evidence;
- clean `cargo fmt --all --check` and scoped diff checks;
- independent review result and any repair evidence.

## 9. Git Rules

The worktree is shared.

- Never use destructive Git commands.
- Never revert files to make tests pass.
- Never stash or clean the worktree.
- Never amend a commit.
- Never stage `.gitignore`.
- Never stage unrelated user or concurrent-agent files.
- Do not push, merge, switch branches, create/delete branches, create tags, or
  mutate worktrees without new explicit user authorization.
- `git add` and one conventional commit per verified task are required by
  `AGENTS.md` and are authorized.

Before each commit:

```bash
git status --short
git diff -- <exact task files>
git add <exact task files only>
git diff --cached --stat
git diff --cached --check
git status --short
git commit -m '<exact task subject>'
```

After each commit, record its SHA in the active checkpoint and evidence ledger.

## 10. Drift And Recovery Rules

If a test fails, trace the failure to the canonical owner named by the spec and
plan. Do not add a caller-side workaround, fallback, alias, duplicate parser,
second projection, or feature reduction.

If concurrent work changes an owned file:

1. stop editing that file;
2. identify the overlapping lines and owning task;
3. preserve the other work;
4. integrate only when the contracts are compatible;
5. otherwise report the collision instead of reverting anything.

After each task, answer in the checkpoint:

- Does this still serve the approved intent?
- Did it stay inside the compatibility boundary?
- Did a new owner, fallback, alias, adapter, or duplicate state appear?
- Were retired paths actually deleted at the planned boundary?
- Does evidence prove the downstream behavior, not only a helper?
- Is the next task still executable without redesign?

## 11. Mandatory Stop Conditions

Stop and ask the user only if:

- a proven active external dependency requires a retired CLI alias;
- V2 support would require writing or migrating V2 data;
- stable child identity cannot be established without title matching or a
  random durable ID;
- real CLI execution would require a second runtime, registry, ToolRegistry, or
  model/provider resolver;
- contextual Save cannot use pinned metadata and canonical permission/registry;
- automatic retention cannot prove path containment or protected-run exclusion;
- generated files would require a queue-local second projection;
- ordinary Task Browser or transcript card behavior would require redesign;
- a real approved capability would have to be removed;
- the approved spec and current source present a direct contradiction that
  cannot be resolved inside the named owner.

Do not stop for implementation difficulty, compiler errors, focused test
failures, large-file pressure, or the amount of refactoring required. Those are
engineering work inside the approved scope.

## 12. Completion And Return Report

Do not claim completion until:

1. all eleven tasks have committed SHAs;
2. all exact task checks pass;
3. all retired paths are absent from active surfaces;
4. black-box assistant behavior passes;
5. native evidence exists for macOS, Linux, and Windows;
6. independent review has no unresolved P0/P1 finding;
7. ADR-0008, landed baseline, EN/ZH docs, checkpoint, evidence, and reflection
   match the tested implementation;
8. unrelated `.gitignore` remains untouched.

Return a report with exactly these sections:

```text
1. Outcome
2. Task 1-11 commit table
3. Product behavior proven
4. Exact test/evidence table
5. Native macOS/Linux/Windows evidence
6. Retired paths negative evidence
7. Independent review findings and repairs
8. Worktree status and unrelated files preserved
9. Remaining risks or blockers
10. Final HEAD SHA
```

Do not push. Hand the committed work and evidence back to the original reviewer
for final review.
