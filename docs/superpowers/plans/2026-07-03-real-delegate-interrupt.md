# Real Delegate Interrupt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `InterruptDelegate` and `TaskStop` genuinely stop a running Delegate or DelegateSwarm child run instead of only marking snapshots/background records as cancelled.

**Architecture:** Register each live child run in `MultiAgentRuntime` with a per-agent `CancellationToken` linked to the parent turn token. `cancel_agent`, `cancel_agent_by_id`, and swarm cancellation APIs cancel those live tokens before marking snapshots terminal. Child event application ignores late stream events after terminal cancellation, and transcript/store updates avoid stale terminal regressions.

**Tech Stack:** Rust 2024, `tokio_util::sync::CancellationToken`, existing `AgentRuntime::run_turn_with_cancel`, `FakeHarness`/delayed model tests, narrow `cargo test --package ... -- <full path> --exact`.

---

## Findings From Code Reading

- `crates/neo-agent-core/src/tools/delegate_controls.rs:722` implements `InterruptDelegateTool::execute`. For an agent ID it calls `ctx.multi_agent.cancel_agent(&agent_id)` and `ctx.background_tasks.cancel_delegate(...)`.
- `crates/neo-agent-core/src/multi_agent/runtime.rs:494` implements `MultiAgentRuntime::cancel_agent`. It only mutates `AgentSnapshot.state`, `terminal_at_ms`, `updated_at_ms`, and `terminal_reason`. It does not cancel a token, abort a task, close a model stream, or stop a spawned worker.
- `crates/neo-agent-core/src/tools/background_tasks.rs:355` implements `cancel_delegate`. It only changes the background task record from `DelegateRunning` to `DelegateFinished { status: Cancelled }`.
- `crates/neo-agent-core/src/tools/delegate.rs:108` and `crates/neo-agent-core/src/tools/delegate.rs:295` create fresh background-mode `CancellationToken`s and pass them into spawned child workers, but those tokens are not stored anywhere reachable by `InterruptDelegate`.
- `crates/neo-agent-core/src/multi_agent/runtime.rs:1546` creates another local child token inside `run_agent_snapshot` and bridges it to `deps.cancel_token`. The stream can stop correctly when that token is cancelled, but `InterruptDelegate` has no reference to it.
- `crates/neo-agent-core/src/runtime/stream_aggregator.rs:238` proves real model stream cancellation is already supported: `next_model_event` selects between `stream.next()` and `cancel_token.cancelled()`.
- `crates/neo-agent-core/src/multi_agent/runtime.rs:1412` prevents `finish_child_run` from overwriting a snapshot already marked `Cancelled`, but `apply_child_event` still accepts late `ThinkingDelta`/`TextDelta` updates after cancellation.
- `crates/neo-agent-core/src/tools/delegate.rs:383` drives swarms from `stream::iter(initial_snapshot.children.clone()).map(...).buffer_unordered(max_concurrency)`. That internal iterator is not connected to `cancel_swarm`; after interrupt it can continue polling queued children.
- `crates/neo-agent-core/src/multi_agent/runtime.rs:225` implements `mark_delegate_running` without a terminal guard. If `cancel_swarm` marks a queued child `Cancelled` before that child is polled, a later `run_started_swarm_child_turn` can mark the same child `Running` again and start its model stream.
- `crates/neo-agent-core/src/tools/delegate.rs:492` builds the final swarm snapshot from the worker's `ordered_children`, not from the runtime's post-interrupt swarm snapshot. If queued/running child futures keep producing results after interruption, the final snapshot can regress cancelled children.
- `crates/neo-tui/src/transcript/store.rs:342` updates delegate cards with any later snapshot for the same agent ID, without terminal precedence or timestamp checks. A stale `Completed` snapshot can therefore visually overwrite a prior cancelled snapshot if it is emitted later.

Conclusion: today `InterruptDelegate` is not a true execution interrupt for either single Delegate or DelegateSwarm. Single-agent interrupt marks state but leaves the child stream running. Swarm interrupt also marks state, but running children keep streaming and queued children can later be started unless the scheduler is taught to observe cancellation.

## File Structure

