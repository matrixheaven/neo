# Neo Multi-Agent P3 Background Controls And Tasks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add explicit background `Delegate`/`DelegateSwarm`, `Ctrl+B` detach, list/wait/interrupt controls, mailbox completion, and `/tasks` integration.

**Architecture:** Extend the existing `BackgroundTaskManager` rather than building a second background agent UI. Background delegates and swarms become new task kinds with snapshots that the existing Task Browser adapter can map into rows/details/previews. Runtime control tools operate on the `MultiAgentRuntime` registry and return model-facing summaries without dumping full child transcripts into context.

**Tech Stack:** Rust 2024, `tokio`, `CancellationToken`, `BackgroundTaskManager`, `TaskBrowser`, `AgentEvent`, `ToolRegistry`, `cargo run -p xtask -- test`.

---

## Constraints

- Follow `/Users/chenyuanhao/Workspace/neo/AGENTS.md`.
- Start with `icm recall-context "Neo Multi-Agent P3 background controls tasks" --limit 5`.
- Use CodeGraph before grep/read.
- Do not add a new background agent page.
- Do not auto-inject large child transcripts into model context.
- Do not implement followup continuation in this plan; P4 owns `MessageDelegate`.

## Current Code Touchpoints

- `crates/neo-agent-core/src/tools/background_tasks.rs`
  - Currently supports `Bash` and `Question`.
- `crates/neo-agent/src/modes/task_browser.rs`
  - Maps `BackgroundTaskSnapshot` to `TaskBrowserItem`.
- `crates/neo-tui/src/tasks_browser/`
  - Already renders task rows, details, previews, and stop confirmation.
- `crates/neo-agent-core/src/multi_agent/`
  - P1/P2 runtime and snapshot types.
- `crates/neo-agent-core/src/tools/delegate.rs`
  - P1 tool surface.
- `crates/neo-agent/src/modes/interactive/input.rs`
  - Existing key handling for active turns; add `Ctrl+B` detach routing here or in the current keybinding owner.

## File Structure

Modify:

- `crates/neo-agent-core/src/tools/background_tasks.rs`
- `crates/neo-agent-core/src/multi_agent/state.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/tools/delegate.rs`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent/src/modes/task_browser.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`
- `crates/neo-agent/src/modes/interactive/turn.rs`
- `crates/neo-tui/src/tasks_browser/state.rs` only if new enum variants are required
- `crates/neo-tui/tests/task_browser.rs`

Create:

- `crates/neo-agent-core/src/tools/delegate_controls.rs`
- `crates/neo-agent-core/tests/multi_agent_background.rs`

## Desired End State

- `Delegate { mode: "background" }` returns immediately.
- `DelegateSwarm { mode: "background" }` returns immediately.
- Foreground running delegate/swarm can be detached with `Ctrl+B`.
- Detached runs continue as the same agent/swarm record.
- `BackgroundTaskKind` includes `Delegate` and `DelegateSwarm`.
- `/tasks` shows delegate and delegate-swarm rows/details/previews.
- `ListDelegates`, `WaitDelegate`, and `InterruptDelegate` are registered tools.
- Completion creates mailbox state but does not dump full transcript into context.

## Phase 1: Background Task Kinds

### Task 1.1: Extend background task snapshots

**Files:**
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`

- [ ] **Step 1: Add task kinds**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskKind {
    Bash,
    Question,
    Delegate,
    DelegateSwarm,
}

impl BackgroundTaskKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Question => "question",
            Self::Delegate => "delegate",
            Self::DelegateSwarm => "delegate-swarm",
        }
    }
}
```

- [ ] **Step 2: Add delegate preview fields**

Extend `BackgroundTaskSnapshot`:

```rust
pub delegate: Option<crate::multi_agent::AgentSnapshot>,
pub swarm: Option<crate::multi_agent::SwarmSnapshot>,
```

Update every existing snapshot construction for Bash/Question with:

```rust
delegate: None,
swarm: None,
```

- [ ] **Step 3: Compile existing tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core background_tasks
```

