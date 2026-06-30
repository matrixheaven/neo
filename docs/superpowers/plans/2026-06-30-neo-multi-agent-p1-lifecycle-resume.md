# Neo Multi-Agent P1 Lifecycle And Resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make delegate lifecycle state truthful and immutable, add `Delegate.resume`, and make `MessageDelegate` live-only.

**Architecture:** Keep the existing `MultiAgentRuntime` as the owner of agent snapshots and add small lifecycle helpers there. `Delegate` becomes a create-or-resume tool, while `MessageDelegate` stops being an offline mailbox API and only sends live follow-up to currently running agents. Background task status vocabulary is aligned with agent lifecycle by returning and persisting `cancelled`, never `stopped`, for delegate cancellation.

**Tech Stack:** Rust 2024, `tokio`, `serde`, `schemars`, `neo-ai::FakeModelClient`, `AgentRuntime`, `ToolRegistry`, `cargo nextest run`.

---

## Source Spec

Use `/Users/chenyuanhao/Workspace/neo/docs/superpowers/specs/2026-06-30-neo-multi-agent-hardening-design.md`.

This plan covers:

- Section 7 Lifecycle Contract.
- Section 8 Delegate Tool.
- Section 9.2 MessageDelegate Agent Target.
- Section 14 TaskStop for agent targets.
- Acceptance criteria under Lifecycle and Resume and Message.

This plan does not implement first-class swarm addressing, Lua workflow hardening, role policy enforcement, or TUI polish. Those are P2-P5.

## Constraints

- Start implementation with `icm recall-context "Neo multi-agent P1 lifecycle resume" --limit 5`.
- Use CodeGraph before grep/read for symbol discovery in this repo.
- Do not run bare `cargo test`; use `cargo nextest run ...`.
- Do not mutate git unless the user explicitly authorizes that exact command.
- Do not preserve offline mailbox compatibility. `MessageDelegate` to terminal or idle agents must return an error.
- Do not keep `stopped` for delegate cancellation. The canonical user-facing and persisted word is `cancelled`.
- Do not add role aliases. `orchestrator` is canonical; `harness` is gone.

## Current Code Touchpoints

- `crates/neo-agent-core/src/multi_agent/state.rs`
  - `AgentLifecycleState` currently lacks `TimedOut` and helper methods.
  - `AgentSnapshot` currently stores long prompt in `task`; P5 will split title/prompt, so this plan only adds `title` request support and stores a deterministic short title where available without forcing TUI changes.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - `DelegateRequest` currently has `task`, `role`, `mode`, `context`.
  - `start_delegate` and `complete_delegate` already exist.
  - Mailbox helpers exist and must be narrowed to live delivery only.
- `crates/neo-agent-core/src/tools/delegate.rs`
  - `DelegateTool` validates and runs foreground/background delegates.
- `crates/neo-agent-core/src/tools/delegate_controls.rs`
  - `InterruptDelegateTool`, `MessageDelegateTool`, `WaitDelegateTool`, and `ListDelegatesTool` use `MultiAgentRuntime`.
- `crates/neo-agent-core/src/tools/background_tasks.rs`
  - `BackgroundTaskStatus::Stopped` is returned by delegate stop paths.
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - Existing fake-runtime delegate tests.
- `crates/neo-agent-core/tests/multi_agent_background.rs`
  - Existing control-tool tests.

## File Structure

Modify:

- `crates/neo-agent-core/src/multi_agent/state.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/tools/delegate.rs`
- `crates/neo-agent-core/src/tools/delegate_controls.rs`
- `crates/neo-agent-core/src/tools/background_tasks.rs`
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- `crates/neo-agent-core/tests/multi_agent_background.rs`

Do not modify:

- `crates/neo-agent-core/src/workflow/lua.rs`
- `crates/neo-tui/**`

## Desired End State

- `AgentLifecycleState` includes `TimedOut` and has `is_terminal()` plus `as_str()`.
- `InterruptDelegate(completed_agent)` returns `is_error: true` and does not mutate state.
- `TaskStop(completed_delegate_task)` returns an error that includes `already completed`.
- Running `InterruptDelegate` and running `TaskStop` both persist and return `cancelled`.
- `Delegate` schema includes `resume?: string` and `title?: string`.
- `Delegate({ resume: agent_id, task: "continue" })` resumes the same agent identity.
- `Delegate({ resume: agent_id, role: "coder", task: "continue" })` is rejected.
- `Delegate({ resume: swarm_id, task: "continue" })` is rejected.
- `MessageDelegate(completed_agent)` is rejected with a resume hint.
- `MessageDelegate(running_agent)` still delivers through the live steer handle.

