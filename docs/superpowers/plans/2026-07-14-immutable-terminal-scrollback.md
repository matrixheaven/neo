# Immutable Terminal Scrollback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Neo's destructive whole-transcript redraw with append-only committed terminal history, a bounded mutable live surface, and request-driven rendering.

**Architecture:** `TranscriptStore` remains the canonical model and assigns stable entry IDs. A presentation ledger emits finalized blocks exactly once and renders only non-finalized blocks into a height-bounded live surface. `InlineTerminal` inserts committed rows above a live-only differential renderer; the controller schedules frames only for input, dirty state, or an actual animation deadline.

**Tech Stack:** Rust 2024, crossterm 0.29, pulldown-cmark, vt100 0.16 (test-only), Tokio, existing Neo terminal primitives.

**Design:** `docs/superpowers/specs/2026-07-13-immutable-terminal-scrollback-design.md`

**Git constraint:** Do not run `git add`, `git commit`, or any other Git mutation without explicit per-command user authorization. The checkpoints below stop at verified worktree changes.

---

## File Structure

- Create `crates/neo-tui/src/transcript/presentation.rs`: stable block IDs, final/live partitioning, commit acknowledgement, and live-height fitting.
- Create `crates/neo-tui/src/transcript/streaming_prefix.rs`: conservative markdown stable-prefix detection for incremental assistant commits.
- Modify `crates/neo-tui/src/transcript/store.rs`: stable `TranscriptEntryId` allocation and entry finalization queries.
- Modify `crates/neo-tui/src/transcript/entry/mod.rs`: presentation finalization for every entry variant.
- Modify `crates/neo-tui/src/transcript/pane.rs`: produce terminal updates while retaining full canonical snapshots for export/tests.
- Create `crates/neo-tui/src/screen_output/live_renderer.rs`: bounded live-region diff and cursor placement.
- Create `crates/neo-tui/src/screen_output/inline_terminal.rs`: canonical line-oriented history/live transaction coordinator and terminal mode RAII.
- Modify `crates/neo-tui/src/screen_output/mod.rs`: export the new terminal contract.
- Delete `crates/neo-tui/src/screen_output/frame_differ.rs`: remove synthetic whole-frame viewport and every `CSI 3 J` path.
- Modify `crates/neo-tui/src/app.rs`: return `TerminalFrame` and acknowledge committed block IDs.
- Modify `crates/neo-agent/src/modes/interactive/terminal_io.rs`: render/acknowledge one transaction.
- Create `crates/neo-agent/src/modes/interactive/frame_scheduler.rs`: coalesced and deadline frame requests.
- Modify `crates/neo-agent/src/modes/interactive/mod.rs`: remove unconditional periodic rendering.
- Modify `crates/neo-tui/Cargo.toml`: add `vt100 = "0.16.2"` as a dev dependency.
- Create `crates/neo-tui/tests/terminal_scrollback.rs`: virtual-terminal lifecycle regression tests.
- Modify focused transcript and interactive tests named in the tasks below.

## Task 1: Stable Entry Identity And Finalization

**Files:**
- Modify: `crates/neo-tui/src/transcript/store.rs`
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/src/transcript/shell_run.rs`
- Modify: `crates/neo-tui/src/transcript/workflow_card.rs`
- Test: `crates/neo-tui/src/transcript/store.rs`

- [ ] **Step 1: Write failing tests for stable IDs across mutation and removal**

Add unit tests that exercise the same structural operations as production:

```rust
#[test]
fn entry_ids_survive_in_place_updates_and_track_removal() {
    let mut store = TranscriptStore::new();
    store.push(TranscriptEntry::status("first"));
    store.push(TranscriptEntry::status("second"));
    let first = store.entry_id(0).expect("first id");
    let second = store.entry_id(1).expect("second id");

    let first_revision = store.entry_revision(0).expect("first revision");
    *store.entry_mut(0).expect("first entry") = TranscriptEntry::status("updated");
    assert_eq!(store.entry_id(0), Some(first));
    assert!(store.entry_revision(0).expect("updated revision") > first_revision);
    assert!(matches!(
        store.remove(0),
        Some(TranscriptEntry::Status { text, .. }) if text == "updated"
    ));
    assert_eq!(store.entry_id(0), Some(second));
}

