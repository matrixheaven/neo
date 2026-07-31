# Neo Native Scrollback Progressive Transcript Implementation Plan

Date: `2026-07-31`

Status: `approved design; ready for implementation`

## Goal

Keep Neo on the terminal's normal screen during ordinary conversation, stream
proven-stable transcript facts into native scrollback exactly once, keep only
genuinely mutable state in a bounded live area, and preserve the earliest
pending approval or question as the operable input focus.

## Architecture

Retain one typed transcript path:

```text
AgentEvent / typed UI action
  -> TranscriptStore entry update + append-only stable-fact capture
  -> TranscriptPresentation history/live decision + acknowledgement ledger
  -> InlineTerminal transactional normal-screen write
  -> acknowledge only after terminal write and flush succeed
```

`TranscriptStore` remains the typed transcript source and canonical entry-order
owner. `TranscriptPresentation` remains the only history-versus-live owner.
`InlineTerminal` remains the physical normal-screen and protected-history
owner. The new `progressive.rs` module is only a pure projection helper under
those owners; it is not a second store or renderer.

`Ctrl+O` remains the sole ordinary-conversation path into an application-owned
alternate screen. Task Browser retains its existing explicit alternate-screen
behavior.

## Tech Stack

- Rust 2024, minimum Rust `1.96.1`;
- existing `TranscriptStore`, `TranscriptPresentation`, `FinalizedBlock`,
  `InlineTerminal`, `QuestionStateMachine`, and typed Delegate/workflow data;
- standard-library ordered maps and sets only;
- no new dependency, feature flag, persistence format, parser, regex-based
  stability inference, or terminal mode.

## Baseline And Authority Refs

- approved design:
  `docs/aegis/specs/2026-07-30-native-scrollback-progressive-transcript-design.md`;
- retained geometry baseline:
  `docs/aegis/specs/2026-07-19-terminal-live-viewport-isolation-design.md`;
- superseded automatic-overflow design:
  `docs/aegis/specs/2026-07-19-transcript-overflow-tool-results-design.md`;
- current user approval of the written design.

## Compatibility Boundary

- preserve `TranscriptStore` entry ordering, session/event persistence, runtime
  execution, permission decisions, approval responses, question answers, and
  workflow control semantics;
- preserve Delegate, DelegateGroup, DelegateSwarm, workflow, ordinary tool,
  shell, approval, and question wording and interaction behavior unless a
  progressive terminal form requires a terminal-status line instead of a
  duplicated complete card;
- preserve existing card layout, hierarchy, badges, progress, expansion, and
  explicit `Ctrl+O` complete review;
- preserve `InlineTerminal::render_to` write/flush/acknowledge ordering and
  retry-after-write-failure behavior;
- preserve failed model-attempt rollback: assistant and thinking content is not
  stable while the current attempt may still be replaced by retry;
- remove automatic overflow, its viewport, fixed chrome, mouse capture, and
  input routing without a compatibility branch.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: post-change focused regressions
- Reason: strict test-first work was not requested; the accepted design and the
  existing automatic-overflow regressions already identify the broken path.
- Verification: every command names one package, one target selector, and one
  exact test.

## Verification

Focused evidence must prove:

1. every acknowledged fact has a typed identity and is never replayed;
2. a failed terminal write leaves all pending facts unacknowledged and retryable;
3. Delegate-family facts survive activity trimming and Delegate-to-group
   replacement;
4. workflow transitions survive snapshot replacement and remain ordered by
   projection sequence;
5. assistant, thinking, ordinary tool, shell, compaction, retry, and MCP startup
   entries remain bounded until their typed state proves finality;
6. the earliest pending approval or question stays visible and owns input while
   later events arrive;
7. tall yolo and ask sessions stay on the normal screen without mouse capture;
8. native terminal scrollback retains the shell launch line and every committed
   fact exactly once;
9. explicit `Ctrl+O` and Task Browser still enter and leave the alternate screen
   with balanced terminal-mode transitions.

## Scope Check

### Requirement Ready Check

