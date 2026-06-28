# `/tasks` Task Browser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the human-facing `/tasks` transcript status output with a full-screen Kimi-style Task Browser that defaults to `ALL`, keeps the left Tasks pane full-height, and preserves model-facing `TaskList` output unchanged.

**Architecture:** Add a pure `neo-tui` task-browser module for state, view models, input handling, and rendering. Integrate it as a blocking rich overlay in `NeoChromeState`, while `neo-agent` owns async refresh and stop actions through the shared `BackgroundTaskManager`. Keep `TaskList`, `TaskOutput`, and `TaskStop` tool behavior as model APIs; do not build compatibility branches around the old `/tasks` transcript output.

**Tech Stack:** Rust, tokio, `neo-agent-core` background task manager, `neo-agent` interactive controller, `neo-tui` overlay/rendering tests, `cargo nextest` via `cargo run -p xtask -- test`.

**Spec:** `docs/superpowers/specs/2026-06-28-task-browser-design.md`

**Git rule:** Do not run `git add`, `git commit`, `git checkout`, `git restore`, `git reset`, `git stash`, or other git mutations unless the user explicitly authorizes that exact command. The checkpoint steps below describe review points only.

---

## File Structure

### New files

| File | Responsibility |
| --- | --- |
| `crates/neo-tui/src/tasks_browser/mod.rs` | Public exports for the task browser module. |
| `crates/neo-tui/src/tasks_browser/state.rs` | `TaskBrowserState`, filter, focus, stop confirmation, selection, scrolling, and input actions. |
| `crates/neo-tui/src/tasks_browser/view.rs` | UI view models: task item, kind, status, snapshot, and helpers used by renderer/controller tests. |
| `crates/neo-tui/src/tasks_browser/render.rs` | Pure terminal rendering for header, full-height left Tasks pane, right Detail/Preview stack, footer, and narrow layout. |
| `crates/neo-tui/tests/task_browser.rs` | Pure renderer/state tests. |
| `crates/neo-agent/src/modes/task_browser.rs` | Adapter from `BackgroundTaskSnapshot` to `TaskBrowserItem` plus async refresh/stop helpers. |

### Modified files

| File | Responsibility | Key changes |
| --- | --- | --- |
| `crates/neo-tui/src/lib.rs` | Module export | Export `tasks_browser`. |
| `crates/neo-tui/src/shell/overlay.rs` | Overlay variants | Add `OverlayKind::TaskBrowser(TaskBrowserState)` and rendering/height support. |
| `crates/neo-tui/src/shell/mod.rs` | Chrome overlay APIs | Add `push_task_browser_overlay`, `task_browser_state(_mut)`, rich-dialog detection, prompt blocking, and focused input dispatch. |
| `crates/neo-tui/src/app.rs` | Full-screen overlay rendering | Render `TaskBrowser` using terminal content height, not compact overlay height. |
| `crates/neo-agent/src/modes/interactive.rs` | Controller integration | Replace `show_background_tasks()` transcript output with browser open/refresh/stop behavior and route browser input before prompt edits. |
| `crates/neo-agent/src/modes/mod.rs` or `crates/neo-agent/src/modes/interactive.rs` module declarations | Module wiring | Add `task_browser` module import if needed. |
| `crates/neo-agent-core/src/tools/background_tasks.rs` | Tool regression tests only | Keep `TaskList` output unchanged; do not change tool format. |
| `crates/neo-agent-core/tests/tool_bash.rs` | Tool regression | Keep existing `TaskList` model-facing tests passing. |
| `crates/neo-agent/src/modes/interactive.rs` tests | Controller tests | Update old `/tasks` tests to assert browser overlay, not transcript status output. |

---

## Task 1: Pure Task Browser State And View Models

**Files:**
- Create: `crates/neo-tui/src/tasks_browser/mod.rs`
- Create: `crates/neo-tui/src/tasks_browser/state.rs`
- Create: `crates/neo-tui/src/tasks_browser/view.rs`
- Modify: `crates/neo-tui/src/lib.rs`
- Test: `crates/neo-tui/tests/task_browser.rs`

- [ ] **Step 1: Write the failing state tests**

Create `crates/neo-tui/tests/task_browser.rs` with these initial tests:

```rust
use neo_tui::tasks_browser::{
    TaskBrowserAction, TaskBrowserFilter, TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot,
    TaskBrowserState, TaskBrowserStatus,
};

fn item(id: &str, status: TaskBrowserStatus) -> TaskBrowserItem {
    TaskBrowserItem {
        id: id.to_owned(),
        kind: TaskBrowserKind::Bash,
        status,
        title: id.to_owned(),
        description: format!("command for {id}"),
        elapsed: "00:01".to_owned(),
        detail_lines: vec![format!("id:          {id}")],
        preview_lines: vec![format!("output for {id}")],
        can_stop: status.is_active(),
    }
}

#[test]
fn task_browser_defaults_to_all_filter() {
    let state = TaskBrowserState::new();
    assert_eq!(state.filter(), TaskBrowserFilter::All);
    assert!(state.selected_task_id().is_none());
}

#[test]
fn task_browser_tab_toggles_filter() {
    let mut state = TaskBrowserState::new();
    assert_eq!(state.handle_action(TaskBrowserAction::ToggleFilter), None);
    assert_eq!(state.filter(), TaskBrowserFilter::Active);
    assert_eq!(state.handle_action(TaskBrowserAction::ToggleFilter), None);
    assert_eq!(state.filter(), TaskBrowserFilter::All);
}

#[test]
fn task_browser_preserves_selection_by_task_id() {
    let mut state = TaskBrowserState::new();
    let first = TaskBrowserSnapshot::new(vec![
        item("bash-a", TaskBrowserStatus::Running),
        item("bash-b", TaskBrowserStatus::Completed),
    ]);
    state.apply_snapshot(&first);
    state.handle_action(TaskBrowserAction::SelectDown);
    assert_eq!(state.selected_task_id(), Some("bash-b"));

    let refreshed = TaskBrowserSnapshot::new(vec![
        item("bash-b", TaskBrowserStatus::Completed),
        item("bash-c", TaskBrowserStatus::Running),
    ]);
    state.apply_snapshot(&refreshed);
    assert_eq!(state.selected_task_id(), Some("bash-b"));
}

#[test]
fn active_filter_hides_terminal_tasks_and_selects_first_visible() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![
        item("bash-done", TaskBrowserStatus::Completed),
        item("bash-run", TaskBrowserStatus::Running),
    ]));

    state.handle_action(TaskBrowserAction::ToggleFilter);

    assert_eq!(state.filter(), TaskBrowserFilter::Active);
    assert_eq!(state.visible_items().len(), 1);
    assert_eq!(state.selected_task_id(), Some("bash-run"));
}

#[test]
fn stop_confirmation_requires_confirm_or_cancel() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item(
        "bash-run",
        TaskBrowserStatus::Running,
    )]));

    assert_eq!(
        state.handle_action(TaskBrowserAction::RequestStop),
        Some("bash-run".to_owned())
    );
    assert_eq!(state.stop_confirmation_task_id(), Some("bash-run"));
    assert_eq!(
        state.handle_action(TaskBrowserAction::Cancel),
        None,
        "Esc cancels the confirmation before closing the browser"
    );
    assert!(state.stop_confirmation_task_id().is_none());
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui task_browser
```

Expected: compile failure because `neo_tui::tasks_browser` does not exist.

- [ ] **Step 3: Create `mod.rs` exports**

Create `crates/neo-tui/src/tasks_browser/mod.rs`:

```rust
mod render;
mod state;
mod view;

pub use render::TaskBrowserRenderer;
pub use state::{TaskBrowserAction, TaskBrowserFilter, TaskBrowserFocus, TaskBrowserState};
pub use view::{TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot, TaskBrowserStatus};
```

Modify `crates/neo-tui/src/lib.rs` to export the module:

```rust
pub mod tasks_browser;
```