#[test]
fn active_assistant_is_live_until_finish() {
    let mut store = TranscriptStore::new();
    store.start_assistant();
    assert_eq!(store.entry_finalization(0), Some(Finalization::Live));
    store.finish_assistant();
    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));
}
```

- [ ] **Step 2: Run the first exact test and confirm RED**

Run:

```bash
cargo test --package neo-tui --lib -- transcript::store::tests::entry_ids_survive_in_place_updates_and_track_removal --exact --nocapture --include-ignored
```

Expected: compile failure because `TranscriptEntryId`, `entry_id`,
`entry_revision`, and their bookkeeping do not exist.

- [ ] **Step 3: Add stable IDs and synchronize every structural mutation**

Implement the following shape in `store.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TranscriptEntryId(u64);

#[derive(Debug, Clone)]
pub struct TranscriptStore {
    entries: Vec<TranscriptEntry>,
    entry_ids: Vec<TranscriptEntryId>,
    entry_revisions: Vec<u64>,
    next_entry_id: u64,
    // existing fields remain
}

impl TranscriptStore {
    fn allocate_entry_id(&mut self) -> TranscriptEntryId {
        let id = TranscriptEntryId(self.next_entry_id);
        self.next_entry_id = self.next_entry_id.saturating_add(1);
        id
    }

    #[must_use]
    pub fn entry_id(&self, index: usize) -> Option<TranscriptEntryId> {
        self.entry_ids.get(index).copied()
    }

    #[must_use]
    pub fn entry_revision(&self, index: usize) -> Option<u64> {
        self.entry_revisions.get(index).copied()
    }
}
```

Update `push`, `start_assistant`, `start_thinking`, Delegate-to-group replacement, approval insertion, and `remove` so `entries`, `entry_ids`, and `render_cache` always have equal lengths. Replacement in the same slot retains its ID; insertion allocates a new ID; removal removes the matching ID.

Keep `entry_revisions` parallel as well. New entries start at revision `0`.
`invalidate_cache(index)` increments that entry's revision before clearing its
cache. Add a targeted `entry_mut(index)` and migrate the current broad
`entries_mut()` callers to targeted mutation or an explicit closure that bumps
only entries that actually changed. Do not revise every historical entry merely
because one live entry changed. Structural replacement increments the retained
slot's revision.

- [ ] **Step 4: Define finalization for all entry variants**

Add `TranscriptEntry::finalization()` and let `TranscriptStore::entry_finalization()` override active assistant/thinking indices:

```rust
#[must_use]
pub fn entry_finalization(&self, index: usize) -> Option<Finalization> {
    if self.active_assistant == Some(index) || self.active_thinking == Some(index) {
        return Some(Finalization::Live);
    }
    self.entries.get(index).map(TranscriptEntry::finalization)
}
```

The exhaustive entry match must classify:

```rust
match self {
    Self::ThinkingBlock { phase: ThinkingPhase::Streaming, .. } => Finalization::Live,
    Self::McpStartupStatus {
        data: McpStartupStatusData {
            phase: McpStartupPhase::Connecting,
            ..
        },
    } => Finalization::Live,
    Self::Compaction { phase, percent, .. }
        if !(*phase == Some(neo_agent_core::CompactionPhase::Applying)
            && *percent >= 100) =>
    {
        Finalization::Live
    }
    Self::ToolRun { component } => component.finalization(),
    Self::ShellRun { component } => component.finalization(),
    Self::ApprovalPrompt(data) if data.resolved.is_none() => Finalization::Live,
    Self::Delegate { component } => component.presentation_finalization(),
    Self::DelegateGroup { component } => component.presentation_finalization(),
    Self::DelegateSwarm { component } => component.presentation_finalization(),
    Self::Workflow { component } => component.presentation_finalization(),
    _ => Finalization::Finalized,
}
```

Reuse the existing `primitive::Finalization` and `Component::finalization`
implementations on Tool, Delegate, DelegateGroup, DelegateSwarm, and Workflow
components. Do not add a second lifecycle enum or parallel card API. Add only
`ShellRunComponent::finalization()`, returning `Live` for
`ShellRunState::Running` and `Finalized` for `Finished`.

- [ ] **Step 5: Run the two exact unit tests and confirm GREEN**

Run each command separately:

```bash
cargo test --package neo-tui --lib -- transcript::store::tests::entry_ids_survive_in_place_updates_and_track_removal --exact --nocapture --include-ignored
cargo test --package neo-tui --lib -- transcript::store::tests::active_assistant_is_live_until_finish --exact --nocapture --include-ignored
```

Expected: each reports one passed test.

- [ ] **Step 6: Checkpoint**

Review `git diff --check` for these files. Do not stage or commit.

## Task 2: Presentation Ledger And Two-Phase Commit Acknowledgement

**Files:**
- Create: `crates/neo-tui/src/transcript/presentation.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Test: `crates/neo-tui/src/transcript/presentation.rs`

