# Neo Slash Help and Completion UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a real `/help` slash command with a Neo-native help panel, and make slash command completion show clean command descriptions without internal metadata.

**Architecture:** Use the existing slash completion catalog as the single source for visible slash commands, including dynamic `/skill:<name>` entries. Add a `HelpPanelState` rich dialog in `neo-tui`, open it from `/help` in `neo-agent`, and keep completion ranking/source metadata internal instead of rendering it into descriptions.

**Tech Stack:** Rust 2024, `neo-agent` interactive controller, `neo-tui` shell overlays/dialogs, existing `PickerItem`, `SkillStore`, `InputEvent`, `InputResult`, ANSI styling via `TuiTheme`, `Style`, and `paint`.

---

## Scope And Policy

- Do not modify vendored reference code under `docs/kimi-code`; it was read only for behavior reference.
- Do not introduce two display modes for completion metadata. Delete the user-visible `provider/trust/source` formatting path.
- Do not run git mutations during implementation unless the user explicitly authorizes them for that execution instance. In this repo, that means no `git add`, `git commit`, `git stash`, `git reset`, `git checkout --`, `git clean`, `git rebase`, `git push`, branch deletion, or similar mutation.
- Verification must stay narrow. Use exact function-level commands listed in each task.

## File Structure

- Modify `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
  - Own the slash command catalog used by completion and help.
  - Remove user-visible `provider/trust/source` formatting.
  - Keep `CompletionSource` only for sorting.

- Modify `crates/neo-agent/src/modes/interactive/slash_commands.rs`
  - Handle `/help`.
  - Build help commands from `session_completion_items(self.skill_store.as_ref())`.
  - Open the TUI help overlay and clear the submitted prompt.

- Modify `crates/neo-agent/src/modes/interactive/tests.rs`
  - Update old completion metadata expectations.
  - Add metadata cleanup, `/help`, and dynamic `/skill:<name>` coverage in the tasks where those behaviors become executable.

- Create `crates/neo-tui/src/dialogs/help_panel.rs`
  - Implement a scrollable help panel with keyboard shortcuts and slash commands.
  - Render command labels with primary/selected styling and descriptions with muted styling.
  - Close on `Esc`, `Enter`, `q`, or `Q`; scroll on arrows, page keys, and mouse wheel events.

- Modify `crates/neo-tui/src/dialogs/mod.rs`
  - Export `HelpPanelCommand`, `HelpPanelOptions`, and `HelpPanelState`.

- Modify `crates/neo-tui/src/shell/overlay.rs`
  - Add `OverlayKind::HelpPanel`.
  - Render it as a rich dialog.
  - Give it a fixed overlay height.

- Modify `crates/neo-tui/src/shell/dialog_factory.rs`
  - Add `NeoChromeState::open_help_panel(...)`.

- Modify `crates/neo-tui/src/shell/input_dispatch.rs`
  - Route input to `HelpPanelState`.
  - Close the overlay directly when the help panel returns `Submitted` or `Cancelled`.

- Modify `crates/neo-tui/src/shell/mod.rs`
  - Treat help as a rich, prompt-blocking overlay.
  - Add a small shell-level test for open/render/height/blocking behavior.

- Modify `crates/neo-tui/src/shell/select_list.rs`
  - Style picker labels/descriptions with ANSI colors using `TuiTheme`.
  - Keep existing layout and filtering semantics.

- Modify `crates/neo-tui/src/shell/pickers.rs`
  - Pass a theme into picker rendering.

- Modify `crates/neo-tui/src/transcript/chrome_render.rs`
  - Pass the app theme to prompt completion dropdown rendering.

---

### Task 1: Clean Slash Completion Data

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Update the failing completion test expectations**

In `crates/neo-agent/src/modes/interactive/tests.rs`, replace the body of `prompt_completions_merges_real_prompt_package_and_session_commands` from the first `assert!(by_value["/review"]...` through the `/resume` assertion with:

```rust
    assert_eq!(
        by_value["/review"].description.as_deref(),
        Some("Review local changes")
    );
    assert_eq!(
        by_value["/refactor"].description.as_deref(),
        Some("Refactor from package")
    );
    assert_eq!(
        by_value["/resume"].description.as_deref(),
        Some("Resume a local session")
    );
    for item in by_value.values() {
        if let Some(description) = item.description.as_deref() {
            assert!(
                !description.contains("source:")
                    && !description.contains("provider:")
                    && !description.contains("trust:"),
                "completion description should be user-facing only: {item:?}"
            );
        }
    }
```

Add this test near `slash_completions_include_permission_commands`:

```rust
#[test]
fn slash_completion_descriptions_hide_internal_metadata() {
    let completions = prompt_completions(&test_workspace_root(), "/ask", &[], None, true)
        .expect("completions resolve");
    let ask = completions
        .iter()
        .find(|item| item.value == "/ask")
        .expect("missing /ask completion");

    assert_eq!(ask.label, "/ask");
    assert_eq!(ask.description.as_deref(), Some("ask permission mode"));
    let description = ask.description.as_deref().unwrap_or_default();
    assert!(!description.contains("provider:"), "{description}");
    assert!(!description.contains("trust:"), "{description}");
    assert!(!description.contains("source:"), "{description}");
}
```

- [ ] **Step 2: Run the first failing test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::prompt_completions_merges_real_prompt_package_and_session_commands --exact --nocapture --include-ignored
```

Expected: FAIL, because descriptions still contain `source:`, `provider:`, and `trust:`.

- [ ] **Step 3: Simplify the slash command catalog**

In `crates/neo-agent/src/modes/interactive/prompt_completion.rs`, replace:

```rust
static STATIC_SLASH_COMMANDS: &[(&str, &str, Option<&str>, Option<&str>)] = &[
```

with:

```rust
static STATIC_SLASH_COMMANDS: &[(&str, &str)] = &[
```

Replace the entire `STATIC_SLASH_COMMANDS` initializer with:

```rust
static STATIC_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/resume", "Resume a local session"),
    ("/new", "Start a fresh local session"),
    ("/clear", "Alias for /new"),
    ("/model", "Switch active model"),
    ("/provider", "View configured providers"),
    ("/mcp", "View and manage MCP servers"),
    ("/tasks", "View active background tasks"),
    ("/plan", "Toggle plan mode (on / off / clear)"),
    ("/compact", "Request manual context compaction"),
    ("/permissions", "select permission mode"),
    ("/ask", "ask permission mode"),
    ("/auto", "auto permission mode"),
    ("/yolo", "yolo permission mode"),
    ("/btw", "Open a temporary side-question panel"),
];
```

Replace `session_completion_items` with:

```rust
pub(super) fn session_completion_items(skill_store: Option<&SkillStore>) -> Vec<PickerItem> {
    let mut items: Vec<PickerItem> = STATIC_SLASH_COMMANDS
        .iter()
        .map(|(value, description)| {
            PickerItem::new(
                (*value).to_owned(),
                (*value).to_owned(),
                Some((*description).to_owned()),
            )
        })
        .collect();

    if let Some(skill_store) = skill_store {
        for skill in skill_store.iter() {
            let value = format!("/skill:{}", skill.name);
            items.push(PickerItem::new(
                value.clone(),
                value,
                Some(skill.manifest.description.clone()),
            ));
        }
    }

    items
}
```

In `prompt_package_completion_items`, replace the `let description = prompt_source_description(...)` block with:

```rust
            let description = (!command.template.description.is_empty())
                .then_some(command.template.description);
```

Then replace:

```rust
            Some(PickerItem::new(value.clone(), value, Some(description)))
```

with:

```rust
            Some(PickerItem::new(value.clone(), value, description))
```

Delete the `prompt_source_description` function completely.

- [ ] **Step 4: Stop rendering internal completion source labels**

In `CompletionCandidate`, remove the `source_label` field:

```rust
pub(super) struct CompletionCandidate {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) description: Option<String>,
    pub(super) source: CompletionSource,
}
```

Update `CompletionCandidate::new` to stop assigning `source_label`:

```rust
    fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<String>,
        source: CompletionSource,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description,
            source,
        }
    }
```

Replace `CompletionCandidate::to_picker_item` with:

```rust
    pub(super) fn to_picker_item(&self) -> PickerItem {
        PickerItem::new(
            self.value.clone(),
            self.label.clone(),
            self.description.clone(),
        )
    }
```

Delete `CompletionSource::label` and delete `completion_description`.

- [ ] **Step 5: Run the exact completion tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::prompt_completions_merges_real_prompt_package_and_session_commands --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completion_descriptions_hide_internal_metadata --exact --nocapture --include-ignored
```

Expected: all PASS.

- [ ] **Step 6: Checkpoint**

Do not run git commands unless the user has explicitly authorized git mutation for this execution instance. If authorization is granted later, this task's logical commit message is:

```text
fix: simplify slash completion descriptions
```

---

### Task 2: Add The Neo Help Panel Dialog

**Files:**
- Create: `crates/neo-tui/src/dialogs/help_panel.rs`
- Modify: `crates/neo-tui/src/dialogs/mod.rs`
- Test: `crates/neo-tui/src/dialogs/help_panel.rs`

- [ ] **Step 1: Create failing tests and the dialog skeleton**

Create `crates/neo-tui/src/dialogs/help_panel.rs` with this full initial content:

```rust
use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::{InputResult, Style, paint, truncate_width, visible_width};
use crate::primitive::theme::TuiTheme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpPanelCommand {
    pub value: String,
    pub description: Option<String>,
}

impl HelpPanelCommand {
    #[must_use]
    pub fn new(value: impl Into<String>, description: Option<impl Into<String>>) -> Self {
        Self {
            value: value.into(),
            description: description.map(Into::into),
        }
    }
}

pub struct HelpPanelOptions {
    pub commands: Vec<HelpPanelCommand>,
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpPanelState {
    commands: Vec<HelpPanelCommand>,
    theme: TuiTheme,
    scroll_offset: usize,
    viewport_height: usize,
}

impl HelpPanelState {
    #[must_use]
    pub fn new(opts: HelpPanelOptions) -> Self {
        let mut commands = opts.commands;
        commands.sort_by(|left, right| help_command_sort_key(left).cmp(&help_command_sort_key(right)));
        Self {
            commands,
            theme: opts.theme,
            scroll_offset: 0,
            viewport_height: 12,
        }
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        Vec::new()
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        InputResult::Ignored
    }
}

fn help_command_sort_key(command: &HelpPanelCommand) -> (bool, &str) {
    (command.value.starts_with("/skill:"), command.value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible_lines(state: &HelpPanelState, width: usize) -> Vec<String> {
        state
            .render_lines(width)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect()
    }

    #[test]
    fn help_panel_renders_shortcuts_commands_and_skill_commands() {
        let state = HelpPanelState::new(HelpPanelOptions {
            commands: vec![
                HelpPanelCommand::new("/skill:refactor", Some("Refactor with project conventions")),
                HelpPanelCommand::new("/ask", Some("ask permission mode")),
                HelpPanelCommand::new("/help", Some("Show help information")),
            ],
            theme: TuiTheme::default(),
        });

        let visible = visible_lines(&state, 80).join("\n");
        assert!(visible.contains("help · Esc / Enter / q close · ↑↓ scroll"), "{visible}");
        assert!(visible.contains("Keyboard"), "{visible}");
        assert!(visible.contains("Shift+Tab"), "{visible}");
        assert!(visible.contains("Slash Commands"), "{visible}");
        assert!(visible.contains("/help"), "{visible}");
        assert!(visible.contains("Show help information"), "{visible}");
        assert!(visible.contains("/ask"), "{visible}");
        assert!(visible.contains("ask permission mode"), "{visible}");
        assert!(visible.contains("/skill:refactor"), "{visible}");
        assert!(visible.contains("Refactor with project conventions"), "{visible}");
    }

    #[test]
    fn help_panel_sorts_skill_commands_after_regular_commands() {
        let state = HelpPanelState::new(HelpPanelOptions {
            commands: vec![
                HelpPanelCommand::new("/skill:zeta", Some("zeta skill")),
                HelpPanelCommand::new("/auto", Some("auto permission mode")),
                HelpPanelCommand::new("/skill:alpha", Some("alpha skill")),
                HelpPanelCommand::new("/ask", Some("ask permission mode")),
            ],
            theme: TuiTheme::default(),
        });

        let visible = visible_lines(&state, 90).join("\n");
        let ask = visible.find("/ask").expect("/ask visible");
        let auto = visible.find("/auto").expect("/auto visible");
        let alpha = visible.find("/skill:alpha").expect("/skill:alpha visible");
        let zeta = visible.find("/skill:zeta").expect("/skill:zeta visible");
        assert!(ask < alpha, "{visible}");
        assert!(auto < alpha, "{visible}");
        assert!(alpha < zeta, "{visible}");
    }

    #[test]
    fn help_panel_scrolls_and_closes() {
        let mut state = HelpPanelState::new(HelpPanelOptions {
            commands: (0..30)
                .map(|index| {
                    HelpPanelCommand::new(
                        format!("/cmd-{index:02}"),
                        Some(format!("command {index:02}")),
                    )
                })
                .collect(),
            theme: TuiTheme::default(),
        });

        let before = visible_lines(&state, 80).join("\n");
        assert!(before.contains("/cmd-00"), "{before}");
        assert_eq!(
            state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown)),
            InputResult::Handled
        );
        let after = visible_lines(&state, 80).join("\n");
        assert_ne!(before, after);
        assert_eq!(
            state.handle_input(&InputEvent::Action(KeybindingAction::SelectCancel)),
            InputResult::Cancelled
        );
        assert_eq!(state.handle_input(&InputEvent::Submit), InputResult::Submitted);
        assert_eq!(state.handle_input(&InputEvent::Insert('q')), InputResult::Cancelled);
    }
}
```

- [ ] **Step 2: Export the skeleton so tests compile far enough to fail behaviorally**

In `crates/neo-tui/src/dialogs/mod.rs`, add:

```rust
pub mod help_panel;
```

and add:

```rust
pub use help_panel::{HelpPanelCommand, HelpPanelOptions, HelpPanelState};
```

- [ ] **Step 3: Run the failing render test**

Run:

```bash
cargo test --package neo-tui --lib dialogs::help_panel::tests::help_panel_renders_shortcuts_commands_and_skill_commands --exact --nocapture
```

Expected: FAIL because `render_lines` returns no content.

- [ ] **Step 4: Implement help panel rendering**

In `crates/neo-tui/src/dialogs/help_panel.rs`, replace `render_lines` and add helper functions below `help_command_sort_key`:

```rust
    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        if width < 8 {
            return Vec::new();
        }

        let inner_width = width.saturating_sub(2).max(1);
        let border_style = Style::default().fg(self.theme.overlay_border);
        let title_style = Style::default().fg(self.theme.text_primary).bold();
        let heading_style = Style::default().fg(self.theme.brand).bold();
        let muted_style = Style::default().fg(self.theme.text_muted);

        let mut body = Vec::new();
        body.push(render_plain_row("Keyboard", heading_style));
        body.push(render_pair_row("Shift+Tab", "cycle development mode", self.theme, inner_width));
        body.push(render_pair_row("Ctrl+S", "steer next break point", self.theme, inner_width));
        body.push(render_pair_row("Alt+Up", "edit last queued message", self.theme, inner_width));
        body.push(render_pair_row("Esc", "cancel dialog or interrupt turn", self.theme, inner_width));
        body.push(String::new());
        body.push(render_plain_row("Slash Commands", heading_style));
        for command in &self.commands {
            body.push(render_pair_row(
                &command.value,
                command.description.as_deref().unwrap_or_default(),
                self.theme,
                inner_width,
            ));
        }

        let viewport_height = self.viewport_height.min(body.len()).max(1);
        let max_scroll = body.len().saturating_sub(viewport_height);
        let start = self.scroll_offset.min(max_scroll);
        let end = (start + viewport_height).min(body.len());

        let mut lines = Vec::with_capacity(viewport_height + 4);
        lines.push(paint(
            &format!("┌{}┐", "─".repeat(inner_width)),
            border_style,
        ));
        lines.push(box_line(
            " help · Esc / Enter / q close · ↑↓ scroll",
            inner_width,
            title_style,
            border_style,
        ));
        for row in &body[start..end] {
            lines.push(box_line_raw(row, inner_width, border_style));
        }
        let position = if body.len() > viewport_height {
            format!(" {}/{} · PgUp/PgDn", start + 1, body.len())
        } else {
            String::new()
        };
        lines.push(box_line(&position, inner_width, muted_style, border_style));
        lines.push(paint(
            &format!("└{}┘", "─".repeat(inner_width)),
            border_style,
        ));
        lines
    }