## Task 1: Add Lifecycle Helpers And Terminal Immutability Tests

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Add failing test for completed interrupt immutability**

Append this test to `crates/neo-agent-core/tests/multi_agent_background.rs` near the existing `InterruptDelegate` tests:

```rust
#[tokio::test]
async fn interrupt_delegate_rejects_completed_agent_without_mutating_state() {
    let (registry, ctx) = registry_with_multi_agent();
    let delegate = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "return exactly done",
                "mode": "foreground"
            }),
        )
        .await
        .expect("foreground delegate should complete");
    let agent_id = delegate
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("delegate result should include agent_id")
        .to_owned();

    let interrupted = registry
        .run(
            "InterruptDelegate",
            &ctx,
            serde_json::json!({ "id": agent_id }),
        )
        .await
        .expect("interrupt should return a tool result");

    assert!(interrupted.is_error);
    assert!(
        interrupted.content.contains("already completed"),
        "{}",
        interrupted.content
    );

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "id": agent_id, "timeout_ms": 1 }),
        )
        .await
        .expect("completed delegate remains queryable");
    assert!(
        waited.content.contains("status: completed"),
        "{}",
        waited.content
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL because `InterruptDelegate` currently rewrites completed agents to `cancelled`.

- [ ] **Step 3: Add lifecycle helper methods**

In `crates/neo-agent-core/src/multi_agent/state.rs`, change `AgentLifecycleState` to include `TimedOut` and add helpers:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl AgentLifecycleState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}
```

- [ ] **Step 4: Add a shared terminal-state error formatter**

In `crates/neo-agent-core/src/tools/delegate_controls.rs`, add this helper near the top-level tool helpers:

```rust
fn terminal_delegate_error(agent_id: &str, state: AgentLifecycleState) -> ToolResult {
    ToolResult::error(format!(
        "agent already {}; terminal delegate state is immutable. To continue it, call Delegate with resume=\"{}\".",
        state.as_str(),
        agent_id
    ))
    .with_details(serde_json::json!({
        "agent_id": agent_id,
        "status": state.as_str(),
        "terminal": true,
        "resume_hint": format!("Delegate with resume=\"{agent_id}\""),
    }))
}
```

- [ ] **Step 5: Guard `InterruptDelegate` before cancellation**

In `InterruptDelegateTool::execute`, after resolving the `AgentSnapshot` and before calling `cancel_agent`, add this branch:

```rust
if agent.state.is_terminal() {
    return Ok(terminal_delegate_error(agent.id.as_str(), agent.state));
}
```

Keep the existing running cancellation path, but ensure it returns `status: cancelled` in content and details.

- [ ] **Step 6: Run focused test**

Run:

```bash
```

Expected: PASS.

## Task 2: Align TaskStop And Background Delegate Cancellation Vocabulary

**Files:**

- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Add failing tests for `TaskStop` terminal and running vocabulary**

Append these tests to `crates/neo-agent-core/tests/multi_agent_background.rs`:

```rust
#[tokio::test]
async fn task_stop_completed_delegate_returns_already_completed_error() {
    let (registry, ctx) = registry_with_multi_agent();
    let delegate = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "return exactly finished",
                "mode": "background"
            }),
        )
        .await
        .expect("background delegate should start");
    let agent_id = delegate
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("delegate result should include agent_id")
        .to_owned();

    registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "id": agent_id, "timeout_ms": 5000 }),
        )
        .await
        .expect("delegate should complete");

    let stopped = registry
        .run(
            "TaskStop",
            &ctx,
            serde_json::json!({ "task_id": agent_id }),
        )
        .await
        .expect("TaskStop should return a tool result");

    assert!(stopped.is_error);
    assert!(stopped.content.contains("already completed"), "{}", stopped.content);
}

#[tokio::test]
async fn task_stop_running_delegate_returns_cancelled_not_stopped() {
    let manager = BackgroundTaskManager::new();
    let snapshot = running_agent_snapshot("agent_task_stop_running");
    manager.start_delegate(snapshot).await;

    let result = manager
        .stop("agent_task_stop_running", "user requested stop", 2048)
        .await
        .expect("running delegate should be cancellable");

    assert!(!result.is_error);
    assert!(result.content.contains("status: cancelled"), "{}", result.content);
    assert!(!result.content.contains("status: stopped"), "{}", result.content);
}
```

