# Neo Multi-Agent Tool Contract Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Neo's multi-agent tool schema and structured results sufficient for a parent model to use `Delegate`, `MessageDelegate`, `ListDelegates`, `WaitDelegate`, `DelegateSwarm`, and `TaskOutput` without reading source or docs.

**Architecture:** Keep lifecycle state in `multi_agent`, add focused result-format helpers under `tools`, and route every multi-agent tool result through those helpers. Replace ambiguous list/wait/swarm outputs with one canonical JSON details contract while keeping human-readable text compact.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `schemars`, `base64`, `cargo-nextest`, existing `FakeHarness` model tests.

---

## Ground Rules

- This plan implements `docs/superpowers/specs/2026-07-02-neo-multi-agent-tool-contract-cleanup-design.md`.
- Do not keep compatibility branches for old ambiguous output shapes.
- Do not use broad `cargo test` or package-wide nextest as evidence.
- Prefix shell commands with `rtk`.
- Git mutation policy for this repo is strict: `git add` and `git commit` steps below are checkpoints for execution sessions that have explicit user authorization for git mutation. If that authorization is absent, skip the commit step and report the exact files that would be staged.
- Subagents must not run git mutation commands unless the main agent explicitly passes fresh per-instance authorization in their prompt.

## File Structure

- Modify `crates/neo-agent-core/src/multi_agent/state.rs`
  - Add run metadata to `AgentSnapshot`.
- Modify `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - Initialize run metadata, increment it on resume, track live-message delivery, and expose lifecycle semantics for formatting.
- Create `crates/neo-agent-core/src/tools/multi_agent_format.rs`
  - Own canonical JSON details and compact text formatting for agent and swarm results.
- Modify `crates/neo-agent-core/src/tools/mod.rs`
  - Register the new internal formatting module.
- Modify `crates/neo-agent-core/src/tools/delegate.rs`
  - Use canonical formatter for `Delegate` and `DelegateSwarm`.
- Modify `crates/neo-agent-core/src/tools/delegate_controls.rs`
  - Add `ListDelegates.include`, safe cursors, improved terminal errors, canonical `WaitDelegate`, and canonical `MessageDelegate` details.
- Modify `crates/neo-agent-core/src/tools/background_tasks.rs`
  - Route delegate and swarm `TaskOutput` through canonical formatter.
- Modify `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - Add focused tool/runtime/schema tests for the new contract.
- Modify `crates/neo-agent-core/tests/tool_bash.rs`
  - Add focused `TaskOutput` delegate/swarm result tests if the behavior sits better with existing task tests.

## Task 1: Add Agent Run Metadata

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Write the failing runtime metadata test**

Add this test near `agent_snapshot_records_timestamps_detach_origin_and_terminal_reason` in `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
#[test]
fn agent_snapshot_records_run_metadata_and_resume_origin() {
    let runtime = MultiAgentRuntime::new();
    let first = runtime.start_foreground_delegate_for_test("inspect mvcc");

    assert_eq!(first.run_count, 1);
    assert_eq!(first.live_messages_received, 0);
    assert_eq!(first.previous_status, None);
    assert_eq!(first.resumed_from, None);

    let completed = runtime.complete_delegate_for_test(&first.id, "mvcc summary");
    assert_eq!(completed.state, AgentLifecycleState::Completed);

    let request = neo_agent_core::multi_agent::DelegateRequest {
        task: "continue with wraparound".to_owned(),
        resume: Some(first.id.as_str().to_owned()),
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: neo_agent_core::multi_agent::DelegateContext::Inherit,
    };
    let resumed = runtime
        .start_resume_delegate(first.id.as_str(), &request)
        .expect("completed agent can be resumed");

    assert_eq!(resumed.run_count, 2);
    assert_eq!(resumed.live_messages_received, 0);
    assert_eq!(resumed.previous_status, Some(AgentLifecycleState::Completed));
    assert_eq!(
        resumed.resumed_from.as_ref().map(neo_agent_core::multi_agent::AgentId::as_str),
        Some(first.id.as_str())
    );
    assert_eq!(resumed.state, AgentLifecycleState::Running);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime agent_snapshot_records_run_metadata_and_resume_origin
```

Expected: FAIL because `AgentSnapshot` has no `run_count`, `live_messages_received`, `previous_status`, or `resumed_from` fields.

- [ ] **Step 3: Add fields to `AgentSnapshot`**

In `crates/neo-agent-core/src/multi_agent/state.rs`, extend `AgentSnapshot` after `terminal_reason`:

```rust
    pub run_count: usize,
    pub live_messages_received: usize,
    pub previous_status: Option<AgentLifecycleState>,
    pub resumed_from: Option<AgentId>,
```

These fields intentionally serialize with the snapshot so tool details can use the same data source.

- [ ] **Step 4: Initialize the fields on new snapshots**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, update `new_agent_snapshot`:

```rust
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        resumed_from: None,
```

Place those assignments immediately after `terminal_reason`.

- [ ] **Step 5: Update resume state transition**

In `start_resume_delegate`, capture the old status and increment the run metadata before setting the new running state:

```rust
        let previous_status = agent.state;
        agent.state = AgentLifecycleState::Running;
        agent.mode = request.mode;
        agent.task = request.task.clone();
        agent.task_title = derive_title(&request.task, request.title.as_deref());
        agent.run_count = agent.run_count.saturating_add(1);
        agent.live_messages_received = 0;
        agent.previous_status = Some(previous_status);
        agent.resumed_from = Some(AgentId::from_existing(agent_id));
```

