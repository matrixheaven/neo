# Neo 测试套件治理设计

日期：`2026-08-07`

状态：`最终交接基线`

架构复核：`需要`

## 1. 目标

将四个 crate 的测试从 TDD 过程遗留物，收敛为少而强、能阻止行为回退的长期行为守护。治理必须同时降低本机执行时间、删除低价值覆盖、合并重复案例，并形成四个 crate 共用的一套目录、命名、夹具和验证规则。

成功不以测试数量下降为准。成功必须同时满足：

1. 高风险行为仍有唯一且清晰的守护测试。
2. 本机热构建后的完整确定性测试耗时至少下降 `60%`，并降到 `20` 分钟以内。
3. 冷构建、测试执行、串行资源组分别计时，不能把编译时间误判为测试时间。
4. 没有巨型测试文件、无意义文件名、重复夹具和仅靠固定等待通过的测试。
5. Windows、Linux、macOS 的平台行为仍由对应原生环境证明。

若无法在保留高价值行为的前提下达到耗时目标，执行者必须停下并提交瓶颈证据，不能继续删除重要测试来凑数字。

## 2. 当前基线

静态计数基于当前工作树，只用于确定治理规模，不代表实际运行数量：

| crate | 含测试的 Rust 文件 | 测试定义 | `tests/` 顶层测试文件 | `tests/` 行数 |
|---|---:|---:|---:|---:|
| `neo-ai` | 22 | 208 | 6 | 5,172 |
| `neo-agent-core` | 136 | 1,382 | 40 | 43,372 |
| `neo-tui` | 84 | 1,064 | 26 | 25,761 |
| `neo-agent` | 48 | 865 | 11 | 9,134 |
| 合计 | 290 | 3,519 | 83 | 83,439 |

最明显的结构热点：

- `crates/neo-agent/src/modes/interactive/tests.rs`：20,552 行，407 个测试。
- `crates/neo-agent-core/tests/runtime_turn.rs`：13,091 行，153 个测试。
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`：4,275 行，66 个测试。
- `crates/neo-tui/tests/multi_agent_transcript.rs`：4,068 行，77 个测试。
- `crates/neo-ai/tests/real_provider_adapters.rs`：3,381 行，66 个测试。
- `crates/neo-tui/tests/tool_cards.rs`：3,350 行，65 个测试。
- `crates/neo-tui/tests/transcript_pane.rs`：3,163 行，68 个测试。

远端历史样本在旧提交上执行 3,311 个测试约耗时 154.5 秒；用户本机完整测试约耗时一小时。两者说明环境差异显著，远端速度不能否定本机问题。本设计只接受当前本机的分段测量作为性能主基线。

## 3. 先看风险

测试治理的主要风险高于收益，必须先约束：

- 按年龄、文件大小或 TDD 来源删除，会丢掉真实回归保护。
- 将所有测试搬到 `tests/` 会迫使私有实现公开，反而破坏模块边界。
- 将大量小测试合成一个长测试，会降低失败定位能力并让后半段断言无法执行。
- 仅拆文件不会降低耗时；增加顶层集成测试文件还会增加编译和链接成本。
- 仅把慢测设为忽略或移到夜间，会隐藏回归，不算治理完成。
- 使用固定等待、重试或放宽超时，会掩盖竞态、锁泄漏和进程回收问题。
- macOS 本机通过不能代替 Windows 或 Linux 原生证据。

收益只有在上述风险被控制后才成立：更快的本机反馈、更少的测试维护、更清晰的行为边界，以及更可靠的后续重构保护。

## 4. 方案选择

### 方案一：只整理文件

风险最低，但几乎不能解决一小时耗时，也保留重复和弱断言。拒绝。

### 方案二：本机测量、语义去重、统一结构、精确调度

先测量本机冷构建、热执行和资源串行组，再按行为价值删除、合并或重写；随后统一四个 crate 的文件结构和 Nextest 分类。采用。

### 方案三：围绕少量端到端测试重写全部测试

可能大量减数，但反馈慢、定位差，且容易遗漏私有算法和失败分支。拒绝。

## 5. 统一测试结构

四个 crate 必须共同遵守以下唯一规则。

### 5.1 私有单元测试

- 仅验证单个模块的私有纯逻辑、解析、状态转换或边界分支。
- 小型测试保留在生产文件内的 `#[cfg(test)] mod tests`。
- 内联测试区域目标不超过 300 行，硬上限 600 行或 12 个测试；超过任一上限必须按行为拆出。
- 拆出后使用明确文件名，例如 `permission_mode_tests.rs`、`stream_assembly_tests.rs`。
- 需要集中多个私有测试文件时，由生产模块使用显式 `#[path = "test_cases/<behavior>.rs"]` 声明；不得创建测试专用 `mod.rs` 或 `tests.rs` 聚合文件。

