# Neo Todo Panel Expand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Kimi Code-style `Ctrl+T` expand/collapse behavior to Neo's TUI todo panel, including the latest five-row smart collapsed selector and all-items expanded view.

**Architecture:** Keep todo display logic inside `neo-tui` and route keyboard control through Neo's existing configurable keybinding system. `NeoChromeState` owns the todo expansion state, `TodoPanel` renders collapsed or expanded views from that state, and `neo-agent` handles the action only when the todo panel overflows.

**Tech Stack:** Rust 2024, `neo-tui` widgets/chrome/input keybindings, `neo-agent` interactive controller,  nextest wrapper.

---

## Source References

- Kimi behavior reference: `docs/kimi-code/apps/kimi-code/src/tui/components/chrome/todo-panel.ts`
- Kimi keyboard reference: `docs/kimi-code/apps/kimi-code/src/tui/components/editor/custom-editor.ts`, `docs/kimi-code/apps/kimi-code/src/tui/controllers/editor-keyboard.ts`, `docs/kimi-code/apps/kimi-code/src/tui/kimi-tui.ts`
- Neo current widget: `crates/neo-tui/src/widgets/todo_panel.rs`
- Neo chrome state: `crates/neo-tui/src/shell/state.rs`
- Neo chrome render: `crates/neo-tui/src/transcript/chrome_render.rs`
- Neo keybindings: `crates/neo-tui/src/input/keybinding.rs`, `crates/neo-agent/src/modes/interactive/keybinding_priority.rs`, `crates/neo-agent/src/modes/interactive/input.rs`

## File Structure

- Modify `crates/neo-tui/src/widgets/todo_panel.rs`
  - Owns todo row selection, hidden counts, collapsed/expanded rendering, footer text, and widget unit tests.
  - Add `VisibleTodos` so render and tests can inspect hidden count distribution without recomputing.
  - Add `TodoPanel::expanded(bool)` and make `height()` match render mode.
- Modify `crates/neo-tui/src/shell/state.rs`
  - Owns persistent `todo_panel_expanded: bool`.
  - Exposes `todo_panel_expanded()`, `todo_panel_has_overflow()`, `set_todo_panel_expanded()`, and `toggle_todo_panel_expanded()`.
  - Resets expansion only when todos are explicitly cleared.
- Modify `crates/neo-tui/src/transcript/chrome_render.rs`
  - Passes `app.todo_panel_expanded()` into `TodoPanel`.
- Modify `crates/neo-tui/src/input/keybinding.rs`
  - Adds `KeybindingAction::TodoPanelToggle`, action id `tui.todo.toggle`, and default `ctrl+t` binding.
- Modify `crates/neo-agent/src/modes/interactive/keybinding_priority.rs`
  - Adds `TodoPanelToggle` to editing priority only, near `ToolOutputToggle`.
- Modify `crates/neo-agent/src/modes/interactive/input.rs`
  - Handles `TodoPanelToggle` in `handle_basic_keybinding_action`.
  - If the todo panel has no overflow, consumes the key as a no-op so `Ctrl+T` does not insert text or open unrelated behavior.
  - If it overflows, clears pending exit confirmation, toggles expansion, and requests the normal render path through state mutation.
- Modify `crates/neo-tui/tests/app_shell.rs`
  - Adds chrome render tests for collapsed footer, expanded all-items footer, and prompt start row with expanded todo height.
- Modify `crates/neo-tui/tests/primitives.rs`
  - Adds keybinding default coverage for `ctrl+t`.
- Modify `crates/neo-agent/src/modes/interactive/tests.rs`
  - Adds integration-style controller coverage for `Ctrl+T`.
- Modify `docs/config.md`
  - Documents `tui.todo.toggle = ["ctrl+t"]` as an overridable keybinding.

## Task 1: Match Kimi's Collapsed Todo Selector

**Files:**
- Modify: `crates/neo-tui/src/widgets/todo_panel.rs`

- [ ] **Step 1: Replace selector tests with Kimi-compatible cases**

In `crates/neo-tui/src/widgets/todo_panel.rs`, inside the existing `#[cfg(test)] mod tests`, replace the selector-specific tests with these tests. Keep the existing `item()` helper.