- Requirement source: approved design and current user approval.
- Goals and non-goals: design Goals, Non-Goals, Ownership And Retirement, and
  Acceptance Criteria.
- Reproduction: tall mutable transcript suffix triggers automatic overflow,
  fixed chrome, mouse capture, and approval displacement in ask and yolo modes.
- Open blockers: none.
- Decision: `ready`.

### Change Necessity

- Configuration cannot prevent a renderer-triggered alternate-screen transition
  without also losing tall live content.
- Input-only changes cannot restore terminal scrollback or remove fixed chrome.
- Removing automatic overflow alone would leave an unbounded live suffix and
  still allow pending dialogs to be displaced.
- Decision: code changes are required at transcript capture, presentation, and
  obsolete terminal/input routing boundaries.

### Architecture Integrity

- `TranscriptStore` owns typed entry state, canonical order, and stable facts
  that must survive source-snapshot trimming before the next frame.
- `TranscriptPresentation` owns fact acknowledgement, deferred history, bounded
  live composition, and history-versus-live decisions.
- `progressive.rs` contains fact identities and pure typed projection helpers;
  it owns no independent lifecycle or persistence.
- existing card components continue to own card-specific typed rendering.
- `InlineTerminal` stays unchanged unless a focused failing test proves a
  missing generic primitive.
- runtime execution and durable JSONL events remain unchanged.

### Existence Check

- New product surface: one `QuestionPrompt` transcript entry, replacing the
  existing duplicate chrome-only question presentation while reusing the same
  `QuestionStateMachine` and key handling.
- New implementation unit: `crates/neo-tui/src/transcript/progressive.rs`.
- Why it exists: `presentation.rs` already combines acknowledgement, assistant
  segmentation, tool grouping, ordering, and live composition; adding every
  entry-family projection there would create another mixed-purpose owner.
- Boundary: types and pure projection only; capture stays in `TranscriptStore`
  and acknowledgement stays in `TranscriptPresentation`.
- Dependency: none.
- Decision: add the focused module, not a framework or second store.

### Complexity And Plan Pressure

- Every stable fact must be derived from typed identity and typed finality.
- Rendered ANSI, human-readable card text, vector position, and regex matching
  are forbidden as identity or finality evidence.
- If implementation cannot prove append-only identity for a producer, keep it
  in a bounded live preview and commit its canonical final entry once.
- Do not use `AgentActivityEntry` vector indexes: retry and trimming can remove
  or reorder text activity.
- Do not progressively commit assistant or thinking text while a model attempt
  can still be rolled back by `RetryScheduled`.
- Do not progressively commit ordinary tool or shell output lines until a typed
  delivery sequence and final coverage range exist; current `LiveOutput` can
  evict rows and final results can repeat aggregate output.
- Stop and report before changing provider streams, shell protocol, persistence,
  retry semantics, runtime execution, or card layout to manufacture proof.

### Current Live Producer Matrix

| Producer | Typed stability decision | Planned behavior |
| --- | --- | --- |
| `ThinkingBlock::Streaming` | attempt may reopen or retry | bounded live; commit once after canonical completion |
| live `ToolRun` | no delivery sequence or coverage range | bounded live; commit final tool group once |
| queued/running `ShellRun` | no delivered-byte coverage proof | bounded live; commit final command result once |
| pending `ApprovalPrompt` | typed `Pending/Resolved/Abandoned` | earliest unresolved entry is a strict focus/order barrier |
| live `Compaction` | percent and phase mutate | bounded live; commit once at typed completion |
| non-exhausted `RetryStatus` | replaced or cleared in place | bounded live; commit only its canonical terminal form |
| connecting `McpStartupStatus` | connecting mutates; settled state rejects late updates | bounded live; commit settled entry once |
| `QueuedMessage` | no production caller; input preview is the active owner | delete the dead duplicate transcript path |
| live `Delegate` | terminal tools and terminal agent runs have typed identities | progressively capture terminal facts; retain mutable preview |
| live `DelegateGroup` | same, with stable entry identity across replacement | progressively capture terminal facts; retain growing tree live |
| live `DelegateSwarm` | item index, agent/run identity, terminal state | progressively capture terminal facts; retain aggregate live |
| live `Workflow` | `projection_sequence` and typed state transitions | capture accepted transitions; retain current phase live |
| active assistant attempt | source offsets exist but retry may replace attempt | bounded until attempt is canonical; then existing source proof commits once |
| `InstructionEpoch` | component is always finalized | unchanged ordinary finalized entry |
| question overlay | typed request and state machine, no transcript entry today | add one typed blocking entry; resolve/cancel in place |