Add this local helper beside the existing snapshot fixture helpers in `crates/neo-agent-core/tests/multi_agent_background.rs`:

```rust
fn running_agent_snapshot(id: &str) -> AgentSnapshot {
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id.trim_start_matches("agent_")),
        display_name: AgentDisplayName::new("Gauss"),
        path: AgentPath::root_child(&AgentDisplayName::new("Gauss")),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        state: AgentLifecycleState::Running,
        task: "long running delegate".to_owned(),
        tool_count: 0,
        token_count: 0,
        elapsed: Duration::from_secs(0),
        latest_text: None,
        activity: Vec::new(),
        outcome: None,
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL because `TaskStop` currently returns or persists `Stopped`.

- [ ] **Step 3: Rename delegate cancellation status paths to `Cancelled`**

In `crates/neo-agent-core/src/tools/background_tasks.rs`, change the `BackgroundTaskStatus` enum variant from `Stopped` to `Cancelled`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    WaitingForUser,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}
```

Update `BackgroundTaskStatus::is_active` so `Cancelled` is not active:

```rust
impl BackgroundTaskStatus {
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::WaitingForUser)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::WaitingForUser => "waiting_for_user",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}
```

Replace every `BackgroundTaskStatus::Stopped` in `background_tasks.rs`, task-browser tests, and related tests with `BackgroundTaskStatus::Cancelled`.

- [ ] **Step 4: Return an error when stopping finished delegate tasks**

In `BackgroundTaskManager::stop`, change `StopAction::Already` into two distinct branches:

```rust
enum StopAction {
    AlreadyTerminal(BackgroundTaskSnapshot),
    StopQuestion {
        started_at: Instant,
        description: String,
    },
    StopBash {
        started_at: Instant,
        description: String,
        command: ManagedBackgroundCommand,
    },
}
```

For `DelegateFinished` and `DelegateSwarmFinished`, return an error before `snapshot_result`:

```rust
BackgroundTaskState::DelegateFinished { status, snapshot } => {
    return Ok(ToolResult::error(format!(
        "agent already {}; terminal delegate state is immutable. To continue it, call Delegate with resume=\"{}\".",
        status.as_str(),
        snapshot.id.as_str()
    ))
    .with_details(serde_json::json!({
        "task_id": task_id,
        "kind": "delegate",
        "status": status.as_str(),
        "agent_id": snapshot.id.as_str(),
        "terminal": true,
        "resume_hint": format!("Delegate with resume=\"{}\"", snapshot.id.as_str()),
    })));
}
```

Keep finished Bash and finished question tasks as non-error snapshots; the terminal immutability rule is for delegate entities.

- [ ] **Step 5: Ensure running delegate stop persists `cancelled`**

In the `DelegateRunning` branch of `BackgroundTaskManager::stop`, set both the snapshot state and task status:

```rust
snapshot.state = crate::multi_agent::AgentLifecycleState::Cancelled;
record.state = BackgroundTaskState::DelegateFinished {
    status: BackgroundTaskStatus::Cancelled,
    snapshot: snapshot.clone(),
};
let snap = BackgroundTaskSnapshot {
    task_id: task_id.to_owned(),
    kind: BackgroundTaskKind::Delegate,
    status: BackgroundTaskStatus::Cancelled,
    description: record.description.clone(),
    elapsed: record.started_at.elapsed(),
    output: None,
    answers: None,
    delegate: Some(snapshot),
    swarm: None,
};
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
```

Expected: PASS.

## Task 3: Add `Delegate.resume` Schema And Validation

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add failing validation tests**

