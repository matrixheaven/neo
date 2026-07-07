# Neo 本地上下文管理重构设计

## 背景

Neo 当前的上下文管理已经具备本地 LLM summary 压缩、micro compaction、overflow recovery、safe tool-exchange boundary 等能力，但这些能力分散在 `runtime/compaction_trigger.rs`、`compaction/mod.rs`、`compaction/micro.rs`、`runtime/tokens.rs`、`runtime/chat_request.rs` 和 `runtime/context.rs` 中。

本设计的目标不是复制 Codex 的 provider remote compaction，也不引入模型主动 `NewContextWindow` 工具。Neo 要做的是一套 runtime-only、local-only、provider-neutral 的上下文管理系统，提升超长工作会话的健壮性和可解释性。

## 目标

- 所有 ctx 显示、压缩触发、overflow recovery、request projection 使用同一个预算快照。
- full compaction 只做本地 summary，不做 provider remote。
- 模型不能主动请求压缩或新窗口；上下文治理只由 Neo runtime 决策。
- parallel tool group 作为原子边界，不在中间 full compact。
- micro compaction 明确定义为 request-time projection，不修改 durable history。
- 删除或迁移旧 compaction orchestration 路径，不保留两套逻辑。
- resume/replay 后根据当前 durable messages 和当前模型配置重新计算预算。

## 非目标

- 不做 provider remote compaction。
- 不做模型可调用的 `NewContextWindow`。
- 不做模型可调用的 `GetContextRemaining`。
- 不复制 Codex 的 window UUID 世代体系。
- 不引入 Codex 式 hooks/analytics 大体系。
- 不把完整 `ContextBudgetSnapshot` 写入 JSONL 作为 replay 权威状态。

## 推荐路线

采用轻量窗口状态路线。

```text
┌──────────────────────────────────────────────┐
│ AgentContext                                 │
│ - durable messages                           │
│ - compaction_summary                         │
│ - turns / queues / todos                     │
│                                              │
│ 不负责“什么时候该压缩”                       │
└──────────────────────┬───────────────────────┘
                       │ messages changed
                       ▼
┌──────────────────────────────────────────────┐
│ ContextBudget / ContextWindowState           │
│ - estimated_effective_tokens                 │
│ - model_max_tokens                           │
│ - trigger_threshold                          │
│ - reserved_headroom                          │
│ - observed_overflow_limit                    │
│ - micro_projected_tokens                     │
│ - last_compaction_result                     │
└──────────────────────┬───────────────────────┘
                       │ decide()
                       ▼
┌──────────────────────────────────────────────┐
│ CompactionController                         │
│ - NoAction                                   │
│ - ApplyMicroProjection                       │
│ - RunFullCompaction                          │
│ - ForceFullCompactionAfterOverflow           │
│ - StopWithContextError                       │
└──────────────────────┬───────────────────────┘
                       │
           ┌───────────┴───────────┐
           ▼                       ▼
┌──────────────────────┐  ┌──────────────────────┐
│ MicroProjection       │  │ LocalSummaryCompact  │
│ - 仅影响 ChatRequest  │  │ - LLM summary         │
│ - 不改 JSONL          │  │ - 改 AgentContext     │
└──────────────────────┘  └──────────────────────┘
```

## 核心数据流

### 模型请求前

```text
run_agent_turn loop
    │
    ▼
append queued / steering / follow-up messages
    │
    ▼
ContextBudget::snapshot(context, config)
    │
    ├─ durable_tokens
    ├─ fixed_request_overhead
    ├─ projected_tokens
    ├─ effective_max_tokens
    └─ thresholds
    │
    ▼
CompactionController::decide(snapshot)
    │
    ├─ NoAction
    ├─ ApplyMicroProjectionOnly
    ├─ RunFullCompaction
    ├─ ForceFullCompactionAfterOverflow
    └─ StopWithContextError
    │
    ▼
如果需要 full compaction:
    run local summary compaction
    AgentContext.apply_compaction()
    重新生成 ContextBudget snapshot
    │
    ▼
chat_request(context, projection_plan)
    │
    ▼
emit ContextWindowUpdated(snapshot)
    │
    ▼
model.stream_chat()
```

