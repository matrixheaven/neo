# Delegate 系列结果交付实施计划

## Goal

修复 `Delegate`、`DelegateGroup`、`DelegateSwarm` 终态结果对主代理不可见或被无意义截断的问题：预算内自动返回完整结果，超预算通过现有 `TaskOutput(view="result")` 分页返回，并统一 `WaitDelegate` 与后台完成通知的终态格式。

## Architecture

规范子代理消息日志是完整结果的唯一来源；`MultiAgentRuntime` 负责子代理生命周期和日志路径，`tools/multi_agent_format.rs` 负责模型可见结果整理，`TaskOutput` 负责分页读取，`BackgroundTaskManager` 只负责任务注册和控制，`ToolResult.details` 只服务界面和事件投影。

不新增工具、不新增结果存储、不把完整回答放入 `AgentSnapshot`。运行中摘要继续使用现有 512 字符预览；终态结果从规范消息记录读取。

`DelegateGroup` 只沿用 Delegate 系列的既有编排/展示路径，不创建独立结果格式或第二个结果所有者。

## Tech Stack

Rust 2024，`neo-agent-core`，现有 `serde_json`、异步文件读取、会话 JSONL 读写、`ToolResult` 和 `TaskOutput` 游标机制。不得新增依赖、提供商分支或平台专用实现。

## Baseline/Authority Refs