Add this constructor to `AgentId` in `crates/neo-agent-core/src/multi_agent/identity.rs`:

```rust
    #[must_use]
    pub fn from_existing(id: impl Into<String>) -> Self {
        Self(id.into())
    }
```

Keep the existing clearing of `elapsed`, `latest_text`, `activity`, and `outcome` after these assignments.

- [ ] **Step 6: Count delivered live messages**

In `deliver_live_agent_message`, increment the count only after `deliver_live_message` returns true:

```rust
        if self.deliver_live_message(agent_id, &mailbox_message) {
            self.record_live_message(agent_id);
            Ok(())
        } else {
            Err(format!(
                "agent is not running; use Delegate with resume=\"{}\" to continue it",
                agent.id.as_str()
            ))
        }
```

Add this private helper in the same `impl MultiAgentRuntime`:

```rust
    fn record_live_message(&self, agent_id: &str) {
        if let Some(agent) = self
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .agents
            .get_mut(agent_id)
        {
            agent.live_messages_received = agent.live_messages_received.saturating_add(1);
            agent.updated_at_ms = now_ms();
        }
    }
```

In `broadcast_live_swarm_message`, call `self.record_live_message(child.agent.id.as_str())` when a child ID is pushed into `delivered`.

- [ ] **Step 7: Run the metadata test**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime agent_snapshot_records_run_metadata_and_resume_origin
```

Expected: PASS.

- [ ] **Step 8: Commit checkpoint after explicit authorization**

If git mutation has been authorized for this execution instance, run:

```bash
rtk git add crates/neo-agent-core/src/multi_agent/state.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/multi_agent/identity.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
rtk git commit -m "feat: track delegate run metadata"
```

Expected: commit succeeds. Without authorization, do not run these commands.

## Task 2: Add Canonical Multi-Agent Result Formatter

**Files:**
- Create: `crates/neo-agent-core/src/tools/multi_agent_format.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Write the failing Delegate result test**

Add this test after `delegate_resume_reuses_agent_identity_and_role`:

```rust
#[tokio::test]
async fn delegate_result_details_include_canonical_run_fields() {
    let (registry, ctx) = registry_with_multi_agent();

    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "inspect result contract",
                "title": "Result contract",
                "context": "summary",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");

    let details = result.details.as_ref().expect("delegate details");
    assert_eq!(details["kind"], "delegate");
    assert_eq!(details["mode"], "foreground");
    assert_eq!(details["status"], "completed");
    assert_eq!(details["title"], "Result contract");
    assert_eq!(details["context_mode"], "summary");
    assert_eq!(details["summary_scope"], "current_run");
    assert_eq!(details["run_index"], 1);
    assert_eq!(details["run_count"], 1);
    assert!(details["created_at_ms"].as_u64().is_some(), "{details}");
    assert!(details["started_at_ms"].as_u64().is_some(), "{details}");
    assert!(details["terminal_at_ms"].as_u64().is_some(), "{details}");
    assert!(details["elapsed_ms"].as_u64().is_some(), "{details}");
    assert!(details["tool_count"].as_u64().is_some(), "{details}");
    assert!(details["token_count"].as_u64().is_some(), "{details}");
    assert!(details.get("agent").is_none(), "old nested agent field should be gone: {details}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_result_details_include_canonical_run_fields
```

Expected: FAIL because details are still nested under `agent` and lack canonical top-level fields.

- [ ] **Step 3: Create the formatter module**

Create `crates/neo-agent-core/src/tools/multi_agent_format.rs`:

```rust
use serde_json::{Value, json};

use crate::multi_agent::{
    AgentLifecycleState, AgentRunMode, AgentSnapshot, DelegateContext, SwarmSnapshot,
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum SummaryScope {
    CurrentRun,
    SwarmItems,
    None,
}

impl SummaryScope {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CurrentRun => "current_run",
            Self::SwarmItems => "swarm_items",
            Self::None => "none",
        }
    }
}

pub(crate) fn context_mode_label(context: DelegateContext) -> &'static str {
    match context {
        DelegateContext::Inherit => "inherit",
        DelegateContext::Summary => "summary",
        DelegateContext::None => "none",
    }
}

pub(crate) fn mode_label(mode: AgentRunMode) -> &'static str {
    match mode {
        AgentRunMode::Foreground => "foreground",
        AgentRunMode::Background => "background",
    }
}

pub(crate) fn agent_details(
    kind: &'static str,
    agent: &AgentSnapshot,
    context: Option<DelegateContext>,
    summary_scope: SummaryScope,
    include_task: bool,
    include_summary: bool,
    include_activity: bool,
) -> Value {
    let mut value = json!({
        "kind": kind,
        "id": agent.id.as_str(),
        "agent_id": agent.id.as_str(),
        "status": agent.state.as_str(),
        "mode": mode_label(agent.mode),
        "role": agent.role.as_str(),
        "actual_role": agent.role.as_str(),
        "display_name": agent.display_name.as_str(),
        "title": agent.task_title.as_str(),
        "created_at_ms": agent.created_at_ms,
        "updated_at_ms": agent.updated_at_ms,
        "started_at_ms": agent.started_at_ms,
        "terminal_at_ms": agent.terminal_at_ms,
        "elapsed_ms": u64::try_from(agent.elapsed.as_millis()).unwrap_or(u64::MAX),
        "tool_count": agent.tool_count,
        "token_count": agent.token_count,
        "run_index": agent.run_count,
        "run_count": agent.run_count,
        "live_messages_received": agent.live_messages_received,
        "previous_status": agent.previous_status.map(AgentLifecycleState::as_str),
        "resumed_from": agent.resumed_from.as_ref().map(crate::multi_agent::AgentId::as_str),
        "summary_scope": summary_scope.as_str(),
    });
    if let Some(context) = context {
        value["context_mode"] = json!(context_mode_label(context));
    }
    if include_task {
        value["task"] = json!(agent.task.as_str());
    }
    if include_summary {
        value["summary"] = json!(agent.outcome.as_ref().map(|outcome| outcome.summary.clone()).unwrap_or_default());
    }
    if include_activity {
        value["activity_tail"] = json!(agent.activity);
    }
    value
}

pub(crate) fn delegate_result_content(agent: &AgentSnapshot, context: DelegateContext) -> String {
    let mut content = format!(
        "agent_id: {}\nname: {}\nstatus: {}\nrun_index: {}\nsummary_scope: current_run\ncontext_mode: {}",
        agent.id.as_str(),
        agent.display_name.as_str(),
        agent.state.as_str(),
        agent.run_count,
        context_mode_label(context),
    );
    if let Some(previous) = agent.previous_status {
        content.push_str(&format!("\nprevious_status: {}", previous.as_str()));
    }
    if let Some(outcome) = &agent.outcome {
        content.push_str(&format!("\nsummary: {}", outcome.summary));
    }
    content
}

pub(crate) fn swarm_details(swarm: &SwarmSnapshot) -> Value {
    let items = swarm
        .children
        .iter()
        .map(|child| {
            let agent = &child.agent;
            json!({
                "index": child.item_index,
                "item": child.item.as_str(),
                "agent_id": agent.id.as_str(),
                "name": agent.display_name.as_str(),
                "status": agent.state.as_str(),
                "title": agent.task_title.as_str(),
                "elapsed_ms": u64::try_from(agent.elapsed.as_millis()).unwrap_or(u64::MAX),
                "tool_count": agent.tool_count,
                "token_count": agent.token_count,
                "summary": agent.outcome.as_ref().map(|outcome| outcome.summary.clone()),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "kind": "delegate_swarm",
        "id": swarm.swarm_id.as_str(),
        "swarm_id": swarm.swarm_id.as_str(),
        "status": swarm.state.as_str(),
        "mode": mode_label(swarm.mode),
        "role": swarm.role.as_str(),
        "description": swarm.description.as_str(),
        "summary_scope": SummaryScope::SwarmItems.as_str(),
        "aggregate": swarm.aggregate,
        "items": items,
        "resume_hint": "Call DelegateSwarm with resume_agent_ids for unfinished children.",
    })
}
```

- [ ] **Step 4: Register the formatter module**

In `crates/neo-agent-core/src/tools/mod.rs`, add:

```rust
mod multi_agent_format;
```

Place it next to `mod delegate_controls;`.

- [ ] **Step 5: Use the formatter for foreground Delegate**

In `crates/neo-agent-core/src/tools/delegate.rs`, import the helper:

```rust
use super::multi_agent_format::{SummaryScope, agent_details, delegate_result_content};
```

Replace the foreground `Delegate` block that currently builds `agent_id`, `name`, `status`, and nested `agent` details with:

```rust
            Ok(ToolResult::ok(delegate_result_content(&completed, request.context)).with_details(
                agent_details(
                    "delegate",
                    &completed,
                    Some(request.context),
                    SummaryScope::CurrentRun,
                    true,
                    true,
                    false,
                ),
            ))
```

- [ ] **Step 6: Use the formatter for background Delegate**

In the background branch, replace the details JSON with:

```rust
                let mut details = agent_details(
                    "delegate",
                    &snapshot,
                    Some(request.context),
                    SummaryScope::CurrentRun,
                    true,
                    false,
                    false,
                );
                details["mode"] = serde_json::json!("background");
                details["task_id"] = serde_json::json!(task_id);
                return Ok(ToolResult::ok(format!(
                    "agent_id: {}\nname: {}\nkind: delegate\nstatus: running\nrun_index: {}\ncontext_mode: {}\nnext_step: Use WaitDelegate to wait for completion.\nnext_step: Use ListDelegates to check status.",
                    snapshot.id.as_str(),
                    snapshot.display_name.as_str(),
                    snapshot.run_count,
                    super::multi_agent_format::context_mode_label(request.context),
                ))
                .with_details(details));
```

- [ ] **Step 7: Run the Delegate result test**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_result_details_include_canonical_run_fields
```

Expected: PASS.

- [ ] **Step 8: Commit checkpoint after explicit authorization**

If git mutation has been authorized for this execution instance, run:

```bash
rtk git add crates/neo-agent-core/src/tools/mod.rs crates/neo-agent-core/src/tools/multi_agent_format.rs crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
rtk git commit -m "feat: format canonical delegate results"
```

Expected: commit succeeds. Without authorization, do not run these commands.

## Task 3: Add Resume Result Metadata And Live Message Count

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Write the failing resume details test**

Extend `delegate_resume_reuses_agent_identity_and_role` with these assertions after the existing `actual_role` assertion:

```rust
    assert_eq!(details["run_index"], 2);
    assert_eq!(details["run_count"], 2);
    assert_eq!(details["resumed_from"], agent_id.as_str());
    assert_eq!(details["previous_status"], "completed");
    assert_eq!(details["summary_scope"], "current_run");
    assert!(
        second.content.contains("previous_status: completed"),
        "{}",
        second.content
    );
