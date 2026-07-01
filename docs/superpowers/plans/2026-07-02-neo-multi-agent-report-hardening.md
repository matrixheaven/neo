# Neo Multi-Agent Report Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix every issue from the latest Multi Agent tool test report by tightening delegate tool contracts, discoverability, swarm child naming, and resume history semantics.

**Architecture:** Keep `Delegate`, `DelegateSwarm`, `ListDelegates`, `WaitDelegate`, `MessageDelegate`, and `InterruptDelegate` as the canonical multi-agent API. Use shared formatting and filtering helpers in `neo-agent-core` so text output and structured `details` stay aligned. Do not add compatibility branches for old swarm item shapes; migrate the canonical schema and update tests/docs together.

**Tech Stack:** Rust 2024, `neo-agent-core`, `serde`, `schemars`, `serde_json`, Tokio unit tests, exact `cargo test --package neo-agent-core --lib <path> --exact --nocapture` verification.

---

## Scope

This plan covers all seven report issues:

| Report item | Implemented in |
|---|---|
| 4.1 `InterruptDelegate` leaks `TaskStop` / `background task` on unknown IDs | Task 1 |
| 4.2 `InterruptDelegate` terminal-agent message is copy-pasted from `MessageDelegate` | Task 1 |
| 4.3 `ListDelegates` empty `next_step` ignores `state` / `include_completed` query | Task 2 |
| 4.4 `DelegateSwarm.resume_agent_ids` schema is not explicit enough | Task 3 |
| 4.5 Generic background task tools and multi-agent tools are hard to discover together | Task 4 |
| 4.6 Swarm child titles default to long repeated prompt prefixes | Task 5 |
| 4.7 Resume overwrites current `state`; old cancelled/completed state is hard to query | Task 6 |

## Git Policy For Execution

The Neo workspace has a strict git mutation policy. Do not run `git add`, `git commit`, `git checkout`, `git restore`, `git reset`, `git stash`, `git rebase`, `git clean`, `git push`, or branch mutation commands unless the user gives explicit per-command authorization in the execution session. Each task below ends with a non-mutating checkpoint instead of an automatic commit.

## File Structure

- Modify `crates/neo-agent-core/src/tools/delegate_controls.rs`
  - Owns `ListDelegates`, `WaitDelegate`, `MessageDelegate`, `InterruptDelegate`.
  - Add shared delegate error helpers.
  - Add query-aware empty list hints.
  - Add `state_scope` filtering for current state vs historical terminal state.
  - Add a `#[cfg(test)]` module for delegate control contract tests.

- Modify `crates/neo-agent-core/src/tools/delegate.rs`
  - Owns `Delegate` and `DelegateSwarm` execution.
  - Update `DelegateSwarm` schema post-processing.
  - Switch new swarm children from string items to titled item objects.
  - Add validation tests for swarm item title/prompt behavior and schema text.

- Modify `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - Owns `DelegateSwarmRequest`, child task construction helpers, and resume state transitions.
  - Replace `Vec<String>` swarm items with `Vec<DelegateSwarmItem>`.
  - Record historical terminal statuses before resume overwrites current state.

- Modify `crates/neo-agent-core/src/multi_agent/state.rs`
  - Owns `AgentSnapshot`.
  - Add `terminal_status_history` for past terminal run states.

- Modify `crates/neo-agent-core/src/multi_agent/mod.rs`
  - Re-export `DelegateSwarmItem` with `DelegateSwarmRequest`.

- Modify `crates/neo-agent-core/src/tools/background_tasks.rs`
  - Owns `TaskList`, `TaskOutput`, `TaskStop`, and background task formatting.
  - Add synthetic task-list rows for active runtime delegates/swarms that are not already in `BackgroundTaskManager`.
  - Keep `TaskOutput` and `TaskStop` delegate-aware without leaking generic internals into `InterruptDelegate`.

- Modify `docs/tools.md`
  - Update tool descriptions for `TaskList`, `ListDelegates`, `DelegateSwarm.items`, and `resume_agent_ids`.

---

### Task 1: Normalize `InterruptDelegate` And Terminal Delegate Errors

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Test: `crates/neo-agent-core/src/tools/delegate_controls.rs`

- [ ] **Step 1: Add failing tests for unknown interrupt IDs and action-specific terminal messages**

Append this test module to `crates/neo-agent-core/src/tools/delegate_controls.rs` after `format_swarm_result`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_context() -> ToolContext {
        let dir = tempfile::tempdir().expect("temp dir");
        ToolContext::new(dir.path()).expect("tool context")
    }

    #[tokio::test]
    async fn interrupt_delegate_unknown_id_uses_delegate_error() {
        let ctx = test_context();
        let tool = InterruptDelegateTool;

        let result = tool
            .execute(&ctx, json!({ "id": "agent_missing" }))
            .await
            .expect("tool should return result");

        assert!(result.is_error);
        assert_eq!(result.content, "unknown delegate target `agent_missing`");
        assert!(!result.content.contains("TaskStop"));
        assert!(!result.content.contains("background task"));
        assert_eq!(result.details.as_ref().unwrap()["kind"], "delegate_target");
        assert_eq!(result.details.as_ref().unwrap()["outcome"], "not_found");
    }

    #[tokio::test]
    async fn terminal_delegate_errors_are_action_specific() {
        let ctx = test_context();
        let agent = ctx.multi_agent.start_foreground_delegate_for_test("calculate 2 + 2");
        ctx.multi_agent
            .complete_delegate_for_test(&agent.id, "The answer is 4.");

        let message_result = MessageDelegateTool
            .execute(
                &ctx,
                json!({ "id": agent.id.as_str(), "message": "another question" }),
            )
            .await
            .expect("message result");
        assert!(message_result.is_error);
        assert!(message_result.content.contains("cannot receive live messages"));
        assert!(!message_result.content.contains("be interrupted"));
        assert_eq!(message_result.details.as_ref().unwrap()["action"], "message");

        let interrupt_result = InterruptDelegateTool
            .execute(&ctx, json!({ "id": agent.id.as_str() }))
            .await
            .expect("interrupt result");
        assert!(interrupt_result.is_error);
        assert!(interrupt_result.content.contains("cannot be interrupted"));
        assert!(!interrupt_result.content.contains("live messages"));
        assert_eq!(interrupt_result.details.as_ref().unwrap()["action"], "interrupt");
    }
}
```