```

Add these helpers below `help_command_sort_key`:

```rust
fn render_plain_row(text: &str, style: Style) -> String {
    paint(text, style)
}

fn render_pair_row(label: &str, description: &str, theme: TuiTheme, width: usize) -> String {
    let label_style = Style::default().fg(theme.text_primary);
    let description_style = Style::default().fg(theme.text_muted);
    let label_width = 22usize.min(width.saturating_sub(6)).max(1);
    let fitted_label = truncate_width(label, label_width, "", false);
    let spacing = " ".repeat(label_width.saturating_sub(visible_width(&fitted_label)).max(1));
    let used = visible_width(&fitted_label) + spacing.len();
    let remaining = width.saturating_sub(used).max(1);
    let fitted_description = truncate_width(description, remaining, "", false);
    format!(
        "{}{}{}",
        paint(&fitted_label, label_style),
        spacing,
        paint(&fitted_description, description_style)
    )
}

fn box_line(text: &str, inner_width: usize, style: Style, border_style: Style) -> String {
    let content = truncate_width(text, inner_width, "", false);
    let padding = " ".repeat(inner_width.saturating_sub(visible_width(&content)));
    format!(
        "{}{}{}{}",
        paint("│", border_style),
        paint(&content, style),
        padding,
        paint("│", border_style)
    )
}

