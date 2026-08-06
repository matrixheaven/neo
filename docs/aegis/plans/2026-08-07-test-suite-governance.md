# Neo 测试套件治理实施计划

> 接手者须知：设计已经固定。直接执行，不要重新发起全仓测试设计讨论，不要按文件大小批量删测试，不要修改生产行为来让旧断言通过。

## Goal

执行 `docs/aegis/specs/2026-08-07-test-suite-governance-design.md`，在四个 crate 中删除低价值测试、合并同分支案例、拆分巨型文件、统一命名和目录，并将用户本机热执行时间降低至少 `60%` 且降到 `20` 分钟以内。

## Architecture

使用 Rust 原生测试、Cargo、Nextest、现有假模型、现有工作流夹具和 `tempfile`。私有逻辑测试留在源码侧；跨模块行为进入 `tests/`；顶层领域入口加嵌套行为模块，避免巨型文件和过多测试二进制。

## Tech Stack

Rust 2024、Cargo、cargo-nextest、现有 GitHub Actions。不得新增依赖。

## Baseline / Authority Refs

- `AGENTS.md`
- `RTK.md`
- `CX.md`
- `.config/nextest.toml`
- `.github/workflows/ci.yml`
- `docs/aegis/specs/2026-08-07-test-suite-governance-design.md`

## Compatibility Boundary

- 不改变任何生产行为、公开接口、持久化格式、上下文顺序或产品展示。
- Context cache prefix、系统提示、历史对话和规范记录保持只追加。
- Delegate、DelegateGroup、DelegateSwarm 卡片保持现状。
- Bash 和 Terminal admission 等待保持 pending；无显式超时或取消的命令仍可无限等待。
- 平台专属行为不得被跨平台替代测试覆盖掉。
- 不触碰无关脏文件，不回退任何其他人的工作。

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: 整理、删重和回归验证，不新增产品行为。
- Reason: 本任务治理现有测试；严格 RED/GREEN 会继续制造临时测试。
- Verification: 每个删除组使用保留测试、必要时临时故障注入和精确 Nextest 目标证明。

## Verification

- 结构变化先用 `cargo nextest list` 对比测试发现结果。
- 功能证据必须满足一个包、一个 `--lib`、`--bin` 或 `--test` 目标，以及一个完整测试路径。先用 Nextest 的 `test(=完整路径)` 确认发现数，再用 Cargo 的 `--exact` 运行；宽泛子串和 `0 tests run` 一律无效。
- 工作区完整测试只运行两次：一次本机性能基线，一次最终性能验收。它不能替代精确功能证据。
- 文档和配置使用 `cargo fmt --all --check`、`git diff --check` 和精确配置核对。

### 每批固定动作

每个 crate 的每个批次必须按以下顺序执行，不得合并步骤：

1. 在移动前保存该目标的 `cargo nextest list` 输出、发现数量和三个关键完整测试路径。
2. 纯移动测试、夹具和模块声明，不改测试名、断言、数据规模、等待方式或生产代码。
3. 再次列出测试；发现数量和完整名称集合必须一致。条件编译导致的差异必须逐项解释。
4. 用三个关键完整路径执行 `cargo test --exact`，确认每条实际运行一个测试。
5. 提交纯移动批次。
6. 另开语义精简批次；每组删除或合并先填写设计说明第 5.10 节短表。
7. 高风险重复无法仅凭调用路径证明时，临时破坏共同生产分支；保留测试必须失败。随后撤销临时故障，再执行精确测试并确认通过。
8. 检查差异中没有临时故障、旧入口、转发模块、数字分片或泛化文件名，再提交语义精简批次。

纯移动提交允许变化的只有路径、`mod`/`#[path]` 声明、可见性所必需的 `use` 路径和 crate 内测试夹具路径。任何断言、测试数据或行为变化都必须移到后续语义精简提交。

## Requirement Ready Check

- 需求来源：用户本轮四项要求和设计说明。
- 范围：四个 crate、根测试规范、Nextest、持续集成测试步骤。
- 场景：本机完整测试约一小时，远端明显更快。
- 验收：结构、价值、性能、平台四组标准已固定。
- 未决问题：无。具体删除名单必须由每个任务的行为映射决定。
- 决定：`ready`