`chat_request()` 不再自己决定是否 micro compact。它接收 `ProjectionPlan`，执行已经由 budget/controller 决定好的 request projection。

### Tool call 后

```text
assistant emits tool calls
    │
    ▼
execute_tool_calls()
    │
    ▼
append ToolResult messages 到 AgentContext
    │
    ▼
ContextBudget::snapshot()
    │
    ▼
emit ContextWindowUpdated()
    │
    ▼
如果本 turn 还要继续模型调用:
    回到 loop 顶部
    再次 decide()
```

tool group 并行执行期间不 full compact。tool results 全部 append 后立刻刷新预算；下一次模型调用前必须重新决策。

## 触发规则

触发规则按优先级执行。

1. Hard overflow recovery
   - provider 返回 `ContextOverflow` 后，记录 `observed_overflow_limit = estimated_at_overflow * 0.85`。
   - 下一次强制 full compaction。

2. Request cannot fit
   - 如果 `projected_tokens >= effective_max_tokens`，必须 full compact。
   - 判断使用 projected tokens，因为真正发给模型的是 projected request。

3. Reserved headroom trigger
   - 如果 `projected_tokens + reserved_context_tokens >= effective_max_tokens`，full compact。
   - 这避免小窗口模型到最后几千 token 才压缩。

4. Ratio trigger
   - 如果 `projected_tokens >= effective_max_tokens * trigger_ratio`，full compact。
   - 建议默认：
     - `<= 128k`: `0.8`
     - `> 128k`: `0.85`

5. Absolute max trigger
   - 如果 `projected_tokens >= runtime.compaction.max_estimated_tokens`，full compact。
   - 用于无窗口模型或模型配置缺失时的兜底。

6. Micro projection trigger
   - 如果 raw durable tokens 很大，但 projected tokens 低于 full compaction 阈值，则只启用 projection，不 full compact。

## Parallel Tool Group 规则

parallel tool group 是不可中途 full compact 的事务边界。

```text
assistant emits 3 tool calls
        │
        ▼
ToolGroupBudgetState starts
        │
        ├─ tool #1 finished
        │     ├─ append / stage result
        │     ├─ refresh budget
        │     └─ 如果过阈值: mark deferred_full_compaction = true
        │
        ├─ tool #2 finished
        │     ├─ append / stage result
        │     ├─ refresh budget
        │     └─ 如果过 hard limit: mark urgent_compaction = true
        │
        └─ tool #3 finished
              ├─ append / stage result
              └─ close tool exchange boundary
        │
        ▼
现在 tool exchange 完整
        │
        ▼
如果 deferred_full_compaction / urgent_compaction:
        立刻 full compaction
        然后才允许下一次 model call
```

规则：

- group 内不 full compact。
- 每个 tool result 完成后刷新 budget。
- 达到 soft threshold 时记录 deferred compaction debt。
- 达到 hard max 时升级为 urgent compaction debt。
- 默认不取消剩余工具。
- group 完整闭合后，如果存在 debt，下一次 model call 前必须清债。
- 如果 full compaction 失败，不继续发下一次 model call。

## Micro Projection

当前 `compaction/micro.rs` 的行为本质上不是 durable compaction，而是 request-time projection：构造 `ChatRequest` 时替换旧的大型 tool result 内容，但不修改 `AgentContext.messages`，也不写 JSONL。

本设计保留这个能力，但改清楚边界。

```text
AgentContext.messages
  仍然完整保存旧 tool result
        │
        ▼
chat_request()
  临时复制 messages
        │
        ▼
ProjectionPlan
  旧的大 tool result 内容替换成 marker
        │
        ▼
ChatRequest.messages
  发给模型的是变小后的版本
```