fn box_line_raw(raw: &str, inner_width: usize, border_style: Style) -> String {
    let visible = visible_width(&crate::primitive::strip_ansi(raw));
    let padding = " ".repeat(inner_width.saturating_sub(visible));
    format!(
        "{}{}{}{}",
        paint("│", border_style),
        raw,
        padding,
        paint("│", border_style)
    )
}
```

- [ ] **Step 5: Implement help panel input handling**

Replace `handle_input` with:

```rust
    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        match input {
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                InputResult::Handled
            }
            InputEvent::ScrollUp(rows) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(*rows);
                InputResult::Handled
            }
            InputEvent::ScrollDown(rows) => {
                self.scroll_offset = self.scroll_offset.saturating_add(*rows);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageUp) | InputEvent::MoveLeft => {
                self.scroll_offset = self.scroll_offset.saturating_sub(self.viewport_height);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageDown) | InputEvent::MoveRight => {
                self.scroll_offset = self.scroll_offset.saturating_add(self.viewport_height);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                InputResult::Submitted
            }
            InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                InputResult::Cancelled
            }
            InputEvent::Insert('q' | 'Q') => InputResult::Cancelled,
            _ => InputResult::Ignored,
        }
    }
```

- [ ] **Step 6: Run help panel unit tests**

Run:

```bash
cargo test --package neo-tui --lib dialogs::help_panel::tests::help_panel_renders_shortcuts_commands_and_skill_commands --exact --nocapture
cargo test --package neo-tui --lib dialogs::help_panel::tests::help_panel_sorts_skill_commands_after_regular_commands --exact --nocapture
cargo test --package neo-tui --lib dialogs::help_panel::tests::help_panel_scrolls_and_closes --exact --nocapture
```

Expected: all PASS.

- [ ] **Step 7: Checkpoint**

Do not run git commands unless the user has explicitly authorized git mutation for this execution instance. If authorization is granted later, this task's logical commit message is:

```text
feat: add help panel dialog
```

---

### Task 3: Wire Help Panel Into Shell Overlays

**Files:**
- Modify: `crates/neo-tui/src/shell/overlay.rs`
- Modify: `crates/neo-tui/src/shell/dialog_factory.rs`
- Modify: `crates/neo-tui/src/shell/input_dispatch.rs`
- Modify: `crates/neo-tui/src/shell/mod.rs`
- Test: `crates/neo-tui/src/shell/mod.rs`

- [ ] **Step 1: Add a failing shell integration test**

In the `#[cfg(test)] mod tests` block at the bottom of `crates/neo-tui/src/shell/mod.rs`, add:

```rust
    #[test]
    fn help_panel_overlay_opens_as_rich_dialog_and_blocks_prompt() {
        let mut chrome = NeoChromeState::new("title", "session", "model", "/tmp");
        let id = chrome.open_help_panel(vec![
            crate::dialogs::HelpPanelCommand::new("/help", Some("Show help information")),
            crate::dialogs::HelpPanelCommand::new("/skill:refactor", Some("Refactor safely")),
        ]);

        assert_eq!(chrome.focused_overlay_id(), Some(id));
        assert!(chrome.focused_overlay_is_rich_dialog());
        assert!(chrome.focused_overlay_blocks_prompt());
        assert_eq!(chrome.focused_overlay_height(), 16);

        let visible = chrome
            .focused_overlay_lines(80)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(visible.contains("help · Esc / Enter / q close"), "{visible}");
        assert!(visible.contains("/help"), "{visible}");
        assert!(visible.contains("/skill:refactor"), "{visible}");

        assert_eq!(
            chrome.handle_focused_dialog_input(crate::input::InputEvent::Insert('q')),
            crate::primitive::InputResult::Cancelled
        );
        assert!(chrome.focused_overlay().is_none());
    }
```

- [ ] **Step 2: Run the failing shell integration test**

Run:

```bash
cargo test --package neo-tui --lib shell::tests::help_panel_overlay_opens_as_rich_dialog_and_blocks_prompt --exact --nocapture
```