## Change Necessity

- 用户可见需要：本机测试反馈过慢，测试维护成本过高。
- 不改代码选项：只写规范无法消除重复、巨型文件、固定等待和粗粒度串行。
- 必要性：必须编辑测试代码和测试运行配置；生产代码只允许移动内联测试声明，不允许行为变化。
- 最小边界：测试文件、测试模块声明、`AGENTS.md`、`.config/nextest.toml`、`.github/workflows/ci.yml`。
- 决定：`code-change`

## Execution Readiness View

- Intent Lock: 少而强的长期行为守护，本机性能优先。
- Scope Fence: 只改测试、测试声明和运行配置；发现生产缺陷时另开任务。
- Baseline Lock: 以当前本机分段计时和静态 3,519 个测试定义为起点。
- Approved Behavior: 设计说明第 8 节不可删除边界。
- Owner Constraints: 私有测试归源码模块，公开行为归 crate 的 `tests/`。
- Compatibility Boundary: 本计划前述边界全部保持。
- Retirement Boundary: 删除旧测试、旧夹具和旧泛化文件名，不保留双轨入口。
- Task Batches: 基线、规范、四个 crate 的结构与删重、运行配置、最终验收。
- Test Obligations: 每个删除组有保留测试证据；平台行为有原生证据。
- Review Gates: 每个 crate 先纯移动，再语义精简；两者不得混成一个提交。
- Drift Rules: 需要修改生产行为、新增依赖或隐藏慢测时立即停止。
- Evidence Required: 文件映射、测试发现计数、精确测试、前后本机耗时、远端状态。

## 固定提交序列

执行者必须按下列顺序提交。某批没有可删除项时仍需在工作记录写“零项及原因”，然后跳过该语义提交；不得把它并入相邻提交。

| 顺序 | 批次 | 允许内容 |
|---:|---|---|
| 1 | 根规范 | 仅 `AGENTS.md` 测试规则 |
| 2 | `neo-ai` 纯移动 | 路径、模块声明、夹具去重所需的机械移动 |
| 3 | `neo-ai` 语义精简 | 已有退役短表的删除、表驱动合并、等待重写 |
| 4 | 核心运行时与会话纯移动 | `runtime_behavior`、`session_behavior` |
| 5 | 核心运行时与会话语义精简 | 仅经证明的重复与弱断言 |
| 6 | 核心工具纯移动 | `tool_behavior` 与相邻源码测试提取 |
| 7 | 核心工具语义精简 | 技能、MCP、权限的已证明重复 |
| 8 | 核心工作流纯移动 | `workflow_behavior` |
| 9 | 核心工作流语义精简 | 同状态机分支的已证明重复 |
| 10 | 核心多代理纯移动 | `multi_agent_behavior` |
| 11 | 核心多代理语义精简 | 不触及持久化、事件路由和终态守护 |
| 12 | `neo-tui` 纯移动 | 七个固定顶层目标及源码侧测试提取 |
| 13 | `neo-tui` 语义精简 | 展示重复、真实等待和弱断言 |
| 14 | `neo-agent` 纯移动 | 四个固定顶层目标及源码侧测试提取 |
| 15 | `neo-agent` 语义精简 | 配置矩阵、RPC 弱往返和重复服务器 |
| 16 | 测试调度 | `.config/nextest.toml`、`.github/workflows/ci.yml` |

每个语义提交前，把设计说明第 5.10 节的短表追加到 `docs/aegis/work/2026-08-07-test-suite-governance/90-evidence.md`。`docs/aegis/work/` 被仓库忽略，因此必须对该精确文件执行 `git add -f docs/aegis/work/2026-08-07-test-suite-governance/90-evidence.md`，再与对应提交一起暂存；不能事后批量补写，也不能使用宽泛 `git add -f docs/aegis/work`。

## Task 1：记录本机真实性能基线

**文件：** 不提交基线日志；输出写入 `target/test-governance/`。

**为什么：** 远端约 154.5 秒的历史样本不能解释本机一小时。先分离冷编译、热执行和串行资源组。

**步骤：**