- [ ] **Step 2: Run the first failing test**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::interrupt_delegate_unknown_id_uses_delegate_error --exact --nocapture
```

Expected: FAIL. The current result contains `TaskStop` or `background task` wording.

- [ ] **Step 3: Run the second failing test**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::terminal_delegate_errors_are_action_specific --exact --nocapture
```

Expected: FAIL. The current terminal helper uses the same text for message and interrupt.

- [ ] **Step 4: Replace the shared terminal helper with action-aware helpers**

In `crates/neo-agent-core/src/tools/delegate_controls.rs`, replace the existing `terminal_delegate_error` function near the top of the file with:

```rust
#[derive(Debug, Clone, Copy)]
enum DelegateTerminalAction {
    Message,
    Interrupt,
}

impl DelegateTerminalAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Interrupt => "interrupt",
        }
    }

    const fn terminal_clause(self) -> &'static str {
        match self {
            Self::Message => "terminal agents cannot receive live messages",
            Self::Interrupt => "terminal agents cannot be interrupted",
        }
    }
}

fn delegate_target_not_found(id: &str) -> ToolResult {
    ToolResult::error(format!("unknown delegate target `{id}`")).with_details(json!({
        "kind": "delegate_target",
        "id": id,
        "outcome": "not_found",
    }))
}

fn terminal_delegate_error(
    agent_id: &str,
    state: AgentLifecycleState,
    action: DelegateTerminalAction,
) -> ToolResult {
    ToolResult::error(format!(
        "agent already {}; {}. To continue this agent, call Delegate with resume=\"{}\".",
        state.as_str(),
        action.terminal_clause(),
        agent_id
    ))
    .with_details(json!({
        "agent_id": agent_id,
        "status": state.as_str(),
        "terminal": true,
        "action": action.as_str(),
        "resume_hint": format!("Delegate with resume=\"{agent_id}\""),
    }))
}
```

Keep the existing `use serde_json::json;`; the helper now uses the imported macro instead of `serde_json::json!`.

- [ ] **Step 5: Update `InterruptDelegate` call sites**

In `InterruptDelegateTool::execute`, replace both calls to:

```rust
terminal_delegate_error(agent.id.as_str(), agent.state)
```

with:

```rust
terminal_delegate_error(
    agent.id.as_str(),
    agent.state,
    DelegateTerminalAction::Interrupt,
)
```

Then replace the current fallback block:

```rust
// Fall back to background task stop.
match ctx
    .background_tasks
    .stop(&input.id, "Interrupted by InterruptDelegate", 1024)
    .await
{
    Ok(result) => Ok(result),
    Err(err) => Ok(ToolResult::error(format!(
        "id: {}\nerror: {}",
        input.id, err
    ))),
}
```

with:

```rust
if ctx.background_tasks.snapshot(&input.id).await.is_ok() {
    return match ctx
        .background_tasks
        .stop(&input.id, "Interrupted by InterruptDelegate", 1024)
        .await
    {
        Ok(result) => Ok(result),
        Err(_) => Ok(delegate_target_not_found(&input.id)),
    };
}

Ok(delegate_target_not_found(&input.id))
```

This keeps a real background delegate record interruptible while preventing unknown IDs from leaking `TaskStop` internals.

- [ ] **Step 6: Update `MessageDelegate` terminal call site**

In `MessageDelegateTool::execute`, replace:

```rust
return Ok(terminal_delegate_error(agent.id.as_str(), agent.state));
```

with:

```rust
return Ok(terminal_delegate_error(
    agent.id.as_str(),
    agent.state,
    DelegateTerminalAction::Message,
));
```

