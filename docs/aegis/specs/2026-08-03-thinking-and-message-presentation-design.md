# Thinking And Message Presentation Design

Date: `2026-08-03`

Status: `approved for implementation planning; source implementation pending`

This document is the complete design baseline for the next implementation
slice. It does not change the already implemented `ThinkingKind` behavior. It
defines the missing summary-body projection and the separate message-phase
projection so a later implementer does not merge unrelated content types.

## Decision In One Sentence

Neo keeps `ThinkingKind` for provider reasoning content, adds an independent
`MessagePhase` for ordinary assistant messages, preserves summary parts as
title-plus-body records, and renders thinking, process commentary, and final
answers through three separate presentation paths.

## Problem

Neo already distinguishes provider reasoning as `Summary`, `Full`, or
`Unknown`, and replaces the active summary title in place while streaming.
However, the current summary renderer finds bold fragments and returns after
rendering those titles. A summary such as:

```text
**Reviewing verification results**

Focused regressions pass. Formatting exposed two separate issues:
• the new condition formatting needs to match rustfmt;
• an unrelated pre-existing formatting mismatch exists.
```

is reduced to the title and loses the useful explanation and bullet list.

There is a second independent gap. Ordinary assistant text can be emitted as
mid-turn progress commentary or as a final answer, but Neo's normalized text
events currently do not carry that distinction. If both kinds are handled as
one assistant stream, a process update can be mistaken for the final answer or
be merged into it.

## Baseline Facts

- `ThinkingKind` already exists in `neo-ai` and is propagated through the
  model-turn aggregator into `AgentEvent::ThinkingStarted`.
- The OpenAI Responses adapter emits `ThinkingKind::Summary` for native
  reasoning-summary events.
- Anthropic and Google adapters emit `ThinkingKind::Full` for provider
  thinking blocks.
- The OpenAI-compatible adapter emits `ThinkingKind::Unknown` when it cannot
  establish the provider's reasoning representation.
- `neo-tui` has a dedicated thinking block, spinner, title extraction, and
  expansion behavior. These remain the existing presentation owner.
- The Codex reference separates reasoning summary text from raw reasoning
  text, extracts a status header from a leading bold fragment, preserves
  summary parts, and distinguishes `Commentary` from `FinalAnswer` messages.
- OpenAI hidden reasoning that is not returned by the API cannot be rendered by
  Neo and must never be reconstructed from a title or from the final answer.

## Terminology

### Thinking kind

`ThinkingKind` describes what the provider marked as reasoning content:

```text
Summary  provider-approved reasoning summary
Full     provider-returned full reasoning block
Unknown  reasoning block whose representation is not known
```

No additional thinking enum values are needed for this design.

### Message phase

`MessagePhase` describes ordinary assistant output:

```text
Commentary  mid-turn progress or explanation before more work
FinalAnswer terminal user-facing answer for the turn
Unknown     provider or historical event did not provide a phase
```

`MessagePhase` is independent from `ThinkingKind`. A model can emit a
reasoning summary followed by commentary, then a tool call, then another
reasoning summary and a final answer.

### Thinking part

The canonical thinking block must retain provider part boundaries instead of
only concatenating one flat string:

```text
ThinkingBlock
├── kind: Summary | Full | Unknown
├── phase: Streaming | Complete
└── parts: [ThinkingPart]

ThinkingPart
├── id or provider part identity when available
└── raw text
```

The title and body are derived presentation values. They are not a replacement
for raw part text and must not be persisted as the only source.

## Provider Adaptation

Provider adapters must use provider-native event meaning. They must not use a
model name, a regular-expression guess, or the presence of Markdown bold text
to decide the semantic kind.

| Provider signal | Normalized kind or phase | Presentation rule |
| --- | --- | --- |
| OpenAI Responses reasoning summary event | `ThinkingKind::Summary` | Show a dynamic summary title; show the returned summary body after completion |
| OpenAI hidden or omitted reasoning content | no event | Show nothing that claims to be raw reasoning |
| Anthropic thinking block | `ThinkingKind::Full` | Show a muted, bounded thinking preview with expansion |
| Google thought content | `ThinkingKind::Full` | Show a muted, bounded thinking preview with expansion |
| Compatible endpoint reasoning event with no known shape | `ThinkingKind::Unknown` | Show generic reasoning status and bounded content; do not extract a title |
| Provider message phase `commentary` | `MessagePhase::Commentary` | Dynamic progress display, then muted transcript entry |
| Provider message phase `final_answer` | `MessagePhase::FinalAnswer` | Normal assistant Markdown display |
| Missing message phase | `MessagePhase::Unknown` | Preserve current legacy assistant behavior |