Expected: FAIL because `open_help_panel` and `OverlayKind::HelpPanel` do not exist.

- [ ] **Step 3: Add `HelpPanel` to `OverlayKind`**

In `crates/neo-tui/src/shell/overlay.rs`, add `HelpPanelState` to the imports:

```rust
    ApiKeyInputState, ChoicePickerState, CustomRegistryImportState, HelpPanelState,
    McpAddFormState, McpManagerState, ModelSelectorState, ProviderManagerState,
    TabbedModelSelectorState, TextInputState, TrustDialogState,
```

Add the enum variant:

```rust
    HelpPanel(HelpPanelState),
```

In `rich_dialog_lines`, add:

```rust
            Self::HelpPanel(state) => Some(state.render_lines(width)),
```

In `input_dialog_height`, include the help panel in the `Some(16)` group:

```rust
            | Self::HelpPanel(_) => Some(16),
```

- [ ] **Step 4: Add `open_help_panel`**

In `crates/neo-tui/src/shell/dialog_factory.rs`, add this method in the rich dialog section:

```rust
    pub fn open_help_panel(
        &mut self,
        commands: Vec<crate::dialogs::HelpPanelCommand>,
    ) -> OverlayId {
        let state = crate::dialogs::HelpPanelState::new(crate::dialogs::HelpPanelOptions {
            commands,
            theme: self.theme,
        });
        self.push_overlay(Overlay::new("help", OverlayKind::HelpPanel(state)))
    }
```

- [ ] **Step 5: Route help input and close behavior**

In `crates/neo-tui/src/shell/input_dispatch.rs`, add a match arm in `handle_focused_dialog_input`:

```rust
            OverlayKind::HelpPanel(state) => {
                let result = state.handle_input(&input);
                if matches!(result, InputResult::Submitted | InputResult::Cancelled) {
                    close_overlay = true;
                }
                result
            }
```

Keep this arm before the final `_ => InputResult::Ignored`.

- [ ] **Step 6: Mark help as rich and prompt-blocking**

In `crates/neo-tui/src/shell/dialog_factory.rs`, add `OverlayKind::HelpPanel(_)` to `focused_overlay_is_rich_dialog`.

In `crates/neo-tui/src/shell/mod.rs`, add `OverlayKind::HelpPanel(_)` to `focused_overlay_blocks_prompt`.

- [ ] **Step 7: Run the shell integration test**

Run:

```bash
cargo test --package neo-tui --lib shell::tests::help_panel_overlay_opens_as_rich_dialog_and_blocks_prompt --exact --nocapture
```

Expected: PASS.

- [ ] **Step 8: Checkpoint**

Do not run git commands unless the user has explicitly authorized git mutation for this execution instance. If authorization is granted later, this task's logical commit message is:

```text
feat: wire help panel overlay
```

---

### Task 4: Open Help From `/help`

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing tests for `/help` completion and dispatch**

In `crates/neo-agent/src/modes/interactive/tests.rs`, add this near the existing slash completion tests:

```rust
#[test]
fn slash_completions_include_help_command() {
    let completions = prompt_completions(&test_workspace_root(), "/", &[], None, true)
        .expect("completions resolve");
    let help = completions
        .iter()
        .find(|item| item.value == "/help")
        .expect("missing /help completion");

    assert_eq!(help.label, "/help");
    assert_eq!(help.description.as_deref(), Some("Show help information"));
}
```

In `crates/neo-agent/src/modes/interactive/tests.rs`, add this near the existing slash overlay tests such as `/resume`:

```rust
#[tokio::test]
async fn slash_help_opens_help_panel_overlay() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured = std::sync::Arc::clone(&captured);
            async move {
                captured.lock().expect("recorded requests").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/help");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash help command runs locally");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::HelpPanel(_))
    ));
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(requests.lock().expect("recorded requests").is_empty());

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
        .await
        .expect("scroll help panel");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
        .await
        .expect("scroll help panel again");

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("help · Esc / Enter / q close"), "{snapshot}");
    assert!(snapshot.contains("/help"), "{snapshot}");
    assert!(snapshot.contains("/ask"), "{snapshot}");
}
```