- [ ] **Step 7: Verify Task 1 tests pass**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::interrupt_delegate_unknown_id_uses_delegate_error --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::terminal_delegate_errors_are_action_specific --exact --nocapture
```

Expected: PASS.

- [ ] **Step 8: Non-mutating checkpoint**

Run:

```bash
git diff -- crates/neo-agent-core/src/tools/delegate_controls.rs
```

Expected: diff only contains the new tests, action-aware terminal helper, and `InterruptDelegate` fallback normalization. Do not stage or commit without explicit user authorization.

---

### Task 2: Make `ListDelegates` Empty-State Hints Query-Aware

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Test: `crates/neo-agent-core/src/tools/delegate_controls.rs`

- [ ] **Step 1: Add failing tests for state-filtered empty results**

Inside the `#[cfg(test)] mod tests` added in Task 1, append:

```rust
#[tokio::test]
async fn list_delegates_empty_steps_follow_state_filter() {
    let ctx = test_context();
    let tool = ListDelegatesTool;

    let result = tool
        .execute(
            &ctx,
            json!({
                "include_completed": true,
                "state": "cancelled"
            }),
        )
        .await
        .expect("list result");

    assert!(!result.is_error);
    assert!(result.content.contains("No delegates found."));
    assert!(result.content.contains("No cancelled delegates found"));
    assert!(!result.content.contains("Pass include_completed=true"));
    assert_eq!(result.details.as_ref().unwrap()["query"]["state"], "cancelled");
    assert_eq!(result.details.as_ref().unwrap()["include_completed"], true);
}

#[tokio::test]
async fn list_delegates_default_empty_steps_explain_active_default() {
    let ctx = test_context();
    let tool = ListDelegatesTool;

    let result = tool
        .execute(&ctx, json!({}))
        .await
        .expect("list result");

    assert!(!result.is_error);
    assert!(result.content.contains("No active delegates found."));
    assert!(result.content.contains("Pass include_completed=true"));
}
```

- [ ] **Step 2: Run the state-filtered failing test**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_empty_steps_follow_state_filter --exact --nocapture
```

Expected: FAIL. The current hint tells the caller to pass `include_completed=true` even when it is already effective.

- [ ] **Step 3: Add a query-aware empty-step helper**

In `delegate_controls.rs`, add this helper after `include_label`:

```rust
fn empty_delegate_list_next_steps(
    input: &ListDelegatesInput,
    include_completed: bool,
    total: usize,
    offset: usize,
) -> Vec<String> {
    if total > 0 && offset >= total {
        return vec![
            "This page is empty because the cursor is past the available rows.".to_owned(),
            "Restart pagination by calling ListDelegates again without cursor.".to_owned(),
        ];
    }

    if let Some(state) = input.state {
        let kind = match input.kind {
            DelegateListKind::Agent => "agents",
            DelegateListKind::Swarm => "swarms",
            DelegateListKind::All => "delegates",
        };
        return vec![format!("No {} {kind} found for the current query.", state.as_str())];
    }

    if include_completed {
        return vec!["No delegates found in active or terminal history for the current query.".to_owned()];
    }

    vec![
        "No active delegates found.".to_owned(),
        "Pass include_completed=true to list completed, failed, cancelled, or timed_out delegates."
            .to_owned(),
    ]
}
```

- [ ] **Step 4: Use the helper in `ListDelegatesTool::execute`**

Replace the current static `empty_next_steps` array and empty content branch:

```rust
let empty_next_steps = [
    "No active delegates found.",
    "Pass include_completed=true to list completed, failed, cancelled, or timed_out delegates.",
];
let mut content = if page_rows.is_empty() {
    format!(
        "No delegates found.\nnext_step: {}\nnext_step: {}\n",
        empty_next_steps[0], empty_next_steps[1]
    )
} else {
    format!("total: {total}\n")
};
```

with:

```rust
let empty_next_steps = empty_delegate_list_next_steps(&input, include_completed, total, offset);
let mut content = if page_rows.is_empty() {
    let mut content = "No delegates found.\n".to_owned();
    for step in &empty_next_steps {
        content.push_str(&format!("next_step: {step}\n"));
    }
    content
} else {
    format!("total: {total}\n")
};
```

Keep the existing details assignment:

```rust
if page_rows.is_empty() {
    details["next_steps"] = json!(empty_next_steps);
}
```

It now serializes the same generated strings that appear in text output.

- [ ] **Step 5: Verify Task 2 tests pass**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_empty_steps_follow_state_filter --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_default_empty_steps_explain_active_default --exact --nocapture
```

Expected: PASS.

- [ ] **Step 6: Non-mutating checkpoint**

Run:

```bash
git diff -- crates/neo-agent-core/src/tools/delegate_controls.rs
```

Expected: diff shows `empty_delegate_list_next_steps` and tests for both terminal-state and default empty states. Do not stage or commit without explicit user authorization.

---

### Task 3: Clarify `DelegateSwarm.resume_agent_ids` Schema

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Test: `crates/neo-agent-core/src/tools/delegate.rs`

- [ ] **Step 1: Add a failing schema test**