```rust
    #[test]
    fn selector_returns_all_items_when_count_fits() {
        let todos = vec![
            item("a", TodoDisplayStatus::Done),
            item("b", TodoDisplayStatus::InProgress),
            item("c", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, MAX_VISIBLE_TODOS);

        assert_eq!(visible.indices, vec![0, 1, 2]);
        assert_eq!(visible.hidden, 0);
        assert_eq!(visible.hidden_counts.done, 0);
        assert_eq!(visible.hidden_counts.in_progress, 0);
        assert_eq!(visible.hidden_counts.pending, 0);
    }

    #[test]
    fn selector_shows_latest_done_active_and_earliest_pending() {
        let todos = vec![
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("d3", TodoDisplayStatus::Done),
            item("ip", TodoDisplayStatus::InProgress),
            item("p1", TodoDisplayStatus::Pending),
            item("p2", TodoDisplayStatus::Pending),
            item("p3", TodoDisplayStatus::Pending),
            item("p4", TodoDisplayStatus::Pending),
            item("p5", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, MAX_VISIBLE_TODOS);

        assert_eq!(visible.indices, vec![2, 3, 4, 5, 6]);
        assert_eq!(
            visible
                .indices
                .iter()
                .map(|&index| todos[index].title.as_str())
                .collect::<Vec<_>>(),
            vec!["d3", "ip", "p1", "p2", "p3"]
        );
        assert_eq!(visible.hidden, 4);
    }

    #[test]
    fn selector_expands_done_when_pending_has_too_few_items() {
        let todos = vec![
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("d3", TodoDisplayStatus::Done),
            item("d4", TodoDisplayStatus::Done),
            item("d5", TodoDisplayStatus::Done),
            item("ip", TodoDisplayStatus::InProgress),
            item("p1", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, MAX_VISIBLE_TODOS);

        assert_eq!(visible.indices, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn selector_all_pending_shows_first_five() {
        let todos: Vec<TodoDisplayItem> = (0..8)
            .map(|index| item(&format!("p{index}"), TodoDisplayStatus::Pending))
            .collect();

        let visible = select_visible_todos(&todos, MAX_VISIBLE_TODOS);

        assert_eq!(visible.indices, vec![0, 1, 2, 3, 4]);
        assert_eq!(visible.hidden, 3);
        assert_eq!(visible.hidden_counts.pending, 3);
    }

    #[test]
    fn selector_all_done_shows_last_five() {
        let todos: Vec<TodoDisplayItem> = (0..8)
            .map(|index| item(&format!("d{index}"), TodoDisplayStatus::Done))
            .collect();

        let visible = select_visible_todos(&todos, MAX_VISIBLE_TODOS);

        assert_eq!(visible.indices, vec![3, 4, 5, 6, 7]);
        assert_eq!(visible.hidden, 3);
        assert_eq!(visible.hidden_counts.done, 3);
    }

    #[test]
    fn selector_mixed_done_pending_without_active_keeps_one_recent_done() {
        let todos = vec![
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("d3", TodoDisplayStatus::Done),
            item("p1", TodoDisplayStatus::Pending),
            item("p2", TodoDisplayStatus::Pending),
            item("p3", TodoDisplayStatus::Pending),
            item("p4", TodoDisplayStatus::Pending),
            item("p5", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, MAX_VISIBLE_TODOS);

        assert_eq!(visible.indices, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn selector_hidden_counts_reflect_hidden_items() {
        let todos = vec![
            item("ip0", TodoDisplayStatus::InProgress),
            item("ip1", TodoDisplayStatus::InProgress),
            item("ip2", TodoDisplayStatus::InProgress),
            item("ip3", TodoDisplayStatus::InProgress),
            item("ip4", TodoDisplayStatus::InProgress),
            item("ip5", TodoDisplayStatus::InProgress),
            item("d0", TodoDisplayStatus::Done),
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("p0", TodoDisplayStatus::Pending),
            item("p1", TodoDisplayStatus::Pending),
            item("p2", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, MAX_VISIBLE_TODOS);

        assert_eq!(visible.indices, vec![0, 1, 2, 3, 4]);
        assert_eq!(visible.hidden, 7);
        assert_eq!(visible.hidden_counts.done, 3);
        assert_eq!(visible.hidden_counts.in_progress, 1);
        assert_eq!(visible.hidden_counts.pending, 3);
    }
```