- [ ] **Step 2: Run the failing `/help` tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_include_help_command --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_help_opens_help_panel_overlay --exact --nocapture --include-ignored
```

Expected: the completion test fails with `missing /help completion`; the controller test fails because `/help` is not dispatched.

- [ ] **Step 3: Implement `/help` dispatch**

In `crates/neo-agent/src/modes/interactive/prompt_completion.rs`, add `/help` to `STATIC_SLASH_COMMANDS`:

```rust
    ("/help", "Show help information"),
```

In `crates/neo-agent/src/modes/interactive/slash_commands.rs`, add `use` imports:

```rust
use neo_tui::dialogs::HelpPanelCommand;
```

In `handle_simple_slash_command`, add this match arm:

```rust
            "/help" => self.open_help_panel(),
```

Add this helper method in the `impl InteractiveController` block near other small slash helpers:

```rust
    fn open_help_panel(&mut self) {
        let commands = super::session_completion_items(self.skill_store.as_ref())
            .into_iter()
            .map(|item| HelpPanelCommand::new(item.value, item.description))
            .collect();
        self.tui.chrome_mut().open_help_panel(commands);
    }
```

The existing `handle_simple_slash_command` tail will call `self.clear_submitted_prompt(); true`, so do not clear the prompt inside `open_help_panel`.

- [ ] **Step 4: Run the `/help` controller test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_help_opens_help_panel_overlay --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_include_help_command --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 5: Checkpoint**

Do not run git commands unless the user has explicitly authorized git mutation for this execution instance. If authorization is granted later, this task's logical commit message is:

```text
feat: open help panel from slash command
```

---

### Task 5: Include Dynamic Skills In Help And Completion

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add a failing dynamic skill help/completion test**

In `crates/neo-agent/src/modes/interactive/tests.rs`, extend the imports near the top:

```rust
use neo_agent_core::skills::{LoadedSkill, SkillManifest, SkillSource, SkillStore, SkillType};
```

If this conflicts with existing grouped imports, merge it into the current `neo_agent_core` imports instead of duplicating names.

Add this helper near `test_workspace_root()`:

```rust
fn test_skill_store() -> SkillStore {
    SkillStore::load(
        &[],
        &[],
        vec![LoadedSkill {
            name: "refactor".to_owned(),
            root: PathBuf::from("builtin/refactor"),
            manifest: SkillManifest {
                name: "refactor".to_owned(),
                description: "Refactor with project conventions".to_owned(),
                skill_type: SkillType::Prompt,
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::new(),
                slash_commands: Vec::new(),
            },
            body: "Refactor safely.".to_owned(),
            source: SkillSource::Builtin,
        }],
    )
    .expect("skill store")
}
```

Add this test near other slash completion tests:

```rust
#[test]
fn slash_completions_include_dynamic_skill_commands_without_metadata() {
    let skill_store = test_skill_store();
    let completions = prompt_completions(
        &test_workspace_root(),
        "/skill:",
        &[],
        Some(&skill_store),
        true,
    )
    .expect("skill completions resolve");
    let skill = completions
        .iter()
        .find(|item| item.value == "/skill:refactor")
        .expect("missing dynamic skill command");

    assert_eq!(skill.label, "/skill:refactor");
    assert_eq!(
        skill.description.as_deref(),
        Some("Refactor with project conventions")
    );
    let description = skill.description.as_deref().unwrap_or_default();
    assert!(!description.contains("provider:"), "{description}");
    assert!(!description.contains("trust:"), "{description}");
    assert!(!description.contains("source:"), "{description}");
}
```

Add this controller test after `slash_help_opens_help_panel_overlay`:

```rust
#[tokio::test]
async fn slash_help_panel_includes_dynamic_skill_commands() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.skill_store = Some(test_skill_store());

    controller.type_text("/help");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash help command runs locally");

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("/skill:refactor"), "{snapshot}");
    assert!(
        snapshot.contains("Refactor with project conventions"),
        "{snapshot}"
    );
}
```

- [ ] **Step 2: Run the dynamic skill tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_include_dynamic_skill_commands_without_metadata --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_help_panel_includes_dynamic_skill_commands --exact --nocapture --include-ignored
```

Expected: both PASS if Tasks 1-4 were implemented correctly.

- [ ] **Step 3: Checkpoint**

Do not run git commands unless the user has explicitly authorized git mutation for this execution instance. If authorization is granted later, this task's logical commit message is:

```text
test: cover dynamic skill help commands
```

---

### Task 6: Style Slash Completion Like Kimi Without Metadata

**Files:**
- Modify: `crates/neo-tui/src/shell/select_list.rs`
- Modify: `crates/neo-tui/src/shell/pickers.rs`
- Modify: `crates/neo-tui/src/shell/overlay.rs`
- Modify: `crates/neo-tui/src/transcript/chrome_render.rs`
- Test: `crates/neo-tui/src/shell/select_list.rs`

- [ ] **Step 1: Add failing select list style tests**

