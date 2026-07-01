# Neo Multi-Agent P2 Swarm Entity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make swarms first-class multi-agent entities that can be listed, waited, messaged, stopped, interrupted, resumed, and inspected like delegates.

**Architecture:** Promote `SwarmSnapshot` from a render snapshot into a runtime-owned entity with lifecycle state and aggregate counts. Control tools accept either `agent_id` or `swarm_id`; swarm actions operate on child agents and return per-child results. `DelegateSwarm` keeps the canonical template contract: only `{{item}}` and optional `{{description}}` are supported.

**Tech Stack:** Rust 2024, `tokio`, `serde`, `schemars`, `serde_json`, `AgentRuntime`, `ToolRegistry`, `cargo nextest run`.

---

## Source Spec

Use `/Users/chenyuanhao/Workspace/neo/docs/superpowers/specs/2026-06-30-neo-multi-agent-hardening-design.md`.

This plan covers:

- Section 6.2 Swarm Entity.
- Section 9.3 MessageDelegate Swarm Target.
- Section 10 DelegateSwarm.
- Section 11 ListDelegates.
- Section 12 WaitDelegate.
- Section 13 TaskOutput.
- Section 14 TaskStop for swarm targets.
- Acceptance criteria under Swarm.

P1 must be complete first.

## Constraints

- Start implementation with `icm recall-context "Neo multi-agent P2 swarm first-class entity" --limit 5`.
- Use CodeGraph before grep/read for symbol discovery in this repo.
- Do not run bare `cargo test`; use `cargo nextest run ...`.
- Do not mutate git unless the user explicitly authorizes that exact command.
- Do not support old template aliases. Supported placeholders are exactly `{{item}}` and `{{description}}`.
- Do not make `description` optional in tool or Lua schema.
- Do not collapse swarm children into one summary-only row. Swarm output must include aggregate plus per-child rows.

## Current Code Touchpoints

- `crates/neo-agent-core/src/multi_agent/state.rs`
  - `SwarmSnapshot` has `swarm_id`, `description`, `mode`, `max_concurrency`, `children`.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - `start_swarm`, `update_swarm_child`, and `run_swarm_children` already exist.
- `crates/neo-agent-core/src/tools/delegate.rs`
  - `DelegateSwarmTool` validates items/template and starts child delegates.
- `crates/neo-agent-core/src/tools/delegate_controls.rs`
  - `ListDelegates`, `WaitDelegate`, `InterruptDelegate`, `MessageDelegate` are agent-biased.
- `crates/neo-agent-core/src/tools/background_tasks.rs`
  - Background swarm snapshots exist but lack rich output and terminal semantics.
- `crates/neo-agent-core/src/tools/task.rs`
  - `TaskOutput` and `TaskStop` route through `BackgroundTaskManager`.
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- `crates/neo-agent-core/tests/multi_agent_background.rs`

## File Structure

Modify:

- `crates/neo-agent-core/src/multi_agent/state.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/tools/delegate.rs`
- `crates/neo-agent-core/src/tools/delegate_controls.rs`
- `crates/neo-agent-core/src/tools/background_tasks.rs`
- `crates/neo-agent-core/src/tools/task.rs`
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- `crates/neo-agent-core/tests/multi_agent_background.rs`

Do not modify TUI rendering in this plan except compile repairs for changed snapshot fields. P5 owns final visual polish.

## Desired End State

- `SwarmSnapshot` includes `state`, `role`, and `aggregate`.
- `SwarmAggregate` counts total, queued, running, completed, failed, cancelled, timed_out.
- `ListDelegates(kind="swarm")` returns swarm rows.
- Default `ListDelegates` order is newest first, with `limit`, `cursor`, `kind`, `state`.
- `WaitDelegate(swarm_id)` returns aggregate and per-child results.
- `TaskOutput(swarm_id)` returns useful swarm aggregate and item results.
- `InterruptDelegate(swarm_id)` and `TaskStop(swarm_id)` cancel non-terminal children only.
- `MessageDelegate(swarm_id)` broadcasts only to running child agents.
- `DelegateSwarm` supports `resume_agent_ids` alone or mixed with new `items`.
- Duplicate expanded prompts are rejected before any child starts.

