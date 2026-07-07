# Neo Local Context Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Neo's split context accounting, micro-compaction, overflow recovery, and full-compaction trigger paths with one runtime-only local context management pipeline.

**Architecture:** Add a pure `ContextBudgetSnapshot` layer, a pure `CompactionController`, and explicit request/summary projection. Then route `turn_loop` through these boundaries and delete the old `maybe_compact` orchestration so footer display, trigger decisions, overflow recovery, and request building share one token budget source.

**Tech Stack:** Rust 2024, `neo-agent-core`, `neo-tui`, `cargo nextest`, serde-compatible JSONL events, local-only `ModelClient` summary compaction.

---

## Review Notes

The requested Subagent review could not be started because this Codex environment did not expose a callable Subagent/Delegate tool. I reviewed the spec locally from four independent angles and carried the findings into the plan:

- Budget correctness: reuse the current `estimate_effective_context_tokens` fix from issue #12 as the source material for `ContextBudgetEstimator`; do not create a second estimator.
- Tool group safety: do not compact mid-group. Record debt after each completed result and enforce it before the next model call.
- Overflow recovery: record overflow from the same budget snapshot used for the failed request, not from messages-only estimates.
- Migration discipline: delete `runtime/compaction_trigger.rs::maybe_compact` and `compaction/mod.rs::run_compaction` after their helpers move. Do not leave dual orchestration paths.

## Policy Notes

- Do not run git mutation commands from this plan without explicit user authorization for that specific command. This includes `git add`, `git commit`, `git stash`, `git checkout`, `git restore`, and `git reset`.
- Use the narrow verification commands listed in each task. Do not use broad `cargo test` or package-wide `cargo nextest run` as completion evidence.
- Keep provider remote compaction and model-callable context tools out of scope.

## File Map

- Create: `crates/neo-agent-core/src/runtime/context_budget.rs`
  - Owns `ContextBudgetSnapshot`, `ContextThresholds`, `ContextWindowSource`, `ContextBudgetEstimator`, and tests for budget math.
- Create: `crates/neo-agent-core/src/runtime/compaction_controller.rs`
  - Owns `CompactionController`, `CompactionDecision`, `CompactionUrgency`, `DeferredCompaction`, and `ToolGroupBudgetState`.
- Create: `crates/neo-agent-core/src/compaction/projection.rs`
  - Replaces request-time behavior from `compaction/micro.rs` with explicit `ProjectionPlan` and `ProjectionResult`.
- Create: `crates/neo-agent-core/src/compaction/summary.rs`
  - Moves summary-generation orchestration out of `runtime/compaction_trigger.rs` and old `compaction/mod.rs::run_compaction`.
- Modify: `crates/neo-agent-core/src/runtime/mod.rs`
  - Expose new runtime modules internally.
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`
  - Export `projection` and `summary`; retain safe-boundary helpers; remove old orchestration entrypoint.
- Modify: `crates/neo-agent-core/src/runtime/chat_request.rs`
  - Accept a `ProjectionPlan`; apply projection only as instructed; keep provider-valid sanitization.
- Modify: `crates/neo-agent-core/src/runtime/turn_loop.rs`
  - Use `CompactionController` before model calls, after tool groups, and during overflow recovery.
- Modify: `crates/neo-agent-core/src/runtime/events.rs`
  - Emit context window updates from `ContextBudgetSnapshot`.
- Modify: `crates/neo-agent-core/src/events.rs`
  - Add serde-compatible optional fields to `ContextWindowUpdated`; add optional deferred event if needed.
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
  - Keep config TOML shape stable while adding internal grouped accessors for budget/projection/full compaction.
- Modify: `crates/neo-agent-core/src/runtime/context.rs`
  - Keep durable state only; prevent `estimated_context_tokens()` from becoming a trigger/UI source.
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
  - Stop seeding live footer usage from raw durable `AgentContext::estimated_context_tokens()`.
- Modify: `crates/neo-tui/src/shell/context.rs`
  - Carry projected/max/trigger/source/deferred display fields without breaking old events.
- Modify: `crates/neo-tui/src/shell/event_router.rs`
  - Route new context window and deferred compaction event fields.
- Modify tests:
  - `crates/neo-agent-core/src/runtime/context_budget.rs`
  - `crates/neo-agent-core/src/runtime/compaction_controller.rs`
  - `crates/neo-agent-core/src/compaction/projection.rs`
  - `crates/neo-agent-core/tests/runtime_turn.rs`
  - `crates/neo-agent-core/tests/session_jsonl.rs`
  - `crates/neo-tui/tests/app_shell.rs`

## Task 1: Add Projection Types and Move Micro Compaction Behavior

**Files:**
- Create: `crates/neo-agent-core/src/compaction/projection.rs`
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`
- Keep temporarily: `crates/neo-agent-core/src/compaction/micro.rs`

- [ ] **Step 1: Write projection tests before implementation**

Add these tests to `crates/neo-agent-core/src/compaction/projection.rs` under `#[cfg(test)]`:

```rust
#[test]
fn request_projection_truncates_old_large_tool_results() {
    let messages = vec![
        AgentMessage::user_text("start"),
        AgentMessage::tool_result(
            "old_call",
            "Read",
            vec![Content::text("x".repeat(8_000))],
            false,
        ),
        AgentMessage::tool_result(
            "new_call",
            "Read",
            vec![Content::text("y".repeat(8_000))],
            false,
        ),
    ];
    let plan = ProjectionPlan {
        enabled: true,
        cutoff_index: 2,
        min_tool_result_tokens: 100,
        keep_recent_messages: 1,
        mode: ProjectionMode::Request,
    };

    let result = project_for_request(&messages, &plan);

    assert_eq!(messages[1].text().len(), 8_000);
    assert!(result.messages[1].text().contains("[tool result omitted"));
    assert_eq!(result.messages[2].text().len(), 8_000);
    assert!(result.omitted_tokens > 1_000);
    assert!(result.projected_tokens < crate::runtime::estimate_messages_tokens(&messages));
}

#[test]
fn projection_never_changes_user_or_assistant_messages() {
    let assistant = AgentMessage::assistant_text("assistant payload");
    let user = AgentMessage::user_text("user payload");
    let messages = vec![user.clone(), assistant.clone()];
    let plan = ProjectionPlan {
        enabled: true,
        cutoff_index: messages.len(),
        min_tool_result_tokens: 1,
        keep_recent_messages: 0,
        mode: ProjectionMode::Request,
    };

    let result = project_for_request(&messages, &plan);

    assert_eq!(result.messages, messages);
}

#[test]
fn summary_projection_can_be_more_aggressive_than_request_projection() {
    let messages = vec![
        AgentMessage::tool_result("a", "Read", vec![Content::text("a".repeat(4_000))], false),
        AgentMessage::tool_result("b", "Read", vec![Content::text("b".repeat(4_000))], false),
    ];
    let request_plan = ProjectionPlan {
        enabled: true,
        cutoff_index: 1,
        min_tool_result_tokens: 100,
        keep_recent_messages: 1,
        mode: ProjectionMode::Request,
    };
    let summary_plan = ProjectionPlan {
        mode: ProjectionMode::SummaryInput,
        keep_recent_messages: 0,
        ..request_plan
    };

    let request = project_for_request(&messages, &request_plan);
    let summary = project_for_summary(&messages, &summary_plan);

    assert!(summary.omitted_tokens > request.omitted_tokens);
    assert!(summary.projected_tokens < request.projected_tokens);
}
```

- [ ] **Step 2: Run the projection test target and confirm it fails because the module does not exist**

Run:

```bash
cargo nextest run -p neo-agent-core --lib request_projection_truncates_old_large_tool_results
```

Expected: FAIL at compile time with an unresolved `projection` module or unresolved `ProjectionPlan`.

- [ ] **Step 3: Implement `projection.rs` by moving behavior from `micro.rs`**

Create `crates/neo-agent-core/src/compaction/projection.rs` with these public types:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMode {
    None,
    Request,
    SummaryInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionPlan {
    pub enabled: bool,
    pub cutoff_index: usize,
    pub min_tool_result_tokens: usize,
    pub keep_recent_messages: usize,
    pub mode: ProjectionMode,
}

impl ProjectionPlan {
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            cutoff_index: 0,
            min_tool_result_tokens: usize::MAX,
            keep_recent_messages: usize::MAX,
            mode: ProjectionMode::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionResult {
    pub messages: Vec<AgentMessage>,
    pub omitted_tokens: usize,
    pub projected_tokens: usize,
}
```

Implement:

```rust
#[must_use]
pub fn project_for_request(messages: &[AgentMessage], plan: &ProjectionPlan) -> ProjectionResult {
    project_messages(messages, plan, ProjectionMode::Request)
}

#[must_use]
pub fn project_for_summary(messages: &[AgentMessage], plan: &ProjectionPlan) -> ProjectionResult {
    project_messages(messages, plan, ProjectionMode::SummaryInput)
}
```

The implementation must:

- Return an owned copy of `messages` unchanged when `plan.enabled == false` or `plan.mode == ProjectionMode::None`.
- Only replace `AgentMessage::ToolResult` content.
- Never replace messages at indexes `>= messages.len().saturating_sub(plan.keep_recent_messages)`.
- Never replace messages at indexes `>= plan.cutoff_index`.
- Replace large tool result text with a marker containing `tool_name` and the omitted token estimate.
- Set `projected_tokens` using `crate::runtime::estimate_messages_tokens(&projected_messages)`.

- [ ] **Step 4: Export the module**

Modify `crates/neo-agent-core/src/compaction/mod.rs`:

```rust
pub mod projection;
pub mod micro;
```

Keep `micro` exported in this task only. It will be removed after `chat_request` migrates.

- [ ] **Step 5: Verify projection tests pass**

Run:

```bash
cargo nextest run -p neo-agent-core --lib request_projection_truncates_old_large_tool_results
cargo nextest run -p neo-agent-core --lib projection_never_changes_user_or_assistant_messages
cargo nextest run -p neo-agent-core --lib summary_projection_can_be_more_aggressive_than_request_projection
```

Expected: all three tests PASS.

## Task 2: Add Context Budget Snapshot

**Files:**
- Create: `crates/neo-agent-core/src/runtime/context_budget.rs`
- Modify: `crates/neo-agent-core/src/runtime/mod.rs`
- Modify: `crates/neo-agent-core/src/runtime/tokens.rs`
- Modify: `crates/neo-agent-core/src/runtime/config.rs`

- [ ] **Step 1: Write budget tests**

Add tests in `context_budget.rs` for these exact names:

```rust
#[test]
fn budget_includes_system_workspace_transform_and_tools() {
    let tool = ToolSpec {
        name: "LargeSchemaTool".to_owned(),
        description: "tool description ".repeat(80),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "payload": { "type": "string", "description": "schema ".repeat(160) }
            }
        }),
    };
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history ".repeat(100)));
    let config = AgentConfig::for_model(fake_model())
        .with_system_prompt("system ".repeat(40))
        .with_tools(vec![tool])
        .with_context_append_transform(|_| vec![AgentMessage::system_text("transform ".repeat(40))])
        .with_compaction(CompactionSettings::new(usize::MAX, 4));

    let snapshot = ContextBudgetEstimator::snapshot(&config, &context, ProjectionPlan::disabled());

    assert!(snapshot.fixed_overhead_tokens > 0);
    assert!(snapshot.tool_schema_tokens > 0);
    assert!(snapshot.raw_effective_tokens > context.estimated_tokens());
}

#[test]
fn budget_uses_observed_overflow_when_smaller() {
    let mut config = AgentConfig::for_model(fake_model())
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.model.capabilities.max_context_tokens = Some(200_000);
    super::config::observe_context_overflow(&config, 100_000);
    let context = AgentContext::new();

    let snapshot = ContextBudgetEstimator::snapshot(&config, &context, ProjectionPlan::disabled());

    assert_eq!(snapshot.effective_max_context_tokens, Some(85_000));
    assert_eq!(snapshot.source, ContextWindowSource::ObservedOverflow);
}

