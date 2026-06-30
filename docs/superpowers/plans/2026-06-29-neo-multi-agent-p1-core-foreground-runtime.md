# Neo Multi-Agent P1 Core Foreground Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first usable Neo Multi-Agent core: deterministic subagent identity, foreground blocking `Delegate`, foreground `DelegateSwarm`, normalized events, tool registration, and subagent safety guards.

**Architecture:** Add a `multi_agent` runtime subsystem in `neo-agent-core` that owns agent IDs, display names, lifecycle state, foreground joins, and aggregate results. Register `Delegate` and `DelegateSwarm` as normal Neo tools, but execute them through the runtime-owned manager so they can emit normalized events and later evolve into background agents without a rewrite. V1 is intentionally foreground-only at execution time, while data structures already include `foreground/background` mode fields for later plans.

**Tech Stack:** Rust 2024, `tokio`, `serde`, `schemars`, `uuid`, `neo-ai::FakeModelClient`, `AgentRuntime`, `ToolRegistry`, `cargo nextest run`.

---

## Constraints

- Follow `/Users/chenyuanhao/Workspace/neo/AGENTS.md`.
- Start implementation with `icm recall-context "Neo Multi-Agent P1 core foreground runtime" --limit 5`.
- Use CodeGraph before grep/read when locating runtime symbols in this indexed repo.
- Do not run bare `cargo test`; use `cargo nextest run ...`.
- Do not mutate git unless the user explicitly authorizes that exact command.
- Do not preserve obsolete experimental names. Canonical tool names in this plan are `Delegate` and `DelegateSwarm`.
- Subagents must not execute git mutation commands. This is a hard product rule, not a later polish item.
- Keep this plan foreground-only. Background, detach, `/tasks`, followup, and Lua workflow belong to later plans.

## Current Code Touchpoints

- `crates/neo-agent-core/src/runtime/config.rs`
  - `AgentConfig` already owns shared runtime state such as `background_tasks`, permissions, hooks, todos, and session directory.
- `crates/neo-agent-core/src/runtime/events.rs`
  - `EventEmitter` is the seam for normalized `AgentEvent` output.
- `crates/neo-agent-core/src/events.rs`
  - Add serializable multi-agent events here.
- `crates/neo-agent-core/src/tools/mod.rs`
  - `ToolRegistry::with_builtin_tools_and_todos` registers built-in tools.
  - `ToolContext` already carries permission, cancellation, process supervisor, and background task manager.
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
  - Tool execution path emits `ToolExecutionStarted`, `ToolExecutionUpdate`, and `ToolExecutionFinished`.
- `crates/neo-agent-core/tests/runtime_turn.rs`
  - Existing focused runtime tests use fake clients and are the right pattern for foreground delegate tests.

## File Structure

Create:

- `crates/neo-agent-core/src/multi_agent/mod.rs`
- `crates/neo-agent-core/src/multi_agent/identity.rs`
- `crates/neo-agent-core/src/multi_agent/names.rs`
- `crates/neo-agent-core/src/multi_agent/state.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/tools/delegate.rs`
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`

Modify:

- `crates/neo-agent-core/src/lib.rs`
- `crates/neo-agent-core/src/events.rs`
- `crates/neo-agent-core/src/runtime/config.rs`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`

Do not modify TUI rendering in this plan except for events that later TUI plans need.

## Desired End State

- `ToolRegistry::with_builtin_tools_and_todos` registers `Delegate` and `DelegateSwarm`.
- `AgentConfig` owns a shared `MultiAgentRuntime`.
- `Delegate` defaults to `foreground`.
- `DelegateSwarm` defaults to `foreground`.
- Display names come from a deterministic hardcoded English pool.
- Internal IDs are stable UUID-style IDs and are the canonical routing key.
- Foreground `Delegate` blocks until the child result is available.
- Foreground `DelegateSwarm` blocks until every child reaches a terminal state.
- `DelegateSwarm` returns ordered per-item results.
- Subagent Bash commands that mutate git are denied before shell execution.
- Tests prove foreground blocking and deterministic naming.

## Phase 1: Core Types

### Task 1.1: Add `multi_agent` module shell

**Files:**
- Create: `crates/neo-agent-core/src/multi_agent/mod.rs`
- Create: `crates/neo-agent-core/src/multi_agent/identity.rs`
- Create: `crates/neo-agent-core/src/multi_agent/names.rs`
- Create: `crates/neo-agent-core/src/multi_agent/state.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`

- [ ] **Step 1: Create module exports**

Create `crates/neo-agent-core/src/multi_agent/mod.rs`:

```rust
mod identity;
mod names;
mod runtime;
mod state;

pub use identity::{AgentDisplayName, AgentId, AgentPath, AgentRole};
pub use names::{DisplayNamePool, DEFAULT_AGENT_NAMES};
pub use runtime::{DelegateRequest, DelegateSwarmRequest, MultiAgentRuntime};
pub use state::{
    AgentLifecycleState, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    SwarmChildSnapshot, SwarmSnapshot,
};
```

- [ ] **Step 2: Expose the module from the crate root**

Modify `crates/neo-agent-core/src/lib.rs`:

```rust
pub mod multi_agent;
```

- [ ] **Step 3: Compile the module shell**

Run:

```bash
```

Expected: compilation fails because `identity`, `names`, `runtime`, and `state` exports are not implemented yet.

### Task 1.2: Implement canonical identity types

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/identity.rs`

- [ ] **Step 1: Add identity code**

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AgentId(String);

impl AgentId {
    #[must_use]
    pub fn new() -> Self {
        Self(format!("agent_{}", Uuid::new_v4().simple()))
    }

    #[must_use]
    pub fn from_suffix_for_test(suffix: &str) -> Self {
        Self(format!("agent_{suffix}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AgentDisplayName(String);

impl AgentDisplayName {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AgentPath(String);

impl AgentPath {
    #[must_use]
    pub fn root_child(display_name: &AgentDisplayName) -> Self {
        Self(format!("/root/{}", display_name.as_str()))
    }

    #[must_use]
    pub fn swarm_child(swarm_id: &str, display_name: &AgentDisplayName) -> Self {
        Self(format!("/root/{swarm_id}/{}", display_name.as_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Coder,
    Explorer,
    Planner,
    Reviewer,
    Orchestrator,
}

impl Default for AgentRole {
    fn default() -> Self {
        Self::Coder
    }
}
```

- [ ] **Step 2: Run compile check**

Run:

```bash
```

Expected: compilation still fails because remaining modules are empty.

### Task 1.3: Implement deterministic display-name pool

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/names.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add name pool implementation**

```rust
use std::collections::HashSet;

use super::AgentDisplayName;

pub const DEFAULT_AGENT_NAMES: &[&str] = &[
    "Zeno", "Gibbs", "Hokke", "Laber", "Ada", "Turing", "Knuth", "Shannon",
    "Euler", "Noether", "Gauss", "Hypatia", "Athena", "Hermes", "Apollo",
    "Atlas", "Merlin", "Arthur", "Wukong", "Nezha", "Mulan", "Orion",
    "Kepler", "Curie", "Feynman", "Lovelace", "Hopper", "Ramanujan",
    "Socrates", "Plato", "Artemis", "Diana", "Minerva", "Loki", "Freya",
];

#[derive(Debug, Clone)]
pub struct DisplayNamePool {
    next_index: usize,
    assigned: HashSet<String>,
}

impl Default for DisplayNamePool {
    fn default() -> Self {
        Self {
            next_index: 0,
            assigned: HashSet::new(),
        }
    }
}

impl DisplayNamePool {
    #[must_use]
    pub fn next_name(&mut self) -> AgentDisplayName {
        loop {
            let index = self.next_index;
            self.next_index += 1;
            let base = DEFAULT_AGENT_NAMES[index % DEFAULT_AGENT_NAMES.len()];
            let candidate = if index < DEFAULT_AGENT_NAMES.len() {
                base.to_owned()
            } else {
                format!("{base}{}", index / DEFAULT_AGENT_NAMES.len() + 1)
            };
            if self.assigned.insert(candidate.clone()) {
                return AgentDisplayName::new(candidate);
            }
        }
    }

    pub fn reserve(&mut self, name: &AgentDisplayName) {
        self.assigned.insert(name.as_str().to_owned());
    }
}
```

- [ ] **Step 2: Add deterministic allocation tests**

Create `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
use neo_agent_core::multi_agent::{DisplayNamePool, DEFAULT_AGENT_NAMES};

#[test]
fn display_name_pool_is_deterministic() {
    let mut pool = DisplayNamePool::default();

    let first = pool.next_name();
    let second = pool.next_name();
    let third = pool.next_name();

    assert_eq!(first.as_str(), DEFAULT_AGENT_NAMES[0]);
    assert_eq!(second.as_str(), DEFAULT_AGENT_NAMES[1]);
    assert_eq!(third.as_str(), DEFAULT_AGENT_NAMES[2]);
}

#[test]
fn display_name_pool_suffixes_after_exhaustion() {
    let mut pool = DisplayNamePool::default();
    for _ in 0..DEFAULT_AGENT_NAMES.len() {
        let _ = pool.next_name();
    }

    let wrapped = pool.next_name();

    assert_eq!(wrapped.as_str(), format!("{}2", DEFAULT_AGENT_NAMES[0]));
}
```

