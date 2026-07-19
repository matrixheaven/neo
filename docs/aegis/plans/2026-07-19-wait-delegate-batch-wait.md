# WaitDelegate Batch Wait Implementation Plan

**Goal:** Replace the singular `WaitDelegate.id` contract with canonical batch
`ids`, using wait-all semantics, one global deadline, and lossless partial
results.

**Architecture:** Keep admission, polling, aggregation, and result formatting
inside the existing `WaitDelegate` owner. Keep `DelegateGroup` presentation-only
and preserve visible card layouts.

**Tech Stack:** Rust 2024, Tokio, Serde, Schemars, existing Neo tool/result APIs.

**Baseline/Authority Refs:**
`docs/aegis/specs/2026-07-19-wait-delegate-batch-wait-brief.md`, root
`AGENTS.md`, `docs/en/reference/tools.md`, `docs/zh/reference/tools.md`.

**Compatibility Boundary:** This is a canonical replacement: singular `id` is
deleted without an alias or fallback. Historical session records need no
migration because tool calls are not re-executed during replay.

**TDD Route:**
- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: the user did not request strict TDD; focused contract regressions are sufficient.
- Verification: exact core and TUI tests plus schema and lingering-reference checks.

## Readiness

- Intent Lock: one `WaitDelegate` call joins one or more known agents/swarms.
- Scope Fence: no group runtime, wait-any mode, card redesign, or persistence migration.
- Canonical Owner: `crates/neo-agent-core/src/tools/delegate_controls.rs`.
- Retirement: delete the singular input path and migrate repository callers.
- Change Necessity: schema text alone cannot batch waits; the existing owner
  must aggregate target snapshots under one deadline.
- Existence Check: reuse `WaitDelegate`; adding a group owner is rejected.
- Complexity: edit in place, but extract small local helpers if needed to keep
  the existing tool method from growing another large branch.

## Task 1: Replace the runtime contract

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`

**Steps:**
1. Replace `id: String` with validated `ids: Vec<String>`.
2. Snapshot every target in input order using existing agent/swarm formatters.
3. Poll all targets against one deadline until all are terminal.
4. Return one stable batch envelope for all-terminal, timeout, and not-found outcomes.

**Verification:** Covered by Task 2 exact tests.

## Task 2: Pin schema, timeout, and partial results

**Files:**
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

**Steps:**
1. Update existing single-swarm wait expectations to the batch envelope.
2. Change the timeout regression to wait on one completed and one running agent,
   proving ordered partial results and a single `wait_timed_out` outcome.
3. Assert the generated schema requires `ids` and does not expose `id`.
4. Migrate existing background delegate/swarm callers to `ids` and update only
   assertions that read fields moved under the batch `items` envelope.

**Verification:**
```bash
cargo test --package neo-agent-core --test multi_agent_runtime -- swarm_result_shape_matches_between_foreground_wait_and_task_output --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_runtime -- wait_delegate_timeout_preserves_completed_partial_results --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_runtime -- multi_agent_tool_descriptions_explain_contract_without_docs --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_background -- restored_running_delegate_is_reported_lost_with_resume_hint --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_background -- wait_and_task_output_return_swarm_aggregate_and_items --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::delegate_controls::tests::wait_delegate_validates_ids_and_returns_unknown_targets --exact --nocapture
```

## Task 3: Preserve presentation and documentation

**Files:**
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs`
- Modify: `crates/neo-tui/tests/tool_cards.rs`
- Modify: `docs/en/reference/tools.md`
- Modify: `docs/zh/reference/tools.md`

**Steps:**
1. Read agent titles and nested swarm child titles from the batch envelope while
   preserving the existing WaitDelegate header layout.
2. Update existing title fixtures to the canonical `ids` input and envelope.
3. Document batch wait-all and global timeout semantics in both languages.

**Verification:**
```bash
cargo test --package neo-tui --test tool_cards -- wait_delegate_header_uses_result_title --exact --nocapture
cargo test --package neo-tui --test tool_cards -- wait_delegate_header_fits_every_swarm_item_title --exact --nocapture
```

## Final Checks And Commit

```bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/delegate_controls.rs crates/neo-agent-core/tests/multi_agent_runtime.rs crates/neo-agent-core/tests/multi_agent_background.rs crates/neo-tui/src/transcript/tool_renderers.rs crates/neo-tui/tests/tool_cards.rs
rg -n 'WaitDelegate.*"id"|"WaitDelegate".*\{"id"' crates --glob '*.rs'
git diff --check
git diff --cached --check
```

Stage only the files named by this plan and commit once with a conventional
`feat(agent): batch WaitDelegate targets` message.
