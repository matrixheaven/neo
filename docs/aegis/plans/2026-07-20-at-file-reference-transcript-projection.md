# At File Reference Transcript Projection Implementation Plan

**Goal:** Keep `@` file references compact in live and replayed user transcript messages while preserving expanded file snapshots for the model and durable context.

**Architecture:** One `AgentMessage::User` remains the durable owner. Its canonical `content` carries the expanded model payload; an optional `display_text` carries the human transcript projection. Composer markers are converted to chips through the existing marker parser before expansion. Runtime/provider paths ignore `display_text`; transcript paths prefer it.

**Tech Stack:** Rust 2024, existing `neo-tui::paste::Marker`, `AgentMessage`, `AgentEvent::MessageAppended`, JSONL session persistence, interactive TUI transcript replay.

**Baseline/Authority Refs:** `docs/aegis/specs/2026-07-08-at-file-reference-composer-design.md`, `crates/neo-tui/src/paste.rs`, `crates/neo-agent/src/prompt/parts.rs`, `crates/neo-agent-core/src/messages.rs`.

**Compatibility Boundary:** Existing JSONL without `display_text` must deserialize unchanged and fall back to the current content display. Expanded content remains provider-visible and durable. No historical session migration or heuristic parsing of expanded `<file>` blocks.

**TDD Route:**
- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: the user did not request strict TDD; focused regressions are sufficient for this bounded contract repair.
- Verification: exact marker, message, interactive-submit, replay, queue, and steer tests listed below.

## Scope And Governance

**Requirement Ready Check:** The approved design requires compact `@[display]` transcript text, expanded provider context, durable replay, queue/steer parity, and no new attachment owner. Decision: ready.

**Change Necessity:** A presentation-only TUI patch cannot survive session replay because `MessageAppended` currently persists only expanded content. The minimum stable change is an optional presentation field on the existing user message plus submission/replay plumbing. Decision: code-change.

**Architecture Integrity:** `AgentMessage::User` is the canonical durable owner; `content` and `display_text` are two projections of that record. No separate event, card, attachment model, or `PromptSubmission` is introduced. Verdict: edit existing owners.

**Anti-Entropy Declaration:**
- Deletion Class: contract-carrying code adjustment; no live-state deletion
- Old Path/Object: `content_to_display_text(expanded_content)` as the sole user transcript source
- New Canonical Owner: optional `AgentMessage::User::display_text`
- Expected Preserved Behavior: expanded snapshots remain in model context and JSONL
- Expected Retired Behavior: new `@` submissions rendering expanded file contents as user prose
- External Boundary Touched: yes, backward-readable JSONL event schema
- Source-of-Truth Data Risk: none
- User Confirmation Required: no

**Retirement Decision:** `delete-first` for the new-submission display path. Retain only serde-default fallback for existing persisted sessions; do not keep a second live display owner.

## Files

- Modify `docs/aegis/specs/2026-07-08-at-file-reference-composer-design.md`: pin the transcript projection contract.
- Modify `docs/aegis/INDEX.md`: index this plan.
- Modify `crates/neo-tui/src/paste.rs`: derive display text by replacing parsed markers with `Marker::as_chip()`.
- Modify `crates/neo-agent-core/src/messages.rs`: persist optional user `display_text`, expose constructors/accessors, and keep provider conversion content-only.
- Modify `crates/neo-agent-core/src/runtime/image_blobs.rs`: preserve `display_text` while rewriting image content.
- Modify `crates/neo-agent/src/modes/interactive/{mod.rs,turn.rs,controller_factory.rs,shell_command.rs}`: carry the compact display projection through direct submit and queued shell handoff.
- Modify `crates/neo-agent/src/modes/run/mod.rs`: construct the durable user message with both projections.
- Modify `crates/neo-tui/src/transcript/pane.rs`: prefer persisted presentation text during replay.
- Modify `crates/neo-agent/src/modes/sessions.rs`: prefer presentation text in human-readable transcript output.
- Modify focused tests beside these owners.

## Task 1: Add Canonical Marker And Message Projections

**Why:** The presentation text must be derived once and persisted with the message that owns the expanded context.

**Repair Track:** Add `markers_as_chips(text)` beside `parse_markers`; add optional serde-defaulted `display_text` to `AgentMessage::User`; add a constructor and accessor; keep `to_chat_message()` content-only.

**Retirement Track:** Do not add another marker parser or presentation event. Existing user messages without the field remain readable through `None`.

**Steps:**

1. Implement marker-to-chip projection using `parse_markers()` and `Marker::as_chip()`.
2. Add `display_text: Option<Arc<str>>` with serde default/skip rules to `AgentMessage::User` and update direct constructors/rebuilders.
3. Add focused tests proving marker projection, old JSON compatibility, new JSON round-trip, and provider conversion isolation.

**Verification:**

```bash
cargo test --package neo-tui --lib paste::tests::markers_as_chips_preserves_text_and_compacts_file_references -- --exact --nocapture
cargo test --package neo-agent-core --lib messages::tests::user_display_text_roundtrips_without_changing_provider_content -- --exact --nocapture
```

## Task 2: Carry Presentation Text Through Interactive Submission

**Why:** Normal submit, queued follow-up, steer, and shell-to-agent handoff must all create the same durable user-message shape.

**Repair Track:** Derive compact display text after prompt-template resolution and before marker expansion. Pass it through the existing `TurnRequest` and run preparation path. Use the same projection for optimistic queue/steer rows and duplicate suppression.

**Retirement Track:** Stop deriving new-submission transcript text from expanded content. Keep expanded text only for prompt history until prompt-history attachment recall is designed separately.

**Steps:**

1. Add optional prompt display text to `TurnRequest` and a display-aware turn starter while retaining the existing starter for non-composer callers.
2. Update direct submit and shell-drained submit to use marker chip projection.
3. Build queue and steer `AgentMessage` values with the same display projection.
4. Update runtime preparation signatures to persist the optional display text.
5. Update focused interactive tests for direct submit, queue, and steer behavior.

**Verification:**

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_file_reference_marker_keeps_chip_in_user_transcript --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::queued_file_reference_keeps_chip_when_appended --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::steered_file_reference_keeps_chip_when_appended --exact --nocapture --include-ignored
```

## Task 3: Replay And Human Transcript Projection

**Why:** A live-only fix would regress after `/resume` and in human-readable transcript output.

**Repair Track:** Make TUI replay and `neo sessions transcript` prefer `display_text`, falling back to current content rendering only when the optional field is absent.

**Retirement Track:** Do not parse `<file>` blocks to reconstruct old displays and do not change raw JSON/event export.

**Steps:**

1. Update `TranscriptPane::replay_message` to pair persisted display text with existing image attachments.
2. Update human-readable session formatting to use the same accessor.
3. Add exact replay and old-event fallback tests.

**Verification:**

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::replay_session_into_transcript_prefers_user_display_text --exact --nocapture --include-ignored
cargo test --package neo-agent-core --test session_jsonl user_display_text_roundtrips_and_old_user_event_remains_readable -- --exact --nocapture
```

## Final Verification

```bash
rustfmt --check --edition 2024 <touched Rust files>
git diff --check -- <touched files>
```

Commit only the scoped files with:

```bash
git add <touched files>
git commit -m "fix(tui): keep file references compact in transcript"
```

## Risks And Stop Conditions

- Stop if preserving presentation text would change provider request bytes or compaction input.
- Stop if an existing session schema cannot deserialize without migration; do not rewrite session JSONL.
- Keep prompt history semantics unchanged; attachment-aware history recall is a separate design.
- No separate transcript attachment card, file-content expansion toggle, or new marker owner.