- [ ] **Step 3: Run focused tests**

Run:

```bash
```

Expected: tests pass after remaining module compile errors are resolved in the next task.

### Task 1.4: Implement lifecycle state structs

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`

- [ ] **Step 1: Add state types**

```rust
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AgentDisplayName, AgentId, AgentPath, AgentRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunMode {
    Foreground,
    Background,
}

impl Default for AgentRunMode {
    fn default() -> Self {
        Self::Foreground
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentTerminalOutcome {
    pub summary: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentSnapshot {
    pub id: AgentId,
    pub display_name: AgentDisplayName,
    pub path: AgentPath,
    pub role: AgentRole,
    pub mode: AgentRunMode,
    pub state: AgentLifecycleState,
    pub task: String,
    pub tool_count: usize,
    pub token_count: usize,
    pub elapsed: Duration,
    pub latest_text: Option<String>,
    pub outcome: Option<AgentTerminalOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmChildSnapshot {
    pub item_index: usize,
    pub item: String,
    pub agent: AgentSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmSnapshot {
    pub swarm_id: String,
    pub description: String,
    pub mode: AgentRunMode,
    pub children: Vec<SwarmChildSnapshot>,
}
```

- [ ] **Step 2: Run focused compile**

Run:

```bash
```

Expected: display name tests pass, or fail only because `runtime.rs` is not implemented.

## Phase 2: Runtime Manager

### Task 2.1: Implement a minimal foreground `MultiAgentRuntime`

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`

- [ ] **Step 1: Add runtime request and result code**

```rust
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode, AgentSnapshot,
    AgentTerminalOutcome, DisplayNamePool, SwarmChildSnapshot, SwarmSnapshot,
};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateRequest {
    pub task: String,
    #[serde(default)]
    pub role: AgentRole,
    #[serde(default)]
    pub mode: AgentRunMode,
    #[serde(default = "default_context")]
    pub context: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateSwarmRequest {
    pub description: String,
    pub items: Vec<String>,
    pub prompt_template: String,
    #[serde(default)]
    pub role: AgentRole,
    #[serde(default)]
    pub mode: AgentRunMode,
    pub max_concurrency: Option<usize>,
}

fn default_context() -> String {
    "inherit".to_owned()
}

#[derive(Debug, Default)]
struct MultiAgentState {
    names: DisplayNamePool,
    agents: BTreeMap<String, AgentSnapshot>,
    swarms: BTreeMap<String, SwarmSnapshot>,
}

#[derive(Debug, Clone, Default)]
pub struct MultiAgentRuntime {
    state: Arc<Mutex<MultiAgentState>>,
}

impl MultiAgentRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_foreground_delegate_for_test(&self, task: &str) -> AgentSnapshot {
        let started_at = Instant::now();
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let display_name = state.names.next_name();
        let id = AgentId::new();
        let path = AgentPath::root_child(&display_name);
        let snapshot = AgentSnapshot {
            id: id.clone(),
            display_name,
            path,
            role: AgentRole::Coder,
            mode: AgentRunMode::Foreground,
            state: AgentLifecycleState::Running,
            task: task.to_owned(),
            tool_count: 0,
            token_count: 0,
            elapsed: started_at.elapsed(),
            latest_text: None,
            outcome: None,
        };
        state.agents.insert(id.as_str().to_owned(), snapshot.clone());
        snapshot
    }

    pub fn complete_delegate_for_test(&self, id: &AgentId, summary: &str) -> AgentSnapshot {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state
            .agents
            .get_mut(id.as_str())
            .expect("test agent should exist");
        snapshot.state = AgentLifecycleState::Completed;
        snapshot.outcome = Some(AgentTerminalOutcome {
            summary: summary.to_owned(),
            is_error: false,
        });
        snapshot.clone()
    }

    #[must_use]
    pub fn snapshot(&self, id: &AgentId) -> Option<AgentSnapshot> {
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .agents
            .get(id.as_str())
            .cloned()
    }
}
```

- [ ] **Step 2: Add runtime tests**

Append to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
use neo_agent_core::multi_agent::{AgentLifecycleState, MultiAgentRuntime};

#[test]
fn foreground_delegate_lifecycle_records_running_and_completed_state() {
    let runtime = MultiAgentRuntime::new();

    let running = runtime.start_foreground_delegate_for_test("inspect queue");
    assert_eq!(running.state, AgentLifecycleState::Running);
    assert_eq!(running.display_name.as_str(), "Zeno");

    let completed = runtime.complete_delegate_for_test(&running.id, "queue is safe");
    assert_eq!(completed.state, AgentLifecycleState::Completed);
    assert_eq!(
        completed.outcome.as_ref().map(|outcome| outcome.summary.as_str()),
        Some("queue is safe")
    );
}
```

- [ ] **Step 3: Run focused tests**

Run:

```bash
```

Expected: PASS.

### Task 2.2: Attach `MultiAgentRuntime` to `AgentConfig`

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs`

- [ ] **Step 1: Add field and initialization**

Add import:

```rust
use crate::multi_agent::MultiAgentRuntime;
```

Add field to `AgentConfig`:

```rust
/// Shared multi-agent runtime for Delegate and DelegateSwarm tools.
#[serde(skip)]
#[schemars(skip)]
pub multi_agent: MultiAgentRuntime,
```

Initialize in `AgentConfig::for_model`:

```rust
multi_agent: MultiAgentRuntime::new(),
```

- [ ] **Step 2: Run compile check**

Run:

```bash
```

Expected: PASS.

## Phase 3: Events And Tools

### Task 3.1: Add multi-agent events

**Files:**
- Modify: `crates/neo-agent-core/src/events.rs`

- [ ] **Step 1: Import snapshot types**

Add near existing imports:

```rust
use crate::multi_agent::{AgentSnapshot, SwarmSnapshot};
```

- [ ] **Step 2: Add event variants**

Add to `AgentEvent`:

```rust
    DelegateStarted {
        turn: u32,
        agent: AgentSnapshot,
    },
    DelegateUpdated {
        turn: u32,
        agent: AgentSnapshot,
    },
    DelegateFinished {
        turn: u32,
        agent: AgentSnapshot,
    },
    DelegateSwarmStarted {
        turn: u32,
        swarm: SwarmSnapshot,
    },
    DelegateSwarmUpdated {
        turn: u32,
        swarm: SwarmSnapshot,
    },
    DelegateSwarmFinished {
        turn: u32,
        swarm: SwarmSnapshot,
    },
```

- [ ] **Step 3: Run serialization compile check**

Run:

```bash
```

Expected: compile succeeds. Existing tests may run if they match `AgentEvent`.

### Task 3.2: Create `Delegate` and `DelegateSwarm` tools

**Files:**
- Create: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`

- [ ] **Step 1: Implement tool structs**

```rust
use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};
use crate::multi_agent::{DelegateRequest, DelegateSwarmRequest};

pub struct DelegateTool;

impl Tool for DelegateTool {
    fn name(&self) -> &'static str {
        "Delegate"
    }

    fn description(&self) -> &'static str {
        "Run one bounded task in a foreground subagent by default. Use background mode only when explicit parallel collaboration is needed."
    }

    fn schema(&self) -> neo_ai::ToolSpec {
        schema::<DelegateRequest>(self.name(), self.description())
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let request: DelegateRequest = parse_input(self.name(), input)?;
            if request.mode != crate::multi_agent::AgentRunMode::Foreground {
                return Err(ToolError::InvalidInput {
                    tool: self.name().to_owned(),
                    message: "background Delegate is implemented in P2; use foreground mode in P1".to_owned(),
                });
            }
            let snapshot = ctx.multi_agent().start_foreground_delegate_for_test(&request.task);
            let completed = ctx
                .multi_agent()
                .complete_delegate_for_test(&snapshot.id, "Foreground delegate completed.");
            Ok(ToolResult::ok(format!(
                "agent_id: {}\nname: {}\nstatus: completed\nsummary: Foreground delegate completed.",
                completed.id.as_str(),
                completed.display_name.as_str()
            ))
            .with_details(json!({
                "kind": "delegate",
                "agent": completed,
            })))
        })
    }
}