```

- [ ] **Step 2: Run the resume test to verify it fails**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_resume_reuses_agent_identity_and_role
```

Expected: FAIL if the previous task did not yet expose the new fields in details and text.

- [ ] **Step 3: Keep previous status visible in Delegate content**

If `delegate_result_content` from Task 2 does not already include `previous_status`, add:

```rust
    if let Some(previous) = agent.previous_status {
        content.push_str(&format!("\nprevious_status: {}", previous.as_str()));
    }
```

Place it before summary text.

- [ ] **Step 4: Write the failing terminal message wording test**

Add this test after `delegate_and_message_descriptions_explain_resume_and_live_followup`:

```rust
#[tokio::test]
async fn message_delegate_terminal_agent_error_explains_resume_without_immutable_confusion() {
    let (registry, ctx) = registry_with_multi_agent();
    let first = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "finish then reject live message",
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
        .expect("agent id")
        .to_owned();

    let result = registry
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({
                "id": agent_id,
                "message": "add one more note"
            }),
        )
        .await
        .expect("message tool should return an error result");

    assert!(result.is_error);
    assert!(result.content.contains("cannot receive live messages"), "{}", result.content);
    assert!(result.content.contains("Delegate with resume"), "{}", result.content);
    assert!(
        !result.content.contains("terminal delegate state is immutable"),
        "{}",
        result.content
    );
}
```

- [ ] **Step 5: Run the terminal message wording test to verify it fails**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime message_delegate_terminal_agent_error_explains_resume_without_immutable_confusion
```

Expected: FAIL because the current error text says only that the agent is not running or uses the older immutable wording.

- [ ] **Step 6: Update terminal error text**

In `crates/neo-agent-core/src/tools/delegate_controls.rs`, replace `terminal_delegate_error` with:

```rust
fn terminal_delegate_error(agent_id: &str, state: AgentLifecycleState) -> ToolResult {
    ToolResult::error(format!(
        "agent already {}; terminal agents cannot receive live messages or be interrupted. To continue this agent, call Delegate with resume=\"{}\".",
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

In `deliver_live_agent_message`, change the non-running error to the same wording for terminal agents:

```rust
        if !matches!(agent.state, AgentLifecycleState::Running) {
            return Err(format!(
                "agent already {}; terminal agents cannot receive live messages. To continue this agent, call Delegate with resume=\"{}\".",
                agent.state.as_str(),
                agent.id.as_str()
            ));
        }
```

- [ ] **Step 7: Run both focused tests**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_resume_reuses_agent_identity_and_role
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime message_delegate_terminal_agent_error_explains_resume_without_immutable_confusion
```

Expected: both PASS.

- [ ] **Step 8: Commit checkpoint after explicit authorization**

If git mutation has been authorized for this execution instance, run:

```bash
rtk git add crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/tools/delegate_controls.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
rtk git commit -m "fix: clarify delegate resume result contract"
```

Expected: commit succeeds. Without authorization, do not run these commands.

## Task 4: Make ListDelegates Meta-Only With Safe Cursors

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Write the failing default list output test**

Add this test after the existing `ListDelegates` tests:

```rust
#[tokio::test]
async fn list_delegates_defaults_to_meta_only_rows_with_title() {
    let (registry, ctx) = registry_with_multi_agent();
    let _ = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "long prompt body that should not appear in default list",
                "title": "Short title",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");

    let result = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "include_completed": true,
                "kind": "agent"
            }),
        )
        .await
        .expect("list should succeed");

    let details = result.details.as_ref().expect("list details");
    assert_eq!(details["include"], serde_json::json!(["meta"]));
    let row = details["delegates"][0].as_object().expect("first row");
    assert_eq!(row["title"], "Short title");
    assert!(row.get("task").is_none(), "{row:#?}");
    assert!(row.get("summary").is_none(), "{row:#?}");
    assert!(
        !result.content.contains("long prompt body"),
        "{}",
        result.content
    );
}
```

- [ ] **Step 2: Run the default list output test to verify it fails**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime list_delegates_defaults_to_meta_only_rows_with_title
```

Expected: FAIL because current list output includes full task text and lacks `include`.

- [ ] **Step 3: Add include enum and default**

In `delegate_controls.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DelegateListInclude {
    Meta,
    Task,
    Summary,
    Activity,
}

fn default_delegate_list_include() -> Vec<DelegateListInclude> {
    vec![DelegateListInclude::Meta]
}
```

Add this field to `ListDelegatesInput`:

```rust
    #[serde(default = "default_delegate_list_include")]
    #[schemars(
        description = "Fields to include in each row. Defaults to [\"meta\"]. Add task, summary, or activity only when needed."
    )]
    include: Vec<DelegateListInclude>,
```

- [ ] **Step 4: Add safe cursor types**

