# Handoff Prompt: Implement The Fullscreen Transcript Document

Copy everything below the separator into the implementation task unchanged.

---

You are the coordinating implementation agent for the approved Neo fullscreen
transcript document in:

```text
/Users/chenyuanhao/Workspace/neo
```

The user has explicitly authorized runtime implementation through this handoff.
Execute the approved plan completely, in order, using subagent-driven
development with a fresh implementer and two independent review stages for
every task. Do not reopen product design, repeat a whole-repository survey, or
substitute another terminal architecture.

This authorization covers scoped source edits, focused verification, and one
local commit per verified plan task. It does not authorize pushing, branch
switching, worktree creation/removal, rebasing, amending, deleting user session
data, releasing, or changing unrelated work.

## 1. Authority Order

Read these before the first source edit:

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-08-04-fullscreen-transcript-document-design.md`
5. `docs/aegis/plans/2026-08-04-fullscreen-transcript-document.md`
6. this handoff
7. `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md`
8. `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`
9. `docs/aegis/specs/2026-08-01-workflow-dynamic-transcript-design.md`

Authority rules:

- the approved design owns user behavior, non-goals, and acceptance;
- the implementation plan owns task order, file boundaries, exact verification,
  retirement, and commit boundaries;
- this handoff owns execution discipline, review gates, and authorization;
- ADR-0010 and the 2026-07-31 baseline are historical evidence to supersede,
  not permission to retain native-history or review-browser behavior;
- current source is implementation evidence, not authority to weaken the design;
- `.references/` is reference-only and must not be copied into production.

Known planning commits:

```text
43cdfd3e docs: design fullscreen transcript document
b62859d8 docs: plan fullscreen transcript document
```

At handoff creation, `HEAD` was:

```text
baf6fb39 feat: add canonical theme repository and startup selection
```

The worktree was clean. Treat this only as a historical snapshot. Before every
task, re-read current status because the shared worktree may have advanced.

## 2. Required Start Protocol

Load and follow these workflows:

- `aegis:subagent-driven-development`;
- `aegis:long-task-continuation`;
- `aegis:requesting-code-review`;
- `aegis:verification-before-completion` before any completion claim;
- `aegis:recording-architecture-decisions` in Task 9.

Do not activate strict test-first development. The approved plan records:

```text
TDD Route: off
Decision: skipped
Test posture: minimum implementation plus focused post-change regression
```

Run before edits:

```bash
icm recall-context "fullscreen transcript document logical anchor DelegateGroup Workflow complete tool output" --limit 5
git status --short --branch
git log -5 --oneline
```

Use CodeGraph first for symbol and call-path discovery. Use targeted `rg` for
literals, test names, and retirement scans. The plan already contains the owner
map; do not perform another broad architectural exploration.

Record a fresh `TaskStartSnapshot` containing:

- branch and `HEAD`;
- every pre-existing modified or untracked path;
- current task number and exact task-owned paths;
- the task base SHA used for review;
- known overlapping work and how it will be preserved.

Every pre-existing dirty path belongs to the user or another task. Never
restore, discard, stash, clean, overwrite, stage, or commit unrelated work.
Forbidden Git operations include `reset`, `checkout --`, `restore`, `stash`,
`clean`, `rebase`, `rm`, amend, force push, branch switching, and worktree
mutation. The coordinator alone may stage and commit exact task-owned paths.

## 3. Root Cause And Selected Architecture

Do not spend tokens rediscovering these settled facts:

1. The current history/live projection keeps mutable assistant and card content
   in a terminal-height-bounded suffix.
2. Content above that suffix can disappear during growth and become complete
   only after finalization.
3. Normal-screen terminal history cannot rewrite arbitrarily tall mutable rows.
4. The old overflow and review alternatives introduce omission, dual viewport
   state, alternate-screen transitions, or unselectable content.
5. Bash output can be dropped or capped before the UI sees it; Terminal keeps a
   bounded ring; completed tool previews are not a complete output source.

The selected path is exactly:

```text
process output bytes
  -> source-side safe text capture
  -> agents/<agent-id>/tasks/<task-id>.log
  -> rebuildable sparse <task-id>.log.idx
  -> typed optional presentation reference
  -> TranscriptStore entry
  -> DocumentLayout + logical TranscriptAnchor
  -> bounded visible slice + bottom chrome
  -> FullscreenTerminal
  -> existing LiveRenderer differential write