- [ ] **Step 4: Create view models**

Create `crates/neo-tui/src/tasks_browser/view.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserKind {
    Bash,
    Question,
}

impl TaskBrowserKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Question => "question",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserStatus {
    Running,
    Waiting,
    Completed,
    Failed,
    Stopped,
    TimedOut,
}

impl TaskBrowserStatus {
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Waiting)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::TimedOut => "timed out",
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Waiting => "◼",
            Self::Completed => "✓",
            Self::Failed | Self::Stopped | Self::TimedOut => "✕",
        }
    }

    #[must_use]
    pub const fn is_interrupted(self) -> bool {
        matches!(self, Self::Failed | Self::Stopped | Self::TimedOut)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserItem {
    pub id: String,
    pub kind: TaskBrowserKind,
    pub status: TaskBrowserStatus,
    pub title: String,
    pub description: String,
    pub elapsed: String,
    pub detail_lines: Vec<String>,
    pub preview_lines: Vec<String>,
    pub can_stop: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskBrowserSnapshot {
    items: Vec<TaskBrowserItem>,
}

impl TaskBrowserSnapshot {
    #[must_use]
    pub fn new(items: Vec<TaskBrowserItem>) -> Self {
        Self { items }
    }

    #[must_use]
    pub fn items(&self) -> &[TaskBrowserItem] {
        &self.items
    }
}
```

- [ ] **Step 5: Create state and action handling**

Create `crates/neo-tui/src/tasks_browser/state.rs`:

```rust
use super::view::{TaskBrowserItem, TaskBrowserSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserFilter {
    All,
    Active,
}

impl TaskBrowserFilter {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Active => "ACTIVE",
        }
    }

    #[must_use]
    pub const fn pane_label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Active => "active",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserFocus {
    List,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserAction {
    SelectUp,
    SelectDown,
    SelectFirst,
    SelectLast,
    SelectPageUp,
    SelectPageDown,
    ToggleFilter,
    ToggleOutputFocus,
    RequestStop,
    ConfirmStop,
    Refresh,
    Cancel,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserState {
    filter: TaskBrowserFilter,
    snapshot: TaskBrowserSnapshot,
    selected_task_id: Option<String>,
    selected_visible_index: usize,
    list_scroll: usize,
    output_scroll: usize,
    focus: TaskBrowserFocus,
    stop_confirmation_task_id: Option<String>,
    footer_message: Option<String>,
}

impl Default for TaskBrowserState {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskBrowserState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            filter: TaskBrowserFilter::All,
            snapshot: TaskBrowserSnapshot::default(),
            selected_task_id: None,
            selected_visible_index: 0,
            list_scroll: 0,
            output_scroll: 0,
            focus: TaskBrowserFocus::List,
            stop_confirmation_task_id: None,
            footer_message: None,
        }
    }

    #[must_use]
    pub const fn filter(&self) -> TaskBrowserFilter {
        self.filter
    }

    #[must_use]
    pub const fn focus(&self) -> TaskBrowserFocus {
        self.focus
    }

    #[must_use]
    pub fn selected_task_id(&self) -> Option<&str> {
        self.selected_task_id.as_deref()
    }

    #[must_use]
    pub fn stop_confirmation_task_id(&self) -> Option<&str> {
        self.stop_confirmation_task_id.as_deref()
    }

    #[must_use]
    pub fn footer_message(&self) -> Option<&str> {
        self.footer_message.as_deref()
    }

    pub fn set_footer_message(&mut self, message: impl Into<String>) {
        self.footer_message = Some(message.into());
    }

    pub fn clear_footer_message(&mut self) {
        self.footer_message = None;
    }

    #[must_use]
    pub const fn list_scroll(&self) -> usize {
        self.list_scroll
    }

    #[must_use]
    pub const fn output_scroll(&self) -> usize {
        self.output_scroll
    }

    pub fn apply_snapshot(&mut self, snapshot: &TaskBrowserSnapshot) {
        self.snapshot = snapshot.clone();
        self.reconcile_selection();
    }

    #[must_use]
    pub fn snapshot(&self) -> &TaskBrowserSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn visible_items(&self) -> Vec<&TaskBrowserItem> {
        self.snapshot
            .items()
            .iter()
            .filter(|item| self.filter == TaskBrowserFilter::All || item.status.is_active())
            .collect()
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&TaskBrowserItem> {
        let selected = self.selected_task_id.as_deref()?;
        self.snapshot.items().iter().find(|item| item.id == selected)
    }

    pub fn handle_action(&mut self, action: TaskBrowserAction) -> Option<String> {
        match action {
            TaskBrowserAction::SelectUp => self.move_selection(-1),
            TaskBrowserAction::SelectDown => self.move_selection(1),
            TaskBrowserAction::SelectFirst => self.set_selected_visible_index(0),
            TaskBrowserAction::SelectLast => {
                let last = self.visible_items().len().saturating_sub(1);
                self.set_selected_visible_index(last);
            }
            TaskBrowserAction::SelectPageUp => self.move_selection(-10),
            TaskBrowserAction::SelectPageDown => self.move_selection(10),
            TaskBrowserAction::ToggleFilter => {
                self.filter = match self.filter {
                    TaskBrowserFilter::All => TaskBrowserFilter::Active,
                    TaskBrowserFilter::Active => TaskBrowserFilter::All,
                };
                self.reconcile_selection();
            }
            TaskBrowserAction::ToggleOutputFocus => {
                self.focus = match self.focus {
                    TaskBrowserFocus::List => TaskBrowserFocus::Output,
                    TaskBrowserFocus::Output => TaskBrowserFocus::List,
                };
            }
            TaskBrowserAction::RequestStop => {
                if let Some(item) = self.selected_item() {
                    if item.can_stop {
                        let id = item.id.clone();
                        self.stop_confirmation_task_id = Some(id.clone());
                        return Some(id);
                    }
                    self.footer_message = Some("Task already finished.".to_owned());
                }
            }
            TaskBrowserAction::ConfirmStop => {
                return self.stop_confirmation_task_id.take();
            }
            TaskBrowserAction::Cancel => {
                if self.stop_confirmation_task_id.take().is_none() {
                    return Some("__close__".to_owned());
                }
            }
            TaskBrowserAction::Refresh | TaskBrowserAction::Close => {}
        }
        None
    }

    fn reconcile_selection(&mut self) {
        let visible = self.visible_items();
        if visible.is_empty() {
            self.selected_task_id = None;
            self.selected_visible_index = 0;
            self.list_scroll = 0;
            return;
        }
        if let Some(selected) = self.selected_task_id.as_deref()
            && let Some(index) = visible.iter().position(|item| item.id == selected)
        {
            self.selected_visible_index = index;
            return;
        }
        self.selected_visible_index = self.selected_visible_index.min(visible.len() - 1);
        self.selected_task_id = Some(visible[self.selected_visible_index].id.clone());
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible_items().len();
        if len == 0 {
            self.selected_task_id = None;
            self.selected_visible_index = 0;
            return;
        }
        let next = self
            .selected_visible_index
            .saturating_add_signed(delta)
            .min(len - 1);
        self.set_selected_visible_index(next);
    }

    fn set_selected_visible_index(&mut self, index: usize) {
        let visible = self.visible_items();
        if visible.is_empty() {
            self.selected_task_id = None;
            self.selected_visible_index = 0;
            return;
        }
        self.selected_visible_index = index.min(visible.len() - 1);
        self.selected_task_id = Some(visible[self.selected_visible_index].id.clone());
    }
}
```

- [ ] **Step 6: Add a stub renderer to satisfy exports**

Create `crates/neo-tui/src/tasks_browser/render.rs`:

```rust
use crate::shell::TuiTheme;

use super::state::TaskBrowserState;

pub struct TaskBrowserRenderer<'a> {
    state: &'a TaskBrowserState,
    theme: TuiTheme,
}

impl<'a> TaskBrowserRenderer<'a> {
    #[must_use]
    pub const fn new(state: &'a TaskBrowserState, theme: TuiTheme) -> Self {
        Self { state, theme }
    }

    #[must_use]
    pub fn render(&self, width: usize, height: usize) -> Vec<String> {
        let _ = self.state;
        let _ = self.theme;
        vec!["TASK BROWSER".to_owned(); height.min(width.max(1))]
    }
}
```

- [ ] **Step 7: Run the state tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui task_browser
```

Expected: PASS for the state tests. Renderer assertions will be added in Task 2.

- [ ] **Checkpoint**

Review only:

- `crates/neo-tui/src/tasks_browser/mod.rs`
- `crates/neo-tui/src/tasks_browser/state.rs`
- `crates/neo-tui/src/tasks_browser/view.rs`
- `crates/neo-tui/src/tasks_browser/render.rs`
- `crates/neo-tui/src/lib.rs`
- `crates/neo-tui/tests/task_browser.rs`

---

## Task 2: Full-Height Three-Pane Renderer

**Files:**
- Modify: `crates/neo-tui/src/tasks_browser/render.rs`
- Modify: `crates/neo-tui/src/tasks_browser/state.rs`
- Test: `crates/neo-tui/tests/task_browser.rs`

- [ ] **Step 1: Add renderer tests for empty states and full-height left pane**

Append to `crates/neo-tui/tests/task_browser.rs`:

```rust
use neo_tui::primitive::strip_ansi;
use neo_tui::shell::TuiTheme;
use neo_tui::tasks_browser::TaskBrowserRenderer;

fn render_plain(state: &TaskBrowserState, width: usize, height: usize) -> Vec<String> {
    TaskBrowserRenderer::new(state, TuiTheme::default())
        .render(width, height)
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect()
}

#[test]
fn empty_all_renderer_shows_product_empty_state() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(Vec::new()));

    let rendered = render_plain(&state, 120, 18).join("\n");

    assert!(rendered.contains("TASK BROWSER"));
    assert!(rendered.contains("filter=ALL"));
    assert!(rendered.contains("0 total"));
    assert!(rendered.contains("Tasks [all]"));
    assert!(rendered.contains("No background tasks in this"));
    assert!(rendered.contains("session."));
    assert!(rendered.contains("Select a task from the list."));
    assert!(rendered.contains("No task selected."));
    assert!(!rendered.contains("active_background_tasks"));
}

#[test]
fn empty_active_renderer_points_to_all_filter() {
    let mut state = TaskBrowserState::new();
    state.handle_action(TaskBrowserAction::ToggleFilter);
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item(
        "bash-done",
        TaskBrowserStatus::Completed,
    )]));

    let rendered = render_plain(&state, 120, 18).join("\n");

    assert!(rendered.contains("filter=ACTIVE"));
    assert!(rendered.contains("No active tasks. Tab = show all."));
}

#[test]
fn left_tasks_pane_consumes_full_content_height() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(Vec::new()));
    let lines = render_plain(&state, 120, 18);
    let tasks_top = lines
        .iter()
        .position(|line| line.contains("Tasks [all]"))
        .expect("tasks pane top");
    let footer = lines
        .iter()
        .position(|line| line.contains("Q/Esc close"))
        .expect("footer");
    let left_bottom = lines
        .iter()
        .position(|line| line.starts_with("└") && line.contains("┘"))
        .expect("left tasks pane bottom border");

    assert!(
        left_bottom + 1 == footer,
        "left pane should run down to the footer, lines:\n{}",
        lines.join("\n")
    );
    assert!(
        footer.saturating_sub(tasks_top) >= 10,
        "left pane should be tall, lines:\n{}",
        lines.join("\n")
    );
}

#[test]
fn populated_renderer_shows_counts_detail_preview_and_footer() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![
        item("bash-run", TaskBrowserStatus::Running),
        item("bash-done", TaskBrowserStatus::Completed),
        item("bash-fail", TaskBrowserStatus::Failed),
    ]));

    let rendered = render_plain(&state, 130, 20).join("\n");

    assert!(rendered.contains("1 running"));
    assert!(rendered.contains("1 completed"));
    assert!(rendered.contains("1 interrupted"));
    assert!(rendered.contains("3 total"));
    assert!(rendered.contains("› ● bash-run"));
    assert!(rendered.contains("id:          bash-run"));
    assert!(rendered.contains("output for bash-run"));
    assert!(rendered.contains("Enter/O output"));
    assert!(rendered.contains("S stop"));
    assert!(rendered.contains("Tab filter"));
}
```

- [ ] **Step 2: Run the failing renderer tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui task_browser
```

Expected: FAIL because the stub renderer only emits `TASK BROWSER`.

- [ ] **Step 3: Replace the stub renderer with real layout code**

Replace `crates/neo-tui/src/tasks_browser/render.rs` with:

```rust
use crate::primitive::{Style, paint, truncate_width, visible_width};
use crate::shell::TuiTheme;

use super::{
    state::{TaskBrowserFilter, TaskBrowserState},
    view::{TaskBrowserItem, TaskBrowserStatus},
};

const MIN_WIDE_WIDTH: usize = 90;
const FOOTER: &str = " ↑↓ select   Enter/O output   S stop   R refresh   Tab filter   Q/Esc close";

pub struct TaskBrowserRenderer<'a> {
    state: &'a TaskBrowserState,
    theme: TuiTheme,
}

impl<'a> TaskBrowserRenderer<'a> {
    #[must_use]
    pub const fn new(state: &'a TaskBrowserState, theme: TuiTheme) -> Self {
        Self { state, theme }
    }

    #[must_use]
    pub fn render(&self, width: usize, height: usize) -> Vec<String> {
        if width == 0 || height == 0 {
            return Vec::new();
        }
        if height < 6 {
            return self.render_tiny(width, height);
        }
        if width >= MIN_WIDE_WIDTH {
            self.render_wide(width, height)
        } else {
            self.render_narrow(width, height)
        }
    }

    fn render_wide(&self, width: usize, height: usize) -> Vec<String> {
        let header_height = 2usize;
        let footer_height = 1usize;
        let content_height = height.saturating_sub(header_height + footer_height).max(3);
        let gap = 1usize;
        let left_width = (width / 3).clamp(30, 42);
        let right_width = width.saturating_sub(left_width + gap).max(1);
        let detail_height = (content_height / 2).max(4);
        let preview_height = content_height.saturating_sub(detail_height).max(3);

        let mut lines = vec![self.header(width), String::new()];
        let left = self.tasks_pane(left_width, content_height);
        let detail = self.detail_pane(right_width, detail_height);
        let preview = self.preview_pane(right_width, preview_height);
        let mut right = detail;
        right.extend(preview);
        right.truncate(content_height);
        while right.len() < content_height {
            right.push(" ".repeat(right_width));
        }

        for row in 0..content_height {
            lines.push(format!("{}{}{}", left[row], " ".repeat(gap), right[row]));
        }
        lines.push(self.footer(width));
        lines.truncate(height);
        while lines.len() < height {
            lines.push(String::new());
        }
        lines
    }

    fn render_narrow(&self, width: usize, height: usize) -> Vec<String> {
        let mut lines = vec![self.header(width)];
        let footer_height = 1usize;
        let content_height = height.saturating_sub(2 + footer_height).max(3);
        let list_height = (content_height / 2).max(4).min(content_height);
        let remaining = content_height.saturating_sub(list_height);
        lines.extend(self.tasks_pane(width, list_height));
        if remaining > 0 {
            lines.extend(self.detail_pane(width, remaining.max(3)));
        }
        lines.push(self.footer(width));
        lines.truncate(height);
        while lines.len() < height {
            lines.push(String::new());
        }
        lines
    }

    fn render_tiny(&self, width: usize, height: usize) -> Vec<String> {
        let mut lines = vec![self.header(width)];
        if height > 2 {
            lines.push(truncate_width("Tasks: use a taller terminal", width, "…", false));
        }
        if height > 1 {
            lines.push(self.footer(width));
        }
        lines.truncate(height);
        lines
    }

    fn header(&self, width: usize) -> String {
        let visible = self.state.visible_items();
        let running = visible
            .iter()
            .filter(|item| item.status == TaskBrowserStatus::Running)
            .count();
        let waiting = visible
            .iter()
            .filter(|item| item.status == TaskBrowserStatus::Waiting)
            .count();
        let completed = visible
            .iter()
            .filter(|item| item.status == TaskBrowserStatus::Completed)
            .count();
        let interrupted = visible
            .iter()
            .filter(|item| item.status.is_interrupted())
            .count();
        let mut header = format!(" TASK BROWSER  filter={} ", self.state.filter().label());
        if running > 0 {
            header.push_str(&format!(" {running} running "));
        }
        if waiting > 0 {
            header.push_str(&format!(" {waiting} waiting "));
        }
        if completed > 0 {
            header.push_str(&format!(" {completed} completed "));
        }
        if interrupted > 0 {
            header.push_str(&format!(" {interrupted} interrupted "));
        }
        header.push_str(&format!(" {} total", visible.len()));
        truncate_width(&header, width, "…", false)
    }

    fn footer(&self, width: usize) -> String {
        if let Some(task_id) = self.state.stop_confirmation_task_id() {
            return truncate_width(
                &format!(" Stop {task_id}?  Enter confirm   Esc cancel"),
                width,
                "…",
                false,
            );
        }
        if let Some(message) = self.state.footer_message() {
            return truncate_width(&format!(" {message}"), width, "…", false);
        }
        truncate_width(FOOTER, width, "…", false)
    }

    fn tasks_pane(&self, width: usize, height: usize) -> Vec<String> {
        let title = format!(" Tasks [{}] ", self.state.filter().pane_label());
        let visible = self.state.visible_items();
        let mut body = Vec::new();
        if visible.is_empty() {
            let empty = match self.state.filter() {
                TaskBrowserFilter::All => "No background tasks in this session.",
                TaskBrowserFilter::Active => "No active tasks. Tab = show all.",
            };
            body.extend(wrap_words(empty, width.saturating_sub(4)));
        } else {
            for item in visible {
                body.push(self.task_row(item, width.saturating_sub(4)));
            }
        }
        pane(&title, width, height, body, self.theme.overlay_border)
    }

    fn detail_pane(&self, width: usize, height: usize) -> Vec<String> {
        let body = self.state.selected_item().map_or_else(
            || vec!["Select a task from the list.".to_owned()],
            |item| item.detail_lines.clone(),
        );
        pane(" Detail ", width, height, body, self.theme.overlay_border)
    }

    fn preview_pane(&self, width: usize, height: usize) -> Vec<String> {
        let body = self.state.selected_item().map_or_else(
            || vec!["No task selected.".to_owned()],
            |item| {
                if item.preview_lines.is_empty() {
                    vec!["No output yet.".to_owned()]
                } else {
                    item.preview_lines.clone()
                }
            },
        );
        pane(" Preview Output ", width, height, body, self.theme.overlay_border)
    }

    fn task_row(&self, item: &TaskBrowserItem, width: usize) -> String {
        let pointer = if self.state.selected_task_id() == Some(item.id.as_str()) {
            "›"
        } else {
            " "
        };
        let raw = format!(
            "{pointer} {} {}  {:<9} {}",
            item.status.marker(),
            item.id,
            item.status.label(),
            item.title
        );
        truncate_width(&raw, width, "…", false)
    }
}

fn pane(title: &str, width: usize, height: usize, body: Vec<String>, color: crate::primitive::Color) -> Vec<String> {
    if width < 2 || height == 0 {
        return Vec::new();
    }
    if height == 1 {
        return vec![truncate_width(title.trim(), width, "…", false)];
    }
    let inner = width.saturating_sub(2);
    let border_style = Style::default().fg(color);
    let mut lines = Vec::with_capacity(height);
    lines.push(paint(&titled_top(title, inner), border_style));
    let content_rows = height.saturating_sub(2);
    for row in 0..content_rows {
        let raw = body.get(row).map_or("", String::as_str);
        lines.push(side_line(raw, inner, border_style));
    }
    lines.push(paint(&format!("└{}┘", "─".repeat(inner)), border_style));
    lines
}

fn titled_top(title: &str, inner: usize) -> String {
    let title_width = visible_width(title);
    if title_width >= inner {
        return format!("┌{}┐", truncate_width(title, inner, "", false));
    }
    format!("┌{title}{}┐", "─".repeat(inner - title_width))
}

fn side_line(raw: &str, inner: usize, border_style: Style) -> String {
    let text = truncate_width(raw, inner.saturating_sub(2), "…", false);
    let padding = " ".repeat(inner.saturating_sub(2).saturating_sub(visible_width(&text)));
    format!(
        "{} {}{} {}",
        paint("│", border_style),
        text,
        padding,
        paint("│", border_style)
    )
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let next_width = if current.is_empty() {
            visible_width(word)
        } else {
            visible_width(&current) + 1 + visible_width(word)
        };
        if next_width > width && !current.is_empty() {
            lines.push(current);
            current = word.to_owned();
        } else if current.is_empty() {
            current = word.to_owned();
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
```

- [ ] **Step 4: Fix `TaskBrowserState` methods needed by the renderer**

If the renderer cannot access `selected_task_id`, `footer_message`, `stop_confirmation_task_id`, or `visible_items`, keep the method names exactly as used above and add missing public accessors in `crates/neo-tui/src/tasks_browser/state.rs`. Do not expose mutable internals.

- [ ] **Step 5: Run renderer tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui task_browser
```

Expected: PASS. The `left_tasks_pane_consumes_full_content_height` test must pass by rendering the left pane down to the Task Browser shortcut footer, not by weakening the assertion.

- [ ] **Checkpoint**

Review the wide layout visually in the test failure output or by adding a temporary local `dbg!(lines.join("\n"))` while developing. Remove temporary debug output before completing the task.

---

## Task 3: Overlay Integration In `neo-tui`

**Files:**
- Modify: `crates/neo-tui/src/shell/overlay.rs`
- Modify: `crates/neo-tui/src/shell/mod.rs`
- Modify: `crates/neo-tui/src/app.rs`
- Test: `crates/neo-tui/tests/app_shell.rs`

- [ ] **Step 1: Write failing overlay tests**

Append to `crates/neo-tui/tests/app_shell.rs`:

```rust
use neo_tui::tasks_browser::{
    TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot, TaskBrowserState, TaskBrowserStatus,
};

fn task_browser_item(id: &str, status: TaskBrowserStatus) -> TaskBrowserItem {
    TaskBrowserItem {
        id: id.to_owned(),
        kind: TaskBrowserKind::Bash,
        status,
        title: "cargo test".to_owned(),
        description: "cargo test".to_owned(),
        elapsed: "00:05".to_owned(),
        detail_lines: vec![format!("id:          {id}")],
        preview_lines: vec!["running tests".to_owned()],
        can_stop: status.is_active(),
    }
}

#[test]
fn task_browser_overlay_blocks_prompt_and_renders_footer() {
    let mut app = NeoChromeState::new("neo", "test-session", "model", "/tmp/neo-ws");
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![task_browser_item(
        "bash-1",
        TaskBrowserStatus::Running,
    )]));
    app.push_task_browser_overlay(state);

    assert!(app.focused_overlay_blocks_prompt());
    assert!(app.focused_overlay_is_rich_dialog());

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(120, 18));
    let lines = tui
        .render(120, 18)
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>();
    let rendered = lines.join("\n");

    assert!(rendered.contains("TASK BROWSER"));
    assert!(rendered.contains("Tasks [all]"));
    assert!(rendered.contains("bash-1"));
    assert!(rendered.contains("Q/Esc close"));
    assert!(!rendered.contains("> "));
    assert!(!rendered.contains("/tmp/neo-ws"));
}
```

- [ ] **Step 2: Run failing overlay tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui task_browser_overlay_blocks_prompt_and_renders_footer
```