Expected: existing background task tests pass after snapshot constructors are updated.

### Task 1.2: Register delegate background records

**Files:**
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`

- [ ] **Step 1: Add state variants**

```rust
DelegateRunning { snapshot: crate::multi_agent::AgentSnapshot },
DelegateFinished {
    status: BackgroundTaskStatus,
    snapshot: crate::multi_agent::AgentSnapshot,
},
DelegateSwarmRunning { snapshot: crate::multi_agent::SwarmSnapshot },
DelegateSwarmFinished {
    status: BackgroundTaskStatus,
    snapshot: crate::multi_agent::SwarmSnapshot,
},
```

- [ ] **Step 2: Add registration methods**

```rust
pub async fn start_delegate(&self, snapshot: crate::multi_agent::AgentSnapshot) -> String {
    let task_id = snapshot.id.as_str().to_owned();
    self.inner.lock().await.insert(
        task_id.clone(),
        BackgroundTaskRecord {
            description: snapshot.task.clone(),
            started_at: Instant::now(),
            state: BackgroundTaskState::DelegateRunning { snapshot },
            detached: true,
            deadline: None,
            detach_timeout: None,
        },
    );
    task_id
}

pub async fn start_delegate_swarm(&self, snapshot: crate::multi_agent::SwarmSnapshot) -> String {
    let task_id = snapshot.swarm_id.clone();
    self.inner.lock().await.insert(
        task_id.clone(),
        BackgroundTaskRecord {
            description: snapshot.description.clone(),
            started_at: Instant::now(),
            state: BackgroundTaskState::DelegateSwarmRunning { snapshot },
            detached: true,
            deadline: None,
            detach_timeout: None,
        },
    );
    task_id
}
```

- [ ] **Step 3: Include delegate states in snapshots**

Update `list()`/snapshot construction so `DelegateRunning` maps to:

```rust
BackgroundTaskSnapshot {
    task_id,
    kind: BackgroundTaskKind::Delegate,
    status: BackgroundTaskStatus::Running,
    description,
    elapsed,
    output: None,
    answers: None,
    delegate: Some(snapshot),
    swarm: None,
}
```

And `DelegateSwarmRunning` maps to `BackgroundTaskKind::DelegateSwarm` with `swarm: Some(snapshot)`.

- [ ] **Step 4: Add manager test**

Create `crates/neo-agent-core/tests/multi_agent_background.rs`:

```rust
use neo_agent_core::multi_agent::MultiAgentRuntime;
use neo_agent_core::tools::{BackgroundTaskKind, BackgroundTaskManager};

#[tokio::test]
async fn background_manager_lists_delegate_tasks() {
    let runtime = MultiAgentRuntime::new();
    let agent = runtime.start_foreground_delegate_for_test("inspect task browser");
    let manager = BackgroundTaskManager::new();

    manager.start_delegate(agent.clone()).await;
    let snapshots = manager.snapshot_all().await;

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].kind, BackgroundTaskKind::Delegate);
    assert_eq!(snapshots[0].task_id, agent.id.as_str());
    assert!(snapshots[0].delegate.is_some());
}
```

Use `manager.list(false, 10).await` to retrieve snapshots.

- [ ] **Step 5: Run test**

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core background_manager_lists_delegate_tasks
```

Expected: PASS.

## Phase 2: Background Tool Semantics

### Task 2.1: Implement `mode=background`

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`

- [ ] **Step 1: Change `DelegateTool` background branch**

When `request.mode == AgentRunMode::Background`, start the agent in `MultiAgentRuntime`, register it in `ctx.background_tasks`, and return:

```text
agent_id: <agent id>
name: Gibbs
kind: delegate
status: running
task: <task text>
next_step: Use WaitDelegate to wait for completion.
next_step: Use /tasks to inspect progress.
```

Tool details must include:

```rust
json!({
    "kind": "delegate",
    "mode": "background",
    "agent": snapshot,
    "task_id": snapshot.id.as_str(),
})
```

- [ ] **Step 2: Change `DelegateSwarmTool` background branch**

Return:

```text
swarm_id: <swarm id>
kind: delegate-swarm
status: running
items: N
next_step: Use WaitDelegate to wait for completion.
next_step: Use /tasks to inspect progress.
```

Details must include `kind`, `mode`, `swarm`, and `task_id`.

- [ ] **Step 3: Run background tool test**

Add a test that executes `Delegate` with `mode=background` through `ToolRegistry::run`, then asserts the tool result details contain `"mode": "background"` and the shared background manager lists one delegate task.

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core delegate_background_registers_task
```