#[test]
fn small_window_uses_lower_trigger_ratio() {
    let mut config = AgentConfig::for_model(fake_model())
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.model.capabilities.max_context_tokens = Some(64_000);
    let context = AgentContext::new();

    let snapshot = ContextBudgetEstimator::snapshot(&config, &context, ProjectionPlan::disabled());

    assert_eq!(snapshot.trigger_tokens, Some(51_200));
}
```

- [ ] **Step 2: Run one budget test and confirm it fails before implementation**

Run:

```bash
cargo nextest run -p neo-agent-core --lib budget_includes_system_workspace_transform_and_tools
```

Expected: FAIL at compile time with unresolved `ContextBudgetEstimator`.

- [ ] **Step 3: Implement budget types**

Create:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub enum ContextWindowSource {
    Configured,
    ObservedOverflow,
    MissingModelWindow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetSnapshot {
    pub turn: u32,
    pub durable_tokens: usize,
    pub fixed_overhead_tokens: usize,
    pub tool_schema_tokens: usize,
    pub raw_effective_tokens: usize,
    pub projected_tokens: usize,
    pub max_context_tokens: Option<usize>,
    pub effective_max_context_tokens: Option<usize>,
    pub trigger_tokens: Option<usize>,
    pub reserved_headroom_tokens: usize,
    pub remaining_to_trigger: Option<usize>,
    pub remaining_to_max: Option<usize>,
    pub source: ContextWindowSource,
    pub projection: ProjectionPlan,
}

pub struct ContextBudgetEstimator;
```

Implement `ContextBudgetEstimator::snapshot(config, context, projection)` by reusing current logic from `runtime/tokens.rs::estimate_effective_context_tokens`:

- `durable_tokens = context.estimated_tokens()`
- fixed request overhead includes system prompt, workspace context, and context append transform
- `tool_schema_tokens = estimate_tool_specs_tokens(&config.tools)`
- `raw_effective_tokens = durable + fixed_overhead + tool_schema`
- `projected_tokens = raw_effective_tokens` when projection disabled
- if projection enabled, estimate projected message tokens plus the same fixed overhead and tool schema
- `max_context_tokens` comes from `config.model.capabilities.max_context_tokens`
- `effective_max_context_tokens` uses configured max capped by `config.observed_max_context_tokens`
- `trigger_tokens` uses `0.8` when effective max is `<= 128_000`, otherwise `settings.trigger_ratio`

- [ ] **Step 4: Export the module internally**

Modify `crates/neo-agent-core/src/runtime/mod.rs`:

```rust
mod context_budget;
pub(crate) use context_budget::*;
```

- [ ] **Step 5: Verify all budget tests**

Run:

```bash
cargo nextest run -p neo-agent-core --lib budget_includes_system_workspace_transform_and_tools
cargo nextest run -p neo-agent-core --lib budget_uses_observed_overflow_when_smaller
cargo nextest run -p neo-agent-core --lib small_window_uses_lower_trigger_ratio
```

Expected: all three tests PASS.

## Task 3: Add Pure Compaction Controller

**Files:**
- Create: `crates/neo-agent-core/src/runtime/compaction_controller.rs`
- Modify: `crates/neo-agent-core/src/runtime/mod.rs`
- Modify: `crates/neo-agent-core/src/events.rs`

- [ ] **Step 1: Write controller tests**

Add these tests in `compaction_controller.rs`:

```rust
#[test]
fn decision_no_action_below_threshold() {
    let snapshot = test_snapshot(10_000, Some(100_000), Some(80_000));
    let decision = CompactionController::decide_before_model_call(snapshot, None, false);
    assert!(matches!(decision, CompactionDecision::NoAction { .. }));
}

#[test]
fn decision_runs_full_compaction_at_ratio_threshold() {
    let snapshot = test_snapshot(80_000, Some(100_000), Some(80_000));
    let decision = CompactionController::decide_before_model_call(snapshot, None, false);
    assert!(matches!(
        decision,
        CompactionDecision::RunFullCompaction {
            reason: CompactionReason::Threshold,
            urgency: CompactionUrgency::Normal,
            ..
        }
    ));
}

#[test]
fn decision_uses_deferred_tool_group_debt_before_next_model_call() {
    let snapshot = test_snapshot(10_000, Some(100_000), Some(80_000));
    let debt = DeferredCompaction {
        reason: CompactionReason::Threshold,
        urgency: CompactionUrgency::DeferredAfterToolGroup,
        first_triggered_after_call_index: 1,
        projected_tokens_at_trigger: 90_000,
    };

    let decision = CompactionController::decide_before_model_call(snapshot, Some(&debt), false);

    assert!(matches!(
        decision,
        CompactionDecision::RunFullCompaction {
            urgency: CompactionUrgency::DeferredAfterToolGroup,
            ..
        }
    ));
}

#[test]
fn tool_group_records_deferred_debt_after_first_result_crosses_threshold() {
    let initial = test_snapshot(40_000, Some(100_000), Some(80_000));
    let mut group = ToolGroupBudgetState::new(7, 3, initial);
    let crossed = test_snapshot(82_000, Some(100_000), Some(80_000));

    let debt = group.observe_completed_result(0, crossed).expect("debt");

    assert_eq!(debt.first_triggered_after_call_index, 0);
    assert_eq!(group.completed_calls, 1);
    assert!(!group.is_finished());
}
```

- [ ] **Step 2: Run one controller test and confirm it fails before implementation**

Run:

```bash
cargo nextest run -p neo-agent-core --lib decision_no_action_below_threshold
```

Expected: FAIL at compile time with unresolved `CompactionController`.

- [ ] **Step 3: Implement controller types**

