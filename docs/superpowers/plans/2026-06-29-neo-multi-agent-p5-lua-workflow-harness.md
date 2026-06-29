# Neo Multi-Agent P5 Lua Workflow Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Lua workflow harness so repeated multi-agent procedures can be scripted, resumed at explicit step boundaries, rendered in the transcript, and verified through Neo's normal permission path.

**Architecture:** Embed Lua through `mlua` behind a small Neo-owned host API. Lua scripts cannot access arbitrary OS capabilities; they can call `neo.delegate`, `neo.swarm`, `neo.wait`, `neo.verify`, `neo.report`, and `neo.fail`. Workflow state persists at Neo step boundaries, not by checkpointing the Lua VM stack. No YAML or JS compatibility layer is introduced.

**Tech Stack:** Rust 2024, `mlua`, `serde`, `tokio`, `AgentEvent`, `ToolRegistry`, `MultiAgentRuntime`, focused `cargo run -p xtask -- test`.

---

## Constraints

- Follow `/Users/chenyuanhao/Workspace/neo/AGENTS.md`.
- Start with `icm recall-context "Neo Multi-Agent P5 Lua workflow harness" --limit 5`.
- Use CodeGraph before grep/read.
- Use Lua as the only canonical workflow language.
- Do not implement YAML orchestration.
- Do not implement JS/TS orchestration.
- Do not expose raw filesystem, process, or network APIs to Lua.
- Workflow shell verification must go through Neo permissions.

## Current Code Touchpoints

- `crates/neo-agent-core/Cargo.toml`
  - Add `mlua` to `neo-agent-core`; workflow execution lives in core.
- `crates/neo-agent-core/src/multi_agent/`
  - Stable delegate/swarm/control APIs from P1-P4.
- `crates/neo-agent-core/src/tools/mod.rs`
  - Register workflow tool if workflow execution is model-callable.
- `crates/neo-agent-core/src/events.rs`
  - Add workflow transcript events.
- `crates/neo-tui/src/transcript/`
  - Render workflow cards.
- `crates/neo-agent-core/src/tools/bash.rs`
  - Verification commands must route through existing shell permission checks, not direct Lua OS calls.

## File Structure

Create:

- `crates/neo-agent-core/src/workflow/mod.rs`
- `crates/neo-agent-core/src/workflow/error.rs`
- `crates/neo-agent-core/src/workflow/state.rs`
- `crates/neo-agent-core/src/workflow/lua.rs`
- `crates/neo-agent-core/src/workflow/host_api.rs`
- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-tui/src/transcript/workflow_card.rs`
- `crates/neo-agent-core/tests/workflow_lua.rs`
- `crates/neo-tui/tests/workflow_transcript.rs`

Modify:

- `crates/neo-agent-core/src/lib.rs`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent-core/src/events.rs`
- `crates/neo-tui/src/transcript/mod.rs`
- `crates/neo-tui/src/transcript/event_handler.rs`
- workspace `Cargo.toml` or crate `Cargo.toml`

## Desired End State

Lua workflow example:

```lua
local audit = neo.swarm({
  description = "Find risky runtime state transitions",
  role = "reviewer",
  mode = "foreground",
  items = {
    "runtime turn lifecycle",
    "queue and steer",
    "background task completion",
  },
  prompt_template = "Review {{item}} and return concrete risks."
})

if audit:has_failures() then
  neo.fail("Audit did not complete cleanly")
end

local fix = neo.delegate({
  role = "coder",
  mode = "foreground",
  task = "Fix the highest-confidence issue from the audit summary."
})

neo.verify("cargo run -p xtask -- test -p neo-agent-core runtime")
neo.report({ audit = audit:summary(), fix = fix:summary() })
```

## Phase 1: Workflow State

### Task 1.1: Add workflow module and state types

**Files:**
- Create: `crates/neo-agent-core/src/workflow/mod.rs`
- Create: `crates/neo-agent-core/src/workflow/state.rs`
- Create: `crates/neo-agent-core/src/workflow/error.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`

- [ ] **Step 1: Implement state**