### 5.2 crate 行为测试

- 只有通过公开接口验证跨模块行为时，才放在 `crates/<crate>/tests/`。
- 每个顶层测试目标对应一个领域，例如 `provider_stream_behavior.rs`、`runtime_context_behavior.rs`、`transcript_behavior.rs`、`cli_session_behavior.rs`。
- 顶层文件只负责声明同领域子模块，测试正文放在 `tests/<domain>/<behavior>.rs`；这样既避免巨型文件，也避免每个小文件都变成独立测试二进制。
- 共享夹具放在 `tests/<domain>/http_server.rs`、`tests/<domain>/isolated_home.rs` 等用途明确的文件中，通过显式 `#[path]` 引入。
- 不得使用 `mod.rs`、`tests.rs`、`test.rs`、`misc.rs`、`common.rs`、`integration.rs` 等无法表达用途的测试文件名。

### 5.3 文件规模

- 测试正文文件目标为 300 至 800 行。
- 测试专用文件硬上限为 1,200 行或 30 个测试。
- 顶层领域入口只允许模块声明和极少量领域级夹具，目标不超过 100 行。
- 只有 1 至 2 个测试的顶层集成文件必须并入相同领域，平台专用或资源专用行为除外。
- 行数只触发拆分复核，不能作为删除测试的理由。

### 5.4 命名

- 测试函数名统一为“条件加可观察结果”，例如 `closed_input_routes_enter_to_next_turn`。
- 不加无意义的 `test_` 前缀，不使用工单号，不保留版本后缀。
- 平台专用文件使用 `_windows`、`_unix` 或 `_macos` 后缀，并必须有真实条件编译。
- 资源压力文件使用 `_resource` 后缀；不能借该后缀逃离正常持续集成。

### 5.5 轻量企业级治理模型

吸收大型单仓项目的成熟原则，但只保留 Neo 实际需要的六条制度：

1. **按生产领域归属。** 测试归对应生产模块或 crate 领域所有，不建立独立测试团队目录、全仓夹具层或第二套所有权表。
2. **最低充分层。** 能由纯函数单元测试守住的行为，不升级为 crate 行为测试；只有新增跨模块、进程、终端、持久化或平台风险时才提升层级。
3. **一个行为一个主要守护。** 同一故障类型只有一个最便宜、最直接的主要测试；上层最多保留一条新增边界链路。
4. **完全隔离。** 测试不依赖执行顺序、共享当前目录、固定端口、真实用户目录、真实凭据、外部网络或其他测试遗留状态。
5. **性能预算。** 测试和生产代码一样受性能复核；新增测试若进入 20 秒慢测范围，必须证明资源边界不可用更小数据或虚拟时间表达。
6. **生命周期明确。** 生产行为删除时，同批删除对应测试；行为替换时迁移主要守护并删除旧守护；测试抖动按缺陷处理，不使用重试或永久隔离。

新增测试只需回答三个问题：

1. 它守护的可观察行为或故障是什么？
2. 最低哪一层足以捕获该故障？
3. 当前是否已有能在同一故障下失败的主要守护？

