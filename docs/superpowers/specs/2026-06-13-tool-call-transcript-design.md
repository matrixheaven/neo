# Tool-Call Transcript UI Redesign

## Status

Design approved. Awaiting implementation plan.

## Problem

In Neo's interactive TUI, every in-flight tool call currently renders as a duplicated orange-yellow line:

```text
● Use bash(find crates/neo-ai/src -name '*.rs' -type f 2>/dev/null | sort)      running
● Use bash(find crates/neo-agent-core/src -name '*.rs' -type f 2>/dev/null | sort)      running
● Use bash(find crates/neo-sdk/src -name '*.rs' -type f 2>/dev/null | sort)      running
```

These lines accumulate because the rendering layer appends a fresh "running" row on each frame instead of updating a single stateful card in place. The result is noisy, wastes vertical space, and obscures the actual transcript.

## Goal

Replace the duplicated "Use XXXX running" lines with a single stateful tool-call card per invocation that mutates in place from `Using` → `Used`/`Failed`/`Cancelled`, similar to the `@kimi-code/` tool-call transcript.

## Scope

- **In scope:** Neo's interactive TUI (`neo` with no subcommand or `neo interactive`).
- **Out of scope:** Non-interactive `neo run` output, JSONL session files, MCP resource commands.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Scope | TUI only | Non-interactive modes intentionally emit machine-readable events. |
| Running indicator | Static bullet `●` | Avoids spinner flicker; kimi-code uses the same choice. |
| Finished indicators | `✓` success, `✗` failure, `⊘` cancelled | Clear, compact, Unicode-only. |
| Running color | Accent blue `#58A6FF` (`theme.accent`) | Moves emphasis away from the old orange "warning" tone. |
| Success/failure colors | Existing green `#65B883` / red `#F85149` | Consistent with current Neo theme. |
| Result body | Collapsed to 3 lines by default | Keeps transcript compact; expandable with `Ctrl+O`. |
| Result chip | Smart per-tool-type summary | More informative than a generic byte count. |
| Parallel calls | Separate cards | Simpler than grouping; matches model-centric view. |
| Live Bash output | Show last 3 lines while running | Users can see progress; cleared on completion. |

## Visual Design

### Header States

The header is exactly one terminal line. It updates in place.

```text
● Using Read (crates/neo-tui/src/components.rs)
● Using Bash (find crates -name '*.rs' | sort)
✓ Used Read (crates/neo-tui/src/components.rs) · 128 lines
✗ Failed Bash (cargo test -p neo-tui) · exit 101
⊘ Cancelled Read (large.log)
```

Color mapping:

- `● Using ...` — status symbol, verb, and tool name in accent blue (`theme.accent`).
- `✓ Used ...` — status symbol, verb, and tool name in success green (`theme.succeeded`).
- `✗ Failed ...` — status symbol, verb, and tool name in failure red (`theme.failed`).
- `⊘ Cancelled ...` — status symbol, verb, and tool name in muted gray (`theme.muted`).
- Key argument and chip are always dim gray.

### Header Layout

```text
<status-symbol><space><verb><space><tool-name><space><key-arg><space><chip>
```

- `<status-symbol>` — one grapheme: `●`, `✓`, `✗`, or `⊘`.
- `<verb>` — `Using`, `Used`, `Failed`, or `Cancelled`.
- `<tool-name>` — bold, colored by status.
- `<key-arg>` — dim gray, parenthesized, truncated to remaining width.
- `<chip>` — dim gray, only when finished, e.g. `· 128 lines`.

### Card Anatomy

```text
● Using Read (crates/neo-tui/src/components.rs)
  use ratatui::{
      widgets::{Block, Borders, Paragraph, Wrap},
      style::{Color, Style, Modifier},
      text::{Line, Span, Text},
  ...
```

- Body is indented by two spaces.
- Default preview is 3 content lines.
- If the result is shorter than 3 lines, show all lines and omit the hint.
- If expanded, show full output wrapped to the card width.

### Collapsed Finished Card

```text
✓ Used Read (crates/neo-tui/src/components.rs) · 128 lines
  pub fn render_transcript(...) {
      let rows = build_rows(transcript);
      widget.render(area, buf);
  ... (125 more lines, ctrl+o to expand)
```

### Live Bash Output While Running

```text
● Using Bash (cargo test -p neo-tui)
  running 12 tests
  test app::tests::event_mapping ... ok
  test app::tests::render_tool ... ok
```

Live output is capped to the last 3 lines and is cleared when the final result arrives.

### Failed Tool

```text
✗ Failed Bash (cargo test -p neo-tui) · exit 101
  error[E0061]: this method takes 2 arguments but 1 argument was supplied
    --> crates/neo-tui/src/components.rs:612:14
  ... (8 more lines, ctrl+o to expand)
```

