# Neo TUI Header/Footer Redesign

**Status:** Approved for implementation  
**Date:** 2026-06-13  
**Author:** Agent design session  
**Scope:** `crates/neo-tui`, `crates/neo-agent/src/modes/interactive.rs`, themes, tests

---

## 1. Background & Problem

Neo’s current interactive TUI renders a **persistent top header** and a **bottom footer** that duplicate the same dynamic information:

| Field | Header | Footer |
|---|---|---|
| Model | `model:<model>` | `<model>` |
| Session | `session:<session>` | `<session>` |
| Context | `ctx <used>/<max>` | `ctx <used>/<max>` |
| Working | `● working · esc interrupt` | `working · esc interrupt` |

This wastes vertical space and creates visual noise. The four reference open-source projects—Claude Code, OpenAI Codex, Pi, and Kimi Code—solve the same problem differently but converge on one idea: **no persistent top header**. They display session/model context in a startup banner that scrolls away, and put operational status in a richer, color-coded footer.

## 2. Design Goals

1. **Eliminate duplication** between header and footer.
2. **Free the top row** for transcript content.
3. **Move identity/context info** into a startup banner rendered as the first transcript item.
4. **Make the footer informative and glanceable** by using semantic color instead of an all-grey palette.
5. **Preserve Neo’s local-only, minimal aesthetic** and keep the bordered composer input.
6. **Remain themeable** via the existing `TuiTheme` / `.neo/themes/*.json` mechanism.

## 3. Reference Analysis

| Project | Top Header | Startup Banner | Footer Style | Color Strategy |
|---|---|---|---|---|
| **Claude Code** | None | Logo card in transcript, scrolls away | Single-line, left/right split | Permission mode is color-coded (teal/violet/red/amber) |
| **OpenAI Codex** | None | Session header cell in history | Single-line bottom pane + configurable status line | Mode (`Plan`/`Pair`/`Execute`) uses magenta/cyan/dim; context usage uses softened semantic colors |
| **Pi** | None | Collapsible help header in transcript | Single-line status bar | Context percentage turns yellow/red above 70%/90%; token stats dim |
| **Kimi Code** | None | Boxed welcome card + optional remote banner | **Two-line footer** | Mode badges (`yolo`, `plan`, `swarm`) use warning/primary/accent; rotating toolbar tips |

Key takeaways:
- A **startup banner** is the conventional place for model/session/directory identity.
- A **two-line footer** (Kimi style) gives the cleanest separation between *status* and *hints*.
- **Color should encode meaning**: permission → semantic color, working → accent, context → threshold-aware, hints → muted.

> Note: Kimi/Codex use product-specific modes such as `yolo`, `plan`, or `Pair Programming`. Neo does not define those modes today; Neo’s footer badges reflect the existing `PermissionDecision` values (`Allow`, `Ask`, `Deny`).

## 4. Proposed Design

### 4.1 High-level Layout

```text
┌────────────────────────────────────────────────────────────────────────────┐
│                                                                            │
│  ╭──────────────────────────────────────────────────────────────────────╮  │
│  │  Welcome to Neo  v0.1.0                                              │  │
│  │  Session: abc-123   Model: anthropic/deepseek-v4-pro[1m]             │  │
│  │  /Users/chenyuanhao/Workspace/neo                                    │  │
│  ╰──────────────────────────────────────────────────────────────────────╯  │
│                                                                            │
│   [transcript body — conversation scrolls here; banner scrolls away]       │
│                                                                            │
│                                                                            │
│   ┌────────────────────────────────────────────────────────────────────┐   │
│   │ > 帮我优化这个 TUI 设计                                            │   │
│   └────────────────────────────────────────────────────────────────────┘   │
│  [ask] ● working · esc interrupt  ~/Workspace/neo                        │
│  enter send · shift+enter newline · / commands        ctx 12.3k / 200k     │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

- **No persistent top header.**
- **Startup banner** is rendered once as the first transcript item and scrolls away naturally.
- **Two-line footer** sits directly beneath the composer input, like Kimi Code.
- **Composer input** keeps its existing bordered style (`#1f232b` background).

### 4.2 Startup Banner

Rendered as a new `TranscriptItem::Banner` variant so it participates in normal scrollback and can be styled as a boxed card. Content:

```text
╭──────────────────────────────────────────────────────────────────────╮
│  Welcome to Neo  v0.1.0                                              │
│  Session: abc-123   Model: anthropic/deepseek-v4-pro[1m]             │
│  /Users/chenyuanhao/Workspace/neo                                    │
╰──────────────────────────────────────────────────────────────────────╯
```

Fields:
1. Brand + version (`Welcome to Neo v0.1.0`).
2. Session label + model label.
3. Working directory (workspace root).

Optional future additions: loaded MCP server count, trust status, hint to send `/help`.

### 4.3 Footer Line 1 — Status Bar

Left-to-right, space-separated:

```text
[ask] ● working · esc interrupt  ~/Workspace/neo
```

| Segment | Example | Color |
|---|---|---|
| Permission summary badge | `[ask]` or `[allow]` or `[deny]` | `accent` for ask, `success` for allow, `danger` for deny |
| Working state | `● working · esc interrupt` | `accent` when active, `muted` when idle |
| Working directory | `~/Workspace/neo` | `muted` |
| Background tasks/goals | `[2 tasks]` | `muted` or `accent` when active |

The line is truncated from the right on narrow screens; lowest-priority segments (task counts, then cwd) are dropped first.

### 4.4 Footer Line 2 — Hint Bar

Left-to-right:

```text
enter send · shift+enter newline · / commands        ctx 12.3k / 200k
```