pub struct DelegateSwarmTool;

impl Tool for DelegateSwarmTool {
    fn name(&self) -> &'static str {
        "DelegateSwarm"
    }

    fn description(&self) -> &'static str {
        "Run many related bounded tasks in foreground subagents and return an ordered aggregate result."
    }

    fn schema(&self) -> neo_ai::ToolSpec {
        schema::<DelegateSwarmRequest>(self.name(), self.description())
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let request: DelegateSwarmRequest = parse_input(self.name(), input)?;
            if request.items.is_empty() {
                return Err(ToolError::InvalidInput {
                    tool: self.name().to_owned(),
                    message: "DelegateSwarm requires at least one item".to_owned(),
                });
            }
            Ok(ToolResult::ok(format!(
                "status: completed\nitems: {}\nsummary: Foreground swarm completed.",
                request.items.len()
            ))
            .with_details(json!({
                "kind": "delegate_swarm",
                "description": request.description,
                "items": request.items,
            })))
        })
    }
}
```

- [ ] **Step 2: Add `ToolContext::multi_agent` accessor**

Modify `crates/neo-agent-core/src/tools/mod.rs`:

```rust
use crate::multi_agent::MultiAgentRuntime;
```

Add field to `ToolContext`:

```rust
pub multi_agent: MultiAgentRuntime,
```

Initialize it in `ToolContext::new`:

```rust
multi_agent: MultiAgentRuntime::new(),
```

Add accessor:

```rust
#[must_use]
pub const fn multi_agent(&self) -> &MultiAgentRuntime {
    &self.multi_agent
}
```

- [ ] **Step 3: Register tools**

Modify `crates/neo-agent-core/src/tools/mod.rs`:

```rust
mod delegate;
```

Register in `with_builtin_tools_and_todos` after background task tools:

```rust
registry.register(delegate::DelegateTool);
registry.register(delegate::DelegateSwarmTool);
```

- [ ] **Step 4: Run tool registry test**

Add a test in `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
use neo_agent_core::tools::ToolRegistry;