- `docs/aegis/specs/2026-08-08-delegate-result-delivery-design.md`
- `docs/aegis/specs/2026-07-30-workflow-model-visible-results-design.md`
- `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- `docs/aegis/specs/2026-08-08-bounded-runtime-memory-design.md`
- `docs/aegis/baseline/2026-07-30-workflow-model-visible-results.md`
- `AGENTS.md` 的上下文完整性、测试治理、Delegate 系列卡片边界和提交规则

BaselineUsageDraft:
- Required baseline refs: 上述规格、工作流结果基线、`AGENTS.md`
- Delivered context refs: 当前会话已确认的结果链路诊断和用户选择
- Acknowledged before plan refs: 工作流模型可见结果设计、内存有界化设计、WaitDelegate 等待路径决定
- Cited in plan refs: 本节、兼容边界、验证矩阵和退休轨迹
- Missing refs: 无
- Decision: continue

## Compatibility Boundary

保持 Delegate、DelegateGroup、DelegateSwarm 卡片布局、层级、排序、进度、展开语义和 transcript 位置；保持规范会话日志追加式写入、缓存前缀、消息顺序、重试语义、`WaitDelegate` 等待生命周期、工作流 `TaskOutput` 视图和现有完整结果字节上限。

不得把 `ToolResult.details` 全局追加到模型上下文。不得通过增大输出上限、压缩历史、重写结果、轮询或重新运行子代理规避问题。

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: 用户未要求严格测试先行
- Test posture: 先完成最小实现，再添加和运行针对历史缺陷的精确回归
- Verification: 每个实现任务使用一个 `neo-agent-core` 测试目标和明确测试名；另行执行格式检查与差异检查

## Verification

核心证据必须覆盖：预算内完整返回、超预算分页连续性、`WaitDelegate` 终态复用、集群有序子项、失败/取消/超时、`details` 不存在时的模型可用性、运行中 512 字符预览仍保留、现有卡片与工作流结果行为不变。

## Requirement Ready Check

- Requirement source refs: 用户确认的“自动内嵌、超限分页”设计选择及前述问题描述
- Goals and scope refs: 本计划 Goal、Architecture、Compatibility Boundary
- User / scenario refs: 前台 Delegate 完成、后台 Delegate 等待、多个子代理的 Swarm 完成
- Requirement item refs: 预算内完整内容、超限精确读取、Swarm 有序结果、真实失败状态
- Acceptance / verification criteria refs: 设计规格第 9 节及本计划验证矩阵
- Open blocker questions: 无
- Decision: ready

## Change Necessity

- User-visible need: 主代理当前无法可靠知道子代理实际交付内容，严重影响后续代码决策。
- No-change / non-code option: 仅修改工具描述不能恢复已经放在 `details` 或被 512 字符截断的内容；仅修改用户文档也不能改变运行时返回。
- Why code change is necessary: `ToolResult.content` 的生产者和 `TaskOutput` 的代理/集群分支必须读取并投影规范最终结果。
- Minimum change boundary: `multi_agent_format.rs`、`delegate.rs`、`delegate_controls.rs`、`background_tasks.rs`、会话结果页读取，以及对应测试和指南。
- Decision: code-change

## Existence Check

- Proposed new surface: 无新工具、无新存储、无第二个结果整理器。
- Existing owner / reuse candidate: `multi_agent_format.rs`、`TaskOutput`、会话 JSONL 读取器、`MultiAgentRuntime`。
- Why existing surface is insufficient: 现有整理器只输出摘要，现有代理/集群 `TaskOutput` 分支没有结果页视图；需要扩展既有所有者而不是增加并行入口。
- Creation proof: 不创建新产品表面；仅补齐既有工具对终态结果的投影。
- Entropy / retirement impact: 删除摘要专用的 `delegate_result_content`、聚合专用的 `swarm_run_result` 以及等待终态的状态专用路径。
- Decision: reuse-existing

## Architecture Integrity Lens

- Invariant: 完整结果只有规范子代理消息日志一个来源，模型可见投影只有一个整理入口。
- Canonical owner: 生命周期和日志由 `MultiAgentRuntime`/会话存储负责，模型结果由 `multi_agent_format.rs` 负责，分页由 `TaskOutput` 负责。
- Responsibility overlap: 当前调用方各自拼接文本，`details` 携带了模型必须知道的字段；计划删除这些重复投影。
- Higher-level simplification: 不改变 `turn_loop` 的内容消费逻辑，不做全局 `details` 注入；修复所有调用方共同经过的结果整理边界。
- Retirement / falsifier: 实现后扫描不得再发现代理或集群终态调用 `delegate_result_content`、`swarm_run_result` 或状态专用终态文本；若完整结果只能从快照摘要取得，则停止并回到结果来源设计。
- Verdict: proceed

## Plan-Time Complexity Check

- Artifact class: `Source Complexity`、`Test Complexity`、`Decision / Plan Complexity`
- Target files / artifacts: `multi_agent_format.rs`、`runtime.rs`、`background_tasks.rs`、`delegate.rs`、`delegate_controls.rs`、现有多代理测试与本规格/计划
- Current pressure: `runtime.rs` 和 `background_tasks.rs` 已较大；结果整理逻辑分散在多个调用方。
- Projected post-change pressure: 在现有整理文件中统一投影，增加一个有界会话结果页读取；删除三条摘要专用路径，净复杂度保持不升。
- Budget result: within-budget
- Planned governance: 只在现有所有者中添加职责，避免继续向 `runtime.rs` 塞入界面格式；会话读取只扩展已有读取器。

## Execution Readiness View

- Intent Lock: 让主代理收到真实子代理交付物；不改变卡片或上下文历史。
- Scope Fence: 只修改本计划文件表和测试/指南；`.references/` 与其他脏文件不可触碰。
- Baseline Lock: 以上基线在实现证据完成前保持有效。
- Approved Behavior: 预算内自动内嵌，超限分页，Swarm 有序子项，统一等待终态。
- Owner Constraints: 单一结果整理入口；会话日志是完整结果来源；`TaskOutput` 是超大结果读取入口。
- Compatibility Boundary: 卡片、追加式消息、工作流视图、等待生命周期不变。
- Retirement Boundary: 删除摘要专用和聚合专用旧路径，不保留内部兼容双轨。
- Task Batches: 结果整理 → 单代理/等待 → Swarm/后台 → 会话分页/回归/文档。
- Test Obligations: 每个任务的精确测试命令、格式检查、旧路径扫描和差异检查。
- Review Gates: 每个任务完成后检查模型可见内容、`details` 降级和卡片边界；全部任务后做一次规格覆盖审查。
- Drift / Rewind Rules: 出现第二个结果存储、需要修改上下文前缀、需要提高全局输出上限或卡片变化时，停止实现并回到规格。
- Evidence Required Before Completion: 精确本地测试通过，旧路径扫描为空，`cargo fmt --all --check` 和 `git diff --check` 通过；远端与其他平台仍单独报告。
- Advisory Boundary: 本视图只负责实施准备，不是运行时授权或完成判定。

## Tasks

### Task 1: 统一 Delegate 系列模型结果整理

Files:
- Modify `crates/neo-agent-core/src/tools/multi_agent_format.rs`
- Modify `crates/neo-agent-core/src/multi_agent/runtime.rs` only for a typed, bounded final-result read accessor if the existing session reader cannot provide it
- Add focused tests in `crates/neo-agent-core/tests/multi_agent_behavior/model_visible_results.rs`
- Modify `crates/neo-agent-core/tests/multi_agent_behavior.rs` to declare the new behavior module

Why: 所有调用方必须把下一步决策所需字段放进 `ToolResult.content`，不能继续依赖不可见的 `details`。

Change Necessity: 现有摘要整理器会丢失完整回答；最小边界是扩展现有 `multi_agent_format.rs`，并通过已有会话读取所有者取得终态文本，不增加工具或快照字段。

Repair Track: 删除 `delegate_result_content`，增加单代理和单项结果的统一模型 JSON，区分 `inline` 与 `page`，保留 512 字符仅给预览字段；终态结果从规范消息记录读取。

Retirement Track: `delegate_result_content` 在所有调用方迁移后删除；若扫描仍有调用，任务不算完成。

Verification:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_behavior model_visible_results::delegate_result_content_includes_complete_text_without_details
cargo nextest run -p neo-agent-core --test multi_agent_behavior model_visible_results::oversized_delegate_result_returns_first_page_and_exact_next_action
```

