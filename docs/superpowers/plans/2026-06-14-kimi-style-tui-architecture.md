# Kimi-style Neo TUI Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild Neo's interactive TUI around a Kimi Code/pi-tui-style component tree that preserves finalized transcript history in native terminal scrollback and keeps the active editor/footer live region stable.

**Architecture:** Introduce a small Rust component tree inside `neo-tui`, split terminal output into committed transcript rows plus a bounded live region, and route streaming/tool events into mounted components that update in place. This plan intentionally implements only the pi-tui contracts Neo needs: components, containers, width-aware text rows, render scheduling, scrollback commit, tool-call components, and replay/live parity.

**Tech Stack:** Rust 2024, `neo-tui`, `neo-agent`, `crossterm`, `unicode-width`, existing `neo_agent_core::AgentEvent` / `ToolResult` types. Do not add a runtime dependency on Kimi Code or TypeScript.

**Git policy:** The user explicitly asked not to commit unless asked. Tasks include verification checkpoints instead of `git commit` steps.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/tui/src/core/mod.rs` | Public module for the new component-tree core. |
| `crates/tui/src/core/line.rs` | `Line`, `Span`, style conversion, width-safe rendering helpers. |
| `crates/tui/src/core/text.rs` | `Text` component and hard wrapping for plain/styled lines. |
| `crates/tui/src/core/component.rs` | `Component`, `InputResult`, `Finalization`, `Expandable`. |
| `crates/tui/src/core/container.rs` | `Container` and `GutterContainer` vertical stack components. |
| `crates/tui/src/core/scheduler.rs` | `RenderScheduler`, `RenderKind`, coalesced render requests. |
| `crates/tui/src/core/terminal.rs` | Pure `TerminalRenderer` state model for commit/live-region rendering. |
| `crates/tui/src/transcript/mod.rs` | Public module for transcript component system. |
| `crates/tui/src/transcript/messages.rs` | Banner, user, assistant, thinking, notice components. |
| `crates/tui/src/transcript/tool_call.rs` | `ToolCallComponent` lifecycle and header/body rendering. |
| `crates/tui/src/transcript/tool_renderers.rs` | Tool renderer registry and Read/Write/Edit/Bash renderers. |
| `crates/tui/src/transcript/diff_preview.rs` | Kimi-style clustered Edit diff preview. |
| `crates/tui/src/transcript/controller.rs` | `TranscriptController` finalized-prefix draining. |
| `crates/tui/src/streaming.rs` | `StreamingController` for live AgentEvent/tool lifecycle updates. |
| `crates/tui/src/runtime.rs` | `NeoTuiRuntime` glue around state, containers, scheduler, renderer. |
| `crates/tui/src/lib.rs` | Export new component-tree/runtime modules; do not expose a second interactive renderer. |
| `crates/neo-agent/src/modes/interactive.rs` | Switch draw/event path to `NeoTuiRuntime` as the single interactive renderer. |
| `crates/tui/tests/kimi_core.rs` | Core component/text/container/scheduler tests. |
| `crates/tui/tests/kimi_scrollback.rs` | Transcript commit/live-region scrollback tests. |
| `crates/tui/tests/kimi_tool_cards.rs` | Tool-call lifecycle and renderer parity tests. |
| `crates/tui/tests/kimi_runtime.rs` | Runtime/replay/live integration tests. |
| `docs/gap/tui.md` | Update after implementation to describe the new architecture. |

---

## Implementation Notes for All Tasks

- Temporary additive modules are acceptable during a task, but the completed work must not leave an old TUI path.
- Delete or migrate viewport-sliced interactive rendering after the runtime path is tested; do not leave two transcript renderers for future work.
- Every test command should be run from repository root: `/Users/chenyuanhao/Workspace/neo`.
- For focused TUI work, prefer `cargo test -p neo-tui <test_name>` or `cargo test -p neo-tui --test <file>` before full checks.
- Use `cargo fmt --all --check` before claiming the implementation is ready.
- Do not run `git commit` unless the user explicitly asks.

---

## Task 1: Add Core Row and Component Primitives

**Files:**
- Create: `crates/tui/src/core/mod.rs`
- Create: `crates/tui/src/core/line.rs`
- Create: `crates/tui/src/core/component.rs`
- Modify: `crates/tui/src/lib.rs`
- Test: `crates/tui/tests/kimi_core.rs`

- [ ] **Step 1: Write failing tests for `Line`, `Span`, and component finalization**

Create `crates/tui/tests/kimi_core.rs` with:

```rust
use neo_tui::ansi::{Color, Style};
use neo_tui::core::{Component, Finalization, InputResult, Line, Span};

struct StaticComponent {
    rows: Vec<Line>,
    finalization: Finalization,
}

impl Component for StaticComponent {
    fn render(&mut self, _width: usize) -> Vec<Line> {
        self.rows.clone()
    }

    fn finalization(&self) -> Finalization {
        self.finalization
    }
}

#[test]
fn line_visible_width_ignores_ansi_styles() {
    let line = Line::from_spans(vec![
        Span::styled("hello", Style::default().fg(Color::Green)),
        Span::raw(" 世界"),
    ]);

    assert_eq!(line.visible_width(), 11);
    let ansi = line.to_ansi();
    assert!(ansi.contains("\x1b[32m"));
    assert!(ansi.contains("hello"));
    assert!(ansi.contains("世界"));
}

#[test]
fn line_truncate_preserves_visible_width_contract() {
    let line = Line::raw("abcdef世界");
    let truncated = line.truncate_to_width(8);

    assert_eq!(truncated.visible_width(), 7);
    assert_eq!(neo_tui::ansi::strip_ansi(&truncated.to_ansi()), "abcdef…");
}

#[test]
fn component_defaults_to_live_and_ignored_input() {
    let mut component = StaticComponent {
        rows: vec![Line::raw("ready")],
        finalization: Finalization::Live,
    };

    assert_eq!(component.finalization(), Finalization::Live);
    assert_eq!(component.handle_input(neo_tui::InputEvent::Cancel), InputResult::Ignored);
    assert_eq!(component.render(80), vec![Line::raw("ready")]);
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test -p neo-tui --test kimi_core line_visible_width_ignores_ansi_styles
```

Expected failure:

```text
error[E0432]: unresolved import `neo_tui::core`
```

- [ ] **Step 3: Create `core/mod.rs`**

Create `crates/tui/src/core/mod.rs`:

```rust
pub mod component;
pub mod line;

pub use component::{Component, Expandable, Finalization, InputResult};
pub use line::{Line, Span};
```

- [ ] **Step 4: Create `core/component.rs`**

Create `crates/tui/src/core/component.rs`:

```rust
use crate::InputEvent;

use super::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finalization {
    Live,
    Finalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputResult {
    Ignored,
    Handled,
    Submitted,
    Cancelled,
}

pub trait Component {
    fn render(&mut self, width: usize) -> Vec<Line>;

    fn invalidate(&mut self) {}

    fn finalization(&self) -> Finalization {
        Finalization::Live
    }

    fn handle_input(&mut self, _input: InputEvent) -> InputResult {
        InputResult::Ignored
    }
}

pub trait Expandable {
    fn set_expanded(&mut self, expanded: bool);
}
```

- [ ] **Step 5: Create `core/line.rs`**

Create `crates/tui/src/core/line.rs`:

```rust
use unicode_width::UnicodeWidthChar;

use crate::ansi::{paint, strip_ansi, truncate_to_width, visible_width, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    text: String,
    style: Style,
}

impl Span {
    #[must_use]
    pub fn raw(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }

    #[must_use]
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    #[must_use]
    pub fn to_ansi(&self) -> String {
        paint(&self.text, self.style)
    }

    #[must_use]
    pub fn visible_width(&self) -> usize {
        visible_width(&self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    spans: Vec<Span>,
}

impl Line {
    #[must_use]
    pub fn raw(text: impl Into<String>) -> Self {
        Self {
            spans: vec![Span::raw(text)],
        }
    }

    #[must_use]
    pub fn from_spans(spans: Vec<Span>) -> Self {
        Self { spans }
    }

    #[must_use]
    pub fn to_ansi(&self) -> String {
        self.spans.iter().map(Span::to_ansi).collect()
    }

    #[must_use]
    pub fn visible_width(&self) -> usize {
        self.spans.iter().map(Span::visible_width).sum()
    }

    #[must_use]
    pub fn truncate_to_width(&self, width: usize) -> Self {
        let mut collected = String::new();
        let mut used = 0usize;
        let mut spans = Vec::new();
        for span in &self.spans {
            if used >= width {
                break;
            }
            let plain = strip_ansi(&span.to_ansi());
            for ch in plain.chars() {
                let cw = ch.width().unwrap_or(0);
                if used + cw > width.saturating_sub(1) {
                    if width > 0 {
                        collected.push('…');
                    }
                    if !collected.is_empty() {
                        spans.push(Span::raw(collected));
                    }
                    return Self { spans };
                }
                collected.push(ch);
                used += cw;
            }
        }
        if !collected.is_empty() {
            spans.push(Span::raw(collected));
        }
        Self { spans }
    }
}
```

- [ ] **Step 6: Update `crates/tui/src/lib.rs` to export the new core module**

Replace `crates/tui/src/lib.rs` with a version that adds:

```rust
pub mod core;
pub use core::{Component, Expandable, Finalization, InputResult, Line, Span};
```

while keeping the old exports in place during migration.

- [ ] **Step 7: Run the core test again**

Run:

```bash
cargo test -p neo-tui --test kimi_core line_visible_width_ignores_ansi_styles
```

Expected: the test compiles after the new core module exists.

- [ ] **Step 8: Run the full `neo-tui` test file**

Run:

```bash
cargo test -p neo-tui --test kimi_core
```

Expected: the core primitives tests pass.

---

## Task 2: Add Container and Text Components

**Files:**
- Create: `crates/tui/src/core/container.rs`
- Create: `crates/tui/src/core/text.rs`
- Modify: `crates/tui/src/core/mod.rs`
- Test: `crates/tui/tests/kimi_core.rs`

- [ ] **Step 1: Write failing tests for vertical stacking and width-aware wrapping**

Append to `crates/tui/tests/kimi_core.rs`:

```rust
use neo_tui::core::{Container, GutterContainer, Text};

#[test]
fn container_stacks_children_in_order() {
    let mut container = Container::new();
    container.add_child(Box::new(StaticComponent {
        rows: vec![Line::raw("first")],
        finalization: Finalization::Finalized,
    }));
    container.add_child(Box::new(StaticComponent {
        rows: vec![Line::raw("second")],
        finalization: Finalization::Finalized,
    }));

    let rendered = container.render(80);
    assert_eq!(rendered, vec![Line::raw("first"), Line::raw("second")]);
}

#[test]
fn gutter_container_pads_left_without_trailing_spaces() {
    let mut container = GutterContainer::new(2, 4);
    container.add_child(Box::new(StaticComponent {
        rows: vec![Line::raw("x")],
        finalization: Finalization::Finalized,
    }));

    let rendered = container.render(10);
    assert_eq!(rendered, vec![Line::raw("  x")]);
}

#[test]
fn text_wraps_by_visible_width() {
    let text = Text::new("hello world 世界");
    let rendered = text.render(8);

    assert!(rendered.iter().all(|line| line.visible_width() <= 8));
    assert_eq!(rendered[0], Line::raw("hello"));
}
```

- [ ] **Step 2: Add `core/container.rs`**

Create `crates/tui/src/core/container.rs`:

```rust
use super::{Component, Finalization, Line};

pub struct Container {
    children: Vec<Box<dyn Component>>,
}

impl Container {
    #[must_use]
    pub fn new() -> Self {
        Self { children: Vec::new() }
    }

    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }

    #[must_use]
    pub fn children(&self) -> &[Box<dyn Component>] {
        &self.children
    }
}

impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let mut rows = Vec::new();
        for child in &mut self.children {
            rows.extend(child.render(width));
        }
        rows
    }

    fn finalization(&self) -> Finalization {
        if self.children.iter().all(|child| child.finalization() == Finalization::Finalized) {
            Finalization::Finalized
        } else {
            Finalization::Live
        }
    }
}

pub struct GutterContainer {
    left: usize,
    right: usize,
    inner: Container,
}

impl GutterContainer {
    #[must_use]
    pub fn new(left: usize, right: usize) -> Self {
        Self {
            left,
            right,
            inner: Container::new(),
        }
    }

    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.inner.add_child(child);
    }
}
```

- [ ] **Step 3: Add `core/text.rs`**

Create `crates/tui/src/core/text.rs`:

```rust
use unicode_width::UnicodeWidthChar;

