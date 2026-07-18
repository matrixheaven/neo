# Historical Tool Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use aegis:subagent-driven-development (recommended) or aegis:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Ctrl+O expand and collapse historical tool output without rewriting or duplicating native terminal scrollback, while preserving wheel-based history navigation and current live-tool behavior.

**Architecture:** Normal interactive output keeps the existing normal-screen append-only contract: committed rows are written once and never replayed. When Ctrl+O targets a transcript that already contains committed expandable rows, Neo opens a full-screen transcript review surface in the alternate screen. The review surface is an app-owned bounded viewport over a cloned canonical pane, so historical cards can be re-rendered in either state; leaving review restores the untouched native screen and scrollback. Live-only toggles continue to mutate the normal pane directly.

**Tech Stack:** Rust 2024, crossterm 0.29, existing TranscriptPane/TranscriptViewport, existing InlineTerminal and frame scheduler, no new dependencies.

## Global Constraints

- Preserve the project minimum Rust version and Windows/Linux/macOS support.
- Never emit CSI 3 J, replay acknowledged history blocks, or append replacement rows to normal-screen native scrollback.
- Keep TranscriptStore as the canonical session/export model; review rendering is a read-only clone plus explicit viewport state.
- Ctrl+O remains a global expand/collapse action for entries with real visual expansion semantics: ToolRun, ThinkingBlock, SkillActivation, and DelegateSwarm.
- DelegateCardComponent, DelegateGroupComponent, and WorkflowCardComponent do not receive new expansion behavior; dead expansion fields and branches are removed.
- No new compatibility flag, renderer fork, or dependency is introduced.
- Every production behavior change starts with a failing focused test and is verified with an exact target/name filter.
- Preserve unrelated worktree changes, currently crates/neo-agent-core/src/tools/read.rs.

## File/Interface Map

- Modify crates/neo-tui/src/transcript/entry/mod.rs: single is_expandable/set_expanded entry contract.
- Modify crates/neo-tui/src/transcript/pane.rs and store.rs: propagation, committed detection, and bounded review rows from a cloned pane.
- Create crates/neo-tui/src/transcript/browser.rs: TranscriptBrowserState (expanded plus TranscriptViewport).
- Modify crates/neo-tui/src/transcript/mod.rs: export browser state and remove obsolete presentation exports after migration.
- Modify crates/neo-tui/src/shell/overlay.rs, shell/mod.rs, and shell/dialog_factory.rs: full-screen TranscriptBrowser marker and lifecycle/accessors.
- Modify crates/neo-tui/src/app.rs: browser rendering before normal history presentation and TerminalFrame surface mode.
- Modify crates/neo-tui/src/screen_output/inline_terminal.rs and terminal_modes.rs: alternate-screen and mouse-capture transactions.
- Modify crates/neo-tui/src/input/mod.rs and input/raw_input.rs: SGR wheel decoding while review capture is active.
- Modify crates/neo-agent/src/modes/interactive/input.rs, mod.rs, and terminal_io.rs: browser input, immediate frame ordering, and mode transitions.
- Keep crates/neo-tui/src/transcript/presentation.rs as the sole normal-screen append-only commit ledger; remove only obsolete replay APIs and diagnostics after review rendering is wired.
- Update focused tests in neo-tui and neo-agent; update the immutable-scrollback design document.

---

### Task 1: Canonical Expandable State And Review Snapshot

Files:
- Modify crates/neo-tui/src/transcript/presentation.rs
- Create crates/neo-tui/src/transcript/browser.rs
- Modify crates/neo-tui/src/transcript/entry/mod.rs
- Modify crates/neo-tui/src/transcript/pane.rs
- Modify crates/neo-tui/src/transcript/store.rs
- Modify crates/neo-tui/src/transcript/presentation.rs (read-only committed-entry query)
- Modify crates/neo-tui/src/transcript/mod.rs
- Modify crates/neo-tui/src/transcript/event_handler.rs
- Modify crates/neo-tui/src/transcript/delegate_card.rs
- Modify crates/neo-tui/src/transcript/workflow_card.rs
- Test crates/neo-tui/tests/transcript_pane.rs and transcript_store.rs