Expected: PASS.

## Phase 3: Control Tools

### Task 3.1: Add `ListDelegates`, `WaitDelegate`, and `InterruptDelegate`

**Files:**
- Create: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`

- [ ] **Step 1: Implement schemas and tools**

`ListDelegates` input:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListDelegatesInput {
    pub include_completed: Option<bool>,
}
```

`WaitDelegate` input:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitDelegateInput {
    pub id: String,
    pub timeout_ms: Option<u64>,
}
```

`InterruptDelegate` input:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InterruptDelegateInput {
    pub id: String,
}
```

Implement each tool with normal `Tool` trait. In P3:

- `ListDelegates` returns current registry/task snapshots.
- `WaitDelegate` waits for completion or timeout using runtime state polling at 100ms intervals.
- `InterruptDelegate` marks running delegate/swarm as cancelled and cancels its token if a token exists.

- [ ] **Step 2: Register tools**

Modify `ToolRegistry::with_builtin_tools_and_todos`:

```rust
registry.register(delegate_controls::ListDelegatesTool);
registry.register(delegate_controls::WaitDelegateTool);
registry.register(delegate_controls::InterruptDelegateTool);
```

- [ ] **Step 3: Add tests**

Tests:

```rust
#[tokio::test]
async fn list_delegates_reports_background_delegate() {
    let ctx = ToolContext::new(std::env::current_dir().unwrap()).unwrap();
    let agent = ctx
        .multi_agent()
        .start_foreground_delegate_for_test("inspect background registry");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run("ListDelegates", &ctx, serde_json::json!({ "include_completed": true }))
        .await
        .expect("list should succeed");

    assert!(result.content.contains(agent.id.as_str()));
    assert!(result.content.contains("inspect background registry"));
}

#[tokio::test]
async fn wait_delegate_times_out_without_completion() {
    let ctx = ToolContext::new(std::env::current_dir().unwrap()).unwrap();
    let agent = ctx
        .multi_agent()
        .start_foreground_delegate_for_test("long running task");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "id": agent.id.as_str(), "timeout_ms": 1 }),
        )
        .await
        .expect("wait should return timeout result");

    assert!(result.content.contains("timed_out"));
}

#[tokio::test]
async fn interrupt_delegate_marks_running_agent_cancelled() {
    let ctx = ToolContext::new(std::env::current_dir().unwrap()).unwrap();
    let agent = ctx
        .multi_agent()
        .start_foreground_delegate_for_test("cancel me");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "InterruptDelegate",
            &ctx,
            serde_json::json!({ "id": agent.id.as_str() }),
        )
        .await
        .expect("interrupt should succeed");

    assert!(result.content.contains("cancelled"));
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core delegate_controls
```

Expected: PASS.

## Phase 4: `Ctrl+B` Detach

### Task 4.1: Route `Ctrl+B` to active delegate detach

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Modify: `crates/neo-agent/src/modes/interactive/turn.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`

- [ ] **Step 1: Add runtime detach methods**

Add:

```rust
pub fn detach_agent(&self, id: &AgentId) -> Option<AgentSnapshot> {
    let mut state = self.state.lock().expect("multi-agent state poisoned");
    let snapshot = state.agents.get_mut(id.as_str())?;
    snapshot.mode = AgentRunMode::Background;
    Some(snapshot.clone())
}

pub fn detach_swarm(&self, swarm_id: &str) -> Option<SwarmSnapshot> {
    let mut state = self.state.lock().expect("multi-agent state poisoned");
    let snapshot = state.swarms.get_mut(swarm_id)?;
    snapshot.mode = AgentRunMode::Background;
    for child in &mut snapshot.children {
        child.agent.mode = AgentRunMode::Background;
    }
    Some(snapshot.clone())
}
```

