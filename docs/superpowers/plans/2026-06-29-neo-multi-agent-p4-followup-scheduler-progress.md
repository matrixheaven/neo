# Neo Multi-Agent P4 Followup Scheduler And Progress Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn background delegates into reusable collaborators and mature `DelegateSwarm` with followup messaging, partial resume, rate-limit retry, adaptive concurrency, per-child outcome reports, and Bayesian-style progress.

**Architecture:** Extend `MultiAgentRuntime` from a task registry into a session-tree collaboration controller. Add bounded mailbox queues per agent, resumable swarm item records, scheduler phases, and a progress estimator that uses completed child durations plus live activity instead of naive item counts. Keep all data normalized in Neo state so transcript and `/tasks` can render the same source of truth.

**Tech Stack:** Rust 2024, `tokio`, `CancellationToken`, `serde`, `AgentEvent`, `BackgroundTaskManager`, `neo-tui` transcript components, focused `cargo run -p xtask -- test`.

---

## Constraints

- Follow `/Users/chenyuanhao/Workspace/neo/AGENTS.md`.
- Start with `icm recall-context "Neo Multi-Agent P4 followup scheduler progress" --limit 5`.
- Use CodeGraph before grep/read.
- Do not add a second agent UI.
- Do not let mailbox messages auto-inject large transcripts into parent context.
- Do not introduce compatibility aliases for tool names.

## Current Code Touchpoints