Interfaces:
- TranscriptBrowserState::new(expanded: bool), expanded, toggle, scroll_up, scroll_down, and follow_bottom.
- TranscriptEntry::is_expandable(&self) -> bool and set_expanded(&mut self, bool) -> bool.
- TranscriptPane::has_committed_expandable_entries(&self) -> bool.
- TranscriptPane::render_browser_rows(&mut self, state: &mut TranscriptBrowserState, width: usize, height: usize) -> Vec<String>. It clones the pane, applies review state to the clone, renders canonical rows, bounds them with state.viewport, and consumes the source dirty bit without acknowledging normal history.

- [ ] Step 1: Write failing state and review tests.

Add tests equivalent to the following:

    #[test]
    fn browser_snapshot_expands_and_collapses_committed_tool_without_mutating_source() {
        let mut pane = TranscriptPane::new(80, 20);
        pane.push_tool_run("tool-1", "Read", Some("{\"path\":\"a\"}".to_owned()));
        let _ = pane.render_terminal_update(80, 20);
        let mut browser = TranscriptBrowserState::new(true);

        let expanded = pane.render_browser_rows(&mut browser, 80, 20).join("\n");
        assert!(expanded.contains("{\"path\":\"a\"}"));
        assert!(!pane.tool_output_expanded());

        browser.toggle();
        let collapsed = pane.render_browser_rows(&mut browser, 80, 20).join("\n");
        assert!(!collapsed.contains("{\"path\":\"a\"}"));
        assert!(!pane.tool_output_expanded());
    }

    #[test]
    fn browser_rows_are_bounded_and_scrollable() {
        let mut pane = TranscriptPane::new(80, 20);
        for index in 0..40 {
            pane.push_status(format!("row-{index}"));
        }
        let mut browser = TranscriptBrowserState::new(false);
        assert_eq!(pane.render_browser_rows(&mut browser, 80, 5).len(), 5);
        browser.scroll_up(usize::MAX);
        assert!(pane.render_browser_rows(&mut browser, 80, 5).join("\n").contains("row-0"));
    }

- [ ] Step 2: Run the focused tests and confirm RED.

    cargo test --package neo-tui --test transcript_pane browser_snapshot_expands_and_collapses_committed_tool_without_mutating_source --exact --nocapture
    cargo test --package neo-tui --test transcript_pane browser_rows_are_bounded_and_scrollable --exact --nocapture

Expected: compile failure because browser state, the entry expansion contract, and render_browser_rows do not exist.

- [ ] Step 3: Add the minimal browser state and one entry expansion contract.

Implement browser.rs as a small state object containing only expanded and TranscriptViewport. In TranscriptEntry, make is_expandable true only for ToolRun, ThinkingBlock, SkillActivation, and DelegateSwarm; make set_expanded mutate only those variants and report whether it changed. Delete expanded fields, is_expanded, and Expandable implementations from Delegate and Workflow components, and remove their pane branches.

Implement TranscriptPane::apply_expand_state_to_entry and set_tool_output_expanded by calling the entry contract. Ensure every new entry path uses the helper: direct tool creation, skill activation, thinking, and delegate swarm upsert. Preserve the existing snapshot on Delegate-to-Group replacement and do not add expansion to the group.

Implement render_browser_rows by cloning self, applying state.expanded on the clone, rendering the clone's canonical body rows, synchronizing state.viewport with row count and requested height, and returning only visible_row_range. Set the source dirty flag false after the snapshot is built; do not call presentation.acknowledge.

- [ ] Step 4: Run focused propagation checks.

    cargo test --package neo-tui --test transcript_pane browser_snapshot_expands_and_collapses_committed_tool_without_mutating_source --exact --nocapture
    cargo test --package neo-tui --test transcript_pane browser_rows_are_bounded_and_scrollable --exact --nocapture
    cargo test --package neo-tui --test transcript_store delegate_group_replacement_preserves_entry_identity --exact --nocapture
    cargo test --package neo-tui --test tool_cards single_read_respects_transcript_expansion_state --exact --nocapture

Expected: all named tests pass and no Delegate/Workflow expansion references remain outside their render tests.

- [ ] Step 5: Commit.

    git add crates/neo-tui/src/transcript/browser.rs crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/mod.rs crates/neo-tui/src/transcript/event_handler.rs crates/neo-tui/src/transcript/delegate_card.rs crates/neo-tui/src/transcript/workflow_card.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-tui/tests/transcript_store.rs crates/neo-tui/tests/tool_cards.rs
    git commit -m "fix: unify transcript expansion state"