- [ ] **Step 1: Write failing tests for canonical-frontier history and acknowledgement**

```rust
#[test]
fn finalized_entries_wait_for_ack_and_live_entries_stay_live() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status("ready");
    pane.start_assistant_message();
    pane.append_assistant_delta("partial");

    let first = pane.render_terminal_update(80, 12);
    let history_text = first
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(strip_ansi(&history_text).contains("ready"));
    assert!(strip_ansi(&first.live.join("\n")).contains("partial"));

    let retry = pane.render_terminal_update(80, 12);
    assert_eq!(retry.history, first.history, "unacked history must retry");

    pane.acknowledge_history(&first.history);
    assert!(pane.render_terminal_update(80, 12).history.is_empty());
}
```

Add a second test with a running Delegate followed by a static status. It must emit the status immediately, keep the Delegate live, and emit the final Delegate card only after its state becomes terminal.

- [ ] **Step 2: Run the exact acknowledgement test and confirm RED**

```bash
cargo test --package neo-tui --lib -- transcript::presentation::tests::finalized_entries_wait_for_ack_and_live_entries_stay_live --exact --nocapture --include-ignored
```

Expected: compile failure because terminal presentation types do not exist.

- [ ] **Step 3: Implement presentation types**

Create focused types in `presentation.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TranscriptBlockId {
    Entries(Vec<TranscriptEntryId>),
    AssistantSegment {
        entry: TranscriptEntryId,
        source_start: usize,
        source_end: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedBlock {
    pub id: TranscriptBlockId,
    pub proof: FinalizedBlockProof,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizedBlockProof {
    EntryRevisions(Vec<u64>),
    AssistantSource(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptTerminalUpdate {
    pub history: Vec<FinalizedBlock>,
    pub live: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TranscriptPresentation {
    committed: BTreeMap<TranscriptBlockId, FinalizedBlockProof>,
    assistant_offsets: BTreeMap<TranscriptEntryId, usize>,
    diagnostics: VecDeque<String>,
}
```

Build visual blocks before partitioning. Consecutive unsuppressed `ToolRun` entries form one block whose ID contains every member ID and whose finalization is `Finalized` only when all members are finalized. Other entries form one-entry blocks. Suppressed tool entries produce no block.

Full-entry and tool-group blocks carry `EntryRevisions`. Assistant segments use
their byte range as identity and carry the exact immutable source slice as
`AssistantSource`, so later deltas can advance the same entry without colliding
with or silently changing already committed stable segments.

- [ ] **Step 4: Implement two-phase acknowledgement**

`render_terminal_update` renders finalized, uncommitted blocks into `history` but does not mutate `committed`. `acknowledge_history` inserts only IDs returned by a successful terminal write:

```rust
pub fn acknowledge_history(&mut self, blocks: &[FinalizedBlock]) {
    for block in blocks {
        self.committed.insert(block.id.clone(), block.proof.clone());
        if let TranscriptBlockId::AssistantSegment { entry, source_end, .. } = &block.id {
            self.assistant_offsets
                .entry(*entry)
                .and_modify(|offset| *offset = (*offset).max(*source_end))
                .or_insert(*source_end);
        }
    }
}
```

If a committed full-entry or tool-group block later has a different revision,
exclude it from output and record one bounded diagnostic rather than rewriting
history. Assistant segment revisions may advance after the segment is
committed; validate those segments against the stored `AssistantSource` proof
instead. `assistant_offsets` advances only during acknowledgement, never while
merely constructing a frame.

- [ ] **Step 5: Preserve the canonical full-frame API**

Keep `TranscriptPane::render_frame` and `frame_ansi_lines` as canonical snapshot helpers for export and existing rendering tests. Add the terminal-only `render_terminal_update` path; do not make snapshots depend on committed presentation state.

- [ ] **Step 6: Run both exact presentation tests and confirm GREEN**

```bash
cargo test --package neo-tui --lib -- transcript::presentation::tests::finalized_entries_wait_for_ack_and_live_entries_stay_live --exact --nocapture --include-ignored
cargo test --package neo-tui --lib -- transcript::presentation::tests::living_card_holds_later_blocks_in_canonical_order_until_it_finishes --exact --nocapture --include-ignored
```

Expected: each reports one passed test.

- [ ] **Step 7: Checkpoint**

Run `git diff --check` on the four touched transcript files. Do not stage or commit.

## Task 3: Stable Assistant Prefix And Bounded Live Surface