- Modify `crates/neo-agent-core/src/multi_agent/runtime.rs`: add live cancellation registration, cancel tokens on interrupt, unregister on child-run completion, prevent terminal children from being marked running, ignore late child events after terminal cancellation.
- Modify `crates/neo-agent-core/src/tools/delegate.rs`: keep background workers independent from the parent turn where intended, but ensure every active child token is registered through `MultiAgentRuntime`; make swarm scheduling observe runtime cancellation before starting queued children.
- Modify `crates/neo-agent-core/src/tools/background_tasks.rs`: add state-based delegate/swarm finish helpers so a cancelled runtime snapshot cannot be recorded as a completed background task.
- Modify `crates/neo-tui/src/transcript/store.rs`: prevent stale terminal snapshots from regressing a delegate card from `Cancelled` to `Completed`.
- Modify `crates/neo-agent-core/tests/multi_agent_runtime.rs`: add direct runtime tests proving `cancel_agent` cancels an active child stream.
- Modify `crates/neo-agent-core/tests/multi_agent_background.rs`: add tool-level `InterruptDelegate` test proving the running background delegate stops and remains cancelled.
- Modify `crates/neo-tui/tests/multi_agent_transcript.rs` or the nearest existing transcript test target: add stale terminal snapshot regression coverage.

## Task 1: Register Live Agent Cancellation Tokens

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add live token storage to `MultiAgentState`**

Add a field next to `steer_handles`:

```rust
agent_cancel_tokens: BTreeMap<String, CancellationToken>,
```

- [ ] **Step 2: Add a live cancellation guard**

Add this near `LiveSteerRegistration`:

```rust
struct LiveCancelRegistration {
    runtime: MultiAgentRuntime,
    agent_id: String,
    token: CancellationToken,
}

impl LiveCancelRegistration {
    fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for LiveCancelRegistration {
    fn drop(&mut self) {
        self.runtime
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .agent_cancel_tokens
            .remove(&self.agent_id);
    }
}
```

- [ ] **Step 3: Add central registration**

Add this method to `impl MultiAgentRuntime`:

```rust
fn register_live_cancel(
    &self,
    agent_id: &str,
    parent_token: &CancellationToken,
) -> LiveCancelRegistration {
    let token = CancellationToken::new();
    if parent_token.is_cancelled() {
        token.cancel();
    }
    let bridge_child = token.clone();
    let bridge_parent = parent_token.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = bridge_parent.cancelled() => bridge_child.cancel(),
            () = bridge_child.cancelled() => {}
        }
    });
    self.state
        .lock()
        .expect("multi-agent state poisoned")
        .agent_cancel_tokens
        .insert(agent_id.to_owned(), token.clone());
    LiveCancelRegistration {
        runtime: self.clone(),
        agent_id: agent_id.to_owned(),
        token,
    }
}
```

- [ ] **Step 4: Use the live token for started child runs**

In `run_started_child_turn` and `run_started_swarm_child_turn`, register before `run_agent_snapshot`:

```rust
let live_cancel = self.register_live_cancel(agent_id.as_str(), &deps.cancel_token);
let deps = deps.with_cancel_token(live_cancel.token());
let run = run_agent_snapshot(
    deps,
    prompt,
    prior_messages,
    live_steer.handle(),
    agent_id.as_str().to_owned(),
    child_wire_path,
    |event| {
        if let Some(updated) = runtime.apply_child_event(&agent_id, started_at, event) {
            on_update(updated);
        }
    },
)
.await;
drop(live_cancel);
```

- [ ] **Step 5: Keep older helper paths covered**

For `run_child_turn` and `run_swarm_child_turn`, either route through the started-run path or add the same `register_live_cancel` block after creating the snapshot. Do not keep a separate uncancellable path.

## Task 2: Make Runtime Cancellation APIs Cancel Live Tokens

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Cancel token in `cancel_agent`**

Change `cancel_agent` to clone the token while holding the lock, drop the lock, then cancel:

```rust
pub fn cancel_agent(&self, id: &AgentId) -> Option<AgentSnapshot> {
    let (snapshot, token) = {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let token = state.agent_cancel_tokens.get(id.as_str()).cloned();
        let snapshot = state.agents.get_mut(id.as_str())?;
        if snapshot.state.is_terminal() {
            return None;
        }
        let now = now_ms();
        snapshot.state = AgentLifecycleState::Cancelled;
        snapshot.terminal_at_ms.get_or_insert(now);
        snapshot.updated_at_ms = now;
        snapshot.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
        snapshot.outcome = Some(AgentTerminalOutcome {
            summary: "Cancelled by user.".to_owned(),
            is_error: true,
        });
        (snapshot.clone(), token)
    };
    if let Some(token) = token {
        token.cancel();
    }
    Some(snapshot)
}
```

- [ ] **Step 2: Share implementation with `cancel_agent_by_id`**