- [ ] **Step 2: Run the selector tests and verify they fail**

Run:

```bash
```

Expected: FAIL. The first failure should mention `no field indices on type Vec<usize>` or a mismatch from the current `Vec<usize>` selector. This confirms the tests are exercising the new selector contract.

- [ ] **Step 3: Implement `TodoHiddenCounts`, `VisibleTodos`, and Kimi-compatible selection**

In `crates/neo-tui/src/widgets/todo_panel.rs`, replace the existing `select_visible_todos` return type and implementation with this code.

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TodoHiddenCounts {
    pub done: usize,
    pub in_progress: usize,
    pub pending: usize,
}

impl TodoHiddenCounts {
    fn add(&mut self, status: TodoDisplayStatus) {
        match status {
            TodoDisplayStatus::Done => self.done += 1,
            TodoDisplayStatus::InProgress => self.in_progress += 1,
            TodoDisplayStatus::Pending => self.pending += 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleTodos {
    pub indices: Vec<usize>,
    pub hidden: usize,
    pub hidden_counts: TodoHiddenCounts,
}

#[must_use]
pub fn select_visible_todos(todos: &[TodoDisplayItem], max_visible: usize) -> VisibleTodos {
    if todos.is_empty() || max_visible == 0 {
        return VisibleTodos {
            indices: Vec::new(),
            hidden: todos.len(),
            hidden_counts: hidden_counts_for(todos, &[]),
        };
    }
    if todos.len() <= max_visible {
        return VisibleTodos {
            indices: (0..todos.len()).collect(),
            hidden: 0,
            hidden_counts: TodoHiddenCounts::default(),
        };
    }

    let mut in_progress = Vec::new();
    let mut pending = Vec::new();
    let mut done = Vec::new();
    for (index, todo) in todos.iter().enumerate() {
        match todo.status {
            TodoDisplayStatus::InProgress => in_progress.push(index),
            TodoDisplayStatus::Pending => pending.push(index),
            TodoDisplayStatus::Done => done.push(index),
        }
    }

    let mut selected = Vec::new();
    selected.extend(in_progress.into_iter().take(max_visible));

    if selected.len() < max_visible {
        let remaining = max_visible - selected.len();
        let mut done_candidates = done;
        done_candidates.reverse();

        let (done_count, pending_count) = if done_candidates.is_empty() {
            (0, remaining.min(pending.len()))
        } else if pending.is_empty() {
            (remaining.min(done_candidates.len()), 0)
        } else {
            let pending_count = (remaining - 1).min(pending.len());
            let done_count = if pending_count < remaining - 1 {
                (remaining - pending_count).min(done_candidates.len())
            } else {
                1
            };
            (done_count, pending_count)
        };

        selected.extend(done_candidates.into_iter().take(done_count));
        selected.extend(pending.into_iter().take(pending_count));
    }

    selected.sort_unstable();
    let hidden = todos.len().saturating_sub(selected.len());
    let hidden_counts = hidden_counts_for(todos, &selected);

    VisibleTodos {
        indices: selected,
        hidden,
        hidden_counts,
    }
}

fn hidden_counts_for(todos: &[TodoDisplayItem], selected: &[usize]) -> TodoHiddenCounts {
    let selected: std::collections::BTreeSet<usize> = selected.iter().copied().collect();
    let mut counts = TodoHiddenCounts::default();
    for (index, todo) in todos.iter().enumerate() {
        if !selected.contains(&index) {
            counts.add(todo.status);
        }
    }
    counts
}
```

- [ ] **Step 4: Update existing render code to use `.indices`**

In `height()` and `render()`, replace direct iteration over the old `Vec<usize>` with `visible.indices`.

Use this shape:

```rust
let visible = select_visible_todos(self.todos, MAX_VISIBLE_TODOS);
let item_lines: usize = visible
    .indices
    .iter()
    .map(|&i| wrap_width(&self.todos[i].title, inner_width).len().max(1))
    .sum();
let hidden = visible.hidden > 0;
```

And:

```rust
let visible = select_visible_todos(self.todos, MAX_VISIBLE_TODOS);
for &index in &visible.indices {
    lines.extend(render_item(&self.todos[index], inner_width, self.theme));
}
```

- [ ] **Step 5: Run selector/widget tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 6: Commit checkpoint**

Neo forbids git mutations without explicit per-command authorization. Stop here and ask the user whether to run exactly:

```bash
git add crates/neo-tui/src/widgets/todo_panel.rs
git commit -m "feat(tui): match kimi todo selector"
```

If authorization is not granted, continue without committing.

## Task 2: Add Collapsed/Expanded Todo Rendering

**Files:**
- Modify: `crates/neo-tui/src/widgets/todo_panel.rs`
- Modify: `crates/neo-tui/src/shell/state.rs`
- Modify: `crates/neo-tui/src/transcript/chrome_render.rs`
- Modify: `crates/neo-tui/tests/app_shell.rs`

- [ ] **Step 1: Add failing widget tests for footer text and expanded rendering**

In `crates/neo-tui/src/widgets/todo_panel.rs`, add these tests inside `mod tests`.

```rust
    #[test]
    fn collapsed_footer_advertises_ctrl_t_and_hidden_distribution() {
        let todos = vec![
            item("ip0", TodoDisplayStatus::InProgress),
            item("ip1", TodoDisplayStatus::InProgress),
            item("ip2", TodoDisplayStatus::InProgress),
            item("ip3", TodoDisplayStatus::InProgress),
            item("ip4", TodoDisplayStatus::InProgress),
            item("ip5", TodoDisplayStatus::InProgress),
            item("d0", TodoDisplayStatus::Done),
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("p0", TodoDisplayStatus::Pending),
            item("p1", TodoDisplayStatus::Pending),
            item("p2", TodoDisplayStatus::Pending),
        ];

        let plain = TodoPanel::new(&todos)
            .render(80)
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(plain.contains("… +7 more (3 done · 1 in progress · 3 pending) · ctrl+t to expand"));
    }

    #[test]
    fn expanded_panel_renders_all_items_and_collapse_hint() {
        let todos: Vec<TodoDisplayItem> = (0..7)
            .map(|index| item(&format!("task-{index}"), TodoDisplayStatus::Pending))
            .collect();

        let plain = TodoPanel::new(&todos)
            .expanded(true)
            .render(80)
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(plain.contains("○ task-0"));
        assert!(plain.contains("○ task-6"));
        assert!(plain.contains("all 7 items · ctrl+t to collapse"));
        assert!(!plain.contains("+2 more"));
    }
```

- [ ] **Step 2: Add failing chrome state/render tests**

In `crates/neo-tui/tests/app_shell.rs`, add this helper near the existing `render_app` helper:

```rust
fn todo_item(title: &str, status: neo_tui::widgets::TodoDisplayStatus) -> neo_tui::widgets::TodoDisplayItem {
    neo_tui::widgets::TodoDisplayItem::new(title, status)
}
```

Then add these tests near the existing todo panel tests.

```rust
#[test]
fn todo_panel_expanded_state_renders_all_items_before_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_todo_items((0..7).map(|index| {
        todo_item(&format!("task-{index}"), neo_tui::widgets::TodoDisplayStatus::Pending)
    }).collect());
    app.set_todo_panel_expanded(true);
    app.prompt_mut().apply_edit(PromptEdit::Insert("next prompt"));

    let lines = render_app(80, &app).join("\n");

    assert!(lines.contains("○ task-0"));
    assert!(lines.contains("○ task-6"));
    assert!(lines.contains("all 7 items · ctrl+t to collapse"));
    assert!(lines.contains("next prompt"));
}

#[test]
fn todo_panel_clear_resets_expanded_state() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_todo_items((0..7).map(|index| {
        todo_item(&format!("task-{index}"), neo_tui::widgets::TodoDisplayStatus::Pending)
    }).collect());
    app.set_todo_panel_expanded(true);

    app.clear_todos();
    app.set_todo_items((0..7).map(|index| {
        todo_item(&format!("new-{index}"), neo_tui::widgets::TodoDisplayStatus::Pending)
    }).collect());

    let lines = render_app(80, &app).join("\n");
    assert!(lines.contains("… +2 more"));
    assert!(lines.contains("ctrl+t to expand"));
    assert!(!lines.contains("new-6"));
}
```

- [ ] **Step 3: Run tests and verify they fail**

Run:

```bash
```

Expected: FAIL. The failures should mention missing `expanded`, `set_todo_panel_expanded`, or old footer text.

- [ ] **Step 4: Implement expanded flag and hidden distribution rendering**

In `TodoPanel`, add an `expanded` field and builder method.

```rust
pub struct TodoPanel<'a> {
    todos: &'a [TodoDisplayItem],
    theme: TuiTheme,
    expanded: bool,
}