In `delegate_controls.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DelegateListCursor {
    offset: usize,
    query: DelegateListCursorQuery,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DelegateListCursorQuery {
    include_completed: bool,
    kind: String,
    state: Option<String>,
    order: String,
    include: Vec<String>,
}

impl DelegateListCursorQuery {
    fn from_input(input: &ListDelegatesInput, include_completed: bool) -> Self {
        Self {
            include_completed,
            kind: match input.kind {
                DelegateListKind::Agent => "agent",
                DelegateListKind::Swarm => "swarm",
                DelegateListKind::All => "all",
            }
            .to_owned(),
            state: input.state.map(|state| state.as_str().to_owned()),
            order: match input.order {
                DelegateListOrder::Newest => "newest",
                DelegateListOrder::Oldest => "oldest",
            }
            .to_owned(),
            include: input
                .include
                .iter()
                .map(|value| match value {
                    DelegateListInclude::Meta => "meta",
                    DelegateListInclude::Task => "task",
                    DelegateListInclude::Summary => "summary",
                    DelegateListInclude::Activity => "activity",
                }
                .to_owned())
                .collect(),
        }
    }
}
```

Use `base64::Engine` with `base64::engine::general_purpose::URL_SAFE_NO_PAD` to encode and decode `DelegateListCursor` as JSON.

- [ ] **Step 5: Replace numeric cursor parsing**

Replace `parse_list_cursor` with:

```rust
fn parse_list_cursor(
    tool: &str,
    cursor: Option<&str>,
    expected_query: &DelegateListCursorQuery,
) -> Result<usize, ToolError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "cursor must be a ListDelegates next_cursor value".to_owned(),
        })?;
    let decoded: DelegateListCursor =
        serde_json::from_slice(&bytes).map_err(|_| ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "cursor must be a ListDelegates next_cursor value".to_owned(),
        })?;
    if decoded.query != *expected_query {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "cursor was created for a different ListDelegates query; restart pagination without cursor".to_owned(),
        });
    }
    Ok(decoded.offset)
}
```

Add `use base64::Engine;` at the top of the file.

- [ ] **Step 6: Encode opaque next cursors**

Add this helper next to `parse_list_cursor`:

```rust
fn encode_list_cursor(
    tool: &str,
    offset: usize,
    query: &DelegateListCursorQuery,
) -> Result<String, ToolError> {
    let cursor = DelegateListCursor {
        offset,
        query: query.clone(),
    };
    let bytes = serde_json::to_vec(&cursor).map_err(|err| ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: format!("failed to encode ListDelegates cursor: {err}"),
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}
```

In `ListDelegatesTool::execute`, build the query before parsing the cursor:

```rust
let cursor_query = DelegateListCursorQuery::from_input(&input, include_completed);
let offset = parse_list_cursor(self.name(), input.cursor.as_deref(), &cursor_query)?;
```

Replace numeric `next_cursor` creation with:

```rust
let next_cursor = if page_end < total {
    Some(encode_list_cursor(self.name(), page_end, &cursor_query)?)
} else {
    None
};
```

Add `"cursor_query": cursor_query` to the details JSON.

- [ ] **Step 7: Build meta-only rows**

When creating agent row JSON, use canonical fields:

```rust
let include_task = input.include.contains(&DelegateListInclude::Task);
let include_summary = input.include.contains(&DelegateListInclude::Summary);
let include_activity = input.include.contains(&DelegateListInclude::Activity);
let mut row = super::multi_agent_format::agent_details(
    "agent",
    agent,
    None,
    super::multi_agent_format::SummaryScope::None,
    include_task,
    include_summary,
    include_activity,
);
row["kind"] = json!("agent");
```

For default text content, render only:

```rust
let detail = format!(
    "\n- agent_id: {} ({}) state: {} title: {}",
    agent.id.as_str(),
    agent.display_name.as_str(),
    agent.state.as_str(),
    agent.task_title,
);
```

- [ ] **Step 8: Add empty-state hint**

When `page_rows.is_empty()`, set content and details to include the hint:

```rust
let empty_next_steps = [
    "No active delegates found.",
    "Pass include_completed=true to list completed, failed, cancelled, or timed_out delegates.",
];
```

Add `"next_steps": empty_next_steps` to details only for empty results.

- [ ] **Step 9: Write the cursor mismatch test**

Add:

```rust
#[tokio::test]
async fn list_delegates_rejects_cursor_reused_with_different_query() {
    let (registry, ctx) = registry_with_multi_agent();
    for index in 0..4 {
        let _ = registry
            .run(
                "Delegate",
                &ctx,
                serde_json::json!({
                    "task": format!("task {index}"),
                    "mode": "foreground"
                }),
            )
            .await
            .expect("delegate should complete");
    }

    let first_page = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "include_completed": true,
                "state": "completed",
                "order": "oldest",
                "limit": 2
            }),
        )
        .await
        .expect("first page should succeed");
    let cursor = first_page.details.as_ref().unwrap()["next_cursor"]
        .as_str()
        .expect("next cursor")
        .to_owned();

    let mismatched = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "include_completed": true,
                "order": "oldest",
                "limit": 2,
                "cursor": cursor
            }),
        )
        .await;

    let err = mismatched.expect_err("mismatched cursor should be rejected");
    assert!(
        err.to_string().contains("different ListDelegates query"),
        "{err}"
    );
}
```

- [ ] **Step 10: Run ListDelegates tests**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime list_delegates_defaults_to_meta_only_rows_with_title
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime list_delegates_rejects_cursor_reused_with_different_query
```

Expected: both PASS.

- [ ] **Step 11: Commit checkpoint after explicit authorization**

If git mutation has been authorized for this execution instance, run:

```bash
rtk git add crates/neo-agent-core/src/tools/delegate_controls.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
rtk git commit -m "feat: tighten delegate list contract"
```

Expected: commit succeeds. Without authorization, do not run these commands.

## Task 5: Canonicalize WaitDelegate, DelegateSwarm, And TaskOutput Swarm Results

**Files:**
- Modify: `crates/neo-agent-core/src/tools/multi_agent_format.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Write the failing swarm shape test**

