# Thinking And Message Presentation Implementation Plan

Date: `2026-08-03`

Parent design:
`docs/aegis/specs/2026-08-03-thinking-and-message-presentation-design.md`

Status: `ready for implementation; source changes pending`

## Aegis Visibility

This plan fixes a cross-crate semantic boundary before implementation: provider
reasoning kind, ordinary assistant message phase, and transcript lifecycle must
remain distinct. The plan prevents a renderer-only patch from losing summary
body text or making commentary look like a final answer.

## Plan Basis

The approved design is the parent specification above. Existing repository
facts are:

- `ThinkingKind` is already present and must be reused.
- OpenAI Responses maps native summary events to `Summary`.
- Anthropic and Google map provider thinking blocks to `Full`.
- OpenAI-compatible reasoning is `Unknown` when its semantic shape is not
  established.
- Current summary rendering has an early title-only path that drops body
  text.
- Current normalized text events do not carry an ordinary-message phase.
- The worktree contains unrelated user changes; they are outside this plan.

## Requirement Ready Check

- Requirement source: the approved design discussion and the parent design
  specification.
- Goal: preserve summary body, support provider-returned full thinking, and
  distinguish commentary from final answer.
- Acceptance: parent design criteria 1-11.
- Open blocker: exact provider phase field availability must be verified in the
  existing OpenAI Responses parser during Task 1. If unavailable, emit
  `MessagePhase::Unknown`; do not invent a phase.
- Decision: `ready`.

## Scope Fence

In scope:

- normalized message phase metadata;
- existing provider event mapping;
- thinking part retention;
- summary title/body projection;
- commentary and final-answer transcript presentation;
- serialization, replay, focused tests, formatting, and retirement scans.

Out of scope:

- new thinking kinds;
- provider runtime rewrites;
- OpenAI hidden-reasoning reconstruction;
- context/session history rewriting;
- Delegate, DelegateGroup, DelegateSwarm, or Workflow cards;
- new display configuration for each provider;
- changes to the final Markdown renderer.

## Change Necessity

- User-visible need: summary body is currently lost, and process text cannot be
  distinguished from final answer text.
- No-change option: a renderer-only title parser would recover body text for a
  narrow case but cannot preserve provider part boundaries or message phase.
- Why code change is necessary: the phase and part metadata must cross provider,
  runtime, persistence, and TUI boundaries.
- Minimum boundary: existing provider event enums, `ModelTurnState`,
  `AgentEvent`, `TranscriptStore`, and existing transcript entry renderers.
- Decision: `code-change`.

## Existence And Ownership Check

- New surface considered: a separate reasoning renderer or second message
  projection owner.
- Existing owner: `TranscriptEntry` rendering and `TranscriptStore` state.
- Why a new owner is unnecessary: the current TUI already owns thinking
  blocks, expansion, animation, and assistant message rendering.
- Decision: `reuse-existing`.
- New semantic carrier: `MessagePhase` is justified because existing events
  cannot preserve commentary/final meaning. It must be a small provider-neutral
  enum, not a second message model.

## Architecture Integrity Lens

- Invariant: provider meaning is normalized once; TUI derives only display
  values; canonical records remain append-only.
- Canonical owners: provider adapters for wire meaning, `ModelTurnState` for
  active stream state, `AgentEvent` for runtime transport, `TranscriptStore`
  for transcript state, and `TranscriptEntry` for rendering.
- Responsibility overlap to avoid: do not infer message phase in the TUI and
  do not extract summary titles in provider adapters.
- Higher-level simplification: keep one `ThinkingKind`, add one independent
  `MessagePhase`, and retain part boundaries in the existing thinking block.
- Retirement: remove the title-only summary projection after body-preserving
  tests pass.
- Verdict: proceed with existing owners.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: not applicable; strict test-first work was not requested.
- Test posture: focused post-change regressions plus provider mapping tests.
- Reason: this is an approved cross-module design, but the user did not request
  strict red/green test-first execution.
- Verification: exact package, target, and filter commands listed below.

## File Map

### Core semantic and provider files

- `crates/neo-ai/src/types.rs`: define the smallest `MessagePhase` enum and
  carry it on message lifecycle events; keep `ThinkingKind` unchanged.
- `crates/neo-ai/src/providers/openai/responses.rs`: preserve native message
  phase when the response item provides it; preserve summary part identity and
  emit `Unknown` when the wire does not provide a phase.
- `crates/neo-ai/src/providers/openai/compatible.rs`: keep reasoning kind
  `Unknown`; do not infer message phase from model name or tool order.
- `crates/neo-ai/src/providers/anthropic.rs` and
  `crates/neo-ai/src/providers/google.rs`: keep `Full` thinking mapping and
  emit `Unknown` message phase unless the provider already exposes one.