任一问题答不清，就不新增该测试。无需表单、标签、覆盖率门槛、长期清单或新管理工具。

### 5.6 五类测试

| 类型 | 用途 | 默认位置 | 禁止事项 |
|---|---|---|---|
| 单元 | 私有纯逻辑、解析、局部状态转换 | 生产文件内或明确的源码侧测试文件 | 启动进程、访问网络、跨 crate |
| crate 行为 | 公开接口和跨模块接线 | `crates/<crate>/tests/` 的领域入口 | 重复单元参数矩阵 |
| 产品边界 | CLI、RPC、终端、持久化、真实进程 | 仅归最终入口 crate | 重复所有下层案例 |
| 平台 | Windows、Linux、macOS 差异 | 明确平台文件和条件编译 | 用非原生结果代替原生证据 |
| 资源 | 数据量、输出量、并发量和回收边界 | 明确 `_resource` 领域 | 用任意大数据冒充边界证明 |

这五类是分类语言，不是新运行框架。测试仍由 Cargo 和 Nextest 执行。

### 5.7 新增测试复核门槛

- 修复缺陷时，优先修改现有主要守护；只有没有覆盖时才新增一个最低充分层回归。
- 新功能只覆盖已承诺行为和关键失败分支，不为每个实现步骤保留 TDD 临时测试。
- 测试名、断言和夹具必须让审查者在 30 秒内看懂故障条件与结果。
- 测试数据使用触发行为所需的最小规模；资源测试必须在代码中说明阈值来源。
- 任意真实等待、全局环境修改、进程启动和文件同步都需要证明不可由更便宜的确定性手段替代。
- 没有唯一行为价值的新增测试在代码复核时直接拒绝，不先合入再积债。

### 5.8 固定的顶层测试目标

本轮整理后的顶层测试目标已经确定。执行者只能按下表迁移，不能另起目录体系、保留旧入口或按个人偏好重新分组。表中的“子模块”均位于同名目录内，例如 `provider_protocol_behavior.rs` 对应 `provider_protocol_behavior/`。