- `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - Registry and foreground/background state from P1-P3.
- `crates/neo-agent-core/src/tools/delegate_controls.rs`
  - Add `MessageDelegate`.
- `crates/neo-agent-core/src/events.rs`
  - Add `DelegateMailboxUpdated` and `DelegateSwarmProgressUpdated` only if existing P1-P3 delegate/swarm update events cannot carry the new fields.
- `crates/neo-tui/src/transcript/swarm_card.rs`
  - Render progress estimate and suspended/rate-limited states.
- `crates/neo-agent/src/modes/task_browser.rs`
  - Preview mailbox and swarm scheduler state.

## File Structure

Create:

- `crates/neo-agent-core/src/multi_agent/mailbox.rs`
- `crates/neo-agent-core/src/multi_agent/scheduler.rs`
- `crates/neo-agent-core/src/multi_agent/progress.rs`
- `crates/neo-agent-core/tests/multi_agent_scheduler.rs`

Modify:

- `crates/neo-agent-core/src/multi_agent/mod.rs`
- `crates/neo-agent-core/src/multi_agent/state.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/tools/delegate_controls.rs`
- `crates/neo-agent-core/src/events.rs`
- `crates/neo-tui/src/transcript/swarm_card.rs`
- `crates/neo-agent/src/modes/task_browser.rs`

## Desired End State

- `MessageDelegate` appends a bounded message to a background or idle subagent.
- `WaitDelegate` can wake on mailbox update, completion, timeout, or interruption.
- Swarms can resume only failed/cancelled/unstarted items.
- Rate-limited children enter `suspended` and retry with backoff.
- Effective concurrency shrinks under provider pressure and recovers after a quiet window.
- Progress estimate advances while work is active but never claims impossible completion.
- Swarm result includes per-child status, summary, usage, and failure reason.

## Phase 1: Mailbox

### Task 1.1: Add mailbox types

**Files:**
- Create: `crates/neo-agent-core/src/multi_agent/mailbox.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/mod.rs`

- [ ] **Step 1: Implement mailbox**

```rust
use std::collections::VecDeque;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegateMailboxMessage {
    pub id: String,
    pub text: String,
    pub delivered: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegateMailbox {
    messages: VecDeque<DelegateMailboxMessage>,
    next_id: u64,
}

impl DelegateMailbox {
    pub fn push(&mut self, text: String) -> DelegateMailboxMessage {
        self.next_id += 1;
        let message = DelegateMailboxMessage {
            id: format!("msg_{}", self.next_id),
            text,
            delivered: false,
        };
        self.messages.push_back(message.clone());
        message
    }

    pub fn pending(&self) -> Vec<DelegateMailboxMessage> {
        self.messages
            .iter()
            .filter(|message| !message.delivered)
            .cloned()
            .collect()
    }

    pub fn mark_delivered(&mut self, id: &str) {
        if let Some(message) = self.messages.iter_mut().find(|message| message.id == id) {
            message.delivered = true;
        }
    }
}
```

- [ ] **Step 2: Export mailbox**

In `multi_agent/mod.rs`:

```rust
mod mailbox;
pub use mailbox::{DelegateMailbox, DelegateMailboxMessage};
```

- [ ] **Step 3: Add mailbox test**

Create `crates/neo-agent-core/tests/multi_agent_scheduler.rs`:

```rust
use neo_agent_core::multi_agent::DelegateMailbox;

#[test]
fn delegate_mailbox_tracks_pending_delivery() {
    let mut mailbox = DelegateMailbox::default();
    let message = mailbox.push("check the failed test".to_owned());

    assert_eq!(mailbox.pending().len(), 1);
    mailbox.mark_delivered(&message.id);
    assert!(mailbox.pending().is_empty());
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core delegate_mailbox_tracks_pending_delivery
```

Expected: PASS.

### Task 1.2: Implement `MessageDelegate`

**Files:**
- Modify: `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`

- [ ] **Step 1: Add input**

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MessageDelegateInput {
    pub id: String,
    pub message: String,
}
```

- [ ] **Step 2: Implement tool**

`MessageDelegate` should:

1. resolve the target by agent ID, swarm ID, or display name
2. reject foreground-only active joins
3. append to the target mailbox
4. return a small model-facing confirmation

Output:

```text
target: Gibbs
status: queued
message_id: msg_1
next_step: Use WaitDelegate if the result is needed before continuing.
```

- [ ] **Step 3: Register tool**

Add:

```rust
registry.register(delegate_controls::MessageDelegateTool);
```

- [ ] **Step 4: Run test**

Add and run:

```bash
cargo run -p xtask -- test -p neo-agent-core message_delegate_queues_mailbox_message
```

Expected: PASS.

## Phase 2: Swarm Scheduler

### Task 2.1: Add scheduler state

**Files:**
- Create: `crates/neo-agent-core/src/multi_agent/scheduler.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/mod.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`

- [ ] **Step 1: Implement scheduler types**

```rust
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SwarmItemState {
    Queued,
    Running,
    SuspendedRateLimit,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct SwarmSchedulerConfig {
    pub max_concurrency: usize,
    pub retry_base_delay: Duration,
    pub provider_quiet_window: Duration,
}

impl Default for SwarmSchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            retry_base_delay: Duration::from_secs(3),
            provider_quiet_window: Duration::from_secs(180),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwarmRetryState {
    pub attempts: usize,
    pub retry_after: Instant,
}

#[derive(Debug, Clone)]
pub struct SwarmScheduler {
    config: SwarmSchedulerConfig,
    effective_concurrency: usize,
}

impl SwarmScheduler {
    #[must_use]
    pub fn new(config: SwarmSchedulerConfig) -> Self {
        let effective_concurrency = config.max_concurrency;
        Self {
            config,
            effective_concurrency,
        }
    }

    #[must_use]
    pub fn effective_concurrency(&self) -> usize {
        self.effective_concurrency
    }

    pub fn record_rate_limit(&mut self) {
        self.effective_concurrency = self.effective_concurrency.saturating_sub(1).max(1);
    }

    #[must_use]
    pub fn retry_delay(&self, attempts: usize) -> Duration {
        self.config.retry_base_delay * (1_u32 << attempts.min(5))
    }
}
```

- [ ] **Step 2: Export scheduler**

```rust
mod scheduler;
pub use scheduler::{SwarmItemState, SwarmRetryState, SwarmScheduler, SwarmSchedulerConfig};
```

- [ ] **Step 3: Add scheduler tests**

```rust
use neo_agent_core::multi_agent::{SwarmScheduler, SwarmSchedulerConfig};

#[test]
fn swarm_scheduler_reduces_concurrency_on_rate_limit() {
    let mut scheduler = SwarmScheduler::new(SwarmSchedulerConfig::default());

    scheduler.record_rate_limit();

    assert_eq!(scheduler.effective_concurrency(), 3);
}

#[test]
fn swarm_scheduler_retry_delay_grows_exponentially() {
    let scheduler = SwarmScheduler::new(SwarmSchedulerConfig::default());

    assert!(scheduler.retry_delay(2) > scheduler.retry_delay(1));
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core swarm_scheduler
```

Expected: PASS.

### Task 2.2: Implement partial swarm resume

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`

- [ ] **Step 1: Add resume method**

Add:

```rust
pub fn resumable_swarm_items(&self, swarm_id: &str) -> Vec<usize> {
    let state = self.state.lock().expect("multi-agent state poisoned");
    let Some(swarm) = state.swarms.get(swarm_id) else {
        return Vec::new();
    };
    swarm
        .children
        .iter()
        .filter(|child| {
            matches!(
                child.agent.state,
                AgentLifecycleState::Queued
                    | AgentLifecycleState::Failed
                    | AgentLifecycleState::Cancelled
            )
        })
        .map(|child| child.item_index)
        .collect()
}
```

- [ ] **Step 2: Add test**

```rust
#[test]
fn partial_swarm_resume_skips_completed_items() {
    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.create_swarm_for_test(vec![
        ("done", AgentLifecycleState::Completed),
        ("failed", AgentLifecycleState::Failed),
        ("queued", AgentLifecycleState::Queued),
    ]);

    let resumable = runtime.resumable_swarm_items(&swarm_id);

    assert_eq!(resumable, vec![1, 2]);
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core partial_swarm_resume_skips_completed_items
```

Expected: PASS.

## Phase 3: Progress Estimator

### Task 3.1: Add Bayesian-style progress estimator

**Files:**
- Create: `crates/neo-agent-core/src/multi_agent/progress.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/mod.rs`

- [ ] **Step 1: Implement estimator**

```rust
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwarmProgressInput {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub running: usize,
    pub queued: usize,
    pub suspended: usize,
    pub median_completed_duration: Option<Duration>,
    pub longest_running_duration: Duration,
}

#[must_use]
pub fn estimate_swarm_progress(input: SwarmProgressInput) -> f32 {
    if input.total == 0 {
        return 1.0;
    }
    let terminal = input.completed + input.failed;
    if terminal >= input.total {
        return 1.0;
    }

    let base = terminal as f32 / input.total as f32;
    let running_credit = if input.running == 0 {
        0.0
    } else {
        let median = input
            .median_completed_duration
            .unwrap_or_else(|| Duration::from_secs(120));
        let ratio = input.longest_running_duration.as_secs_f32() / median.as_secs_f32().max(1.0);
        ratio.clamp(0.05, 0.85)
    };
    let unfinished_weight = input.running as f32 / input.total as f32;
    (base + running_credit * unfinished_weight).min(0.95)
}
```

- [ ] **Step 2: Export estimator**

```rust
mod progress;
pub use progress::{SwarmProgressInput, estimate_swarm_progress};
```

- [ ] **Step 3: Add tests**

```rust
use std::time::Duration;
use neo_agent_core::multi_agent::{SwarmProgressInput, estimate_swarm_progress};

#[test]
fn progress_estimate_never_claims_completion_while_items_are_active() {
    let progress = estimate_swarm_progress(SwarmProgressInput {
        total: 4,
        completed: 3,
        failed: 0,
        running: 1,
        queued: 0,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(10)),
        longest_running_duration: Duration::from_secs(100),
    });

    assert!(progress < 1.0);
    assert!(progress <= 0.95);
}
```

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core progress_estimate
```

Expected: PASS.

## Phase 4: TUI And Task Browser Updates

### Task 4.1: Render mature progress and suspended states

**Files:**
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-agent/src/modes/task_browser.rs`

- [ ] **Step 1: Add status labels**

Swarm child rows must render:

```text
running
done
failed
cancelled
suspended rate-limit
queued
```

- [ ] **Step 2: Show estimated percent**

Add percent to header:

```text
▸ DelegateSwarm  Audit and fix Neo tool schemas                     41%
```

Use `estimate_swarm_progress` from runtime state, not a local TUI-only guess.

- [ ] **Step 3: Add tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui swarm_card_renders_suspended_rate_limit
cargo run -p xtask -- test -p neo-agent task_browser_adapter_maps_swarm_progress
```

Expected: PASS.

## Phase 5: Verification

- [ ] Run:

```bash
cargo run -p xtask -- test -p neo-agent-core multi_agent_scheduler
```

Expected: PASS.

- [ ] Run:

```bash
cargo run -p xtask -- test -p neo-tui multi_agent_transcript
```

Expected: PASS.

- [ ] Run:

```bash
cargo run -p xtask -- check
```

Expected: PASS unless unrelated dirty-worktree changes break the global check. Report unrelated breakage without reverting files.

## Handoff Notes For P5

- P5 owns Lua workflow orchestration.
- Do not add YAML or JS compatibility.
- Lua should call the stable Delegate/DelegateSwarm/control APIs built by P1-P4.