Add:

```rust
#[tokio::test]
async fn swarm_result_shape_matches_between_foreground_wait_and_task_output() {
    let (registry, ctx) = registry_with_multi_agent();
    let foreground = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "shape check",
                "items": ["a", "b"],
                "prompt_template": "Inspect {{item}}",
                "mode": "foreground"
            }),
        )
        .await
        .expect("foreground swarm should complete");
    let swarm_id = foreground.details.as_ref().unwrap()["swarm_id"]
        .as_str()
        .expect("swarm id")
        .to_owned();

    let waited = registry
        .run("WaitDelegate", &ctx, serde_json::json!({ "id": swarm_id }))
        .await
        .expect("wait should read completed swarm");
    let output = registry
        .run("TaskOutput", &ctx, serde_json::json!({ "task_id": swarm_id }))
        .await
        .expect("task output should read completed swarm");

    let foreground_details = foreground.details.as_ref().unwrap();
    let waited_details = waited.details.as_ref().unwrap();
    let output_details = output.details.as_ref().unwrap();

    for details in [foreground_details, waited_details, output_details] {
        assert_eq!(details["kind"], "delegate_swarm");
        assert_eq!(details["summary_scope"], "swarm_items");
        assert!(details["aggregate"]["total"].as_u64().is_some(), "{details}");
        assert!(details["items"][0]["name"].as_str().is_some(), "{details}");
        assert!(details["items"][0]["elapsed_ms"].as_u64().is_some(), "{details}");
        assert!(details["items"][0]["tool_count"].as_u64().is_some(), "{details}");
        assert!(details["items"][0]["token_count"].as_u64().is_some(), "{details}");
    }
}
```

- [ ] **Step 2: Run the swarm shape test to verify it fails**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime swarm_result_shape_matches_between_foreground_wait_and_task_output
```

Expected: FAIL because the three paths currently return different details shapes.

- [ ] **Step 3: Use `swarm_details` in foreground DelegateSwarm**

In `delegate.rs`, import:

```rust
use super::multi_agent_format::swarm_details;
```

Replace the foreground `DelegateSwarm` details JSON with:

```rust
            Ok(ToolResult::ok(format!(
                "swarm_id: {}\nstatus: {}\nsummary_scope: swarm_items\naggregate: total={} queued={} running={} completed={} failed={} cancelled={} timed_out={}",
                final_snapshot.swarm_id,
                final_snapshot.state.as_str(),
                final_snapshot.aggregate.total,
                final_snapshot.aggregate.queued,
                final_snapshot.aggregate.running,
                final_snapshot.aggregate.completed,
                final_snapshot.aggregate.failed,
                final_snapshot.aggregate.cancelled,
                final_snapshot.aggregate.timed_out,
            ))
            .with_details(swarm_details(&final_snapshot)))
```

- [ ] **Step 4: Use `swarm_details` in WaitDelegate**

In `delegate_controls.rs`, replace `format_swarm_result` details with:

```rust
    ToolResult::ok(content).with_details(super::multi_agent_format::swarm_details(swarm))
```

Keep compact text content.

- [ ] **Step 5: Use `swarm_details` in TaskOutput**

In `background_tasks.rs`, import or qualify `super::multi_agent_format::swarm_details`. In the `TaskOutput` swarm branch, replace the custom JSON details with:

```rust
                return Ok(ToolResult::ok(content)
                    .with_details(super::multi_agent_format::swarm_details(&swarm)));
```

- [ ] **Step 6: Run the swarm shape test**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime swarm_result_shape_matches_between_foreground_wait_and_task_output
```

Expected: PASS.

- [ ] **Step 7: Commit checkpoint after explicit authorization**

If git mutation has been authorized for this execution instance, run:

```bash
rtk git add crates/neo-agent-core/src/tools/multi_agent_format.rs crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/tools/delegate_controls.rs crates/neo-agent-core/src/tools/background_tasks.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
rtk git commit -m "feat: unify swarm result shape"
```

Expected: commit succeeds. Without authorization, do not run these commands.

## Task 6: Distinguish Wait Timeout From Delegate Timed-Out Status

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Write the failing wait timeout test**

Add:

```rust
#[tokio::test]
async fn wait_delegate_timeout_preserves_running_status_with_wait_timed_out_outcome() {
    let runtime = MultiAgentRuntime::new();
    let running = runtime.start_foreground_delegate_for_test("still running");
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_multi_agent(runtime);
    let registry = ToolRegistry::with_builtin_tools();

    let result = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({
                "id": running.id.as_str(),
                "timeout_ms": 1
            }),
        )
        .await
        .expect("wait should return timeout result");

    let details = result.details.as_ref().expect("wait details");
    assert_eq!(details["kind"], "delegate_wait");
    assert_eq!(details["outcome"], "wait_timed_out");
    assert_eq!(details["status"], "running");
    assert_eq!(details["id"], running.id.as_str());
}
```