Either delegate through `AgentId::from_existing(id)` or extract a private `cancel_agent_locked_id(&self, id: &str)`. Keep one implementation so `TaskStop` and `InterruptDelegate` cannot drift.

- [ ] **Step 3: Cancel tokens in swarm cancellation**

In `cancel_swarm` and `cancel_swarm_by_id`, collect `agent_cancel_tokens` for every non-terminal child and cancel them after releasing the state lock:

```rust
let tokens = cancelled_ids
    .iter()
    .filter_map(|agent_id| state.agent_cancel_tokens.get(agent_id).cloned())
    .collect::<Vec<_>>();
drop(state);
for token in tokens {
    token.cancel();
}
```

Use a block expression rather than explicit `drop(state)` if borrow lifetimes get awkward.

## Task 3: Stop Late Child Events From Extending Cancelled Cards

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Ignore post-terminal child stream events**

At the start of `apply_child_event`, after fetching the snapshot:

```rust
if snapshot.state.is_terminal() {
    return None;
}
```

This prevents late buffered `ThinkingDelta` or `TextDelta` events from making a cancelled child look active or from extending its transcript body.

- [ ] **Step 2: Preserve the final cancelled summary**

Keep the existing `finish_child_run` guard that returns the current cancelled snapshot. Ensure the cancelled snapshot has an `outcome` from Task 2 so `render_child_final` can show a short cancelled final row if desired.

## Task 4: Keep Background Detachment But Make It Interruptible

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Keep explicit background root tokens**

Keep these two patterns unless product semantics intentionally change:

```rust
deps = deps.with_cancel_token(CancellationToken::new());
```

in background `Delegate` and background `DelegateSwarm`. They detach background workers from the current parent turn. True user interrupt comes from the runtime's per-agent live token registered in Task 1, not from reusing the parent turn token.

- [ ] **Step 2: Assert the token is reachable through runtime registration**

Do not pass raw child tokens from tools to controls. The only path should be:

```rust
Delegate tool -> run_started_child_turn -> register_live_cancel -> MultiAgentRuntime::cancel_agent/cancel_swarm
```

This keeps one cancellation ownership model and avoids a second map in `BackgroundTaskManager`.

- [ ] **Step 3: Add state-based background finish helpers**

In `BackgroundTaskManager`, add helpers that derive `BackgroundTaskStatus` from the snapshot state:

```rust
pub async fn finish_delegate(
    &self,
    task_id: &str,
    snapshot: crate::multi_agent::AgentSnapshot,
) {
    let status = status_from_agent_state(snapshot.state);
    let mut tasks = self.inner.lock().await;
    if let Some(record) = tasks.get_mut(task_id)
        && matches!(record.state, BackgroundTaskState::DelegateRunning { .. })
    {
        record.state = BackgroundTaskState::DelegateFinished { status, snapshot };
    }
}

pub async fn finish_delegate_swarm(
    &self,
    task_id: &str,
    snapshot: crate::multi_agent::SwarmSnapshot,
) {
    let status = status_from_agent_state(snapshot.state);
    let mut tasks = self.inner.lock().await;
    if let Some(record) = tasks.get_mut(task_id)
        && matches!(record.state, BackgroundTaskState::DelegateSwarmRunning { .. })
    {
        record.state = BackgroundTaskState::DelegateSwarmFinished { status, snapshot };
    }
}
```

Keep `complete_*` and `cancel_*` only if existing callers still need explicit names; new worker completion code should use the state-based helpers.

- [ ] **Step 4: Use `finish_delegate` for background Delegate workers**

Replace:

```rust
background_tasks
    .complete_delegate(&task_id_for_worker, output.snapshot)
    .await;
```

with:

```rust
background_tasks
    .finish_delegate(&task_id_for_worker, output.snapshot)
    .await;
```

This prevents a cancelled child run from being recorded as `BackgroundTaskStatus::Completed` if cancellation happened through runtime state before the background record was finalized.

## Task 4.5: Make Swarm Scheduling Cancellation-Aware

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Stop reviving terminal children**

Change `mark_delegate_running` so terminal snapshots are returned unchanged and never mutated back to `Running`:

```rust
#[must_use]
pub fn mark_delegate_running(&self, id: &AgentId) -> Option<AgentSnapshot> {
    let mut state = self.state.lock().expect("multi-agent state poisoned");
    let snapshot = state.agents.get_mut(id.as_str())?;
    if snapshot.state.is_terminal() {
        return Some(snapshot.clone());
    }
    let now = now_ms();
    snapshot.state = AgentLifecycleState::Running;
    snapshot.started_at_ms.get_or_insert(now);
    snapshot.terminal_at_ms = None;
    snapshot.terminal_reason = None;
    snapshot.updated_at_ms = now;
    Some(snapshot.clone())
}
```