Projection 分两类：

- request-time projection：普通模型请求前使用。
- summary-time projection：full compaction summary 请求前使用，避免“为了压缩而再次溢出”。

Projection 不修改 durable history。

## 模块和 API 设计

### 新模块

```text
crates/neo-agent-core/src/runtime/context_budget.rs
crates/neo-agent-core/src/runtime/compaction_controller.rs
crates/neo-agent-core/src/compaction/projection.rs
crates/neo-agent-core/src/compaction/summary.rs
```

目标结构：

```text
runtime/
  context.rs
    AgentContext
    durable conversation state only

  context_budget.rs
    ContextBudgetSnapshot
    ContextBudgetEstimator
    ContextThresholds
    ContextWindowSource

  compaction_controller.rs
    CompactionController
    CompactionDecision
    CompactionDebt
    ToolGroupBudgetState

  turn_loop.rs
    orchestration only
    no token math

compaction/
  mod.rs
    public compaction types + safe boundary helpers
    no full runtime orchestration

  summary.rs
    generate_compaction_summary
    generate_with_retry
    summary-time projection integration

  projection.rs
    request-time projection
    summary-time projection
    tool result truncation helpers

  compaction_instruction.md
```

### `ContextBudgetSnapshot`

```rust
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
```

### `ProjectionPlan`

```rust
pub struct ProjectionPlan {
    pub enabled: bool,
    pub cutoff_index: usize,
    pub min_tool_result_tokens: usize,
    pub keep_recent_messages: usize,
    pub mode: ProjectionMode,
}

pub enum ProjectionMode {
    None,
    Request,
    SummaryInput,
}

pub struct ProjectionResult {
    pub messages: Vec<AgentMessage>,
    pub omitted_tokens: usize,
    pub projected_tokens: usize,
}
```

Entrypoints:

```rust
pub fn project_for_request(
    messages: &[AgentMessage],
    plan: &ProjectionPlan,
) -> ProjectionResult;

pub fn project_for_summary(
    messages: &[AgentMessage],
    plan: &ProjectionPlan,
) -> ProjectionResult;
```

### `CompactionDecision`

```rust
pub enum CompactionDecision {
    NoAction {
        snapshot: ContextBudgetSnapshot,
    },

    UseProjectionOnly {
        snapshot: ContextBudgetSnapshot,
        plan: ProjectionPlan,
    },

    RunFullCompaction {
        snapshot: ContextBudgetSnapshot,
        reason: CompactionReason,
        urgency: CompactionUrgency,
    },

    ForceAfterOverflow {
        snapshot: ContextBudgetSnapshot,
        observed_limit: usize,
    },

    StopWithContextError {
        snapshot: ContextBudgetSnapshot,
        message: String,
    },
}

pub enum CompactionUrgency {
    Normal,
    DeferredAfterToolGroup,
    UrgentBeforeNextModelCall,
}
```

### `ToolGroupBudgetState`

```rust
pub struct ToolGroupBudgetState {
    pub turn: u32,
    pub total_calls: usize,
    pub completed_calls: usize,

    pub latest_snapshot: ContextBudgetSnapshot,
    pub deferred_compaction: Option<DeferredCompaction>,
}

pub struct DeferredCompaction {
    pub reason: CompactionReason,
    pub urgency: CompactionUrgency,
    pub first_triggered_after_call_index: usize,
    pub projected_tokens_at_trigger: usize,
}
```

### `CompactionController`

```rust
pub struct CompactionController;

impl CompactionController {
    pub fn decide_before_model_call(
        context: &AgentContext,
        config: &AgentConfig,
        pending_debt: Option<&DeferredCompaction>,
    ) -> CompactionDecision;

    pub fn decide_after_tool_result(
        context: &AgentContext,
        config: &AgentConfig,
        tool_group: &mut ToolGroupBudgetState,
    ) -> Option<DeferredCompaction>;

    pub fn observe_overflow(
        config: &AgentConfig,
        snapshot: &ContextBudgetSnapshot,
    ) -> usize;
}
```