Expected: compile failure because `push_task_browser_overlay` and `OverlayKind::TaskBrowser` do not exist.

- [ ] **Step 3: Add overlay variant and render support**

In `crates/neo-tui/src/shell/overlay.rs`, import task browser types:

```rust
use crate::tasks_browser::{TaskBrowserRenderer, TaskBrowserState};
```

Add the enum variant:

```rust
TaskBrowser(TaskBrowserState),
```

Update `render_standalone_lines` to render the task browser when present:

```rust
pub(super) fn render_standalone_lines(
    &self,
    width: usize,
    theme: &TuiTheme,
) -> Option<Vec<String>> {
    match &self.kind {
        OverlayKind::TaskBrowser(state) => {
            Some(TaskBrowserRenderer::new(state, *theme).render(width, 24))
        }
        _ => self
            .kind
            .session_picker_lines(width, theme)
            .or_else(|| self.kind.rich_dialog_lines(width)),
    }
}
```

This temporary `24` height will be replaced in Step 5 when the app passes actual full-screen overlay height.

Update `height()` for `TaskBrowser`:

```rust
OverlayKind::TaskBrowser(_) => 0,
```

- [ ] **Step 4: Add chrome APIs and prompt blocking**

In `crates/neo-tui/src/shell/mod.rs`, import `TaskBrowserState` and add:

```rust
pub fn push_task_browser_overlay(&mut self, state: TaskBrowserState) -> OverlayId {
    self.push_overlay(Overlay::new("tasks", OverlayKind::TaskBrowser(state)))
}

#[must_use]
pub fn task_browser_state(&self) -> Option<&TaskBrowserState> {
    let OverlayKind::TaskBrowser(state) = &self.focused_overlay()?.kind else {
        return None;
    };
    Some(state)
}

pub fn task_browser_state_mut(&mut self) -> Option<&mut TaskBrowserState> {
    let OverlayKind::TaskBrowser(state) = &mut self.focused_overlay_mut()?.kind else {
        return None;
    };
    Some(state)
}
```

Add `OverlayKind::TaskBrowser(_)` to `focused_overlay_is_rich_dialog()` and `focused_overlay_blocks_prompt()`.

- [ ] **Step 5: Pass full content height into task browser rendering**

Change `Overlay::render_standalone_lines` to accept height:

```rust
pub(super) fn render_standalone_lines(
    &self,
    width: usize,
    height: usize,
    theme: &TuiTheme,
) -> Option<Vec<String>> {
    match &self.kind {
        OverlayKind::TaskBrowser(state) => {
            Some(TaskBrowserRenderer::new(state, *theme).render(width, height))
        }
        _ => self
            .kind
            .session_picker_lines(width, theme)
            .or_else(|| self.kind.rich_dialog_lines(width)),
    }
}
```

Update `NeoChromeState::render_focused_overlay` in `crates/neo-tui/src/shell/mod.rs` to accept height:

```rust
pub fn render_focused_overlay(&self, width: usize, height: usize) -> Option<Vec<String>> {
    self.focused_overlay()?
        .render_standalone_lines(width, height, &self.theme)
}
```

Update the blocking-overlay path in `crates/neo-tui/src/app.rs` so `TaskBrowser` owns the full available frame, including its own shortcut footer. Do not append Neo's normal composer/footer under a `TaskBrowser` overlay.

```rust
if matches!(
    app.focused_overlay().map(|overlay| &overlay.kind),
    Some(OverlayKind::TaskBrowser(_))
) {
    let overlay = app
        .render_focused_overlay(content_width, height)
        .unwrap_or_default();
    return ChromeRender {
        lines: overlay,
        cursor: None,
        prompt_start_row: 0,
    };
}
let footer = render_footer_only_lines(app, width);
let overlay_height = height.saturating_sub(footer.len()).max(1);
let overlay = app
    .render_focused_overlay(content_width, overlay_height)
    .unwrap_or_default();
```

Keep old overlay tests compiling by updating any `render_focused_overlay(width)` call sites to pass a reasonable height such as `24`.

- [ ] **Step 6: Route task browser input in chrome**

In `NeoChromeState::handle_focused_dialog_input`, add:

```rust
OverlayKind::TaskBrowser(state) => state.handle_input(&input),
```

If `TaskBrowserState::handle_input` does not exist yet, add it in `crates/neo-tui/src/tasks_browser/state.rs`:

```rust
use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::InputResult;
```

and:

```rust
pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
    let result = match input {
        InputEvent::Action(KeybindingAction::SelectUp) => self.handle_action(TaskBrowserAction::SelectUp),
        InputEvent::Action(KeybindingAction::SelectDown) => self.handle_action(TaskBrowserAction::SelectDown),
        InputEvent::Action(KeybindingAction::SelectPageUp) => self.handle_action(TaskBrowserAction::SelectPageUp),
        InputEvent::Action(KeybindingAction::SelectPageDown) => self.handle_action(TaskBrowserAction::SelectPageDown),
        InputEvent::MoveHome => self.handle_action(TaskBrowserAction::SelectFirst),
        InputEvent::MoveEnd => self.handle_action(TaskBrowserAction::SelectLast),
        InputEvent::Submit => self.handle_action(TaskBrowserAction::ConfirmStop),
        InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
            self.handle_action(TaskBrowserAction::Cancel)
        }
        InputEvent::Insert('q' | 'Q') => self.handle_action(TaskBrowserAction::Close),
        InputEvent::Insert('r' | 'R') => self.handle_action(TaskBrowserAction::Refresh),
        InputEvent::Insert('s' | 'S') => self.handle_action(TaskBrowserAction::RequestStop),
        InputEvent::Insert('o' | 'O') => self.handle_action(TaskBrowserAction::ToggleOutputFocus),
        InputEvent::Action(KeybindingAction::InputTab) => self.handle_action(TaskBrowserAction::ToggleFilter),
        _ => return InputResult::Ignored,
    };
    if result.as_deref() == Some("__close__") {
        InputResult::Cancelled
    } else if result.is_some() {
        InputResult::Submitted
    } else {
        InputResult::Handled
    }
}
```

This handles pure state input; Task 5 will add controller-side stop/refresh effects.

