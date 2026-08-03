# Thinking and Message Presentation - Intent

## TaskIntentDraft

- Requested outcome: 固化 ThinkingKind、摘要正文、完整思考、过程回复和最终回复的统一设计，并交接给后续实现者
- Goal: 让后续实现严格区分提供方思考、过程回复和最终回答，且保留摘要正文
- Success evidence:
  - 设计规格、实施计划、交接说明、工作记录和索引均自洽。
  - 规格完整覆盖摘要正文、完整思考、未知思考、过程回复和最终回答。
  - 当前源代码未被本切片修改，已有用户改动保持原样。
- Stop condition: 文档通过自审、工作区结构检查和差异检查后提交；源代码实现留给后续计划
- Non-goals:
- 本切片不改源代码、不增加新的思考枚举、不重写 provider runtime、不修改 Delegate-family 卡片
- Scope: neo-ai provider events, neo-agent-core normalized events, neo-tui transcript rendering, persistence and focused tests
- Change kinds:
- architecture, contract, documentation
- Risk hints:
- 不要把 OpenAI 摘要当成隐藏思考；不要凭模型名称猜测类型；不要丢弃摘要正文；不要改变 Delegate-family 卡片

## BaselineReadSetHint

- AGENTS.md; crates/neo-ai/src/types.rs; crates/neo-ai/src/providers/openai/responses.rs; crates/neo-agent-core/src/runtime/stream_aggregator.rs; crates/neo-tui/src/transcript/entry/render_thinking.rs; .references/codex/codex-rs/tui/src/chatwidget/streaming.rs

## BaselineUsageDraft

- Required baseline refs:
- AGENTS.md; crates/neo-ai/src/types.rs; crates/neo-ai/src/providers/openai/responses.rs; crates/neo-agent-core/src/runtime/stream_aggregator.rs; crates/neo-tui/src/transcript/entry/render_thinking.rs; .references/codex/codex-rs/tui/src/chatwidget/streaming.rs
- Acknowledged before plan:
  - AGENTS.md and project rules
  - current `ThinkingKind` provider mappings and transcript renderer
  - Codex reference reasoning summary and message-phase behavior
- Cited in plan:
  - `crates/neo-ai/src/types.rs`
  - `crates/neo-ai/src/providers/openai/responses.rs`
  - `crates/neo-agent-core/src/runtime/stream_aggregator.rs`
  - `crates/neo-tui/src/transcript/entry/render_thinking.rs`
  - `.references/codex/codex-rs/tui/src/chatwidget/streaming.rs`
- Missing refs:
  - exact provider message-phase wire availability, to verify during source implementation
- Advisory decision: baseline-readback-complete; implementation must preserve `Unknown` when wire phase is absent

## ImpactStatementDraft

- Compatibility boundary: 保留现有 ThinkingKind、旧 ThinkingStarted 序列化默认值、未知消息阶段的现有正文行为和 Delegate-family 卡片
- Affected layers:
- provider normalization, agent events, transcript store, thinking renderer, assistant message renderer, focused regressions
- Owners:
- existing provider adapters, ModelTurnState, AgentEvent, TranscriptStore and TranscriptEntry rendering
- Invariants:
- 原始事件和会话记录追加保留；显示层可去重但不得改写；最终回复不得与过程回复或思考块合并
- Non-goals:
- 本切片不改源代码、不增加新的思考枚举、不重写 provider runtime、不修改 Delegate-family 卡片


## Implementation Continuation

The documentation handoff is now being executed in the existing workspace. The
implementation route is subagent-driven with sequential bounded slices and
spec-compliance plus code-quality review after each slice. This continuation
must preserve the original intent, scope, compatibility, and retirement locks;
it must not reinterpret the earlier documentation-only stop condition as a
source-code non-goal.


## BaselineUsageDraft

- Required baseline refs:
- AGENTS.md
- crates/neo-ai/src/types.rs
- crates/neo-ai/src/providers/openai/responses.rs
- crates/neo-agent-core/src/runtime/stream_aggregator.rs
- crates/neo-tui/src/transcript/entry/render_thinking.rs
- .references/codex/codex-rs/tui/src/chatwidget/streaming.rs
- Delivered context refs:
- none
- Acknowledged before plan:
- AGENTS.md
- crates/neo-ai/src/types.rs
- crates/neo-ai/src/providers/openai/responses.rs
- crates/neo-agent-core/src/runtime/stream_aggregator.rs
- crates/neo-tui/src/transcript/entry/render_thinking.rs
- .references/codex/codex-rs/tui/src/chatwidget/streaming.rs
- Cited in plan:
- AGENTS.md
- crates/neo-ai/src/types.rs
- crates/neo-ai/src/providers/openai/responses.rs
- crates/neo-agent-core/src/runtime/stream_aggregator.rs
- crates/neo-tui/src/transcript/entry/render_thinking.rs
- .references/codex/codex-rs/tui/src/chatwidget/streaming.rs
- Missing refs:
- none
- Advisory decision: continue