- `crates/neo-ai/tests/openai_compatible_provider.rs` and the narrow provider
  test owner for Responses events: add mapping regressions without live network
  assumptions.

### Runtime and persistence files

- `crates/neo-agent-core/src/events.rs`: carry message phase with message
  lifecycle events, using serde defaults for historical events.
- `crates/neo-agent-core/src/runtime/stream_aggregator.rs`: forward phase and
  preserve active message/part ordering without changing tool execution.
- `crates/neo-agent-core/src/messages.rs`: retain phase and thinking part
  metadata in normalized assistant content where the existing session model
  requires it.
- `crates/neo-agent-core/src/session/event_persistence.rs`: update only the
  event serialization/replay cases needed for new fields and defaults.

### Transcript files

- `crates/neo-tui/src/transcript/store.rs`: retain thinking part boundaries and
  active message phase; do not merge distinct summary parts solely by
  `ThinkingKind`.
- `crates/neo-tui/src/transcript/entry/mod.rs`: add phase/part state to existing
  entry variants only where needed; do not add a second transcript model.
- `crates/neo-tui/src/transcript/entry/render_thinking.rs`: replace the
  title-only early return with a narrow leading-heading parser that preserves
  the body and inline Markdown.
- `crates/neo-tui/src/transcript/event_handler.rs`: route thinking,
  commentary, and final-answer lifecycle events to their existing owners.
- `crates/neo-tui/tests/thinking_blocks.rs` and the focused transcript tests:
  verify the complete display contract and legacy unknown-phase behavior.

## Implementation Tasks

### Task 1: Normalize message phase and provider evidence

Files: `crates/neo-ai/src/types.rs`, the existing OpenAI Responses parser,
`crates/neo-agent-core/src/events.rs`,
`crates/neo-agent-core/src/runtime/stream_aggregator.rs`.

Steps:

1. Add `MessagePhase::{Commentary, FinalAnswer, Unknown}` with a serde default
   of `Unknown`.
2. Carry the phase on message start/completion events using the existing event
   path; do not create a parallel message event stream.
3. Trace the OpenAI Responses output-item phase field and map only explicit
   `commentary` and `final_answer` values.
4. Emit `Unknown` when an endpoint omits the phase or uses a shape Neo cannot
   establish.
5. Keep `ThinkingKind` mapping exactly as it is today.
6. Add a focused provider/runtime regression for explicit commentary and final
   phases and for missing-phase compatibility.

Expected result: phase meaning is available to the transcript without any TUI
text heuristic, while existing providers continue to work as `Unknown`.

### Task 2: Preserve thinking part boundaries

Files: `crates/neo-ai/src/types.rs`,
`crates/neo-agent-core/src/messages.rs`,
`crates/neo-tui/src/transcript/store.rs`,
`crates/neo-tui/src/transcript/entry/mod.rs`.

Steps:

1. Keep the existing `ThinkingKind` and lifecycle events.
2. Preserve each thinking start/end part identity where the provider supplies
   one; use the current event id rather than adding a second identity system.
3. Store summary parts inside the existing thinking block or equivalent
   existing entry state, without flattening away boundaries.
4. Ensure adjacent parts with the same `ThinkingKind` are not merged in a way
   that loses their body or identity.
5. Keep raw content append-only and derive title/body only while rendering.
6. Add a focused store regression proving that two summary parts remain two
   parts while still rendering as one compact visible block.

Expected result: repeated titles can be collapsed in the projection without
losing distinct bodies or changing session records.

### Task 3: Replace title-only summary rendering

File: `crates/neo-tui/src/transcript/entry/render_thinking.rs`.

Steps:

1. Replace the current `summary_titles`-only path with a parser for one
   leading `**title**` and the remaining body.
2. Preserve body Markdown, bullets, links, and inline bold text.
3. Recognize `<!-- -->` only as an empty presentation placeholder when it is
   the body of a summary part.
4. Use the latest non-empty title from the active part for the streaming row.
5. Deduplicate identical adjacent titles only in rendered output.
6. Keep the existing spinner, bounded preview, expansion key, width handling,
   and theme style.
7. Do not use a regular-expression list or infer a title for `Full` or
   `Unknown` thinking.

Expected result: the second example renders both its title and explanatory
   bullets; summary-only examples stay compact.

### Task 4: Separate commentary from final answer presentation

Files: `crates/neo-tui/src/transcript/entry/mod.rs`,
`crates/neo-tui/src/transcript/store.rs`,
`crates/neo-tui/src/transcript/event_handler.rs`, and existing assistant
message rendering helpers.

Steps:

1. Carry `MessagePhase` into the active assistant message state.
2. Render `Commentary` in the dynamic working area while streaming and commit
   it as a muted, separately identifiable transcript block after completion.
3. Keep `FinalAnswer` on the existing normal Markdown path.
4. Never merge a commentary block into a later final answer.
5. Preserve the current behavior for `Unknown` message phase.
6. Keep all existing tool, Delegate-family, Workflow, approval, and question
   presentation paths unchanged.

Expected result: a commentary message followed by a final answer remains two
  ordered records with distinct visual emphasis.

### Task 5: Persistence and replay compatibility

Files: `crates/neo-agent-core/src/messages.rs`,
`crates/neo-agent-core/src/session/event_persistence.rs`, existing replay
tests, and the TUI transcript replay owner.

Steps:

1. Add serde defaults for message phase and any new part metadata.
2. Read historical events without phase as `Unknown`.
3. Preserve part order and raw text on replay.
4. Apply the same summary parser and commentary/final presentation to replay
   as to live events.
5. Confirm no existing context prefix, historical message, or session event is
   rewritten.

Expected result: old sessions remain readable and new display semantics do not
require a migration that changes canonical history.

### Task 6: Focused regression suite

Add or update only tests that prove the new behavior:

- `crates/neo-tui/tests/thinking_blocks.rs`
  - `summary_thinking_preserves_body_after_title`
  - `summary_thinking_keeps_inline_bold_body`
  - `summary_thinking_collapses_adjacent_duplicate_titles`
  - `full_thinking_renders_bounded_preview`
  - `unknown_thinking_does_not_extract_title`
- focused transcript tests
  - `commentary_and_final_answer_render_as_separate_entries`
  - `unknown_message_phase_preserves_legacy_rendering`
- focused `neo-ai` provider tests
  - explicit OpenAI message phases map correctly;
  - missing provider phase maps to `Unknown`;
  - existing Summary, Full, and Unknown thinking mappings remain unchanged.
- focused `neo-agent-core` event/persistence tests
  - message phase and thinking defaults survive serialization;
  - event order remains unchanged.

Use exact commands after implementation, for example:

```bash
cargo nextest run -p neo-tui --test thinking_blocks summary_thinking_preserves_body_after_title
cargo nextest run -p neo-tui --test thinking_blocks commentary_and_final_answer_render_as_separate_entries
cargo nextest run -p neo-agent-core --test runtime_turn message_phase
cargo nextest run -p neo-ai --test openai_compatible_provider thinking_kind
```

The implementer must adjust the target selector only if the existing test
owner differs; every command must name one package, one target, and one test
filter.

### Task 7: Retirement and verification

Steps:

1. Search touched owners for the old summary title-only early return and remove
   it after the new body-preserving path is active.
2. Search for any TUI inference of phase from model name, tool order, or text
   formatting and remove it.
3. Confirm no second reasoning/message transcript owner was added.
4. Run focused tests from Task 6.
5. Run formatting on touched Rust files and `git diff --check`.
6. Review the final diff against every compatibility and non-goal item in the
   parent design.

## Verification Matrix

| Boundary | Evidence |
| --- | --- |
| Provider kind mapping | exact `neo-ai` provider test filters |
| Message phase mapping | explicit, missing, and legacy event tests |
| Summary body | `neo-tui` thinking-body test |
| Duplicate titles | rendered projection test plus raw-part retention assertion |
| Full thinking | bounded preview and expansion test |
| Commentary/final | ordered transcript presentation test |
| Replay | exact persistence/replay test filter |
| Compatibility | old serialized event test and existing focused regressions |
| Scope hygiene | `git diff --check`, touched-file formatting, no card diffs |

Focused local evidence proves the named macOS worktree paths only. It does not
prove every live provider, remote CI, or native Windows/Linux terminal.

## Risks And Responses

- Provider phase fields may not exist on every endpoint. Emit `Unknown`; do
  not guess.
- Flattening thinking parts can reintroduce lost body or duplicate-title bugs.
  Preserve part boundaries before changing rendering.
- Commentary may be emitted without a completion phase. Preserve current
  behavior for `Unknown`.
- Full thinking may contain sensitive or provider-specific content. Keep it in
  a muted, bounded, expandable thinking block and never merge it into the final
  answer.
- Shared worktree dirt may be unrelated. Stage only implementation files when
  the later source task is committed; this documentation commit stages only
  the named `docs/aegis` files.

## Retirement Boundary

After implementation, the following old behavior must no longer be active:

1. summary rendering that returns after extracting titles and drops body text;
2. any phase inference based on text or tool ordering;
3. any merge that removes summary part boundaries;
4. any path that presents commentary as the final answer;
5. any new compatibility owner or parallel transcript model.
