# Kimi-style Neo TUI Architecture Design

> Status: Implemented in code by `docs/superpowers/plans/2026-06-14-kimi-style-tui-architecture.md`; automated tests cover runtime scrollback, bounded live rows, tool-card finalization, streamed tool arguments, replayed messages/tools, and terminal-buffer sequencing. The remaining smoke evidence is covered by pure-buffer tests; this spec does not claim a manual real-terminal pass.

## Status

Approved direction from user: rebuild Neo's TUI around a Kimi Code / pi-tui-style architecture, with native terminal scrollback as the primary history model.

This design intentionally does **not** attempt to create a complete general-purpose clone of `@earendil-works/pi-tui`. It copies the parts Kimi Code actually uses and that Neo needs: component-tree rendering, render scheduling, append-oriented transcript components, stable in-place tool-call updates, bounded live previews, and native terminal scrollback preservation.

## Problem

Neo currently loses historical TUI content when output exceeds one screen. After `neo` starts, the welcome banner and earlier tool calls can be pushed upward and disappear instead of remaining in the terminal's native scrollback.

The root cause is architectural:

1. `crates/neo-agent/src/modes/interactive.rs::NeoTerminal::draw()` calls `render_app_lines(app, cols, rows)`.
2. `crates/neo-tui/src/app_renderer.rs::render_app_lines()` computes a body height from the current terminal height.
3. `render_body()` slices transcript rows to the currently visible row range and clamps them to that body height.
4. `crates/neo-tui/src/renderer.rs::InlineRenderer::render()` receives only a screen-sized frame, so old rows are already gone before the renderer can emit them into native scrollback.
5. The existing `committed_count`, `drain_newly_committed()`, and `live_transcript_items()` design in `crates/neo-tui/src/app.rs` is not wired into the active render path.

As a result, Neo overwrites one screen-sized frame instead of committing finalized history into terminal scrollback.

## Goals

- Preserve all finalized transcript history in the terminal's native scrollback.
- Keep the bottom live region stable: active assistant/tool output, activity panels, editor, and footer.
- Rebuild the TUI around a component tree similar to Kimi Code's practical use of pi-tui.
- Update tool calls in place instead of appending duplicate rows for each lifecycle event.
- Bound live and streaming previews so output height does not balloon and snap back.
- Make replay and live rendering use the same component paths.
- Ensure all rendered physical terminal rows are width-aware and do not rely on terminal auto-wrap.
- Add tests for scrollback, component rendering, tool-card lifecycles, and renderer behavior before relying on the new architecture.

## Non-goals

- Do not build a complete generic pi-tui replacement.
- Do not move Neo to TypeScript or depend on Kimi Code at runtime.
- Do not use alternate-screen mode as the primary interactive experience.
- Do not implement every Kimi Code side panel in the first phase.
- Do not keep duplicate live render paths once the new architecture is active.

## Reference: Kimi Code TUI Shape

Kimi Code uses a pi-tui component tree roughly shaped as:

```text
TUI
├─ transcriptContainer
├─ activityContainer
├─ todoPanelContainer
├─ queueContainer
├─ btwPanelContainer
├─ editorContainer
└─ footer
```

Important source references in `kimi-code/apps/kimi-code/src/tui`:

- `tui-state.ts`: creates `ProcessTerminal`, `TUI`, root containers, editor, and footer.
- `kimi-tui.ts`: builds the root layout, mounts footer, handles full-screen swaps, and coordinates app state.
- `controllers/streaming-ui.ts`: owns streaming assistant/tool state, throttled flushes, and in-place tool component updates.
- `components/messages/tool-call.ts`: renders one mounted tool-call component through start, streaming args, progress, result, expansion, and subagent state.
- `components/messages/tool-renderers/registry.ts`: selects tool-specific result renderers.
- `components/media/diff-preview.ts`: implements clustered edit diff previews.

Neo should copy these architectural contracts in Rust, not copy TypeScript implementation details line-for-line.

## Proposed Architecture

### Top-level runtime

Introduce a new TUI runtime layer:

```text
NeoTuiRuntime
├─ TuiState
│  ├─ transcript_container
│  ├─ activity_container
│  ├─ task_panel_container
│  ├─ queue_container
│  ├─ btw_panel_container
│  ├─ editor_container
│  └─ footer_container
├─ StreamingController
├─ RenderScheduler
└─ TerminalRenderer
```

The main interactive loop should route events into controllers, mark components dirty, and request renders. It should not directly flatten the whole app into a one-screen transcript frame.