1. 记录当前提交、系统信息、可用内存、Cargo/Rust/Nextest 版本和现有 `target` 状态。
2. 创建日志目录，并用 `mktemp -d` 创建全新的冷构建目录。不得复用仓库现有 `target`，也不得通过删除用户的 `target` 制造冷环境：

   ```bash
   mkdir -p target/test-governance
   NEO_TEST_COLD_TARGET="$(mktemp -d "${TMPDIR:-/tmp}/neo-test-governance.XXXXXX")"
   printf '%s\n' "$NEO_TEST_COLD_TARGET" > target/test-governance/cold-target-path.txt
   ```

3. 用默认 Nextest 配置计时冷构建与发现；不使用 `--profile ci`：

   ```bash
   export CARGO_TARGET_DIR="$NEO_TEST_COLD_TARGET"
   /usr/bin/time -p -o target/test-governance/baseline-cold.time \
     cargo nextest list --workspace --all-features \
     > target/test-governance/baseline-list.txt 2>&1
   ```

4. 使用同一已编译目录运行一次热执行基线，并保存完整状态：

   ```bash
   /usr/bin/time -p -o target/test-governance/baseline-hot.time \
     cargo nextest run --workspace --all-features \
       --status-level all --final-status-level all \
     > target/test-governance/baseline-hot.log 2>&1
   ```

5. 使用相同编译目录和当前串行表达式独立计时资源组：

   ```bash
   /usr/bin/time -p -o target/test-governance/baseline-serial.time \
     cargo nextest run --workspace --all-features \
       --status-level all --final-status-level all \
       -E 'binary(process_guard) | binary(shell_admission_runtime) | binary(tool_bash_guardian) | binary(tool_terminal_guardian) | binary(runtime_turn) | binary(tool_bash)' \
     > target/test-governance/baseline-serial.log 2>&1
   ```

6. 记录每段 `real` 时间、发现测试数、测试二进制数、超过 20 秒项、超时、失败和资源泄漏。任一命令失败时保留退出码与日志，不继续用缺失数字计算降幅。
7. 最终测量必须重新创建另一个全新冷目录，并重复完全相同的三条命令；基线与最终都使用默认配置。热执行降幅按 `(基线热执行 - 最终热执行) / 基线热执行` 计算。
8. 若热执行并不慢而冷阶段占主导，仍完成结构和价值治理，但把缓存或编译优化列为独立后续；不得在本任务新增缓存。

**提交：** 无。基线是诊断证据，不是仓库资产。

## Task 2：写入四个 crate 共用的测试规范

**修改：** `AGENTS.md`

**为什么：** 规范必须只有一个根级入口，不能四个 crate 各写一份。

**步骤：**

1. 将设计说明第 5、6、7、9 节压缩写入 `AGENTS.md` 的测试规则。
2. 明确禁止测试专用 `mod.rs`、`tests.rs`、泛化文件名、无证据删除、固定等待和重试掩盖。
3. 明确测试文件规模上限、顶层领域入口和嵌套行为文件模式。
4. 写入“行为归谁、最低哪一层、是否已有主要守护”的三问决策路径，以及单元、crate 行为、产品边界、平台、资源五类测试。
5. 写入新增测试、缺陷回归、抖动测试和生产行为退役的生命周期规则。
6. 不添加新脚本、标签系统、覆盖率门槛、测试布局检查器或长期台账；依靠根规则和代码复核。
7. 把设计说明第 5.8、5.9 节作为本轮迁移清单引用，不把一次性路径清单复制进永久根规范。
8. 运行：

   ```bash
   rtk proxy git diff --check -- AGENTS.md
   ```

**提交：** `docs: define unified test suite rules`

## Task 3：整理 `neo-ai` 测试结构

**主要文件：**

- 设计说明第 5.8 节规定的三个最终顶层目标及同名目录。
- `crates/neo-ai/src/providers/common/error.rs` 旁的 `test_cases/`。

**结构轨：**

1. 严格按第 5.8 节把六个旧目标收敛为 `provider_protocol_behavior`、`model_resolution_behavior`、`environment_behavior`；旧目标迁空后删除。
2. 回环服务器只保留 `provider_protocol_behavior/http_server.rs` 一个实现；请求捕获属于该文件，不再创建第二个通用夹具。
3. 将 `src/providers/common/error.rs` 的测试按 `classification.rs`、`retry_after.rs` 拆到相邻 `test_cases/`。
4. 按“每批固定动作”完成纯移动，发现集合必须完全相同。