`decide_*` 只做纯判断，不做 async，不调用模型。

### `run_full_compaction`

```rust
pub async fn run_full_compaction(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    context: &mut AgentContext,
    reason: CompactionReason,
    snapshot: ContextBudgetSnapshot,
    cancel_token: &CancellationToken,
    emit: &mut EventEmitter,
) -> Result<CompactionOutcome, CompactionError>;

pub struct CompactionOutcome {
    pub compacted_message_count: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub summary_tokens: usize,
    pub projection_omitted_tokens: usize,
}
```

## ChatRequest API 改造

当前：

```rust
chat_request(config, context).await
```

建议：

```rust
chat_request(config, context, projection_plan).await
```

`chat_request` 执行 projection，不做 projection 决策。

## 事件设计

迁移 `ContextWindowUpdated`：

```rust
ContextWindowUpdated {
    turn: u32,
    used_tokens: usize,

    #[serde(default)]
    projected_tokens: Option<usize>,

    #[serde(default)]
    max_tokens: Option<usize>,

    #[serde(default)]
    trigger_tokens: Option<usize>,

    #[serde(default)]
    remaining_tokens: Option<usize>,

    #[serde(default)]
    source: Option<ContextWindowSource>,
}
```

新增可选事件：

```rust
ContextCompactionDeferred {
    turn: u32,
    reason: CompactionReason,
    urgency: CompactionUrgency,
    projected_tokens: usize,
    max_tokens: Option<usize>,
}
```

TUI 默认仍显示简单形式：

```text
ctx 43k/64k
```

debug/status 可显示：

```text
ctx 43k/64k · projected 31k · compact at 51k · overflow-adjusted
```

tool group 中途过阈值：

```text
ctx 67k/64k · compact required after tool group
```

## 错误恢复

### 错误分类

```rust
pub enum ContextManagementError {
    ProviderContextOverflow {
        snapshot: ContextBudgetSnapshot,
        provider_message: String,
    },

    CompactionNoSafeBoundary {
        snapshot: ContextBudgetSnapshot,
    },

    CompactionSummaryFailed {
        snapshot: ContextBudgetSnapshot,
        error: CompactionError,
    },

    CompactionInsufficientReduction {
        before: ContextBudgetSnapshot,
        after: ContextBudgetSnapshot,
    },

    ProjectionInvariantViolation {
        message: String,
    },
}
```

### Provider overflow

```text
model call returns ContextOverflow
    │
    ▼
record observed_overflow_limit
    effective_max = min(configured_max, observed * 0.85)
    │
    ▼
build fresh ContextBudgetSnapshot
    │
    ▼
run_full_compaction(reason = OverflowRecovery)
    │
    ▼
build snapshot again
    │
    ├─ if projected_tokens < effective_max:
    │     retry model call once
    │
    └─ else:
          stop with ContextManagementError::CompactionInsufficientReduction
```

Overflow 后最多自动 retry 一次 model call。

### Full compaction 失败

```text
generate summary failed
    │
    ├─ retryable LLM error:
    │     shrink prefix -> retry
    │
    ├─ empty / truncated summary:
    │     shrink prefix -> retry
    │
    └─ no smaller safe boundary:
          fail hard
```

每次 retry 后都必须重新应用 summary-time projection。

## Resume / Replay

Replay 后必须重建预算，不信任旧预算事件。

```text
load JSONL events
    │
    ▼
AgentContext::from_replay()
    │
    ├─ replay MessageAppended
    ├─ replay CompactionApplied
    ├─ sanitize incomplete tool exchange
    └─ recompute estimated_tokens
    │
    ▼
ContextBudget::snapshot(context, current_config)
    │
    ▼
if snapshot requires compaction:
    compact before first resumed model call
```

如果 JSONL 末尾是未完成 tool exchange：