- [ ] **Step 2: Run the wait timeout test to verify it fails**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime wait_delegate_timeout_preserves_running_status_with_wait_timed_out_outcome
```

Expected: FAIL because current details use `outcome: "timed_out"` and omit current status.

- [ ] **Step 3: Update agent wait timeout branch**

In `WaitDelegateTool::execute`, replace the agent timeout details with:

```rust
                    return Ok(ToolResult::ok(format!(
                        "id: {}\nstatus: running\noutcome: wait_timed_out\nnext_step: The delegate is still running. Increase timeout_ms, call ListDelegates, or wait for automatic completion.",
                        input.id,
                    ))
                    .with_details(json!({
                        "kind": "delegate_wait",
                        "id": input.id,
                        "task_id": input.id,
                        "status": "running",
                        "outcome": "wait_timed_out",
                        "next_steps": [
                            "The delegate is still running.",
                            "Increase timeout_ms, call ListDelegates, or wait for automatic completion."
                        ],
                    })));
```

- [ ] **Step 4: Update swarm wait timeout branch**

For swarm timeout, fetch the current snapshot and include the aggregate if present:

```rust
                        let mut details = json!({
                            "kind": "delegate_wait",
                            "id": input.id,
                            "task_id": input.id,
                            "status": "running",
                            "outcome": "wait_timed_out",
                        });
                        if let Some(swarm) = ctx.multi_agent.swarm_snapshot(&input.id) {
                            details["status"] = json!(swarm.state.as_str());
                            details["aggregate"] = json!(swarm.aggregate);
                        }
                        return Ok(ToolResult::ok(format!(
                            "id: {}\nstatus: {}\noutcome: wait_timed_out\nnext_step: The swarm is still running. Increase timeout_ms or use ListDelegates to check status.",
                            input.id,
                            details["status"].as_str().unwrap_or("running"),
                        ))
                        .with_details(details));
```

- [ ] **Step 5: Run the wait timeout test**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime wait_delegate_timeout_preserves_running_status_with_wait_timed_out_outcome
```

Expected: PASS.

- [ ] **Step 6: Commit checkpoint after explicit authorization**

If git mutation has been authorized for this execution instance, run:

```bash
rtk git add crates/neo-agent-core/src/tools/delegate_controls.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
rtk git commit -m "fix: distinguish delegate wait timeout outcome"
```

Expected: commit succeeds. Without authorization, do not run these commands.

## Task 7: Complete Model-Visible Tool Schema Guidance

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Replace the schema guidance test**

Replace `delegate_and_message_descriptions_explain_resume_and_live_followup` with:

```rust
#[test]
fn multi_agent_tool_descriptions_explain_contract_without_docs() {
    let registry = ToolRegistry::with_builtin_tools_and_todos(Arc::new(Mutex::new(Vec::new())));
    let specs = registry.specs();

    let spec = |name: &str| {
        specs
            .iter()
            .find(|spec| spec.name == name)
            .unwrap_or_else(|| panic!("{name} spec registered"))
    };

    let delegate = spec("Delegate");
    assert!(delegate.description.contains("Default mode is foreground"), "{}", delegate.description);
    assert!(delegate.description.contains("resume"), "{}", delegate.description);
    assert!(delegate.description.contains("role must be omitted"), "{}", delegate.description);
    assert!(delegate.description.contains("context"), "{}", delegate.description);

    let message = spec("MessageDelegate");
    assert!(message.description.contains("live"), "{}", message.description);
    assert!(message.description.contains("agent or swarm"), "{}", message.description);
    assert!(message.description.contains("running children"), "{}", message.description);
    assert!(message.description.contains("Delegate with resume"), "{}", message.description);

    let list = spec("ListDelegates");
    assert!(list.description.contains("active-only"), "{}", list.description);
    assert!(list.description.contains("meta-only"), "{}", list.description);
    assert!(list.description.contains("include_completed=true"), "{}", list.description);
    assert!(list.description.contains("same query"), "{}", list.description);

    let wait = spec("WaitDelegate");
    assert!(wait.description.contains("wait_timed_out"), "{}", wait.description);
    assert!(wait.description.contains("delegate itself reached timed_out"), "{}", wait.description);

    let swarm = spec("DelegateSwarm");
    assert!(swarm.description.contains("foreground"), "{}", swarm.description);
    assert!(swarm.description.contains("WaitDelegate"), "{}", swarm.description);
    assert!(swarm.description.contains("TaskOutput"), "{}", swarm.description);
}
```

- [ ] **Step 2: Run the schema guidance test to verify it fails**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime multi_agent_tool_descriptions_explain_contract_without_docs
```

Expected: FAIL because current descriptions do not mention every required behavior.

- [ ] **Step 3: Update `Delegate` description**

In `DelegateTool::description`, use:

```rust
        "Delegate work to a subagent. Default mode is foreground, so the main agent waits for the result. \
         Use mode=\"background\" only when the main agent should continue in parallel. \
         To continue an existing completed/failed/cancelled/timed_out agent, pass resume=\"agent_xxx\" and a new task; this starts a new run on the same agent. \
         When resume is set, role must be omitted because the resumed agent keeps its original role/profile/name/history. \
         context controls parent context passed to the child: inherit passes selected parent context, summary passes a compact parent summary, and none passes only the task plus role/profile prompt."
```

- [ ] **Step 4: Update `MessageDelegate` description**

In `MessageDelegateTool::description`, use:

```rust
        "Send a live follow-up message to a currently running delegate agent or broadcast \
         to running children of a swarm. The id may be an agent or swarm ID. \
         MessageDelegate does not queue offline messages for idle or terminal agents. \
         If the target is completed, failed, cancelled, timed_out, or not running, call Delegate with resume=\"agent_xxx\" instead."