Append this test module to `crates/neo-agent-core/src/tools/delegate.rs` after `reject_unknown_placeholders`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegate_swarm_schema_describes_resume_agent_ids_as_object_map() {
        let schema = DelegateSwarmTool.input_schema();
        let resume = &schema["properties"]["resume_agent_ids"];
        let description = resume["description"]
            .as_str()
            .expect("resume_agent_ids description");

        assert!(description.contains("JSON object"));
        assert!(description.contains("agent_id"));
        assert!(description.contains("per-agent resume prompt"));
        assert_eq!(resume["type"], "object");
    }
}
```

- [ ] **Step 2: Run the failing schema test**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate::tests::delegate_swarm_schema_describes_resume_agent_ids_as_object_map --exact --nocapture
```

Expected: FAIL because the description only says “Existing agent_id to prompt mapping”.

- [ ] **Step 3: Update the `resume_agent_ids` field description**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, replace:

```rust
#[schemars(description = "Existing agent_id to prompt mapping for resumed child agents.")]
pub resume_agent_ids: std::collections::BTreeMap<String, String>,
```

with:

```rust
#[schemars(
    description = "JSON object map from existing agent_id to per-agent resume prompt, for example {\"agent_xxx\": \"continue with this prompt\"}. Do not pass an array."
)]
pub resume_agent_ids: std::collections::BTreeMap<String, String>,
```

- [ ] **Step 4: Make the generated schema friendlier for map values**

In `schema_with_role_guide` in `crates/neo-agent-core/src/tools/delegate.rs`, after the existing role-description merge, add:

```rust
if let Some(resume_agent_ids) = props.get_mut("resume_agent_ids") {
    resume_agent_ids["type"] = serde_json::Value::String("object".to_owned());
    resume_agent_ids["additionalProperties"] = serde_json::json!({
        "type": "string",
        "description": "Prompt used when resuming that specific agent_id."
    });
}
```

This is schema metadata only; request parsing remains the existing `BTreeMap<String, String>`.

- [ ] **Step 5: Verify Task 3 test passes**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate::tests::delegate_swarm_schema_describes_resume_agent_ids_as_object_map --exact --nocapture
```

Expected: PASS.

- [ ] **Step 6: Non-mutating checkpoint**

Run:

```bash
git diff -- crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/tools/delegate.rs
```

Expected: diff only clarifies `resume_agent_ids` schema and adds the schema test. Do not stage or commit without explicit user authorization.

---

### Task 4: Make `TaskList` A Unified Background Work Index

**Files:**
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Test: `crates/neo-agent-core/src/tools/background_tasks.rs`

- [ ] **Step 1: Add failing tests for runtime delegates in `TaskList`**

Inside the existing `#[cfg(test)] mod tests` in `background_tasks.rs`, append:

```rust
#[tokio::test]
async fn task_list_tool_includes_active_runtime_delegate_without_background_record() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("calculate a small sum");

    let tool = TaskListTool;
    let result = tool.execute(&ctx, json!({})).await.expect("execute");

    assert!(!result.is_error);
    assert!(result.content.contains("active_background_tasks: 1"));
    assert!(result.content.contains(&format!("task_id: {}", agent.id.as_str())));
    assert!(result.content.contains("kind: delegate"));
    assert!(result.content.contains("status: running"));
    assert_eq!(result.details.as_ref().unwrap()["tasks"][0]["task_id"], agent.id.as_str());
    assert_eq!(result.details.as_ref().unwrap()["tasks"][0]["kind"], "delegate");
}

#[tokio::test]
async fn task_list_tool_deduplicates_delegate_background_records() {
    let manager = BackgroundTaskManager::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_background_tasks(manager.clone());
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("calculate another small sum");
    manager.start_delegate(agent.clone()).await;

    let tool = TaskListTool;
    let result = tool.execute(&ctx, json!({})).await.expect("execute");
    let tasks = result.details.as_ref().unwrap()["tasks"].as_array().unwrap();

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["task_id"], agent.id.as_str());
    assert_eq!(tasks[0]["kind"], "delegate");
}
```

- [ ] **Step 2: Run the first failing test**

Run:

```bash
cargo test --package neo-agent-core --lib tools::background_tasks::tests::task_list_tool_includes_active_runtime_delegate_without_background_record --exact --nocapture
```

Expected: FAIL because `TaskList` only reads `BackgroundTaskManager`.

- [ ] **Step 3: Add runtime delegate snapshot collection**

In `background_tasks.rs`, add `use std::collections::HashSet;` near the top if it is not already imported.

Add this helper near `task_list_result`:

```rust
fn status_from_agent_state(state: crate::multi_agent::AgentLifecycleState) -> BackgroundTaskStatus {
    match state {
        crate::multi_agent::AgentLifecycleState::Queued
        | crate::multi_agent::AgentLifecycleState::Running => BackgroundTaskStatus::Running,
        crate::multi_agent::AgentLifecycleState::Completed => BackgroundTaskStatus::Completed,
        crate::multi_agent::AgentLifecycleState::Failed => BackgroundTaskStatus::Failed,
        crate::multi_agent::AgentLifecycleState::Cancelled => BackgroundTaskStatus::Cancelled,
        crate::multi_agent::AgentLifecycleState::TimedOut => BackgroundTaskStatus::TimedOut,
    }
}

fn runtime_delegate_task_snapshots(
    ctx: &ToolContext,
    active_only: bool,
    existing_ids: &HashSet<String>,
) -> Vec<BackgroundTaskSnapshot> {
    let mut snapshots = Vec::new();

    for agent in ctx.multi_agent.list_agents(!active_only) {
        let task_id = agent.id.as_str().to_owned();
        if existing_ids.contains(&task_id) {
            continue;
        }
        let status = status_from_agent_state(agent.state);
        if active_only && !status.is_active() {
            continue;
        }
        snapshots.push(BackgroundTaskSnapshot {
            task_id,
            kind: BackgroundTaskKind::Delegate,
            status,
            description: agent.display_title(),
            elapsed: agent.elapsed,
            output: None,
            answers: None,
            delegate: Some(agent),
            swarm: None,
        });
    }

    for swarm in ctx.multi_agent.list_swarms() {
        if existing_ids.contains(&swarm.swarm_id) {
            continue;
        }
        let status = status_from_agent_state(swarm.state);
        if active_only && !status.is_active() {
            continue;
        }
        snapshots.push(BackgroundTaskSnapshot {
            task_id: swarm.swarm_id.clone(),
            kind: BackgroundTaskKind::DelegateSwarm,
            status,
            description: swarm.description.clone(),
            elapsed: Duration::ZERO,
            output: None,
            answers: None,
            delegate: None,
            swarm: Some(swarm),
        });
    }

    snapshots
}
```

- [ ] **Step 4: Merge runtime snapshots into `TaskListTool::execute`**

Replace:

```rust
let tasks = ctx.background_tasks.list(active_only, limit).await;
Ok(task_list_result(&tasks, active_only))
```

with:

```rust
let mut tasks = ctx.background_tasks.list(active_only, limit).await;
let existing_ids = tasks
    .iter()
    .map(|task| task.task_id.clone())
    .collect::<HashSet<_>>();
tasks.extend(runtime_delegate_task_snapshots(ctx, active_only, &existing_ids));
tasks.sort_by(|left, right| left.task_id.cmp(&right.task_id));
tasks.truncate(limit);
Ok(task_list_result(&tasks, active_only))
```

- [ ] **Step 5: Update `TaskListTool` description**

In `TaskListTool::description`, replace the kind line:

```rust
- kind: The type of background task (e.g. \"bash\", \"question\").\n\
```

with:

```rust
- kind: The type of background work, such as \"bash\", \"question\", \"delegate\", or \"delegate-swarm\".\n\
```

- [ ] **Step 6: Verify Task 4 tests pass**

Run:

```bash
cargo test --package neo-agent-core --lib tools::background_tasks::tests::task_list_tool_includes_active_runtime_delegate_without_background_record --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::background_tasks::tests::task_list_tool_deduplicates_delegate_background_records --exact --nocapture
```

Expected: PASS.

- [ ] **Step 7: Non-mutating checkpoint**

Run:

```bash
git diff -- crates/neo-agent-core/src/tools/background_tasks.rs
```

Expected: diff shows unified `TaskList` visibility for runtime delegates and swarms, with deduplication against existing background task records. Do not stage or commit without explicit user authorization.

---

### Task 5: Require Titled `DelegateSwarm.items`

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/mod.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Test: `crates/neo-agent-core/src/tools/delegate.rs`

- [ ] **Step 1: Add failing tests for titled swarm items**

Inside `crates/neo-agent-core/src/tools/delegate.rs` test module from Task 3, append:

```rust
#[test]
fn delegate_swarm_request_rejects_empty_item_title() {
    let request: DelegateSwarmRequest = serde_json::from_value(serde_json::json!({
        "description": "math checks",
        "items": [
            { "title": "   ", "value": "2 + 2" }
        ],
        "prompt_template": "Calculate {{item}}"
    }))
    .expect("request parses");

    let err = validate_swarm_request("DelegateSwarm", &request).expect_err("empty title rejected");
    assert_eq!(
        err.to_string(),
        "invalid input for DelegateSwarm: items[0].title must not be empty"
    );
}

#[test]
fn delegate_swarm_titled_items_drive_child_titles_and_prompts() {
    let request: DelegateSwarmRequest = serde_json::from_value(serde_json::json!({
        "description": "math checks",
        "items": [
            { "title": "addition", "value": "2 + 2" },
            { "title": "multiplication", "value": "3 * 3" }
        ],
        "prompt_template": "Calculate {{item}} for {{description}}"
    }))
    .expect("request parses");

    assert_eq!(request.items[0].title, "addition");
    assert_eq!(request.items[0].value, "2 + 2");
    assert_eq!(
        apply_swarm_template(
            request.prompt_template.as_deref().unwrap(),
            request.items[0].value.as_str(),
            request.description.as_str()
        ),
        "Calculate 2 + 2 for math checks"
    );
}
```