Error output is tinted with the failure red.

## Smart Chips

| Tool | Chip |
|------|------|
| Read / Write / Edit | `· N lines` |
| Bash / Shell | `· N bytes`, or `· exit N` on failure |
| Grep | `· N matches` |
| Find / Glob | `· N files` |
| Generic / MCP | `· N bytes` |

Chip text is dim gray and appended to the header line.

## Architecture

### State Model

Extend `ToolRunTranscript` (or wrap it) with status and live output:

```rust
pub struct ToolCard {
    pub id: String,
    pub name: String,
    pub key_arg: String,
    pub status: ToolStatusKind,
    pub live_output: Vec<String>,
    pub result: Option<ToolResult>,
    pub expanded: bool,
}
```

`ToolStatusKind` already exists:

```rust
pub enum ToolStatusKind {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}
```

### Event Flow

```text
AgentEvent::ToolCallStarted
  → NeoTuiApp::apply_stream_update(StreamUpdate::ToolStarted { id, name, detail })
  → ChatTranscript::push_or_update_tool(id)
  → TranscriptWidget renders the card

AgentEvent::ToolExecutionUpdate(partial)
  → append to card.live_output
  → request render

AgentEvent::ToolExecutionFinished(result)
  → card.status = Succeeded | Failed
  → card.live_output.clear()
  → card.result = Some(result)
  → request render
```

Because ratatui re-renders the whole widget each frame, the previous frame's header/body is overwritten naturally. No explicit line-diffing is required.

### Rendering Pipeline

In `TranscriptWidget::render`:

```rust
for item in &transcript.items {
    match item {
        TranscriptItem::User(u) => render_user_bubble(u),
        TranscriptItem::Assistant(a) => render_assistant_bubble(a),
        TranscriptItem::Tool(t) => render_tool_card(t),
        TranscriptItem::Image(i) => render_image(i),
        // ...
    }
}
```

`render_tool_card` builds:

1. Header line via `build_tool_header`.
2. Body lines via `build_tool_body`:
   - If running and `live_output` non-empty: last 3 live lines.
   - If finished and result present: first 3 result lines or full output if expanded.
3. Expand hint if body is truncated and not expanded.

### Key Argument Extraction

For each tool, extract a single human-readable argument:

- `Read` / `Write` / `Edit` — file path.
- `Bash` / `Shell` — the command string.
- `Grep` / `Find` / `Glob` — pattern or glob.
- Generic / MCP — first non-empty string argument, or tool name if none.

The argument is one-lined and truncated to fit the header width.

### Interaction

- `Ctrl+O` toggles expand/collapse of the selected tool card.
- The selected card is highlighted with a subtle background or left border.
- Existing transcript scrolling and selection behavior remains unchanged.

## Error Handling

- If a tool finishes without a result, render `✗ Failed <name>` with no chip and a generic error body.
- If live output exceeds the cap, drop oldest lines first.
- If Unicode symbols are unavailable (e.g. fallback terminal font), keep the symbol logic but allow ASCII fallbacks via theme config.

## Testing

- Unit tests in `crates/neo-tui/tests/primitives.rs` update expected header strings from `● Use read(` to `● Using Read` and assert no duplicate "running" suffix.
- New tests assert:
  - Header color matches status.
  - Chip appears only when finished.
  - Live output is cleared on final result.
  - Expand/collapse toggles body line count.
- Integration test in `crates/neo-agent/tests/` drives the TUI with `FakeHarness` and verifies a tool card renders without duplicate lines.

## Migration Notes

- Remove `tool_status_suffix` and the old `running`/`pending` suffix strings from `crates/neo-tui/src/components.rs`.
- Keep `tool_status_symbol` but change its output to the new static symbols.
- Update `status_style` so that `Running` maps to `theme.accent` instead of `theme.running`.
- Existing user themes can keep `running`; the new UI ignores it for the running header and uses `accent` instead.

## Open Questions (resolved)

| Question | Resolution |
|----------|------------|
| Scope | TUI only. |
| Running indicator | Static bullet `●`. |
| Result body default | Collapsed to 3 lines. |
| Header chip | Smart per-tool-type summary. |
| Parallel calls | Separate cards. |
| Running color | Primary blue. |
| Live Bash output | Stream last 3 lines while running. |

## References

- Current Neo TUI rendering: `crates/neo-tui/src/components.rs`, `crates/neo-tui/src/app.rs`.
- Current Neo event handling: `crates/neo-agent/src/modes/interactive.rs`, `crates/neo-agent-core/src/events.rs`.
- Reference implementation: `kimi-code/apps/kimi-code/src/tui/components/messages/tool-call.ts`.