use crate::ansi::Style;

use super::{Component, Finalization, Line, Span};

pub struct Text {
    content: String,
    style: Style,
}

impl Text {
    #[must_use]
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            style: Style::default(),
        }
    }

    #[must_use]
    pub fn styled(content: impl Into<String>, style: Style) -> Self {
        Self {
            content: content.into(),
            style,
        }
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<Line> {
        let mut rows = Vec::new();
        for raw in self.content.split('\n') {
            if raw.is_empty() {
                rows.push(Line::raw(String::new()));
                continue;
            }
            let mut current = String::new();
            let mut current_width = 0usize;
            for ch in raw.chars() {
                let cw = ch.width().unwrap_or(0);
                if current_width + cw > width && !current.is_empty() {
                    rows.push(Line::from_spans(vec![Span::styled(current.clone(), self.style)]));
                    current.clear();
                    current_width = 0;
                }
                current.push(ch);
                current_width += cw;
            }
            if !current.is_empty() {
                rows.push(Line::from_spans(vec![Span::styled(current, self.style)]));
            }
        }
        rows
    }
}

impl Component for Text {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_lines(width)
    }

    fn finalization(&self) -> Finalization {
        Finalization::Finalized
    }
}
```

- [ ] **Step 4: Export the new core modules**

Update `crates/tui/src/core/mod.rs` to add:

```rust
pub mod container;
pub mod text;

pub use container::{Container, GutterContainer};
pub use text::Text;
```

- [ ] **Step 5: Run the new component tests**

Run:

```bash
cargo test -p neo-tui --test kimi_core container_stacks_children_in_order
cargo test -p neo-tui --test kimi_core text_wraps_by_visible_width
```

Expected: both tests fail before the new modules exist, then pass after implementation.

- [ ] **Step 6: Run `cargo fmt --all --check`**

Expected: no formatting regressions before moving on.


---

## Task 3: Add Render Scheduler and Terminal Renderer

**Files:**
- Create: `crates/tui/src/core/scheduler.rs`
- Create: `crates/tui/src/core/terminal.rs`
- Modify: `crates/tui/src/core/mod.rs`
- Test: `crates/tui/tests/kimi_core.rs`

- [ ] **Step 1: Write failing tests for render coalescing and live-region preservation**

Append to `crates/tui/tests/kimi_core.rs`:

```rust
use neo_tui::core::{RenderKind, RenderScheduler, TerminalRenderer};

#[test]
fn scheduler_coalesces_multiple_incremental_requests() {
    let mut scheduler = RenderScheduler::new();
    assert!(!scheduler.is_dirty());

    scheduler.request(RenderKind::Incremental);
    scheduler.request(RenderKind::Incremental);
    assert!(scheduler.is_dirty());
    assert!(!scheduler.requires_full_redraw());

    let kind = scheduler.take_next().expect("pending render kind");
    assert_eq!(kind, RenderKind::Incremental);
    assert!(!scheduler.is_dirty());
}

#[test]
fn scheduler_promotes_force_full_over_incremental() {
    let mut scheduler = RenderScheduler::new();

    scheduler.request(RenderKind::Incremental);
    scheduler.request(RenderKind::ForceFull);

    assert!(scheduler.requires_full_redraw());
    assert_eq!(scheduler.take_next(), Some(RenderKind::ForceFull));
    assert!(!scheduler.requires_full_redraw());
}

#[test]
fn terminal_renderer_keeps_committed_rows_separate_from_live_rows() {
    let mut renderer = TerminalRenderer::new(80, 24);
    renderer.commit_rows(&[Line::raw("banner"), Line::raw("first tool")]);
    renderer.render_live_region(&[Line::raw("> prompt")], None);

    assert_eq!(renderer.committed_rows(), &[Line::raw("banner"), Line::raw("first tool")]);
    assert_eq!(renderer.live_rows(), &[Line::raw("> prompt")]);
}
```

- [ ] **Step 2: Add `core/scheduler.rs`**

Create `crates/tui/src/core/scheduler.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderKind {
    Incremental,
    ForceFull,
}

#[derive(Debug, Default)]
pub struct RenderScheduler {
    dirty: bool,
    force_full: bool,
    pending: Option<RenderKind>,
}

impl RenderScheduler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&mut self, kind: RenderKind) {
        self.dirty = true;
        match (self.pending, kind) {
            (Some(RenderKind::ForceFull), _) => {}
            (_, RenderKind::ForceFull) => {
                self.force_full = true;
                self.pending = Some(RenderKind::ForceFull);
            }
            (None, RenderKind::Incremental) => {
                self.pending = Some(RenderKind::Incremental);
            }
            (Some(RenderKind::Incremental), RenderKind::Incremental) => {}
        }
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    #[must_use]
    pub fn requires_full_redraw(&self) -> bool {
        self.force_full
    }

    pub fn take_next(&mut self) -> Option<RenderKind> {
        self.dirty = false;
        self.force_full = false;
        self.pending.take()
    }
}
```

- [ ] **Step 3: Add `core/terminal.rs`**

Create `crates/tui/src/core/terminal.rs`:

```rust
use crate::renderer::CursorPos;

use super::Line;

#[derive(Debug, Clone)]
pub struct TerminalRenderer {
    width: usize,
    height: usize,
    committed_rows: Vec<Line>,
    live_rows: Vec<Line>,
    cursor: Option<CursorPos>,
}

impl TerminalRenderer {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            committed_rows: Vec::new(),
            live_rows: Vec::new(),
            cursor: None,
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
    }

    pub fn commit_rows(&mut self, rows: &[Line]) {
        self.committed_rows.extend_from_slice(rows);
    }

    pub fn render_live_region(&mut self, rows: &[Line], cursor: Option<CursorPos>) {
        self.live_rows = rows.to_vec();
        self.cursor = cursor;
    }

    #[must_use]
    pub fn committed_rows(&self) -> &[Line] {
        &self.committed_rows
    }

    #[must_use]
    pub fn live_rows(&self) -> &[Line] {
        &self.live_rows
    }

    #[must_use]
    pub fn cursor(&self) -> Option<CursorPos> {
        self.cursor
    }

    #[must_use]
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}
```

- [ ] **Step 4: Export the new scheduler and terminal modules**

Update `crates/tui/src/core/mod.rs`:

```rust
pub mod scheduler;
pub mod terminal;

pub use scheduler::{RenderKind, RenderScheduler};
pub use terminal::TerminalRenderer;
```

- [ ] **Step 5: Run the new scheduler/terminal tests**

Run:

```bash
cargo test -p neo-tui --test kimi_core scheduler_coalesces_multiple_incremental_requests
cargo test -p neo-tui --test kimi_core terminal_renderer_keeps_committed_rows_separate_from_live_rows
```

Expected: fail before implementation, pass after.

- [ ] **Step 6: Run the whole core test file again**

Run:

```bash
cargo test -p neo-tui --test kimi_core
```

Expected: all core tests pass.

## Task 4: Add Transcript Commit and Live-Region Draining

**Files:**
- Create: `crates/tui/src/transcript/mod.rs`
- Create: `crates/tui/src/transcript/controller.rs`
- Create: `crates/tui/src/transcript/messages.rs`
- Modify: `crates/tui/src/lib.rs`
- Test: `crates/tui/tests/kimi_scrollback.rs`

- [ ] **Step 1: Write failing tests for finalized transcript commit**

Create `crates/tui/tests/kimi_scrollback.rs`:

```rust
use neo_tui::core::{Finalization, Line, TerminalRenderer};
use neo_tui::transcript::{TranscriptController, TranscriptEntry};

#[test]
fn finalized_banner_and_user_messages_commit_into_scrollback() {
    let mut controller = TranscriptController::new();
    let mut terminal = TerminalRenderer::new(80, 24);

    controller.push(TranscriptEntry::banner("Welcome to neo"));
    controller.push(TranscriptEntry::user("hello"));

    let committed = controller.drain_finalized_rows(80);
    terminal.commit_rows(&committed);

    assert!(terminal.committed_rows().iter().any(|row| row == &Line::raw("Welcome to neo")));
    assert!(terminal.committed_rows().iter().any(|row| row == &Line::raw("hello")));
}

#[test]
fn live_tool_rows_stay_out_of_committed_scrollback() {
    let mut controller = TranscriptController::new();
    let mut terminal = TerminalRenderer::new(80, 24);

    controller.push(TranscriptEntry::tool_call_running("Read", "crates/tui/src/app.rs"));

    let committed = controller.drain_finalized_rows(80);
    terminal.commit_rows(&committed);

    assert!(terminal.committed_rows().is_empty());
    assert!(!controller.live_entries().is_empty());
}
```

- [ ] **Step 2: Add `transcript/mod.rs`**

Create `crates/tui/src/transcript/mod.rs`:

```rust
pub mod controller;
pub mod messages;

pub use controller::{TranscriptController, TranscriptEntry};
```

- [ ] **Step 3: Add `transcript/controller.rs`**

Create `crates/tui/src/transcript/controller.rs`:

```rust
use crate::core::{Finalization, Line};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    Banner(String),
    User(String),
    Assistant { thinking: Option<String>, content: String, finalized: bool },
    ToolCallRunning { name: String, detail: String },
    ToolCallFinished { name: String, detail: String },
    Notice(String),
}

impl TranscriptEntry {
    #[must_use]
    pub fn banner(title: impl Into<String>) -> Self {
        Self::Banner(title.into())
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::User(content.into())
    }

    #[must_use]
    pub fn assistant_live(content: impl Into<String>) -> Self {
        Self::Assistant { thinking: None, content: content.into(), finalized: false }
    }

    #[must_use]
    pub fn assistant_final(content: impl Into<String>) -> Self {
        Self::Assistant { thinking: None, content: content.into(), finalized: true }
    }

    #[must_use]
    pub fn tool_call_running(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::ToolCallRunning { name: name.into(), detail: detail.into() }
    }

    #[must_use]
    pub fn tool_call_finished(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::ToolCallFinished { name: name.into(), detail: detail.into() }
    }

    #[must_use]
    pub fn finalization(&self) -> Finalization {
        match self {
            Self::Banner(_) | Self::User(_) | Self::Notice(_) | Self::ToolCallFinished { .. } => Finalization::Finalized,
            Self::Assistant { finalized, .. } if *finalized => Finalization::Finalized,
            Self::Assistant { .. } | Self::ToolCallRunning { .. } => Finalization::Live,
        }
    }