impl<'a> TodoPanel<'a> {
    #[must_use]
    pub fn new(todos: &'a [TodoDisplayItem]) -> Self {
        Self {
            todos,
            theme: TuiTheme::default(),
            expanded: false,
        }
    }

    #[must_use]
    pub const fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }
}
```

Add hidden-count formatting near `render_item`.

```rust
fn format_hidden_counts(counts: TodoHiddenCounts) -> String {
    let mut parts = Vec::new();
    if counts.done > 0 {
        parts.push(format!("{} done", counts.done));
    }
    if counts.in_progress > 0 {
        parts.push(format!("{} in progress", counts.in_progress));
    }
    if counts.pending > 0 {
        parts.push(format!("{} pending", counts.pending));
    }
    parts.join(" · ")
}
```

Update `render()` to branch on `self.expanded`.

```rust
let indices: Vec<usize> = if self.expanded {
    (0..self.todos.len()).collect()
} else {
    select_visible_todos(self.todos, MAX_VISIBLE_TODOS).indices
};

for &index in &indices {
    lines.extend(render_item(&self.todos[index], inner_width, self.theme));
}

if self.expanded {
    if self.todos.len() > MAX_VISIBLE_TODOS {
        lines.push(paint(
            &format!("  all {} items · ctrl+t to collapse", self.todos.len()),
            Style::default().fg(self.theme.text_muted),
        ));
    }
} else {
    let visible = select_visible_todos(self.todos, MAX_VISIBLE_TODOS);
    if visible.hidden > 0 {
        let distribution = format_hidden_counts(visible.hidden_counts);
        let suffix = if distribution.is_empty() {
            String::new()
        } else {
            format!(" ({distribution})")
        };
        lines.push(paint(
            &format!("  … +{} more{} · ctrl+t to expand", visible.hidden, suffix),
            Style::default().fg(self.theme.text_muted),
        ));
    }
}
```

Keep the final `truncate_width` mapping unchanged.

- [ ] **Step 5: Update `height()` for expanded mode**

In `TodoPanel::height`, compute indices from expanded state:

```rust
let indices: Vec<usize> = if self.expanded {
    (0..self.todos.len()).collect()
} else {
    select_visible_todos(self.todos, MAX_VISIBLE_TODOS).indices
};
let item_lines: usize = indices
    .iter()
    .map(|&i| wrap_width(&self.todos[i].title, inner_width).len().max(1))
    .sum();