### Component model

Create a lightweight component system in `crates/neo-tui`:

```rust
trait Component {
    fn render(&mut self, width: usize) -> Vec<Line>;
    fn invalidate(&mut self) {}
    fn handle_input(&mut self, input: InputEvent) -> InputResult {
        InputResult::Ignored
    }
}
```

Core components:

- `Container`: vertical stack of child components.
- `GutterContainer`: applies left/right gutter; avoids trailing padding.
- `Text`: width-aware, ANSI-aware wrapped text.
- `MarkdownText`: theme-aware markdown-ish rendering, built on existing markdown rendering where possible.
- `EditorComponent`: prompt box and cursor marker.
- `FooterComponent`: model/status/workspace/help hints.
- `ActivityComponent`: current thinking/tool/waiting state.
- Transcript components:
  - `BannerComponent`
  - `UserMessageComponent`
  - `AssistantMessageComponent`
  - `ThinkingComponent`
  - `ToolCallComponent`
  - `NoticeComponent`

The component tree should own visual state. High-level app/session state should update components rather than re-rendering unrelated transcript items from scratch.

## Native Scrollback Model

The terminal is split conceptually into two zones:

```text
native terminal scrollback
  finalized transcript rows

bottom live region
  active transcript components
  panels
  editor
  footer
```

### Finalized component commit

Each transcript component exposes whether it is finalized:

```rust
enum Finalization {
    Live,
    Finalized,
}
```

Examples:

- Banner: finalized immediately.
- User message: finalized immediately after submit.
- Assistant message: finalized after message finished.
- Thinking: finalized after thinking finished or merged into assistant display.
- Tool call: finalized after final tool result.
- Notice: finalized immediately unless explicitly live/progress-oriented.

On each render tick:

1. Drain the finalized prefix of `transcript_container`.
2. Render those components to stable rows.
3. Call `TerminalRenderer::commit_rows(rows)` to append them to native scrollback.
4. Remove or mark those components as committed so the live region only contains changing content.
5. Render the remaining live root components into the bottom live region.

This makes historical output persist after the `neo` command in normal terminal scrollback.

### Commit constraints

A component may only be committed when future events cannot change its rendered rows. Running tools, active assistant messages, live output, spinners, editor, and footer must not be committed.

The live region must be bounded by terminal height. If active content exceeds available rows, it should use local tail/collapse behavior, not push editor/footer off-screen.

## Render Scheduler

Replace unconditional full-frame rendering with explicit render requests:

```rust
enum RenderKind {
    Incremental,
    ForceFull,
}
```

`request_render()` schedules a normal incremental render. `request_render_force()` is reserved for structural cases.

Use normal incremental render for:

- assistant text deltas;
- thinking deltas;
- tool argument deltas;
- tool progress updates;
- live shell output;
- footer/activity changes;
- editor input.

Use force render only for:

- terminal resize;
- theme changes;
- entering or leaving full-screen overlays;
- returning from external editor;
- renderer state invalidation;
- root layout replacement.

Streaming flushes should be coalesced, matching Kimi Code's cadence:

```text
STREAMING_UI_FLUSH_MS = 50ms
STREAMING_ARGS_PREVIEW_MAX_CHARS = 64 KiB
```

## Terminal Renderer

The terminal renderer should have two separate operations:

```rust
fn commit_rows(&mut self, rows: &[Line]) -> io::Result<()>;
fn render_live_region(&mut self, rows: &[Line], cursor: Option<CursorPos>) -> io::Result<()>;
```

`commit_rows()` appends finalized rows into native scrollback and does not clear the screen.

`render_live_region()` diffs the bounded bottom live region. It may use the existing `InlineRenderer` logic after refactoring, but its state must be limited to live-region rows, not full transcript history.

All rows passed to either operation must satisfy:

```text
visible_width(row) <= terminal_width
```

Long unbroken strings must be hard-wrapped or truncated before reaching the terminal renderer.

## Streaming Controller

Move streaming event state out of monolithic app rendering into a controller similar to Kimi Code's `StreamingUIController`.

Responsibilities:

- track active assistant/thinking components;
- track active tool calls by id;
- store bounded streaming tool args;
- coalesce dirty tool ids;
- update mounted components in place;
- request renders through the scheduler.

Tool event lifecycle:

```text
ToolCallStarted
  → create ToolCallComponent if absent
  → add to transcript_container
  → request_render()

ToolCallArgumentsDelta
  → append bounded arg preview
  → mark tool dirty
  → schedule_flush()

ToolCallFinished
  → finalize args on existing component
  → request_render()

ToolExecutionUpdate
  → append progress/live output
  → request_render()

ToolExecutionFinished
  → set result on existing component
  → mark finalized
  → request_render()
```