| crate | 最终顶层目标 | 迁入的现有顶层文件 | 固定子模块 |
|---|---|---|---|
| `neo-ai` | `provider_protocol_behavior.rs` | `real_provider_adapters.rs`、`openai_compatible_provider.rs`、`tool_schema_and_stream.rs` | `openai_responses.rs`、`openai_compatible.rs`、`anthropic.rs`、`google.rs`、`image_generation.rs`、`stream_events.rs`、`tool_schema.rs`、`http_server.rs` |
| `neo-ai` | `model_resolution_behavior.rs` | `model_registry.rs`、`provider_resolver.rs` | `model_registry.rs`、`provider_resolver.rs` |
| `neo-ai` | `environment_behavior.rs` | `env_and_options.rs` | `environment.rs`、`request_options.rs` |
| `neo-agent-core` | `runtime_behavior.rs` | `runtime_turn.rs`、`goals.rs` | `context.rs`、`streaming.rs`、`thinking.rs`、`tool_dispatch.rs`、`permissions.rs`、`compaction.rs`、`retry.rs`、`plan_and_goal.rs`、`fake_harness.rs` |
| `neo-agent-core` | `session_behavior.rs` | `session_jsonl.rs`、`session_state.rs`、`session_tree.rs`、`instruction_registry.rs` | `jsonl_append.rs`、`jsonl_recovery.rs`、`schema_compatibility.rs`、`state.rs`、`tree.rs`、`instructions.rs` |
| `neo-agent-core` | `tool_behavior.rs` | `shell_messages.rs`、`skills.rs`、`tool_bash.rs`、`tool_files.rs`、`tool_names.rs`、`tool_output_capture.rs`、`tool_permissions.rs`、`tool_schema_descriptions.rs` | `bash.rs`、`files.rs`、`names.rs`、`output_capture.rs`、`permissions.rs`、`schema.rs`、`shell_messages.rs`、`skills.rs` |
| `neo-agent-core` | `multi_agent_behavior.rs` | `multi_agent_background.rs`、`multi_agent_roles.rs`、`multi_agent_runtime.rs`、`multi_agent_scheduler.rs` | `background.rs`、`roles.rs`、`lifecycle.rs`、`progress.rs`、`event_routing.rs`、`usage.rs`、`cancellation.rs`、`scheduler.rs` |
| `neo-agent-core` | `workflow_behavior.rs` | 所有现有 `workflow_*.rs` | 去掉 `workflow_` 前缀后的同名模块；`runtime.rs` 进一步拆为 `runtime_lifecycle.rs`、`runtime_effects.rs`、`runtime_recovery.rs` |
| `neo-agent-core` | `rpc_behavior.rs` | `rpc_jsonl.rs` | `jsonl.rs` |
| `neo-tui` | `app_behavior.rs` | `app_shell.rs`、`task_browser.rs`、`theme_manager.rs`、`todo_question.rs` | `shell.rs`、`footer.rs`、`blocking_dialogs.rs`、`task_browser.rs`、`theme_manager.rs`、`questions.rs` |
| `neo-tui` | `transcript_behavior.rs` | `transcript.rs`、`transcript_pane.rs`、`transcript_store.rs`、`transcript_selection.rs`、`fullscreen_transcript.rs`、`progressive_transcript.rs`、`live_renderer.rs` | `pane.rs`、`store.rs`、`selection.rs`、`fullscreen.rs`、`progressive.rs`、`live_renderer.rs` |
| `neo-tui` | `agent_transcript_behavior.rs` | `multi_agent_transcript.rs`、`workflow_transcript.rs` | `delegate.rs`、`delegate_group.rs`、`delegate_swarm.rs`、`workflow.rs`、`background_updates.rs` |
| `neo-tui` | `tool_card_behavior.rs` | `tool_cards.rs`、`tool_grouping.rs` | `cards.rs`、`grouping.rs`、`approval.rs`、`shell.rs` |
| `neo-tui` | `rendering_behavior.rs` | `markdown_rendering.rs`、`thinking_blocks.rs`、`primitives.rs`、`diff_model.rs`、`terminal_frame.rs`、`core_components.rs` | 与现有文件同名的明确模块 |
| `neo-tui` | `terminal_behavior.rs` | `fullscreen_terminal.rs`、`shell_events.rs`、`shell_mode_render.rs`、`image_protocols.rs` | `fullscreen.rs`、`shell_events.rs`、`shell_mode.rs`、`images.rs` |
| `neo-tui` | `input_behavior.rs` | `chrome_selection.rs` | `chrome_selection.rs` |
| `neo-agent` | `cli_behavior.rs` | `cli_commands.rs`、`mock_provider_e2e.rs`、`fullscreen_output.rs` | `commands.rs`、`sessions.rs`、`config.rs`、`mock_provider.rs`、`fullscreen_output.rs`、`http_server.rs` |
| `neo-agent` | `rpc_behavior.rs` | `rpc_mode.rs` | `state.rs`、`sessions.rs`、`commands.rs`、`streaming.rs`、`recovery.rs` |
| `neo-agent` | `process_behavior.rs` | `process_guard.rs`、`process_guard_windows.rs`、`shell_admission_runtime.rs`、`tool_bash_guardian.rs`、`tool_terminal_guardian.rs` | `process_guard_unix.rs`、`process_guard_windows.rs`、`shell_admission.rs`、`bash_guardian.rs`、`terminal_guardian.rs` |
| `neo-agent` | `workflow_behavior.rs` | `workflow_cli.rs`、`workflow_notifications.rs` | `cli.rs`、`notifications.rs` |