let has_footer = if self.expanded {
    self.todos.len() > MAX_VISIBLE_TODOS
} else {
    self.todos.len() > indices.len()
};
let total = 2 + item_lines + usize::from(has_footer);
```

The `2` is border plus `Todo` header. Do not keep the previous extra `+ 1`; render does not emit a blank separator.

- [ ] **Step 6: Add expansion state to `NeoChromeState`**

In `crates/neo-tui/src/shell/state.rs`, add the field:

```rust
pub(super) todo_panel_expanded: bool,
```

Initialize it in `NeoChromeState::new`:

```rust
todo_panel_expanded: false,
```

Add methods near the existing todo methods:

```rust
#[must_use]
pub const fn todo_panel_expanded(&self) -> bool {
    self.todo_panel_expanded
}

#[must_use]
pub fn todo_panel_has_overflow(&self) -> bool {
    self.todo_items.len() > crate::widgets::MAX_VISIBLE_TODOS
}

pub const fn set_todo_panel_expanded(&mut self, expanded: bool) {
    self.todo_panel_expanded = expanded;
}

pub fn toggle_todo_panel_expanded(&mut self) {
    self.todo_panel_expanded = !self.todo_panel_expanded;
}
```

Update `clear_todos()`:

```rust
pub fn clear_todos(&mut self) {
    self.todo_items.clear();
    self.todo_panel_expanded = false;
}
```

Do not reset expansion in `set_todo_items`; Kimi keeps expanded state across list updates.

- [ ] **Step 7: Pass expansion state into chrome rendering**

In both `render_chrome_lines` and `render_chrome_lines_mut`, change todo panel construction to:

```rust
TodoPanel::new(app.todo_items())
    .with_theme(app.theme())
    .expanded(app.todo_panel_expanded())
    .render(content_width)