**精简轨：**

1. 表驱动合并 API 类型推断、`Retry-After`、错误代码映射和推理强度名称。
2. 用强分片工具调用测试评估较弱单帧重复；高风险时做临时故障注入。
3. 删除仅重复证明响应格式省略的弱测试。
4. 将后台固定 5 秒等待改成就绪信号或暂停时间。
5. 保留四种协议各自的解析输入，只共享终止状态断言，不把协议接线删成单元测试。

**验证示例：**

```bash
cargo nextest list -p neo-ai --test provider_protocol_behavior -E 'test(=openai_responses::openai_responses_client_posts_responses_payload_and_streams_events)'
cargo test --package neo-ai --test provider_protocol_behavior -- openai_responses::openai_responses_client_posts_responses_payload_and_streams_events --exact --nocapture
cargo nextest list -p neo-ai --test provider_protocol_behavior -E 'test(=openai_compatible::openai_compatible_client_finishes_tool_call_on_tool_calls_finish_reason_without_done)'
cargo test --package neo-ai --test provider_protocol_behavior -- openai_compatible::openai_compatible_client_finishes_tool_call_on_tool_calls_finish_reason_without_done --exact --nocapture
```

**提交：** 结构和精简各一个提交。

## Task 4：拆分 `neo-agent-core` 运行时与上下文测试

**主要文件：**

- `crates/neo-agent-core/tests/runtime_turn.rs`
- 新的 `crates/neo-agent-core/tests/runtime_behavior.rs`
- 新的 `crates/neo-agent-core/tests/runtime_behavior/*.rs`
- `crates/neo-agent-core/tests/session_jsonl.rs`
- 设计说明第 5.8 节的 `runtime_behavior.rs`、`session_behavior.rs` 及同名目录。
- `.config/nextest.toml` 只改旧二进制名称，不在本任务改变串行成员。

**结构轨：**

1. 将 13,091 行运行时测试按第 5.8 节固定模块拆进 `runtime_behavior/`，不得新增类别。
2. 顶层入口只声明模块；共享夹具按职责拆分，不建立通用测试框架。
3. 保留缓存前缀、只追加、压缩恢复、显示内容不进入模型上下文等测试原文语义。
4. 把 Nextest 的旧 `runtime_turn` 名称机械替换为 `runtime_behavior`，串行表达式的语义保持不变。
5. 把 `session_jsonl`、`session_state`、`session_tree`、`instruction_registry` 迁入 `session_behavior`；兼容读取与真实追加恢复测试不得合并。

**精简轨：**

1. 主题保存权限保留一组权限矩阵和一条完整运行链路，删除第三层重复。
2. 删除只验证文本非空的弱断言，前提是更强模型可见性或注册表测试能捕获清空故障。
3. 会话持久化往返测试只在确实仅验证派生类型时删除；经过真实文件恢复的测试保留。
4. 检查 `JsonlSessionWriter` 夹具是否持锁等待自身，必须显式释放 seed，不能放宽锁语义。

**验证示例：**

```bash
cargo nextest list -p neo-agent-core --test runtime_behavior -E 'test(=context::unchanged_session_keeps_cache_prefix_and_new_context_appends)'
cargo test --package neo-agent-core --test runtime_behavior -- context::unchanged_session_keeps_cache_prefix_and_new_context_appends --exact --nocapture
cargo nextest list -p neo-agent-core --test session_behavior -E 'test(=jsonl_recovery::jsonl_session_drops_torn_final_line_on_replay)'
cargo test --package neo-agent-core --test session_behavior -- jsonl_recovery::jsonl_session_drops_torn_final_line_on_replay --exact --nocapture
```

**提交：** 结构和精简各一个提交。

## Task 5：整理 `neo-agent-core` 工具测试

**主要文件：**

- 设计说明第 5.8 节的 `tool_behavior.rs` 及同名目录。
- `crates/neo-agent-core/src/tools/skills_manager.rs`
- `crates/neo-agent-core/src/tools/mcp_manager.rs`
- 设计说明第 5.9 节列出的其他源码侧工具测试。