- [ ] **Step 2: Run the first failing titled-item test**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate::tests::delegate_swarm_request_rejects_empty_item_title --exact --nocapture
```

Expected: FAIL because `items` is currently `Vec<String>`.

- [ ] **Step 3: Replace string swarm items with canonical titled item objects**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, delete the entire `deserialize_string_vec` function. Then add this public struct above `DelegateSwarmRequest`:

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateSwarmItem {
    #[schemars(description = "Short human title for this child agent in ListDelegates and transcripts.")]
    pub title: String,
    #[schemars(description = "Item value inserted into prompt_template as {{item}}.")]
    pub value: String,
}
```

Replace the current `items` field:

```rust
#[serde(default, deserialize_with = "deserialize_string_vec")]
#[schemars(
    description = "New child task items. When present, prompt_template is required and must contain {{item}}."
)]
pub items: Vec<String>,
```

with:

```rust
#[serde(default)]
#[schemars(
    description = "New child task items as JSON objects with title and value. value is inserted into prompt_template as {{item}}."
)]
pub items: Vec<DelegateSwarmItem>,
```

- [ ] **Step 4: Re-export the item type**

In `crates/neo-agent-core/src/multi_agent/mod.rs`, replace:

```rust
DelegateSwarmRequest, MultiAgentRuntime, apply_swarm_template,
```

with:

```rust
DelegateSwarmItem, DelegateSwarmRequest, MultiAgentRuntime, apply_swarm_template,
```

- [ ] **Step 5: Update child construction in `DelegateSwarmTool::execute`**

In `crates/neo-agent-core/src/tools/delegate.rs`, keep using the existing `DelegateSwarmRequest` import from `crate::multi_agent`; `DelegateSwarmItem` is re-exported for schema consumers and does not need a direct import in this file. Replace the new-item loop:

```rust
for item in &request.items {
    let template = request.prompt_template.as_deref().unwrap_or("");
    let task = apply_swarm_template(template, item, &request.description);
    let snapshot = ctx.multi_agent.queue_delegate(
        &task,
        None,
        request.role,
        request.mode,
        crate::multi_agent::AgentPathKind::SwarmChild(&swarm_id),
    );
    initial_children.push(SwarmChildSnapshot {
        item_index,
        item: item.clone(),
        agent: snapshot,
    });
    item_index += 1;
}
```

with:

```rust
for item in &request.items {
    let template = request.prompt_template.as_deref().unwrap_or("");
    let task = apply_swarm_template(template, item.value.as_str(), &request.description);
    let snapshot = ctx.multi_agent.queue_delegate(
        &task,
        Some(item.title.as_str()),
        request.role,
        request.mode,
        crate::multi_agent::AgentPathKind::SwarmChild(&swarm_id),
    );
    initial_children.push(SwarmChildSnapshot {
        item_index,
        item: item.value.clone(),
        agent: snapshot,
    });
    item_index += 1;
}
```

- [ ] **Step 6: Update request validation for titled items**

In `validate_swarm_request`, replace the item validation loop:

```rust
for (index, item) in request.items.iter().enumerate() {
    if item.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("items[{index}] must not be empty"),
        });
    }
}
```

with:

```rust
for (index, item) in request.items.iter().enumerate() {
    if item.title.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("items[{index}].title must not be empty"),
        });
    }
    if item.value.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("items[{index}].value must not be empty"),
        });
    }
}
```

Then replace every prompt expansion over `request.items`:

```rust
for item in &request.items {
    let prompt = apply_swarm_template(template, item, &request.description);
```

with:

```rust
for item in &request.items {
    let prompt = apply_swarm_template(template, item.value.as_str(), &request.description);
```

- [ ] **Step 7: Update the legacy runtime helper that still accepts raw item text**

In `MultiAgentRuntime::run_swarm_child_turn`, keep the function signature unchanged because it already receives a single `item: &str` and is an internal helper. No compatibility branch is needed there because public `DelegateSwarmRequest.items` is now object-only.

