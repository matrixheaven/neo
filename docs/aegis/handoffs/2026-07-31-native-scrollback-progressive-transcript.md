# Handoff Prompt: Implement Native Scrollback Progressive Transcript

Copy everything below the separator into the implementation task unchanged.

---

You are implementing the approved Neo native-scrollback progressive-transcript
redesign in:

```text
/Users/chenyuanhao/Workspace/neo
```

The design is approved and closed. Execute the implementation plan completely
and in order. Do not restart product discovery, repeat a whole-repository
survey, reopen the alternate-screen decision, or substitute a different TUI
architecture. Read the named authority and only the current source required for
each task.

## 1. Read Authority In This Order

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-07-30-native-scrollback-progressive-transcript-design.md`
5. `docs/aegis/plans/2026-07-31-native-scrollback-progressive-transcript.md`
6. this handoff
7. `docs/aegis/specs/2026-07-19-terminal-live-viewport-isolation-design.md`
8. `docs/aegis/specs/2026-07-19-transcript-overflow-tool-results-design.md`

Authority rules:

- the approved design owns behavior, physical constraints, non-goals, and
  acceptance;
- the implementation plan owns task order, files, tests, retirement, and commit
  boundaries;
- this handoff owns execution discipline and final evidence;
- the 2026-07-19 viewport-isolation design remains authority only for normal
  geometry, protected history insertion, transactionality, and explicit review;
- the 2026-07-19 automatic-overflow design is superseded wherever it requires
  automatic alternate-screen entry, a latch, fixed chrome, or mouse capture;
- current code is evidence, not permission to weaken the approved behavior;
- `.references/` and historical reports are not implementation authority.

Known approved-design commit:

```text
be17c322 docs(tui): design progressive native scrollback
```

The plan and handoff are added by a later planning commit. Confirm it with
`git log -5 --oneline`; do not guess its SHA.

Before source edits:

```bash
icm recall-context "native scrollback progressive transcript automatic overflow approval alternate screen Delegate workflow" --limit 5
git status --short
git log -5 --oneline
```

At handoff creation, these unrelated user changes existed:

```text
 M docs/en/configuration/config-files.md
 M docs/zh/configuration/config-files.md
 M docs/zh/guides/interaction.md