Append these tests to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
#[tokio::test]
async fn delegate_resume_rejects_role_override() {
    let (registry, ctx) = registry_with_multi_agent();

    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "resume": "agent_existing",
                "role": "coder",
                "task": "continue"
            }),
        )
        .await
        .expect("tool should return validation result");

    assert!(result.is_error);
    assert!(
        result.content.contains("role must be omitted when resume is set"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn delegate_resume_rejects_swarm_id() {
    let (registry, ctx) = registry_with_multi_agent();

    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "resume": "swarm_123",
                "task": "continue"
            }),
        )
        .await
        .expect("tool should return validation result");

    assert!(result.is_error);
    assert!(
        result.content.contains("resume must be an agent_id"),
        "{}",
        result.content
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL because `DelegateRequest` has no `resume` field and role override is accepted.

- [ ] **Step 3: Change `DelegateRequest` role to optional and add resume/title**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, replace the `DelegateRequest` struct with:

```rust
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateRequest {
    #[schemars(description = "Required non-empty task for the subagent. For resume, this is the next user prompt for the same child agent.")]
    pub task: String,
    #[serde(default)]
    #[schemars(description = "Existing agent_id to continue. Must be omitted for a new agent. Must start with agent_, not swarm_.")]
    pub resume: Option<String>,
    #[serde(default)]
    #[schemars(description = "Short UI title. If omitted, Neo derives a deterministic local title from task.")]
    pub title: Option<String>,
    #[serde(default)]
    #[schemars(description = "Subagent role for new agents only. Defaults to coder. Must be omitted when resume is set.")]
    pub role: Option<AgentRole>,
    #[serde(default)]
    #[schemars(description = "Run mode. Defaults to foreground.")]
    pub mode: AgentRunMode,
    #[serde(default = "default_context")]
    #[schemars(description = "Context mode: inherit, summary, or none. Defaults to inherit.")]
    pub context: DelegateContext,
}

impl DelegateRequest {
    #[must_use]
    pub fn actual_role(&self) -> AgentRole {
        self.role.unwrap_or_default()
    }
}
```

Update all new-agent call sites from `request.role` to `request.actual_role()`.

- [ ] **Step 4: Update `validate_delegate_request`**

In `crates/neo-agent-core/src/tools/delegate.rs`, change `validate_delegate_request` to:

```rust
fn validate_delegate_request(tool: &str, request: &DelegateRequest) -> Result<(), ToolError> {
    if request.task.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "task must not be empty".to_owned(),
        });
    }
    if let Some(resume) = request.resume.as_deref() {
        if !resume.starts_with("agent_") {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: "resume must be an agent_id returned by Delegate, not a swarm_id or task id".to_owned(),
            });
        }
        if request.role.is_some() {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: "role must be omitted when resume is set; resumed agents keep their original role/profile".to_owned(),
            });
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Run validation tests**

Run:

```bash
```

Expected: PASS.

## Task 4: Implement Resume On The Same Agent Identity

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add failing resume behavior test**

Append this test to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
#[tokio::test]
async fn delegate_resume_reuses_agent_identity_and_role() {
    let (registry, ctx) = registry_with_multi_agent();
    let first = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "first investigation",
                "role": "explorer",
                "mode": "foreground"
            }),
        )
        .await
        .expect("first delegate should complete");
    let agent_id = first
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("first delegate should expose agent_id")
        .to_owned();

    let second = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "resume": agent_id,
                "task": "continue with one more check",
                "mode": "foreground"
            }),
        )
        .await
        .expect("resume should complete");

    let details = second.details.as_ref().expect("resume details");
    assert_eq!(
        details.get("agent_id").and_then(serde_json::Value::as_str),
        Some(agent_id.as_str())
    );
    assert_eq!(
        details.get("actual_role").and_then(serde_json::Value::as_str),
        Some("explorer")
    );
    assert!(
        second.content.contains("status: completed"),
        "{}",
        second.content
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL because `Delegate` creates a new agent.

- [ ] **Step 3: Add runtime lookup helpers**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, add methods to `impl MultiAgentRuntime`:

```rust
pub fn agent_snapshot(&self, agent_id: &str) -> Option<AgentSnapshot> {
    self.state
        .lock()
        .expect("multi-agent state poisoned")
        .agents
        .get(agent_id)
        .cloned()
}

pub fn ensure_resumable_agent(&self, agent_id: &str) -> Result<AgentSnapshot, String> {
    let Some(agent) = self.agent_snapshot(agent_id) else {
        return Err(format!("unknown delegate target `{agent_id}`"));
    };
    if agent.state == AgentLifecycleState::Running || agent.state == AgentLifecycleState::Queued {
        return Err("agent is already running; use MessageDelegate for live follow-up".to_owned());
    }
    Ok(agent)
}
```

Use the current `MultiAgentRuntime` field name `state`; do not introduce a new async lock wrapper.

- [ ] **Step 4: Add runtime resume starter**

Add this method to `impl MultiAgentRuntime`:

```rust
pub fn start_resume_delegate(
    &self,
    agent_id: &str,
    request: &DelegateRequest,
) -> Result<AgentSnapshot, String> {
    let mut state = self.state.lock().expect("multi-agent state poisoned");
    let Some(agent) = state.agents.get_mut(agent_id) else {
        return Err(format!("unknown delegate target `{agent_id}`"));
    };
    if matches!(agent.state, AgentLifecycleState::Queued | AgentLifecycleState::Running) {
        return Err("agent is already running; use MessageDelegate for live follow-up".to_owned());
    }
    agent.state = AgentLifecycleState::Running;
    agent.mode = request.mode;
    agent.task = request.task.clone();
    agent.elapsed = Duration::from_secs(0);
    agent.latest_text = None;
    agent.activity.clear();
    agent.outcome = None;
    Ok(agent.clone())
}
```

- [ ] **Step 5: Route `DelegateTool` through resume when requested**

In `DelegateTool::execute`, after validation and before creating a new agent:

```rust
let mut snapshot = if let Some(agent_id) = request.resume.as_deref() {
    match ctx.multi_agent.start_resume_delegate(agent_id, &request) {
        Ok(snapshot) => snapshot,
        Err(message) => return Ok(ToolResult::error(message)),
    }
} else {
    ctx.multi_agent.start_delegate(
        &request.task,
        request.actual_role(),
        request.mode,
        AgentPathKind::Root,
    )
};
```

Use `snapshot.role` for `actual_role` in result details. For foreground and background paths, keep `snapshot.id.as_str()` as the task key.

- [ ] **Step 6: Run resume behavior test**

Run:

```bash
```

Expected: PASS.

## Task 5: Make `MessageDelegate` Live-Only For Agent Targets

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Add failing completed-agent message test**

Append this test to `crates/neo-agent-core/tests/multi_agent_background.rs`:

```rust
#[tokio::test]
async fn message_delegate_rejects_completed_agent_with_resume_hint() {
    let (registry, ctx) = registry_with_multi_agent();
    let delegate = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "finish quickly",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");
    let agent_id = delegate
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("delegate result should include agent_id")
        .to_owned();

    let message = registry
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({
                "id": agent_id,
                "message": "please do more"
            }),
        )
        .await
        .expect("MessageDelegate should return a tool result");

    assert!(message.is_error);
    assert!(
        message.content.contains("agent is not running; use Delegate with resume"),
        "{}",
        message.content
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL because `MessageDelegate` currently queues mail for completed or idle agents.

- [ ] **Step 3: Add runtime helper for live-only delivery**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, add:

```rust
pub fn deliver_live_agent_message(
    &self,
    agent_id: &str,
    message: String,
) -> Result<(), String> {
    let Some(agent) = self.agent_snapshot(agent_id) else {
        return Err(format!("unknown delegate target `{agent_id}`"));
    };
    if !matches!(agent.state, AgentLifecycleState::Running) {
        return Err(format!(
            "agent is not running; use Delegate with resume=\"{}\" to continue it",
            agent.id.as_str()
        ));
    }
    let mailbox_message = super::DelegateMailboxMessage {
        id: format!("live_{}", uuid::Uuid::new_v4().simple()),
        text: message,
        delivered: false,
    };
    if self.deliver_live_message(agent_id, &mailbox_message) {
        Ok(())
    } else {
        Err(format!(
            "agent is not running; use Delegate with resume=\"{}\" to continue it",
            agent.id.as_str()
        ))
    }
}
```

Leave existing mailbox storage functions in place only if other live-running code still needs them internally. Do not call `push_mailbox_message` from `MessageDelegate`.

- [ ] **Step 4: Change `MessageDelegateTool` agent branch**

In `crates/neo-agent-core/src/tools/delegate_controls.rs`, replace the current agent message path with:

```rust
match ctx
    .multi_agent
    .deliver_live_agent_message(&input.id, input.message.clone())
{
    Ok(()) => Ok(ToolResult::ok(format!(
        "target: {}\nstatus: delivered\nmessage: {}",
        input.id, input.message
    ))
    .with_details(serde_json::json!({
        "target": input.id,
        "status": "delivered",
        "delivered": [input.id],
        "message": input.message,
    }))),
    Err(message) => Ok(ToolResult::error(message)),
}
```

P2 will extend this branch to support `swarm_id`. In P1, `swarm_` targets may keep the current unknown-target error.

- [ ] **Step 5: Run message tests**

Run:

```bash
```

Expected: PASS, with the completed-agent test passing and the existing running-agent delivery test still passing.

## Task 6: Update Tool Schema Descriptions For Agent Controls

**Files:**

- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add schema-description assertion**

Append this test to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
#[test]
fn delegate_and_message_descriptions_explain_resume_and_live_followup() {
    let registry = ToolRegistry::with_builtin_tools_and_todos(TodoStore::default());
    let specs = registry.specs();
    let delegate = specs
        .iter()
        .find(|spec| spec.name == "Delegate")
        .expect("Delegate spec registered");
    let message = specs
        .iter()
        .find(|spec| spec.name == "MessageDelegate")
        .expect("MessageDelegate spec registered");

    assert!(delegate.description.contains("resume"), "{}", delegate.description);
    assert!(
        delegate.description.contains("role must be omitted"),
        "{}",
        delegate.description
    );
    assert!(
        message.description.contains("running"),
        "{}",
        message.description
    );
    assert!(
        message.description.contains("Delegate with resume"),
        "{}",
        message.description
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL until descriptions are expanded.

- [ ] **Step 3: Update `DelegateTool::description`**

Use this exact content:

```rust
fn description(&self) -> &'static str {
    "Delegate work to a subagent. Default mode is foreground, so the main agent waits for the result. \
     To continue an existing completed/failed/cancelled/timed_out agent, pass resume=\"agent_...\" and a new task. \
     When resume is set, role must be omitted because the resumed agent keeps its original role/profile/name/history. \
     Use mode=\"background\" only when the main agent should continue in parallel."
}
```

- [ ] **Step 4: Update `MessageDelegateTool::description`**

Use this exact content:

```rust
fn description(&self) -> &'static str {
    "Send a live follow-up message to a currently running delegate. \
     MessageDelegate does not queue offline messages for idle or terminal agents. \
     If the target is completed, failed, cancelled, timed_out, or idle, call Delegate with resume=\"agent_...\" instead."
}
```

- [ ] **Step 5: Run schema-description test**

Run:

```bash
```

Expected: PASS.

## Task 7: Focused Verification And Commit Boundary

**Files:**

- Verify all files changed by this plan.

- [ ] **Step 1: Run P1 focused test group**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 2: Run background controls focused group**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 3: Inspect diff for accidental compatibility aliases**

Run:

```bash
rg -n "harness|Stopped|stopped|mailbox_pending|queued messages|TASK_PLACEHOLDER|\\{task\\}|%s" crates/neo-agent-core/src crates/neo-agent-core/tests
```

Expected:

- No `harness`.
- No `BackgroundTaskStatus::Stopped`.
- No `status: stopped` assertions.
- No new mailbox wording that tells the model messages will be delivered to terminal agents.
- Template aliases may still appear only in old tests scheduled for P2 removal; if found in changed tests, delete them now.

- [ ] **Step 4: Commit if authorized**

Only if the user has explicitly authorized git mutation in this session:

```bash
git add crates/neo-agent-core/src/multi_agent/state.rs \
  crates/neo-agent-core/src/multi_agent/runtime.rs \
  crates/neo-agent-core/src/tools/delegate.rs \
  crates/neo-agent-core/src/tools/delegate_controls.rs \
  crates/neo-agent-core/src/tools/background_tasks.rs \
  crates/neo-agent-core/tests/multi_agent_runtime.rs \
  crates/neo-agent-core/tests/multi_agent_background.rs
git commit -m "fix: harden multi-agent lifecycle and resume"
```

Expected: one logical commit for P1.