### Anti-Entropy Decision

- Deletion class: internal code retirement.
- Path: delete first when the replacement behavior is proven.
- Delete `NeoTui::automatic_overflow`, automatic latch/release, automatic
  viewport rendering, automatic mouse capture, wheel/page routing, overflow
  result flags, dead `QueuedMessage`, and duplicate visible question chrome.
- Preserve `TranscriptViewport` where explicit review and Task Browser use it.
- Preserve manual mouse capture only on explicit application-owned surfaces.
- No feature flag, fallback, alias, or permission-mode branch.

## File Map

- `crates/neo-tui/src/transcript/progressive.rs`: typed fact identities and pure
  stable/live projection helpers.
- `crates/neo-tui/src/transcript/mod.rs`: private module wiring.
- `crates/neo-tui/src/transcript/store.rs`: capture stable facts before mutable
  snapshots overwrite or trim them; preserve entry order.
- `crates/neo-tui/src/transcript/presentation.rs`: acknowledgement ledger,
  approval/question barrier, deferred ordered facts, and bounded live output.
- `crates/neo-tui/src/transcript/entry/mod.rs`: question entry and removal of the
  dead queued-message variant.
- Delegate-family component and shared activity files: typed progressive rows,
  live projection, and one terminal summary without changing full-card render.
- `crates/neo-tui/src/transcript/workflow_card.rs`: accepted transition capture,
  live projection, and terminal summary.
- approval/question pane and shell files: one ordered blocking-entry source and
  one visible owner.
- `crates/neo-tui/src/app.rs`: normal-screen-only ordinary rendering.
- `crates/neo-agent/src/modes/interactive/input.rs`: remove automatic viewport
  routing and route input to the earliest blocking entry.
- focused tests listed in each task; add
  `crates/neo-tui/tests/progressive_transcript.rs` for public end-to-end
  presentation behavior instead of enlarging existing multi-thousand-line files.

## Execution Readiness View

- Intent Lock: implement the approved native-scrollback design.
- Scope Fence: transcript capture/presentation, blocking-dialog projection,
  automatic terminal/input retirement, focused tests, and decision records.
- Baseline Lock: approved design plus retained normal-screen geometry baseline.
- Owner Constraints: no second transcript store, renderer, runtime, or terminal.
- Compatibility Boundary: runtime, persistence, card layout, and explicit review
  remain unchanged.
- Retirement Boundary: no automatic alternate-screen path remains.
- Task Batches: seven ordered tasks below.
- Drift Rule: if typed stability requires provider/runtime protocol or durable
  event changes, stop and report instead of inferring from display text.
- Completion Evidence: exact tests, lingering-reference search, formatting,
  diff checks, implementation commits, ADR, and landed baseline.

## Task 1: Establish Typed Progressive Facts And A Bounded Live Area

### Files

- `crates/neo-tui/src/transcript/progressive.rs` (new)
- `crates/neo-tui/src/transcript/mod.rs`
- `crates/neo-tui/src/transcript/store.rs`
- `crates/neo-tui/src/transcript/presentation.rs`
- `crates/neo-tui/src/transcript/entry/mod.rs`
- `crates/neo-tui/tests/progressive_transcript.rs` (new)

### Why And Change Necessity

The current earliest-live-entry barrier turns every later row into one unbounded
suffix. A typed fact identity and an actual live-budget policy are required
before automatic overflow can be removed safely.

### Repair Track

1. Add private `ProgressiveFactId`, `ProgressiveFact`, and finality/order helpers
   in `progressive.rs`. Identities must include `TranscriptEntryId` plus source
   identities such as agent/run, tool call, swarm item, workflow projection
   sequence, or blocking-dialog ID.