```

Re-check current status because the shared worktree may have advanced. Every
pre-existing dirty or untracked path belongs to the user or another task. Never
revert, restore, stash, clean, overwrite, stage, or commit unrelated paths.
Forbidden Git operations include `reset`, `checkout --`, `restore`, `stash`,
`clean`, `rebase`, `rm`, amend, force push, branch switching, and worktree
mutation. Ordinary `git add` and `git commit` are required only for each plan
task's exact files after focused verification.

## 2. Root Cause Already Proven

Do not spend tokens rediscovering this chain:

1. `TranscriptPresentation` treats the earliest mutable entry as a global
   commit barrier.
2. A running Delegate, DelegateGroup, DelegateSwarm, workflow, tool, shell, or
   model attempt therefore retains every later row in one mutable suffix.
3. `TranscriptTerminalUpdate.live_overflow` reports that suffix taller than the
   live budget.
4. `NeoTui::render_terminal_frame_at` latches `automatic_overflow`, creates an
   application viewport, marks a review surface, appends Todo/composer/footer as
   fixed chrome, enables mouse capture, and enters the alternate screen.
5. `handle_automatic_overflow_event` then routes wheel and page keys into the
   application viewport instead of terminal-native scrollback.
6. This is permission-mode independent, so it occurs in yolo before a
   conversation and in ask during approvals.
7. A pending approval still owns input but can be displaced visually by later
   Delegate/workflow updates, making selection and confirmation appear stuck.

Decisive current owners:

- `crates/neo-tui/src/transcript/presentation.rs`:
  history/live decisions, acknowledgement, current global barrier;
- `crates/neo-tui/src/transcript/store.rs`:
  typed entry order and mutable snapshot updates;
- `crates/neo-tui/src/app.rs`:
  automatic-overflow latch, viewport, review flag, and mouse capture;
- `crates/neo-agent/src/modes/interactive/input.rs`:
  automatic viewport input routing;
- `crates/neo-tui/src/screen_output/inline_terminal.rs`:
  correct transactional normal-screen writes; preserve it unless a focused
  failure proves a missing generic primitive;
- Delegate-family component files and `child_activity.rs`:
  existing full-card rendering that must remain intact;
- `workflow_card.rs`, approval pane data, question state machine, and chrome:
  typed workflow/dialog state and current duplicate focus/presentation paths.

Use CodeGraph before text search for symbols and call paths. Use targeted `rg`
only for literals, tests, current references, and retirement scans. Do not run
another broad architecture review.

## 3. Closed Required Behavior

Implement every item below:

1. Ordinary ask, auto, and yolo conversation never enters the alternate screen
   automatically, regardless of transcript height or entry type.
2. Only explicit `Ctrl+O` review and Task Browser may intentionally use the
   alternate screen and mouse capture.
3. On the normal screen, mouse wheel, selection, and scrollback remain owned by
   the terminal. Scrolling can reach the shell launch line and can move Todo,
   composer, and footer out of view.
4. `TranscriptStore` remains the typed source and canonical entry-order owner.
5. `TranscriptPresentation` remains the only owner of history acknowledgement,
   deferred ordered facts, bounded live composition, and history-versus-live
   decisions.
6. `InlineTerminal` remains the sole physical normal-screen owner and keeps its
   existing write, flush, failure rollback, and post-success acknowledgement.
7. Stable facts enter native scrollback exactly once using typed identity and
   typed finality.
8. Only truly mutable state remains live, and the live result is actually bounded
   by `live_budget`; it must not merely report overflow.
9. Completion appends remaining unacknowledged facts plus one final status. It
   never appends a complete duplicate card after progressive rows.
10. The earliest unresolved approval or question remains visible and owns input.
    Later events cannot displace it or steal selection, Enter, cancel, or
    feedback input.
11. Stable facts after a blocking entry remain unacknowledged until resolution,
    then commit once in canonical transcript order.
12. Every approval and question has one visible owner. Preserve runtime choices,
    `QuestionStateMachine`, key behavior, and responses.
13. Delegate-family terminal tools and terminal agent runs are captured before
    source activity trimming or snapshot replacement can remove them.
14. Workflow transitions are captured when accepted by
    `TranscriptStore::upsert_workflow`, keyed by typed projection sequence,
    before the newest snapshot overwrites the prior one.
15. All non-terminal workflow presentation states converge on interrupt, stop,
    session change, or exit.
16. Every current `Finalization::Live` producer has one explicit behavior:
    progressive facts, blocking focus, or bounded-until-final fallback.
17. Delete the dead `QueuedMessage` transcript path if current source still
    confirms it has no production caller and input preview is the real owner.
18. Record the landed architecture decision and baseline after implementation.

## 4. Stability Rules That Must Not Be Weakened

Native terminal history cannot be rewritten. A false stability decision is a
permanent visible error.

Use only these typed proofs:

- Delegate/group tool fact:
  `(TranscriptEntryId, agent id, run_count, tool id)` and phase `Done/Failed`;
- Delegate/group terminal fact:
  entry plus agent id/run count and `AgentLifecycleState::is_terminal()`;
- swarm fact:
  swarm ID, item index, agent id/run count, and typed terminal state;
- workflow fact:
  entry plus accepted `projection_sequence` and typed phase/state;
- blocking dialog:
  typed dialog ID plus resolved/abandoned/answered/cancelled state;
- assistant source:
  existing source range proof only after the current model attempt is canonical.

Important failure cases:

- `AgentSnapshot.activity` is trimmed to 24 entries. Capture terminal facts in
  the store update path, not at a later frame render.
- Activity vector positions are not identities. Retry can remove text entries.
- `RetryScheduled` can roll back the active assistant/thinking/tool attempt.
  Do not write its content into immutable terminal history before the attempt is
  canonical.
- completed thinking may be reopened. Keep attempt-scoped thinking bounded until
  canonical completion.
- `LiveOutput` can evict older rows, and current tool/shell events do not carry a
  delivered-byte or final-coverage cursor. Do not claim progressive tool/shell
  output lines are stable.
- ordinary tool and shell live output therefore remains bounded and commits its
  canonical finalized group/card once.
- Compaction, RetryStatus, and connecting MCP startup remain bounded until typed
  terminal state.

Never use rendered ANSI, human-readable row text, regex, line equality, title,
role, row position, or vector index to infer identity or finality. If a planned
fact still lacks typed proof, use the bounded fallback and report it. Do not
expand into provider, shell protocol, persistence, or runtime redesign.

## 5. Card And UI Preservation

The existing full Delegate, DelegateGroup, DelegateSwarm, workflow, tool, shell,
approval, and question interaction designs are not open for redesign.

- Keep full-card layout, hierarchy, labels, badges, progress, ordering,
  expansion, timing, and explicit-review content unchanged.
- Reuse existing child activity and card render helpers for progressive rows.
- Add only the minimum live projection and terminal-summary forms needed by the
  approved normal-screen behavior.
- Do not compact or rewrite full cards, add "earlier rows omitted", invent a
  new visual language, or remove activity from explicit `Ctrl+O` review.
- Do not duplicate full cards in terminal history after stable child facts were
  emitted.
- Do not move Todo, composer, or footer into a persistent dock.
- Do not add settings for live height, alternate-screen behavior, mouse capture,
  or compatibility modes.

## 6. Ownership And Simplicity Rules

Use the minimum approved structure:

- add `crates/neo-tui/src/transcript/progressive.rs` for private fact identities
  and pure typed projection helpers only;
- keep source fact capture inside `TranscriptStore`;
- keep acknowledgement and bounded live composition inside
  `TranscriptPresentation`;
- reuse standard-library ordered collections;
- add no dependency, trait with one implementation, factory, second store,
  second renderer, second queue, or persistence layer;
- recompute unacknowledged deferred facts from the typed store and
  acknowledgement ledger instead of adding a parallel presentation queue;
- delete obsolete owners in the same task that establishes their replacement.

Question state currently has no separate durable answered/cancelled runtime
event. Keep the question transcript projection UI-local and terminate it through
all existing answer/cancel/stop/switch/interrupt/exit paths. If correct behavior
would require a persistence change, stop and report the exact conflict instead
of inventing a new event format.

## 7. Prohibitions

Do not add or change:

- automatic alternate-screen entry for any provider, mode, tool, card, or
  terminal;
- a feature flag, fallback, hidden setting, compatibility path, or deprecated
  automatic-overflow alias;
- provider streams, retry policy, shell execution protocol, tool schemas,
  permission policy, workflow execution, multi-agent execution, or JSONL format;
- a second transcript store, renderer, viewport, dialog state machine, or focus
  owner;
- rendered-text parsing, fuzzy matching, row hashing, or regex stability logic;
- a product-level agent count limit or fixed workflow child limit;
- changes to `InlineTerminal` without a focused failing proof;
- unrelated cleanup, broad refactors, or `.references/` edits;
- new dependencies.

If implementation appears to require a prohibited item, stop and report the
specific contradiction with the approved design. Do not improvise around it.

## 8. Execute The Plan Exactly

Open and follow:

```text
docs/aegis/plans/2026-07-31-native-scrollback-progressive-transcript.md
```

Execute Tasks 1 through 7 in order. The order is dependency-bearing:

1. establish typed fact acknowledgement and bounded live composition;
2. capture and project Delegate-family stable facts;
3. preserve workflow transitions and blocking-dialog order/focus;
4. close all remaining live producers with typed or bounded behavior;
5. delete automatic alternate-screen overflow and input routing;
6. prove terminal/controller behavior end to end;
7. retire references and record ADR/baseline evidence.

Do not start Task 5 by merely deleting alternate-screen code. Tasks 1-4 must
first prove the live region is bounded and blocking focus is safe.

For every task:

1. read only that task's plan section and named current owner files;
2. trace direct callers/downstream consumers with CodeGraph, then stop discovery;
3. restate the task goal, exact files, non-edits, and exact checks;
4. make the minimum coherent owner-level change;
5. view only that task's diff;
6. run every exact test command listed for that task;
7. run a scoped `git diff --check`;
8. stage only that task's exact paths;
9. run `git diff --cached --stat` and `git diff --cached --check`;
10. commit with the plan's exact subject;
11. run `git status --short` and preserve every unrelated path.

If a planned regression does not yet exist, create it with the exact behavior.
If an existing test name has drifted, locate it with targeted `rg`, keep the
same package and target selector, and report the resolved exact name. Do not
replace focused checks with broad workspace tests, package-wide tests, or
unrelated cleanup.

If implementation uses subagents, give them disjoint file ownership and the
same no-Git-mutation restriction. The root implementer remains responsible for
integration, fresh verification, staging, and commits. Store/presentation tasks
are serial; do not parallelize agents editing those shared owners.

## 9. Mandatory Retirement Evidence

Before completion, this search must have zero active-code/test results:

```bash
rg -n "automatic_overflow|live_overflow|has_live_frontier|handle_automatic_overflow_event|scroll_automatic_overflow" crates/neo-tui crates/neo-agent/src/modes/interactive
```

Then view every remaining alternate-screen and mouse-capture caller. Each must
belong to explicit `Ctrl+O` review, Task Browser, or another existing explicit
application-owned surface. No ordinary transcript height path may reach one.

Also verify there is one visible question owner, one active blocking-focus
decision, no complete-card replay after progressive facts, and no production
`QueuedMessage` path.

## 10. Verification Discipline

Run all exact commands in Tasks 1-6. Preserve these existing behaviors with the
focused tests named by the plan:

- failed model attempts stay out of immutable terminal history;
- assistant source proof never rewinds;
- full expanded swarm review remains unchanged;
- explicit review preserves primary scrollback;
- explicit review acknowledgement does not advance the normal ledger;
- tool and shell live output still reassembles split lines/control sequences.

After all focused tests:

```bash
cargo fmt --all --check
git diff --check
git status --short
git log --oneline -12
```

The worktree is shared. If unrelated dirty Rust files make global formatting or
compilation evidence invalid, use the narrowest touched-file or exact-target
proof, preserve the unrelated work, and report the blocker. Never revert it.

Automated virtual-terminal tests can prove the absence of alternate-screen and
mouse-capture sequences, but not physical mouse selection. Perform a real macOS
terminal smoke test before claiming selection verified. For Windows/Linux
native checks, follow `AGENTS.md`: check host memory, boot only one Parallels VM
at a time, use targeted checks, and shut it down afterward. Clearly distinguish
local Rust proof, real-terminal proof, native platform proof, and anything not
run. Focused local tests are not remote CI evidence.

## 11. Manual Acceptance Scenarios

After deterministic tests pass, exercise these in a real terminal:

1. Start `neo` in yolo mode with a tall workflow or DelegateSwarm. Confirm no
   alternate-screen transition, shell launch line remains reachable, Todo and
   composer scroll away, and mouse drag selects terminal text.
2. Start ask mode and trigger multiple Delegate approvals. Leave one approval
   pending while later Delegate/workflow events arrive. Confirm the same card
   remains visible, arrow/digit selection still targets it, Enter resolves it,
   and deferred history appears once afterward in order.
3. Trigger a question before and after an approval. Confirm the earliest
   unresolved transcript item owns input and later items do not steal focus.
4. Complete, fail, cancel, and interrupt Delegate/workflow work. Confirm stable
   facts appear once, one final status appears, and no complete duplicate card
   is appended.
5. Press `Ctrl+O`. Confirm explicit review still enters an alternate screen,
   captures the mouse, shows complete current state, and returns with one
   balanced enter/leave transition and unchanged primary scrollback.

If model/provider/network access blocks a scenario, use deterministic injected
events where possible and report the exact live blocker. Do not claim a manual
scenario passed when only a unit test ran.

## 12. Completion Standard

Do not claim completion until:

- all seven plan tasks are implemented in order;
- each logical task has its own verified commit;
- every exact focused test passes or has an explicit external blocker;
- retirement searches are clean;
- full card designs and explicit review behavior are preserved;
- ADR and landed baseline record exact commits and evidence;
- unrelated worktree changes remain untouched;
- no required terminal or platform evidence is overstated.

Final report, in Chinese, must contain:

- conclusion first;
- commits in task order;
- exact test commands and results;
- files changed, grouped by the existing owner;
- evidence that no automatic alternate-screen path remains;
- evidence that pending approval/question focus survives later events;
- evidence that stable facts commit once and final cards do not duplicate;
- explicit confirmation that full Delegate-family card design and
  `InlineTerminal` behavior were preserved;
- real macOS terminal result or exact blocker;
- Windows/Linux native evidence or clearly stated residual risk;
- unrelated dirty paths preserved;
- no claim that focused local proof means remote CI or all platforms passed.

Before responding, store the required concise ICM completion record from
`AGENTS.md`.