- [ ] **Step 8: Verify titled item tests pass**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate::tests::delegate_swarm_request_rejects_empty_item_title --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate::tests::delegate_swarm_titled_items_drive_child_titles_and_prompts --exact --nocapture
```

Expected: PASS.

- [ ] **Step 9: Non-mutating checkpoint**

Run:

```bash
git diff -- crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/multi_agent/mod.rs crates/neo-agent-core/src/tools/delegate.rs
```

Expected: diff removes string-only swarm item parsing, adds canonical titled item objects, and passes titles into `queue_delegate`. Do not stage or commit without explicit user authorization.

---

### Task 6: Preserve Resume History Without Adding A `resumed` Lifecycle State

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Test: `crates/neo-agent-core/src/tools/delegate_controls.rs`

- [ ] **Step 1: Add failing tests for historical terminal-state filtering**

Inside `delegate_controls.rs` test module, append:

```rust
#[tokio::test]
async fn list_delegates_any_run_state_finds_resumed_cancelled_agent() {
    let ctx = test_context();
    let agent = ctx.multi_agent.start_foreground_delegate_for_test("first run");
    let cancelled = ctx
        .multi_agent
        .cancel_agent_by_id(agent.id.as_str())
        .expect("agent cancelled");
    assert_eq!(cancelled.state, AgentLifecycleState::Cancelled);

    ctx.multi_agent
        .start_resume_delegate(
            agent.id.as_str(),
            &crate::multi_agent::DelegateRequest {
                task: "second run".to_owned(),
                resume: Some(agent.id.as_str().to_owned()),
                title: None,
                role: None,
                mode: crate::multi_agent::AgentRunMode::Foreground,
                context: crate::multi_agent::DelegateContext::None,
            },
        )
        .expect("resume starts");
    ctx.multi_agent
        .complete_delegate_for_test(&agent.id, "second run done");

    let result = ListDelegatesTool
        .execute(
            &ctx,
            json!({
                "include_completed": true,
                "state": "cancelled",
                "state_scope": "any_run"
            }),
        )
        .await
        .expect("list result");

    assert!(!result.is_error);
    assert!(result.content.contains(agent.id.as_str()));
    let details = result.details.as_ref().unwrap();
    assert_eq!(details["query"]["state"], "cancelled");
    assert_eq!(details["query"]["state_scope"], "any_run");
    assert_eq!(details["delegates"][0]["current_status"], "completed");
    assert_eq!(details["delegates"][0]["terminal_status_history"][0], "cancelled");
}

#[tokio::test]
async fn list_delegates_current_state_does_not_match_resumed_cancelled_agent() {
    let ctx = test_context();
    let agent = ctx.multi_agent.start_foreground_delegate_for_test("first run");
    ctx.multi_agent
        .cancel_agent_by_id(agent.id.as_str())
        .expect("agent cancelled");
    ctx.multi_agent
        .start_resume_delegate(
            agent.id.as_str(),
            &crate::multi_agent::DelegateRequest {
                task: "second run".to_owned(),
                resume: Some(agent.id.as_str().to_owned()),
                title: None,
                role: None,
                mode: crate::multi_agent::AgentRunMode::Foreground,
                context: crate::multi_agent::DelegateContext::None,
            },
        )
        .expect("resume starts");
    ctx.multi_agent
        .complete_delegate_for_test(&agent.id, "second run done");

    let result = ListDelegatesTool
        .execute(
            &ctx,
            json!({
                "include_completed": true,
                "state": "cancelled"
            }),
        )
        .await
        .expect("list result");

    assert!(!result.is_error);
    assert!(result.content.contains("No cancelled delegates found"));
}
```

- [ ] **Step 2: Run the first failing history test**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_any_run_state_finds_resumed_cancelled_agent --exact --nocapture
```

Expected: FAIL because `state_scope` and `terminal_status_history` do not exist yet.

- [ ] **Step 3: Add terminal status history to `AgentSnapshot`**

In `crates/neo-agent-core/src/multi_agent/state.rs`, add this field after `previous_status`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub terminal_status_history: Vec<AgentLifecycleState>,
```

In `new_agent_snapshot` in `runtime.rs`, add:

```rust
terminal_status_history: Vec::new(),
```

immediately after `previous_status: None,`.

- [ ] **Step 4: Record old terminal state before resume overwrites current state**

In `start_resume_delegate` in `runtime.rs`, after:

```rust
let previous_status = agent.state;
```

add:

```rust
if previous_status.is_terminal()
    && agent
        .terminal_status_history
        .last()
        .copied()
        != Some(previous_status)
{
    agent.terminal_status_history.push(previous_status);
}
```

Keep the existing `agent.previous_status = Some(previous_status);`.

- [ ] **Step 5: Add `state_scope` input and filtering helpers to `ListDelegates`**

In `delegate_controls.rs`, add this enum near `DelegateListOrder`:

```rust
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DelegateStateScope {
    #[default]
    Current,
    AnyRun,
}
```

Add this field to `ListDelegatesInput` after `state`:

```rust
#[serde(default)]
#[schemars(
    description = "When state is set, current matches only the current lifecycle state. any_run also matches terminal states recorded before resume."
)]
state_scope: DelegateStateScope,
```

Add these helpers after `empty_delegate_list_next_steps`:

```rust
fn state_scope_label(scope: DelegateStateScope) -> &'static str {
    match scope {
        DelegateStateScope::Current => "current",
        DelegateStateScope::AnyRun => "any_run",
    }
}