---

### Task 2: Transcript Browser Surface And Terminal Mode Lifecycle

Files:
- Modify crates/neo-tui/src/shell/overlay.rs
- Modify crates/neo-tui/src/shell/mod.rs
- Modify crates/neo-tui/src/shell/dialog_factory.rs
- Modify crates/neo-tui/src/app.rs
- Modify crates/neo-tui/src/screen_output/inline_terminal.rs
- Modify crates/neo-tui/src/screen_output/terminal_modes.rs
- Modify crates/neo-tui/src/screen_output/mod.rs
- Test crates/neo-tui/tests/terminal_frame.rs and terminal_scrollback.rs

Interfaces:
- Add OverlayKind::TranscriptBrowser(TranscriptBrowserState) and NeoChromeState open/state/close accessors.
- Extend TerminalFrame with review_surface: bool. Review frames carry no history blocks.
- InlineTerminal::render_to transitions normal -> review with alternate screen and mouse capture, and review -> normal with symmetric cleanup. Neither transition writes CSI 2 J or CSI 3 J to the primary screen.

- [ ] Step 1: Write failing lifecycle tests.

    #[test]
    fn transcript_browser_frame_is_bounded_and_marked_review_surface() {
        let mut tui = test_tui_with_many_status_rows();
        tui.chrome_mut().open_transcript_browser(false);
        let frame = tui.render_terminal_frame_at(80, 12, Instant::now());
        assert!(frame.review_surface);
        assert!(frame.history.is_empty());
        assert!(frame.live.len() <= 12);
    }

    #[test]
    fn review_surface_transition_preserves_primary_scrollback() {
        let mut terminal = InlineTerminal::for_test(80, 12);
        let normal = TerminalFrame::with_surface(Vec::new(), vec!["normal".into()], None, false, None);
        terminal.render_to(&mut Vec::new(), &normal).unwrap();
        let review = TerminalFrame::with_surface(Vec::new(), vec!["review".into()], None, true, None);
        let mut bytes = Vec::new();
        terminal.render_to(&mut bytes, &review).unwrap();
        terminal.render_to(&mut bytes, &normal).unwrap();
        let output = String::from_utf8(bytes).unwrap();
        assert!(output.contains("?1049h"));
        assert!(output.contains("?1049l"));
        assert!(!output.contains("\x1b[3J"));
    }

- [ ] Step 2: Run exact tests and confirm RED.

    cargo test --package neo-tui --test terminal_frame transcript_browser_frame_is_bounded_and_marked_review_surface --exact --nocapture
    cargo test --package neo-tui --test inline_terminal review_surface_transition_preserves_primary_scrollback --exact --nocapture

Expected: compile failure for the new frame/surface APIs.

- [ ] Step 3: Add marker overlay and bounded frame path.

Add the marker state to OverlayKind without storing the canonical pane. In NeoTui::render_terminal_frame_at, handle the marker before the normal full-screen overlay branch: borrow browser state, ask TranscriptPane::render_browser_rows for at most height rows, apply the existing gutter, and return a review TerminalFrame with empty history. Keep other dialog overlays unchanged.

The browser state owns its viewport; NeoTui must not reuse native terminal scroll position. When the marker opens, initialize it at transcript tail; when it closes, do not mutate the canonical presentation ledger.

- [ ] Step 4: Add transactional alternate-screen and mouse-capture transitions.

Extend TerminalModeGuard with symmetric enter_review/leave_review methods using crossterm EnterAlternateScreen, LeaveAlternateScreen, EnableMouseCapture, and DisableMouseCapture. Track active review so suspend, resume, drop, and error paths always disable capture before leaving alternate screen. InlineTerminal resets only its live renderer anchor on leaving review; it must not replay TerminalFrame.history while re-anchoring normal output.

- [ ] Step 5: Run terminal lifecycle verification.

    cargo test --package neo-tui --test terminal_frame transcript_browser_frame_is_bounded_and_marked_review_surface --exact --nocapture
    cargo test --package neo-tui --test inline_terminal review_surface_transition_preserves_primary_scrollback --exact --nocapture
    cargo test --package neo-tui --lib -- screen_output::terminal_modes::tests::review_modes_are_symmetric --exact --nocapture