```

There must be exactly one typed transcript, one document layout owner, one
scroll owner, one complete output source, one fullscreen terminal owner, and
one existing frame scheduler. Do not add a compatibility renderer, second
store, second viewport, second scheduler, or fallback inline mode.

## 4. Behavior Locks

All of the following are mandatory:

1. Interactive Neo enters one application-owned fullscreen and mouse lifecycle
   at startup and restores it on normal exit, error, panic, and supported
   suspend paths.
2. The physical frame never exceeds terminal height. The logical transcript may
   be arbitrarily tall.
3. At the bottom, new content follows automatically.
4. After upward scrolling, preserve the current logical position. New content
   sets one activity indicator and never forces the viewport to the bottom.
5. A streaming assistant may grow beyond many terminal heights without losing
   any reachable row before completion.
6. A background DelegateGroup remains one updating transcript entry even when
   later assistant tools push it above the current viewport. Its state continues
   updating while off-screen; only off-screen presentation animation pauses.
   Scrolling back renders its latest complete state. Locking the viewport on it
   prevents later content from pulling the user away.
7. Delegate, DelegateGroup, and DelegateSwarm component layout, hierarchy,
   ordering, progress, activity rows, expansion, and card-local limits are
   frozen. Fix only outer document presentation.
8. Workflow always renders main card, optional Delegate summary, and optional
   DelegateSwarm summary in that order with no viewport omission strings.
9. A direct Workflow tool remains one row by default and expands in place using
   typed output identity and a bounded visible range from the complete source.
10. Complete Bash and Terminal display output is captured before preview,
    result, queue, ring, six-line, 50,000-character, and 10 MiB limits for the
    main agent, Delegate-family children, and Workflow children.
11. `ToolResult`, canonical messages, provider requests, compaction input, and
    cache-prefix bytes never contain the complete display file, its path, or its
    presentation reference.
12. Old JSONL remains readable. Missing legacy output is explicitly incomplete
    and is never relabeled complete.
13. Cross-entry selection, drag auto-scroll, double-click word selection, and
    system clipboard copy operate on materialized plain text. Shift-drag remains
    available to the terminal.
14. `Ctrl+O` toggles the selected tool inside the primary document. It never
    opens a second transcript surface.
15. Task Browser remains an overlay inside the already active fullscreen.
16. Exit restores the terminal first and only then prints the approved bounded
    static projection.
17. Print, pipe, export, `neo run`, and non-TTY resume never construct the
    fullscreen terminal or emit its control sequences.
18. Existing 100 ms animation cadence and `FrameScheduler` remain. Only visible
    active entries request animation frames. Add no animation setting.

## 5. Hard Non-Goals And Stop Signals

Do not:

- restore assistant stable-prefix writes into native terminal history;
- preserve history/live, automatic overflow, protected-history insertion, or
  the old transcript browser under another name;
- edit Delegate-family component bodies to solve viewport loss;
- infer output ownership from rendered text, JSON details, timing, row position,
  or unrelated IDs;
- put complete output into JSONL event bodies or model-visible results;
- use an unbounded in-memory vector or channel for complete output;
- add dependencies, feature flags, output-size settings, cleanup UI, hosted
  services, or migration commands;
- change Workflow execution, journal, scheduling, recovery, or model result;
- change shell admission, timeout, cancellation, background, or Terminal yield
  semantics;
- enable micro compaction or snip/dedup, both of which remain independently
  disabled by default;
- broaden verification into workspace-wide tests merely for reassurance.

Stop and report to the user instead of improvising when:

- an output file cannot be opened before process launch;
- a midstream output write failure cannot stop the corresponding process and
  report possible partial side effects;
- an event field would enter model context, provider projection, or cache
  prefix;
- child output association would require heuristic inference;
- a required change would alter Delegate-family component rendering;
- the document would pass more than terminal height to the physical writer;
- selection would need a second independent row map;
- terminal write failure could advance the differential baseline or prevent
  restoration;
- the approved plan is structurally wrong or requires a new architecture.

## 6. Subagent-Driven Execution Protocol

Create a nine-item todo list from the plan. Tasks are executed strictly in
dependency order. Do not dispatch multiple implementation subagents in
parallel.

For each task, use this complete loop:

```text
fresh implementer
  -> questions answered by coordinator
  -> implementation and self-check
  -> fresh design-compliance reviewer
  -> same implementer fixes every finding
  -> same reviewer checks again until PASS
  -> fresh code-quality reviewer
  -> same implementer fixes every Critical and Important finding
  -> same reviewer checks again until PASS
  -> coordinator runs fresh task verification
  -> coordinator stages exact task-owned paths
  -> coordinator commits once
  -> coordinator reads back HEAD, files, remaining delta
  -> checkpoint, evidence, and drift state updated
  -> next task