- [ ] **Step 7: Run overlay tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui task_browser task_browser_overlay_blocks_prompt_and_renders_footer
```

Expected: PASS.

- [ ] **Checkpoint**

Review that the task browser is full-screen: prompt is hidden, footer remains visible, and the left Tasks pane uses the overlay content height.

---

## Task 4: Adapter From Background Snapshots To Browser Items

**Files:**
- Create: `crates/neo-agent/src/modes/task_browser.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs` or module declarations as needed
- Test: `crates/neo-agent/src/modes/task_browser.rs` unit tests

- [ ] **Step 1: Write adapter tests**

Create `crates/neo-agent/src/modes/task_browser.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use neo_agent_core::tools::{
        BackgroundTaskKind, BackgroundTaskSnapshot, BackgroundTaskStatus, CommandOutput,
    };
    use neo_tui::tasks_browser::{TaskBrowserKind, TaskBrowserStatus};

    use super::snapshot_to_item;

    fn bash_snapshot(status: BackgroundTaskStatus) -> BackgroundTaskSnapshot {
        BackgroundTaskSnapshot {
            task_id: "bash-abc".to_owned(),
            kind: BackgroundTaskKind::Bash,
            status,
            description: "cargo run -p xtask -- check".to_owned(),
            elapsed: Duration::from_secs(125),
            output: Some(CommandOutput {
                exit_code: Some(0),
                stdout: "line one\nline two\n".to_owned(),
                stderr: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
            }),
            answers: None,
        }
    }

    #[test]
    fn maps_running_bash_snapshot_to_browser_item() {
        let item = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Running));

        assert_eq!(item.id, "bash-abc");
        assert_eq!(item.kind, TaskBrowserKind::Bash);
        assert_eq!(item.status, TaskBrowserStatus::Running);
        assert!(item.can_stop);
        assert!(item.detail_lines.iter().any(|line| line.contains("id:          bash-abc")));
        assert!(item.detail_lines.iter().any(|line| line.contains("elapsed:     02:05")));
        assert!(item.preview_lines.iter().any(|line| line.contains("line two")));
    }

    #[test]
    fn terminal_bash_snapshot_cannot_stop() {
        let item = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Completed));

        assert_eq!(item.status, TaskBrowserStatus::Completed);
        assert!(!item.can_stop);
    }

    #[test]
    fn maps_question_snapshot_to_waiting_item() {
        let snapshot = BackgroundTaskSnapshot {
            task_id: "question-1".to_owned(),
            kind: BackgroundTaskKind::Question,
            status: BackgroundTaskStatus::WaitingForUser,
            description: "Pick one".to_owned(),
            elapsed: Duration::from_secs(31),
            output: None,
            answers: None,
        };

        let item = snapshot_to_item(&snapshot);

        assert_eq!(item.kind, TaskBrowserKind::Question);
        assert_eq!(item.status, TaskBrowserStatus::Waiting);
        assert!(item.can_stop);
        assert!(item.detail_lines.iter().any(|line| line.contains("prompt:      Pick one")));
        assert_eq!(item.preview_lines, vec!["Pick one".to_owned()]);
    }

    #[test]
    fn truncated_output_gets_marker() {
        let mut snapshot = bash_snapshot(BackgroundTaskStatus::Running);
        let output = snapshot.output.as_mut().expect("output");
        output.stdout_truncated = true;

        let item = snapshot_to_item(&snapshot);

        assert!(item.preview_lines.iter().any(|line| line.contains("[output truncated]")));
    }
}
```

- [ ] **Step 2: Run failing adapter tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser
```

Expected: compile failure because `snapshot_to_item` is not defined or module is not wired.

- [ ] **Step 3: Implement adapter functions**

Above the tests in `crates/neo-agent/src/modes/task_browser.rs`, add:

```rust
use std::time::Duration;

use neo_agent_core::tools::{
    BackgroundTaskKind, BackgroundTaskSnapshot, BackgroundTaskStatus, CommandOutput,
};
use neo_tui::tasks_browser::{
    TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot, TaskBrowserStatus,
};

pub fn snapshots_to_browser_snapshot(snapshots: &[BackgroundTaskSnapshot]) -> TaskBrowserSnapshot {
    TaskBrowserSnapshot::new(snapshots.iter().map(snapshot_to_item).collect())
}

pub fn snapshot_to_item(snapshot: &BackgroundTaskSnapshot) -> TaskBrowserItem {
    let kind = match snapshot.kind {
        BackgroundTaskKind::Bash => TaskBrowserKind::Bash,
        BackgroundTaskKind::Question => TaskBrowserKind::Question,
    };
    let status = map_status(snapshot.status);
    let elapsed = format_duration(snapshot.elapsed);
    let mut detail_lines = vec![
        format!("id:          {}", snapshot.task_id),
        format!("kind:        {}", snapshot.kind.as_str()),
        format!("status:      {}", status.label()),
        format!("elapsed:     {elapsed}"),
    ];
    match snapshot.kind {
        BackgroundTaskKind::Bash => {
            detail_lines.push(format!("command:     {}", snapshot.description));
            if let Some(output) = &snapshot.output
                && !snapshot.status.is_active()
            {
                if let Some(exit_code) = output.exit_code {
                    detail_lines.push(format!("exit code:   {exit_code}"));
                }
            }
        }
        BackgroundTaskKind::Question => {
            detail_lines.push(format!("prompt:      {}", snapshot.description));
            if let Some(answers) = &snapshot.answers {
                detail_lines.push(format!("answers:     {}", answers.join(", ")));
            }
        }
    }

    TaskBrowserItem {
        id: snapshot.task_id.clone(),
        kind,
        status,
        title: snapshot.description.clone(),
        description: snapshot.description.clone(),
        elapsed,
        detail_lines,
        preview_lines: preview_lines(snapshot),
        can_stop: snapshot.status.is_active(),
    }
}

fn map_status(status: BackgroundTaskStatus) -> TaskBrowserStatus {
    match status {
        BackgroundTaskStatus::Running => TaskBrowserStatus::Running,
        BackgroundTaskStatus::WaitingForUser => TaskBrowserStatus::Waiting,
        BackgroundTaskStatus::Completed => TaskBrowserStatus::Completed,
        BackgroundTaskStatus::Failed => TaskBrowserStatus::Failed,
        BackgroundTaskStatus::Stopped => TaskBrowserStatus::Stopped,
        BackgroundTaskStatus::TimedOut => TaskBrowserStatus::TimedOut,
    }
}

fn preview_lines(snapshot: &BackgroundTaskSnapshot) -> Vec<String> {
    match snapshot.kind {
        BackgroundTaskKind::Question => vec![snapshot.description.clone()],
        BackgroundTaskKind::Bash => snapshot
            .output
            .as_ref()
            .map_or_else(Vec::new, output_preview_lines),
    }
}

fn output_preview_lines(output: &CommandOutput) -> Vec<String> {
    let mut lines = Vec::new();
    if output.stdout_truncated || output.stderr_truncated {
        lines.push("[output truncated]".to_owned());
    }
    lines.extend(output.stdout.lines().map(ToOwned::to_owned));
    lines.extend(output.stderr.lines().map(ToOwned::to_owned));
    const MAX_PREVIEW_LINES: usize = 200;
    if lines.len() > MAX_PREVIEW_LINES {
        lines = lines[lines.len() - MAX_PREVIEW_LINES..].to_vec();
    }
    lines
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}
```

Wire the module by adding the appropriate module declaration near other `modes` modules. If `interactive.rs` uses sibling modules directly, add:

```rust
mod task_browser;
```

or, if `crates/neo-agent/src/modes/mod.rs` owns module exports, add it there.

- [ ] **Step 4: Run adapter tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser
```

Expected: PASS.

- [ ] **Checkpoint**

Review that adapter output uses human labels and does not expose raw enum strings such as `waiting_for_user` in the Task Browser UI.

---

## Task 5: Open `/tasks` As Browser Instead Of Transcript Status

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-agent/src/modes/task_browser.rs`
- Test: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: Replace old `/tasks` controller tests**

Find existing tests:

- `slash_tasks_lists_shared_background_tasks`
- `shell_mode_slash_tasks_lists_tasks_instead_of_running_shell_command`

Replace their assertions with browser assertions:

```rust
assert!(
    matches!(
        controller.chrome().focused_overlay().map(|overlay| &overlay.kind),
        Some(OverlayKind::TaskBrowser(_))
    ),
    "expected /tasks to open Task Browser"
);
let rendered = controller.chrome().focused_overlay_lines(120).join("\n");
assert!(rendered.contains("TASK BROWSER"));
assert!(rendered.contains("filter=ALL"));
assert!(rendered.contains("question-1"));
assert!(!transcript_has_status(&controller, "active_background_tasks"));
```

For the shell-mode test, keep:

```rust
assert!(!controller.chrome().shell_running());
```

and add:

```rust
assert!(
    controller.chrome().shell_mode_active(),
    "opening /tasks from shell mode should not exit shell mode"
);
```

