# Assistant Streaming Native-History Presentation Design

## Status

Approved in conversation on 2026-08-04. This design refines the existing native-scrollback transcript presentation. It does not reopen the normal-screen, explicit-review, or dynamic card decisions recorded in ADR-0010.

## Problem

During a streaming assistant response, `TranscriptPresentation` renders the whole unresolved assistant suffix as a bounded live block whenever the Markdown parser cannot prove a stable prefix. Once that block is taller than the rows above the bottom chrome, `bound_live_blocks` keeps only the bounded live projection. The terminal therefore appears to scroll the response upward while the model is still producing it. The complete response becomes visible only after finalization moves the remaining content into history.

This is a presentation defect. The canonical assistant content, event order, `InlineTerminal` geometry, and dynamic Delegate-family card state are not defective.

## Goals

- Progressively append assistant content to native terminal history as soon as a typed Markdown stability proof exists.
- Keep only the still-ambiguous assistant suffix in the bounded live area.
- Preserve continuous visual identity across the history/live boundary: one assistant start marker (`●`), no artificial separator, and continuation indentation for the live tail.
- Preserve the existing `TranscriptPresentation` owner, `FinalizedBlock` acknowledgement protocol, `assistant_offsets` ledger, and `AssistantSource` proof.
- Keep the final response complete and non-duplicated.
- Keep Todo, composer, footer, approvals, questions, and all Delegate-family cards within their current owners and height budget.
- Keep ordinary conversation on the normal terminal screen with native scrollback.

## Non-goals

- Redesigning `Delegate`, `DelegateGroup`, `DelegateSwarm`, workflow, tool, shell, approval, or question card content, layout, expansion, ordering, or activity semantics.
- Adding a second transcript store, renderer, viewport, scroll owner, compatibility renderer, feature flag, or persistence format.
- Committing assistant content before a parser-level stability proof.
- Treating `MessagePhase::FinalAnswer` as proof that the current source can no longer change.
- Rewriting canonical session events, historical conversation, context cache prefixes, or provider streams.
- Removing the existing bounded-live fallback for a genuinely unresolved Markdown construct.

## Existing owners and evidence

- `TranscriptStore` owns canonical typed entries and revisions.
- `TranscriptPresentation` owns history/live projection and acknowledgement.
- `TranscriptPane::render_terminal_update` passes the effective live budget.
- `NeoTui::render_terminal_frame_at` composes history, live rows, and chrome.
- `InlineTerminal` owns physical normal-screen writes and protected history insertion.
- `LiveRenderer` validates the already-composed live frame; it must not become a Markdown or transcript owner.

Current source evidence:

- `crates/neo-tui/src/transcript/presentation.rs`: `render_assistant_entry`, `PresentationFrame::finish`, and `bound_live_blocks`.
- `crates/neo-tui/src/transcript/streaming_prefix.rs`: `stable_prefix_len` and Markdown offset parsing.
- `crates/neo-tui/src/transcript/pane.rs`: `render_terminal_update` and history acknowledgement.
- `crates/neo-tui/src/app.rs`: normal terminal frame composition and `acknowledge_history`.
- `crates/neo-agent/src/modes/interactive/terminal_io.rs`: every drawn terminal frame is rendered and acknowledged after a successful write.

## Selected design

### 1. Keep one assistant entry and one presentation ledger

The source remains one `TranscriptEntry::AssistantMessage`. Each progressively committed portion is represented as an `AssistantSegment` block with:

```text
(entry, source_start, source_end)
```

The range must start exactly at the acknowledged `assistant_offsets[entry]` value. Acknowledgement advances that offset only after the physical frame write succeeds. The next live tail starts at the same contiguous `source_end`.

No new transcript entry is created for a streamed chunk.

### 2. Extend only parser-proven stable boundaries

`stable_prefix_len` remains the sole parser boundary helper. It may be refined to identify smaller append-only Markdown units where the parser can prove that appending later source cannot change the already committed rendered prefix.

Allowed boundaries are parser-derived offsets, not line equality, ANSI comparison, rendered text comparison, or guessed delimiter matching. The implementation must preserve the existing conservative handling of reference definitions, footnotes, open code blocks, and other constructs whose later source can reflow earlier rendering.

For a stream that produces successive safe boundaries, each new stable range is emitted in the next terminal frame. If the current construct has no safe boundary, it remains bounded live until a later delta closes it or the message finalizes. This is an intentional correctness fallback, not a new omission mechanism.

### 3. Preserve one visual message across history and live

The first assistant segment renders with the existing message-start prefix:

```text
● ...
```

Every continuation segment, including the live tail after a committed history prefix, renders with the existing continuation mode and indentation:

```text
  ...
```

The live tail must use the same `TranscriptEntryId` owner as the history segment. `separator_before` must remain false between adjacent segments of the same assistant entry. The resulting frame is visually continuous:

```text
history:
● first committed paragraph
  second committed paragraph
live:
  paragraph still being generated
```

There must never be a second `●` solely because the source crossed from native history into the mutable live region.

### 4. Keep card and bottom-region behavior unchanged

The change is limited to ordinary assistant streaming projection. `bound_live_blocks` continues to bound genuinely mutable cards and tails. Delegate-family card rendering, card-local activity limits, expansion behavior, dynamic projection ordering, chrome budgeting, and `LiveRenderer` validation remain unchanged.

No new `earlier rows omitted` text or assistant-specific compaction path is introduced. The normal frame must remain height-valid before `LiveRenderer` receives it.

### 5. Finalization converges without replay

When the assistant entry is finalized, the remaining source from the acknowledged offset through `content.len()` becomes one or more continuation segments as appropriate. Previously acknowledged source is never replayed. The final native history therefore contains the complete assistant message exactly once, with the original first marker and no duplicate final copy.