```

Never begin code-quality review before design compliance passes. Never move to
the next task while either reviewer has an open finding. Implementer self-review
does not replace either independent review.

### Implementer Context Packet

The coordinator must paste the full active task text from the plan into the
implementer prompt. Do not tell the subagent to read the whole plan. Add this
packet:

```text
Task:
Goal:
Stop condition:
Relevant authority refs:
Relevant source and test files:
Known facts:
Constrained unknowns:
Non-goals:
Expected output:
Exact verification:
Must-read source excerpts:
Latest checkpoint:
Resume-state hint:
Unsafe assumptions:
Git boundary: no staging, commits, branches, worktrees, or destructive Git
```

The implementer returns exactly one status:

- `DONE`: implementation and focused checks complete;
- `DONE_WITH_CONCERNS`: completed, but concerns require coordinator review;
- `NEEDS_CONTEXT`: provide missing evidence and re-dispatch;
- `BLOCKED`: assess whether context, model capability, task size, or plan defect
  caused the block. Never retry unchanged.

The report must list implementation, commands and results, changed files,
task-owned untracked files, self-review findings, and residual concerns.

### Design-Compliance Review

Dispatch a fresh read-only reviewer with:

- full active task text;
- relevant approved-design sections;
- implementer report;
- task base SHA and declared task-owned untracked paths;
- explicit frozen behaviors and retirement requirements.

The reviewer must distrust the report, read the actual diff and source, and
return findings first with file and line references. It checks:

- every requested behavior exists;
- no requested behavior is missing;
- no extra feature, fallback, alias, or scope growth appeared;
- task-specific compatibility and retirement boundaries hold;
- tests assert the real behavior rather than merely mirroring implementation.

The only passing result is:

```text
PASS: design compliant; no open findings
```

Otherwise return precise findings. The implementer fixes them and the same
reviewer checks again.

### Code-Quality Review

Only after design compliance passes, dispatch another fresh read-only reviewer
over the working-tree delta from the task base SHA. Supply:

```text
What was implemented:
Plan task and authority refs:
Fresh evidence:
Compatibility boundary:
Retirement notes:
Review scope: working-tree
Base SHA:
Head: WORKTREE
Task-owned untracked paths:
```

The reviewer leads with Critical, Important, and Minor findings and checks:

- root-cause ownership and absence of caller-side workarounds;
- data loss, error propagation, path safety, and bounded memory;
- cross-platform behavior and `cfg` isolation;
- context/cache-prefix integrity;
- duplicate owners, stale fallbacks, aliases, and dead paths;
- file responsibility, naming, maintainability, and unnecessary abstractions;
- whether focused evidence actually proves the claim;
- whether the task creates an architecture-record or baseline-sync obligation.

Critical and Important findings block the task. Minor findings are fixed when
they are in scope and low risk; otherwise record the exact residual reason.
After fixes, the same reviewer must check again and return:

```text
PASS: quality review complete; no blocking findings
```

Reviewers never edit, stage, commit, create branches, or create worktrees.

### Coordinator Closeout Per Task

After both reviews pass:

1. Run the task's exact plan verification freshly. Do not rely on implementer or
   reviewer output.
2. Confirm every test command names one package, one target selector, and a
   test-name filter.
3. Run touched-file formatting and `git diff --check` without reverting
   unrelated work.
4. Compare final status with the task snapshot.
5. Stage only explicit task-owned paths.
6. Commit using the plan's task commit message.
7. Read back `HEAD`, commit subject, committed file list, task delta, and
   remaining repository delta.
8. Update `docs/aegis/work/2026-08-04-fullscreen-transcript-document/` checkpoint,
   evidence, todo, resume, and drift records.
9. Run a drift check. Allowed results are `continue`, `pause-for-user`,
   `needs-baseline-readback`, `needs-verification`, or `blocked`.

Task-clean does not mean repository-clean. Do not stage unrelated paths to make
the repository appear clean.

## 7. Task Queue And Review Focus

### Task 1: Establish The Complete Session Output Store

- Use the full Task 1 text from the plan.
- Review focus: path traversal rejection, chunk-split UTF-8/newlines, monotonic
  counts, sparse-index rebuild, bounded range reads, data beyond 10 MiB, and
  injected write failure.
- Do not touch JSONL or process readers yet.
- Commit: `feat(core): add complete tool output store`.

### Task 2: Capture Bash And Terminal Before Existing Limits

- Use the full Task 2 text from the plan.
- Review focus: capture occurs before every bounded sink; open failure prevents
  launch; midstream failure stops supervision and reports possible partial side
  effects; existing result caps and shell semantics remain.
- No unbounded queue and no result-field expansion.
- Commit: `feat(core): capture complete shell output`.

### Task 3: Persist Typed Output References Through Every Agent Path

- Use the full Task 3 text from the plan.
- Review focus: main, Terminal, child, DelegateGroup, DelegateSwarm, and Workflow
  origins survive persistence and resume using typed identity.
- Prove old JSONL reads and complete output, paths, and references never enter
  model context or cache-prefix input.
- Commit: `feat(core): persist tool output references`.

After Task 3, run an additional fresh reviewer over Tasks 1-3 together for the
complete-output and persistence boundary. Resolve findings before Task 4.

### Task 4: Build One Incremental Document And Logical Scroll Anchor

- Use the full Task 4 text from the plan.
- Review focus: per-entry revision invalidation, virtual start shifts, tail
  follow, upward lock, one activity indicator, retry removal fallback,
  resize/reflow, bounded visible composition, and huge-output range layout.
- Explicitly test a background DelegateGroup updating after later tool entries
  push it outside the viewport, then scroll back and verify its latest full
  state remains reachable.
- Commit: `feat(tui): add fullscreen transcript document`.

### Task 5: Retire History/Live And Enter One Fullscreen Lifecycle

- Use the full Task 5 text from the plan.
- Review focus: one enter/leave lifecycle, transactional differential writes,
  guaranteed restoration, no native history write, no review transition, Task
  Browser preserved, and every old owner actually deleted.
- `Ctrl+O` toggles the selected primary-document tool only.
- Commit: `refactor(tui): use one fullscreen transcript`.

### Task 6: Add Document-Coordinate Selection And Clipboard Routing

- Use the full Task 6 text from the plan.
- Review focus: exact SGR coordinates, press/drag/release preservation,
  wide-character cell mapping, cross-entry materialization, auto-scroll,
  Unicode words, Shift bypass, clipboard failure, and dialog/Task Browser input
  priority.
- Do not add another layout map or timer.
- Commit: `feat(tui): add transcript text selection`.

After Task 6, run an additional fresh reviewer over Tasks 4-6 together for the
document, terminal lifecycle, selection, and input boundary. Resolve findings
before Task 7.

### Task 7: Remove Card Budgets And Add Complete Workflow Tool Expansion

- Use the full Task 7 text from the plan.
- Review focus: Workflow has no viewport height parameters or omission strings;
  fixed sibling order; direct tools expand in place; complete output is read by
  visible range; old/missing/corrupt output states are honest.
- Frozen ordinary Delegate-family fixtures must be byte-for-byte unchanged.
- Commit: `feat(tui): show complete workflow tools`.

### Task 8: Complete Resume, Animation, Exit, And Static-Mode Integration

- Use the full Task 8 text from the plan.
- Review focus: lazy resume, no persisted UI state, visible-only animation,
  restore-before-print ordering, interrupted-state recovery, static modes with
  no fullscreen sequences, and unchanged context prefix.
- Do not add an animation setting or another scheduler.
- Commit: `feat(agent): integrate fullscreen transcript lifecycle`.

After Task 8, run a fresh whole-implementation reviewer over Tasks 1-8. It must
check the entire data path from process bytes through persistence and document
rendering, plus retirement of every old interactive owner. Resolve every
blocking finding through the owning task's implementer/reviewer loop before
starting native closeout.

### Task 9: Run Native Acceptance And Record The Landed Decision

- Use the full Task 9 text from the plan.
- Run the exact automated preflight first.
- Check host memory before any VM action. Use only one VM at a time and shut it
  down after use.
- Separate macOS, Fedora, and Windows automated evidence from real-terminal
  mouse, clipboard, resize, suspend, error restoration, and normal exit smoke.
- `prlctl exec` does not prove real terminal selection or alternate-screen
  behavior.
- Create ADR-0012 and a new 2026-08-04 landed baseline only from verified
  evidence. Mark ADR-0010 superseded without rewriting it. Keep the 2026-07-31
  baseline unchanged as historical evidence.
- Commit: `docs(tui): record fullscreen transcript architecture`.

Task 9 also receives design-compliance and quality reviews before commit.

## 8. Final Review And Completion Boundary

After Task 9, dispatch one final fresh reviewer over the full implementation
range from the initial implementation base SHA through `HEAD`. Supply the
approved design, plan, all per-task review verdicts, exact test results, native
evidence, lingering-reference scans, and committed file list.

The final reviewer must check:

- every approved acceptance item has direct evidence;
- no Delegate-family component body changed;
- the background DelegateGroup off-screen update scenario works;
- Workflow has complete structure and direct-tool expansion;
- complete output survives old caps in main and child paths;
- no complete output leaks into model-visible context;
- old history/live, stable-prefix, overflow, and browser owners are absent;
- one fullscreen lifecycle restores on every supported path;
- static modes remain static;
- cross-platform claims match actual native evidence;
- ADR-0012 and the new baseline state only what evidence proves.

If the final reviewer finds a defect, route it back to the owning task with a
fresh fix subagent and repeat that task's two reviews and verification. Do not
amend old commits. Add a scoped fix commit, update evidence and architecture
records, and repeat final review.

Before claiming completion:

1. Load `aegis:verification-before-completion`.
2. Confirm all nine todos and every review gate are complete.
3. Run the plan's final lingering-reference and omission scans.
4. Assemble the workstream proof bundle.
5. Run the Aegis workspace check and distinguish pre-existing workspace debt
   from new errors caused by this work.
6. Report exact local and native evidence, uncovered scope, residual risk,
   branch, commits, task-clean state, and repository-clean state.

Do not claim remote pipeline, live-provider, or native-platform proof from a
focused local test. Do not push or create a pull request unless the user grants
separate explicit authorization.

## 9. Required Final Report

Return:

- one row per task with implementer status, design-review result,
  quality-review result, verification result, and commit SHA;
- final whole-range review verdict;
- exact automated commands and results;
- macOS, Fedora, and Windows evidence separated by platform and by automated
  versus real-terminal observation;
- preserved compatibility boundaries;
- retired owners and lingering-reference scan results;
- context/cache-prefix proof;
- remaining risks and anything not verified;
- final `git status --short --branch`;
- confirmation that no unrelated path was staged or committed.

The task is not complete while any task, review finding, verification, native
platform obligation, retirement scan, architecture record, or residual-risk
statement remains unresolved or falsely claimed.