Steps:

1. 在现有整理文件中定义稳定的单代理结果字段：目标、真实状态、结果模式、当前文本、总字符数、是否还有后续页和精确下一步调用；空字段省略。
2. 让模型结果整理接收终态结果读取器返回的页面，不从 `outcome.summary` 推断完整回答。
3. 让 `details` 继续使用现有类型化快照和有界活动预览，确保完整回答不被复制进长期快照。
4. 添加预算内、超预算和 `details` 为空的回归，确认模型可见内容仍包含下一步所需全部字段。
5. 运行上述测试，格式化新增测试文件，并检查差异。

### Task 2: 让 Delegate 与 WaitDelegate 复用终态结果

Files:
- Modify `crates/neo-agent-core/src/tools/delegate.rs`
- Modify `crates/neo-agent-core/src/tools/delegate_controls.rs`
- Modify `crates/neo-agent-core/src/tools/background_tasks.rs` only where completion notification delegates to the common formatter
- Extend `crates/neo-agent-core/tests/multi_agent_behavior/model_visible_results.rs`
- Extend `crates/neo-agent-core/tests/multi_agent_behavior/background.rs` only when existing background lifecycle fixtures are needed

Why: 前台完成、后台完成通知和等待完成必须对主代理呈现同一份真实结果。

Change Necessity: 当前 `Delegate` 使用摘要，`WaitDelegate` 使用状态，两个调用方会产生不同且都不完整的模型输入；必须在终态调用共同整理入口。

Repair Track: 前台 Delegate 完成后按预算选择内嵌或第一页；`WaitDelegate` 的 `all_terminal` 复用同一终态结果，`wait_timed_out` 保留当前状态并只给继续等待动作；后台完成通知复用该路径。

Retirement Track: 删除 `WaitDelegate` 的状态专用终态文本和 Delegate 调用方的手工摘要拼接；保留超时与未找到错误的现有真实状态语义。

Compatibility: 不改变等待轮询间隔、取消生命周期、前后台启动行为和 TUI 事件。

Verification:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_behavior model_visible_results::wait_delegate_terminal_result_matches_delegate_projection
cargo nextest run -p neo-agent-core --test multi_agent_behavior model_visible_results::background_delegate_completion_exposes_result_or_exact_page_action
```

Steps:

1. 将前台 Delegate 完成分支改为调用统一整理入口，并保留实际用量和类型化 `details`。
2. 将 WaitDelegate 的终态分支改为按目标类型调用单代理或集群整理；超时分支不得声称结果已返回。
3. 将后台完成通知改为携带相同的结果字段；运行中通知仍是轻量状态预览。
4. 保持 `turn_loop` 只消费 `ToolResult.content`，不加入全局 `details` 注入。
5. 运行精确测试并检查旧状态专用字符串不再作为终态结果来源。

### Task 3: 完成 Swarm 与 TaskOutput 的结果读取

Files:
- Modify `crates/neo-agent-core/src/tools/delegate.rs`
- Modify `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify `crates/neo-agent-core/src/session/mod.rs` only to add bounded page extraction over the existing JSONL reader
- Modify `crates/neo-agent-core/src/multi_agent/runtime.rs` only to expose the existing child wire path through a typed result-page accessor
- Extend `crates/neo-agent-core/tests/multi_agent_behavior/model_visible_results.rs`
- Extend `crates/neo-agent-core/src/session/test_cases` or the existing session reader test owner with a bounded-page regression

Why: Swarm 当前只返回聚合统计，TaskOutput 当前对代理/集群只返回摘要；这是造成主代理无法重建整体结果的最大缺口。

Change Necessity: 仅改变 Swarm 文案无法恢复每项正文；必须让现有 `TaskOutput(view="result")` 读取规范子代理日志，并由单一整理入口组装有序子项。

Repair Track: Swarm 结果包含每个子项的编号、`agent_id`、标题、状态、摘要和内嵌/分页结果；TaskOutput 对代理和 Swarm 启用 `view="result"`，使用目标、视图、查询绑定的不透明游标。