2. Extend `TranscriptBlockId` and `FinalizedBlockProof` with typed progressive
   facts. Keep existing entry-revision and assistant-source proofs.
3. Add an acknowledged-fact set to `TranscriptPresentation`; advance it only in
   existing `acknowledge` after successful terminal write and flush.
4. Let `TranscriptStore` retain captured stable fact payloads in canonical
   arrival order when the current source snapshot may later trim or overwrite
   them. Do not duplicate runtime state or persist this cache.
5. Replace the global live barrier with two rules: ordinary mutable entries do
   not block unrelated stable facts; the earliest unresolved approval/question
   blocks later history until resolution.
6. Recompute deferred facts from `TranscriptStore` and the acknowledgement set
   on every frame. Do not add a second queue when the existing typed source can
   reproduce the same unacknowledged order.
7. Make `live_budget` bound actual live rows. Preserve the blocking dialog and
   current mutable rows first; summarize omitted mutable items from typed counts.
   Never omit stable history.
8. On entry completion emit remaining facts, one terminal status, then remove
   the mutable projection. Never append the complete card again.

### Retirement Track

- Keep the overflow result fields temporarily for the existing caller, but make
  the bounded live result incapable of requesting automatic overflow. Task 5
  removes the fields and their callers in one compiling change.
- Delete `QueuedMessage` and its test-only pane APIs because input preview is
  already the production owner.
- Delete tests that assert an unbounded live suffix is preserved.

### Impact

- Changes presentation memory only; no durable data migration.
- Preserves terminal two-phase acknowledgement.
- Creates the shared base required by all later entry-family tasks.

### Verification

```bash
cargo test --package neo-tui --lib -- transcript::presentation::tests::progressive_facts_retry_until_ack_then_never_replay --exact --nocapture
cargo test --package neo-tui --test progressive_transcript -- unsupported_live_entry_stays_bounded_and_commits_once --exact --nocapture
cargo test --package neo-tui --test progressive_transcript -- stable_facts_after_ordinary_live_entry_keep_canonical_order --exact --nocapture
```

### Commit

```bash
git add crates/neo-tui/src/transcript/progressive.rs crates/neo-tui/src/transcript/mod.rs crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/tests/progressive_transcript.rs
git commit -m "refactor(tui): add progressive transcript facts"
```

## Task 2: Project Delegate-Family Stable Facts Exactly Once

### Files

- `crates/neo-tui/src/transcript/store.rs`
- `crates/neo-tui/src/transcript/progressive.rs`
- `crates/neo-tui/src/transcript/child_activity.rs`
- `crates/neo-tui/src/transcript/delegate_card.rs`
- `crates/neo-tui/src/transcript/delegate_group.rs`
- `crates/neo-tui/src/transcript/swarm_card.rs`
- `crates/neo-tui/tests/transcript_store.rs`
- `crates/neo-tui/tests/progressive_transcript.rs`
- `crates/neo-tui/tests/multi_agent_transcript.rs`

### Why And Change Necessity

Delegate-family snapshots trim activity to 24 items, and multiple updates can
arrive before a render. Stable tools must be captured at store update time or
they can disappear before acknowledgement.

### Repair Track

1. During `upsert_delegate`, `upsert_delegate_progress`,
   `upsert_delegate_swarm`, and `upsert_delegate_swarm_progress`, capture a tool
   only when its typed phase is `Done` or `Failed`.
2. Use `(entry, agent id, run_count, tool id)` for Delegate/group tool identity;
   include swarm ID and item index for swarm children. Preserve the same entry
   identity when a Delegate becomes a DelegateGroup.
3. Capture agent terminal facts only when `AgentLifecycleState::is_terminal()`;
   freeze final summary, outcome, counts, usage, and terminal reason then.
4. Keep queued/ongoing tools, mutable file rows, streaming text, thinking,
   elapsed time, progress, detach hints, and growing group tree in the live
   projection.
5. Reuse existing `ChildToolRow` and child render helpers for fact rows. Expose
   only the typed accessors needed by `progressive.rs`.