    #[must_use]
    pub fn render(&self, _width: usize) -> Vec<Line> {
        match self {
            Self::Banner(title) | Self::User(title) | Self::Notice(title) => vec![Line::raw(title.clone())],
            Self::Assistant { thinking, content, .. } => {
                let mut rows = Vec::new();
                if let Some(thinking) = thinking.as_ref().filter(|value| !value.is_empty()) {
                    rows.push(Line::raw(format!("● {thinking}")));
                }
                if !content.is_empty() {
                    rows.push(Line::raw(content.clone()));
                }
                rows
            }
            Self::ToolCallRunning { name, detail } => vec![Line::raw(format!("● Using {name} ({detail})"))],
            Self::ToolCallFinished { name, detail } => vec![Line::raw(format!("✓ Used {name} ({detail})"))],
        }
    }
}

#[derive(Debug, Default)]
pub struct TranscriptController {
    live: Vec<TranscriptEntry>,
}

impl TranscriptController {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: TranscriptEntry) {
        self.live.push(entry);
    }

    #[must_use]
    pub fn live_entries(&self) -> &[TranscriptEntry] {
        &self.live
    }

    pub fn drain_finalized_rows(&mut self, width: usize) -> Vec<Line> {
        let finalized_count = self
            .live
            .iter()
            .take_while(|entry| entry.finalization() == Finalization::Finalized)
            .count();
        let drained: Vec<TranscriptEntry> = self.live.drain(..finalized_count).collect();
        drained
            .into_iter()
            .flat_map(|entry| entry.render(width))
            .collect()
    }

    #[must_use]
    pub fn render_live_rows(&self, width: usize) -> Vec<Line> {
        self.live
            .iter()
            .flat_map(|entry| entry.render(width))
            .collect()
    }
}
```

- [ ] **Step 4: Export the transcript module**

Update `crates/tui/src/lib.rs` with:

```rust
pub mod transcript;
```

and keep the old exports compiling while the controller is being finished.

- [ ] **Step 5: Run the scrollback tests once the controller exists**

Run:

```bash
cargo test -p neo-tui --test kimi_scrollback
```

Expected: fail until the controller methods are fully implemented.

- [ ] **Step 6: Run `cargo fmt --all --check`**

Expected: formatting remains clean before adding tool cards.


---

## Task 5: Add Kimi-style ToolCallComponent and Tool Renderer Registry

**Files:**
- Create: `crates/tui/src/transcript/tool_call.rs`
- Create: `crates/tui/src/transcript/tool_renderers.rs`
- Modify: `crates/tui/src/transcript/mod.rs`
- Test: `crates/tui/tests/kimi_tool_cards.rs`

- [ ] **Step 1: Write failing tests for tool card lifecycle**

Create `crates/tui/tests/kimi_tool_cards.rs`:

```rust
use neo_tui::ToolStatusKind;
use neo_tui::core::{Component, Expandable, Finalization, Line};
use neo_tui::transcript::{ToolCallComponent, ToolCallState};

fn plain(rows: Vec<Line>) -> Vec<String> {
    rows.into_iter()
        .map(|row| neo_tui::ansi::strip_ansi(&row.to_ansi()))
        .collect()
}

#[test]
fn tool_call_renders_running_header_and_key_arg() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: Some(r#"{"path":"crates/tui/src/app.rs"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("● Using Read (crates/tui/src/app.rs)")));
    assert_eq!(card.finalization(), Finalization::Live);
}

#[test]
fn tool_call_updates_in_place_to_finished_state() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: Some(r#"{"path":"README.md"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    card.set_result(Some("line one\nline two".to_owned()), None, false, None);

    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("✓ Used Read (README.md)")));
    assert!(rows.iter().any(|line| line.contains("2 lines")));
    assert_eq!(card.finalization(), Finalization::Finalized);
}

#[test]
fn ctrl_o_expansion_switches_preview_limit() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(r#"{"command":"printf many"}"#.to_owned()),
        result: Some("1\n2\n3\n4\n5\n6\n7\n8".to_owned()),
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: Some(0),
    });

    let collapsed = plain(card.render(80));
    assert!(collapsed.iter().any(|line| line.contains("more lines")));

    card.set_expanded(true);
    let expanded = plain(card.render(80));
    assert!(expanded.iter().any(|line| line.trim() == "8"));
}
```

- [ ] **Step 2: Add module exports**

Update `crates/tui/src/transcript/mod.rs`:

```rust
pub mod controller;
pub mod messages;
pub mod tool_call;
pub mod tool_renderers;

pub use controller::{TranscriptController, TranscriptEntry};
pub use tool_call::{ToolCallComponent, ToolCallState};
```

- [ ] **Step 3: Add `ToolCallState` and `ToolCallComponent` skeleton**

Create `crates/tui/src/transcript/tool_call.rs`:

```rust
use crate::ToolStatusKind;
use crate::core::{Component, Expandable, Finalization, Line};

use super::tool_renderers::{render_tool_body, tool_header};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallState {
    pub id: String,
    pub name: String,
    pub arguments: Option<String>,
    pub result: Option<String>,
    pub details: Option<serde_json::Value>,
    pub status: ToolStatusKind,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct ToolCallComponent {
    state: ToolCallState,
    expanded: bool,
    progress_lines: Vec<String>,
    live_output: Vec<String>,
}

impl ToolCallComponent {
    #[must_use]
    pub fn new(state: ToolCallState) -> Self {
        Self {
            state,
            expanded: false,
            progress_lines: Vec::new(),
            live_output: Vec::new(),
        }
    }

    pub fn update_call(&mut self, arguments: Option<String>) {
        self.state.arguments = arguments;
    }

    pub fn append_progress(&mut self, line: impl Into<String>) {
        self.progress_lines.push(line.into());
        if self.progress_lines.len() > 24 {
            let extra = self.progress_lines.len() - 24;
            self.progress_lines.drain(..extra);
        }
    }

    pub fn append_live_output(&mut self, output: impl Into<String>) {
        self.live_output.extend(output.into().lines().map(ToOwned::to_owned));
        if self.live_output.len() > 6 {
            let extra = self.live_output.len() - 6;
            self.live_output.drain(..extra);
        }
    }

    pub fn set_result(
        &mut self,
        result: Option<String>,
        details: Option<serde_json::Value>,
        is_error: bool,
        exit_code: Option<i32>,
    ) {
        self.state.result = result;
        self.state.details = details;
        self.state.exit_code = exit_code;
        self.state.status = if is_error {
            ToolStatusKind::Failed
        } else {
            ToolStatusKind::Succeeded
        };
        self.progress_lines.clear();
        self.live_output.clear();
    }
}

impl Expandable for ToolCallComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
}

impl Component for ToolCallComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let mut rows = vec![Line::raw(tool_header(&self.state))];
        rows.extend(render_tool_body(&self.state, self.expanded, width));
        if self.state.status == ToolStatusKind::Running {
            rows.extend(self.progress_lines.iter().map(|line| Line::raw(format!("  {line}"))));
            rows.extend(self.live_output.iter().map(|line| Line::raw(format!("  {line}"))));
        }
        rows
    }

    fn finalization(&self) -> Finalization {
        match self.state.status {
            ToolStatusKind::Succeeded | ToolStatusKind::Failed | ToolStatusKind::Cancelled => {
                Finalization::Finalized
            }
            ToolStatusKind::Pending | ToolStatusKind::Running => Finalization::Live,
        }
    }
}
```

- [ ] **Step 4: Add basic tool renderer registry helpers**

Create `crates/tui/src/transcript/tool_renderers.rs`:

```rust
use crate::ToolStatusKind;
use crate::core::{Line, Text};

use super::tool_call::ToolCallState;

const RESULT_PREVIEW_LINES: usize = 3;
const COMMAND_PREVIEW_LINES: usize = 10;

#[must_use]
pub fn tool_header(state: &ToolCallState) -> String {
    let symbol = match state.status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "●",
        ToolStatusKind::Succeeded => "✓",
        ToolStatusKind::Failed => "✗",
        ToolStatusKind::Cancelled => "⊘",
    };
    let verb = match state.status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "Using",
        ToolStatusKind::Succeeded => "Used",
        ToolStatusKind::Failed => "Failed",
        ToolStatusKind::Cancelled => "Cancelled",
    };
    let key = key_argument(state.arguments.as_deref());
    let chip = result_chip(state);
    if key.is_empty() {
        format!("{symbol} {verb} {}{chip}", state.name)
    } else {
        format!("{symbol} {verb} {} ({key}){chip}", state.name)
    }
}

#[must_use]
pub fn render_tool_body(state: &ToolCallState, expanded: bool, width: usize) -> Vec<Line> {
    let Some(result) = state.result.as_deref().filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    let limit = if expanded { usize::MAX } else { RESULT_PREVIEW_LINES };
    let mut rows = Vec::new();
    let mut rendered = 0usize;
    for line in result.lines() {
        for wrapped in Text::new(line).render(width.saturating_sub(2).max(1)) {
            if rendered >= limit {
                let remaining = result.lines().count().saturating_sub(rendered);
                rows.push(Line::raw(format!("  ... ({remaining} more lines, ctrl+o to expand)")));
                return rows;
            }
            rows.push(Line::raw(format!("  {}", crate::ansi::strip_ansi(&wrapped.to_ansi()))));
            rendered += 1;
        }
    }
    rows
}

fn key_argument(arguments: Option<&str>) -> String {
    let Some(arguments) = arguments.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) {
        for key in ["path", "file_path", "command", "pattern", "query", "url", "description"] {
            if let Some(text) = value.get(key).and_then(|value| value.as_str()) {
                return one_line(text);
            }
        }
    }
    one_line(arguments)
}

fn result_chip(state: &ToolCallState) -> String {
    let Some(result) = state.result.as_deref().filter(|value| !value.is_empty()) else {
        return String::new();
    };
    let lower = state.name.to_lowercase();
    if lower == "read" || lower == "write" {
        return format!(" · {} lines", result.lines().count());
    }
    if lower == "bash" || lower == "shell" {
        if let Some(code) = state.exit_code && code != 0 {
            return format!(" · exit {code}");
        }
    }
    String::new()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
```

- [ ] **Step 5: Run tool card tests**

Run:

```bash
cargo test -p neo-tui --test kimi_tool_cards
```

Expected: the first three tool card lifecycle tests pass.

- [ ] **Step 6: Run focused crate tests**

Run:

```bash
cargo test -p neo-tui --test kimi_core
cargo test -p neo-tui --test kimi_scrollback
cargo test -p neo-tui --test kimi_tool_cards
```

Expected: all new focused tests pass before continuing.


---

## Task 6: Add Clustered Edit Diff Preview

**Files:**
- Create: `crates/tui/src/transcript/diff_preview.rs`
- Modify: `crates/tui/src/transcript/mod.rs`
- Modify: `crates/tui/src/transcript/tool_renderers.rs`
- Test: `crates/tui/tests/kimi_tool_cards.rs`

- [ ] **Step 1: Add failing tests for Kimi-style diff clusters**

Append to `crates/tui/tests/kimi_tool_cards.rs`:

```rust
use neo_tui::transcript::diff_preview::render_diff_lines_clustered;

#[test]
fn edit_diff_preview_clusters_changes_with_context_and_hidden_footer() {
    let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
    let new = "a\nb changed\nc\nd\ne\nf\ng changed\nh\ni\nj\n";

    let rows = render_diff_lines_clustered(old, new, "src/lib.rs", 1, Some(4));
    let plain: Vec<String> = rows
        .into_iter()
        .map(|row| neo_tui::ansi::strip_ansi(&row.to_ansi()))
        .collect();

    assert!(plain[0].contains("+2 -2 src/lib.rs"));
    assert!(plain.iter().any(|line| line.contains("- b")));
    assert!(plain.iter().any(|line| line.contains("+ b changed")));
    assert!(plain.iter().any(|line| line.contains("more changes hidden")));
}

#[test]
fn edit_tool_card_renders_finalized_clustered_diff_from_args() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Edit".to_owned(),
        arguments: Some(serde_json::json!({
            "path": "src/lib.rs",
            "old_string": "old\nline\n",
            "new_string": "new\nline\nextra\n"
        }).to_string()),
        result: Some("edited src/lib.rs".to_owned()),
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("+2 -1 src/lib.rs")));
    assert!(rows.iter().any(|line| line.contains("- old")));
    assert!(rows.iter().any(|line| line.contains("+ new")));
}
```

- [ ] **Step 2: Export `diff_preview`**

Update `crates/tui/src/transcript/mod.rs`:

```rust
pub mod diff_preview;
```

- [ ] **Step 3: Create `transcript/diff_preview.rs`**

Create `crates/tui/src/transcript/diff_preview.rs`:

```rust
use crate::core::Line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    Context,
    Add,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiffLine {
    kind: DiffKind,
    line_num: usize,
    code: String,
}