## Task 1: Add Swarm State And Aggregate Types

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add failing aggregate unit test**

Append to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
#[test]
fn swarm_aggregate_counts_child_states_and_derives_status() {
    use neo_agent_core::multi_agent::{AgentLifecycleState, SwarmAggregate};

    let aggregate = SwarmAggregate::from_states([
        AgentLifecycleState::Completed,
        AgentLifecycleState::Failed,
        AgentLifecycleState::Cancelled,
        AgentLifecycleState::Queued,
    ]);

    assert_eq!(aggregate.total, 4);
    assert_eq!(aggregate.completed, 1);
    assert_eq!(aggregate.failed, 1);
    assert_eq!(aggregate.cancelled, 1);
    assert_eq!(aggregate.queued, 1);
    assert_eq!(aggregate.status(), AgentLifecycleState::Queued);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL because `SwarmAggregate` does not exist.

- [ ] **Step 3: Add `SwarmAggregate`**

In `crates/neo-agent-core/src/multi_agent/state.rs`, add:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmAggregate {
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub timed_out: usize,
}

impl SwarmAggregate {
    #[must_use]
    pub fn from_states(states: impl IntoIterator<Item = AgentLifecycleState>) -> Self {
        let mut aggregate = Self::default();
        for state in states {
            aggregate.total += 1;
            match state {
                AgentLifecycleState::Queued => aggregate.queued += 1,
                AgentLifecycleState::Running => aggregate.running += 1,
                AgentLifecycleState::Completed => aggregate.completed += 1,
                AgentLifecycleState::Failed => aggregate.failed += 1,
                AgentLifecycleState::Cancelled => aggregate.cancelled += 1,
                AgentLifecycleState::TimedOut => aggregate.timed_out += 1,
            }
        }
        aggregate
    }

    #[must_use]
    pub const fn status(self) -> AgentLifecycleState {
        if self.running > 0 {
            AgentLifecycleState::Running
        } else if self.queued > 0 {
            AgentLifecycleState::Queued
        } else if self.failed > 0 || self.timed_out > 0 {
            AgentLifecycleState::Failed
        } else if self.cancelled > 0 {
            AgentLifecycleState::Cancelled
        } else {
            AgentLifecycleState::Completed
        }
    }
}
```

Export `SwarmAggregate` from `crates/neo-agent-core/src/multi_agent/mod.rs`.

- [ ] **Step 4: Extend `SwarmSnapshot`**

Change `SwarmSnapshot`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmSnapshot {
    pub swarm_id: String,
    pub description: String,
    pub role: AgentRole,
    pub mode: AgentRunMode,
    pub state: AgentLifecycleState,
    #[serde(default = "default_swarm_max_concurrency")]
    pub max_concurrency: usize,
    pub aggregate: SwarmAggregate,
    pub children: Vec<SwarmChildSnapshot>,
}
```

Add fields to all existing test fixtures. Use `role: AgentRole::Coder`, `state: AgentLifecycleState::Running` for running fixtures, and `aggregate: SwarmAggregate::from_states(children.iter().map(|child| child.agent.state))`.

- [ ] **Step 5: Run aggregate test**

Run:

```bash
```

Expected: PASS.

## Task 2: Store And Update First-Class Swarm Entities

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add failing runtime swarm lookup test**

Append:

```rust
#[tokio::test]
async fn runtime_keeps_swarm_entity_after_foreground_completion() {
    let (registry, ctx) = registry_with_multi_agent();

    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "count files",
                "items": ["a", "b"],
                "prompt_template": "Inspect {{item}} for {{description}}",
                "mode": "foreground"
            }),
        )
        .await
        .expect("swarm should complete");

    let swarm_id = result
        .details
        .as_ref()
        .and_then(|details| details.get("swarm_id"))
        .and_then(serde_json::Value::as_str)
        .expect("swarm_id");
    let snapshot = ctx
        .multi_agent
        .swarm_snapshot(swarm_id)
        .await
        .expect("swarm remains queryable");

    assert_eq!(snapshot.swarm_id, swarm_id);
    assert_eq!(snapshot.aggregate.total, 2);
    assert_eq!(snapshot.state, AgentLifecycleState::Completed);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL if no public `swarm_snapshot` exists or state/aggregate is not updated.

- [ ] **Step 3: Add swarm registry helpers**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, add:

```rust
pub fn swarm_snapshot(&self, swarm_id: &str) -> Option<SwarmSnapshot> {
    self.state
        .lock()
        .expect("multi-agent state poisoned")
        .swarms
        .get(swarm_id)
        .cloned()
}

pub fn list_swarms(&self) -> Vec<SwarmSnapshot> {
    self.state
        .lock()
        .expect("multi-agent state poisoned")
        .swarms
        .values()
        .cloned()
        .collect()
}

fn refresh_swarm(snapshot: &mut SwarmSnapshot) {
    snapshot.aggregate =
        SwarmAggregate::from_states(snapshot.children.iter().map(|child| child.agent.state));
    snapshot.state = snapshot.aggregate.status();
}
```

The current runtime state already has `swarms: BTreeMap<String, SwarmSnapshot>`. Use that map; do not add a second swarm registry.

- [ ] **Step 4: Update swarm creation and child update paths**

When creating a swarm, set:

```rust
snapshot.role = request.role.unwrap_or_default();
snapshot.state = AgentLifecycleState::Queued;
snapshot.aggregate = SwarmAggregate::from_states(
    snapshot.children.iter().map(|child| child.agent.state),
);
```

After every child state mutation, call `refresh_swarm(&mut snapshot)` before storing or emitting.

- [ ] **Step 5: Run runtime swarm lookup test**

Run:

```bash
```

Expected: PASS.

## Task 3: Harden DelegateSwarm Template And Resume Schema

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add failing template contract tests**

Append:

```rust
#[tokio::test]
async fn delegate_swarm_rejects_unknown_template_placeholder() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "audit",
                "items": ["one"],
                "prompt_template": "Audit {{task}} and {{item}}"
            }),
        )
        .await
        .expect("tool returns validation result");

    assert!(result.is_error);
    assert!(
        result.content.contains("only {{item}} and {{description}} are supported"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn delegate_swarm_rejects_duplicate_expanded_prompts() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "audit",
                "items": ["same", "same"],
                "prompt_template": "Audit {{item}}"
            }),
        )
        .await
        .expect("tool returns validation result");

    assert!(result.is_error);
    assert!(
        result.content.contains("duplicate expanded child prompt"),
        "{}",
        result.content
    );
}
```

- [ ] **Step 2: Add failing resume schema test**

Append:

```rust
#[tokio::test]
async fn delegate_swarm_resume_agent_ids_restarts_existing_children() {
    let (registry, ctx) = registry_with_multi_agent();
    let first = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "initial child",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");
    let agent_id = first
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("agent_id")
        .to_owned();

    let swarm = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "resume unfinished child",
                "resume_agent_ids": {
                    agent_id.clone(): "continue inside swarm"
                },
                "mode": "foreground"
            }),
        )
        .await
        .expect("swarm resume should complete");

    assert!(!swarm.is_error, "{}", swarm.content);
    assert!(
        swarm.content.contains(agent_id.as_str()),
        "{}",
        swarm.content
    );
}
```

If Rust macro parsing rejects `agent_id.clone()` inside `json!`, build the value with `serde_json::Map` in the test.

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL.

- [ ] **Step 4: Update `DelegateSwarmRequest`**

In `runtime.rs`, change request fields:

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateSwarmRequest {
    #[schemars(description = "Required non-empty human title for the swarm.")]
    pub description: String,
    #[serde(default)]
    #[schemars(description = "New child task items. When present, prompt_template is required and must contain {{item}}.")]
    pub items: Vec<String>,
    #[serde(default)]
    #[schemars(description = "Template for new child tasks. Supports exactly {{item}} and optional {{description}}.")]
    pub prompt_template: Option<String>,
    #[serde(default)]
    #[schemars(description = "Existing agent_id to prompt mapping for resumed child agents.")]
    pub resume_agent_ids: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    #[schemars(description = "Subagent role for new children. Defaults to coder.")]
    pub role: Option<AgentRole>,
    #[serde(default)]
    #[schemars(description = "Run mode. Defaults to foreground.")]
    pub mode: AgentRunMode,
    #[schemars(description = "Optional max parallel child agents. Must be greater than 0 when provided.")]
    pub max_concurrency: Option<usize>,
}
```

