# Thinking And Message Presentation Handoff

Date: `2026-08-03`

## Handoff State

This documentation slice is complete and source implementation is pending.
The next implementer must read these documents before editing Rust:

1. `docs/aegis/specs/2026-08-03-thinking-and-message-presentation-design.md`
2. `docs/aegis/plans/2026-08-03-thinking-and-message-presentation.md`
3. this handoff

The current worktree contains unrelated user changes. Preserve them. This
handoff does not authorize changes outside the named implementation boundary.

## Intent Lock

Fix the lost OpenAI reasoning-summary body and distinguish ordinary progress
commentary from final answer text, while preserving existing `ThinkingKind`,
title replacement, provider mappings, append-only session records, and all
Delegate-family card behavior.

## Scope Fence

Implementation may touch only the existing owners in these layers:

- `neo-ai` normalized types and provider adapters;
- `neo-agent-core` stream aggregation, normalized events, and persistence;
- `neo-tui` transcript store, existing entry variants, event routing, and
  thinking/assistant rendering;
- focused tests for those owners.

Do not rewrite the provider runtime, add a second transcript model, add a new
thinking enum value, or redesign tool cards.

## Baseline Lock

The following facts are already settled:

- `ThinkingKind::Summary`, `Full`, and `Unknown` already exist and work.
- OpenAI Responses native summary events map to `Summary`.
- Anthropic and Google thinking blocks map to `Full`.
- OpenAI-compatible reasoning maps to `Unknown` when its shape is not known.
- Streaming summary titles already replace one dynamic row in place.
- The current summary renderer loses body text after it finds title fragments.
- OpenAI hidden reasoning that is not returned cannot be rendered or rebuilt.
- The Codex reference separates reasoning summary, raw reasoning, commentary,
  and final-answer paths.

## Locked Data Model

Keep these concepts separate:

```text
ThinkingKind  = Summary | Full | Unknown
MessagePhase  = Commentary | FinalAnswer | Unknown
ThinkingPhase = Streaming | Complete
```

The existing thinking block must retain provider part boundaries. A derived
presentation value may split a part into:

```text
leading **title** + remaining Markdown body
```

but the raw part text remains canonical.

## Locked Provider Rules

1. Use provider-native event meaning, not model name.
2. Map explicit message phases only.
3. Emit `Unknown` when the provider does not expose a phase.
4. Do not infer a title for `Full` or `Unknown` thinking.
5. Do not turn a final answer into reasoning because it contains Markdown
   headings or bold text.
6. Do not reconstruct OpenAI hidden reasoning.

## Locked Rendering Rules

### Summary streaming

```text
⠋ 思考 · Planning final focused verification
```

Replace the row when the current summary part changes. Do not append one row
per title.

### Summary completion

Title-only:

```text
● Planning final focused verification
```

Title and body:

```text
● Reviewing verification results
  Focused regressions pass. Formatting exposed two separate issues:
  • the new condition formatting needs to match rustfmt;
  • an unrelated pre-existing formatting mismatch exists.
```

Inline bold text in the body remains visible. Adjacent duplicate titles may be
collapsed in the rendered projection only.

### Full and unknown thinking

```text
⠋ 思考中...
```

Use a muted, bounded preview and existing expansion behavior. Unknown thinking
does not receive an inferred title.

### Commentary

```text
▸ Planning final focused verification
  Running exact regressions after formatting correction
```

Commentary is retained as a lower-emphasis ordered transcript block and is
never merged into final answer text.

### Final answer

Use the existing normal Markdown assistant path. No thinking spinner,
commentary marker, or muted reasoning style.

## Required Event Ordering

The implementation must preserve an ordering such as:

```text
Summary thinking -> Commentary -> Tool -> Summary thinking -> Final answer
```

The renderer may project each event differently, but it may not drop or reorder
the canonical events.

## Exact Source Map

- `crates/neo-ai/src/types.rs`: `AiStreamEvent`, `ThinkingKind`, and the new
  provider-neutral message phase.
- `crates/neo-ai/src/providers/openai/responses.rs`: native summary and message
  phase mapping.
- `crates/neo-ai/src/providers/openai/compatible.rs`: retain unknown reasoning
  mapping; no semantic guessing.
- `crates/neo-ai/src/providers/anthropic.rs`: retain full-thinking mapping.
- `crates/neo-ai/src/providers/google.rs`: retain full-thinking mapping.
- `crates/neo-agent-core/src/events.rs`: normalized message lifecycle fields.
- `crates/neo-agent-core/src/runtime/stream_aggregator.rs`: active message and
  thinking-part forwarding.
- `crates/neo-agent-core/src/messages.rs`: normalized content/persistence
  metadata where required.
- `crates/neo-agent-core/src/session/event_persistence.rs`: serde defaults and
  replay compatibility.
- `crates/neo-tui/src/transcript/store.rs`: part retention and active phase.
- `crates/neo-tui/src/transcript/entry/mod.rs`: existing transcript state and
  entry variants.
- `crates/neo-tui/src/transcript/entry/render_thinking.rs`: summary parser and
  rendering; remove the title-only body loss.
- `crates/neo-tui/src/transcript/event_handler.rs`: phase-aware routing.
- `crates/neo-tui/tests/thinking_blocks.rs`: focused thinking presentation
  regressions.

## Implementation Order

1. Normalize `MessagePhase` and verify the OpenAI Responses wire field.
2. Preserve thinking part boundaries without changing `ThinkingKind`.
3. Replace title-only summary rendering with title-plus-body rendering.
4. Keep commentary and final answer as separate ordered transcript entries.
5. Add serde defaults and replay coverage.
6. Add exact focused tests, formatting, and retirement scans.

Do not start with a broad provider refactor or a renderer-only workaround.

## Required Verification Evidence

Before claiming implementation complete, provide exact local evidence for:

- OpenAI summary title and body both render;
- inline bold body text remains visible;
- adjacent duplicate titles collapse only in the projection;
- full thinking stays bounded and expandable;
- unknown thinking does not get a guessed title;
- commentary and final answer remain separate;
- missing message phase preserves legacy behavior;
- historical events without new fields replay successfully;
- existing provider thinking mappings remain unchanged;
- no Delegate-family or Workflow card diff occurred;
- touched-file formatting and `git diff --check` pass.

Use one package, one target, and one test-name filter for each focused test
command. A local macOS result must not be described as live-provider,
cross-platform, or remote-CI proof.

## Drift Rules

Stop and return to the design if implementation proposes any of the following:

- a new `ThinkingKind` value for commentary or final answer;
- title inference from model name, timing, tool order, or arbitrary Markdown;
- a second transcript or provider-normalization owner;
- flattening summary parts back into one string before rendering;
- rewriting historical events or context prefixes;
- showing OpenAI hidden reasoning as if it were returned;
- changing Delegate, DelegateGroup, DelegateSwarm, or Workflow cards;
- adding a repair request, retry, or new provider call for display purposes.

## Stop Condition

This handoff is satisfied only when the implementation follows the parent
specification, focused evidence covers every acceptance boundary, and the final
diff contains no unrelated source or card changes. Until then, the task state
is implementation-pending rather than complete.