**Files:**
- Create: `crates/neo-tui/src/transcript/streaming_prefix.rs`
- Modify: `crates/neo-tui/src/transcript/presentation.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Test: `crates/neo-tui/src/transcript/streaming_prefix.rs`
- Test: `crates/neo-tui/tests/transcript_pane.rs`

- [ ] **Step 1: Write failing stable-prefix tests**

Cover conservative behavior, not optimistic markdown guessing:

```rust
#[test]
fn complete_plain_paragraph_is_stable_but_open_markdown_tail_is_not() {
    assert_eq!(stable_prefix_len("first paragraph\n\nsecond"), "first paragraph\n\n".len());
    assert_eq!(stable_prefix_len("```rust\nfn main() {}\n"), 0);
    assert_eq!(stable_prefix_len("[link][target]\n\n[target]: /later"), 0);
}
```

Add an integration test that streams more than one terminal height of plain paragraphs and asserts earlier finalized rows move to `history` while only the tail stays live.

- [ ] **Step 2: Run the exact unit test and confirm RED**

```bash
cargo test --package neo-tui --lib -- transcript::streaming_prefix::tests::complete_plain_paragraph_is_stable_but_open_markdown_tail_is_not --exact --nocapture --include-ignored
```

Expected: compile failure because `stable_prefix_len` does not exist.

- [ ] **Step 3: Implement conservative stable-prefix detection**

Use `pulldown_cmark::Parser::into_offset_iter` to find top-level block ends. Accept a boundary only when it ends before EOF, the source after it contains a blank-line separator, all fenced/code/html blocks are closed, and the accepted prefix has no unresolved reference-link event. Return byte offsets on UTF-8 boundaries. Complex or ambiguous input returns `0`; message completion remains the universal flush.

Expose this exact contract:

```rust
#[must_use]
pub fn stable_prefix_len(markdown: &str) -> usize;
```

- [ ] **Step 4: Track per-assistant committed source offsets**

Extend presentation state with an assistant offset keyed by `TranscriptEntryId`. Render the stable source prefix as finalized history blocks and render only the suffix as live. The final message emits the remaining suffix once. Never derive stability from ANSI-string common prefixes.

- [ ] **Step 5: Bound the aggregate live surface**

Fit live blocks into:

```rust
let live_budget = terminal_height.saturating_sub(chrome_height).max(1);
```

Retain complete compact headers for running Tool/Delegate/Swarm cards, then retain the newest body rows that fit. Insert one muted omitted-row line per truncated block. Do not mark truncated rows committed.

- [ ] **Step 6: Run exact prefix and live-budget tests and confirm GREEN**

```bash
cargo test --package neo-tui --lib -- transcript::streaming_prefix::tests::complete_plain_paragraph_is_stable_but_open_markdown_tail_is_not --exact --nocapture --include-ignored
cargo test --package neo-tui --test transcript_pane -- streaming_assistant_commits_stable_prefix_and_bounds_live_tail --exact --nocapture --include-ignored
```

Expected: each reports one passed test.

- [ ] **Step 7: Checkpoint**

Run `git diff --check` on the streaming and presentation files. Do not stage or commit.

## Task 4: Coordinated Line-Oriented History Commit

**Files:**
- Create: `crates/neo-tui/src/screen_output/inline_terminal.rs`
- Create: `crates/neo-tui/src/screen_output/live_renderer.rs`
- Modify: `crates/neo-tui/src/screen_output/mod.rs`
- Modify: `crates/neo-tui/Cargo.toml`
- Test: `crates/neo-tui/tests/terminal_scrollback.rs`

- [ ] **Step 1: Add the test-only virtual terminal dependency**

Add under `[dev-dependencies]`:

```toml
vt100 = "0.16.2"
```

- [ ] **Step 2: Write a failing transaction-level ghost test**

Seed shell content, draw a multi-row live surface away from the physical bottom,
then commit history while replacing the live surface. Assert old live rows are
absent and the final order is history, current live, composer, with no non-empty
content below the composer.

- [ ] **Step 3: Run the exact test and confirm RED**

```bash
cargo test --package neo-tui --test terminal_scrollback -- history_commit_does_not_leave_ghost_live_rows_above_terminal_bottom --exact --nocapture --include-ignored
```

Expected before the coordinated path: old live rows remain above the current
frame or committed history appears below the composer.

- [ ] **Step 4: Implement one coordinated transaction**

When finalized rows exist, clone renderer state, clear only the current known
live surface, append history with normal CRLF scrolling, and redraw bounded
live rows. Write and flush once, then swap renderer state only on success. Do
not retain scrolling-region, reverse-index, ConPTY-specialized, or capability-
selected history paths.

- [ ] **Step 5: Add a vt100 lifecycle test**

Seed 40 shell rows into a 10-row parser, establish a 3-row live surface, and
insert 30 history rows across multiple transactions. To inspect all retained
rows, call `screen_mut().set_scrollback(usize::MAX)`, read the clamped maximum
from `screen().scrollback()`, and collect `screen().contents()` at every offset
from that maximum back to zero. Assert the combined text contains the seed
sentinel and every committed row. Feed every generated byte sequence through
`vt100::Parser::process`.

Assert generated output never emits `CSI 2 J`, `CSI 3 J`, or absolute cursor
addressing into committed history.

- [ ] **Step 6: Run exact transaction and virtual-terminal tests and confirm GREEN**

```bash
cargo test --package neo-tui --test terminal_scrollback -- history_commit_does_not_leave_ghost_live_rows_above_terminal_bottom --exact --nocapture --include-ignored
cargo test --package neo-tui --test terminal_scrollback -- shell_and_committed_history_survive_live_updates_resize_and_exit --exact --nocapture --include-ignored
```

Expected: each reports one passed test.

- [ ] **Step 7: Checkpoint**

Inspect generated sequences and `git diff --check`. Do not stage or commit.

## Task 5: Bounded Live Renderer And Transactional Inline Terminal

**Files:**
- Create: `crates/neo-tui/src/screen_output/live_renderer.rs`
- Create: `crates/neo-tui/src/screen_output/inline_terminal.rs`
- Modify: `crates/neo-tui/src/screen_output/mod.rs`
- Modify: `crates/neo-tui/src/app.rs`
- Modify: `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- Test: `crates/neo-tui/src/screen_output/live_renderer.rs`
- Test: `crates/neo-tui/src/screen_output/inline_terminal.rs`