- [ ] **Step 5: Replace swarm validation**

In `validate_swarm_request`, enforce:

```rust
if request.description.trim().is_empty() {
    return Err(invalid(tool, "description must not be empty"));
}
if request.items.is_empty() && request.resume_agent_ids.is_empty() {
    return Err(invalid(tool, "items or resume_agent_ids must contain at least one child"));
}
if !request.items.is_empty() && request.prompt_template.as_deref().unwrap_or("").trim().is_empty() {
    return Err(invalid(tool, "prompt_template is required when items are provided"));
}
if let Some(template) = request.prompt_template.as_deref() {
    if !request.items.is_empty() && !template.contains("{{item}}") {
        return Err(invalid(tool, "prompt_template must include {{item}}; only {{item}} and optional {{description}} are supported"));
    }
    reject_unknown_placeholders(tool, template)?;
}
for (index, item) in request.items.iter().enumerate() {
    if item.trim().is_empty() {
        return Err(invalid(tool, format!("items[{index}] must not be empty")));
    }
}
for (agent_id, prompt) in &request.resume_agent_ids {
    if !agent_id.starts_with("agent_") {
        return Err(invalid(tool, "resume_agent_ids keys must be agent_id values"));
    }
    if prompt.trim().is_empty() {
        return Err(invalid(tool, format!("resume_agent_ids[{agent_id}] must not be empty")));
    }
}
if request.max_concurrency == Some(0) {
    return Err(invalid(tool, "max_concurrency must be greater than 0 when provided"));
}
let mut expanded = std::collections::HashSet::new();
if let Some(template) = request.prompt_template.as_deref() {
    for item in &request.items {
        let prompt = expand_swarm_prompt(template, item, &request.description);
        if !expanded.insert(prompt.clone()) {
            return Err(invalid(tool, format!("duplicate expanded child prompt: {prompt}")));
        }
    }
}
for prompt in request.resume_agent_ids.values() {
    if !expanded.insert(prompt.clone()) {
        return Err(invalid(tool, format!("duplicate expanded child prompt: {prompt}")));
    }
}
```