`workflow_*.rs` 的展开清单固定为：`admission`、`artifacts`、`builtins`、`check`、`child_journal`、`dispatch`、`harness`、`journal`、`launch`、`lineage`、`lua`、`model_visibility`、`output`、`recovery_dispatch`、`registry`、`runtime_contract`、`schema`、`swarm`、`tool_policy`、`user_input`，以及前述三个运行时子模块。现有顶层文件迁空后立即删除，不保留转发入口。

### 5.9 当前超限文件的强制处理清单

下列测试专用文件在当前工作树已经超过 1,200 行或 30 个测试，全部属于本轮范围，不得留作“后续治理”：

- `neo-ai`：`tests/real_provider_adapters.rs`。
- `neo-agent-core`：`tests/instruction_registry.rs`、`tests/multi_agent_background.rs`、`tests/multi_agent_runtime.rs`、`tests/runtime_turn.rs`、`tests/session_jsonl.rs`、`tests/workflow_dispatch.rs`、`tests/workflow_journal.rs`、`tests/workflow_registry.rs`、`tests/workflow_runtime.rs`。
- `neo-tui`：`tests/app_shell.rs`、`tests/multi_agent_transcript.rs`、`tests/progressive_transcript.rs`、`tests/tool_cards.rs`、`tests/transcript_pane.rs`、`tests/transcript_store.rs`、`tests/workflow_transcript.rs`。
- `neo-agent`：`tests/cli_commands.rs`、`tests/tool_terminal_guardian.rs`、`src/modes/interactive/tests.rs`。

源码侧测试以“超过 12 个测试”为本轮强制提取条件。当前清单如下；执行者只能在对应生产模块旁创建 `test_cases/`，不能迁入 crate 顶层 `tests/`：

- `neo-ai`：`src/providers/common/error.rs`。
- `neo-agent-core`：`src/compaction/mod.rs`、`src/compaction/projection.rs`、`src/multi_agent/progress.rs`、`src/multi_agent/runtime.rs`、`src/runtime/permission.rs`、`src/tools/ask_user.rs`、`src/tools/background_tasks.rs`、`src/tools/glob.rs`、`src/tools/grep.rs`、`src/tools/mcp/oauth/service.rs`、`src/tools/mcp_manager.rs`、`src/tools/mod.rs`、`src/tools/plan_mode.rs`、`src/tools/read.rs`、`src/tools/shell_env.rs`、`src/tools/skills_manager.rs`、`src/tools/todo.rs`。
- `neo-tui`：`src/dialogs/custom_endpoint_wizard.rs`、`src/dialogs/mcp_add_form.rs`、`src/dialogs/mcp_manager.rs`、`src/dialogs/model_selector.rs`、`src/dialogs/provider_manager.rs`、`src/input/mod.rs`、`src/input/raw_input.rs`、`src/markdown.rs`、`src/paste.rs`、`src/primitive/ansi_escape.rs`、`src/shell/theme_manager.rs`、`src/transcript/entry/mod.rs`、`src/transcript/plan_box.rs`、`src/widgets/btw_panel.rs`、`src/widgets/todo_panel.rs`。
- `neo-agent`：`src/config/mod.rs`、`src/config/mutations.rs`、`src/mcp_ops.rs`、`src/modes/run/mod.rs`、`src/theme_draft.rs`、`src/themes.rs`、`src/workspaces.rs`。

`src/modes/interactive/tests.rs` 使用固定目录 `src/modes/interactive/test_cases/`，固定文件为 `input.rs`、`sessions.rs`、`workflow.rs`、`themes.rs`、`tasks.rs`、`approvals.rs`、`clipboard.rs`、`transcript.rs`、`terminal.rs`。其他源码侧文件按现有测试函数的生产行为前缀拆分；若无法在不自创分类的情况下满足 30 个测试上限，停止该文件并请求协调者裁决，不得使用 `part1.rs`、`other.rs` 或数字分片。

