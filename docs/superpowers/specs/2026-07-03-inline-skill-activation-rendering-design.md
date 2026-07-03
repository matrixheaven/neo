# Inline skill activation rendering design

## Problem

When a user prompt contains `/skill:<name>` together with a pasted block (e.g. `[paste #1 +86 lines]`), Neo currently renders the same content twice in the TUI transcript:

1. Once inside the `✦ Skill activated:` card, where the body still contains the raw `[paste #1 +86 lines]` marker.
2. Again as a separate `✨` user-message card, because the runtime expands the paste marker when it echoes the user message back.

The expected behavior, matching Kimi Code, is a **single** transcript card that shows the skill activation and the user's full prompt.

## Root cause

`parse_inline_skill_directives` strips `/skill:` syntax and returns a `stripped_prompt` that still contains raw paste/image markers. Two code paths then diverge:

- `activate_skill_directives` pushes a `SkillActivation` transcript entry whose body is the raw `stripped_prompt`.
- `start_turn_from_submitted_prompt` calls `expand_prompt_markers` on the same `stripped_prompt`, so the runtime's `MessageAppended` event carries the expanded text.

`render_appended_user_message_if_needed` tries to suppress the duplicate user message by comparing the raw `stripped_prompt` with the expanded display text. They no longer match when markers are present, so the suppression fails and a second card appears.

## Goals

1. A prompt with inline `/skill:<name>` renders as exactly one `SkillActivation` transcript card.
2. The card body shows the same text the user will see in the runtime-echoed user message.
3. `/skill:` may still appear anywhere in the prompt (Neo keeps its inline behavior).
4. Existing tests and suppression logic remain valid; only the pre-comparison expansion step changes.

## Non-goals

- Changing how model-tool skill activations (`AgentEvent::SkillActivated`) render; those still produce a separate empty-body card.
- Changing slash commands such as `/model`, `/plan`, `/permissions`.
- Adding a new transcript entry type; the existing `SkillActivation` variant is sufficient.

## Design

### Data flow change

In `crates/neo-agent/src/modes/interactive/mod.rs`, inside `submit_current_prompt`, after inline skill directives are parsed and before the `SkillActivation` card is pushed:

1. Expand paste/image markers in `stripped_prompt` using the same `expand_prompt_markers` function that the turn uses:
   ```rust
   let expanded = crate::prompt::parts::expand_prompt_markers(
       &stripped_prompt,
       &self.paste_store,
       &self.image_attachment_store,
   );
   let display_text = content_to_display_text(&expanded);
   ```
2. Pass `display_text` to `activate_skill_directives` so the `SkillActivation` card body is the final display text.
3. Store `display_text` in `pending_skill_user_message_to_suppress`.
4. Continue submitting the original `stripped_prompt` to the runtime; `start_turn_from_submitted_prompt` will expand markers again as usual.

Because the card body and the runtime-echoed user message are now derived from the same expanded content, the existing exact-match suppression in `render_appended_user_message_if_needed` succeeds.

### Skill context

The XML skill context (`<neo-user-request>`) injected via `pending_skill_context` keeps the original `stripped_prompt` with markers. This avoids duplicating a large pasted block inside the model context: the actual user message already carries the expanded content.

If future experience shows the model needs the expanded text inside the skill context, this can be changed independently without affecting the UI fix.

### Edge cases

| Scenario | Behavior |
|---|---|
| No markers in prompt | `display_text` equals `stripped_prompt`; behavior unchanged. |
| Large paste | `SkillActivation` body contains full expanded text. It is collapsible (3-line preview, `ctrl+o` to expand), same as today. |
| Image markers | `display_text` renders as `[image #N (WxH)]`, identical to the runtime echo. |
| `/skill:foo args` with empty body | No turn is submitted; only the skill card is shown. |
| Skill activation fails | No card is pushed; error status is shown. |

## Testing

Add one integration test in `crates/neo-agent/src/modes/interactive/tests.rs`:

- Input: `/skill:skill_one [paste #1 +3 lines]review this`
- Seed the paste store with a multi-line string.
- Assert the transcript contains exactly one `SkillActivation` whose body equals the expanded paste text plus `review this`.
- Assert the transcript does **not** contain a `UserMessage` with the same text.

Update the existing suppression test if its assertions need to reflect marker expansion.

## Verification

Run the targeted test after implementation:

```bash
cargo nextest run -p neo-agent --lib inline_skill_directive
```

## References

- `crates/neo-agent/src/modes/interactive/prompt_edit.rs` — `parse_inline_skill_directives`, `content_to_display_text`
- `crates/neo-agent/src/modes/interactive/slash_commands.rs` — `activate_skill_directives`, `push_skill_invocation_entry`
- `crates/neo-agent/src/modes/interactive/mod.rs` — `submit_current_prompt`, `render_appended_user_message_if_needed`
- `crates/neo-agent/src/prompt/parts.rs` — `expand_prompt_markers`
- `crates/neo-tui/src/transcript/entry/mod.rs` — `SkillActivation` rendering