- [ ] **Step 1: Write failing live-only and write-failure tests**

```rust
#[test]
fn unchanged_live_frame_emits_no_bytes() {
    let mut renderer = LiveRenderer::new(80, 24);
    renderer.render_to(&mut Vec::new(), vec!["live".into()], None).unwrap();
    let mut second = Vec::new();
    renderer.render_to(&mut second, vec!["live".into()], None).unwrap();
    assert!(second.is_empty());
}

#[test]
fn failed_transaction_does_not_advance_history_or_live_state() {
    let mut terminal = InlineTerminal::for_test(80, 24);
    let before = terminal.state_for_test();
    let mut writer = FailAfterBytes::new(4);
    assert!(terminal.render_to(&mut writer, sample_frame()).is_err());
    assert_eq!(terminal.state_for_test(), before);
}
```

- [ ] **Step 2: Run the exact no-op test and confirm RED**

```bash
cargo test --package neo-tui --lib -- screen_output::live_renderer::tests::unchanged_live_frame_emits_no_bytes --exact --nocapture --include-ignored
```

Expected: compile failure because `LiveRenderer` does not exist.

- [ ] **Step 3: Implement live-only differential rendering**

`LiveRenderer` accepts at most terminal-height rows, diffs only those rows, uses `CSI 2 K` for changed/removed live lines, and positions the cursor relative to the live anchor. It never owns or receives committed history. Pure growth creates new physical rows with CRLF rather than cursor-down. A resize reuses the old anchor only when its geometry remains provable; otherwise it starts a fresh line without cursor-up or erase-display against unknown rows. It never uses `2 J` or `3 J`.

- [ ] **Step 4: Define the app-to-terminal frame contract**

Export:

```rust
#[derive(Debug, Clone)]
pub struct TerminalFrame {
    pub history: Vec<FinalizedBlock>,
    pub live: Vec<String>,
    pub cursor: Option<CursorPos>,
    pub next_animation_deadline: Option<Instant>,
}
```

`NeoTui::render_terminal_frame(width, height)` combines `TranscriptTerminalUpdate.live` with chrome and offsets the chrome cursor. `NeoTui::acknowledge_history(frame)` forwards successfully written block IDs to the pane.
The deadline is present only while a visible spinner, elapsed timer, or live
card animation can actually change pixels.

- [ ] **Step 5: Implement transactional coordination**

`InlineTerminal::render_to` builds history and live output against cloned state, wraps both in synchronized output only when bytes are non-empty, writes once, flushes once, then swaps in next state. On error it emits a best-effort synchronized-output end marker but does not retry or purge.

Keep the existing terminal-mode enter/leave tests and add an assertion that no
normal-screen enter or leave sequence enables mouse tracking (`?1000`, `?1002`,
`?1003`, or `?1006`). Native wheel scrolling and selection remain terminal-owned.

- [ ] **Step 6: Wire `NeoTerminal::draw_tui`**

Replace the whole-frame call with:

```rust
let frame = tui.render_terminal_frame(usize::from(cols), usize::from(rows));
self.tui.render(&frame)?;
tui.acknowledge_history(&frame);
```

Rename the field type from `TuiRenderer` to `InlineTerminal`; preserve title synchronization and terminal-mode RAII.

- [ ] **Step 7: Run exact renderer and transaction tests and confirm GREEN**

```bash
cargo test --package neo-tui --lib -- screen_output::live_renderer::tests::unchanged_live_frame_emits_no_bytes --exact --nocapture --include-ignored
cargo test --package neo-tui --lib -- screen_output::inline_terminal::tests::failed_transaction_does_not_advance_history_or_live_state --exact --nocapture --include-ignored
```

Expected: each reports one passed test.

- [ ] **Step 8: Checkpoint**

Run `git diff --check` on screen output, app, and terminal I/O files. Do not stage or commit.

## Task 6: Request-Driven Frame Scheduler

**Files:**
- Create: `crates/neo-agent/src/modes/interactive/frame_scheduler.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-tui/src/app.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Test: `crates/neo-agent/src/modes/interactive/frame_scheduler.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Write failing scheduler tests**