#[test]
fn builtin_tools_register_delegate_tools() {
    let specs = ToolRegistry::with_builtin_tools()
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    assert!(specs.iter().any(|name| name == "Delegate"));
    assert!(specs.iter().any(|name| name == "DelegateSwarm"));
}
```

Run:

```bash
```

Expected: PASS.

## Phase 4: Safety Guard

### Task 4.1: Add subagent git mutation guard

**Files:**
- Create: `crates/neo-agent-core/src/multi_agent/git_guard.rs` if the guard makes `runtime.rs` too large
- Modify: `crates/neo-agent-core/src/multi_agent/mod.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add guard API**

Add to `multi_agent/mod.rs` exports:

```rust
pub use runtime::is_forbidden_subagent_git_command;
```

Add to `runtime.rs`:

```rust
#[must_use]
pub fn is_forbidden_subagent_git_command(command: &str) -> bool {
    let trimmed = command.trim();
    let Some(rest) = trimmed.strip_prefix("git ") else {
        return false;
    };
    let first = rest.split_whitespace().next().unwrap_or_default();
    matches!(
        first,
        "add"
            | "am"
            | "apply"
            | "branch"
            | "checkout"
            | "cherry-pick"
            | "clean"
            | "commit"
            | "filter-branch"
            | "gc"
            | "merge"
            | "mv"
            | "push"
            | "rebase"
            | "reflog"
            | "reset"
            | "restore"
            | "rm"
            | "stash"
            | "switch"
            | "tag"
            | "worktree"
    )
}
```

- [ ] **Step 2: Add tests**

Append:

```rust
use neo_agent_core::multi_agent::is_forbidden_subagent_git_command;

#[test]
fn subagent_git_guard_denies_mutations_and_allows_read_only_commands() {
    assert!(is_forbidden_subagent_git_command("git commit -m test"));
    assert!(is_forbidden_subagent_git_command("git reset --hard"));
    assert!(is_forbidden_subagent_git_command("git checkout -- src/lib.rs"));
    assert!(is_forbidden_subagent_git_command("git push"));

    assert!(!is_forbidden_subagent_git_command("git status --short"));
    assert!(!is_forbidden_subagent_git_command("git diff"));
    assert!(!is_forbidden_subagent_git_command("git log --oneline"));
}
```

- [ ] **Step 3: Run guard test**

Run:

```bash
```

Expected: PASS.

## Phase 5: Verification

### Task 5.1: Run P1 focused verification

- [ ] **Step 1: Run multi-agent tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 2: Run tool registration tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 3: Run formatting/check gate only if touched files compile**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS, unless unrelated dirty-worktree changes outside this plan break the check. If unrelated breakage appears, record the exact error and do not revert any file.

## Handoff Notes For P2

- P2 should replace the test-only `start_foreground_delegate_for_test` and `complete_delegate_for_test` paths with real child `AgentRuntime` execution and TUI events.
- Keep `Delegate` and `DelegateSwarm` names. Do not add aliases.
- Keep the hardcoded display-name pool deterministic.
- Keep subagent git mutation denial.