```

- [ ] **Step 8: Run focused tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 9: Commit checkpoint**

Stop and ask the user whether to run exactly:

```bash
git add crates/neo-tui/src/widgets/todo_panel.rs crates/neo-tui/src/shell/state.rs crates/neo-tui/src/transcript/chrome_render.rs crates/neo-tui/tests/app_shell.rs
git commit -m "feat(tui): expand todo panel"
```

If authorization is not granted, continue without committing.

## Task 3: Add Ctrl+T Keybinding and Controller Routing

**Files:**
- Modify: `crates/neo-tui/src/input/keybinding.rs`
- Modify: `crates/neo-tui/tests/primitives.rs`
- Modify: `crates/neo-agent/src/modes/interactive/keybinding_priority.rs`
- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing keybinding test**

In `crates/neo-tui/tests/primitives.rs`, inside `keybinding_manager_matches_defaults_overrides_and_conflicts`, add:

```rust
    assert!(manager.matches(
        &KeyId::new("ctrl+t").expect("valid key"),
        KeybindingAction::TodoPanelToggle
    ));
    assert_eq!(
        KeybindingAction::from_id("tui.todo.toggle"),
        Some(KeybindingAction::TodoPanelToggle)
    );
```

- [ ] **Step 2: Add failing controller tests**

In `crates/neo-agent/src/modes/interactive/tests.rs`, near the existing `ctrl+o` tool output toggle test, add:

```rust
    #[tokio::test]
    async fn event_loop_ctrl_t_expands_overflowing_todo_panel() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.chrome_mut().set_todo_items((0..7).map(|index| {
            neo_tui::widgets::TodoDisplayItem::new(
                format!("task-{index}"),
                neo_tui::widgets::TodoDisplayStatus::Pending,
            )
        }).collect());

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
            .await
            .expect("ctrl-t toggles todo panel");

        assert!(controller.chrome().todo_panel_expanded());

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
            .await
            .expect("ctrl-t toggles todo panel back");

        assert!(!controller.chrome().todo_panel_expanded());
    }

    #[tokio::test]
    async fn event_loop_ctrl_t_is_noop_when_todo_panel_does_not_overflow() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.chrome_mut().set_todo_items(vec![
            neo_tui::widgets::TodoDisplayItem::new(
                "task-0",
                neo_tui::widgets::TodoDisplayStatus::Pending,
            ),
        ]);

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
            .await
            .expect("ctrl-t noop succeeds");

        assert!(!controller.chrome().todo_panel_expanded());
        assert_eq!(controller.chrome().prompt().text, "");
    }
```

- [ ] **Step 3: Run tests and verify they fail**

Run:

```bash
```

Expected: FAIL. The keybinding test should fail because `TodoPanelToggle` does not exist.

- [ ] **Step 4: Add keybinding action and default binding**

In `crates/neo-tui/src/input/keybinding.rs`, add enum variant after `ToolOutputToggle`:

```rust
    TodoPanelToggle,
```

Add action id after `ToolOutputToggle`:

```rust
    (KeybindingAction::TodoPanelToggle, "tui.todo.toggle"),
```

Add default binding in `input_keybinding_definitions()` after `CycleDevelopmentMode`:

```rust
        definition(
            Action::TodoPanelToggle,
            &["ctrl+t"],
            "Expand or collapse the todo panel",
        ),
```

This keeps it in the input context because the todo panel lives in chrome before the prompt, not in transcript history.

- [ ] **Step 5: Add editing priority**

In `crates/neo-agent/src/modes/interactive/keybinding_priority.rs`, add `TodoPanelToggle` after `ToolOutputToggle`:

```rust
    KeybindingAction::ToolOutputToggle,
    KeybindingAction::TodoPanelToggle,
    KeybindingAction::ModelPickerOpen,