- [ ] Step 6: Commit.

    git add crates/neo-tui/src/shell/overlay.rs crates/neo-tui/src/shell/mod.rs crates/neo-tui/src/shell/dialog_factory.rs crates/neo-tui/src/app.rs crates/neo-tui/src/screen_output/inline_terminal.rs crates/neo-tui/src/screen_output/terminal_modes.rs crates/neo-tui/src/screen_output/mod.rs crates/neo-tui/tests/terminal_frame.rs crates/neo-tui/tests/terminal_scrollback.rs
    git commit -m "feat: add transcript review surface"

---

### Task 3: Ctrl+O Routing, Wheel Input, And Immediate Rendering

Files:
- Modify crates/neo-tui/src/input/mod.rs
- Modify crates/neo-tui/src/input/raw_input.rs
- Modify crates/neo-agent/src/modes/interactive/input.rs
- Modify crates/neo-agent/src/modes/interactive/mod.rs
- Modify crates/neo-agent/src/modes/interactive/terminal_io.rs
- Test crates/neo-agent/src/modes/interactive/tests.rs and crates/neo-tui/src/input/mod.rs

Interfaces:
- SGR button 64 maps to InputEvent::ScrollUp(3); button 65 maps to ScrollDown(3) while review capture is active. Other mouse events remain ignored.
- InteractiveController handles browser input before prompt editing when no approval/dialog owns focus: Ctrl+O toggles review, Esc closes it, wheel/PageUp/PageDown/Up/Down scroll it.
- A user input frame is rendered before drain_active_turn, so queued ToolExecutionFinished cannot delay a Ctrl+O visual change.

- [ ] Step 1: Write failing input and event-order tests.

    #[test]
    fn sgr_mouse_wheel_maps_to_transcript_scroll_events() {
        let mut parser = InputParser::new();
        assert_eq!(parser.feed_bytes(b"\x1b[<64;20;10M"), vec![InputEvent::ScrollUp(3)]);
        assert_eq!(parser.feed_bytes(b"\x1b[<65;20;10M"), vec![InputEvent::ScrollDown(3)]);
    }

Add an interactive regression that queues ToolExecutionFinished, sends Ctrl+O, renders once, and proves the first rendered frame already contains the toggled live card; the finished event may be drained only after that frame.

- [ ] Step 2: Run tests and confirm RED.

    cargo test --package neo-tui --lib -- input::tests::sgr_mouse_wheel_maps_to_transcript_scroll_events --exact --nocapture
    cargo test --package neo-agent --bin neo -- interactive::tests::ctrl_o_renders_before_queued_tool_finish --exact --nocapture

- [ ] Step 3: Parse wheel sequences and route browser input.

Recognize SGR mouse payloads before parse_key, mask modifier bits, and emit only the two wheel events. Add browser-specific handling ahead of prompt editing but after approval/dialog guards. Reuse browser scroll methods and mark the transcript dirty so the scheduler emits one immediate frame.

- [ ] Step 4: Render immediate user changes before asynchronous draining.

In run_terminal_loop_with_suspend, after handle_input_event returns and before drain_active_turn, call the existing render_due_frame path with the immediate request. Preserve coalescing for subsequent agent events. Update terminal_io to pass review_surface through every frame and to avoid acknowledging review frames.

- [ ] Step 5: Run focused interaction verification.

    cargo test --package neo-tui --lib -- input::tests::sgr_mouse_wheel_maps_to_transcript_scroll_events --exact --nocapture
    cargo test --package neo-agent --bin neo -- interactive::tests::ctrl_o_renders_before_queued_tool_finish --exact --nocapture
    cargo test --package neo-agent --bin neo -- interactive::tests::event_loop_dispatches_mouse_wheel_to_transcript_view --exact --nocapture
    cargo test --package neo-agent --bin neo -- interactive::tests::ctrl_o_enters_and_leaves_transcript_browser --exact --nocapture

- [ ] Step 6: Commit.

    git add crates/neo-tui/src/input/mod.rs crates/neo-tui/src/input/raw_input.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/terminal_io.rs crates/neo-agent/src/modes/interactive/tests.rs
    git commit -m "fix: route historical ctrl-o through review surface"

---

### Task 4: Finalize Immutable Boundaries And Review The Branch

