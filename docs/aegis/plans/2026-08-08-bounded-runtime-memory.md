# Neo 运行时内存有界化实施计划

**Goal:** 删除随流式事件数量线性增长的回合、子任务快照和重试缓冲持有，使长会话与多子任务常态内存保持在 `300-500MB`。

**Architecture:** 复用现有流式通道、`AgentProgressSnapshot` 和 `SessionEventPersistence`；流式调用返回小结果，子任务文字进度限频，最终结果增量汇总，相邻重试增量原位拼接。

**Tech Stack:** Rust 2024、Tokio、现有 JSONL 会话写入、Cargo Nextest；无新依赖。

**Baseline/Authority Refs:**

- `AGENTS.md`
- `docs/aegis/specs/2026-08-08-bounded-runtime-memory-design.md`
- `docs/aegis/specs/2026-08-07-test-suite-governance-design.md` §5.5-5.8
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/session/event_persistence.rs`
- `crates/neo-agent/src/modes/run/mod.rs`

**Compatibility Boundary:** 不改追加式上下文、缓存前缀、历史消息、重试语义、既有会话 schema 和 Delegate-family 卡片；不触碰当前脏工作区中的 task-browser 文件。

**TDD Route:**

```text
TDD Route:
- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change focused regression plus isolated resource measurement
- Reason: 用户未要求 strict TDD，根因已有系统分配和对象尺寸证据
- Verification: 每个任务使用单包、单目标、精确测试名过滤
```

## 计划检查

- Requirement Ready Check: `ready`，目标、场景、验收和不可变边界已明确。
- Change Necessity: `code-change`；重启、配置和清理无法消除事件级持有。
- Existence Check: `reuse-existing`；使用现有 `AgentProgressSnapshot`、轻量进度事件和会话持久化所有者。
- Architecture Integrity Lens: 展示事件只进界面通道，回合结果只保留小结果，canonical 持久化只由 `SessionEventPersistence` 负责。
- Complexity Budget: `at-risk`；`multi_agent/runtime.rs` 已较大，因此只替换现有结果与发布逻辑，不增加新模块或第二套状态机。
- Retirement: 删除 `ChildRunOutput.events`、流式 `Vec<AgentEvent>` 和运行中 `DelegateUpdated` 生产路径；保留旧事件变体用于既有会话重放。

## 文件表

| 文件 | 动作 | 内容 |
| --- | --- | --- |
| `crates/neo-agent/src/modes/run/mod.rs` | 修改 | 流式小结果、准确事件计数、不保留事件全集 |
| `crates/neo-agent/src/rpc/server.rs` | 修改 | RPC 使用小结果的 `event_count` |
| `crates/neo-agent/src/modes/run/session_mgmt.rs` | 修改 | 标题生成只接收需要的字段 |
| `crates/neo-agent/src/modes/run/test_cases/stream.rs` | 修改 | 流式事件计数与无全集持有回归 |
| `crates/neo-agent-core/src/multi_agent/runtime.rs` | 修改 | 轻量限频进度、增量子任务结果 |
| `crates/neo-agent-core/src/tools/delegate.rs` | 修改 | 直接子任务发布轻量进度、使用汇总 usage |
| `crates/neo-agent-core/src/workflow/runtime.rs` | 修改 | 结构化输出和 workflow usage 使用汇总字段 |
| `crates/neo-agent-core/src/session/event_persistence.rs` | 修改 | 相邻增量原位拼接及单元回归 |
| `crates/neo-agent-core/src/runtime/stream_aggregator.rs` | 修改 | 活动文字和思考原位累计，边界处生成最终内容 |
| `crates/neo-agent-core/tests/multi_agent_behavior/progress.rs` | 修改 | 高频文字门控与边界事件回归 |

## Task 1：压缩重试尝试缓冲

**Why:** 删除每个流式增量一个 `AgentEvent` 对象的线性开销，同时保持失败丢弃和胜出追加。

**Steps:**

- 在 `SessionEventPersistence` 增加私有 `push_attempt_event`，只合并严格相邻且键相同的三类增量。
- 扩展现有 `session_event_persistence_discards_failed_attempt`，验证大量增量拼接内容、事件顺序和失败丢弃。
- 运行：

```bash
cargo nextest run -p neo-agent-core --lib session::event_persistence::tests::session_event_persistence_discards_failed_attempt --exact
```

## Task 2：有界化子任务运行中状态

**Why:** 每个文字增量深拷贝完整快照是当前 GB 级增长的主要来源。

**Steps:**

- 将直接子任务的更新回调统一为 `AgentProgressSnapshot`。
- 在直接与 swarm 子任务的共同事件消费处，仅对 `TextDelta` / `ThinkingDelta` 应用 `33ms` 发布门控；其他变化立即发布。
- 用现有 `DelegateProgressUpdated` 替换运行中的 `DelegateUpdated` 生产路径；开始与结束仍发送完整快照。
- 把 `run_agent_snapshot` 改为固定大小汇总，删除 `ChildRunOutput.events` 和扫描式总结函数。
- 更新 Delegate、workflow 的结构化输出与 usage 消费点。
- 扩展 `multi_agent_behavior::progress` 的一个主要守护，证明大量文字增量只产生有界更新且工具边界立即到达。
- 运行：

```bash
cargo nextest run -p neo-agent-core --test multi_agent_behavior progress::child_text_updates_are_rate_limited_without_delaying_boundaries --exact
cargo nextest run -p neo-agent-core --test multi_agent_behavior event_routing::failed_child_run_discards_partial_model_attempt_from_agent_wire --exact
```

## Task 3：删除交互流的回合级事件全集

**Why:** 界面已经通过通道消费事件，`PromptTurn.events` 是第二份无用保留。

**Steps:**

- 新增只含 `session_id`、`assistant_text`、`event_count` 的流式结果。
- `append_streaming_event` 只计数、持久化和转发，不再 push 到回合向量。
- RPC 返回准确 `event_count`；非流式 `PromptTurn` 不变。
- 扩展现有 stream 测试，证明事件被转发且结果不拥有事件向量。
- 运行：

```bash
cargo nextest run -p neo-agent --bin neo modes::run::test_cases::stream::streaming_turn_counts_forwarded_events_without_retaining_them --exact
```

## Task 4：资源证据、复核与提交

**Steps:**

- 使用 release 二进制、临时 `NEO_HOME` 和可重复假模型/会话驱动运行 15 万增量、4 个并发子任务压力场景。
- 记录峰值与结束 RSS；再把增量翻倍，核对 retained event/snapshot 数量不翻倍。
- 运行精确格式和差异检查：

```bash
cargo fmt --all --check
git diff --check
```

- 做一次独立架构符合性与代码质量复核。
- 精确暂存本计划文件，排除既有 task-browser 改动，提交一个 `fix:` 逻辑提交，不推送。

## 最终证据

- 15 万条、4 个并发子任务：峰值 `109.12MiB`，结束 `109.12MiB`，发布事件 `50`。
- 30 万条、4 个并发子任务：峰值 `212.38MiB`，结束 `189.56MiB`，发布事件 `55`。
- 15 万条旧版基线：峰值 `468.00MiB`，结束 `233.16MiB`，发布事件 `73,774`。
- 峰值下降 `76.7%`，结束值下降 `53.2%`。
- 30 万条结束后 `vmmap` 显示物理占用 `43.6MiB`、活跃堆分配 `1.7MiB`；较高 RSS 主要来自可回收的空分配区，不是仍存活的事件或快照。

## 执行就绪视图

- Intent Lock: 修复 PID `47366` 的事件级内存线性增长并达到 `300-500MB` 常态目标。
- Scope Fence: 仅流式结果、子任务进度/结果、重试持久化与对应测试。
- Baseline Lock: `AGENTS.md`、本设计、测试治理 §5.5-5.8。
- Approved Behavior: 追加式历史、重试安全、现有卡片、工具边界保持不变。
- Compatibility Boundary: 不改 schema，不重写历史，不新增用户配置。
- Retirement Boundary: 删除运行时原始事件向量持有；旧持久化事件只读兼容保留。
- Task Batches: 持久化压缩 → 子任务有界化 → 流式小结果 → 资源证据。
- Test Obligations: 三个精确回归、本机隔离 RSS 测量、格式与 diff 检查。
- Review Gates: 规范符合性、代码质量、上下文前缀与重试语义复核。
- Drift Rule: 任何需要新持久化 schema、历史改写或卡片变化的发现立即回到设计。
- Evidence Required: 非零测试数、压力数据、精确 diff、独立复核。
- Advisory Boundary: 此视图只指导执行，不代替最终验证判断。

## 执行路线

- Decision: `inline`
- Evidence: 三个生产任务修改同一条事件流并重叠 `multi_agent/runtime.rs`，并行写入会互相冲突；四个子代理只读核对独立边界。
- Fallback: 某个边界出现未预期 schema 或持久化变化时暂停该片段并更新计划。
- User confirmation required: `no`
