# Plan Mode UI/UX Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve the plan mode TUI/UX across four areas: empty-args tool headers, plan content in approval dialog, approval dialog titles, and Write tool rendering — plus add a "Current plan · Approved" header for ExitPlanMode.

**Architecture:** All changes are in the TUI rendering layer (`crates/neo-tui`) and the runtime event layer (`crates/neo-agent-core/src/runtime.rs`). No core agent logic changes. The plan content flows from the `PlanMode` state through the `ApprovalRequested` event into the transcript pane's `ApprovalPromptData`, where a `PlanBoxComponent` renders it inside the approval dialog.

**Tech Stack:** Rust, ratatui-style primitives, serde_json for event arguments.

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `crates/neo-tui/src/transcript/tool_renderers.rs` | Tool header spans, body dispatch, result chips | 1, 2 |
| `crates/neo-tui/src/transcript/tool_call.rs` | ToolCallComponent render orchestration, ExitPlanMode card | 6 |
| `crates/neo-tui/src/transcript/entry.rs` | `ApprovalPromptData` struct, `render_approval_prompt` | 4 |
| `crates/neo-tui/src/transcript/pane.rs` | `ApprovalPromptSummary`, `approval_prompt()`, `upsert_approval` | 4, 5 |
| `crates/neo-tui/src/transcript/plan_box.rs` | PlanBox border rendering (no changes needed, just consumed) | — |
| `crates/neo-agent-core/src/runtime.rs` | `resolve_approval`, `attach_exit_plan_details` | 3, 6 |
| `crates/neo-tui/tests/tool_cards.rs` | Integration tests for tool card rendering | 1, 2 |

---

## Task 1: Fix empty-args tools showing `({})`

**Problem:** Tools like `EnterPlanMode`, `ExitPlanMode`, `TodoList` with no meaningful arguments display `● Used EnterPlanMode ({})` because `extract_key_argument` falls through to showing the raw JSON `{}`.