6. Add live-projection and terminal-summary methods beside each existing full
   card renderer. The full renderer remains the explicit-review path.
7. After history acknowledgement, release no-longer-needed captured fact
   payload while retaining enough identity to prevent replay during the entry's
   lifetime.

### Retirement Track

- Stop sending a complete Delegate, DelegateGroup, or DelegateSwarm card to
  native history after progressive rows were emitted.
- Do not retain a rendered-line comparison, activity-index identity, or second
  multi-agent renderer.

### Impact

- TUI projection only; no multi-agent runtime or JSONL changes.
- Card layout, expansion, ordering, badges, and progress remain unchanged.

### Verification

```bash
cargo test --package neo-tui --test transcript_store -- delegate_family_captures_terminal_facts_before_activity_trimming --exact --nocapture
cargo test --package neo-tui --test transcript_store -- delegate_to_group_replacement_preserves_progressive_fact_identity --exact --nocapture
cargo test --package neo-tui --test progressive_transcript -- delegate_family_completion_appends_one_terminal_status_without_complete_card_duplicate --exact --nocapture
cargo test --package neo-tui --test multi_agent_transcript -- option_b_expanded_swarm_preserves_full_child_transcripts --exact --nocapture
```

### Commit

```bash
git add crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/progressive.rs crates/neo-tui/src/transcript/child_activity.rs crates/neo-tui/src/transcript/delegate_card.rs crates/neo-tui/src/transcript/delegate_group.rs crates/neo-tui/src/transcript/swarm_card.rs crates/neo-tui/tests/transcript_store.rs crates/neo-tui/tests/progressive_transcript.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "fix(tui): stream stable delegate activity"
```

## Task 3: Preserve Workflow Transitions And Blocking-Dialog Order

### Files

- `crates/neo-tui/src/transcript/store.rs`
- `crates/neo-tui/src/transcript/progressive.rs`
- `crates/neo-tui/src/transcript/presentation.rs`
- `crates/neo-tui/src/transcript/workflow_card.rs`
- `crates/neo-tui/src/transcript/entry/mod.rs`
- `crates/neo-tui/src/transcript/approval_data.rs`
- `crates/neo-tui/src/transcript/pane.rs`
- `crates/neo-tui/src/transcript/chrome_render.rs`
- `crates/neo-tui/src/shell/event_router.rs`
- `crates/neo-agent/src/modes/interactive/approval.rs`
- `crates/neo-agent/src/modes/interactive/questions.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`
- focused tests named below

### Why And Change Necessity

`WorkflowCardComponent` currently keeps only the newest snapshot, so intermediate
typed transitions can be overwritten before drawing. Approvals are split between
the transcript and a separate queue, while questions have a second visible
chrome owner and can steal focus from an earlier approval.

### Repair Track

1. In `TranscriptStore::upsert_workflow`, capture accepted snapshot transitions
   keyed by `projection_sequence`; ignore duplicate or older sequences.
2. Project only typed phase/state changes and the terminal workflow outcome as
   stable facts. Keep current phase, elapsed time, controls, mutable summaries,
   and counts live.
3. Give every approval its transcript position on arrival. Remove the separate
   delayed visible-owner behavior; select the earliest unresolved blocking entry
   as active while later entries remain present but inactive.
4. Add typed `QuestionPrompt` transcript data that borrows the existing
   `QuestionStateMachine` display state. Update it in place on answer, cancel,
   task stop, session switch, interrupt, and terminal exit.
5. Route keys to the earliest unresolved approval/question by transcript order.
   A later question must not replace an earlier approval, and a later approval
   must not replace an earlier question.
6. Make the active blocking entry mandatory in bounded live composition. Suppress
   later mutable previews that would displace it; retain their typed state for
   the next frame.
7. While a blocking entry is unresolved, return no later stable fact as terminal
   history. After resolution, release all unacknowledged facts once in transcript
   order through the existing acknowledgement path.
8. Keep resolved/abandoned approvals and answered/cancelled questions as one
   terminal transcript fact. Do not persist UI selection or feedback drafts.