#[must_use]
pub fn render_diff_lines_clustered(
    old_text: &str,
    new_text: &str,
    path: &str,
    context_lines: usize,
    max_body_lines: Option<usize>,
) -> Vec<Line> {
    let diff = compute_diff_lines(old_text, new_text);
    let added = diff.iter().filter(|line| line.kind == DiffKind::Add).count();
    let removed = diff.iter().filter(|line| line.kind == DiffKind::Delete).count();
    let changed = added + removed;
    let mut rows = vec![Line::raw(format!("+{added} -{removed} {path}"))];
    if changed == 0 {
        return rows;
    }

    let change_indices: Vec<usize> = diff
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.kind != DiffKind::Context).then_some(index))
        .collect();
    let mut emitted = 0usize;
    let mut shown_changes = 0usize;
    let cap = max_body_lines.unwrap_or(usize::MAX);
    let mut previous_end: Option<usize> = None;

    for cluster in build_clusters(&change_indices, diff.len(), context_lines) {
        if emitted >= cap {
            break;
        }
        if let Some(previous_end) = previous_end {
            let gap = cluster.0.saturating_sub(previous_end + 1);
            if gap > 0 && emitted < cap {
                rows.push(Line::raw(format!("     … {gap} unchanged lines …")));
                emitted += 1;
            }
        }
        for index in cluster.0..=cluster.1 {
            if emitted >= cap {
                break;
            }
            let line = &diff[index];
            rows.push(format_diff_row(line));
            emitted += 1;
            if line.kind != DiffKind::Context {
                shown_changes += 1;
            }
        }
        previous_end = Some(cluster.1);
    }

    let hidden = changed.saturating_sub(shown_changes);
    if hidden > 0 {
        rows.push(Line::raw(format!(
            "     … {hidden} more changes hidden (ctrl+o to expand)"
        )));
    }
    rows
}

fn compute_diff_lines(old_text: &str, new_text: &str) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    let mut dp = vec![vec![0usize; new_lines.len() + 1]; old_lines.len() + 1];
    for i in 1..=old_lines.len() {
        for j in 1..=new_lines.len() {
            if old_lines[i - 1] == new_lines[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    let mut reversed = Vec::new();
    let mut i = old_lines.len();
    let mut j = new_lines.len();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_lines[i - 1] == new_lines[j - 1] {
            reversed.push(DiffLine { kind: DiffKind::Context, line_num: j, code: new_lines[j - 1].to_owned() });
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            reversed.push(DiffLine { kind: DiffKind::Add, line_num: j, code: new_lines[j - 1].to_owned() });
            j -= 1;
        } else {
            reversed.push(DiffLine { kind: DiffKind::Delete, line_num: i, code: old_lines[i - 1].to_owned() });
            i -= 1;
        }
    }
    reversed.reverse();
    reversed
}

fn build_clusters(changes: &[usize], len: usize, context: usize) -> Vec<(usize, usize)> {
    let Some((&first, rest)) = changes.split_first() else {
        return Vec::new();
    };
    let mut clusters = Vec::new();
    let mut start = first;
    let mut end = first;
    for &index in rest {
        if index.saturating_sub(end) <= context * 2 {
            end = index;
        } else {
            clusters.push((start.saturating_sub(context), (end + context).min(len - 1)));
            start = index;
            end = index;
        }
    }
    clusters.push((start.saturating_sub(context), (end + context).min(len - 1)));
    clusters
}

fn format_diff_row(line: &DiffLine) -> Line {
    let marker = match line.kind {
        DiffKind::Context => ' ',
        DiffKind::Add => '+',
        DiffKind::Delete => '-',
    };
    Line::raw(format!("{:>4} {marker} {}", line.line_num, line.code))
}
```

- [ ] **Step 4: Route Edit cards to clustered diff renderer**

Modify `crates/tui/src/transcript/tool_renderers.rs` in `render_tool_body` before generic result rendering:

```rust
if state.name.eq_ignore_ascii_case("Edit") {
    if let Some(arguments) = state.arguments.as_deref().and_then(parse_edit_arguments) {
        let max = if expanded { None } else { Some(COMMAND_PREVIEW_LINES) };
        return crate::transcript::diff_preview::render_diff_lines_clustered(
            &arguments.old,
            &arguments.new,
            &arguments.path,
            3,
            max,
        )
        .into_iter()
        .map(|line| Line::raw(format!("  {}", crate::ansi::strip_ansi(&line.to_ansi()))))
        .collect();
    }
}
```

Add helper types/functions in the same file:

```rust
struct EditArguments {
    path: String,
    old: String,
    new: String,
}

fn parse_edit_arguments(arguments: &str) -> Option<EditArguments> {
    let value = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
    let path = value
        .get("path")
        .or_else(|| value.get("file_path"))?
        .as_str()?
        .to_owned();
    let old = value
        .get("old_string")
        .or_else(|| value.get("old"))?
        .as_str()?
        .to_owned();
    let new = value
        .get("new_string")
        .or_else(|| value.get("new"))?
        .as_str()?
        .to_owned();
    Some(EditArguments { path, old, new })
}
```

- [ ] **Step 5: Run the diff preview tests**

Run:

```bash
cargo test -p neo-tui --test kimi_tool_cards edit_diff_preview_clusters_changes_with_context_and_hidden_footer
cargo test -p neo-tui --test kimi_tool_cards edit_tool_card_renders_finalized_clustered_diff_from_args
```

Expected: both pass.

---

## Task 7: Add Write Preview and Bash Live Tail Behavior

**Files:**
- Modify: `crates/tui/src/transcript/tool_renderers.rs`
- Modify: `crates/tui/src/transcript/tool_call.rs`
- Test: `crates/tui/tests/kimi_tool_cards.rs`

- [ ] **Step 1: Add failing tests for Write cap and Bash live tail**

Append to `crates/tui/tests/kimi_tool_cards.rs`:

```rust
#[test]
fn write_tool_card_caps_finalized_content_preview() {
    let content = (1..=20).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Write".to_owned(),
        arguments: Some(serde_json::json!({
            "path": "src/generated.rs",
            "content": content,
        }).to_string()),
        result: Some("wrote src/generated.rs".to_owned()),
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("src/generated.rs · 20 lines")));
    assert!(rows.iter().any(|line| line.contains("ctrl+o to expand")));
    assert!(!rows.iter().any(|line| line.contains("line 20")));

    card.set_expanded(true);
    let expanded = plain(card.render(80));
    assert!(expanded.iter().any(|line| line.contains("line 20")));
}

#[test]
fn bash_running_card_shows_live_output_tail() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(r#"{"command":"cargo test"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    for n in 1..=10 {
        card.append_live_output(format!("line {n}"));
    }

    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("line 10")));
    assert!(!rows.iter().any(|line| line.contains("line 1")));
}
```

- [ ] **Step 2: Add Write renderer branch**

In `crates/tui/src/transcript/tool_renderers.rs`, add this branch before generic result rendering:

```rust
if state.name.eq_ignore_ascii_case("Write") {
    if let Some((path, content)) = parse_write_arguments(state.arguments.as_deref()) {
        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let limit = if expanded { total } else { COMMAND_PREVIEW_LINES.min(total) };
        let mut rows = vec![Line::raw(format!("  {path} · {total} lines"))];
        for (index, line) in lines.iter().take(limit).enumerate() {
            rows.push(Line::raw(format!("  {:>4} {line}", index + 1)));
        }
        if limit < total {
            rows.push(Line::raw(format!(
                "  ... ({} more lines, {total} total, ctrl+o to expand)",
                total - limit
            )));
        }
        return rows;
    }
}
```

Add helper:

```rust
fn parse_write_arguments(arguments: Option<&str>) -> Option<(String, String)> {
    let value = serde_json::from_str::<serde_json::Value>(arguments?).ok()?;
    let path = value
        .get("path")
        .or_else(|| value.get("file_path"))?
        .as_str()?
        .to_owned();
    let content = value.get("content")?.as_str()?.to_owned();
    Some((path, content))
}
```

- [ ] **Step 3: Add earlier-output hint to Bash live tail**

In `ToolCallComponent::append_live_output`, track dropped lines with a counter:

```rust
// Add field to ToolCallComponent:
dropped_live_output_lines: usize,
```

Initialize it in `new()`:

```rust
dropped_live_output_lines: 0,
```

Update `append_live_output`:

```rust
pub fn append_live_output(&mut self, output: impl Into<String>) {
    self.live_output.extend(output.into().lines().map(ToOwned::to_owned));
    if self.live_output.len() > 6 {
        let extra = self.live_output.len() - 6;
        self.dropped_live_output_lines += extra;
        self.live_output.drain(..extra);
    }
}
```

Update `render()` before rendering `live_output`:

```rust
if self.dropped_live_output_lines > 0 && self.state.status == ToolStatusKind::Running {
    rows.push(Line::raw(format!(
        "  ... ({} earlier lines)",
        self.dropped_live_output_lines
    )));
}
```

- [ ] **Step 4: Clear live output after final result**

`set_result()` already clears `live_output`; add:

```rust
self.dropped_live_output_lines = 0;
```

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p neo-tui --test kimi_tool_cards write_tool_card_caps_finalized_content_preview
cargo test -p neo-tui --test kimi_tool_cards bash_running_card_shows_live_output_tail
```

Expected: both pass.

- [ ] **Step 6: Run all tool-card tests**

Run:

```bash
cargo test -p neo-tui --test kimi_tool_cards
```

Expected: all tool-card tests pass.


---

## Task 8: Add StreamingController for In-place Event Updates

**Files:**
- Create: `crates/tui/src/streaming.rs`
- Modify: `crates/tui/src/lib.rs`
- Test: `crates/tui/tests/kimi_runtime.rs`

- [ ] **Step 1: Write failing tests for event lifecycle updates**

Create `crates/tui/tests/kimi_runtime.rs`:

```rust
use neo_agent_core::{AgentEvent, ToolResult};
use neo_tui::ToolStatusKind;
use neo_tui::core::Finalization;
use neo_tui::streaming::StreamingController;

#[test]
fn streaming_controller_updates_one_tool_card_in_place() {
    let mut controller = StreamingController::new();

    controller.apply_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
    });
    controller.apply_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"README.md"}"#.to_owned(),
    });
    controller.apply_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: ToolResult::ok("line one\nline two"),
    });

    assert_eq!(controller.tool_count(), 1);
    let card = controller.tool("tool-1").expect("tool card exists");
    assert_eq!(card.finalization(), Finalization::Finalized);
}

#[test]
fn streaming_controller_keeps_running_tool_live() {
    let mut controller = StreamingController::new();

    controller.apply_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
    });

    let card = controller.tool("tool-1").expect("tool card exists");
    assert_eq!(card.status(), ToolStatusKind::Running);
    assert_eq!(card.finalization(), Finalization::Live);
}
```