Add helpers:

```rust
fn invalid(tool: &str, message: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: message.into(),
    }
}

fn reject_unknown_placeholders(tool: &str, template: &str) -> Result<(), ToolError> {
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err(invalid(tool, "template placeholder is missing closing }}"));
        };
        let name = after_start[..end].trim();
        if name != "item" && name != "description" {
            return Err(invalid(
                tool,
                "only {{item}} and {{description}} are supported in prompt_template",
            ));
        }
        rest = &after_start[end + 2..];
    }
    Ok(())
}

fn expand_swarm_prompt(template: &str, item: &str, description: &str) -> String {
    template
        .replace("{{item}}", item)
        .replace("{{description}}", description)
}
```

- [ ] **Step 6: Implement resumed children in swarm creation**

In the `DelegateSwarmTool` creation path, build child requests from:

- New `items` expanded through `prompt_template`.
- Existing `resume_agent_ids` by calling P1 `start_resume_delegate`.

For resumed children:

```rust
let resumed = ctx
    .multi_agent
    .start_resume_delegate(agent_id, &DelegateRequest {
        task: prompt.clone(),
        resume: Some(agent_id.clone()),
        title: None,
        role: None,
        mode: request.mode,
        context: DelegateContext::Inherit,
    })
    .await
    .map_err(|message| ToolError::InvalidInput {
        tool: "DelegateSwarm".to_owned(),
        message,
    })?;
```

Keep the original agent id, display name, role, path, and profile.

- [ ] **Step 7: Run template and resume tests**

Run:

```bash
```

Expected: PASS.

## Task 4: ListDelegates Filtering, Ordering, And Swarm Rows

**Files:**

- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Add failing list test**

Append:

```rust
#[tokio::test]
async fn list_delegates_can_filter_swarms_and_orders_newest_first() {
    let (registry, ctx) = registry_with_multi_agent();
    registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "first swarm",
                "items": ["a"],
                "prompt_template": "inspect {{item}}",
                "mode": "background"
            }),
        )
        .await
        .expect("first swarm starts");
    let second = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "second swarm",
                "items": ["b"],
                "prompt_template": "inspect {{item}}",
                "mode": "background"
            }),
        )
        .await
        .expect("second swarm starts");
    let second_id = second
        .details
        .as_ref()
        .and_then(|details| details.get("swarm_id"))
        .and_then(serde_json::Value::as_str)
        .expect("swarm_id")
        .to_owned();

    let listed = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "kind": "swarm",
                "include_completed": true,
                "limit": 1,
                "order": "newest"
            }),
        )
        .await
        .expect("list should succeed");

    assert!(listed.content.contains(second_id.as_str()), "{}", listed.content);
    assert!(!listed.content.contains("first swarm"), "{}", listed.content);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL because `ListDelegatesInput` only has `include_completed` and swarms are not first-class list rows.

- [ ] **Step 3: Extend list input**

In `delegate_controls.rs`:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DelegateListKind {
    Agent,
    Swarm,
    All,
}

impl Default for DelegateListKind {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DelegateListOrder {
    Newest,
    Oldest,
}

impl Default for DelegateListOrder {
    fn default() -> Self {
        Self::Newest
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListDelegatesInput {
    #[serde(default)]
    include_completed: bool,
    #[serde(default)]
    kind: DelegateListKind,
    #[serde(default)]
    state: Option<AgentLifecycleState>,
    #[serde(default = "default_delegate_list_limit")]
    limit: usize,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    order: DelegateListOrder,
}

fn default_delegate_list_limit() -> usize {
    20
}
```

- [ ] **Step 4: Render swarm rows**

In `ListDelegatesTool::execute`, collect rows from both `ctx.multi_agent.list_agents(include_completed)` and `ctx.multi_agent.list_swarms()`.

Use this textual row format for swarms:

```text
swarm_id: swarm_xxx
kind: swarm
status: running
description: second swarm
aggregate: total=1 queued=0 running=1 completed=0 failed=0 cancelled=0 timed_out=0
```

Use JSON details:

```rust
serde_json::json!({
    "delegates": rows,
    "next_cursor": next_cursor,
})
```

Rows must include:

- `kind`: `"agent"` or `"swarm"`.
- `id`: agent id or swarm id.
- `status`.
- `created_index`: runtime insertion order.

Add `created_index: u64` to `AgentSnapshot` and `SwarmSnapshot`. Add `next_created_index: u64` to `MultiAgentState`, initialize it to `0`, increment it when creating agents and swarms, and use it for newest/oldest ordering.

- [ ] **Step 5: Run list test**

Run:

```bash
```

Expected: PASS.

## Task 5: WaitDelegate And TaskOutput For Swarms

**Files:**

- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Add failing wait/output test**

Append:

```rust
#[tokio::test]
async fn wait_and_task_output_return_swarm_aggregate_and_items() {
    let (registry, ctx) = registry_with_multi_agent();
    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "read-only audit",
                "items": ["core", "tui"],
                "prompt_template": "Audit {{item}}",
                "mode": "background"
            }),
        )
        .await
        .expect("swarm starts");
    let swarm_id = started
        .details
        .as_ref()
        .and_then(|details| details.get("swarm_id"))
        .and_then(serde_json::Value::as_str)
        .expect("swarm_id")
        .to_owned();

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "id": swarm_id, "timeout_ms": 5000 }),
        )
        .await
        .expect("wait succeeds");
    assert!(waited.content.contains("kind: swarm"), "{}", waited.content);
    assert!(waited.content.contains("aggregate:"), "{}", waited.content);
    assert!(waited.content.contains("items:"), "{}", waited.content);

    let output = registry
        .run(
            "TaskOutput",
            &ctx,
            serde_json::json!({ "task_id": swarm_id, "block": false }),
        )
        .await
        .expect("task output succeeds");
    assert!(output.content.contains("kind: swarm"), "{}", output.content);
    assert!(output.content.contains("aggregate:"), "{}", output.content);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL because `TaskOutput` lacks rich swarm output.

- [ ] **Step 3: Add `format_swarm_result` helper**

In `delegate_controls.rs`, add:

```rust
fn format_swarm_result(swarm: &SwarmSnapshot) -> ToolResult {
    let mut content = format!(
        "kind: swarm\nswarm_id: {}\nstatus: {}\naggregate: total={} queued={} running={} completed={} failed={} cancelled={} timed_out={}\nitems:",
        swarm.swarm_id,
        swarm.state.as_str(),
        swarm.aggregate.total,
        swarm.aggregate.queued,
        swarm.aggregate.running,
        swarm.aggregate.completed,
        swarm.aggregate.failed,
        swarm.aggregate.cancelled,
        swarm.aggregate.timed_out,
    );
    let items = swarm
        .children
        .iter()
        .map(|child| {
            serde_json::json!({
                "index": child.item_index,
                "item": child.item,
                "agent_id": child.agent.id.as_str(),
                "status": child.agent.state.as_str(),
                "summary": child.agent.outcome.as_ref().map(|outcome| outcome.summary.clone()),
            })
        })
        .collect::<Vec<_>>();
    for child in &swarm.children {
        content.push_str(&format!(
            "\n- {} {} {}",
            child.item_index,
            child.agent.id.as_str(),
            child.agent.state.as_str()
        ));
    }
    ToolResult::ok(content).with_details(serde_json::json!({
        "kind": "swarm",
        "swarm_id": swarm.swarm_id,
        "status": swarm.state.as_str(),
        "aggregate": swarm.aggregate,
        "items": items,
        "resume_hint": "Call DelegateSwarm with resume_agent_ids for unfinished children.",
    }))
}
```

- [ ] **Step 4: Use helper in `WaitDelegate` for swarm ids**

In `WaitDelegateTool::execute`, if `input.id.starts_with("swarm_")`, poll `ctx.multi_agent.swarm_snapshot(&input.id)` until `swarm.state.is_terminal()` or timeout. Return `format_swarm_result(&swarm)`.

- [ ] **Step 5: Use helper from `BackgroundTaskManager::output`**

In `snapshot_result`, for `BackgroundTaskKind::DelegateSwarm` with `snapshot.swarm.is_some()`, return the same shape:

```rust
kind: swarm
swarm_id: ...
status: ...
aggregate: ...
items:
```

Keep JSON details aligned with `format_swarm_result`.

- [ ] **Step 6: Run wait/output test**

Run:

```bash
```

Expected: PASS.

## Task 6: Swarm Stop, Interrupt, And Message Broadcast

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Add failing swarm control tests**

Append:

```rust
#[tokio::test]
async fn interrupt_delegate_accepts_swarm_id_and_cancels_running_children() {
    let (registry, ctx) = registry_with_multi_agent();
    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "long swarm",
                "items": ["a", "b"],
                "prompt_template": "Wait and inspect {{item}}",
                "mode": "background",
                "max_concurrency": 1
            }),
        )
        .await
        .expect("swarm starts");
    let swarm_id = started.details.as_ref().unwrap()["swarm_id"].as_str().unwrap().to_owned();

    let interrupted = registry
        .run("InterruptDelegate", &ctx, serde_json::json!({ "id": swarm_id }))
        .await
        .expect("interrupt returns result");

    assert!(!interrupted.is_error, "{}", interrupted.content);
    assert!(interrupted.content.contains("status: cancelled"), "{}", interrupted.content);
}

#[tokio::test]
async fn message_delegate_broadcasts_to_running_swarm_children() {
    let (registry, ctx) = registry_with_multi_agent();
    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "live swarm",
                "items": ["a", "b"],
                "prompt_template": "Wait for follow-up about {{item}}",
                "mode": "background",
                "max_concurrency": 2
            }),
        )
        .await
        .expect("swarm starts");
    let swarm_id = started.details.as_ref().unwrap()["swarm_id"].as_str().unwrap().to_owned();

    let message = registry
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({
                "id": swarm_id,
                "message": "continue now"
            }),
        )
        .await
        .expect("message returns result");

    assert!(!message.is_error, "{}", message.content);
    assert!(message.content.contains("delivered:"), "{}", message.content);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL because swarm ids are not valid targets.

- [ ] **Step 3: Add runtime swarm cancellation**

In `MultiAgentRuntime`:

```rust
pub fn cancel_swarm(&self, swarm_id: &str) -> Result<SwarmSnapshot, String> {
    let mut state = self.state.lock().expect("multi-agent state poisoned");
    let Some(swarm) = state.swarms.get_mut(swarm_id) else {
        return Err(format!("unknown delegate target `{swarm_id}`"));
    };
    if swarm.state.is_terminal() {
        return Err(format!("swarm already {}; terminal swarm state is immutable", swarm.state.as_str()));
    }
    for child in &mut swarm.children {
        if !child.agent.state.is_terminal() {
            child.agent.state = AgentLifecycleState::Cancelled;
            child.agent.outcome = Some(AgentTerminalOutcome {
                summary: "Cancelled by user.".to_owned(),
                is_error: true,
            });
            if let Some(agent) = state.agents.get_mut(child.agent.id.as_str()) {
                agent.state = AgentLifecycleState::Cancelled;
                agent.outcome = child.agent.outcome.clone();
            }
        }
    }
    refresh_swarm(swarm);
    Ok(swarm.clone())
}
```

- [ ] **Step 4: Add runtime swarm broadcast**

In `MultiAgentRuntime`:

```rust
pub async fn broadcast_live_swarm_message(
    &self,
    swarm_id: &str,
    message: String,
) -> Result<(Vec<String>, Vec<(String, AgentLifecycleState)>), String> {
    let Some(swarm) = self.swarm_snapshot(swarm_id) else {
        return Err(format!("unknown delegate target `{swarm_id}`"));
    };
    let mut delivered = Vec::new();
    let mut skipped = Vec::new();
    for child in swarm.children {
        if child.agent.state == AgentLifecycleState::Running
            && self
                .deliver_live_message(child.agent.id.as_str(), message.clone())
        {
            delivered.push(child.agent.id.as_str().to_owned());
        } else {
            skipped.push((child.agent.id.as_str().to_owned(), child.agent.state));
        }
    }
    if delivered.is_empty() {
        return Err("swarm has no running children; use DelegateSwarm with resume_agent_ids to continue unfinished children".to_owned());
    }
    Ok((delivered, skipped))
}
```

- [ ] **Step 5: Route `InterruptDelegate` and `MessageDelegate` by id prefix**

In `delegate_controls.rs`:

- `agent_` target uses P1 agent path.
- `swarm_` target uses `cancel_swarm` or `broadcast_live_swarm_message`.
- Any other id returns `unknown delegate target`.

For swarm message details:

```rust
serde_json::json!({
    "target": input.id,
    "delivered": delivered,
    "skipped": skipped.iter().map(|(agent_id, state)| {
        serde_json::json!({ "agent_id": agent_id, "state": state.as_str() })
    }).collect::<Vec<_>>(),
})
```

- [ ] **Step 6: Run swarm control tests**

Run:

```bash
```

Expected: PASS.

## Task 7: P2 Verification And Commit Boundary

**Files:**

- Verify all files changed by this plan.

- [ ] **Step 1: Run multi-agent core and background tests**

Run:

```bash
```

Expected: both PASS.

- [ ] **Step 2: Run template alias scan**

Run:

```bash
rg -n "TASK_PLACEHOLDER|\\{task\\}|%s|\\{\\}|\\{item\\}" crates/neo-agent-core/src crates/neo-agent-core/tests
```

Expected:

- No supported-alias descriptions.
- No test expecting `{item}`, `{task}`, `%s`, `{}`, or `TASK_PLACEHOLDER`.
- `{{item}}` and `{{description}}` may appear.

- [ ] **Step 3: Commit if authorized**

Only if the user has explicitly authorized git mutation in this session:

```bash
git add crates/neo-agent-core/src/multi_agent/state.rs \
  crates/neo-agent-core/src/multi_agent/runtime.rs \
  crates/neo-agent-core/src/tools/delegate.rs \
  crates/neo-agent-core/src/tools/delegate_controls.rs \
  crates/neo-agent-core/src/tools/background_tasks.rs \
  crates/neo-agent-core/src/tools/task.rs \
  crates/neo-agent-core/tests/multi_agent_runtime.rs \
  crates/neo-agent-core/tests/multi_agent_background.rs
git commit -m "feat: make multi-agent swarms first-class"
```

Expected: one logical commit for P2.