- [ ] **Step 2: Short-circuit started child runs that are already terminal**

In `run_started_child_turn` and `run_started_swarm_child_turn`, immediately return if `mark_delegate_running` returns a terminal snapshot:

```rust
let snapshot = self.mark_delegate_running(&snapshot.id).unwrap_or(snapshot);
on_update(snapshot.clone());
if snapshot.state.is_terminal() {
    return ChildRunOutput {
        snapshot,
        events: Vec::new(),
        messages: Vec::new(),
    };
}
```

This is the guard that prevents queued swarm children from starting after `cancel_swarm`.

- [ ] **Step 3: Check swarm cancellation before polling each child**

At the top of the `async move` block inside `run_swarm_children`, before calling `run_started_swarm_child_turn`, check the runtime's latest child snapshot:

```rust
if let Some(current) = runtime.agent_snapshot(child.agent.id.as_str())
    && current.state.is_terminal()
{
    return SwarmChildSnapshot {
        item_index: child.item_index,
        item: child.item,
        agent: current,
    };
}
```

This makes queued children cheap to skip after interrupt.

- [ ] **Step 4: Preserve runtime-cancelled swarm as final**

Before returning from `run_swarm_children`, prefer the runtime snapshot if it is already terminal cancelled:

```rust
if let Some(current) = runtime.swarm_snapshot(&initial_snapshot.swarm_id)
    && current.state == AgentLifecycleState::Cancelled
{
    return current;
}
```

Then fall back to `swarm_snapshot_from_progress(...)`.

- [ ] **Step 5: Emit/categorize final background state by actual swarm state**

In the background swarm worker, replace unconditional `complete_delegate_swarm` with state-based finalization:

```rust
background_tasks
    .finish_delegate_swarm(&task_id_for_worker, final_snapshot)
    .await;
```

## Task 5: Prevent Transcript Terminal Regression