Use these signatures:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionDecision {
    NoAction { snapshot: ContextBudgetSnapshot },
    UseProjectionOnly { snapshot: ContextBudgetSnapshot, plan: ProjectionPlan },
    RunFullCompaction {
        snapshot: ContextBudgetSnapshot,
        reason: CompactionReason,
        urgency: CompactionUrgency,
    },
    ForceAfterOverflow {
        snapshot: ContextBudgetSnapshot,
        observed_limit: usize,
    },
    StopWithContextError { snapshot: ContextBudgetSnapshot, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub enum CompactionUrgency {
    Normal,
    DeferredAfterToolGroup,
    UrgentBeforeNextModelCall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredCompaction {
    pub reason: CompactionReason,
    pub urgency: CompactionUrgency,
    pub first_triggered_after_call_index: usize,
    pub projected_tokens_at_trigger: usize,
}
```

`CompactionController::decide_before_model_call(snapshot, pending_debt, manual_requested)` must prioritize:

1. pending debt
2. manual request
3. projected tokens at or above effective max
4. projected tokens plus reserved headroom at or above effective max
5. projected tokens at or above trigger tokens
6. projection-only when projection enabled and full compaction is not required
7. no action

- [ ] **Step 4: Export the module internally**

Modify `runtime/mod.rs`:

```rust
mod compaction_controller;
pub(crate) use compaction_controller::*;
```

- [ ] **Step 5: Verify controller tests**

Run:

```bash
cargo nextest run -p neo-agent-core --lib decision_no_action_below_threshold
cargo nextest run -p neo-agent-core --lib decision_runs_full_compaction_at_ratio_threshold
cargo nextest run -p neo-agent-core --lib decision_uses_deferred_tool_group_debt_before_next_model_call
cargo nextest run -p neo-agent-core --lib tool_group_records_deferred_debt_after_first_result_crosses_threshold
```

Expected: all four tests PASS.

## Task 4: Make Chat Requests Consume Projection Plans

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/chat_request.rs`
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`
- Delete later: `crates/neo-agent-core/src/compaction/micro.rs`

- [ ] **Step 1: Add chat request projection tests**

Add unit tests to `chat_request.rs`:

```rust
#[tokio::test]
async fn chat_request_applies_supplied_projection_plan() {
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::tool_result(
        "call",
        "Read",
        vec![Content::text("x".repeat(8_000))],
        false,
    ));
    let config = AgentConfig::for_model(test_model()).with_compaction(CompactionSettings::new(usize::MAX, 4));
    let plan = ProjectionPlan {
        enabled: true,
        cutoff_index: 1,
        min_tool_result_tokens: 100,
        keep_recent_messages: 0,
        mode: ProjectionMode::Request,
    };

    let request = chat_request(&config, &context, &plan).await;

    let rendered = format!("{:?}", request.messages);
    assert!(rendered.contains("[tool result omitted"));
}

#[tokio::test]
async fn chat_request_disabled_projection_keeps_tool_result_content() {
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::tool_result(
        "call",
        "Read",
        vec![Content::text("x".repeat(8_000))],
        false,
    ));
    let config = AgentConfig::for_model(test_model()).with_compaction(CompactionSettings::new(usize::MAX, 4));

    let request = chat_request(&config, &context, &ProjectionPlan::disabled()).await;

    let rendered = format!("{:?}", request.messages);
    assert!(rendered.contains(&"x".repeat(100)));
}
```

- [ ] **Step 2: Run the first chat request test and confirm it fails**

Run:

```bash
cargo nextest run -p neo-agent-core --lib chat_request_applies_supplied_projection_plan
```

Expected: FAIL at compile time because `chat_request` still accepts two arguments.

- [ ] **Step 3: Change `chat_request` signature**

Change:

```rust
pub(super) async fn chat_request(config: &AgentConfig, context: &AgentContext) -> ChatRequest
```

to:

```rust
pub(super) async fn chat_request(
    config: &AgentConfig,
    context: &AgentContext,
    projection_plan: &ProjectionPlan,
) -> ChatRequest
```

Replace the internal `micro::apply_micro_compaction` block with:

```rust
let projection = crate::compaction::projection::project_for_request(
    &context_messages,
    projection_plan,
);
let context_messages = projection.messages;
```

Keep `sanitize_tool_exchange_messages` after projection.

- [ ] **Step 4: Update call sites to pass disabled projection temporarily**

Update current call sites in `turn_loop.rs` to pass `&ProjectionPlan::disabled()` until Task 7 wires the controller:

```rust
let request = chat_request(&config, &emitter.context, &ProjectionPlan::disabled()).await;
```

- [ ] **Step 5: Verify chat request tests**

Run:

```bash
cargo nextest run -p neo-agent-core --lib chat_request_applies_supplied_projection_plan
cargo nextest run -p neo-agent-core --lib chat_request_disabled_projection_keeps_tool_result_content
```

Expected: both tests PASS.

## Task 5: Extract Local Summary Compaction

**Files:**
- Create: `crates/neo-agent-core/src/compaction/summary.rs`
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`
- Modify: `crates/neo-agent-core/src/runtime/compaction_trigger.rs`

- [ ] **Step 1: Add summary orchestration tests**

Add unit tests in `summary.rs`:

```rust
fn fake_summary_harness() -> FakeHarness {
    FakeHarness::from_events([
        AiStreamEvent::MessageStart { id: "summary".to_owned() },
        AiStreamEvent::TextDelta { text: "summary".to_owned() },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ])
}

fn context_with_old_large_tool_result() -> AgentContext {
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("before"));
    context.append_message(AgentMessage::tool_result(
        "call",
        "Read",
        vec![Content::text("x".repeat(16_000))],
        false,
    ));
    context.append_message(AgentMessage::user_text("after"));
    context
}

#[tokio::test]
async fn full_compaction_uses_summary_projection_before_llm_request() {
    let harness = fake_summary_harness();
    let model = harness.client();
    let mut context = context_with_old_large_tool_result();
    let config = AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(1, 1));
    let snapshot = ContextBudgetEstimator::snapshot(
        &config,
        &context,
        ProjectionPlan {
            enabled: true,
            cutoff_index: context.messages().len(),
            min_tool_result_tokens: 100,
            keep_recent_messages: 0,
            mode: ProjectionMode::SummaryInput,
        },
    );

    let outcome = run_full_compaction(
        &model,
        &config,
        &mut context,
        CompactionReason::Threshold,
        snapshot,
        &CancellationToken::new(),
        |_| {},
    )
    .await
    .expect("compaction should succeed");

    assert!(outcome.projection_omitted_tokens > 0);
    assert!(context.compaction_summary().is_some());
}
```

- [ ] **Step 2: Run summary test and confirm it fails before implementation**

Run:

```bash
cargo nextest run -p neo-agent-core --lib full_compaction_uses_summary_projection_before_llm_request
```

Expected: FAIL at compile time with unresolved `summary::run_full_compaction`.

- [ ] **Step 3: Move summary orchestration into `summary.rs`**

Move these behaviors from `runtime/compaction_trigger.rs` into `compaction/summary.rs`:

- summary task spawning
- progress-loop handling
- stale history check
- `CompactionSummary` construction
- `AgentContext.apply_compaction`

Expose:

```rust
pub async fn run_full_compaction(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    context: &mut AgentContext,
    reason: CompactionReason,
    snapshot: ContextBudgetSnapshot,
    cancel_token: &CancellationToken,
    mut emit: impl FnMut(AgentEvent),
) -> Result<CompactionOutcome, CompactionError>
```

`run_full_compaction` must call `project_for_summary` before `generate_with_retry`, and `CompactionOutcome` must include:

```rust
pub struct CompactionOutcome {
    pub compacted_message_count: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub summary_tokens: usize,
    pub projection_omitted_tokens: usize,
}
```

- [ ] **Step 4: Export summary module**

Modify `compaction/mod.rs`:

```rust
pub mod projection;
pub mod summary;
```

- [ ] **Step 5: Verify summary test**

Run:

```bash
cargo nextest run -p neo-agent-core --lib full_compaction_uses_summary_projection_before_llm_request
```

Expected: PASS.

## Task 6: Add JSONL-Compatible Context Events

**Files:**
- Modify: `crates/neo-agent-core/src/events.rs`
- Modify: `crates/neo-agent-core/src/runtime/events.rs`
- Modify: `crates/neo-agent-core/tests/session_jsonl.rs`

- [ ] **Step 1: Add JSONL compatibility tests**

Add tests to `session_jsonl.rs`:

```rust
#[test]
fn replay_accepts_old_context_window_updated_shape() {
    let json = r#"{"type":"ContextWindowUpdated","turn":1,"used_tokens":123}"#;
    let event: AgentEvent = serde_json::from_str(json).expect("old event should parse");
    assert!(matches!(
        event,
        AgentEvent::ContextWindowUpdated { turn: 1, used_tokens: 123, .. }
    ));
}

#[test]
fn replay_accepts_compaction_summary_without_new_metadata() {
    let json = r#"{
        "summary":"old summary",
        "tokens_before":100,
        "tokens_after":50,
        "first_kept_message_index":2
    }"#;
    let summary: CompactionSummary = serde_json::from_str(json).expect("old summary should parse");
    assert_eq!(summary.summary, "old summary");
    assert_eq!(summary.first_kept_message_index, 2);
}
```

- [ ] **Step 2: Run compatibility test before event migration**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_jsonl replay_accepts_old_context_window_updated_shape
```

Expected: PASS before and after migration. If it fails before migration because enum tagging differs, adjust the test JSON to match the existing session format and keep the assertion.

- [ ] **Step 3: Extend `ContextWindowUpdated` with optional fields**

Change event shape to:

```rust
ContextWindowUpdated {
    turn: u32,
    used_tokens: u32,
    #[serde(default)]
    projected_tokens: Option<u32>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    trigger_tokens: Option<u32>,
    #[serde(default)]
    remaining_tokens: Option<u32>,
    #[serde(default)]
    source: Option<ContextWindowSource>,
}
```

Add:

```rust
ContextCompactionDeferred {
    turn: u32,
    reason: CompactionReason,
    urgency: CompactionUrgency,
    projected_tokens: u32,
    max_tokens: Option<u32>,
}
```

- [ ] **Step 4: Emit updates from snapshots**

Replace `emit_context_window_update(emitter, turn, used_tokens)` with:

```rust
pub(super) fn emit_context_window_snapshot(
    emitter: &mut EventEmitter,
    snapshot: &ContextBudgetSnapshot,
) {
    let used_tokens = u32::try_from(snapshot.raw_effective_tokens).unwrap_or(u32::MAX);
    let projected_tokens = Some(u32::try_from(snapshot.projected_tokens).unwrap_or(u32::MAX));
    let max_tokens = snapshot
        .effective_max_context_tokens
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let trigger_tokens = snapshot
        .trigger_tokens
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let remaining_tokens = snapshot
        .remaining_to_trigger
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));

    emitter.emit(AgentEvent::ContextWindowUpdated {
        turn: snapshot.turn,
        used_tokens,
        projected_tokens,
        max_tokens,
        trigger_tokens,
        remaining_tokens,
        source: Some(snapshot.source),
    });
}
```

- [ ] **Step 5: Verify JSONL compatibility tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_jsonl replay_accepts_old_context_window_updated_shape
cargo nextest run -p neo-agent-core --test session_jsonl replay_accepts_compaction_summary_without_new_metadata
```