fn agent_matches_state(
    agent: &crate::multi_agent::AgentSnapshot,
    filter_state: AgentLifecycleState,
    state_scope: DelegateStateScope,
) -> bool {
    if agent.state == filter_state {
        return true;
    }
    matches!(state_scope, DelegateStateScope::AnyRun)
        && agent
            .terminal_status_history
            .iter()
            .copied()
            .any(|state| state == filter_state)
}
```

- [ ] **Step 6: Include `state_scope` in cursor query and details**

Add `state_scope: String,` to `DelegateListCursorQuery`.

In `DelegateListCursorQuery::from_input`, add:

```rust
state_scope: state_scope_label(input.state_scope).to_owned(),
```

after the `state` field.

In the `details["query"]` object, add:

```rust
"state_scope": state_scope_label(input.state_scope),
```

- [ ] **Step 7: Use historical matching for agents and expose history in rows**

In the agent loop in `ListDelegatesTool::execute`, replace:

```rust
if let Some(filter_state) = input.state
    && agent.state != filter_state
{
    continue;
}
```

with:

```rust
if let Some(filter_state) = input.state
    && !agent_matches_state(agent, filter_state, input.state_scope)
{
    continue;
}
```

After `row["kind"] = json!("agent");`, add:

```rust
row["current_status"] = json!(agent.state.as_str());
row["terminal_status_history"] = json!(
    agent
        .terminal_status_history
        .iter()
        .map(|state| state.as_str())
        .collect::<Vec<_>>()
);
```

Leave swarm filtering as current-state only. Swarms do not currently have per-run resume history.

- [ ] **Step 8: Verify Task 6 tests pass**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_any_run_state_finds_resumed_cancelled_agent --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_current_state_does_not_match_resumed_cancelled_agent --exact --nocapture
```

Expected: PASS.

- [ ] **Step 9: Non-mutating checkpoint**

Run:

```bash
git diff -- crates/neo-agent-core/src/multi_agent/state.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/tools/delegate_controls.rs
```

Expected: diff preserves current-run `state`, records terminal history before resume, and adds explicit `state_scope` filtering. Do not stage or commit without explicit user authorization.

---

### Task 7: Update Tool Documentation And Run Focused Verification

**Files:**
- Modify: `docs/tools.md`
- Verify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Verify: `crates/neo-agent-core/src/tools/delegate.rs`
- Verify: `crates/neo-agent-core/src/tools/background_tasks.rs`

- [ ] **Step 1: Update `docs/tools.md` multi-agent tool descriptions**

Edit the multi-agent/background task sections in `docs/tools.md` so they state:

```markdown
- `TaskList` is the general index for active background work. It lists bash/question tasks plus active delegate agents and delegate swarms.
- `ListDelegates` is the detailed multi-agent index. It supports `include_completed`, `kind`, `state`, `state_scope`, `order`, `limit`, `cursor`, and `include`.
- `DelegateSwarm.items` uses object items: `{ "title": "short label", "value": "item inserted into {{item}}" }`.
- `DelegateSwarm.resume_agent_ids` is a JSON object map: `{ "agent_xxx": "per-agent resume prompt" }`.
- `MessageDelegate` sends live messages only to running agents or running swarm children.
- `InterruptDelegate` interrupts running delegates or swarms. Unknown delegate IDs return `unknown delegate target`.
```

Keep the existing docs concise; remove any examples that still show string-only swarm items.

- [ ] **Step 2: Run all new exact tests individually**

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::interrupt_delegate_unknown_id_uses_delegate_error --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::terminal_delegate_errors_are_action_specific --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_empty_steps_follow_state_filter --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_default_empty_steps_explain_active_default --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate::tests::delegate_swarm_schema_describes_resume_agent_ids_as_object_map --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::background_tasks::tests::task_list_tool_includes_active_runtime_delegate_without_background_record --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::background_tasks::tests::task_list_tool_deduplicates_delegate_background_records --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate::tests::delegate_swarm_request_rejects_empty_item_title --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate::tests::delegate_swarm_titled_items_drive_child_titles_and_prompts --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_any_run_state_finds_resumed_cancelled_agent --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib tools::delegate_controls::tests::list_delegates_current_state_does_not_match_resumed_cancelled_agent --exact --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run targeted formatting check**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS. If formatting fails, run `cargo fmt --all`, then repeat `cargo fmt --all --check`.

- [ ] **Step 4: Run targeted lint for the touched crate**

Run:

```bash
cargo clippy -p neo-agent-core --lib -- -D clippy::all
```

Expected: PASS.

- [ ] **Step 5: Non-mutating final checkpoint**

Run:

```bash
git diff --stat
```

Expected: changed files are limited to:

```text
crates/neo-agent-core/src/tools/delegate_controls.rs
crates/neo-agent-core/src/tools/delegate.rs
crates/neo-agent-core/src/multi_agent/runtime.rs
crates/neo-agent-core/src/multi_agent/state.rs
crates/neo-agent-core/src/multi_agent/mod.rs
crates/neo-agent-core/src/tools/background_tasks.rs
docs/tools.md
```

Do not stage or commit without explicit user authorization.

## Self-Review

- Spec coverage: all seven report issues map to Tasks 1-6, and Task 7 covers docs plus focused verification.
- Placeholder scan: this plan contains concrete file paths, helper names, test names, code snippets, and exact commands.
- Type consistency: `DelegateSwarmItem { title, value }`, `terminal_status_history`, `DelegateStateScope`, and action-aware terminal helpers are introduced before later tasks reference them.