```rust
#[test]
fn idle_poll_does_not_request_a_frame() {
    let now = Instant::now();
    let mut scheduler = FrameScheduler::new(now, Duration::from_millis(33));
    assert!(!scheduler.take_due(now + Duration::from_secs(1)));
}

#[test]
fn coalesced_requests_wait_but_immediate_requests_do_not() {
    let now = Instant::now();
    let mut scheduler = FrameScheduler::new(now, Duration::from_millis(33));
    scheduler.request_coalesced();
    assert!(!scheduler.take_due(now + Duration::from_millis(10)));
    scheduler.request_immediate();
    assert!(scheduler.take_due(now + Duration::from_millis(10)));
}
```

- [ ] **Step 2: Run the exact idle test and confirm RED**

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::frame_scheduler::tests::idle_poll_does_not_request_a_frame --exact --nocapture --include-ignored
```

Expected: compile failure because `FrameScheduler` does not exist.

- [ ] **Step 3: Implement the scheduler**

Use explicit request state:

```rust
pub struct FrameScheduler {
    last_frame: Instant,
    min_interval: Duration,
    immediate: bool,
    coalesced: bool,
    animation_deadline: Option<Instant>,
}
```

`request_immediate`, `request_coalesced`, and `request_animation_at` only set state. `take_due(now)` clears a due request and updates `last_frame`. `poll_timeout(now, maximum)` returns the smaller of the input poll budget and next frame deadline.

- [ ] **Step 4: Replace the unconditional event-loop condition**

Input and resize request immediate frames. Drained transcript/tool events and
completed async picker/probe state request coalesced frames. Visible live
animation schedules the deadline returned by `TerminalFrame`; no live animation
schedules none. Remove unconditional `advance_activity_frame()` and the
`dirty || elapsed` expression. Advance animation state only when an animation
request becomes due. Mutation paths must explicitly report whether they changed
visible state; do not retain a periodic catch-all render.

- [ ] **Step 5: Add an event-loop regression test**

Use a `TerminalEvents` fake that returns several timeout polls before an interrupt. Count render callback invocations and assert only the initial frame is rendered during idle polls.

- [ ] **Step 6: Run exact scheduler and event-loop tests and confirm GREEN**

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::frame_scheduler::tests::idle_poll_does_not_request_a_frame --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::idle_terminal_polling_does_not_render_repeated_frames --exact --nocapture --include-ignored
```

Expected: each reports one passed test.

- [ ] **Step 7: Checkpoint**

Run `git diff --check` on scheduler and event-loop files. Do not stage or commit.

## Task 7: Delete The Whole-Frame Renderer And Purge Semantics

**Files:**
- Delete: `crates/neo-tui/src/screen_output/frame_differ.rs`
- Modify: `crates/neo-tui/src/screen_output/mod.rs`
- Modify: `crates/neo-tui/src/screen_output/debug_log.rs`
- Modify: `crates/neo-tui/src/screen_output/kitty_image.rs`
- Modify: `crates/neo-tui/tests/transcript.rs`
- Modify: `crates/neo-agent/src/modes/interactive/snapshot.rs`

- [ ] **Step 1: Move reusable primitives before deleting the file**

Move `CursorPos`, `CURSOR_MARKER`, line-width normalization, terminal protocol guards, and Windows input-mode restoration into the focused new modules. Keep Kitty image helpers only if referenced by committed/live rendering.

- [ ] **Step 2: Delete obsolete state and tests**

Remove `ViewportState`, `previous_viewport_top`, `max_lines_rendered`, `clear_on_shrink`, full-render methods, `force_full_redraw`, and tests that assert `ESC[2J ESC[H ESC[3J`. Do not translate them into compatibility tests.

- [ ] **Step 3: Replace the old full-history transcript assertion**

Change `transcript_render_frame_preserves_full_history_for_terminal_scrollback` into two focused assertions: canonical snapshot still contains every entry, while terminal updates move finalized blocks into `history` and keep only live blocks in `live`.