```text
Assistant(tool_calls=[a,b,c])
ToolResult(a)
<session ended/crashed>
```

resume 后：

- `sanitize_tool_exchange_messages` 去掉不完整尾部，保证 provider-valid。
- 不尝试继续执行剩余 tool call。

## JSONL 兼容

### `ContextWindowUpdated`

新增字段必须带 `serde(default)`，旧 session 可 parse。

### `CompactionSummary`

可增加诊断 metadata：

```rust
pub struct CompactionSummary {
    pub summary: String,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub first_kept_message_index: usize,

    #[serde(default)]
    pub projected_tokens_before: Option<usize>,

    #[serde(default)]
    pub projected_tokens_after: Option<usize>,

    #[serde(default)]
    pub projection_omitted_tokens: Option<usize>,

    #[serde(default)]
    pub reason: Option<CompactionReason>,
}
```

Replay 仍只依赖 `summary` 和 `first_kept_message_index`。

### 不写完整 snapshot

不要把完整 `ContextBudgetSnapshot` 写入 JSONL。

原因：

- 容易和当前 config/model 脱节。
- 字段会演化。
- replay 可能误用旧预算。
- JSONL 会膨胀。

## 要删除或迁移的旧边界

### 删除 `compaction/mod.rs::run_compaction`

这是旧 orchestration 路径。它和 `runtime/compaction_trigger.rs::maybe_compact` 语义重复，而且 token 口径不同。迁移有用 helper 后删除。

### 拆除 `runtime/compaction_trigger.rs::maybe_compact`

迁移成：

- `CompactionController::decide_before_model_call`
- `run_full_compaction`
- `apply_compaction_outcome`

迁移后删除旧函数名。

### 改造 `chat_request.rs` 内部 micro 调用

`chat_request` 不再直接调用 `micro::apply_micro_compaction()` 做决策。它接收 `ProjectionPlan` 并执行投影。

### 降级 `AgentContext::estimated_context_tokens()`

该 API 只表示 raw durable estimate，不能作为 UI/footer/trigger 来源。UI 和 trigger 只能读 `ContextBudgetSnapshot`。

### 拆分 `CompactionSettings`

runtime 内部拆分：

```rust
pub struct CompactionSettings {
    pub full: FullCompactionSettings,
    pub projection: ProjectionSettings,
    pub budget: ContextBudgetSettings,
}
```

配置文件表面可以暂时保持扁平，但 runtime 内部必须分组。

## 测试矩阵

### ContextBudget 单元测试

目标模块：

```text
crates/neo-agent-core/src/runtime/context_budget.rs
```

| 测试 | 覆盖点 |
|---|---|
| `budget_includes_system_workspace_transform_and_tools` | fixed overhead 不漏算 |
| `budget_uses_observed_overflow_when_smaller` | observed limit 降窗 |
| `budget_reports_remaining_to_trigger_and_max` | remaining 数值稳定 |
| `budget_uses_projected_tokens_for_decision` | projection 参与判定 |
| `small_window_uses_lower_trigger_ratio` | 小窗口更早压缩 |

### Projection 单元测试

目标模块：

```text
crates/neo-agent-core/src/compaction/projection.rs
```

| 测试 | 覆盖点 |
|---|---|
| `request_projection_truncates_old_large_tool_results` | 旧大 tool result 被 marker 替换 |
| `projection_preserves_recent_tool_results` | 最近 N 条不截断 |
| `projection_never_changes_user_or_assistant_messages` | 不碰非 tool result |
| `summary_projection_can_be_more_aggressive_than_request_projection` | summary 输入可更激进 |
| `projection_marker_includes_omitted_token_estimate` | marker 告知省略规模 |
| `projection_preserves_tool_result_pairing` | 不破坏 tool exchange |

### CompactionController 单元测试

目标模块：

```text
crates/neo-agent-core/src/runtime/compaction_controller.rs
```