Files:
- Modify crates/neo-tui/src/transcript/presentation.rs and streaming_prefix.rs only where the review surface needs a clean boundary
- Modify crates/neo-tui/src/transcript/mod.rs, pane.rs, app.rs, and screen_output/inline_terminal.rs
- Modify focused terminal/transcript/multi-agent tests
- Modify docs/aegis/specs/2026-07-13-immutable-terminal-scrollback-design.md

Interfaces:
- Normal frames retain append-only history until the review surface is entered; review frames carry no history blocks, and the centralized acknowledge_history boundary ignores them without advancing the normal ledger.
- TranscriptPresentation remains the normal-screen ledger. Its committed-revision mismatch guard remains a safety invariant for the primary screen, while browser snapshots bypass it without mutating or acknowledging it.
- No production or test code treats a committed revision mismatch as a user-visible history update; review rendering is the only path that reflows committed content.
- The design document states the final two-surface contract: primary native scrollback is immutable; historical reflow happens only in the app-owned review surface.

- [ ] Step 1: Write migration tests proving the old path is unreachable.

Add exact assertions to terminal_scrollback.rs that a committed tool remains byte-for-byte unchanged while a review frame expands and collapses it, and that review frames contain no history append blocks. Replace the old test that treats committed revision mismatch without replay as user-visible behavior.

- [ ] Step 2: Run migration tests and confirm RED against the old history-only behavior.

    cargo test --package neo-tui --test terminal_scrollback committed_tool_review_does_not_duplicate_native_scrollback --exact --nocapture
    cargo test --package neo-tui --test terminal_scrollback review_acknowledgement_does_not_advance_normal_history_ledger --exact --nocapture

Expected: assertion failures until the browser path is the only way to reflow committed content.

- [ ] Step 3: Remove only obsolete history-replay assumptions and migrate callers.

Keep the presentation ledger, finalized-block proof types, streaming-prefix module, and write-then-acknowledge transaction for normal-screen output. Remove any API that attempts to mutate an acknowledged block or replay it after Ctrl+O. Keep canonical TranscriptStore rendering and the browser clone as the only historical reflow paths. Update NeoTui, InlineTerminal, and interactive terminal I/O so normal frames append only newly produced rows and review frames render only bounded live rows.

- [ ] Step 4: Update design/spec and focused tests.

Document why an already emitted native row cannot be addressed, the review-surface transition, viewport ownership, input routing, and the no-CSI-2J/3J invariant. Cover both directions of Tool/Thinking/Skill/Swarm expansion, live and committed history, browser scrolling, suspend/resume symmetry, and no duplicate native rows.

- [ ] Step 5: Run scoped verification and commit.

    cargo test --package neo-tui --test terminal_scrollback committed_tool_review_does_not_duplicate_native_scrollback --exact --nocapture
    cargo test --package neo-tui --test transcript_pane browser_snapshot_expands_and_collapses_committed_tool_without_mutating_source --exact --nocapture
    cargo test --package neo-agent --bin neo -- interactive::tests::ctrl_o_enters_and_leaves_transcript_browser --exact --nocapture
    cargo fmt --all --check
    git diff --check

    git add crates/neo-tui/src/transcript crates/neo-tui/src/app.rs crates/neo-tui/src/screen_output crates/neo-tui/tests crates/neo-agent/src/modes/interactive docs/aegis/specs/2026-07-13-immutable-terminal-scrollback-design.md
    git commit -m "refactor: make historical transcript review explicit"

- [ ] Step 6: Whole-branch review and fix gate.

Run scripts/review-package "$(git merge-base HEAD HEAD~4)" HEAD, dispatch the resulting package to the strongest available reviewer, and fix every Critical or Important finding in one follow-up task. Re-run exact covering tests named by the reviewer, then cargo fmt --all --check and git diff --check. Do not claim completion while any review finding or verification command remains unresolved.

## Plan Self-Review

- The physical conflict between native append-only rows and in-place historical mutation is handled explicitly; no task proposes replaying committed blocks or clearing primary scrollback.
- Every new interface is named with its owning file and has a focused failing test before implementation.
- The only persistent state added for history review is TranscriptBrowserState; canonical transcript data remains in TranscriptStore.
- Dead Delegate/Workflow expansion fields are deleted rather than kept as a misleading compatibility path.
- The unrelated crates/neo-agent-core/src/tools/read.rs worktree change is outside every task's stage list and must remain untouched.