**Files:**
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs:438-463`
- Test: `crates/neo-tui/tests/tool_cards.rs`

- [ ] **Step 1: Write the failing test**

In `crates/neo-tui/tests/tool_cards.rs`, add a test that verifies no `(...)` appears for a tool with `{}` arguments:

```rust
#[test]
fn empty_json_args_do_not_show_parens() {
    use neo_tui::transcript::ToolCallComponent;
    use neo_tui::transcript::ToolCallState;
    use neo_tui::transcript::ToolStatusKind;

    let state = ToolCallState {
        id: "test-1".to_string(),
        name: "EnterPlanMode".to_string(),
        arguments: Some("{}".to_string()),
        result: Some("OK".to_string()),
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    };
    let mut comp = ToolCallComponent::new(state);
    let lines = comp.render_with_theme(80, &neo_tui::shell::TuiTheme::default());
    let header = lines[0].to_ansi();
    assert!(
        !header.contains('('),
        "header should not contain parens for empty args, got: {header}"
    );
    assert!(header.contains("EnterPlanMode"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo run -p xtask -- test -p neo-tui empty_json_args`
Expected: FAIL — the header contains `({})`.

- [ ] **Step 3: Fix `extract_key_argument`**

In `crates/neo-tui/src/transcript/tool_renderers.rs`, replace the `extract_key_argument` function (lines 438–463). The fix: when JSON parses successfully but no known key matches, return `None` instead of falling through to the raw-string fallback. Only use the raw-string fallback when JSON parsing fails entirely.

```rust
fn extract_key_argument(arguments: Option<&str>) -> Option<(String, bool)> {
    let arguments = arguments.map(str::trim).filter(|value| !value.is_empty())?;
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) {
        for key in [
            "path",
            "file_path",
            "command",
            "pattern",
            "query",
            "url",
            "description",
        ] {
            if let Some(text) = value.get(key).and_then(serde_json::Value::as_str) {
                let is_path = PATH_KEYS.contains(&key);
                return Some((one_line(text), is_path));
            }
        }
        // Valid JSON but no recognized key — return None so the header
        // omits the `(...)` suffix entirely (e.g. EnterPlanMode with `{}`).
        return None;
    }
    if let Some(path) = arguments
        .strip_prefix(r#"{"path":"#)
        .and_then(|rest| rest.strip_suffix(r#""}"#))
    {
        return Some((one_line(path), true));
    }
    Some((one_line(arguments), false))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo run -p xtask -- test -p neo-tui empty_json_args`
Expected: PASS.

- [ ] **Step 5: Run broader tool card tests to check no regressions**

Run: `cargo run -p xtask -- test -p neo-tui tool_cards`
Expected: All existing tests still pass (tools with real arguments still show them).

---

## Task 2: Fix Write tool using diff rendering

**Problem:** The `Write` tool (which creates or overwrites files) renders its content as a unified diff with all-green `+` lines. Write should use a syntax-highlighted content preview instead, like the existing `render_write_preview` already supports.

**Files:**
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs:164-196` (`render_diff_details`)
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs:542-558` (`result_chip`)

- [ ] **Step 1: Fix `render_diff_details` to skip Write entirely**

In `crates/neo-tui/src/transcript/tool_renderers.rs`, replace the `render_diff_details` function (lines 164–196). Change the guard at the top from `!is_file_write_tool` to only allow `Edit`:

```rust
fn render_diff_details(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    // Only Edit uses unified diff rendering. Write always uses a
    // syntax-highlighted content preview (via render_write_body).
    if state.name != "Edit" {
        return None;
    }
    if let Some(model) = state
        .details
        .as_ref()
        .and_then(DiffModel::from_tool_details)
    {
        return Some(render_diff_model_lines(
            &model,
            expanded,
            width,
            palette.theme,
        ));
    }
    None
}
```

This removes the `"created"` check (no longer needed since Write never enters this function) and the `is_file_write_tool` guard (replaced with `state.name != "Edit"`).

- [ ] **Step 2: Fix `result_chip` to show line count for Write**

In the same file, replace `result_chip` (lines 542–558). Write should show `· N lines` instead of `· +N -M`:

```rust
fn result_chip(state: &ToolCallState) -> String {
    if state.name == "Edit"
        && let Some(model) = state
            .details
            .as_ref()
            .and_then(DiffModel::from_tool_details)
    {
        return format!(" · +{} -{}", model.stats().added, model.stats().removed);
    }
    let Some(result) = state.result.as_deref().filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if state.name == "Read" || state.name == "Write" {
        return format!(" · {} lines", result.lines().count());
    }
    String::new()
}
```

- [ ] **Step 3: Run tool card tests**

Run: `cargo run -p xtask -- test -p neo-tui tool_cards`
Expected: PASS. If any existing test asserts `+N -M` for Write, update it to assert `N lines` instead.

- [ ] **Step 4: Build check**

Run: `cargo build -p neo-tui`
Expected: Clean build, no warnings about unused imports (the `is_file_write_tool` import in this function context should still be used by streaming preview elsewhere).

---

## Task 3: Plumb plan content into the approval event

**Problem:** The `ApprovalRequested` event for `PlanTransition` carries the ExitPlanMode tool call's arguments (`plan_summary`, `options`), but not the plan file content. The approval dialog therefore cannot display the plan.

**Files:**
- Modify: `crates/neo-agent-core/src/runtime.rs:3308-3336` (`resolve_approval`)

- [ ] **Step 1: Enrich arguments with plan content in `resolve_approval`**

In `crates/neo-agent-core/src/runtime.rs`, find the `resolve_approval` function (line 3308). After the `let request = ApprovalRequest { ... }` block (line 3319–3327), and before the `emitter.emit(...)` call (line 3328), add logic to enrich the arguments for `PlanTransition`:

Replace lines 3319–3336 with:

```rust
    let mut arguments = tool_call.arguments.clone();
    // For plan transitions, inject the plan file content so the TUI can
    // render it inside the approval dialog.
    if operation == PermissionOperation::PlanTransition {
        if let Ok(plan_mode) = config.plan_mode.read() {
            if let Ok(Some(plan_data)) = plan_mode.data() {
                if let Some(obj) = arguments.as_object_mut() {
                    obj.insert(
                        "plan_content".to_string(),
                        serde_json::Value::String(plan_data.content.clone()),
                    );
                    obj.insert(
                        "plan_path".to_string(),
                        serde_json::Value::String(plan_data.path.display().to_string()),
                    );
                }
            }
        }
    }
    let request = ApprovalRequest {
        turn,
        id: tool_call.id.clone(),
        operation,
        subject: subject.clone(),
        arguments: arguments.clone(),
        session_scope: session_scope.clone(),
        prefix_rule: prefix_rule.clone(),
    };
    emitter.emit(AgentEvent::ApprovalRequested {
        turn: request.turn,
        id: request.id.clone(),
        operation: request.operation,
        subject: request.subject.clone(),
        arguments: request.arguments.clone(),
        session_scope: request.session_scope.clone(),
        prefix_rule: request.prefix_rule.clone(),
    });
```

Note: This replaces the existing `let request = ApprovalRequest { arguments: tool_call.arguments.clone(), ... }` and the `emitter.emit(...)` block. The `config` parameter is already available in `resolve_approval`'s signature.

- [ ] **Step 2: Build check**

Run: `cargo build -p neo-agent-core`
Expected: Clean build.

---

## Task 4: Render plan content inside the approval dialog

**Problem:** The approval dialog (`render_approval_prompt`) shows only a title, details text, and option list — no plan content. We want a `PlanBoxComponent` rendered between the title and the options, matching kimi-code's "Current plan" box.

**Files:**
- Modify: `crates/neo-tui/src/transcript/entry.rs:24-42` (`ApprovalPromptData` struct)
- Modify: `crates/neo-tui/src/transcript/pane.rs:670-716` (`upsert_approval`)
- Modify: `crates/neo-tui/src/transcript/pane.rs:854-858` (`ApprovalPromptSummary` struct)
- Modify: `crates/neo-tui/src/transcript/pane.rs:860-941` (`approval_prompt` function)
- Modify: `crates/neo-tui/src/transcript/entry.rs:646-730` (`render_approval_prompt` function)

- [ ] **Step 1: Add plan fields to `ApprovalPromptSummary`**

In `crates/neo-tui/src/transcript/pane.rs`, add two fields to the `ApprovalPromptSummary` struct (line 854):

```rust
struct ApprovalPromptSummary {
    title: String,
    details: Vec<String>,
    queued_label: String,
    plan_content: Option<String>,
    plan_path: Option<String>,
}
```

- [ ] **Step 2: Populate plan fields in `approval_prompt` for PlanTransition**

In the same file, find the `PlanTransition` arm of `approval_prompt` (line 930). Replace it to extract `plan_content`/`plan_path` from the event arguments:

```rust
            PermissionOperation::PlanTransition => {
                let plan_content = arguments
                    .get("plan_content")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned);
                let plan_path = arguments
                    .get("plan_path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                ApprovalPromptSummary {
                    title: "Plan Review".to_owned(),
                    details: compact_details([Some("Ready to build with this plan?".to_owned())]),
                    queued_label: String::new(),
                    plan_content,
                    plan_path,
                }
            }
```

Note: This also changes the title from `"Plan mode transition"` to `"Plan Review"` and the details from the subject to `"Ready to build with this plan?"` (covering Task 5).

Also add `plan_content: None, plan_path: None` to all other arms in the `match` and `if/else` branches of `approval_prompt`. Every `ApprovalPromptSummary { ... }` literal needs these two fields. There are 8 construction sites total in this function:
- `is_task_stop` branch
- `is_terminal` branch
- `is_edit` branch
- `Shell`, `FileWrite`, `FileRead`, `Tool`, `UserQuestion`, `GoalTransition` arms

Add `plan_content: None,\n plan_path: None,` before the closing `}` of each.

- [ ] **Step 3: Add plan fields to `ApprovalPromptData`**

In `crates/neo-tui/src/transcript/entry.rs`, add two fields to `ApprovalPromptData` (line 24):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPromptData {
    pub id: String,
    pub title: String,
    pub details: Vec<String>,
    pub queued_label: String,
    pub queued_count: usize,
    pub selected: usize,
    pub feedback_input: String,
    pub resolved: Option<String>,
    #[serde(default)]
    pub session_option_label: Option<String>,
    #[serde(default)]
    pub prefix_option_label: Option<String>,
    /// Plan file content to render inside the approval dialog (PlanTransition only).
    #[serde(default)]
    pub plan_content: Option<String>,
    /// Plan file path for the box title (PlanTransition only).
    #[serde(default)]
    pub plan_path: Option<String>,
}
```

- [ ] **Step 4: Pass plan fields through `upsert_approval`**

In `crates/neo-tui/src/transcript/pane.rs`, update `upsert_approval` (line 670). The `ApprovalPromptSummary` already carries `plan_content`/`plan_path` from Step 2. Update the update path (line 681) and the create path (line 696):

For the update path (line 682–693), add after `approval.queued_label = prompt.queued_label;`:

```rust
            approval.plan_content = prompt.plan_content;
            approval.plan_path = prompt.plan_path;
```

For the create path (line 696–707), add the fields:

```rust
        let data = ApprovalPromptData {
            id,
            title: prompt.title,
            details: prompt.details,
            queued_label: prompt.queued_label,
            queued_count: 0,
            selected: 0,
            feedback_input: String::new(),
            resolved: None,
            session_option_label,
            prefix_option_label,
            plan_content: prompt.plan_content,
            plan_path: prompt.plan_path,
        };
```

- [ ] **Step 5: Render PlanBox in `render_approval_prompt`**

In `crates/neo-tui/src/transcript/entry.rs`, find `render_approval_prompt` (line 646). After the details loop (line 668) and before the empty line + options list (line 669), insert the PlanBox rendering. Add this `use` at the top of the file if not already present:

```rust
use crate::transcript::PlanBoxComponent;
```

Then in `render_approval_prompt`, after the `for detail in &data.details { ... }` loop and the `rows.push(Line::raw(""));` that follows it, insert:

```rust
    // Render the plan content box (PlanTransition only).
    if let Some(plan_content) = &data.plan_content {
        let plan_box = PlanBoxComponent::new(plan_content.clone(), data.plan_path.clone());
        let box_lines = plan_box.render(width, theme);
        for line in box_lines {
            rows.push(line);
        }
        rows.push(Line::raw(""));
    }
```

This goes between the existing `rows.push(Line::raw(""));` after details and the `let mut options: Vec<String> = ...` line.

- [ ] **Step 6: Build and test**

Run: `cargo build -p neo-tui`
Expected: Clean build. Fix any `ApprovalPromptData` construction sites that are missing the new fields (search for `ApprovalPromptData {` across the codebase).

Run: `cargo run -p xtask -- test -p neo-tui`
Expected: All tests pass.

---

## Task 5: Improve approval dialog title text

**Note:** This task is already handled in Task 4 Step 2, where the `PlanTransition` arm of `approval_prompt` was changed to use `"Plan Review"` as the title and `"Ready to build with this plan?"` as the details text.

- [ ] **Step 1: Verify no other references to old strings**

Search for `"Plan mode transition"` and `"Exit plan mode"` in the TUI crate:

Run: `rg "Plan mode transition" crates/neo-tui/`
Expected: No matches in `pane.rs` (the string was replaced). If found elsewhere, update.

- [ ] **Step 2: Run full TUI test suite**

Run: `cargo run -p xtask -- test -p neo-tui`
Expected: PASS.

---

## Task 6: Add "Current plan · Approved" header to ExitPlanMode card

**Problem:** The ExitPlanMode tool card shows a generic `● Used ExitPlanMode` header. It should show `● Current plan` with an optional `· Approved: <chosen>` or `· Rejected` suffix. The chosen approach label needs to be plumbed from the approval flow into the tool result `details`.

**Files:**
- Modify: `crates/neo-agent-core/src/runtime.rs:1561-1597` (`attach_exit_plan_details`)
- Modify: `crates/neo-tui/src/transcript/tool_call.rs:207-261` (`render_with_theme`)
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs:28-56` (`tool_header_spans`)

- [ ] **Step 1: Store selected label in tool result details**

In `crates/neo-agent-core/src/runtime.rs`, find `attach_exit_plan_details` (line 1561). Inside the `if !result.is_error && let Some(labels) = ...` block (line 1584–1593), after prefixing `result.content`, also store the label in `result.details`:

Replace lines 1584–1594 with:

```rust
            if !result.is_error
                && let Some(labels) = selected_labels.as_mut()
                && let Some(label) = labels.remove(&tool_call.id)
                && !label.trim().is_empty()
            {
                result.content = format!(
                    "Selected approach: {label}\n\
                     Execute ONLY the selected approach. Do not execute any unselected alternatives.\n\n{}",
                    result.content
                );
                if let Some(details) = result.details.as_mut() {
                    if let Some(obj) = details.as_object_mut() {
                        obj.insert(
                            "plan_selected_label".to_string(),
                            serde_json::Value::String(label),
                        );
                    }
                }
            }
```

- [ ] **Step 2: Add custom header builder for ExitPlanMode**

In `crates/neo-tui/src/transcript/tool_renderers.rs`, add a new function after `tool_header_spans` (after line 56):

```rust
/// Build a custom header for the ExitPlanMode tool card.
///
/// Replaces the generic "Used ExitPlanMode" with "Current plan",
/// optionally appending "· Approved: <label>" (on success with a chosen
/// approach) or nothing extra (on rejection — the PlanBox shows "Rejected").
#[must_use]
pub fn exit_plan_mode_header_spans(
    state: &ToolCallState,
    theme: &TuiTheme,
) -> Vec<Span> {
    let symbol = tool_symbol(state.status);
    let status_color = tool_status_color(state.status, theme);
    let name_color = theme.brand;
    let success_color = theme.status_ok;
    let muted_color = theme.text_muted;

    let mut spans = vec![
        Span::styled(format!("{symbol} "), Style::default().fg(status_color)),
        Span::styled("Current plan", Style::default().fg(name_color).bold()),
    ];

    // On success, show "· Approved" or "· Approved: <label>"
    if state.status == ToolStatusKind::Succeeded {
        let label = state
            .details
            .as_ref()
            .and_then(|d| d.get("plan_selected_label"))
            .and_then(serde_json::Value::as_str);
        let chip = match label {
            Some(l) if !l.is_empty() => format!(" · Approved: {l}"),
            _ => " · Approved".to_string(),
        };
        spans.push(Span::styled(chip, Style::default().fg(success_color)));
    }

    // On failure, show "· Rejected"
    if state.status == ToolStatusKind::Failed {
        spans.push(Span::styled(
            " · Rejected".to_string(),
            Style::default().fg(theme.status_error),
        ));
    }

    let _ = muted_color; // muted_color reserved for future use
    spans
}
```

- [ ] **Step 3: Use custom header in `render_with_theme`**

In `crates/neo-tui/src/transcript/tool_call.rs`, find `render_with_theme` (line 207). Replace the header construction (line 208–210):

```rust
    pub fn render_with_theme(&mut self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let header_spans = if self.state.name == "ExitPlanMode" {
            crate::transcript::tool_renderers::exit_plan_mode_header_spans(&self.state, theme)
        } else {
            tool_header_spans(&self.state, theme, self.workspace_dir.as_deref())
        };
        let header_width = width.saturating_sub(2).max(1);
        let mut rows = vec![Line::from_spans(header_spans).truncate_to_width(header_width)];
```

- [ ] **Step 4: Fix the PlanBox status for ExitPlanMode (remove redundant Rejected)**

Since the header now shows "· Rejected", the PlanBox title should NOT also show "· Rejected" (avoid duplication). In `tool_call.rs`, find the ExitPlanMode PlanBox rendering (around line 221). Change the status logic to never pass a status to the PlanBox (the header handles it):

Replace lines 221–229:

```rust
            let mut plan_box = PlanBoxComponent::new(plan_content.to_string(), plan_path);
            // Status (Approved/Rejected) is shown in the card header, not the box title.
            rows.extend(plan_box.render(width, theme));
```

Remove the `with_status` call entirely — the header now carries the status.

- [ ] **Step 5: Export the new function**

In `crates/neo-tui/src/transcript/mod.rs`, ensure `exit_plan_mode_header_spans` is accessible from `tool_call.rs`. Check if `tool_renderers` module items are exported with `pub`. If `tool_header_spans` is already `pub`, the new function is also `pub` in the same module and should be accessible via the same path. If not, add it to the re-exports.

- [ ] **Step 6: Build and test**

Run: `cargo build -p neo-tui`
Expected: Clean build.

Run: `cargo run -p xtask -- test -p neo-tui`
Expected: All tests pass. Add a test for the new header:

```rust
#[test]
fn exit_plan_mode_header_shows_current_plan() {
    use neo_tui::transcript::ToolCallComponent;
    use neo_tui::transcript::ToolCallState;
    use neo_tui::transcript::ToolStatusKind;

    let state = ToolCallState {
        id: "test-exit".to_string(),
        name: "ExitPlanMode".to_string(),
        arguments: None,
        result: Some("OK".to_string()),
        details: Some(serde_json::json!({
            "plan_content": "# Plan\nStep 1",
            "plan_path": "/tmp/plan.md",
            "plan_selected_label": "incremental"
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    };
    let mut comp = ToolCallComponent::new(state);
    let lines = comp.render_with_theme(80, &neo_tui::shell::TuiTheme::default());
    let header = lines[0].to_ansi();
    assert!(header.contains("Current plan"), "header: {header}");
    assert!(header.contains("Approved"), "header: {header}");
}
```

- [ ] **Step 7: Run full check**

Run: `cargo run -p xtask -- test -p neo-tui`
Run: `cargo run -p xtask -- test -p neo-agent-core`
Expected: All tests pass.

---

## Self-Review Notes

1. **Spec coverage:** All four user requirements + the additional "Current plan · Approved" requirement are covered:
   - Empty-args `({})` → Task 1
   - Plan content in approval dialog → Tasks 3, 4
   - Title improvements → Task 5 (folded into Task 4)
   - Write diff rendering → Task 2
   - "Current plan · Approved" → Task 6

2. **Type consistency:** `plan_content: Option<String>` and `plan_path: Option<String>` are used consistently across `ApprovalPromptSummary`, `ApprovalPromptData`, and the PlanBox rendering. The `plan_selected_label` key in `details` matches between runtime injection and TUI consumption.

3. **No placeholder steps:** Every step includes actual code or exact commands.