At the bottom of `crates/neo-tui/src/shell/select_list.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::theme::TuiTheme;

    #[test]
    fn selected_item_styles_label_and_description_separately() {
        let theme = TuiTheme::default();
        let list = SelectListState::new(
            vec![SelectItem::new(
                "/ask",
                "/ask",
                Some("ask permission mode"),
            )],
            8,
        );

        let line = list.render_lines(80, &theme).remove(0);
        assert!(line.contains("/ask"), "{line}");
        assert!(line.contains("ask permission mode"), "{line}");
        assert!(line.contains("\x1b["), "expected ANSI styling: {line:?}");
        let plain = crate::primitive::strip_ansi(&line);
        assert!(plain.starts_with("> /ask"), "{plain}");
    }

    #[test]
    fn select_list_never_invents_metadata() {
        let theme = TuiTheme::default();
        let list = SelectListState::new(
            vec![SelectItem::new(
                "/ask",
                "/ask",
                Some("ask permission mode"),
            )],
            8,
        );

        let plain = crate::primitive::strip_ansi(&list.render_lines(80, &theme).remove(0));
        assert!(!plain.contains("provider:"), "{plain}");
        assert!(!plain.contains("trust:"), "{plain}");
        assert!(!plain.contains("source:"), "{plain}");
    }
}
```

- [ ] **Step 2: Run the failing select list style test**

Run:

```bash
cargo test --package neo-tui --lib shell::select_list::tests::selected_item_styles_label_and_description_separately --exact --nocapture
```

Expected: FAIL because `render_lines` currently does not accept a theme and returns unstyled strings.

- [ ] **Step 3: Update select list rendering signature**

In `crates/neo-tui/src/shell/select_list.rs`, add imports:

```rust
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Style, paint};
```

Change:

```rust
pub fn render_lines(&self, width: usize) -> Vec<String>
```

to:

```rust
pub fn render_lines(&self, width: usize, theme: &TuiTheme) -> Vec<String>
```

Inside that method, change the empty state to:

```rust
return vec![paint(
    &truncate_width("  No matching commands", width, "", false),
    Style::default().fg(theme.text_muted),
)];
```

Change the row call to:

```rust
            lines.push(render_select_item(
                item,
                filtered_index == self.selected_index,
                width,
                theme,
            ));
```

Change the page info push to:

```rust
            lines.push(paint(
                &truncate_width(&info, width, "", false),
                Style::default().fg(theme.text_muted),
            ));
```

- [ ] **Step 4: Style labels and descriptions**

Replace `render_select_item` with:

```rust
fn render_select_item(item: &SelectItem, selected: bool, width: usize, theme: &TuiTheme) -> String {
    let prefix = if selected { "> " } else { "  " };
    let label = if item.label.is_empty() {
        &item.value
    } else {
        &item.label
    };
    let prefix_width = visible_width(prefix);
    let description = item
        .description
        .as_deref()
        .map(|description| description.replace(['\r', '\n'], " ").trim().to_string())
        .filter(|description| !description.is_empty());

    let prefix_style = if selected {
        Style::default().fg(theme.selected_fg).bg(theme.selected_bg)
    } else {
        Style::default().fg(theme.text_muted)
    };
    let label_style = if selected {
        Style::default().fg(theme.selected_fg).bg(theme.selected_bg).bold()
    } else {
        Style::default().fg(theme.text_primary)
    };
    let description_style = if selected {
        Style::default().fg(theme.text_muted).bg(theme.selected_bg)
    } else {
        Style::default().fg(theme.text_muted)
    };

    if let Some(description) = description.filter(|_| width > 40) {
        let primary_width = 32usize.min(width.saturating_sub(prefix_width + 4)).max(1);
        let fitted_label = truncate_width(label, primary_width.saturating_sub(2).max(1), "", false);
        let spacing = " ".repeat(primary_width.saturating_sub(visible_width(&fitted_label)).max(1));
        let used = prefix_width + visible_width(&fitted_label) + spacing.len();
        let remaining = width.saturating_sub(used + 2);
        if remaining > 10 {
            let fitted_description = truncate_width(&description, remaining, "", false);
            return format!(
                "{}{}{}{}",
                paint(prefix, prefix_style),
                paint(&fitted_label, label_style),
                spacing,
                paint(&fitted_description, description_style)
            );
        }
    }

    let max_label_width = width.saturating_sub(prefix_width + 2).max(1);
    let fitted_label = truncate_width(label, max_label_width, "", false);
    format!(
        "{}{}",
        paint(prefix, prefix_style),
        paint(&fitted_label, label_style)
    )
}
```

- [ ] **Step 5: Thread theme through picker renderers**

In `crates/neo-tui/src/shell/pickers.rs`, change:

```rust
pub fn render_lines(&self, width: usize) -> Vec<String>
```

on `PromptCompletionState` to:

```rust
pub fn render_lines(&self, width: usize, theme: &crate::primitive::theme::TuiTheme) -> Vec<String> {
    self.picker.render_lines(width, theme)
}
```

Change:

```rust
pub fn render_lines(&self, width: usize) -> Vec<String>
```

on `PickerState` to:

```rust
pub fn render_lines(&self, width: usize, theme: &crate::primitive::theme::TuiTheme) -> Vec<String> {
    self.list.render_lines(width, theme)
}
```

In `crates/neo-tui/src/shell/overlay.rs`, update picker calls:

```rust
            Self::CommandPalette(palette) => Some(palette.render_lines(width)),
            Self::SessionPicker(_) => self.session_picker_lines(width, theme),
            Self::ModelPicker(picker) => Some(picker.render_lines(width, theme)),
            Self::PromptCompletion(completions) => Some(completions.render_lines(width, theme)),
```

In `crates/neo-tui/src/transcript/chrome_render.rs`, update:

```rust
    let raw_lines = state.render_lines(inner_width);
```

to:

```rust
    let theme = app.theme();
    let raw_lines = state.render_lines(inner_width, &theme);
```

and remove the later duplicate `let theme = app.theme();` in that function.

In `crates/neo-agent/src/modes/interactive/snapshot.rs`, change the `ModelPicker` branch from:

```rust
        Some(OverlayKind::ModelPicker(picker)) => {
            render_picker_snapshot("Models", picker, content_width)
        }
```

to:

```rust
        Some(OverlayKind::ModelPicker(picker)) => {
            let theme = app.theme();
            render_picker_snapshot("Models", picker, content_width, &theme)
        }
```

Then change `render_picker_snapshot` from:

```rust
pub(super) fn render_picker_snapshot(
    title: &str,
    picker: &neo_tui::shell::PickerState,
    width: usize,
) -> Vec<String> {
    let mut lines = vec![title.to_owned()];
    lines.extend(picker.render_lines(width));
    lines
}
```

to:

```rust
pub(super) fn render_picker_snapshot(
    title: &str,
    picker: &neo_tui::shell::PickerState,
    width: usize,
    theme: &neo_tui::shell::TuiTheme,
) -> Vec<String> {
    let mut lines = vec![title.to_owned()];
    lines.extend(picker.render_lines(width, theme));
    lines
}
```

- [ ] **Step 6: Run select list style tests**

Run:

```bash
cargo test --package neo-tui --lib shell::select_list::tests::selected_item_styles_label_and_description_separately --exact --nocapture
cargo test --package neo-tui --lib shell::select_list::tests::select_list_never_invents_metadata --exact --nocapture
```

Expected: both PASS.

- [ ] **Step 7: Run prompt completion rendering smoke test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_tabs_through_real_filesystem_prompt_completions --exact --nocapture --include-ignored
```

Expected: PASS. This guards the composer dropdown path after the picker render signature change.

- [ ] **Step 8: Checkpoint**

Do not run git commands unless the user has explicitly authorized git mutation for this execution instance. If authorization is granted later, this task's logical commit message is:

```text
style: refine slash completion dropdown
```

---

### Task 7: Final Narrow Verification

**Files:**
- No new files.
- Verify the exact touched behavior across `neo-agent` and `neo-tui`.

- [ ] **Step 1: Run exact completion behavior tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::prompt_completions_merges_real_prompt_package_and_session_commands --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_include_help_command --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completion_descriptions_hide_internal_metadata --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_include_dynamic_skill_commands_without_metadata --exact --nocapture --include-ignored
```

Expected: all PASS.

- [ ] **Step 2: Run exact help behavior tests**

Run:

```bash
cargo test --package neo-tui --lib dialogs::help_panel::tests::help_panel_renders_shortcuts_commands_and_skill_commands --exact --nocapture
cargo test --package neo-tui --lib dialogs::help_panel::tests::help_panel_sorts_skill_commands_after_regular_commands --exact --nocapture
cargo test --package neo-tui --lib dialogs::help_panel::tests::help_panel_scrolls_and_closes --exact --nocapture
cargo test --package neo-tui --lib shell::tests::help_panel_overlay_opens_as_rich_dialog_and_blocks_prompt --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_help_opens_help_panel_overlay --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_help_panel_includes_dynamic_skill_commands --exact --nocapture --include-ignored
```

Expected: all PASS.

- [ ] **Step 3: Run exact dropdown rendering tests**

Run:

```bash
cargo test --package neo-tui --lib shell::select_list::tests::selected_item_styles_label_and_description_separately --exact --nocapture
cargo test --package neo-tui --lib shell::select_list::tests::select_list_never_invents_metadata --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_tabs_through_real_filesystem_prompt_completions --exact --nocapture --include-ignored
```

Expected: all PASS.

- [ ] **Step 4: Formatting check**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS.

- [ ] **Step 5: Report results**

Report:

```text
Implemented /help help panel and cleaned slash completion display.
Verified with exact neo-agent/neo-tui tests and cargo fmt --all --check.
No git mutations were run.
```

If any command fails because of unrelated concurrent work, stop widening verification and report the exact failure output plus which touched behavior was already verified.

---

## Self-Review

- Spec coverage: `/help` becomes real, banner text now points to a working command, help panel contains keyboard shortcuts, built-in slash commands, and dynamic `/skill:<name>` commands, completion descriptions are clean, and metadata remains internal for sorting only.
- Placeholder scan: no task uses TBD/TODO/fill-in language; every code-changing step gives concrete Rust snippets or exact replacement text.
- Type consistency: `HelpPanelCommand`, `HelpPanelOptions`, and `HelpPanelState` are introduced in `neo_tui::dialogs` and used consistently from shell overlay and agent slash dispatch.
- Verification scope: tests are exact function-level commands, plus `cargo fmt --all --check`; no broad package-wide test command is required.
- Git policy: plan intentionally omits executable git mutation steps and records logical commit messages only for a separately authorized commit phase.