- [ ] **Step 2: Run failing controller tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent slash_tasks shell_mode_slash_tasks
```

Expected: FAIL because `/tasks` still appends transcript status.

- [ ] **Step 3: Replace `show_background_tasks` implementation**

In `crates/neo-agent/src/modes/interactive.rs`, replace:

```rust
async fn show_background_tasks(&mut self) {
    let Some(config) = self.local_config.as_ref() else {
        self.push_status("No config available");
        return;
    };
    let tasks = config.background_tasks.list(true, 50).await;
    let result = neo_agent_core::tools::task_list_result(&tasks, true);
    self.push_status(result.content);
}
```

with:

```rust
async fn show_background_tasks(&mut self) {
    let Some(config) = self.local_config.as_ref() else {
        self.push_status("No config available");
        return;
    };
    let tasks = config.background_tasks.list(false, 100).await;
    let snapshot = task_browser::snapshots_to_browser_snapshot(&tasks);
    let mut state = self
        .tui
        .chrome()
        .task_browser_state()
        .cloned()
        .unwrap_or_else(neo_tui::tasks_browser::TaskBrowserState::new);
    state.apply_snapshot(&snapshot);
    self.tui.chrome_mut().push_task_browser_overlay(state);
}
```

Ensure imports resolve:

```rust
use neo_tui::tasks_browser::TaskBrowserState;
```

only if fully-qualified paths become noisy.

Important: use `list(false, 100)` so default `ALL` includes terminal tasks.

- [ ] **Step 4: Run controller tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent slash_tasks shell_mode_slash_tasks
```

Expected: PASS.

- [ ] **Step 5: Run model-facing tool regression**

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core tool_bash task_list_result
```

Expected: PASS. `TaskList` must still return `active_background_tasks: 0` and `No background tasks found.` for model/tool use.

- [ ] **Checkpoint**

Review that `/tasks` no longer calls `task_list_result` in `interactive.rs`. The only remaining uses of `task_list_result` should be model/tool paths and tests.

---

## Task 6: Controller-Side Browser Input, Refresh, Close, And Stop

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-tui/src/tasks_browser/state.rs`
- Test: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: Add controller tests for close, filter, refresh, and stop**

Append these tests near the existing `/tasks` tests in `crates/neo-agent/src/modes/interactive.rs`:

```rust
#[tokio::test]
async fn task_browser_tab_toggles_filter_and_escape_closes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);

    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("open tasks");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("toggle filter");
    let rendered = controller.chrome().focused_overlay_lines(120).join("\n");
    assert!(rendered.contains("filter=ACTIVE"));

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("close tasks");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn task_browser_stop_confirmation_stops_selected_task() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);

    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("open tasks");
    controller
        .handle_input_event(InputEvent::Insert('s'))
        .await
        .expect("request stop");
    let confirm = controller.chrome().focused_overlay_lines(120).join("\n");
    assert!(confirm.contains("Stop question-1?"));

    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm stop");
    let refreshed = controller.chrome().focused_overlay_lines(120).join("\n");
    assert!(refreshed.contains("stopped"));
}
```

Add `KeybindingAction` to the test imports if needed:

```rust
use neo_tui::input::KeybindingAction;
```

- [ ] **Step 2: Run failing input tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser_
```

Expected: FAIL because rich-dialog input handling currently consumes task browser events without async controller effects or close behavior.

- [ ] **Step 3: Route task browser input before generic rich-dialog handling**

In `InteractiveController::handle_input_event`, before `handle_rich_dialog_event`, add:

```rust
if self.handle_task_browser_event(event.clone()).await? {
    return Ok(false);
}
```

Add the method:

```rust
async fn handle_task_browser_event(&mut self, event: InputEvent) -> Result<bool> {
    if self.tui.chrome().task_browser_state().is_none() {
        return Ok(false);
    }

    use neo_tui::tasks_browser::TaskBrowserAction;
    let action = match event {
        InputEvent::Action(KeybindingAction::SelectUp) => TaskBrowserAction::SelectUp,
        InputEvent::Action(KeybindingAction::SelectDown) => TaskBrowserAction::SelectDown,
        InputEvent::Action(KeybindingAction::SelectPageUp) => TaskBrowserAction::SelectPageUp,
        InputEvent::Action(KeybindingAction::SelectPageDown) => TaskBrowserAction::SelectPageDown,
        InputEvent::MoveHome => TaskBrowserAction::SelectFirst,
        InputEvent::MoveEnd => TaskBrowserAction::SelectLast,
        InputEvent::Action(KeybindingAction::InputTab) => TaskBrowserAction::ToggleFilter,
        InputEvent::Insert('o' | 'O') => TaskBrowserAction::ToggleOutputFocus,
        InputEvent::Insert('r' | 'R') => TaskBrowserAction::Refresh,
        InputEvent::Insert('s' | 'S') => TaskBrowserAction::RequestStop,
        InputEvent::Submit => TaskBrowserAction::ConfirmStop,
        InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
            TaskBrowserAction::Cancel
        }
        InputEvent::Insert('q' | 'Q') => TaskBrowserAction::Close,
        _ => return Ok(false),
    };

    self.apply_task_browser_action(action).await?;
    Ok(true)
}
```

- [ ] **Step 4: Implement async browser actions**

Add:

```rust
async fn apply_task_browser_action(
    &mut self,
    action: neo_tui::tasks_browser::TaskBrowserAction,
) -> Result<()> {
    use neo_tui::tasks_browser::TaskBrowserAction;

    match action {
        TaskBrowserAction::Refresh => {
            self.refresh_task_browser().await;
        }
        TaskBrowserAction::Close => {
            let _ = self.tui.chrome_mut().close_focused_overlay();
        }
        TaskBrowserAction::ConfirmStop => {
            let task_id = self
                .tui
                .chrome_mut()
                .task_browser_state_mut()
                .and_then(|state| state.handle_action(TaskBrowserAction::ConfirmStop));
            if let Some(task_id) = task_id {
                self.stop_task_from_browser(&task_id).await;
            }
        }
        TaskBrowserAction::Cancel => {
            let close = self
                .tui
                .chrome_mut()
                .task_browser_state_mut()
                .and_then(|state| state.handle_action(TaskBrowserAction::Cancel))
                .as_deref()
                == Some("__close__");
            if close {
                let _ = self.tui.chrome_mut().close_focused_overlay();
            }
        }
        other => {
            let stop_requested = self
                .tui
                .chrome_mut()
                .task_browser_state_mut()
                .and_then(|state| state.handle_action(other));
            if stop_requested.is_some() && !matches!(other, TaskBrowserAction::RequestStop) {
                self.refresh_task_browser().await;
            }
        }
    }
    Ok(())
}
```

Add refresh helper:

```rust
async fn refresh_task_browser(&mut self) {
    let Some(config) = self.local_config.as_ref() else {
        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
            state.set_footer_message("No config available");
        }
        return;
    };
    let tasks = config.background_tasks.list(false, 100).await;
    let snapshot = task_browser::snapshots_to_browser_snapshot(&tasks);
    if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
        state.apply_snapshot(&snapshot);
        state.clear_footer_message();
    }
}
```

Add stop helper:

```rust
async fn stop_task_from_browser(&mut self, task_id: &str) {
    let Some(config) = self.local_config.as_ref() else {
        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
            state.set_footer_message("No config available");
        }
        return;
    };
    let result = config
        .background_tasks
        .stop(task_id, "Stopped from /tasks", 4096)
        .await;
    match result {
        Ok(_) => self.refresh_task_browser().await,
        Err(error) => {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message(format!("Could not stop task: {error}"));
            }
        }
    }
}
```

- [ ] **Step 5: Keep chrome-level task browser input from double-handling**

Because `InteractiveController` now handles task browser events first, remove `OverlayKind::TaskBrowser(state) => state.handle_input(...)` from `NeoChromeState::handle_focused_dialog_input` if it causes duplicate handling. Keep `TaskBrowser` listed as a rich dialog for blocking/rendering, but let `neo-agent` own browser actions that have async side effects.

- [ ] **Step 6: Run controller tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser_ slash_tasks shell_mode_slash_tasks
```

Expected: PASS.

- [ ] **Checkpoint**

Review event ordering: task browser input must run before prompt edits, so pressing `s`, `r`, `q`, or `Tab` inside the browser never mutates the composer.