9. Make every non-terminal workflow state converge to a terminal presentation
   on interrupt or exit; do not leave queued, paused, or waiting-for-input cards
   permanently live.

### Retirement Track

- Delete the independent approval presentation queue after every request has a
  transcript identity and earliest-entry activation is proven.
- Delete question rendering from fixed chrome after the transcript is the sole
  visible owner; keep the existing state machine and key semantics.
- Delete input priority based solely on overlay type.

### Impact

- Does not change approval decisions, question answers, workflow execution, or
  durable workflow journals.
- Makes transcript order the single visible focus order.
- Requires explicit lifecycle tests because questions have no separate durable
  answered/cancelled event today.

### Verification

```bash
cargo test --package neo-tui --test workflow_transcript -- workflow_phase_and_terminal_facts_commit_once_by_projection_sequence --exact --nocapture
cargo test --package neo-tui --test progressive_transcript -- pending_approval_defers_later_facts_in_canonical_order --exact --nocapture
cargo test --package neo-tui --test todo_question -- earliest_blocking_entry_keeps_focus_across_later_requests --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::pending_approval_keeps_input_while_later_delegate_events_arrive --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::pending_question_keeps_input_while_later_workflow_events_arrive --exact --nocapture --include-ignored
```

### Commit

```bash
git add crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/progressive.rs crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/transcript/workflow_card.rs crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/src/transcript/approval_data.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/transcript/chrome_render.rs crates/neo-tui/src/shell/event_router.rs crates/neo-agent/src/modes/interactive/approval.rs crates/neo-agent/src/modes/interactive/questions.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-tui/tests/workflow_transcript.rs crates/neo-tui/tests/progressive_transcript.rs crates/neo-tui/tests/todo_question.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "fix(tui): preserve blocking transcript focus"
```

## Task 4: Close Every Remaining Live-Entry Path Without False Stability

### Files

- `crates/neo-tui/src/transcript/store.rs`
- `crates/neo-tui/src/transcript/presentation.rs`
- `crates/neo-tui/tests/progressive_transcript.rs`
- `crates/neo-tui/tests/transcript_pane.rs`
- `crates/neo-tui/tests/tool_cards.rs`

### Why And Change Necessity

The approved design requires every `Finalization::Live` producer to avoid an
unbounded suffix, but current assistant retry, thinking reopen, `LiveOutput`
eviction, and aggregate shell results do not prove append-only row identity.

### Repair Track

1. Preserve the existing live-model-attempt barrier for assistant and thinking
   content. Bound their preview; after the attempt is canonical, commit the
   assistant source and completed thinking entry once with existing proofs.
2. Keep ordinary running tools and shell commands bounded. Commit their canonical
   finalized card once; do not add a byte/line protocol in this task.
3. Keep tool grouping unchanged: adjacent tools commit only through their
   existing canonical group after every grouped tool is finalized.
4. Keep Compaction, RetryStatus, and connecting MCP startup bounded; commit only
   the typed terminal form. Preserve late-update rejection for settled MCP state.
5. Prove interruption, cancellation, retry, resize, and session exit remove or
   terminalize live projections without acknowledging mutable data.
6. Add an assertion that every live producer in `TranscriptEntry::finalization`
   is handled by a progressive projector, a blocking projector, or the bounded
   finalization fallback.

### Retirement Track

- Remove old tests that expect failed model-attempt text in terminal history or
  tool/shell live rows to survive by growing the global live suffix.
- Do not add producer sequence fields, a shell protocol revision, or a text
  deduplication heuristic without a separately approved design.

### Impact

- No provider, retry, shell runtime, or persistence change.
- This intentionally chooses bounded live preview over false progressive claims
  for sources without append-only proof.

### Verification

```bash
cargo test --package neo-tui --test transcript_pane -- retry_attempt_stays_out_of_terminal_history_until_message_finishes --exact --nocapture
cargo test --package neo-tui --lib -- transcript::presentation::tests::assistant_stable_prefix_never_rewinds_when_markdown_becomes_reference_based --exact --nocapture
cargo test --package neo-tui --test progressive_transcript -- every_live_entry_family_is_bounded_or_progressive --exact --nocapture
cargo test --package neo-tui --test tool_cards -- tool_call_live_output_reassembles_split_lines_and_ansi --exact --nocapture
cargo test --package neo-tui --test tool_cards -- shell_run_live_output_reassembles_split_control_sequences --exact --nocapture
```