规模上限的适用范围是：本轮清单中的文件、所有本轮触及的测试文件、所有新增测试文件。未触及且不在清单中的现有源码测试不因生产文件总行数被迫搬迁。这样既保证当前巨型文件全部处理，又不把生产文件总长度误当成测试块长度。

### 5.10 测试退役记录

每个语义精简提交必须在提交说明或对应工作记录中附一张短表，逐组填写，不允许只写总数：

| 删除或合并的测试 | 保留的主要守护 | 两者共同捕获的生产故障 | 证明方式 | 精确命令 | 实际运行数 |
|---|---|---|---|---|---:|
| 完整测试名 | 完整测试名 | 具体到被破坏的分支或可观察结果 | 调用路径或临时故障注入 | 一个包、一个目标、完整名称、精确匹配 | 非零 |

缺少任一列时，该组不得删除。临时故障注入只用于证明，证明后必须撤销，不能进入提交。

## 6. 测试价值判定

每个现有测试只能得到以下四种结论之一。

### 保留

符合任一条件即保留：

- 唯一守护用户可见行为、公开接口或跨模块集成。
- 守护上下文只追加、缓存前缀、持久化、权限、安全、数据丢失或恢复语义。
- 守护真实历史缺陷，且同类故障仍可能再次出现。
- 守护平台差异、错误分支、资源边界、协议顺序或并发终态。
- 删除后没有更便宜、更强的测试能捕获同一故障。

### 合并

以下情况改为一个表驱动测试或一个共享断言：

- 多个测试只改变输入值，却走相同生产分支和相同断言。
- 同一协议的成功、拒绝、失败状态可以由小型案例表清晰表达。
- 多个测试重复完整夹具，只为验证同一映射表。

合并后的每个案例必须带名称；一个案例失败时必须能直接定位，不得将多个有先后依赖的场景塞进同一个测试。

### 删除

只有满足全部条件才删除：

1. 不守护独立的用户行为、风险边界或历史缺陷。
2. 保留测试能在相同生产故障下失败。
3. 断言仅验证派生能力、标准库行为、测试辅助接口、非空文本或重复快照细节。
4. 删除后运行精确目标，保留测试仍能证明相同语义。

高风险重复组若无法从调用路径直接证明，必须做一次临时故障注入；保留测试能失败后，才允许删除较弱测试。不得新增永久变异测试依赖。

### 重写

行为重要但测试依赖固定等待、真实网络、固定端口、共享当前目录、全局环境或不完整进程回收时，重写而不是删除。优先使用暂停时间、就绪信号、`127.0.0.1:0`、`tempfile`、现有假模型和现有确定性工作流夹具。

## 7. 测试层级去重

同一行为最多保留：

1. 一组最便宜的单元参数矩阵，证明局部分支。
2. 一条必要的 crate 行为链路，证明模块接线。
3. 只有跨进程或最终产品入口新增了真实风险时，才保留一条 `neo-agent` 端到端链路。

上层测试不得重复下层全部案例。上层只验证接线、序列化、进程边界、终端状态或持久化等新增风险。

## 8. 四个 crate 的不可删除边界

### `neo-ai`

- 各提供方请求形状、认证头敏感标记和密钥不泄漏。
- 流事件顺序、工具调用生命周期、终止原因和错误分类。
- 分片、交错、缺名和最终参数覆盖。
- 上下文缓存前缀稳定。
- Windows 与非 Windows 环境变量解析差异。

### `neo-agent-core`

- 上下文、系统提示、历史消息和规范记录只追加；缓存前缀稳定。
- Session JSONL 的追加、断尾恢复、损坏隔离、指令纪元和符号链接边界。
- 工具权限、路径边界、Shell admission、无限等待语义和进程回收。
- 工作流先持久化后副作用、恢复不重复执行、异常收尾和子任务原子终态。
- 多代理调度、事件路由、使用量和取消语义。

### `neo-tui`