```

Do not add it to overlay, question, or prompt-completion priority arrays. Blocking dialogs should own input while focused.

- [ ] **Step 6: Handle the action in the controller**

In `crates/neo-agent/src/modes/interactive/input.rs`, inside `handle_basic_keybinding_action`, add this arm after `ToolOutputToggle`:

```rust
            KeybindingAction::TodoPanelToggle => {
                if self.tui.chrome().todo_panel_has_overflow() {
                    self.clear_pending_exit_confirmation();
                    self.tui.chrome_mut().toggle_todo_panel_expanded();
                }
            }
```

The action returns `Ok(Some(false))` through the existing match tail. This consumes `Ctrl+T` without editing prompt text even when there is no overflow. Kimi lets the editor default see non-overflow `Ctrl+T`; Neo's keybinding parser already converted the control sequence into a semantic key, so a no-op is the smallest product-equivalent behavior.

- [ ] **Step 7: Run focused keybinding tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 8: Commit checkpoint**

Stop and ask the user whether to run exactly:

```bash
git add crates/neo-tui/src/input/keybinding.rs crates/neo-tui/tests/primitives.rs crates/neo-agent/src/modes/interactive/keybinding_priority.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat(tui): bind ctrl-t to todo panel"
```

If authorization is not granted, continue without committing.

## Task 4: Docs and Final Verification

**Files:**
- Modify: `docs/config.md`
- Verify: `crates/neo-tui/src/widgets/todo_panel.rs`
- Verify: `crates/neo-tui/tests/app_shell.rs`
- Verify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Document the new keybinding override**

In `docs/config.md`, in the `[tui.keybindings]` example or nearby keybinding list, add this row or TOML line beside existing TUI keybindings:

```toml
"tui.todo.toggle" = ["ctrl+t"] # Expand/collapse an overflowing todo panel
```

If the existing section uses prose instead of TOML-only examples, add this sentence:

```markdown
`tui.todo.toggle` defaults to `ctrl+t` and expands or collapses the todo panel when more than five todo items exist.
```

- [ ] **Step 2: Run doc/keybinding validation tests**

Run:

```bash
```

Expected: PASS. This catches unsupported action id regressions through config loader tests.

- [ ] **Step 3: Run the focused final test set**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 4: Run formatting/check gate for touched crates**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS.

- [ ] **Step 5: Store ICM completion memory**

Run:

```bash
icm store -t context-neo -c "Implemented Neo todo panel Ctrl+T expand/collapse: collapsed view uses Kimi-compatible five-row smart selector, expanded view renders all todo items, ctrl+t toggles through configurable keybinding action tui.todo.toggle, and docs/config.md documents the override." -i high -k "todo-panel,ctrl-t,expand,neo-tui,keybinding"
```

Expected: output starts with `Stored:`.

- [ ] **Step 6: Commit checkpoint**

Stop and ask the user whether to run exactly:

```bash
git add docs/config.md
git commit -m "docs: document todo panel keybinding"
```

If authorization is not granted, leave changes uncommitted and report verification evidence.

## Acceptance Criteria

- Collapsed todo panel shows at most five todo items.
- Collapsed selector includes in-progress items first, then earliest pending items while reserving one slot for the latest done item when both done and pending exist.
- All-pending lists show the first five items.
- All-done lists show the last five items.
- Collapsed footer includes `ctrl+t to expand` and hidden status distribution when items are hidden.
- Expanded panel shows every todo item in original order and footer `all N items · ctrl+t to collapse`.
- `Ctrl+T` toggles only via keybinding action `tui.todo.toggle`.
- Focused overlays and blocking dialogs do not receive the todo toggle through their priority arrays.
- `clear_todos()` resets expanded state; ordinary `set_todo_items()` preserves it.

## Self-Review

- Spec coverage: The plan covers Kimi selector behavior, footer text, expanded rendering, `Ctrl+T` binding, chrome state, and docs.
- Placeholder scan: No implementation step relies on unspecified behavior; code snippets define new structs, methods, action ids, and tests.
- Type consistency: The plan consistently uses `TodoPanelToggle`, `tui.todo.toggle`, `todo_panel_expanded`, `todo_panel_has_overflow`, and `VisibleTodos { indices, hidden, hidden_counts }`.