A failed terminal write leaves the acknowledgement ledger unchanged and allows the same pending segment to retry through the existing transaction path.

## Data flow

```text
assistant delta
  -> TranscriptStore assistant content
  -> TranscriptPresentation::render_assistant_entry
  -> stable_prefix_len(content)
  -> pending AssistantSegment history + unresolved live tail
  -> NeoTui::render_terminal_frame_at
  -> InlineTerminal protected history insertion + LiveRenderer live redraw
  -> successful write
  -> TranscriptPresentation::acknowledge assistant source range
```

The canonical content remains append-only. The presentation creates derived history/live blocks but never mutates or replaces the source entry.

## Invariants

1. `source_start` equals the presentation ledger offset for the entry.
2. Each acknowledged segment is contiguous with the prior acknowledged segment.
3. An acknowledged source range never changes on a later delta.
4. A live assistant suffix begins at or after the acknowledged offset and is never acknowledged until it has a stability proof or finalization proof.
5. History and live segments for one entry share the same owner and do not add a new message marker or separator.
6. The stripped concatenation of acknowledged segments plus the live suffix converges to the same canonical assistant source rendering after finalization.
7. A terminal write failure does not advance offsets or progressive acknowledgement.
8. Dynamic Delegate-family card output is byte/row compatible with the existing renderer for the same snapshot.
9. The live frame remains within the measured bottom-region budget.
10. No context cache prefix or canonical conversation record is changed.

## Options considered

### Parser-proven progressive assistant segments (selected)

Uses the existing assistant source proof and acknowledgement ledger, and fixes the observed behavior at the existing presentation owner. It preserves native scrollback and avoids new state. The trade-off is that an open Markdown construct can remain bounded live until it becomes safe.

### Commit every received line or delta

Rejected. Later Markdown syntax, reference definitions, list continuation, or code fences can change earlier rendering. Native history cannot be rewritten, so this would create stale visible history.

### Add an assistant-specific application viewport

Rejected. It introduces a second scroll owner and another presentation surface, conflicts with the native normal-screen decision, and still does not make the assistant content part of native scrollback during generation.

## Scope

Expected implementation files:

- `crates/neo-tui/src/transcript/streaming_prefix.rs`
- `crates/neo-tui/src/transcript/presentation.rs` only if the boundary or continuation handling requires a focused adjustment
- focused transcript tests under `crates/neo-tui/src/transcript/presentation.rs` and/or `crates/neo-tui/tests/transcript_pane.rs`
- an end-to-end terminal-frame test only if the focused projection tests cannot prove history/live continuity

Do not edit Delegate-family component files, `InlineTerminal`, `LiveRenderer`, agent runtime, provider code, or persistence code unless a focused failing proof identifies a contract violation outside this scope.

## Acceptance criteria

- A multi-frame assistant stream with several parser-proven stable boundaries grows `TerminalFrame.history` before finalization.
- The live frame contains only the uncommitted assistant suffix plus any independently live card content.
- The combined rendered assistant output contains exactly one renderer-generated message-start prefix (`●`) and no artificial history/live separator; user-authored `●` glyphs are not part of this assertion.
- A later Markdown delta cannot change an acknowledged source prefix.
- An open reference-based link, footnote, code fence, or equivalent unresolved construct remains live until safe; it is never prematurely committed.
- Finalization produces complete assistant history exactly once and does not replay an acknowledged prefix.
- A failed terminal write retries the same assistant source range without advancing the ledger.
- Existing Delegate/DelegateSwarm card tests and normal-screen height tests remain unchanged and pass.
- The final frame remains height-valid and preserves the existing bottom chrome contract.

## Verification plan

Use focused tests only:

- assistant progressive segment projection and marker continuity;
- stable-prefix non-rewind behavior with later reference definitions;
- unresolved Markdown fallback remains live;
- finalization no-duplicate convergence;
- terminal-frame history/live height and chrome preservation if an integration regression is needed.

Run formatting and the exact affected test targets after the implementation tasks. Do not use a broad workspace test as evidence.

## Compatibility and ADR signal

This is a refinement of the existing `TranscriptPresentation` contract, not a new architecture. It should amend ADR-0010 or its evidence/baseline only after implementation and focused verification prove the new assistant streaming behavior. The amendment must explicitly retain all existing normal-screen, dynamic-card, approval, question, context-integrity, and native-scrollback boundaries.

## Working artifacts

### TaskIntentDraft

- Outcome: stable streaming assistant prefixes enter native history while the unresolved suffix remains live.
- Goal: remove the apparent upward-scroll bug without changing dynamic cards.
- Success evidence: monotonic ranges, one marker, no duplicate final, focused terminal/projection tests.
- Stop condition: scoped implementation and review pass without new owner or compatibility path.
- Non-goals: cards, runtime, persistence, provider streams, context records.

### BaselineUsageDraft

- Required refs: `AGENTS.md`, ADR-0010, native-scrollback design/baseline, workflow dynamic transcript design, current presentation and terminal source.
- Delivered refs: current codegraph source, focused tests, current draw/acknowledgement path.
- Acknowledged before plan refs: the same authority files and the approved 2026-08-04 conversation decision.
- Cited refs: current owners and source paths above.
- Missing refs: `docs/current/AEGIS_MINIMALITY_REFERENCE.md` is not present; no decision depends on it because no new surface is proposed.
- Decision: continue.

### ImpactStatementDraft

- Affected layers: assistant Markdown stable-boundary projection and focused TUI tests.
- Canonical owner: `TranscriptPresentation`.
- Invariants: append-only acknowledged source, one message marker, contiguous offsets, bounded unresolved tail, unchanged cards/chrome.
- Compatibility: existing JSONL and terminal ownership remain unchanged.