Expected: both tests PASS.

## Task 7: Wire Controller Into Turn Loop and Overflow Recovery

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/turn_loop.rs`
- Modify: `crates/neo-agent-core/src/runtime/compaction_trigger.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Add runtime tests for pre-model compaction and shared snapshot events**

Add tests to `runtime_turn.rs`:

```rust
async fn collect_turn_events(
    harness: &FakeHarness,
    config: AgentConfig,
    context: &mut AgentContext,
    input: AgentMessage,
) -> Vec<AgentEvent> {
    let runtime = AgentRuntime::new(config, harness.client());
    runtime
        .run_turn(context, input)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed")
}

#[tokio::test]
async fn runtime_context_window_events_share_budget_snapshot() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart { id: "msg_1".to_owned() },
        AiStreamEvent::TextDelta { text: "done".to_owned() },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history ".repeat(4_000)));
    let config = AgentConfig::for_model(harness.model())
        .with_system_prompt("system ".repeat(1_000))
        .with_compaction(CompactionSettings::new(usize::MAX, 4));

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    ).await;

    let update = events.iter().find_map(|event| match event {
        AgentEvent::ContextWindowUpdated {
            used_tokens,
            projected_tokens,
            trigger_tokens,
            ..
        } => Some((*used_tokens, *projected_tokens, *trigger_tokens)),
        _ => None,
    }).expect("context update");
    assert!(update.0 > 0);
    assert!(update.1.is_some());
    assert!(update.2.is_some());
}

#[tokio::test]
async fn runtime_overflow_records_observed_window_and_retries_once() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::ContextOverflow { message: "too long".to_owned() })],
        vec![
            Ok(AiStreamEvent::MessageStart { id: "msg_2".to_owned() }),
            Ok(AiStreamEvent::TextDelta { text: "recovered".to_owned() }),
            Ok(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history ".repeat(20_000)));
    let config = AgentConfig::for_model(harness.model())
        .with_compaction(CompactionSettings::new(1, 1));

    let events = collect_turn_events(
        &harness,
        config.clone(),
        &mut context,
        AgentMessage::user_text("continue"),
    ).await;

    assert!(config.observed_max_context_tokens.lock().unwrap().is_some());
    assert!(events.iter().any(|event| matches!(event, AgentEvent::CompactionApplied { .. })));
    assert!(events.iter().any(|event| matches!(event, AgentEvent::TurnFinished { stop_reason: StopReason::EndTurn, .. })));
}
```