---

## Task 7: Periodic Refresh While Browser Is Open

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: Add focused test for refresh helper**

Append:

```rust
#[tokio::test]
async fn task_browser_manual_refresh_keeps_completed_tasks_visible_under_all() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);

    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("open tasks");
    controller
        .handle_input_event(InputEvent::Insert('s'))
        .await
        .expect("request stop");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm stop");
    controller
        .handle_input_event(InputEvent::Insert('r'))
        .await
        .expect("refresh");

    let rendered = controller.chrome().focused_overlay_lines(120).join("\n");
    assert!(rendered.contains("filter=ALL"));
    assert!(rendered.contains("question-1"));
    assert!(rendered.contains("stopped"));
}
```

- [ ] **Step 2: Run the focused refresh test**

Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser_manual_refresh
```

Expected: PASS after Task 6. This guards the default `ALL` behavior.

- [ ] **Step 3: Add lightweight periodic refresh in the terminal loop**

Find the main terminal loop in `crates/neo-agent/src/modes/interactive.rs`. Add a field to `InteractiveController`:

```rust
last_task_browser_refresh: Option<std::time::Instant>,
```

Initialize it to `None` in constructors.

Inside the loop tick path where existing periodic work happens, call:

```rust
self.maybe_refresh_task_browser().await;
```

Add:

```rust
async fn maybe_refresh_task_browser(&mut self) {
    if self.tui.chrome().task_browser_state().is_none() {
        self.last_task_browser_refresh = None;
        return;
    }
    let now = std::time::Instant::now();
    if self
        .last_task_browser_refresh
        .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_secs(1))
    {
        return;
    }
    self.last_task_browser_refresh = Some(now);
    self.refresh_task_browser().await;
}
```

If there is no clean periodic tick location, do not create a new background task. Instead call `maybe_refresh_task_browser()` from the render-loop path that already wakes for input/ticks. Keep it local to the controller.

- [ ] **Step 4: Run controller tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser_ slash_tasks
```

Expected: PASS.

- [ ] **Checkpoint**

Review that refresh is only active while the browser is open and does not append transcript messages.

---

## Task 8: Narrow Layout And Overflow Hardening

**Files:**
- Modify: `crates/neo-tui/src/tasks_browser/render.rs`
- Test: `crates/neo-tui/tests/task_browser.rs`

- [ ] **Step 1: Add narrow/overflow tests**

Append:

```rust
#[test]
fn narrow_renderer_uses_stacked_layout_without_overflow() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![TaskBrowserItem {
        id: "bash-very-long-task-id-that-will-not-fit".to_owned(),
        kind: TaskBrowserKind::Bash,
        status: TaskBrowserStatus::Running,
        title: "a very long command title that must be truncated".to_owned(),
        description: "a very long command title that must be truncated".to_owned(),
        elapsed: "12:34".to_owned(),
        detail_lines: vec!["command:     a very long command title that must be truncated".to_owned()],
        preview_lines: vec!["a very long output line that must be truncated cleanly".to_owned()],
        can_stop: true,
    }]));

    let lines = render_plain(&state, 56, 16);

    assert!(lines.join("\n").contains("TASK BROWSER"));
    assert!(lines.join("\n").contains("Tasks [all]"));
    assert!(lines.iter().all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 56));
}

#[test]
fn low_height_renderer_keeps_footer_visible() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(Vec::new()));

    let lines = render_plain(&state, 100, 6);

    assert_eq!(lines.len(), 6);
    assert!(lines.iter().any(|line| line.contains("Q/Esc close")));
}
```

If `unicode_width` is not already available to the test crate, replace that assertion with Neo's existing `neo_tui::primitive::visible_width(line) <= 56`.

- [ ] **Step 2: Run failing/passing narrow tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui narrow_renderer low_height_renderer
```

Expected: PASS if Task 2 already handled this; otherwise FAIL with overflow or missing footer.

- [ ] **Step 3: Harden renderer only if tests fail**

If the tests fail, update `render_narrow`, `render_tiny`, `pane`, and line truncation code in `crates/neo-tui/src/tasks_browser/render.rs` so:

- returned line count equals requested height
- each visible line width is `<= width`
- footer is present for height `>= 2`
- empty states wrap rather than overflow

Use existing `truncate_width` and `visible_width`; do not introduce a second width-calculation helper.

- [ ] **Step 4: Run renderer tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui task_browser
```

Expected: PASS.

- [ ] **Checkpoint**

Review the wide empty-state character art against the spec: left pane full-height, right column split into Detail and Preview Output.

---

## Task 9: Final Regression And Cleanup

**Files:**
- Modify only files touched in prior tasks if verification exposes issues.
- Test: focused crates.

- [ ] **Step 1: Search for obsolete `/tasks` transcript behavior**

Run:

```bash
rg -n "active_background_tasks|No background tasks found|show_background_tasks|task_list_result\\(&tasks" crates/neo-agent crates/neo-tui crates/neo-agent-core
```

Expected:

- `active_background_tasks` and `No background tasks found` remain in `neo-agent-core` tool tests/tool result code.
- `show_background_tasks` may remain as the browser-opening function name, or be renamed to `open_task_browser`.
- No `neo-agent/src/modes/interactive.rs` path should call `task_list_result(&tasks, ...)` for `/tasks`.

- [ ] **Step 2: Run focused TUI tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui task_browser app_shell
```

Expected: PASS.

- [ ] **Step 3: Run focused agent tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser_ slash_tasks shell_mode_slash_tasks
```

Expected: PASS.

- [ ] **Step 4: Run focused core tool regressions**

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core tool_bash task_list_result
```

Expected: PASS.

- [ ] **Step 5: Build binary**

Run:

```bash
cargo build -p neo-agent
```

Expected: PASS.

- [ ] **Step 6: Default project check**

Run:

```bash
cargo run -p xtask -- check
```

Expected: PASS.

- [ ] **Step 7: Manual smoke**

Run Neo locally in a terminal and verify:

```text
/tasks
```

Expected:

- opens full-screen Task Browser
- default header includes `filter=ALL`
- empty state says `No background tasks in this session.`
- left Tasks pane consumes the full content height down to the footer
- `Q` or `Esc` closes the browser

If a background task is available:

- `Tab` toggles `ACTIVE`
- `R` refreshes
- `S` shows stop confirmation
- `Esc` cancels confirmation first
- `Enter` confirms stop

- [ ] **Checkpoint**

Before reporting completion, include exact verification commands and results. If `cargo run -p xtask -- check --workspace` is attempted and fails on unrelated existing warnings, report that separately; do not widen this task to fix unrelated workspace issues.

---

## Self-Review

### Spec Coverage

- Full-screen Kimi-style Task Browser: Tasks 2 and 3.
- Default `ALL`: Tasks 1, 5, and 7.
- Full-height left Tasks pane: Task 2 renderer test and Task 8 visual/overflow hardening.
- Detail and Preview Output panes: Task 2.
- Empty states: Task 2.
- Filtering and selection preservation: Task 1 and Task 6.
- Stop confirmation: Task 1, Task 6.
- Refresh model: Task 6 manual refresh and Task 7 periodic refresh.
- Shell-mode exact `/tasks`: Task 5.
- Keep `TaskList` tool output unchanged: Tasks 5 and 9.

### Placeholder Scan

No implementation step uses placeholder markers or fill-in instructions. Steps that depend on local code shape include explicit names, expected command outcomes, and concrete fallback instructions.

### Type Consistency

The plan consistently uses:

- `TaskBrowserState`
- `TaskBrowserSnapshot`
- `TaskBrowserItem`
- `TaskBrowserFilter`
- `TaskBrowserStatus`
- `TaskBrowserAction`
- `TaskBrowserRenderer`
- `snapshot_to_item`
- `snapshots_to_browser_snapshot`

These names are introduced before later tasks reference them.