**步骤：**

1. 纯移动八个工具目标到 `tool_behavior` 的固定子模块；旧目标迁空后删除。
2. 源码侧超过 12 个测试的工具模块只提取到相邻 `test_cases/`，不与顶层行为测试混合。
3. 语义精简仅处理技能路径拒绝、MCP 状态字符串和提示分类的同分支案例；每组单独填写退役记录。
4. 五处短小 `launch_request` 夹具保持局部，不建立跨模块构造器。

**验证示例：**

```bash
cargo nextest list -p neo-agent-core --test tool_behavior -E 'test(=permissions::theme_draft_never_accepts_file_write_in_place_of_tool_access)'
cargo test --package neo-agent-core --test tool_behavior -- permissions::theme_draft_never_accepts_file_write_in_place_of_tool_access --exact --nocapture
```

**提交：** 工具纯移动一个提交，工具语义精简一个提交。

## Task 6：整理 `neo-agent-core` 工作流测试

**文件：** 所有 `crates/neo-agent-core/tests/workflow_*.rs`，以及最终 `workflow_behavior.rs` 和 `workflow_behavior/`。

**纯移动轨：**

1. 严格按设计说明第 5.8 节的展开清单迁移；现有文件名已经准确的，子模块继续使用该职责名。
2. `workflow_runtime.rs` 只能拆为 `runtime_lifecycle.rs`、`runtime_effects.rs`、`runtime_recovery.rs`，不得自行新增“杂项”模块。
3. 先持久化后副作用、重放不重复执行、异常收尾、子任务终态和模型可见结果的测试原样保留。
4. 纯移动后删除全部旧 `workflow_*.rs` 顶层目标。

**语义精简轨：**

1. 只合并同一状态机分支、相同断言而输入枚举不同的案例。
2. `runtime_effects::invoke_persists_start_before_effect_and_finish_after_effect`、`runtime_recovery::incomplete_invocation_is_interrupted_and_never_reexecuted` 必须保留为独立主守护。
3. 不修改工作流生产代码、持久化格式、恢复语义或卡片展示。

**精确验证：**

```bash
cargo nextest list -p neo-agent-core --test workflow_behavior -E 'test(=runtime_effects::invoke_persists_start_before_effect_and_finish_after_effect)'
cargo test --package neo-agent-core --test workflow_behavior -- runtime_effects::invoke_persists_start_before_effect_and_finish_after_effect --exact --nocapture
cargo nextest list -p neo-agent-core --test workflow_behavior -E 'test(=runtime_recovery::incomplete_invocation_is_interrupted_and_never_reexecuted)'
cargo test --package neo-agent-core --test workflow_behavior -- runtime_recovery::incomplete_invocation_is_interrupted_and_never_reexecuted --exact --nocapture
```

**提交：** 工作流纯移动一个提交，工作流语义精简一个提交。

## Task 7：整理 `neo-agent-core` 多代理测试

**文件：** 四个 `multi_agent_*.rs`，以及最终 `multi_agent_behavior.rs` 和同名目录。

**纯移动轨：** 按固定模块迁移；生命周期、进度、事件路由、使用量、取消和调度保持独立。`multi_agent_runtime.rs` 中的测试不得因文件太大而删除。

**语义精简轨：** 只合并展示名等纯输入矩阵；持久化恢复、晚到事件、原子终态和取消测试必须保留。

**精确验证：**

```bash
cargo nextest list -p neo-agent-core --test multi_agent_behavior -E 'test(=progress::older_terminal_progress_cannot_clear_a_newer_outcome)'
cargo test --package neo-agent-core --test multi_agent_behavior -- progress::older_terminal_progress_cannot_clear_a_newer_outcome --exact --nocapture
cargo nextest list -p neo-agent-core --test multi_agent_behavior -E 'test(=lifecycle::resumed_child_turn_replays_prior_messages_from_agent_wire)'
cargo test --package neo-agent-core --test multi_agent_behavior -- lifecycle::resumed_child_turn_replays_prior_messages_from_agent_wire --exact --nocapture
```

**提交：** 多代理纯移动一个提交，多代理语义精简一个提交。

## Task 8：整理 `neo-tui` 测试

**主要文件：**