- [ ] **Step 2: Run first runtime test and confirm it fails before wiring**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_context_window_events_share_budget_snapshot
```

Expected: FAIL because old `ContextWindowUpdated` does not include snapshot fields or controller-derived projection.

- [ ] **Step 3: Add `prepare_model_request` helper**

In `turn_loop.rs`, add a helper that:

1. Builds a `ProjectionPlan` from config and context.
2. Creates a `ContextBudgetSnapshot`.
3. Calls `CompactionController::decide_before_model_call`.
4. Runs `summary::run_full_compaction` when required.
5. Rebuilds the snapshot after compaction.
6. Emits `ContextWindowUpdated`.
7. Returns the final `ProjectionPlan`.

Signature:

```rust
async fn prepare_model_request(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    pending_debt: Option<&DeferredCompaction>,
    cancel_token: &CancellationToken,
    turn: u32,
) -> Result<ProjectionPlan, AgentRuntimeError>
```

- [ ] **Step 4: Replace pre-model `maybe_compact` call**

Replace:

```rust
maybe_compact(&model, &config, emitter, &cancel_token).await;
...
let request = chat_request(&config, &emitter.context).await;
emit_effective_context_window(&config, emitter, turn).await;
```

with:

```rust
let turn = emitter.context.turns.saturating_add(1);
let projection_plan = prepare_model_request(
    &model,
    &config,
    emitter,
    pending_compaction_debt.as_ref(),
    &cancel_token,
    turn,
).await?;
pending_compaction_debt = None;
let request = chat_request(&config, &emitter.context, &projection_plan).await;
```

- [ ] **Step 5: Replace overflow recovery**

Change `recover_from_overflow` so it:

- receives the failed request snapshot or rebuilds the exact snapshot before retry
- calls `observe_context_overflow(config, snapshot.projected_tokens)`
- runs `run_full_compaction(reason = CompactionReason::OverflowRecovery, ...)`
- rebuilds a new snapshot
- retries once only when `new_snapshot.projected_tokens < new_snapshot.effective_max_context_tokens.unwrap_or(usize::MAX)`

- [ ] **Step 6: Verify runtime tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_context_window_events_share_budget_snapshot
cargo nextest run -p neo-agent-core --test runtime_turn runtime_overflow_records_observed_window_and_retries_once
```

Expected: both tests PASS.

## Task 8: Track Parallel Tool Group Compaction Debt

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/turn_loop.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Add parallel tool group tests**

Add tests to `runtime_turn.rs`:

```rust
fn large_read_call(id: &str) -> AgentToolCall {
    AgentToolCall {
        id: id.into(),
        name: "LargeTool".into(),
        raw_arguments: "{}".into(),
    }
}

#[derive(Clone)]
struct LargeTool;

impl Tool for LargeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "LargeTool".to_owned(),
            description: "returns a large payload".to_owned(),
            input_schema: serde_json::json!({"type":"object"}),
        }
    }

    fn call(&self, _input: serde_json::Value, _context: ToolContext) -> ToolFuture {
        Box::pin(async {
            Ok(ToolResult {
                content: "tool output ".repeat(20_000),
                is_error: false,
            })
        })
    }
}

#[tokio::test]
async fn runtime_does_not_compact_mid_parallel_tool_group() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart { id: "msg_1".to_owned() },
            AiStreamEvent::ToolCallStart { id: "a".to_owned(), name: "LargeTool".to_owned() },
            AiStreamEvent::ToolCallEnd { id: "a".to_owned(), raw_arguments: "{}".to_owned() },
            AiStreamEvent::ToolCallStart { id: "b".to_owned(), name: "LargeTool".to_owned() },
            AiStreamEvent::ToolCallEnd { id: "b".to_owned(), raw_arguments: "{}".to_owned() },
            AiStreamEvent::ToolCallStart { id: "c".to_owned(), name: "LargeTool".to_owned() },
            AiStreamEvent::ToolCallEnd { id: "c".to_owned(), raw_arguments: "{}".to_owned() },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart { id: "msg_2".to_owned() },
            AiStreamEvent::TextDelta { text: "after tools".to_owned() },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(LargeTool));
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Parallel)
        .with_compaction(CompactionSettings::new(1, 1));
    let runtime = AgentRuntime::new(config, harness.client()).with_tools(Arc::new(registry));
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("use tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let first_tool_result = events.iter().position(|event| matches!(
        event,
        AgentEvent::MessageAppended { message: AgentMessage::ToolResult { .. } }
    )).expect("tool result");
    let first_compaction = events.iter().position(|event| matches!(
        event,
        AgentEvent::CompactionApplied { .. }
    )).expect("compaction");
    assert!(first_compaction > first_tool_result);
}

#[tokio::test]
async fn runtime_compacts_after_parallel_tool_group_before_followup() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart { id: "msg_1".to_owned() },
            AiStreamEvent::ToolCallStart { id: "a".to_owned(), name: "LargeTool".to_owned() },
            AiStreamEvent::ToolCallEnd { id: "a".to_owned(), raw_arguments: "{}".to_owned() },
            AiStreamEvent::ToolCallStart { id: "b".to_owned(), name: "LargeTool".to_owned() },
            AiStreamEvent::ToolCallEnd { id: "b".to_owned(), raw_arguments: "{}".to_owned() },
            AiStreamEvent::ToolCallStart { id: "c".to_owned(), name: "LargeTool".to_owned() },
            AiStreamEvent::ToolCallEnd { id: "c".to_owned(), raw_arguments: "{}".to_owned() },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart { id: "msg_2".to_owned() },
            AiStreamEvent::TextDelta { text: "after compaction".to_owned() },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(LargeTool));
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Parallel)
        .with_compaction(CompactionSettings::new(1, 1));
    let runtime = AgentRuntime::new(config, harness.client()).with_tools(Arc::new(registry));
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("use tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let compaction = events.iter().position(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
        .expect("compaction");
    let second_model_text = events.iter().rposition(|event| matches!(
        event,
        AgentEvent::MessageAppended { message: AgentMessage::Assistant { .. } }
    )).expect("assistant");
    assert!(compaction < second_model_text);
}
```