If the exact `AgentEvent` field names differ, adjust the test to the current definitions in `crates/agent-core/src/events.rs` before implementation. Keep the test's semantic assertions: one card per id, running is live, final result is finalized.

- [ ] **Step 2: Create `streaming.rs`**

Create `crates/tui/src/streaming.rs`:

```rust
use std::collections::BTreeMap;

use neo_agent_core::{AgentEvent, ToolResult};

use crate::ToolStatusKind;
use crate::transcript::{ToolCallComponent, ToolCallState};

#[derive(Debug, Default)]
pub struct StreamingController {
    tools: BTreeMap<String, ToolCallComponent>,
    streaming_args: BTreeMap<String, String>,
}

impl StreamingController {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::ToolCallStarted { id, name, .. } => {
                self.ensure_tool(id, name, None);
            }
            AgentEvent::ToolCallArgumentsDelta { id, json_fragment, .. } => {
                let args = self.streaming_args.entry(id.clone()).or_default();
                args.push_str(&json_fragment);
                if let Some(tool) = self.tools.get_mut(&id) {
                    tool.update_call(Some(args.clone()));
                }
            }
            AgentEvent::ToolExecutionStarted { id, name, arguments, .. } => {
                self.ensure_tool(id, name, Some(arguments.to_string()));
            }
            AgentEvent::ToolExecutionUpdate { id, partial_result, .. } => {
                if let Some(tool) = self.tools.get_mut(&id) {
                    tool.append_progress(partial_result.content);
                }
            }
            AgentEvent::ToolExecutionFinished { id, name, result, .. } => {
                self.finish_tool(id, name, result);
            }
            _ => {}
        }
    }

    fn ensure_tool(&mut self, id: String, name: String, arguments: Option<String>) {
        self.tools.entry(id.clone()).or_insert_with(|| {
            ToolCallComponent::new(ToolCallState {
                id,
                name,
                arguments,
                result: None,
                details: None,
                status: ToolStatusKind::Running,
                exit_code: None,
            })
        });
    }

    fn finish_tool(&mut self, id: String, name: String, result: ToolResult) {
        let tool = self.tools.entry(id.clone()).or_insert_with(|| {
            ToolCallComponent::new(ToolCallState {
                id,
                name,
                arguments: None,
                result: None,
                details: None,
                status: ToolStatusKind::Running,
                exit_code: None,
            })
        });
        tool.set_result(
            Some(result.content),
            result.details,
            result.is_error,
            None,
        );
    }

    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn tool(&self, id: &str) -> Option<&ToolCallComponent> {
        self.tools.get(id)
    }

    pub fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCallComponent> {
        self.tools.get_mut(id)
    }
}
```

- [ ] **Step 3: Add read-only accessors to `ToolCallComponent`**

In `crates/tui/src/transcript/tool_call.rs`, add:

```rust
impl ToolCallComponent {
    #[must_use]
    pub const fn status(&self) -> ToolStatusKind {
        self.state.status
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.state.id
    }
}
```

- [ ] **Step 4: Export the streaming module**

Update `crates/tui/src/lib.rs`:

```rust
pub mod streaming;
```

- [ ] **Step 5: Run streaming tests**

Run:

```bash
cargo test -p neo-tui --test kimi_runtime streaming_controller_updates_one_tool_card_in_place
cargo test -p neo-tui --test kimi_runtime streaming_controller_keeps_running_tool_live
```

Expected: pass after adjusting any `AgentEvent` constructor names to current source.

---

## Task 9: Add NeoTuiRuntime Commit/Render Glue

**Files:**
- Create: `crates/tui/src/runtime.rs`
- Modify: `crates/tui/src/lib.rs`
- Test: `crates/tui/tests/kimi_runtime.rs`

- [ ] **Step 1: Add failing runtime test for scrollback plus live rows**

Append to `crates/tui/tests/kimi_runtime.rs`:

```rust
use neo_tui::core::{Line, RenderKind};
use neo_tui::runtime::NeoTuiRuntime;
use neo_tui::transcript::TranscriptEntry;

#[test]
fn runtime_commits_finalized_rows_and_keeps_live_region_bounded() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.push_transcript(TranscriptEntry::banner("Welcome to neo"));
    runtime.push_transcript(TranscriptEntry::user("hello"));
    runtime.push_transcript(TranscriptEntry::tool_call_running("Bash", "cargo test"));
    runtime.request_render(RenderKind::Incremental);
    runtime.render_tick();

    assert!(runtime.terminal().committed_rows().contains(&Line::raw("Welcome to neo")));
    assert!(runtime.terminal().committed_rows().contains(&Line::raw("hello")));
    assert!(runtime
        .terminal()
        .live_rows()
        .iter()
        .any(|row| neo_tui::ansi::strip_ansi(&row.to_ansi()).contains("Using Bash")));
}
```

- [ ] **Step 2: Create `runtime.rs`**

Create `crates/tui/src/runtime.rs`:

```rust
use crate::core::{Line, RenderKind, RenderScheduler, TerminalRenderer};
use crate::streaming::StreamingController;
use crate::transcript::{TranscriptController, TranscriptEntry};

#[derive(Debug)]
pub struct NeoTuiRuntime {
    width: usize,
    height: usize,
    transcript: TranscriptController,
    streaming: StreamingController,
    scheduler: RenderScheduler,
    terminal: TerminalRenderer,
}

impl NeoTuiRuntime {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            transcript: TranscriptController::new(),
            streaming: StreamingController::new(),
            scheduler: RenderScheduler::new(),
            terminal: TerminalRenderer::new(width, height),
        }
    }

    pub fn push_transcript(&mut self, entry: TranscriptEntry) {
        self.transcript.push(entry);
        self.request_render(RenderKind::Incremental);
    }

    pub fn request_render(&mut self, kind: RenderKind) {
        self.scheduler.request(kind);
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.terminal.resize(width, height);
        self.scheduler.request(RenderKind::ForceFull);
    }

    pub fn render_tick(&mut self) {
        let Some(_kind) = self.scheduler.take_next() else {
            return;
        };
        let committed = self.transcript.drain_finalized_rows(self.width);
        self.terminal.commit_rows(&committed);

        let mut live = self.transcript.render_live_rows(self.width);
        live.extend(self.render_streaming_rows());
        live = clamp_tail(live, self.height);
        self.terminal.render_live_region(&live, None);
    }

    fn render_streaming_rows(&mut self) -> Vec<Line> {
        // The first runtime slice keeps tool cards testable through StreamingController.
        // Later wiring can move active tool cards directly into TranscriptController.
        Vec::new()
    }

    #[must_use]
    pub const fn terminal(&self) -> &TerminalRenderer {
        &self.terminal
    }

    pub fn streaming_mut(&mut self) -> &mut StreamingController {
        &mut self.streaming
    }
}

fn clamp_tail(mut rows: Vec<Line>, max_rows: usize) -> Vec<Line> {
    if rows.len() > max_rows {
        rows.drain(..rows.len() - max_rows);
    }
    rows
}
```

- [ ] **Step 3: Export runtime module**

Update `crates/tui/src/lib.rs`:

```rust
pub mod runtime;
pub use runtime::NeoTuiRuntime;
```

- [ ] **Step 4: Run runtime test**

Run:

```bash
cargo test -p neo-tui --test kimi_runtime runtime_commits_finalized_rows_and_keeps_live_region_bounded
```

Expected: pass after `TranscriptController` and `TerminalRenderer` are implemented.

- [ ] **Step 5: Run all new tests**

Run:

```bash
cargo test -p neo-tui --test kimi_core
cargo test -p neo-tui --test kimi_scrollback
cargo test -p neo-tui --test kimi_tool_cards
cargo test -p neo-tui --test kimi_runtime
```

Expected: all pass.


---

## Task 10: Wire Runtime into Interactive Terminal Behind a Feature Flag

**Files:**
- Modify: `crates/tui/src/runtime.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/tui/tests/kimi_runtime.rs`

- [ ] **Step 1: Add runtime render output conversion test**

Append to `crates/tui/tests/kimi_runtime.rs`:

```rust
#[test]
fn runtime_exposes_ansi_live_rows_for_terminal_writer() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.push_transcript(TranscriptEntry::tool_call_running("Bash", "cargo test"));
    runtime.render_tick();

    let lines = runtime.live_ansi_lines();
    assert!(lines.iter().any(|line| line.contains("Using Bash")));
}
```

- [ ] **Step 2: Add live ANSI conversion methods**

In `crates/tui/src/runtime.rs`, add:

```rust
impl NeoTuiRuntime {
    #[must_use]
    pub fn live_ansi_lines(&self) -> Vec<String> {
        self.terminal
            .live_rows()
            .iter()
            .map(Line::to_ansi)
            .collect()
    }

    #[must_use]
    pub fn committed_ansi_lines(&self) -> Vec<String> {
        self.terminal
            .committed_rows()
            .iter()
            .map(Line::to_ansi)
            .collect()
    }
}
```

- [ ] **Step 3: Route interactive mode through the runtime only**

In `crates/neo-agent/src/modes/interactive.rs`, add the runtime field to the controller that owns the TUI state and draw through `NeoTuiRuntime` directly. Do not preserve or bridge the old viewport-sliced draw path.

```rust
fn draw_kimi_runtime(runtime: &mut neo_tui::NeoTuiRuntime) -> Result<()> {
    let (cols, rows) = crossterm::terminal::size()?;
    runtime.resize(usize::from(cols), usize::from(rows));
    runtime.render_tick();
    runtime.write_to_terminal()?;
    Ok(())
}
```

This step switches production rendering to the new path; the old draw path should be removed rather than kept around as a second route.

- [ ] **Step 4: Run neo-agent compile check**

Run:

```bash
cargo check -p neo-agent
```

Expected: compiles. If this exposes privacy errors, export only the smallest runtime methods needed by the runtime draw path.

- [ ] **Step 5: Remove old draw-path ownership**

Replace `NeoTerminal::draw()` call sites with the runtime draw path and delete unused bridge/default comments that imply an old-path period. The production binary should use `NeoTuiRuntime` for interactive transcript rendering after this task.

---

## Task 11: Implement Real Native Scrollback Commit Writer

**Files:**
- Modify: `crates/tui/src/core/terminal.rs`
- Modify: `crates/tui/src/renderer.rs` if shared cursor/raw-mode helpers are needed
- Test: `crates/tui/tests/kimi_core.rs`

- [ ] **Step 1: Add pure output-buffer tests for commit writer**

Append to `crates/tui/tests/kimi_core.rs`:

```rust
#[test]
fn terminal_renderer_builds_commit_buffer_without_clear_screen() {
    let renderer = TerminalRenderer::new(80, 24);
    let buffer = renderer.commit_buffer(&[Line::raw("one"), Line::raw("two")]);

    assert!(buffer.contains("one"));
    assert!(buffer.contains("two"));
    assert!(buffer.contains("\r\n"));
    assert!(!buffer.contains("\x1b[2J"));
}

#[test]
fn terminal_renderer_live_buffer_clears_only_live_lines() {
    let mut renderer = TerminalRenderer::new(80, 24);
    renderer.render_live_region(&[Line::raw("old")], None);
    let buffer = renderer.live_region_buffer(&[Line::raw("new")], None);

    assert!(buffer.contains("new"));
    assert!(buffer.contains("\x1b[2K"));
    assert!(!buffer.contains("\x1b[2J"));
}
```

- [ ] **Step 2: Add pure buffer builders**

In `crates/tui/src/core/terminal.rs`, add:

```rust
impl TerminalRenderer {
    #[must_use]
    pub fn commit_buffer(&self, rows: &[Line]) -> String {
        let mut buffer = String::new();
        for row in rows {
            buffer.push_str("\r\n");
            buffer.push_str(&row.to_ansi());
        }
        buffer
    }

    #[must_use]
    pub fn live_region_buffer(&self, rows: &[Line], _cursor: Option<CursorPos>) -> String {
        let mut buffer = String::new();
        buffer.push_str("\x1b[?2026h");
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                buffer.push_str("\r\n");
            }
            buffer.push_str("\x1b[2K");
            buffer.push_str(&row.to_ansi());
        }
        buffer.push_str("\x1b[?2026l");
        buffer
    }
}
```

- [ ] **Step 3: Keep real stdout writing behind a small method**

Add this method but keep tests on pure buffers:

```rust
pub fn write_commit<W: std::io::Write>(&mut self, writer: &mut W, rows: &[Line]) -> std::io::Result<()> {
    let buffer = self.commit_buffer(rows);
    writer.write_all(buffer.as_bytes())?;
    writer.flush()?;
    self.commit_rows(rows);
    Ok(())
}
```

Do not directly call `stdout()` from core tests.

- [ ] **Step 4: Run commit writer tests**

Run:

```bash
cargo test -p neo-tui --test kimi_core terminal_renderer_builds_commit_buffer_without_clear_screen
cargo test -p neo-tui --test kimi_core terminal_renderer_live_buffer_clears_only_live_lines
```

Expected: pass and confirm no clear-screen escape is used for normal commits.

---

## Task 12: Switch Production Draw Path to Runtime Commit + Live Render

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/tui/src/runtime.rs`
- Test: `crates/tui/tests/kimi_runtime.rs`

- [ ] **Step 1: Add runtime method to consume committed rows for writing**

In `crates/tui/src/runtime.rs`, add a method that returns newly committed rows instead of only storing them internally:

```rust
pub struct RenderOutput {
    pub committed: Vec<Line>,
    pub live: Vec<Line>,
}

impl NeoTuiRuntime {
    pub fn render_output(&mut self) -> Option<RenderOutput> {
        self.scheduler.take_next()?;
        let committed = self.transcript.drain_finalized_rows(self.width);
        let mut live = self.transcript.render_live_rows(self.width);
        live.extend(self.render_streaming_rows());
        live = clamp_tail(live, self.height);
        self.terminal.commit_rows(&committed);
        self.terminal.render_live_region(&live, None);
        Some(RenderOutput { committed, live })
    }
}
```

Update `render_tick()` to call `render_output()` and discard the returned rows.

- [ ] **Step 2: Add a test for returned committed rows**

Append to `crates/tui/tests/kimi_runtime.rs`:

```rust
#[test]
fn runtime_render_output_returns_newly_committed_rows_once() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.push_transcript(TranscriptEntry::banner("Welcome to neo"));

    let first = runtime.render_output().expect("first render output");
    assert_eq!(first.committed, vec![Line::raw("Welcome to neo")]);

    runtime.request_render(RenderKind::Incremental);
    let second = runtime.render_output().expect("second render output");
    assert!(second.committed.is_empty());
}
```

- [ ] **Step 3: Update `NeoTerminal` to own terminal writer logic**

In `crates/neo-agent/src/modes/interactive.rs`, extend `NeoTerminal` with a helper:

```rust
fn draw_runtime(&mut self, runtime: &mut neo_tui::NeoTuiRuntime) -> Result<()> {
    let (cols, rows) = size()?;
    runtime.resize(usize::from(cols), usize::from(rows));
    runtime.request_render(neo_tui::core::RenderKind::Incremental);
    if let Some(output) = runtime.render_output() {
        let mut stdout = std::io::stdout();
        // Commit stable history before redrawing the live region.
        let mut terminal = neo_tui::core::TerminalRenderer::new(usize::from(cols), usize::from(rows));
        terminal.write_commit(&mut stdout, &output.committed)?;
        let live_lines = output.live.iter().map(neo_tui::core::Line::to_ansi).collect();
        self.renderer.render(live_lines, None)?;
    }
    Ok(())
}
```

This version may need import adjustments. Keep it small and isolated.

- [ ] **Step 4: Switch the interactive loop only after runtime population exists**

Do not switch calls from `NeoTerminal::draw(&mut NeoTuiApp)` until `NeoTuiRuntime` receives real transcript/tool events. If this task is implemented before Task 13, leave `draw_runtime` unused but compiling.

- [ ] **Step 5: Run compile check**

Run:

```bash
cargo check -p neo-agent
```

Expected: compiles.

---

## Task 13: Map Existing NeoTuiApp Events into NeoTuiRuntime

**Files:**
- Modify: `crates/tui/src/runtime.rs`
- Modify: `crates/tui/src/streaming.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/tui/tests/kimi_runtime.rs`

- [ ] **Step 1: Add runtime event mapping tests**

Append to `crates/tui/tests/kimi_runtime.rs`:

```rust
#[test]
fn runtime_maps_user_and_assistant_events_to_transcript_entries() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.push_user_message("hello");
    runtime.push_assistant_final("world");
    runtime.request_render(RenderKind::Incremental);
    let output = runtime.render_output().expect("render output");

    assert!(output.committed.contains(&Line::raw("hello")));
    assert!(output.committed.contains(&Line::raw("world")));
}
```

- [ ] **Step 2: Add simple runtime transcript APIs**

In `crates/tui/src/runtime.rs`, add:

```rust
impl NeoTuiRuntime {
    pub fn push_user_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::user(content));
    }

    pub fn push_assistant_final(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::assistant_final(content));
    }

    pub fn push_banner(&mut self, title: impl Into<String>) {
        self.push_transcript(TranscriptEntry::banner(title));
    }
}
```

- [ ] **Step 3: Add `apply_agent_event` passthrough for tools**

In `crates/tui/src/runtime.rs`, add:

```rust
impl NeoTuiRuntime {
    pub fn apply_agent_event(&mut self, event: neo_agent_core::AgentEvent) {
        self.streaming.apply_event(event);
        self.request_render(RenderKind::Incremental);
    }
}
```

This is intentionally narrow: assistant text can continue using existing `NeoTuiApp` until the runtime owns full transcript streaming.

- [ ] **Step 4: Add migration wiring in `interactive.rs` without deleting old app state**

In the interactive controller struct, add:

```rust
kimi_runtime: Option<neo_tui::NeoTuiRuntime>,
```

Initialize it after terminal size is known or lazily on first draw:

```rust
let (cols, rows) = size().unwrap_or((80, 24));
self.kimi_runtime.get_or_insert_with(|| {
    neo_tui::NeoTuiRuntime::new(usize::from(cols), usize::from(rows))
});
```

- [ ] **Step 5: Mirror user prompt submission into runtime**

Where `submit_prompt()` returns `Some(submitted)`, add:

```rust
if let Some(runtime) = &mut self.kimi_runtime {
    runtime.push_user_message(submitted.clone());
}
```

Keep the old `NeoTuiApp` call until full migration is complete.

- [ ] **Step 6: Mirror tool AgentEvents into runtime**

Where `self.app.apply_agent_event(event)` is called during event draining, clone tool lifecycle events into runtime:

```rust
if let Some(runtime) = &mut self.kimi_runtime {
    runtime.apply_agent_event(event.clone());
}
self.app.apply_agent_event(event);
```

If `AgentEvent` is not `Clone`, change the event drain code to pass references or branch before moving the event. Do not add broad cloning to large payloads without checking current type definitions.

- [ ] **Step 7: Compile check**

Run:

```bash
cargo check -p neo-agent
```

Expected: compiles with both old and new TUI state present.


---

## Task 14: Complete Runtime Ownership of Transcript Rendering

**Files:**
- Modify: `crates/tui/src/runtime.rs`
- Modify: `crates/tui/src/streaming.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/tui/tests/kimi_runtime.rs`

- [ ] **Step 1: Add test for assistant streaming finalization**

Append to `crates/tui/tests/kimi_runtime.rs`:

```rust
#[test]
fn runtime_keeps_streaming_assistant_live_until_finalized() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.start_assistant_message();
    runtime.append_assistant_delta("hello");
    runtime.request_render(RenderKind::Incremental);
    let first = runtime.render_output().expect("first output");
    assert!(first.committed.is_empty());
    assert!(first.live.iter().any(|row| row == &Line::raw("hello")));

    runtime.finish_assistant_message();
    runtime.request_render(RenderKind::Incremental);
    let second = runtime.render_output().expect("second output");
    assert!(second.committed.contains(&Line::raw("hello")));
}
```

- [ ] **Step 2: Add assistant streaming fields to runtime**

In `crates/tui/src/runtime.rs`, add fields:

```rust
active_assistant: Option<String>,
```

Initialize in `new()`:

```rust
active_assistant: None,
```

- [ ] **Step 3: Add assistant lifecycle methods**

In `crates/tui/src/runtime.rs`, add:

```rust
impl NeoTuiRuntime {
    pub fn start_assistant_message(&mut self) {
        self.active_assistant = Some(String::new());
        self.request_render(RenderKind::Incremental);
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        self.active_assistant.get_or_insert_with(String::new).push_str(text);
        self.request_render(RenderKind::Incremental);
    }

    pub fn finish_assistant_message(&mut self) {
        if let Some(content) = self.active_assistant.take() {
            self.transcript.push(TranscriptEntry::assistant_final(content));
        }
        self.request_render(RenderKind::Incremental);
    }
}
```

- [ ] **Step 4: Include active assistant in live render**

In `NeoTuiRuntime::render_output`, before clamping live rows, append active assistant content:

```rust
if let Some(content) = &self.active_assistant {
    if !content.is_empty() {
        live.push(Line::raw(content.clone()));
    }
}
```

- [ ] **Step 5: Map assistant AgentEvents in `interactive.rs`**

When handling events:

```rust
match &event {
    neo_agent_core::AgentEvent::MessageStarted { .. } => {
        if let Some(runtime) = &mut self.kimi_runtime {
            runtime.start_assistant_message();
        }
    }
    neo_agent_core::AgentEvent::TextDelta { text, .. } => {
        if let Some(runtime) = &mut self.kimi_runtime {
            runtime.append_assistant_delta(text);
        }
    }
    neo_agent_core::AgentEvent::MessageFinished { .. } => {
        if let Some(runtime) = &mut self.kimi_runtime {
            runtime.finish_assistant_message();
        }
    }
    _ => {}
}
```

Keep existing app behavior until the final switch is made.

- [ ] **Step 6: Run runtime tests**

Run:

```bash
cargo test -p neo-tui --test kimi_runtime runtime_keeps_streaming_assistant_live_until_finalized
```

Expected: pass.

---

## Task 15: Replace Default Draw Path with Kimi-style Runtime

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/tui/tests/kimi_runtime.rs`

- [ ] **Step 1: Add a manual smoke checklist to code comments**

Near the switch in `interactive.rs`, add this comment:

```rust
// Manual smoke after switching default draw path:
// 1. Run `cargo run -p neo-agent --`.
// 2. Confirm welcome banner remains visible in terminal scrollback after long output.
// 3. Ask for a project summary that triggers several Read/Grep calls.
// 4. Confirm tool cards update in place and old cards remain in native scrollback.
// 5. Confirm editor/footer stay pinned at the bottom.
```

- [ ] **Step 2: Replace the draw call after runtime mirrors user/assistant/tool state**

In the main terminal loop, replace the viewport-sliced draw path:

```rust
terminal.draw(&mut self.app)?;
```

with the runtime draw path:

```rust
terminal.draw_runtime(&mut self.kimi_runtime)?;
```