**Files:**
- Modify: `crates/neo-tui/src/transcript/store.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add delegate snapshot merge**

Add a helper similar to swarm child terminal merging:

```rust
fn merge_delegate_snapshot(current: &AgentSnapshot, incoming: AgentSnapshot) -> AgentSnapshot {
    if current.state.is_terminal()
        && incoming.state.is_terminal()
        && incoming.updated_at_ms < current.updated_at_ms
    {
        return current.clone();
    }
    if current.state == AgentLifecycleState::Cancelled
        && incoming.state == AgentLifecycleState::Completed
        && incoming.updated_at_ms <= current.updated_at_ms
    {
        return current.clone();
    }
    incoming
}
```

- [ ] **Step 2: Use the merge in `upsert_delegate`**

Replace direct update:

```rust
entry.update(snapshot);
```

with:

```rust
let merged = merge_delegate_snapshot(entry.snapshot(), snapshot);
entry.update(merged);
```

- [ ] **Step 3: Add the same merge inside delegate groups**

If `DelegateGroupComponent::upsert` directly overwrites snapshots, add equivalent merge there so grouped root delegates obey the same terminal precedence.

## Task 6: Add Runtime Reproduction Tests

**Files:**
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add a model that blocks until cancelled**

Reuse `DelayedTurnModel` if it observes stream drop/cancel sufficiently. Otherwise add a small test model whose stream sends `MessageStart`, waits on `Notify`, and records whether the stream was dropped.

- [ ] **Step 2: Write the failing test**

Add:

```rust
#[tokio::test]
async fn cancel_agent_stops_active_child_stream() {
    use neo_agent_core::multi_agent::{ChildRuntimeDeps, DelegateContext};

    let runtime = MultiAgentRuntime::new();
    let model = Arc::new(DelayedTurnModel::new(vec![vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            id: "child".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::ThinkingStart {
            id: "thinking".to_owned(),
        }),
        DelayedStep::Delay(std::time::Duration::from_secs(30)),
        DelayedStep::Event(AiStreamEvent::ThinkingDelta {
            text: "should not arrive".to_owned(),
        }),
    ]]));
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(neo_agent_core::harness::fake_model()),
        model,
        Arc::new(ToolRegistry::new()),
    );
    let snapshot = runtime.start_delegate(
        "slow child",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::Root,
    );
    let agent_id = snapshot.id.clone();
    let run = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run_started_child_turn(deps, snapshot, DelegateContext::None, |_| {})
                .await
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let cancelled = runtime.cancel_agent(&agent_id).expect("agent cancels");
    assert_eq!(cancelled.state, AgentLifecycleState::Cancelled);

    let output = tokio::time::timeout(std::time::Duration::from_secs(2), run)
        .await
        .expect("child run should stop after interrupt")
        .expect("join should succeed");
    assert_eq!(output.snapshot.state, AgentLifecycleState::Cancelled);
    assert!(
        !output
            .snapshot
            .activity
            .iter()
            .any(|entry| matches!(&entry.kind, AgentActivityKind::Text { text, .. } if text.contains("should not arrive")))
    );
}
```

- [ ] **Step 3: Run the failing test before implementation**

Run:

```bash
cargo test --package neo-agent-core --test multi_agent_runtime -- cancel_agent_stops_active_child_stream --exact --nocapture --include-ignored
```

Expected before implementation: timeout or activity contains late output.

- [ ] **Step 4: Run after implementation**

Run the same command. Expected after implementation: PASS.

## Task 7: Add Tool-Level Interrupt Coverage

**Files:**
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Write a background Delegate interrupt test**

Use the built-in `Delegate` tool to start a background agent with a delayed child model, then call `InterruptDelegate`, then `WaitDelegate`.

Core assertions:

```rust
assert!(interrupt.content.contains("status: cancelled"));
assert_eq!(interrupt.details.as_ref().unwrap()["outcome"], "cancelled");
assert!(waited.content.contains("status: cancelled"));
assert!(!waited.content.contains("should not arrive"));
```

- [ ] **Step 2: Run exactly this test**

Run:

```bash
cargo test --package neo-agent-core --test multi_agent_background -- interrupt_delegate_stops_background_child_stream --exact --nocapture --include-ignored
```

Expected after implementation: PASS.

## Task 8: Add Swarm Cancellation Coverage

**Files:**
- Modify: `crates/neo-agent-core/tests/multi_agent_background.rs`

- [ ] **Step 1: Write a swarm interrupt test**

Start a background `DelegateSwarm` with two delayed children and `max_concurrency: 2`. Call `InterruptDelegate` with the `swarm_id`.

Core assertions:

```rust
assert!(interrupt.content.contains("status: cancelled"));
assert!(interrupt.content.contains("cancelled=2"));
assert!(waited.content.contains("status: cancelled"));
```

- [ ] **Step 2: Run exactly this test**

Run:

```bash
cargo test --package neo-agent-core --test multi_agent_background -- interrupt_delegate_stops_running_swarm_children --exact --nocapture --include-ignored
```

Expected after implementation: PASS.

## Task 9: Add Transcript Regression Test

**Files:**
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Write a stale terminal update test**

Create a delegate card with a cancelled snapshot, then apply an older/equal timestamp completed snapshot for the same agent.

Core assertion:

```rust
let lines = plain_rendered_transcript_lines(...);
assert!(lines.iter().any(|line| line.contains("cancelled")));
assert!(!lines.iter().any(|line| line.contains(" · done · ")));
```

- [ ] **Step 2: Run exactly this test**

Run:

```bash
cargo test --package neo-tui --test multi_agent_transcript -- delegate_card_does_not_regress_cancelled_to_done --exact --nocapture --include-ignored
```

Expected after implementation: PASS.

## Task 10: Final Verification

- [ ] **Step 1: Run the three exact behavior tests**

```bash
cargo test --package neo-agent-core --test multi_agent_runtime -- cancel_agent_stops_active_child_stream --exact --nocapture --include-ignored
cargo test --package neo-agent-core --test multi_agent_background -- interrupt_delegate_stops_background_child_stream --exact --nocapture --include-ignored
cargo test --package neo-agent-core --test multi_agent_background -- interrupt_delegate_stops_running_swarm_children --exact --nocapture --include-ignored
cargo test --package neo-tui --test multi_agent_transcript -- delegate_card_does_not_regress_cancelled_to_done --exact --nocapture --include-ignored
```

- [ ] **Step 2: Run narrow existing cancellation guard**

```bash
cargo test --package neo-agent-core --test multi_agent_runtime -- foreground_delegate_cancel_marks_child_cancelled_when_tool_future_is_dropped --exact --nocapture --include-ignored
```

- [ ] **Step 3: Commit only after tests pass and only if explicitly authorized by the user**

Use a conventional message:

```bash
git add crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/tests/multi_agent_runtime.rs crates/neo-agent-core/tests/multi_agent_background.rs crates/neo-tui/src/transcript/store.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "fix: cancel live delegate runs"
```