- [ ] **Step 2: Run the first tool group test and confirm it fails before debt wiring**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_does_not_compact_mid_parallel_tool_group
```

Expected: FAIL because there is no explicit debt event/order guarantee yet.

- [ ] **Step 3: Add `pending_compaction_debt` to the turn loop**

In `run_agent_turn`, create:

```rust
let mut pending_compaction_debt: Option<DeferredCompaction> = None;
```

After `append_tool_result_messages`, build a `ToolGroupBudgetState`, refresh the budget after each appended result, and set `pending_compaction_debt` when the group observes threshold crossing. Emit `ContextCompactionDeferred` when the first debt is recorded.

- [ ] **Step 4: Block pending follow-up until debt is cleared**

Before calling `next_pending_after_assistant`, ensure the next loop iteration calls `prepare_model_request` with `pending_compaction_debt.as_ref()`. Do not drain follow-up or goal continuation into a model request before `prepare_model_request` has run.

- [ ] **Step 5: Verify tool group tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_does_not_compact_mid_parallel_tool_group
cargo nextest run -p neo-agent-core --test runtime_turn runtime_compacts_after_parallel_tool_group_before_followup
```

Expected: both tests PASS.

## Task 9: Delete Old Orchestration Paths

**Files:**
- Delete: `crates/neo-agent-core/src/runtime/compaction_trigger.rs`
- Delete: `crates/neo-agent-core/src/compaction/micro.rs`
- Modify: `crates/neo-agent-core/src/runtime/mod.rs`
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`
- Modify: `crates/neo-agent-core/src/runtime/agent.rs`

- [ ] **Step 1: Remove old module exports and imports**

Remove from `runtime/mod.rs`:

```rust
mod compaction_trigger;
```

Remove from `compaction/mod.rs`:

```rust
pub mod micro;
```

- [ ] **Step 2: Remove old functions and call sites**

Delete:

- `runtime/compaction_trigger.rs::maybe_compact`
- `compaction/mod.rs::run_compaction`

Update `runtime/agent.rs` manual compaction turn to call the new `summary::run_full_compaction` path directly through the same controller decision code used by `turn_loop`.

- [ ] **Step 3: Search for stale references**

Run:

```bash
rg -n "maybe_compact|run_compaction|apply_micro_compaction|MicroCompactionConfig|compaction_trigger|micro::" crates/neo-agent-core crates/neo-agent crates/neo-tui
```

Expected: no production references. Test names may mention migration only if they assert absence; prefer no references.

- [ ] **Step 4: Verify deleted-boundary compile smoke with a narrow existing test**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_context_window_events_share_budget_snapshot
```

Expected: PASS.

## Task 10: Replay and Resume Robustness

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/context.rs`
- Modify: `crates/neo-agent-core/tests/session_jsonl.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Add replay tests**

Add to `session_jsonl.rs`:

```rust
#[test]
fn replay_ignores_old_context_window_event_for_authority() {
    let events = vec![
        AgentEvent::MessageAppended { message: AgentMessage::user_text("real history ".repeat(1_000)) },
        AgentEvent::ContextWindowUpdated {
            turn: 1,
            used_tokens: 1,
            projected_tokens: Some(1),
            max_tokens: Some(1_000_000),
            trigger_tokens: Some(800_000),
            remaining_tokens: Some(799_999),
            source: Some(ContextWindowSource::Configured),
        },
    ];

    let context = AgentContext::from_replay(&events);

    assert!(context.estimated_tokens() > 1);
}

#[test]
fn replay_drops_incomplete_trailing_tool_exchange_before_budgeting() {
    let events = vec![
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                Vec::new(),
                vec![
                    AgentToolCall { id: "a".into(), name: "Read".into(), raw_arguments: "{}".into() },
                    AgentToolCall { id: "b".into(), name: "Read".into(), raw_arguments: "{}".into() },
                ],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result("a", "Read", vec![Content::text("done")], false),
        },
    ];

    let context = AgentContext::from_replay(&events);

    assert!(context.messages().is_empty());
}
```