Session read boundary: 扩展 `JsonlSessionReader` 的已有读取所有者，按事件顺序定位最终助手消息并只保留当前页、固定解析缓冲和游标信息；不得调用无界 `read_all` 来拼装超大结果页。

Retirement Track: 删除 `swarm_run_result` 的 aggregate-only 内容和 TaskOutput 对代理/集群的摘要复用；保留 `swarm_details` 作为界面投影。

Compatibility: 工作流 `TaskOutput` 的五种视图、游标校验、字节上限和工作流日志行为保持不变；代理/集群新增的 `result` 视图不改变其他任务类型。

Verification:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_behavior model_visible_results::swarm_result_preserves_input_order_and_child_ids
cargo nextest run -p neo-agent-core --test multi_agent_behavior model_visible_results::task_output_result_pages_reconstruct_delegate_text
cargo nextest run -p neo-agent-core --lib session::tests::result_page_is_bounded_and_utf8_safe
```

Steps:

1. 让 Swarm 前台终态调用统一集群整理入口，逐项填充结果，不再生成只有统计的文本。
2. 将 `TaskOutputInput.view` 文案更新为代理、集群和工作流都支持 `result`，并把代理/集群分支路由到有界结果页读取。
3. 让第一页返回 `has_more`、游标和精确下一步 `TaskOutput` 调用；连续调用必须推进，错误游标必须明确拒绝。
4. 为失败、取消、超时和部分完成的 Swarm 保留每项真实状态；只对仍有正文的项提供结果页动作。
5. 运行 Swarm、分页和会话读取测试，确认单页重建不会把完整结果复制到运行时快照。

### Task 4: 修正文案、指南并完成收口验证

Files:
- Modify `crates/neo-agent-core/src/tools/delegate.rs` tool descriptions
- Modify `crates/neo-agent-core/src/tools/delegate_controls.rs` tool descriptions
- Modify `crates/neo-agent-core/src/tools/background_tasks.rs` TaskOutput description
- Modify `docs/user_guide/zh/customization/agents.md`
- Modify `docs/user_guide/en/customization/agents.md`
- Extend `crates/neo-agent-core/tests/multi_agent_behavior/model_visible_results.rs` for tool guidance assertions

Why: 运行时返回修好后，工具描述必须教会普通模型何时直接使用结果、何时执行返回的精确分页动作，避免重新引入 `Sleep` 与列表轮询。

Change Necessity: 仅靠运行时字段不能保证模型选对下一步；当前指南仍明确写着“只返回摘要”，属于与实现目标冲突的旧说明。

Repair Track: 文案只保留一条明确路径：已返回结果就直接使用；存在 `next_actions` 就按其参数调用 `TaskOutput`；`ListDelegates` 只用于发现与状态。

Retirement Track: 删除中英文“只返回结果摘要”的说明和终态中竞争性的列表/轮询提示；不新增别名或兼容文案。

Verification:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_behavior model_visible_results::delegate_tool_descriptions_teach_inline_or_paged_result_reading
rtk rg -n "only the result summary|只.*结果摘要|Use ListDelegates|使用 ListDelegates" crates/neo-agent-core/src/tools docs/user_guide/zh/customization/agents.md docs/user_guide/en/customization/agents.md
```

Expected scan output: no stale summary-only or terminal polling guidance in the Delegate-family result paths; legitimate `ListDelegates` discovery description may remain.

## Risks

- 子代理日志格式若无法在固定缓冲下读取最终助手正文，停止并报告来源缺口，不把快照摘要冒充正文。
- Swarm 同时有多个超大子项时，`next_actions` 可能很多；实现应在现有结果预算内按顺序返回可继续读取的项，不改变子项顺序。
- 本地聚焦测试不代表远端持续集成、实时提供商或原生 Windows/Linux 结果；完成时分开报告。
- 共享脏工作区中的无关改动不得纳入本任务提交。

## Retirement

完成后必须通过源码扫描确认：

- `delegate_result_content` 无生产调用；
- `swarm_run_result` 不再作为 aggregate-only 结果生产者；
- `WaitDelegate` 终态不再只返回 `id/kind/status`；
- `TaskOutput` 代理/集群分支不再返回摘要代替结果。

保留 `bounded_latest_text` 及进度快照截断，因为它们只服务运行中界面预览；删除它们会扩大内存和终端输出风险，不能作为本任务的“清理”。

## Execution Route

- Decision: `inline`
- Evidence: 任务之间共享统一整理入口和结果页边界，适合单一协调者按顺序实现并逐任务验证。
- Fallback: 若实现时出现独立且不重叠的文件边界，再由协调者重新评估是否拆分；默认不派生并行改动。
- User confirmation required: no
