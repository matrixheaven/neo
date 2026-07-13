# Skill Invocation Transcript Design

## Problem

AI-initiated `Skill` calls execute successfully but do not produce a transcript card when the runtime uses the default parallel tool execution mode. The sequential path emits `AgentEvent::SkillActivated`; the parallel path does not. The TUI intentionally suppresses the generic `Skill` tool card, so the missing semantic event leaves no visible trace.

The same suppression also makes failed AI skill invocations invisible. Manual `/skill:*` activation renders directly through a separate transcript entry path and has no source marker.

## Goals

- Render every automatic skill invocation in the transcript under the default parallel execution mode.
- Use one visual component for automatic and manual activation.
- Distinguish sources with a muted `auto` or `manual` marker.
- Show failed automatic invocations instead of silently dropping them.
- Remove the success-only `SkillActivated` event and its sequential-only emission path.

## Non-Goals

- Showing the expanded skill instructions in the transcript.
- Rendering the internal `Skill` call as a generic tool card.
- Changing skill discovery, argument expansion, or invocation policy.
- Adding compatibility handling for the obsolete `SkillActivated` event shape.

## Event Model

Replace `AgentEvent::SkillActivated` with `AgentEvent::SkillInvocation`:

```rust
pub enum SkillInvocationSource {
    Auto,
    Manual,
}

pub enum SkillInvocationOutcome {
    Activated,
    Failed,
}

AgentEvent::SkillInvocation {
    names: Vec<String>,
    source: SkillInvocationSource,
    outcome: SkillInvocationOutcome,
    body: String,
}
```

The event has no `turn` field because manual activation happens before the associated runtime turn starts and the transcript renderer does not use a turn number for skill cards.

Automatic invocation emits the semantic event in the common post-execution stage shared by sequential and parallel scheduling. Successful events use formatted invocation arguments as `body`; failed events use the tool error content. Manual `/skill:*` activation constructs the same event with `source: Manual` and the user request as `body`.

The runtime continues emitting the ordinary tool lifecycle events for recording and model execution. The TUI continues absorbing the generic `Skill` tool card so only the semantic invocation card is visible.

## Transcript Model

`TranscriptEntry::SkillActivation` stores the same source, outcome, names, body, and expansion state. One renderer handles every combination.

No-body success:

```text
✦ Skill activated: using-superpowers · auto
```

Success with details:

```text
✦ Skill activated: code-simplifier · auto
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
target: transcript rendering
focus: remove duplicated branches
```

Manual activation:

```text
✦ Skill activated: brainstorming · manual
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Design the skill transcript card
```

Failure:

```text
✕ Skill failed: using-superpowers · auto
  Skill `using-superpowers` is not available
```

The source marker uses muted text. Skill names retain the brand color. Activated and failed labels use the existing warning and error colors respectively. Successful cards render a divider only when the body is non-empty. Failed cards omit the divider and indent the error body by two spaces. Bodies retain the current three-line collapsed preview and `ctrl+o` expansion behavior.

## Session Replay

Recorded automatic `SkillInvocation` events are replayed into the transcript alongside delegate lifecycle events. Manual activation uses the same live semantic event but does not expand this task into changing the existing persistence boundary for controller-created transcript entries.

## Error Handling

- Successful `Skill` results produce `Activated`.
- Any error result, including invalid arguments, missing skills, disabled skills, and hook-modified failures, produces `Failed`.
- When a skill name cannot be recovered from parsed arguments, the card uses `unknown` rather than hiding the event.
- Failed calls never render the generic tool card in addition to the semantic failure card.

## Testing

- A runtime integration test uses the default parallel mode and asserts that a successful automatic call emits `SkillInvocation { source: Auto, outcome: Activated }`.
- The same runtime boundary asserts that a missing skill emits a failed semantic event with the error message.
- TUI tests assert the compact no-body header, source marker, conditional divider, and failure rendering.
- An interactive test drives a real fake-model turn and asserts that one semantic skill entry appears without a generic `ToolRun` entry.