- `crates/neo-tui/tests/multi_agent_transcript.rs`
- `crates/neo-tui/tests/tool_cards.rs`
- `crates/neo-tui/tests/transcript_pane.rs`
- `crates/neo-tui/tests/workflow_transcript.rs`
- `crates/neo-tui/tests/app_shell.rs`
- `crates/neo-tui/tests/transcript_store.rs`
- 相关内联测试模块

**结构轨：**

1. 严格建立设计说明第 5.8 节的七个顶层领域入口；26 个旧入口迁空后删除。
2. 将巨型正文拆入嵌套行为文件，不改变 Delegate 系列卡片断言。
3. 合并重复的 `rendered`、宽度和事件构造夹具，只在领域内复用。

**精简轨：**

1. 删除无断言 Markdown 测试和只验证测试读取接口的测试。
2. Markdown 代码框只保留一个完整展示案例，其他层验证纯解析或宽度新增风险。
3. 主题无效条目在内联层保留状态转换，集成层只保留焦点路由和可见错误。
4. 将输入选择的真实等待改为显式时间参数。
5. 表驱动合并计划退出状态、MCP 结果状态、主题断点和字符输入映射。
6. 精确快照只允许终端转义协议和冻结的 Delegate 系列卡片；其他展示断言改为文本语义、顺序和显示宽度。

**验证示例：**

```bash
cargo nextest list -p neo-tui --test agent_transcript_behavior -E 'test(=background_updates::background_delegate_group_updates_offscreen_and_latest_state_is_reachable)'
cargo test --package neo-tui --test agent_transcript_behavior -- background_updates::background_delegate_group_updates_offscreen_and_latest_state_is_reachable --exact --nocapture
cargo nextest list -p neo-tui --test transcript_behavior -E 'test(=fullscreen::logical_anchor_survives_growth_removal_resize_and_wrap)'
cargo test --package neo-tui --test transcript_behavior -- fullscreen::logical_anchor_survives_growth_removal_resize_and_wrap --exact --nocapture
cargo nextest list -p neo-tui --test app_behavior -E 'test(=blocking_dialogs::task_browser_overlay_blocks_prompt_and_renders_own_footer)'
cargo test --package neo-tui --test app_behavior -- blocking_dialogs::task_browser_overlay_blocks_prompt_and_renders_own_footer --exact --nocapture
```

**提交：** 结构和精简各一个提交。

## Task 9：整理 `neo-agent` 测试

**主要文件：**

- `crates/neo-agent/src/modes/interactive/tests.rs`
- `crates/neo-agent/src/modes/run/mod.rs`
- `crates/neo-agent/tests/cli_commands.rs`
- `crates/neo-agent/tests/mock_provider_e2e.rs`
- `crates/neo-agent/tests/rpc_mode.rs`
- `crates/neo-agent/tests/fullscreen_output.rs`
- `crates/neo-agent/tests/workflow_cli.rs`

**结构轨：**

1. 在 `interactive/mod.rs` 使用显式 `#[path = "test_cases/<behavior>.rs"]` 声明输入、会话、工作流、主题、任务、审批、复制和转录测试；删除旧巨型 `tests.rs`。
2. 将 `run/mod.rs` 的测试固定拆为相邻 `test_cases/session.rs`、`context.rs`、`stream.rs`、`output.rs`。
3. 将四套模拟响应服务器收敛为 `tests/cli_behavior/http_server.rs`，不得使用 `support/mod.rs`。
4. 将 `cli_commands.rs` 中的工作流测试移入 `workflow_behavior/cli.rs`。
5. 将 `fullscreen_output.rs` 并入 `cli_behavior/fullscreen_output.rs`，删除其独立顶层目标和重复服务器。
6. 其余 11 个旧顶层文件严格按设计说明第 5.8 节收敛为四个最终目标。

**精简轨：**

1. 表驱动合并推理配置、权限模式和令牌上限。
2. 合并两个实际只证明输出顺序的工作流命令测试，并按真实证明能力改名。
3. 删除 RPC 派生类型回原结构的重复断言，保留字段名和缺失字段边界。
4. 旧配置名测试只与对应生产兼容路径同批退出；本任务不得单独删除测试却留下旧生产路径。
5. 保留真实进程、session 恢复、RPC 流、Unix 进程树和 Windows 作业对象行为。