- [ ] **Step 4: Run exact replacement tests**

```bash
cargo test --package neo-tui --test transcript -- canonical_snapshot_retains_full_history_after_terminal_commit --exact --nocapture --include-ignored
cargo test --package neo-tui --test transcript -- terminal_update_does_not_replay_committed_history --exact --nocapture --include-ignored
```

Expected: each reports one passed test.

- [ ] **Step 5: Prove purge code is absent**

Run:

```bash
rg -n '\\x1b\[3J|CSI 3 J|ClearType::Purge' crates/neo-tui/src crates/neo-agent/src
```

Expected: no runtime matches. Test fixtures may construct a forbidden sequence only to assert rejection; keep those fixtures in test-only code.

- [ ] **Step 6: Checkpoint**

Run `git diff --check`. Do not stage or commit.

## Task 8: End-To-End Scrollback, Resize, Exit, And `/clear` Invariants

**Files:**
- Modify: `crates/neo-tui/tests/terminal_scrollback.rs`
- Modify: `crates/neo-tui/src/screen_output/inline_terminal.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs` and the `/new` session-reset owner discovered by CodeGraph before editing

- [ ] **Step 1: Write the full lifecycle regression test**

The virtual-terminal test must:

1. Seed at least 40 shell rows with unique sentinels.
2. Start an inline terminal at 80x12.
3. Commit at least 30 transcript rows.
4. Apply at least 200 changing live frames.
5. Resize to 50x8 and then 100x20.
6. Commit a final tool card.
7. Exit and restore terminal modes.
8. Assert every shell and committed sentinel remains in screen plus scrollback.
9. Assert the concatenated output never contains `\x1b[3J`.

- [ ] **Step 2: Run the lifecycle test and confirm RED if any invariant remains**

```bash
cargo test --package neo-tui --test terminal_scrollback -- shell_and_committed_history_survive_live_updates_resize_and_exit --exact --nocapture --include-ignored
```

Expected before final fixes: failure naming the first violated history or cursor invariant.

- [ ] **Step 3: Make `/clear` logical-only**

Trace `/clear` through its current `/new` alias with CodeGraph; it is unrelated
to the `KeybindingAction::AppClear` editor/exit gesture. Clear canonical/live
Neo state and reset the presentation ledger for future entries without sending
terminal purge or replay sequences. Already committed rows remain native
scrollback.

- [ ] **Step 4: Cover suspend/resume and interrupted finalization**

Add exact tests that suspend clears only the live region, resume reanchors without replaying history, and shutdown converts remaining live cards into immutable interrupted snapshots before final output.

- [ ] **Step 5: Run the focused final test set**

Run each command separately:

```bash
cargo test --package neo-tui --test terminal_scrollback -- shell_and_committed_history_survive_live_updates_resize_and_exit --exact --nocapture --include-ignored
cargo test --package neo-tui --test terminal_scrollback -- suspend_resume_preserves_committed_history --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_clear_does_not_request_terminal_scrollback_purge --exact --nocapture --include-ignored
```

Expected: each reports one passed test.

- [ ] **Step 6: Run format and narrow compilation checks**

```bash
cargo fmt --all --check
cargo test --package neo-tui --lib -- screen_output::inline_terminal::tests --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::frame_scheduler::tests --nocapture
```

The two test commands name exactly one package and one target. Use their exact test paths as the final evidence; the module filters here are iteration aids, not final verification evidence.

- [ ] **Step 7: Manual terminal matrix**

Run Neo in Terminal.app, iTerm2, Ghostty, WezTerm, Windows Terminal, tmux, and zellij. During a running tool and DelegateSwarm, scroll to committed history and drag a selection across several rows. Verify no Neo refresh moves the viewport, expands the selection, or clears it. Record terminals whose explicit scroll-on-output setting overrides native scroll position.

- [ ] **Step 8: Final checkpoint**

Review the complete diff for scope and deletion of the old path. Do not stage, commit, push, or perform any other Git mutation without explicit authorization.