- 输入焦点、阻塞弹窗、Other 光标编辑和平台按键分支。
- 全屏进入、暂停、恢复、退出、图片清理和终端模式恢复。
- 转录锚点、尾随、锁定滚动、选择和后台更新后回看最新状态。
- Delegate、DelegateGroup、DelegateSwarm 卡片现有布局、层级、进度、展开和内容语义。
- 终端转义协议、显示宽度和跨平台通知安全。

### `neo-agent`

- CLI 真实进程入口、参数冲突、退出状态、路径和符号链接拒绝。
- 跨工作区 session 索引、精确恢复、启动目录锚定和转录重放。
- 假提供方端到端请求投影、稳定事件输出和上下文加载。
- RPC 流式事件、输入未结束前响应和失败后继续服务。
- Unix 进程树与 Windows 作业对象。
- 上下文只追加、恢复不重复和指令变化追加新事件。

## 9. 本机性能治理

本机先记录以下三段，不得用远端数字替代：

1. 冷构建与测试发现时间。
2. 编译完成后的完整确定性测试执行时间。
3. `shell-guardian` 当前串行组的独立执行时间。

本机前后测量必须使用默认 Nextest 配置和相同命令，只额外打开完整状态输出。`profile.ci` 的普通慢测阈值不同，只用于远端持续集成，不能作为本机性能基线。

优先排查顺序：

1. 粗粒度串行组是否把无资源冲突的测试串行化。
2. 固定等待、锁自等待、进程或伪终端清理是否卡到超时。
3. 10,000 次同步日志写入和 12 MiB 输出测试是否使用了超过行为阈值所需的数据量。
4. 顶层测试二进制数量是否造成重复编译和链接。
5. 确认测试执行已快后，才单独讨论冷编译缓存；本任务不新增缓存系统。

约束：

- `retries = 0` 保持不变。
- 不通过 `#[ignore]`、夜间任务或放宽超时伪造性能改善。
- 资源测试仍在完整持续集成中运行，只允许拆成可归因的独立步骤。
- 进程测试必须有就绪期限、操作期限和回收断言；不用固定等待替代状态信号。
- Nextest 分组必须精确到真正共享资源的测试，不再按整个巨型测试目标粗暴串行。

## 10. 明确不做

- 不修改生产行为来迎合旧测试。
- 不新增测试框架、跨 crate 测试基础库、缓存服务或常驻统计系统。
- 不把所有私有测试迁到 `tests/`，不为测试公开生产内部接口。
- 不用覆盖率百分比或测试数量作为质量目标。
- 不编辑 `.references/`。
- 不重做 Delegate 系列卡片设计，不改变 ShellRuntime 等产品语义。

## 11. 验收标准

### 结构

- 四个 crate 全部符合第 5 节的同一规则。
- 不存在测试专用 `mod.rs`、`tests.rs` 或其他泛化文件名。
- 第 5.9 节的当前超限清单全部迁移完成；所有本轮触及或新增测试文件均不超过硬上限。
- 每个 crate 的重复夹具只在实际发生语义漂移时抽取，且只抽到 crate 内。

### 价值

- 每个删除组都有保留行为说明和精确验证证据。
- 每个语义精简提交都有第 5.10 节的逐组退役记录，且精确命令的实际运行数非零。
- 上下文完整性、持久化、权限、安全、进程、终端和跨平台边界无覆盖空洞。
- 不保留无断言测试、仅验证测试辅助接口的测试或明显派生往返断言。

### 性能

- 同一台本机、相同编译状态、相同命令下，热执行至少快 `60%` 且不超过 `20` 分钟。
- 默认 20 秒慢测阈值内没有未分类慢测。
- 串行组只包含有实证资源冲突的精确测试。
- 当前完整确定性测试在 GitHub Actions 通过；远端结果只证明远端，不替代本机目标。

### 平台

- 普通逻辑由本机精确测试证明。
- Windows、Linux、macOS 专属行为分别由原生环境或对应持续集成证明。
- 任何缺少原生平台证据的结论必须明确标记为未验证。