| 测试 | 覆盖点 |
|---|---|
| `decision_no_action_below_threshold` | 正常低水位 |
| `decision_runs_full_compaction_at_ratio_threshold` | ratio 触发 |
| `decision_runs_full_compaction_at_reserved_headroom` | reserved 触发 |
| `decision_forces_after_overflow` | overflow recovery |
| `decision_stops_when_no_safe_boundary_after_required_compaction` | 必须压缩但无边界 |
| `decision_uses_deferred_tool_group_debt_before_next_model_call` | tool group debt 优先 |

### ToolGroupBudgetState 测试

| 测试 | 覆盖点 |
|---|---|
| `tool_group_records_deferred_debt_after_first_result_crosses_threshold` | 第 1 个 result 过 soft 阈值 |
| `tool_group_upgrades_to_urgent_when_hard_limit_crossed` | 第 2 个 result 过 hard max |
| `tool_group_does_not_compact_until_all_results_finish` | group 原子边界 |
| `tool_group_debt_survives_finish` | finish 后返回 debt |
| `tool_group_debt_blocks_pending_followup_until_compacted` | pending follow-up 不插队 |

### Runtime integration 测试

目标文件：

```text
crates/neo-agent-core/tests/runtime_turn.rs
```

| 测试 | 覆盖点 |
|---|---|
| `runtime_compacts_before_model_call_when_resume_exceeds_window` | resume 后第一轮先 compact |
| `runtime_compacts_after_parallel_tool_group_before_followup` | tool group 完成后先 compact |
| `runtime_does_not_compact_mid_parallel_tool_group` | 不拆 tool exchange |
| `runtime_overflow_records_observed_window_and_retries_once` | overflow recovery |
| `runtime_stops_when_compaction_cannot_reduce_below_effective_window` | 防无限 retry |
| `runtime_context_window_events_share_budget_snapshot` | footer/trigger/request 同源 |

### JSONL replay 测试

| 测试 | 覆盖点 |
|---|---|
| `replay_ignores_old_context_window_event_for_authority` | 不信旧 budget event |
| `replay_recomputes_budget_from_messages_and_current_model` | 换小窗口后重新判定 |
| `replay_accepts_old_context_window_updated_shape` | 旧 JSONL 兼容 |
| `replay_accepts_compaction_summary_without_new_metadata` | 旧 CompactionSummary 兼容 |
| `replay_drops_incomplete_trailing_tool_exchange_before_budgeting` | provider-valid |

### TUI 测试

目标文件：

```text
crates/neo-tui/tests/app_shell.rs
crates/neo-tui/tests/tool_cards.rs
```

| 测试 | 覆盖点 |
|---|---|
| `footer_renders_projected_context_when_available` | projected 显示 |
| `footer_falls_back_to_used_tokens_for_old_events` | 旧事件兼容 |
| `footer_shows_compaction_queued_after_tool_group` | deferred debt 文案 |
| `footer_shows_overflow_adjusted_source_in_debug_status` | source 可见 |
| `compaction_card_shows_projection_omitted_tokens_when_present` | 压缩卡片诊断信息 |

## 不变量

1. 任何发给 provider 的请求都必须 provider-valid。
2. full compaction 不允许切开未完成 tool exchange。
3. parallel tool group 中途不 full compact。
4. 下一次 model call 前必须清理 urgent compaction debt。
5. JSONL replay 的权威状态来自 durable messages，不来自历史 budget events。
6. projection 永远不修改 durable context。
7. overflow recovery 最多 retry 一次 model call。
8. ctx footer、trigger、request projection 必须来自同一个 budget snapshot。
9. 旧 compaction orchestration 路径必须删除或迁移，不能双路径并存。

## 实施约束

- 每个阶段只改一个清晰边界。
- 先引入纯计算模块和测试，再接入 runtime。
- 不保留旧 runtime 路径作为兼容 fallback。
- 不做 broad test 作为证据；每个行为用精确测试覆盖。
- 不做 provider remote。
- 不做模型主动上下文管理工具。