`state.rs`:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    Running,
    Failed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowStepRecord {
    pub index: usize,
    pub name: String,
    pub state: WorkflowState,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowSnapshot {
    pub id: WorkflowId,
    pub title: String,
    pub state: WorkflowState,
    pub steps: Vec<WorkflowStepRecord>,
}
```

`error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("lua error: {0}")]
    Lua(String),
    #[error("workflow failed: {0}")]
    Failed(String),
    #[error("host API error: {0}")]
    Host(String),
}
```

`mod.rs`:

```rust
mod error;
mod host_api;
mod lua;
mod state;

pub use error::WorkflowError;
pub use lua::LuaWorkflowRunner;
pub use state::{WorkflowId, WorkflowSnapshot, WorkflowState, WorkflowStepRecord};
```

Crate root:

```rust
pub mod workflow;
```

- [ ] **Step 2: Run compile**

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core workflow
```

Expected: compile fails only until Lua files are implemented in the next task.

## Phase 2: Lua Runner

### Task 2.1: Add `mlua` dependency and minimal runner

**Files:**
- Modify: `crates/neo-agent-core/Cargo.toml`
- Create: `crates/neo-agent-core/src/workflow/lua.rs`
- Test: `crates/neo-agent-core/tests/workflow_lua.rs`

- [ ] **Step 1: Add dependency**

Add to `crates/neo-agent-core/Cargo.toml`:

```toml
mlua = { version = "0.10", features = ["lua54", "vendored", "send"] }
```

- [ ] **Step 2: Implement minimal runner**

```rust
use mlua::Lua;

use super::WorkflowError;

#[derive(Debug, Clone, Default)]
pub struct LuaWorkflowRunner;

impl LuaWorkflowRunner {
    pub fn run_script(&self, source: &str) -> Result<(), WorkflowError> {
        let lua = Lua::new();
        lua.load(source)
            .exec()
            .map_err(|err| WorkflowError::Lua(err.to_string()))
    }
}
```

- [ ] **Step 3: Add tests**

```rust
use neo_agent_core::workflow::LuaWorkflowRunner;

#[test]
fn lua_workflow_runner_executes_basic_script() {
    let runner = LuaWorkflowRunner::default();

    runner.run_script("local x = 1 + 1").expect("script should run");
}

#[test]
fn lua_workflow_runner_reports_lua_errors() {
    let runner = LuaWorkflowRunner::default();

    let err = runner.run_script("error('boom')").expect_err("script should fail");

    assert!(err.to_string().contains("boom"));
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core lua_workflow_runner
```

Expected: PASS.

## Phase 3: Host API

### Task 3.1: Add sandboxed `neo` table

**Files:**
- Create: `crates/neo-agent-core/src/workflow/host_api.rs`
- Modify: `crates/neo-agent-core/src/workflow/lua.rs`
- Test: `crates/neo-agent-core/tests/workflow_lua.rs`

- [ ] **Step 1: Implement host API result recorder**

```rust
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct WorkflowHostRecorder {
    calls: Arc<Mutex<Vec<String>>>,
}

impl WorkflowHostRecorder {
    pub fn record(&self, call: impl Into<String>) {
        self.calls.lock().expect("workflow recorder poisoned").push(call.into());
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("workflow recorder poisoned").clone()
    }
}
```

- [ ] **Step 2: Install `neo.report` and `neo.fail`**

In `lua.rs`, add a method that creates a `neo` table:

```rust
let neo = lua.create_table()?;
let report = lua.create_function(|_, value: mlua::Value| {
    Ok(format!("{value:?}"))
})?;
neo.set("report", report)?;
let fail = lua.create_function(|_, message: String| -> mlua::Result<()> {
    Err(mlua::Error::RuntimeError(message))
})?;
neo.set("fail", fail)?;
lua.globals().set("neo", neo)?;
```

Map errors through `WorkflowError::Lua`.

- [ ] **Step 3: Add tests**