- [ ] **Step 2: Add interactive routing**

When an active foreground delegate/swarm is running and `Ctrl+B` is received:

1. call the runtime detach method
2. register the detached snapshot in `BackgroundTaskManager`
3. release the foreground join
4. leave the transcript card visible
5. show status text: `Moved to background. Use /tasks to view.`

Do not detach unrelated Bash tasks through this path.

- [ ] **Step 3: Add detach test**

Use an interactive input test or runtime-level test if TUI key simulation is too broad:

```rust
#[tokio::test]
async fn ctrl_b_detach_preserves_agent_id_and_registers_background_task() {
    let runtime = MultiAgentRuntime::new();
    let manager = BackgroundTaskManager::new();
    let running = runtime.start_foreground_delegate_for_test("detach me");

    let detached = runtime.detach_agent(&running.id).expect("agent should detach");
    manager.start_delegate(detached.clone()).await;
    let tasks = manager.list(false, 10).await;

    assert_eq!(detached.id, running.id);
    assert_eq!(detached.mode, AgentRunMode::Background);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, running.id.as_str());
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent ctrl_b_detach
```

Expected: PASS if an interactive test exists; otherwise run the core detach test in `neo-agent-core`.

## Phase 5: `/tasks` Integration

### Task 5.1: Map delegate snapshots into Task Browser rows

**Files:**
- Modify: `crates/neo-agent/src/modes/task_browser.rs`
- Modify: `crates/neo-tui/src/tasks_browser/state.rs` if `TaskBrowserKind` needs new variants
- Test: `crates/neo-tui/tests/task_browser.rs`

- [ ] **Step 1: Add browser kinds**

Add:

```rust
Delegate,
DelegateSwarm,
```

to `TaskBrowserKind`, with labels `delegate` and `delegate-swarm`.

- [ ] **Step 2: Map snapshots**

In `snapshot_to_item`, map:

```rust
BackgroundTaskKind::Delegate => TaskBrowserKind::Delegate,
BackgroundTaskKind::DelegateSwarm => TaskBrowserKind::DelegateSwarm,
```

For delegate detail lines, include:

```text
id:
kind:
name:
profile:
mode:
status:
elapsed:
tokens:
tools:
parent:
task:
latest:
```

For swarm detail lines, include:

```text
id:
kind:
status:
elapsed:
progress:
children:
task:
```

- [ ] **Step 3: Add adapter tests**

Add tests that build delegate and swarm `BackgroundTaskSnapshot` values and assert:

```rust
assert_eq!(item.kind, TaskBrowserKind::Delegate);
assert!(item.detail_lines.iter().any(|line| line.contains("name:")));
assert!(item.preview_lines.iter().any(|line| line.contains("latest")));
```

And for swarm:

```rust
assert_eq!(item.kind, TaskBrowserKind::DelegateSwarm);
assert!(item.detail_lines.iter().any(|line| line.contains("children:")));
```

- [ ] **Step 4: Run task browser tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser_adapter_maps_delegate
cargo run -p xtask -- test -p neo-tui task_browser
```

Expected: PASS.

## Phase 6: Verification

- [ ] Run:

```bash
cargo run -p xtask -- test -p neo-agent-core multi_agent_background
```

Expected: PASS.

- [ ] Run:

```bash
cargo run -p xtask -- test -p neo-agent task_browser_adapter
```

Expected: PASS.

- [ ] Run:

```bash
cargo run -p xtask -- check
```

Expected: PASS unless unrelated dirty-worktree changes break the global check. Report unrelated breakage without reverting files.

## Handoff Notes For P4

- P4 owns `MessageDelegate`, swarm resume, adaptive scheduler, and mature Bayesian-style progress.
- Keep `/tasks` as the human inspection surface.
- Keep mailbox summaries small and explicit.