- [ ] **Step 2: Run replay tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_jsonl replay_ignores_old_context_window_event_for_authority
cargo nextest run -p neo-agent-core --test session_jsonl replay_drops_incomplete_trailing_tool_exchange_before_budgeting
```

Expected: both tests PASS. The second may already pass; keep it as a regression guard for the new budget-before-resume behavior.

- [ ] **Step 3: Add resume compaction test**

Add to `runtime_turn.rs`:

```rust
#[tokio::test]
async fn runtime_compacts_before_model_call_when_resume_exceeds_window() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart { id: "msg_1".to_owned() },
        AiStreamEvent::TextDelta { text: "resumed".to_owned() },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history ".repeat(40_000)));
    let mut config = AgentConfig::for_model(harness.model())
        .with_compaction(CompactionSettings::new(1, 1));
    config.model.capabilities.max_context_tokens = Some(8_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    ).await;

    let compaction = events.iter().position(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
        .expect("compaction");
    let assistant = events.iter().position(|event| matches!(
        event,
        AgentEvent::MessageAppended { message: AgentMessage::Assistant { .. } }
    )).expect("assistant");
    assert!(compaction < assistant);
}
```

- [ ] **Step 4: Verify resume compaction test**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_compacts_before_model_call_when_resume_exceeds_window
```

Expected: PASS.

## Task 11: TUI Context Display Migration

**Files:**
- Modify: `crates/neo-tui/src/shell/context.rs`
- Modify: `crates/neo-tui/src/shell/event_router.rs`
- Modify: `crates/neo-tui/tests/app_shell.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`

- [ ] **Step 1: Add TUI tests**

Add to `app_shell.rs`:

```rust
#[test]
fn footer_renders_projected_context_when_available() {
    let mut app = test_app();
    app.apply_agent_event(neo_agent_core::AgentEvent::ContextWindowUpdated {
        turn: 1,
        used_tokens: 72_000,
        projected_tokens: Some(43_000),
        max_tokens: Some(64_000),
        trigger_tokens: Some(51_200),
        remaining_tokens: Some(8_200),
        source: Some(neo_agent_core::ContextWindowSource::Configured),
    });

    assert_eq!(app.context_window_label(), Some("ctx 43k/64k".to_owned()));
}

#[test]
fn footer_falls_back_to_used_tokens_for_old_events() {
    let mut app = test_app();
    app.apply_agent_event(neo_agent_core::AgentEvent::ContextWindowUpdated {
        turn: 1,
        used_tokens: 12_345,
        projected_tokens: None,
        max_tokens: Some(200_000),
        trigger_tokens: None,
        remaining_tokens: None,
        source: None,
    });

    assert_eq!(app.context_window_label(), Some("ctx 12k/200k".to_owned()));
}
```

- [ ] **Step 2: Run TUI fallback test**

Run:

```bash
cargo nextest run -p neo-tui --test app_shell footer_falls_back_to_used_tokens_for_old_events
```

Expected: PASS before and after migration if event constructors are updated correctly.

- [ ] **Step 3: Extend `ContextWindow`**

In `context.rs`, add fields:

```rust
used_tokens: Option<u32>,
projected_tokens: Option<u32>,
max_tokens: Option<u32>,
trigger_tokens: Option<u32>,
source: Option<ContextWindowSource>,
```

Make `ContextWindow::label()` prefer `projected_tokens` when present, then fall back to `used_tokens`.

- [ ] **Step 4: Stop interactive mode from seeding raw durable usage**

In `crates/neo-agent/src/modes/interactive/mod.rs`, remove use of:

```rust
context.estimated_context_tokens()
```

for footer context usage. Initial footer should show max window only until the first runtime `ContextWindowUpdated` snapshot arrives.

- [ ] **Step 5: Verify TUI tests**

Run:

```bash
cargo nextest run -p neo-tui --test app_shell footer_renders_projected_context_when_available
cargo nextest run -p neo-tui --test app_shell footer_falls_back_to_used_tokens_for_old_events
```

Expected: both tests PASS.

## Task 12: Final Narrow Verification

**Files:**
- No new files.

- [ ] **Step 1: Scan for forbidden old paths**

Run:

```bash
rg -n "maybe_compact|run_compaction|apply_micro_compaction|MicroCompactionConfig|compaction_trigger|micro::" crates/neo-agent-core crates/neo-agent crates/neo-tui
```

Expected: no matches.

- [ ] **Step 2: Verify core budget/controller/projection tests**

Run:

```bash
cargo nextest run -p neo-agent-core --lib budget_includes_system_workspace_transform_and_tools
cargo nextest run -p neo-agent-core --lib decision_uses_deferred_tool_group_debt_before_next_model_call
cargo nextest run -p neo-agent-core --lib request_projection_truncates_old_large_tool_results
```

Expected: all PASS.

- [ ] **Step 3: Verify runtime edge tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_compacts_after_parallel_tool_group_before_followup
cargo nextest run -p neo-agent-core --test runtime_turn runtime_overflow_records_observed_window_and_retries_once
cargo nextest run -p neo-agent-core --test runtime_turn runtime_compacts_before_model_call_when_resume_exceeds_window
```

Expected: all PASS.

- [ ] **Step 4: Verify JSONL and TUI compatibility**

Run:

```bash
cargo nextest run -p neo-agent-core --test session_jsonl replay_accepts_old_context_window_updated_shape
cargo nextest run -p neo-tui --test app_shell footer_falls_back_to_used_tokens_for_old_events
```

Expected: both PASS.

## Acceptance Criteria

- `ContextWindowUpdated`, compaction decisions, overflow recovery, and request projection all originate from `ContextBudgetSnapshot`.
- `chat_request` applies a supplied projection plan and does not decide projection on its own.
- Full compaction uses summary-time projection before the summary LLM call.
- Parallel tool groups never full compact mid-group and always clear compaction debt before the next model call.
- Overflow recovery records observed overflow from projected/effective snapshot tokens and retries once.
- JSONL replay ignores historical budget events as authority and accepts old event shapes.
- TUI footer prefers projected token usage when present and falls back for old events.
- `runtime/compaction_trigger.rs::maybe_compact`, `compaction/mod.rs::run_compaction`, and `compaction/micro.rs` are gone.