```

- [ ] **Step 5: Update `ListDelegates` description**

In `ListDelegatesTool::description`, use:

```rust
        "List delegate agents and/or swarms with their current status. \
         Defaults to newest-first, active-only, all kinds, and meta-only rows. \
         Pass include_completed=true to see completed, failed, cancelled, or timed_out history. \
         Use include=[\"task\"], include=[\"summary\"], or include=[\"activity\"] only when that extra context is needed. \
         Pagination cursors are valid only with the same query parameters that produced them."
```

- [ ] **Step 6: Update `WaitDelegate` description**

In `WaitDelegateTool::description`, use:

```rust
        "Wait for a delegate agent or swarm to reach a terminal state (completed, failed, \
         cancelled, timed_out). A wait timeout returns outcome=\"wait_timed_out\" while preserving \
         the target's current status; this differs from a delegate whose own lifecycle status is timed_out. \
         For swarms, terminal results use the same structured shape as DelegateSwarm and TaskOutput."
```

- [ ] **Step 7: Update `DelegateSwarm` description**

In `DelegateSwarmTool::description`, use:

```rust
        "Run many related bounded tasks in subagents and return an ordered aggregate result. \
         Default mode is foreground; background returns immediately and exposes the same structured swarm result through WaitDelegate and TaskOutput. \
         Required: description, and either items with prompt_template containing {{item}}, resume_agent_ids, or both. \
         Optional {{description}} inserts the swarm description. Only {{item}} and {{description}} placeholders are supported."
```

- [ ] **Step 8: Update `TaskOutput` description for delegate IDs**

In `TaskOutputTool::description`, add this sentence inside Guidelines:

```rust
         - For delegate agent IDs and swarm IDs, this tool returns the canonical multi-agent result shape used by Delegate, DelegateSwarm, and WaitDelegate.\n\
```

- [ ] **Step 9: Run the schema guidance test**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime multi_agent_tool_descriptions_explain_contract_without_docs
```

Expected: PASS.

- [ ] **Step 10: Commit checkpoint after explicit authorization**

If git mutation has been authorized for this execution instance, run:

```bash
rtk git add crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/tools/delegate_controls.rs crates/neo-agent-core/src/tools/background_tasks.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
rtk git commit -m "docs: complete multi-agent tool schema guidance"
```

Expected: commit succeeds. Without authorization, do not run these commands.

## Task 8: Final Focused Verification

**Files:**
- Verify only files touched by Tasks 1-7.

- [ ] **Step 1: Run focused multi-agent runtime tests**

Run:

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime agent_snapshot_records_run_metadata_and_resume_origin
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_result_details_include_canonical_run_fields
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_resume_reuses_agent_identity_and_role
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime message_delegate_terminal_agent_error_explains_resume_without_immutable_confusion
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime list_delegates_defaults_to_meta_only_rows_with_title
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime list_delegates_rejects_cursor_reused_with_different_query
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime swarm_result_shape_matches_between_foreground_wait_and_task_output
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime wait_delegate_timeout_preserves_running_status_with_wait_timed_out_outcome
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime multi_agent_tool_descriptions_explain_contract_without_docs
```

Expected: all listed tests PASS.

- [ ] **Step 2: Run focused TaskOutput tests only if TaskOutput code changed outside swarm routing**

Run this only if implementation changed generic bash/question output behavior in `background_tasks.rs`:

```bash
rtk cargo nextest run -p neo-agent-core --test tool_bash bash_background_run_returns_task_id_and_task_output_finishes
```

Expected: PASS.

- [ ] **Step 3: Run rustfmt check**

Run:

```bash
rtk cargo fmt --all --check
```

Expected: PASS.

- [ ] **Step 4: Run targeted clippy for the touched crate test target**

Run:

```bash
rtk cargo clippy -p neo-agent-core --test multi_agent_runtime -- -D clippy::all
```

Expected: PASS.

- [ ] **Step 5: Commit final verification note after explicit authorization**

If all previous tasks were committed and git mutation has been authorized for this execution instance, no new commit is needed unless formatting changed files. If formatting changed files, run:

```bash
rtk git add crates/neo-agent-core
rtk git commit -m "chore: format multi-agent contract cleanup"
```

Expected: commit succeeds only when formatting changed files. Without authorization, do not run these commands.

## Self-Review Checklist

- Spec coverage:
  - Summary scope and resume metadata are covered by Tasks 1-3.
  - Terminal wording is covered by Task 3.
  - `ListDelegates` meta-only rows, empty hints, title, lifecycle fields, and cursor safety are covered by Task 4.
  - Swarm result consistency across foreground, wait, and task output is covered by Task 5.
  - Wait timeout versus delegate terminal timed-out status is covered by Task 6.
  - Context mode and model-visible lifecycle/schema wording are covered by Task 7.
- Placeholder scan:
  - This plan intentionally uses concrete test names, commands, file paths, and code snippets.
  - There are no deferred implementation sections.
- Type consistency:
  - Runtime fields use `run_count`, `live_messages_received`, `previous_status`, and `resumed_from`.
  - Result details expose `run_index` as the current `run_count`.
  - Swarm details use `kind: "delegate_swarm"` consistently.