This replaces duplicate transcript updates with one component per tool call.

## ToolCallComponent

`ToolCallComponent` owns the visual lifecycle of one tool call.

State:

```rust
struct ToolCallComponent {
    id: String,
    name: String,
    args: ToolArgs,
    streaming_args: Option<String>,
    result: Option<ToolResultView>,
    status: ToolStatus,
    expanded: bool,
    progress_lines: Vec<String>,
    live_output: String,
}
```

Rendered structure:

```text
ToolCallComponent
├─ spacer
├─ header
├─ call preview
├─ progress block
├─ live output block
└─ result body
```

Header examples:

```text
● Using Read (crates/neo-tui/src/app.rs)
✓ Used Read (crates/neo-tui/src/app.rs) · 420 lines
✗ Failed Bash (cargo test -p neo-tui) · exit 101
```

Updates must be in-place:

- `update_tool_call()` updates args/header/preview.
- `set_result()` clears live progress/output, updates status/chip/body, and marks finalized.
- No lifecycle event should append a second card for the same tool id.

## Tool Renderer Registry

Add a flat registry for per-tool previews, chips, and results:

```text
Read           -> read_summary
ReadMediaFile  -> media_summary
Grep           -> grep_summary
Glob           -> glob_summary
FetchURL       -> fetch_summary
WebSearch      -> web_search_summary
Bash/Shell     -> shell_renderer
Edit           -> edit_preview
Write          -> write_preview
Agent          -> agent_renderer
AgentSwarm     -> swarm_renderer
default        -> truncated_renderer
```

Suggested Rust interface:

```rust
trait ToolRenderer {
    fn render_call_preview(&self, ctx: &ToolRenderContext) -> Vec<Line>;
    fn render_result(&self, ctx: &ToolRenderContext) -> Vec<Line>;
    fn chip(&self, ctx: &ToolRenderContext) -> Option<String>;
}
```

The registry prevents `app_renderer.rs::render_tool_lines()` from becoming a large duplicated match and keeps tests focused.

## Kimi-style Tool Behavior

### Edit

While args are streaming, do not render partial diffs. Render a stable progress line:

```text
Preparing changes for crates/neo-tui/src/app.rs... 12.4 KB · 3s elapsed
```

After args finalize, render a clustered diff from `old_string`, `new_string`, and `path`/`file_path`:

```text
+12 -4 crates/neo-tui/src/app.rs
  120   context
  121 - old line
  122 + new line
        … 35 unchanged lines …
  158 + another change
        … 8 more changes hidden (ctrl+o to expand)
```

Defaults:

- `context_lines = 3`
- collapsed max body rows = `10`
- expanded removes the collapsed cap or uses a high safety cap.

This preview should not depend solely on `ToolResult.details.diff`; it should primarily use call args, matching Kimi Code's behavior.

### Write

Once write args finalize, immediately cap the preview. Do not briefly render the full file and then collapse after the result arrives.

Collapsed preview:

```text
path/to/file.rs · 80 lines
1 use ...
2 ...
...
... (70 more lines, 80 total, ctrl+o to expand)
```

### Bash / Shell

While running, show command plus bounded live output tail. Final result clears live output and renders the authoritative result state.

### Read

Initially implement single read cards. Later, add same-step `ReadGroupComponent` that upgrades multiple consecutive reads into:

```text
✓ Read 4 files · 830 lines
  ├─ crates/neo-tui/src/app.rs · 420 lines
  ├─ crates/neo-tui/src/renderer.rs · 180 lines
  └─ ...
```

### Agent / AgentSwarm

After the base tool-card path is stable, implement same-step agent grouping using child component snapshots, matching Kimi Code's pattern.

## Global Expansion

Add a global `tool_output_expanded` state toggled by `Ctrl+O`.

Behavior:

1. Toggle global expansion.
2. Iterate transcript components.
3. Call `set_expanded()` on expandable components.
4. Request render.

New live/replayed components inherit the current expansion state.

## Replay and Live Consistency

Session replay must call the same component lifecycle methods as live events:

```text
Replay tool call
  → StreamingController::on_tool_call_start()
Replay tool result
  → StreamingController::on_tool_result()
```

There should not be a separate visual renderer for replayed tool calls.

## Text and ANSI Width Infrastructure

Add or consolidate utilities:

- `strip_ansi()`
- `visible_width_ansi()`
- `wrap_ansi()`
- `truncate_ansi()`
- `hard_wrap_plain_or_ansi()`
- `Line` / `Span` styled row model

Every component must render width-bounded rows. This avoids terminal auto-wrap, which otherwise breaks cursor and live-region diff calculations.

## Removal Plan

The policy for this refactor is no old TUI path and no second transcript renderer to maintain. Intermediate scaffolding may exist only while a task is in progress; the completed architecture migrates or removes viewport-sliced interactive rendering instead of leaving a second renderer behind.

### Phase 1: TUI core

- Add component trait and row/span model.
- Add `Container`, `GutterContainer`, and `Text`.
- Add `RenderScheduler`.
- Refactor or replace `InlineRenderer` into a live-region `TerminalRenderer`.
- Add minimal editor/footer components.

### Phase 2: Transcript scrollback

- Add transcript components and finalization state.
- Implement finalized-prefix draining.
- Implement `commit_rows()`.
- Prove banner, user messages, assistant messages, and completed tools remain in native scrollback.

### Phase 3: Streaming and tool cards

- Add `StreamingController`.
- Add `ToolCallComponent`.
- Add tool renderer registry.
- Implement Read, Write, Edit, and Bash basics.
- Ensure tool lifecycle updates happen in place.

### Phase 4: Kimi behavior parity

- Add global `Ctrl+O` expansion.
- Add clustered Edit diff preview.
- Add bounded Write preview.
- Add live Bash tail.
- Add replay/live path unification.
- Add ReadGroup and AgentGroup after single-card stability.

### Phase 5: Remove old duplicated paths

- Remove `app_renderer.rs` from the interactive transcript render path and do not leave a second renderer behind.
- Migrate or delete `components.rs` duplicate transcript rendering so the component tree is the single maintained renderer.
- Remove or replace dead `committed_count` plumbing.
- Update `docs/gap/tui.md` and relevant architecture docs to state that the old TUI path has been removed or migrated.

## Testing Strategy

### Component tests

- Container stacks children in order.
- GutterContainer applies left gutter and avoids trailing padding.
- Text wrapping never exceeds width.
- ANSI styles do not corrupt visible-width calculations.

### Scrollback tests

- Finalized banner/user/assistant/tool rows are committed.
- Running tool rows remain live.
- Finished tool rows commit after result.
- Committed rows are not re-rendered in live region.
- More than one screen of transcript remains available as native scrollback output.

### Renderer tests

- Live-region update does not clear committed history.
- Resize triggers force redraw of live region only.
- Long unbroken lines are wrapped or truncated before terminal output.
- Cursor remains stable when live region height changes.

### Tool-card tests

- Tool start creates one component.
- Args deltas update that component in place.
- Result completes that component in place.
- Edit streaming shows a progress line, not partial diff.
- Edit finalized args show clustered diff.
- Write finalized args are capped before result.
- Bash live output tails while running and clears on final result.
- `Ctrl+O` toggles all expandable cards.

### Replay tests

- Replayed tool calls use the same component lifecycle as live calls.
- Replay output matches live rendering for equivalent events.

## Risks and Mitigations

### Risk: the rewrite becomes too broad

Mitigation: copy only Kimi Code's practical component contracts. Do not build unrelated widgets or a public TUI framework.

### Risk: scrollback gets corrupted by full redraws

Mitigation: isolate committed rows from live-region redraws. Reserve force redraw for structural changes.

### Risk: terminal auto-wrap breaks cursor math

Mitigation: enforce width-bounded rows before terminal output and test long JSON/path/URL cases.

### Risk: duplicated old/new render paths drift

Mitigation: migrate replay and live to the new component path, then retire old transcript rendering paths.

### Risk: large Edit/Write previews cause height churn

Mitigation: match Kimi Code: Edit streaming uses a stable progress line, Write finalized args are capped immediately, and finalized Edit uses clustered diff with collapsed caps.

## Acceptance Criteria

- Running `neo` preserves the startup banner and earlier tool calls in native terminal scrollback after output exceeds one screen.
- New streaming content updates the bottom live region without erasing committed transcript history.
- Tool calls update in place from running to finished/failed.
- Edit, Write, Bash, and Read cards match Kimi Code's stable collapsed behavior.
- `Ctrl+O` expands/collapses existing and future expandable transcript components.
- Replay and live event rendering produce the same visual structure.
- Tests cover component layout, scrollback commit, terminal rendering, tool-card lifecycle, and replay parity.