### Commit

```bash
git add crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/tests/progressive_transcript.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-tui/tests/tool_cards.rs
git commit -m "fix(tui): bound mutable transcript entries"
```

## Task 5: Delete Automatic Alternate-Screen Overflow

### Files

- `crates/neo-tui/src/app.rs`
- `crates/neo-tui/src/transcript/pane.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`
- `crates/neo-tui/tests/terminal_frame.rs`
- `crates/neo-agent/src/modes/interactive/tests.rs`

### Why And Change Necessity

After live output is genuinely bounded, automatic alternate-screen overflow is
obsolete and is the direct owner of fixed chrome, mouse capture, missing native
scrollback, and wheel routing.

### Repair Track

1. Render ordinary frames directly from bounded history/live updates on the
   normal screen for ask, auto, and yolo modes.
2. Preserve manual transcript review precedence and Task Browser behavior.
3. Preserve mouse capture only when an explicit review surface requests it.

### Retirement Track

1. Delete `NeoTui::automatic_overflow` and its active/up/down methods.
2. Delete `TranscriptTerminalUpdate.live_overflow` and `has_live_frontier`, then
   delete latching, release, viewport frame composition, automatic fixed chrome,
   automatic review-surface marking, and automatic mouse capture.
3. Delete `handle_automatic_overflow_event` and its early input interception.
4. Delete `render_viewport_rows` and `viewport_splits_terminal_image` if the
   lingering-reference search confirms they have no explicit-review caller.
5. Replace tests whose premise is automatic alternate-screen entry; do not
   rename them while keeping the old behavior.

### Impact

- Normal screen returns wheel, selection, and scrollback ownership to the
  terminal.
- Explicit application-owned surfaces remain unchanged.

### Verification

```bash
cargo test --package neo-tui --test terminal_frame -- tall_live_projection_stays_on_normal_screen_without_mouse_capture --exact --nocapture
cargo test --package neo-tui --test terminal_frame -- transcript_browser_frame_is_bounded_and_marked_review_surface --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::tall_transcript_keeps_prompt_input_on_normal_screen --exact --nocapture --include-ignored
```

### Commit

```bash
git add crates/neo-tui/src/app.rs crates/neo-tui/src/transcript/pane.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-tui/tests/terminal_frame.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "fix(tui): keep ordinary transcript on normal screen"
```

## Task 6: Prove Native Scrollback, Dialog Focus, And Explicit Review End To End

### Files

- `crates/neo-tui/tests/terminal_scrollback.rs`
- `crates/neo-tui/tests/terminal_frame.rs`
- `crates/neo-tui/tests/progressive_transcript.rs`
- `crates/neo-agent/src/modes/interactive/tests.rs`

### Why And Change Necessity

Unit projection tests cannot prove terminal escape sequences, shell-line
retention, mouse capture, or controller input ownership under concurrent events.

### Repair Track

1. Replace `automatic_overflow_preserves_primary_scrollback_and_appends_deferred_history_once`
   with a tall live workload that seeds a shell launch line and proves normal
   scrollback retains it.
2. Replace `automatic_transcript_overflow_scrolls_without_blocking_prompt` with
   ask/yolo controller cases that prove no application viewport is activated.
3. Assert no automatic alternate-screen enter sequence and no automatic mouse
   capture for tall Delegate/workflow/approval content.
4. Assert progressively committed facts occur once and a complete final card
   does not occur after them.
5. Inject later Delegate/workflow events while an approval or question is
   active; prove selection, Enter, cancel, and feedback still reach the original
   request.
6. Preserve and rerun explicit `Ctrl+O`, Task Browser, terminal-write retry,
   resize, suspend/resume, and balanced enter/leave tests.

### Retirement Track