| Segment | Example | Color |
|---|---|---|
| Keyboard hints | `enter send · shift+enter newline · / commands` | `muted` |
| Context usage | `ctx 12.3k / 200k` | `muted` under 70%, `warning` at 70–89%, `danger` at 90%+ |
| Cost (optional future) | `$0.004` | `muted` |

The context segment is right-aligned. When the terminal is too narrow, hints collapse to the most essential (`enter send · esc interrupt`) and context remains visible.

### 4.5 Color Semantics

Extend `TuiTheme` with these semantic tokens (all default to existing palette colors):

```rust
pub struct TuiTheme {
    // existing fields ...

    // Footer semantic colors
    pub footer_permission_allow: Color,   // default: success
    pub footer_permission_ask: Color,     // default: accent
    pub footer_permission_deny: Color,    // default: danger
    pub footer_working: Color,            // default: accent
    pub footer_context_ok: Color,         // default: muted
    pub footer_context_warn: Color,       // default: warning
    pub footer_context_critical: Color,   // default: danger
    pub footer_hint: Color,               // default: muted
}
```

Theme JSON files can override any of these. Existing themes that do not specify them fall back gracefully.

## 5. Architecture Changes

### 5.1 Files to Modify

| File | Change |
|---|---|
| `crates/neo-tui/src/components.rs` | Remove header row from `app_layout()`; remove header render block; rewrite `render_footer()` as two lines; add banner render path for `TranscriptItem::Banner`. |
| `crates/neo-tui/src/app.rs` | Add `workspace_root` to `NeoTuiApp`; add semantic footer colors to `TuiTheme`; add helper methods for footer segments (`permission_badge()`, `working_label()`, `context_color()`). |
| `crates/neo-agent/src/themes.rs` | Deserialize new semantic color keys with defaults. |
| `crates/neo-agent/src/modes/interactive.rs` | Pass `workspace_root` into `NeoTuiApp::new`; ensure startup notices are rendered as the banner item. |
| `crates/neo-tui/tests/app_shell.rs` | Update assertions: no top header, footer has two lines, banner appears in transcript. |

### 5.2 Layout Geometry

Current `AppLayout` reserves one row for header and one for footer. New layout:

```text
body:    terminal.height - footer_rows - prompt_height
footer:  2 rows
prompt:  variable (bordered multi-line composer)
```

`footer_rows` is 2 when terminal height ≥ some minimum (e.g., 12 rows); on very short terminals collapse to 1 row (status only, hints hidden) or 0 rows (full-screen body only) when height is critically small.

### 5.3 Data Flow

1. `NeoTuiApp` computes `permission_badge`, `working_label`, `cwd_label`, `context_label` each frame from existing state.
2. `render_footer()` receives the footer rectangle split into two 1-row `Line`s.
3. Each line is built from left-justified and right-justified `Span`s.
4. Colors come from `TuiTheme` semantic tokens.
5. The startup banner is pushed into `transcript.items` once at session start; subsequent frames do not re-render it.

## 6. Responsive Behavior

| Terminal Width | Footer Line 1 | Footer Line 2 |
|---|---|---|
| ≥ 120 cols | Full badge + cwd + tasks | Full hints + right-aligned context |
| 80–119 cols | Drop task counts | Collapse hints to essentials |
| 60–79 cols | Drop cwd, keep badge + working | Keep context, hide hints |
| < 60 cols | Keep only badge + working | Keep only context |

Footer height:
- ≥ 12 rows: 2-line footer.
- 8–11 rows: 1-line footer (status + context only, hints hidden).
- < 8 rows: 0-line footer (full body; hints shown transiently on demand, future work).

## 7. Accessibility & Motion

- Respect existing `tui.reduced_motion` setting: the `working` indicator uses a static dot instead of a spinner when reduced motion is enabled.
- All color choices must maintain ≥ 4.5:1 contrast against the default background `#13161c`.
- Footer colors are themeable so users with color-vision needs can adjust them.

## 8. Testing Plan

1. **Unit tests** in `crates/neo-tui/tests/app_shell.rs`:
   - Assert no top header row is rendered.
   - Assert footer has exactly two rows when height ≥ 12.
   - Assert startup banner is the first transcript item.
   - Assert context color changes at 70% and 90% thresholds.
   - Assert responsive truncation drops segments in priority order.

2. **Theme tests**:
   - Default theme JSON round-trips with new semantic keys.
   - Custom theme overrides footer colors correctly.

3. **Manual smoke**:
   - `cargo run -p neo-agent --` in a large terminal shows banner + two-line footer.
   - Resize to < 60 cols and verify footer collapses.

## 9. Migration & Backward Compatibility

- Existing user themes without the new keys continue to work; missing keys fall back to existing `muted`, `warning`, `danger`, `accent`, and `success` values.
- The `TuiTheme` struct gains new fields; this is a non-breaking additive change for serialized themes.
- No CLI flags change.

## 10. Open Questions / Future Work

1. Should the banner support a `/compact` command to hide it immediately?
2. Should token cost (`$0.004`) be shown in footer line 2 behind a config flag?
3. Should the footer support a user-configurable status line command like Codex’s `/statusline`?
4. Should git branch and change counts be shown in the footer, possibly via a reactive `FooterDataProvider` like Pi uses?

## 11. Summary

Remove Neo’s persistent top header. Replace it with a startup banner that lives in the transcript. Redesign the footer as a two-line, semantic-color status bar:

- **Line 1:** permission badge + working state + cwd + background tasks.
- **Line 2:** keyboard hints + threshold-aware context usage.

This matches the dominant pattern in Claude Code, Codex, Pi, and Kimi Code while keeping Neo’s local-only, minimal identity.