Do not retain an `else` branch back to `terminal.draw(&mut self.app)`: interactive mode must have one renderer path after this switch.

- [ ] **Step 3: Add runtime initialization before first draw**

Before the first render call in `run_terminal_loop_with_suspend`, ensure runtime exists:

```rust
self.ensure_kimi_runtime();
```

Implement:

```rust
fn ensure_kimi_runtime(&mut self) {
    if self.kimi_runtime.is_some() {
        return;
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut runtime = neo_tui::NeoTuiRuntime::new(usize::from(cols), usize::from(rows));
    runtime.push_banner(format!("Welcome to {}", self.app.title()));
    self.kimi_runtime = Some(runtime);
}
```

If `NeoTuiApp::title()` is not public, add a minimal getter in `crates/tui/src/app.rs`:

```rust
#[must_use]
pub fn title(&self) -> &str {
    &self.title
}
```

- [ ] **Step 4: Compile**

Run:

```bash
cargo check -p neo-agent
```

Expected: compiles.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p neo-tui --test kimi_runtime
```

Expected: runtime tests pass.

- [ ] **Step 6: Manual smoke run**

Run:

```bash
cargo run -p neo-agent -- --help
```

Expected: CLI help still works. Do not run a live model call as an automated test.

---

## Task 16: Add Ctrl+O Global Expansion to Runtime Components

**Files:**
- Modify: `crates/tui/src/runtime.rs`
- Modify: `crates/tui/src/transcript/controller.rs`
- Modify: `crates/tui/src/transcript/tool_call.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/tui/tests/kimi_tool_cards.rs`

- [ ] **Step 1: Add expansion test**

Append to `crates/tui/tests/kimi_tool_cards.rs`:

```rust
#[test]
fn runtime_global_expansion_affects_future_tool_cards() {
    let mut runtime = neo_tui::NeoTuiRuntime::new(80, 12);
    runtime.set_tool_output_expanded(true);
    assert!(runtime.tool_output_expanded());
}
```

- [ ] **Step 2: Add runtime expansion state**

In `crates/tui/src/runtime.rs`, add field:

```rust
tool_output_expanded: bool,
```

Initialize:

```rust
tool_output_expanded: false,
```

Add methods:

```rust
impl NeoTuiRuntime {
    pub fn set_tool_output_expanded(&mut self, expanded: bool) {
        self.tool_output_expanded = expanded;
        self.request_render(RenderKind::Incremental);
    }

    pub fn toggle_tool_output_expanded(&mut self) {
        self.set_tool_output_expanded(!self.tool_output_expanded);
    }

    #[must_use]
    pub const fn tool_output_expanded(&self) -> bool {
        self.tool_output_expanded
    }
}
```

- [ ] **Step 3: Wire Ctrl+O input**

In `interactive.rs`, where Ctrl+O currently opens the model picker or toggles expansion depending on mode, map the Kimi runtime path to:

```rust
if let Some(runtime) = &mut self.kimi_runtime {
    runtime.toggle_tool_output_expanded();
    return Ok(false);
}
```

If Ctrl+O is already reserved for model picker, preserve existing `/model` or keybinding behavior and choose the same expansion key Kimi Code uses only if it does not conflict with Neo's current keybindings. Document the final key in `docs/gap/tui.md`.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p neo-tui --test kimi_tool_cards runtime_global_expansion_affects_future_tool_cards
cargo check -p neo-agent
```

Expected: pass/compile.

---

## Task 17: Replay Uses the Same Runtime Path

**Files:**
- Modify: `crates/tui/src/runtime.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/tui/tests/kimi_runtime.rs`

- [ ] **Step 1: Add replay parity test**

Append to `crates/tui/tests/kimi_runtime.rs`:

```rust
#[test]
fn replayed_messages_commit_through_same_runtime_path() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.replay_user_message("previous prompt");
    runtime.replay_assistant_message("previous answer");
    runtime.request_render(RenderKind::Incremental);

    let output = runtime.render_output().expect("render output");
    assert!(output.committed.contains(&Line::raw("previous prompt")));
    assert!(output.committed.contains(&Line::raw("previous answer")));
}
```

- [ ] **Step 2: Add replay methods**

In `crates/tui/src/runtime.rs`, add:

```rust
impl NeoTuiRuntime {
    pub fn replay_user_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::user(content));
    }

    pub fn replay_assistant_message(&mut self, content: impl Into<String>) {
        self.push_transcript(TranscriptEntry::assistant_final(content));
    }
}
```

- [ ] **Step 3: Use replay methods during session load**

In `interactive.rs`, locate session transcript loading. For each replayed user/assistant message, call the runtime replay methods in parallel with the existing `NeoTuiApp::load_session_transcript` path.

Example pattern:

```rust
if let Some(runtime) = &mut self.kimi_runtime {
    match message.role.as_str() {
        "user" => runtime.replay_user_message(rendered_content.clone()),
        "assistant" => runtime.replay_assistant_message(rendered_content.clone()),
        _ => {}
    }
}
```

Use the existing message-to-display conversion already used by `NeoTuiApp` to avoid inventing a second transcript parser.

- [ ] **Step 4: Run replay tests**

Run:

```bash
cargo test -p neo-tui --test kimi_runtime replayed_messages_commit_through_same_runtime_path
cargo check -p neo-agent
```

Expected: pass/compile.


---

## Task 18: Remove the Old Viewport-Sliced Interactive Draw Path

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/tui/src/lib.rs`
- Modify: `crates/tui/src/app.rs`
- Modify: `crates/tui/src/app_renderer.rs` if it still exists
- Test: shell assertions plus existing Rust tests

- [ ] **Step 1: Verify the one-screen renderer is no longer used by interactive mode**

Run:

```bash
grep -R "render_app_lines" -n crates/neo-agent/src crates/tui/src \
  | grep -v "fn render_app_lines" \
  | grep -v "mod app_renderer" || true
```

Expected after Task 15 is complete:

```text
# no output
```

If the command prints a call or public export, remove it or migrate the caller to the `NeoTuiRuntime::render_output()` / terminal writer path from Task 15 before continuing.

- [ ] **Step 2: Delete or migrate `app_renderer.rs`**

Remove `crates/tui/src/app_renderer.rs` if no non-interactive caller remains. If a small helper is still needed, move that helper into the new component/runtime modules and delete the full-frame transcript renderer from the public surface. Do not keep it as a second renderer, fixture renderer, or comparison path.

- [ ] **Step 3: Remove dead scrollback ownership from `NeoTuiApp` after runtime owns commits**

In `crates/tui/src/app.rs`, remove these fields and methods if no caller remains:

```rust
committed_count: usize,
```

```rust
pub fn drain_newly_committed(&mut self) -> Vec<TranscriptItem> {
    let start = self.committed_count.min(self.transcript.len());
    let drained = self.transcript[start..].to_vec();
    self.committed_count = self.transcript.len();
    drained
}

pub fn live_transcript_items(&self) -> &[TranscriptItem] {
    let start = self.committed_count.min(self.transcript.len());
    &self.transcript[start..]
}
```

If either method is still used by a non-interactive test, update that test to use `TranscriptController::drain_finalized_rows()` and `NeoTuiRuntime::live_rows_for_test()` instead.

- [ ] **Step 4: Remove any remaining `render_app_lines` export**

If `crates/tui/src/lib.rs` still exports `render_app_lines`, remove the export and migrate downstream callers to `NeoTuiRuntime`. The crate should not advertise a second transcript renderer.

- [ ] **Step 5: Search for stale `committed_count` and old live-transcript plumbing**

Run:

```bash
grep -R "committed_count\|drain_newly_committed\|live_transcript_items" -n crates/tui/src crates/neo-agent/src || true
```

Expected:

```text
# no output
```

If there is output in tests only, migrate those tests to `TranscriptController::drain_finalized_rows()` and `NeoTuiRuntime::live_rows_for_test()` rather than preserving renderer fixtures.

- [ ] **Step 6: Run cleanup verification**

Run:

```bash
cargo test -p neo-tui --test kimi_core
cargo test -p neo-tui --test kimi_scrollback
cargo test -p neo-tui --test kimi_tool_cards
cargo test -p neo-tui --test kimi_runtime
cargo check -p neo-agent
```

Expected: all commands pass. Do not continue to documentation until this cleanup compiles.

---

## Task 19: Update TUI Gap Documentation and Architecture Notes

**Files:**
- Modify: `docs/gap/tui.md`
- Modify: `docs/superpowers/specs/2026-06-14-kimi-style-tui-architecture-design.md`
- Test: documentation grep checks

- [ ] **Step 1: Read the existing TUI gap page**

Run:

```bash
sed -n '1,220p' docs/gap/tui.md
```

Use the existing heading style. Do not rewrite unrelated gap claims.

- [ ] **Step 2: Add the new architecture status block**

Insert this block under the first status/overview section in `docs/gap/tui.md`:

```markdown
### Kimi-style interactive renderer

Neo's interactive TUI uses a component-tree runtime inspired by Kimi Code/pi-tui:

- finalized transcript rows are committed to native terminal scrollback;
- active assistant/tool/editor/footer rows stay in a bounded live region;
- tool calls are mounted components that update in place while running and finalize into transcript history;
- Edit previews use clustered diffs;
- Write previews are capped so large files do not resize the live region unpredictably;
- Bash output keeps a bounded live tail and stores the final output in the finished tool card;
- replayed session messages use the same runtime path as live messages.

