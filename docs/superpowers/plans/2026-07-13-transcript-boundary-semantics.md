# Transcript Boundary Semantics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent in-place delegate, swarm, and workflow card updates from interrupting a streamed thinking block.

**Architecture:** Remove eager text-block completion from event routing. Let `TranscriptStore` establish a visible boundary only on the code paths that actually append a new transcript entry.

**Tech Stack:** Rust 2024, `neo-tui`, Cargo nextest

---

### Task 1: Protect Thinking Across In-Place Card Updates

**Files:**
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Modify: `crates/neo-tui/tests/workflow_transcript.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Modify: `crates/neo-tui/src/transcript/store.rs`

- [x] **Step 1: Write the failing regression test**

Add tests that start existing delegate, swarm, and workflow cards, begin a thinking block, interleave in-place updates with deltas, and assert continuous `ThinkingBlock` content plus updated card snapshots.

- [x] **Step 2: Run the exact tests and verify they fail**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript in_place_card_updates_preserve_active_thinking
cargo nextest run -p neo-tui --test workflow_transcript in_place_workflow_update_preserves_active_thinking
```

Expected: FAIL because progress routing finishes thinking and inserts separators before later deltas.

- [x] **Step 3: Centralize visible-boundary handling**

Remove eager `finish_active_text_blocks` calls from delegate/workflow event routing. In store upserts, retain boundary creation only on paths that call `push`; existing-entry updates and group replacements remain in place without changing active text state.

- [x] **Step 4: Run the exact tests and verify they pass**

Run the same exact nextest command.

Expected: PASS with continuous thinking content and updated progress cards.

- [x] **Step 5: Verify formatting and focused neighboring behavior**

Run the exact relevant `neo-tui` integration test filters and `cargo fmt --all --check`.

- [x] **Step 6: Commit**

Stage only the design, plan, regression test, and transcript implementation files. Commit with `fix(tui): preserve thinking across card updates`.