If an adapter cannot prove a phase, it emits `Unknown`. It must not infer
`FinalAnswer` merely because text is the last text seen so far, and it must not
infer `Commentary` merely because tools may follow.

## Normalized Event Shape

The implementation should extend existing normalized events with the smallest
additional information needed to preserve message phase and part boundaries.

The intended shape is conceptually:

```text
MessageStarted { id, phase }
TextDelta      { text }
MessageFinished { id, phase, stop_reason }

ThinkingStarted { id, kind }
ThinkingDelta  { text }
ThinkingFinished { signature, redacted }
```

Text deltas continue to use the currently active message. A provider that
cannot provide a phase emits `Unknown` and follows the legacy path.

Thinking start/end boundaries identify parts. The transcript store must not
merge distinct summary parts solely because their `ThinkingKind` values match.
It may group them into one visible thinking block while retaining the part
list.

## Summary Parsing

Summary parsing is a narrow Markdown presentation parser, not semantic model
interpretation.

For each provider summary part:

1. Trim only presentation-level leading and trailing whitespace.
2. Treat a leading `**...**` pair as the part title when it is the first
   meaningful content.
3. Keep the remainder, including Markdown, bullets, links, and inline bold
   text, as the body.
4. Treat an empty placeholder body such as `<!-- -->` as no body.
5. If there is no leading title, keep all text as body and use the generic
   thinking status while streaming.
6. Deduplicate identical adjacent titles only in the rendered projection.
7. Never rewrite the canonical raw part text.

This prevents the current error where all bold fragments are interpreted as
titles and all non-bold content is discarded.

## Presentation States

### Streaming summary

Only one dynamic row is shown. It is replaced in place when a new summary part
or title arrives:

```text
⠋ 思考 · Planning final focused verification
```

Then:

```text
⠙ 思考 · Running exact regressions after formatting correction
```

The active title is the latest non-empty title from the current summary part.
Repeated titles do not create additional rows. The completed summary body is
not streamed into the ordinary scrollback because that would make the layout
unstable and would expose partial text as if it were a final answer.

### Completed summary

Title-only parts remain compact:

```text
● Planning final focused verification
```

A part with body retains both pieces:

```text
● Reviewing verification results
  Focused regressions pass. Formatting exposed two separate issues:
  • the new condition formatting needs to match rustfmt;
  • an unrelated pre-existing formatting mismatch exists.
  • I’m correcting only the lines I changed.
```

Collapsed output uses a bounded preview and a continuation hint. Expanded
output shows every retained part and body. The exact existing `Ctrl+O`
expansion interaction remains the owner for this view.

### Full thinking

Provider-returned full thinking remains separate from assistant messages:

```text
⠋ 思考中...
  checking the provider response
  reviewing the current transcript state
```

The completed preview is muted and bounded. Expansion reveals the retained
content. The `signature` and `redacted` metadata remain provider data and are
not displayed as ordinary answer text.

### Unknown thinking

Unknown reasoning content uses a generic status and does not extract a title:

```text
⠋ 正在处理...
```

If the provider explicitly sent a thinking event, the bounded content may be
shown in the same muted thinking block. If the provider did not send a
thinking event, ordinary text must never be reclassified as thinking.

### Commentary

Commentary is ordinary assistant output, but it is not the final answer. While
streaming it may use the dynamic status area:

```text
⠋ 工作中 · Planning final focused verification
```

When complete, it is retained as a low-emphasis transcript block:

```text
▸ Planning final focused verification
  Running exact regressions after formatting correction
```

Commentary must not be merged into a later `FinalAnswer` message.

### Final answer

Final-answer output is rendered through the existing normal Markdown path:

```text
已完成修改。

验证结果：

- 格式化检查通过
- 定点回归测试通过
```

It has no thinking spinner, no commentary marker, and no muted reasoning
style.

## Event Ordering

The renderer must preserve the source order:

```text
ThinkingStarted(Summary)
ThinkingDelta(...)
ThinkingFinished(...)
MessageStarted(Commentary)
TextDelta(...)
MessageFinished(Commentary)
ToolCallStarted(...)
ThinkingStarted(Summary)
ThinkingDelta(...)
ThinkingFinished(...)
MessageStarted(FinalAnswer)
TextDelta(...)
MessageFinished(FinalAnswer)
```

The visible projection is:

```text
⠋ 思考 · Reading the repository
▸ Checking the relevant files
[existing tool presentation]
⠙ 思考 · Reviewing the result
▸ Running the focused tests

最终回复正文
```

No event may be dropped merely because another event has a more convenient
visual representation.

## Persistence And Replay

- Normalized events remain provider-neutral.
- Message phase and thinking kind are persisted with defaults so historical
  events without the fields remain readable.
- Raw thinking and summary parts remain append-only records.
- Titles, collapse state, and deduplication are derived at render time.
- Replay uses the same semantic projection as live events.
- Historical events with missing message phase use `Unknown` and retain the
  existing assistant-message behavior.

## Compatibility Boundary

- Keep the existing `ThinkingKind` values and provider assignments.
- Keep existing thinking spinner and title replacement behavior.
- Preserve old serialized `ThinkingStarted` events through defaults.
- Preserve unknown-message-phase legacy rendering.
- Do not modify context cache prefixes, historical message order, or canonical
  session records.
- Do not change Delegate, DelegateGroup, DelegateSwarm, or Workflow card
  layout, hierarchy, expansion, sorting, or transcript placement.
- Do not create a second transcript owner or a second provider-normalization
  layer.

## Non-Goals

- Reconstructing OpenAI hidden reasoning.
- Treating final answers as reasoning because they contain Markdown headings.
- Inferring message phase from model names, timing, tool order, or text style.
- Adding user configuration for every provider-specific display variation.
- Rewriting the provider runtime or replacing the existing TUI transcript
  architecture.
- Changing the existing final-answer Markdown renderer.
- Changing Delegate-family cards.

## Alternatives Rejected

### Extract every bold fragment as a title

Rejected because it discards useful body text and misclassifies inline bold
content.

### Treat every provider reasoning block as a summary

Rejected because full reasoning providers and OpenAI summary providers have
different privacy and presentation semantics.

### Infer commentary from whether tools follow

Rejected because tool order is runtime behavior, not message semantics.

### Add one unified `ContentKind` enum

Rejected because thinking source, message phase, and lifecycle phase are
orthogonal and would create ambiguous combinations.

## Acceptance Criteria

1. A completed OpenAI summary containing a leading bold title and prose body
   renders both title and body.
2. Inline bold text in a summary body remains visible and is not promoted to a
   second title.
3. Repeated adjacent summary titles produce one rendered title while raw parts
   remain unchanged.
4. A streaming summary replaces one dynamic title row and does not append one
   row per title.
5. Full thinking from Anthropic or Google remains in a separate muted,
   bounded, expandable thinking block.
6. Unknown thinking never receives a title inferred from arbitrary Markdown.
7. Commentary and final-answer messages remain separate in live rendering,
   persisted events, replay, and transcript output.
8. Missing message phase preserves legacy assistant rendering.
9. OpenAI hidden reasoning is never fabricated or displayed as if returned.
10. Existing thinking tests, final-answer tests, persistence tests, and all
    non-Workflow Delegate-family presentation tests remain valid.
11. No context prefix or canonical session record is rewritten by the
    presentation change.

## Architecture Review Signal

- Canonical provider meaning remains in provider adapters and normalized
  `AiStreamEvent` values.
- Canonical lifecycle forwarding remains in `ModelTurnState` and
  `AgentEvent`.
- Canonical transcript projection remains in `TranscriptStore` and
  `TranscriptEntry` rendering.
- Derived title/body parsing remains a TUI presentation concern.
- No new fallback owner or compatibility runtime is introduced.
- The old summary-title-only projection is retired after body-preserving
  rendering is verified.

## Implementation Evidence Boundary

The later implementation must provide focused local evidence for provider
kind mapping, summary body preservation, full/unknown thinking rendering,
commentary/final separation, serialization defaults, replay, formatting, and
`git diff --check`. Local evidence does not prove live-provider behavior for
every endpoint or native Windows/Linux rendering.