**验证示例：**

```bash
cargo nextest list -p neo-agent --test cli_behavior -E 'test(=sessions::resume_specific_session_uses_indexed_workspace)'
cargo test --package neo-agent --test cli_behavior -- sessions::resume_specific_session_uses_indexed_workspace --exact --nocapture
cargo nextest list -p neo-agent --test rpc_behavior -E 'test(=streaming::rpc_responds_before_stdin_eof_and_accepts_next_request)'
cargo test --package neo-agent --test rpc_behavior -- streaming::rpc_responds_before_stdin_eof_and_accepts_next_request --exact --nocapture
```

**提交：** 结构和精简各一个提交。

## Task 10：精确化 Nextest 与持续集成

**修改：**

- `.config/nextest.toml`
- `.github/workflows/ci.yml`

**步骤：**

1. 用 Task 1 的本机证据移除 `shell-guardian` 对整个 `runtime_turn`、`tool_bash` 等二进制的粗粒度串行；只保留实证会争用系统资源的精确测试。
2. 保持 `retries = 0`、默认 20 秒慢测阈值和现有真正慢测的明确说明。
3. 先建立当前串行成员表。每个成员记录共享资源、单独耗时、与另一个成员并行时是否失败。只允许真实共享全局 Guardian 容量、环境或终端资源的完整测试名进入串行组；禁止再次使用 `binary(...)`。
4. `child_pages_cover_thousand_and_ten_thousand_rows_with_stable_cursor` 的 10,000 条记录是明确分页边界；`complete_agent_output_survives_preview_queue_pressure` 的 12 MiB 是跨越 10 MiB 旧日志上限的必要数据。两者保留为资源测试，不缩小、不忽略、不重试，并使用精确慢测覆盖。
5. CI 测试命令显式使用 `--profile ci`。
6. 在全新 `CARGO_TARGET_DIR` 中不预先执行 `cargo build`，精确运行 `cli_behavior::commands::root_command_reports_interactive_entrypoint_without_placeholders`。只有它能找到并执行 `CARGO_BIN_EXE_neo` 时，才删除重复的独立构建步骤；否则保留该步骤并记录证据。
7. 不加缓存、不加重试、不把资源测试移到夜间。
8. 将最终保留的完整串行过滤表达式原样写入未提交文件 `target/test-governance/final-serial-filter.txt`；文件内容必须与 `.config/nextest.toml` 中 `shell-guardian` 的精确成员一致，供 Task 11 独立计时。

**验证：**

```bash
cargo nextest list --workspace --all-features
git diff --check -- .config/nextest.toml .github/workflows/ci.yml
```

**提交：** `ci: streamline deterministic test execution`

## Task 11：最终本机与平台验收

**步骤：**

1. 运行所有变更目标的精确测试，确认没有 `0 tests run`。
2. 运行格式和差异检查：

   ```bash
   rtk cargo fmt --all --check
   rtk proxy git diff --check
   ```

3. 按 Task 1 的命令创建新的独立冷构建目录，依次测量最终冷发现、最终热执行和最终资源组；不得复用基线冷目录：

   ```bash
   NEO_TEST_FINAL_TARGET="$(mktemp -d "${TMPDIR:-/tmp}/neo-test-governance-final.XXXXXX")"
   export CARGO_TARGET_DIR="$NEO_TEST_FINAL_TARGET"
   /usr/bin/time -p -o target/test-governance/final-cold.time cargo nextest list --workspace --all-features > target/test-governance/final-list.txt 2>&1
   /usr/bin/time -p -o target/test-governance/final-hot.time cargo nextest run --workspace --all-features --status-level all --final-status-level all > target/test-governance/final-hot.log 2>&1
   NEO_TEST_FINAL_SERIAL_FILTER="$(< target/test-governance/final-serial-filter.txt)"
   test -n "$NEO_TEST_FINAL_SERIAL_FILTER"
   /usr/bin/time -p -o target/test-governance/final-serial.time cargo nextest run --workspace --all-features --status-level all --final-status-level all -E "$NEO_TEST_FINAL_SERIAL_FILTER" > target/test-governance/final-serial.log 2>&1
   ```