```rust
#[test]
fn lua_workflow_exposes_neo_report() {
    let runner = LuaWorkflowRunner::default();

    runner.run_script("neo.report({ ok = true })").expect("report should run");
}

#[test]
fn lua_workflow_exposes_neo_fail() {
    let runner = LuaWorkflowRunner::default();

    let err = runner.run_script("neo.fail('not good')").expect_err("fail should error");

    assert!(err.to_string().contains("not good"));
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core lua_workflow_exposes_neo
```

Expected: PASS.

### Task 3.2: Add `neo.delegate`, `neo.swarm`, and `neo.verify` host stubs

**Files:**
- Modify: `crates/neo-agent-core/src/workflow/host_api.rs`
- Modify: `crates/neo-agent-core/src/workflow/lua.rs`

- [ ] **Step 1: Add stub semantics**

For P5 initial harness:

- `neo.delegate(table)` records a workflow step and returns an object with `summary()`.
- `neo.swarm(table)` records a workflow step and returns an object with `summary()` and `has_failures()`.
- `neo.verify(command)` records a verification command; command execution must be wired to Neo permissions in the next task.

Do not use `std::process::Command` directly.

- [ ] **Step 2: Add tests**

```rust
#[test]
fn lua_workflow_can_call_delegate_swarm_and_verify() {
    let runner = LuaWorkflowRunner::default();
    runner
        .run_script(
            r#"
            local audit = neo.swarm({ description = "audit", items = {"a"}, prompt_template = "{{item}}" })
            assert(audit:has_failures() == false)
            local fix = neo.delegate({ task = "fix issue" })
            neo.verify("cargo run -p xtask -- test -p neo-agent-core workflow")
            neo.report({ audit = audit:summary(), fix = fix:summary() })
            "#,
        )
        .expect("workflow should run");
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core lua_workflow_can_call_delegate_swarm_and_verify
```

Expected: PASS.

## Phase 4: Workflow Tool And Events

### Task 4.1: Add `RunWorkflow` tool

**Files:**
- Create: `crates/neo-agent-core/src/tools/workflow.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`

- [ ] **Step 1: Implement tool input**

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RunWorkflowInput {
    pub title: String,
    pub script: String,
}
```

- [ ] **Step 2: Implement tool**

`RunWorkflow` executes `LuaWorkflowRunner` and returns:

```text
workflow: <title>
status: completed
```

or:

```text
workflow: <title>
status: failed
error: <message>
```

Details include `kind = "workflow"`, title, status, and recorded steps.

- [ ] **Step 3: Register tool**

```rust
mod workflow;
registry.register(workflow::RunWorkflowTool);
```

- [ ] **Step 4: Add tests**

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core run_workflow_tool_executes_lua
```

Expected: PASS.

### Task 4.2: Add workflow transcript card

**Files:**
- Create: `crates/neo-tui/src/transcript/workflow_card.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Test: `crates/neo-tui/tests/workflow_transcript.rs`

- [ ] **Step 1: Add card**

Render:

```text
▸ Workflow  Runtime audit and fix             running
  ✓ swarm: audit
  ✓ delegate: fix issue
  ● verify: cargo run -p xtask -- test ...
```

- [ ] **Step 2: Add events**

Add `AgentEvent::WorkflowStarted`, `WorkflowUpdated`, `WorkflowFinished` carrying `WorkflowSnapshot`.

- [ ] **Step 3: Add routing tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui workflow_transcript
```

Expected: PASS.

## Phase 5: Verification

- [ ] Run:

```bash
cargo run -p xtask -- test -p neo-agent-core workflow
```

Expected: PASS.

- [ ] Run:

```bash
cargo run -p xtask -- test -p neo-tui workflow_transcript
```

Expected: PASS.

- [ ] Run:

```bash
cargo run -p xtask -- check
```

Expected: PASS unless unrelated dirty-worktree changes break the global check. Report unrelated breakage without reverting files.

## Completion Notes

- Lua is the only built-in workflow language.
- YAML may be used later for metadata only, not orchestration.
- JS/TS is not implemented.
- Workflow resume is step-based. Do not promise Lua VM checkpointing.