- Remove all remaining test names, assertions, fixtures, and comments that
  require automatic overflow, its latch, or fixed chrome.
- Do not weaken explicit-review mouse-capture assertions.

### Impact

- Verification-only task; no production behavior change expected.

### Verification

```bash
cargo test --package neo-tui --test terminal_scrollback -- native_scrollback_keeps_shell_and_progressive_history_exactly_once --exact --nocapture
cargo test --package neo-tui --test terminal_scrollback -- review_surface_transition_preserves_primary_scrollback --exact --nocapture
cargo test --package neo-tui --test terminal_scrollback -- review_acknowledgement_does_not_advance_normal_history_ledger --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::pending_approval_keeps_input_while_later_delegate_events_arrive --exact --nocapture --include-ignored
```

### Commit

```bash
git add crates/neo-tui/tests/terminal_scrollback.rs crates/neo-tui/tests/terminal_frame.rs crates/neo-tui/tests/progressive_transcript.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "test(tui): cover native progressive scrollback"
```

## Task 7: Retire References And Record The Landed Decision

### Files

- `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md` (new)
- `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md` (new)
- `docs/aegis/INDEX.md`

### Why And Change Necessity

This implementation supersedes an approved automatic-overflow architecture.
The landed decision and exact proof must prevent a future cleanup from restoring
the deleted alternate-screen path.

### Repair Track

1. Search all product and test code for automatic-overflow names, overflow flags,
   obsolete viewport routing, fixed-chrome assumptions, and duplicate question
   presentation.
2. Record one architecture decision: native normal-screen scrollback,
   type-proven progressive facts, bounded mutable live state, earliest blocking
   focus, and explicit-only alternate screens.
3. Record the exact implementation commits and focused verification in a landed
   baseline; state any manual-terminal or cross-platform evidence not run.
4. Update the Aegis index.

### Retirement Track

- Required zero-result search:

```bash
rg -n "automatic_overflow|live_overflow|has_live_frontier|handle_automatic_overflow_event|scroll_automatic_overflow" crates/neo-tui crates/neo-agent/src/modes/interactive
```

- Review every remaining alternate-screen and mouse-capture caller; each must be
  an explicit user-selected surface or Task Browser.
- Do not keep stale tests, comments, or superseded design claims as active
  implementation guidance.

### Impact

- Documentation and verification only; no further production change.
- Makes the explicit-only alternate-screen decision durable for future work.

### Verification

Run the exact tests listed in Tasks 1-6, then:

```bash
cargo fmt --all --check
git diff --check
git status --short
```

If the shared worktree prevents global formatting evidence, format/check only
the touched Rust files and report the blocker without changing unrelated files.
Do not run a broad workspace test as completion evidence.

### Commit

```bash
git add docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md docs/aegis/baseline/2026-07-31-native-terminal-transcript.md docs/aegis/INDEX.md
git commit -m "docs(tui): record native transcript presentation"
```

## Residual Risk And Stop Conditions

- Committing mutable data to native history is irreversible. Any missing typed
  finality proof is a stop condition, not permission to compare rendered text.
- Activity and workflow transitions can disappear between frames. Capture must
  happen at `TranscriptStore` update time before replacement or trimming.
- Question completion currently lacks a separate durable answered/cancelled
  runtime event. Keep the change UI-local and verify every existing lifecycle
  path; stop if implementation would require a persistence change.
- Real terminal selection cannot be fully proven by an in-memory virtual
  terminal. Automated evidence proves no mouse capture and no alternate-screen
  sequence; perform a macOS terminal smoke test before claiming physical
  selection behavior verified.
- Linux and Windows share the same presentation decisions, but native terminal
  smoke evidence must be reported separately from Rust test evidence.
- If another task claims ADR-0010 before Task 7, use the next available ADR
  number and update the index in the same commit; do not overwrite another
  decision.

## Execution Choice

After this plan is approved, choose one execution mode:

1. Subagent-driven execution in the current session, recommended for the
   independent Delegate-family, workflow/dialog, and terminal-test slices.
2. Inline execution by the primary agent in task order.

Do not begin product-code implementation until the user chooses.