4. 对最终最慢测试和资源组再运行一次精确目标，排除偶然波动。
5. 计算热执行降幅；必须达到至少 `60%` 且不超过 `20` 分钟。
6. 不得自行推送。先提交本地结果，再向用户请求一次具体 `git push` 命令授权；只有获授权并推送后，才查看当前提交的 GitHub Actions。未授权时将远端状态明确记为“未验证”。
7. 按下方原生平台矩阵验证。未运行的平台必须明确报告，不能由其他平台或旧持续集成结果替代。
8. 若性能目标未达到，提交瓶颈排序和保留理由，停止继续删高价值测试。

### 原生平台矩阵

开始前在 macOS 主机运行 `vm_stat` 和 `prlctl list`。可用内存低于 8 GiB 时不启动虚拟机；任何时刻只能有一个虚拟机运行。每个平台都必须在同一提交上先执行 `git rev-parse HEAD` 并记录结果；共享目录无法证明同一提交时立即停止。

| 平台 | 固定目标 | 必跑的完整测试 |
|---|---|---|
| macOS 主机 | `neo-agent/process_behavior`、`neo-tui/transcript_behavior` | `process_guard_unix::process_guard_parent_eof_kills_bash_descendant`、`fullscreen::fullscreen_lifecycle_enters_and_restores_once` |
| Fedora 虚拟机 | `neo-agent/process_behavior` | `process_guard_unix::process_guard_parent_eof_kills_bash_descendant`、`terminal_guardian::terminal_tool_start_write_read_resize_and_stop_uses_real_pty` |
| Windows 11 虚拟机 | `neo-agent/process_behavior` | `process_guard_windows::windows_terminal_stop_closes_job_with_descendant`、`process_guard_windows::windows_parent_eof_closes_assigned_job_with_descendant`、`terminal_guardian::terminal_windows_session_remains_usable_without_signal_guarantee` |

执行顺序固定如下：

1. 在 macOS 主机运行表中两条完整测试，命令均使用 `cargo test --package <crate> --test <target> -- <完整路径> --exact --nocapture`。
2. 查看内存和 `prlctl list`。若 Windows 正在运行，先正常关机并确认状态为 `stopped`；然后 `prlctl start "Fedora Linux"`。
3. 在 Fedora 的既有共享检出中记录绝对路径和提交，运行表中两条精确测试。不得使用本机交叉编译结果冒充原生运行。
4. `prlctl stop "Fedora Linux"`，轮询 `prlctl list` 直到 Fedora 为 `stopped`，再执行 `prlctl start "Windows 11"`。
5. 在 Windows 的既有共享检出中记录绝对路径和提交，用 PowerShell 或 Git Bash 运行表中三条精确测试。不得用 Linux 子系统或交叉编译替代。
6. `prlctl stop "Windows 11"` 并确认状态为 `stopped`。最终报告记录启动、停止、提交、命令、实际运行数和结果。

若执行者改动了其他带 `cfg(target_os)`、`cfg(unix)` 或 `cfg(windows)` 的测试，还必须把对应完整测试加入该平台，不得删减上表固定目标。

**最终证据：**

- 前后本机冷阶段、热执行、资源组时间。
- 测试定义和顶层测试二进制前后数量。
- 删除、合并、重写数量及对应行为类别。
- 所有精确测试命令和实际运行数量。
- 当前提交的远端状态和原生平台缺口。

## 风险与回退

- 纯移动提交可独立回退；语义精简提交不得与移动混合。
- 删除后发现覆盖空洞时，恢复行为守护到新的规范位置，不恢复旧巨型结构。
- 测试失败若揭示生产缺陷，保留失败证据并另开修复任务；本计划不修产品。
- 任何需要新增依赖、公开私有接口或修改持久化格式的方案都超出范围。

## Execution Route

- Decision: `subagent-driven`
- Evidence: 四个 crate 的结构治理基本独立，仓库规则要求重大工作至少三个子代理。
- Fallback: 同一协调者按 crate 顺序执行，每个任务保持独立提交。
- User confirmation required: 本地实施和提交不需要额外确认；启动和关闭按项目规则执行；`git push`、分支切换、合并和标签始终需要逐命令授权。