The viewport-sliced `render_app_lines` path is removed from interactive rendering. Interactive mode uses `NeoTuiRuntime` as the single live TTY renderer so there is no second transcript renderer to maintain.
```

If `docs/gap/tui.md` has a table instead of narrative sections, add this row instead:

```markdown
| Kimi-style interactive renderer | Implemented | `NeoTuiRuntime` commits finalized transcript rows to native scrollback and renders active components in a bounded live region. |
```

- [ ] **Step 3: Document the final expansion key**

Add this paragraph to the same TUI gap page after the architecture block:

```markdown
Tool-card expansion follows Neo's active keybinding table. If `Ctrl+O` is available, it toggles global tool output expansion. If `Ctrl+O` is reserved by the model picker in the current configuration, `/model` remains the model picker path and the expansion binding is documented in the runtime keybinding help.
```

If implementation chose a key other than `Ctrl+O`, replace `Ctrl+O` in the paragraph with the exact key used in `interactive.rs`.

- [ ] **Step 4: Mark the design spec as implemented or partially implemented**

At the top of `docs/superpowers/specs/2026-06-14-kimi-style-tui-architecture-design.md`, below the title, add one of these exact status lines.

Use this if every task in this plan has been completed:

```markdown
> Status: Implemented by `docs/superpowers/plans/2026-06-14-kimi-style-tui-architecture.md`.
```

Do not use a status that leaves interactive mode behind an old-path flag; either complete the migration to `NeoTuiRuntime` or document the implementation as incomplete without promising a second renderer.

- [ ] **Step 5: Run docs consistency checks**

Run:

```bash
grep -R "single live TTY renderer\|no second transcript renderer" -n docs/gap/tui.md docs/superpowers/specs/2026-06-14-kimi-style-tui-architecture-design.md
grep -R "Status: .*2026-06-14-kimi-style-tui-architecture.md" -n docs/superpowers/specs/2026-06-14-kimi-style-tui-architecture-design.md
cargo run -p xtask -- parity
```

Expected:

```text
# first grep prints the no-old-renderer policy lines
# second grep prints the spec status line
# xtask parity exits successfully
```

If `cargo run -p xtask -- parity` reports an intentionally documented fixture, add the existing project convention comment to that fixture line:

```markdown
# xtask-parity: allow documented tui renderer migration note
```

Do not use the allow comment for real broken links or stale source claims.


---

## Task 20: Full Verification and Manual Scrollback Smoke

**Files:**
- No source edits expected unless verification fails
- Test: Rust unit/integration tests, formatting, clippy/check, manual terminal smoke

- [ ] **Step 1: Run all focused Kimi-style TUI tests**

Run:

```bash
cargo test -p neo-tui --test kimi_core
cargo test -p neo-tui --test kimi_scrollback
cargo test -p neo-tui --test kimi_tool_cards
cargo test -p neo-tui --test kimi_runtime
```

Expected: all focused tests pass.

If one test fails, fix the implementation task that introduced the failing behavior. Do not weaken assertions that protect native scrollback, live-region separation, tool-card lifecycle, or replay/live parity.

- [ ] **Step 2: Run existing TUI crate tests**

Run:

```bash
cargo test -p neo-tui
```

Expected: all `neo-tui` tests pass.

If renderer tests fail because output changed intentionally, migrate their expectations to the `NeoTuiRuntime` output shape rather than preserving full-frame renderer fixtures.

- [ ] **Step 3: Run agent compile checks**

Run:

```bash
cargo check -p neo-agent
cargo test -p neo-agent --no-default-features
```

Expected: both commands pass.

If `cargo test -p neo-agent --no-default-features` is not supported by the crate's feature graph, run this instead and record it in the final handoff:

```bash
cargo test -p neo-agent
```

- [ ] **Step 4: Run workspace formatting**

Run:

```bash
cargo fmt --all --check
```

Expected: no diff.

If formatting fails, run:

```bash
cargo fmt --all
cargo fmt --all --check
```

Expected after formatting: no diff.

- [ ] **Step 5: Run the stable maintenance gate**

Run:

```bash
cargo run -p xtask -- check
```

Expected: exits successfully.

Do not use `cargo run -p xtask -- check --workspace` as the only gate unless the whole workspace is already known to be clean; the project notes mention unrelated pre-existing `neo-ai` clippy warnings can exist.

- [ ] **Step 6: Run docs parity after docs updates**

Run:

```bash
cargo run -p xtask -- parity
```

Expected: exits successfully.

If this fails on a stale gap claim introduced by Task 19, fix `docs/gap/tui.md` instead of adding an allow comment.

- [ ] **Step 7: Build the CLI binary**

Run:

```bash
cargo build -p neo-agent
```

Expected: `target/debug/neo` builds successfully.

- [ ] **Step 8: Manual scrollback smoke in a real terminal**

Run in a real terminal from the repository root:

```bash
target/debug/neo
```

Then perform this exact manual script:

1. Wait for the startup banner to render.
2. Send a prompt that causes several read/search/tool events, for example:
   ```text
   inspect README.md, Cargo.toml, and crates/tui/src/lib.rs, then summarize them briefly
   ```
3. Wait until output exceeds one screen.
4. Use the terminal emulator's native scrollback gesture or scrollbar.
5. Verify the startup banner and earlier tool cards are still present above the current live region.
6. While the model is streaming, confirm only the bottom live region updates; committed rows above it should not flicker or disappear.
7. Trigger a tool call with a larger output, for example:
   ```text
   list the public modules exported by crates/tui/src/lib.rs and show the command you used
   ```
8. Toggle tool expansion with the final keybinding documented in Task 19.
9. Verify collapsed cards show a bounded preview and expanded cards show additional output without losing native scrollback.

Expected manual result:

```text
PASS: native scrollback contains finalized banner/user/tool/assistant rows after output exceeds one screen.
PASS: active editor/footer remain stable at the bottom.
PASS: running tool cards update in place and finalized tool cards do not keep repainting.
PASS: replayed session messages use the same visual structure after restarting/resuming.
```

- [ ] **Step 9: Manual replay smoke**

In a real terminal:

```bash
target/debug/neo resume
```

Select a session with multiple prior messages and tool calls. Verify:

```text
PASS: prior transcript appears as committed history, not as one clipped viewport.
PASS: new messages append below replayed history and preserve terminal scrollback.
PASS: editor/footer are visible after replay completes.
```

- [ ] **Step 10: Collect final verification output for handoff**

Record the exact commands run and whether they passed in the final implementation note. Use this format:

```markdown
Verification run:
- `cargo test -p neo-tui --test kimi_core` — pass
- `cargo test -p neo-tui --test kimi_scrollback` — pass
- `cargo test -p neo-tui --test kimi_tool_cards` — pass
- `cargo test -p neo-tui --test kimi_runtime` — pass
- `cargo test -p neo-tui` — pass
- `cargo check -p neo-agent` — pass
- `cargo test -p neo-agent` — pass
- `cargo fmt --all --check` — pass
- `cargo run -p xtask -- check` — pass
- `cargo run -p xtask -- parity` — pass
- manual `target/debug/neo` scrollback smoke — pass
- manual `target/debug/neo resume` replay smoke — pass
```

Do not claim manual smoke passed unless it was actually run in a real terminal.

---

## Task 21: Final Code Review Pass Before Handoff

**Files:**
- Inspect: `crates/tui/src/core/*.rs`
- Inspect: `crates/tui/src/transcript/*.rs`
- Inspect: `crates/tui/src/streaming.rs`
- Inspect: `crates/tui/src/runtime.rs`
- Inspect: `crates/neo-agent/src/modes/interactive.rs`
- Inspect: `docs/gap/tui.md`

- [ ] **Step 1: Search for forbidden incomplete markers in implementation**

Run:

```bash
grep -R "T[B]D\|T[O]DO\|F[I]XME\|implement [l]ater\|st[u]b\|place[h]older" -n \
  crates/tui/src/core \
  crates/tui/src/transcript \
  crates/tui/src/streaming.rs \
  crates/tui/src/runtime.rs \
  crates/neo-agent/src/modes/interactive.rs \
  docs/gap/tui.md || true
```

Expected:

```text
# no output, or only pre-existing comments unrelated to the Kimi-style TUI runtime
```

If output appears in new code, either remove the incomplete marker or replace it with completed behavior. If output appears in old code, do not edit it unless it blocks this feature.

- [ ] **Step 2: Search for accidental TypeScript/Kimi source leakage**

Run:

```bash
grep -R "StreamingUIController\|ProcessTerminal\|pi-tui\|kimi-code/apps" -n crates/tui/src crates/neo-agent/src || true
```

Expected:

```text
# no output, except documentation comments that explicitly say the runtime is inspired by Kimi Code/pi-tui
```

If implementation copied TypeScript names that do not fit Rust conventions, rename them to the Rust names used in this plan: `StreamingController`, `TerminalRenderer`, `NeoTuiRuntime`, `TranscriptController`.

- [ ] **Step 3: Check render ownership boundaries**

Run:

```bash
grep -R "render_live_region\|commit_rows\|render_output" -n crates/tui/src crates/neo-agent/src
```

Verify these boundaries manually:

```text
- `TranscriptController` decides which transcript entries are finalized.
- `NeoTuiRuntime` drains finalized rows and asks `TerminalRenderer` to commit them.
- `TerminalRenderer` stores/writes committed rows separately from live rows.
- `interactive.rs` writes committed rows before drawing the live region.
- `app_renderer.rs` is not used for interactive transcript history.
```

If `interactive.rs` directly filters transcript history by terminal height, remove that filtering and route through `NeoTuiRuntime`.

- [ ] **Step 4: Check capped live output constants**

Run:

```bash
grep -R "MAX_PROGRESS_LINES\|MAX_LIVE_OUTPUT_CHARS\|RESULT_PREVIEW_LINES\|COMMAND_PREVIEW_LINES" -n crates/tui/src/transcript crates/tui/src/streaming.rs
```

Expected constants or equivalent values:

```rust
const RESULT_PREVIEW_LINES: usize = 3;
const COMMAND_PREVIEW_LINES: usize = 10;
const MAX_PROGRESS_LINES: usize = 24;
const MAX_LIVE_OUTPUT_CHARS: usize = 50_000;
```

If names differ, ensure the values and behavior match: running cards must not grow without bound.

- [ ] **Step 5: Check public API exports are intentional**

Run:

```bash
sed -n '1,140p' crates/tui/src/lib.rs
```

Expected export shape:

```rust
pub mod core;
pub mod streaming;
pub mod transcript;
pub mod runtime;

pub use core::{Component, Container, Expandable, Finalization, GutterContainer, InputResult, Line, RenderKind, RenderScheduler, Span, TerminalRenderer, Text};
pub use runtime::{NeoTuiRenderOutput, NeoTuiRuntime};
```

Keep old exports only if existing callers still need them during migration.

- [ ] **Step 6: Confirm no git mutation happened accidentally**

Run:

```bash
git status --short
```

Expected: shows changed files but no commits were created by the agent. Do not run `git commit`.

- [ ] **Step 7: Prepare final implementation summary**

Use this template when reporting implementation completion:

```markdown
Implemented Kimi-style Neo TUI runtime:
- Added component core, transcript controller, tool cards, diff/write/bash renderers, streaming controller, and runtime renderer.
- Switched interactive mode to commit finalized rows into native terminal scrollback and render active rows in a bounded live region.
- Updated docs/gap/tui.md and spec status.

Verification:
<copy exact verification list from Task 20 Step 10>

Notes:
- No git commit was created.
- Manual terminal smoke: <pass/not run + reason>.
```

---

## Plan Self-Review Checklist

Use this checklist before executing the plan or handing it to another agent.

- [ ] **Spec coverage:** Confirm each design-spec requirement maps to a task:
  - Component tree primitives: Tasks 1-3.
  - Native scrollback split: Tasks 3-4, 9, 11-12, 15.
  - Tool cards updating in place: Tasks 5, 8, 13, 15.
  - Edit clustered diff preview: Task 6.
  - Write preview cap and Bash live tail: Task 7.
  - Streaming event ownership: Tasks 8, 13-14.
  - Interactive bridge and production draw path: Tasks 10-12, 15.
  - Ctrl+O/global expansion: Task 16.
  - Replay/live parity: Task 17.
  - Cleanup/docs/verification: Tasks 18-21.

- [ ] **Incomplete-marker scan:** Run this command on the plan itself:

  ```bash
  grep -n "T[B]D\|T[O]DO\|implement [l]ater\|fill in [d]etails\|Similar to [T]ask\|add [a]ppropriate\|handle [e]dge cases" docs/superpowers/plans/2026-06-14-kimi-style-tui-architecture.md || true
  ```

  Expected: no output.

- [ ] **Type consistency:** Check these names are consistent across the plan:
  - `Line`, `Span`, `Component`, `Finalization`, `InputResult`, `Expandable`.
  - `Container`, `GutterContainer`, `Text`.
  - `RenderKind`, `RenderScheduler`, `TerminalRenderer`.
  - `TranscriptController`, `TranscriptEntry`.
  - `ToolCallState`, `ToolCallComponent`.
  - `StreamingController`.
  - `NeoTuiRuntime`, `NeoTuiRenderOutput`.

- [ ] **Command consistency:** All commands run from repository root `/Users/chenyuanhao/Workspace/neo`.

- [ ] **Git policy:** The plan intentionally has no `git commit` steps because the user explicitly said not to commit unless asked.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-14-kimi-style-tui-architecture.md`.

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh implementation subagent per task or per small task group, review between tasks, and keep the main session focused on integration decisions.
2. **Inline Execution** — execute tasks in this session using `superpowers:executing-plans`, with checkpoints after each major group.

Because the user already approved the architecture and asked not to pause while writing the plan, the next agent can begin with Task 1 directly. Do not run `git commit` unless the user explicitly asks.
