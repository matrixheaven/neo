# Neo 测试套件治理执行交接

将本文件、设计说明和实施计划完整交给接手者：

- `docs/aegis/specs/2026-08-07-test-suite-governance-design.md`
- `docs/aegis/plans/2026-08-07-test-suite-governance.md`

设计已经固定。接手者的职责是执行和举证，不是重新设计测试目录、性能口径或删除标准。

## 1. 任务终态

四个 crate 使用同一套测试归属、命名、目录和验证规则；当前所有巨型测试文件完成拆分；低价值重复测试仅在有主要守护证明时删除或合并；本机热执行至少加速 60%，并降到 20 分钟以内。

测试数量下降不是独立目标。若性能目标只能通过删除上下文、持久化、权限、进程、终端、工作流、多代理或平台守护达成，立即停止并报告瓶颈。

## 2. 开始前必须读取

按顺序读取：

1. 根 `AGENTS.md`、`RTK.md`、`CX.md`。
2. 本交接、设计说明、实施计划。
3. `.config/nextest.toml`、`.github/workflows/ci.yml`。
4. `docs/aegis/work/2026-08-07-test-suite-governance/20-checkpoint.md` 和 `90-evidence.md`。
5. 只查看当前批次涉及的生产模块和测试，不重做无边界全仓审计。

先运行：

```bash
icm recall-context "Neo test suite governance local slow tests" --limit 5
git status --short --untracked-files=all
```

工作树可能包含其他人的改动。禁止 `reset`、`restore`、`checkout -- <path>`、`stash`、`clean`、`rebase`、`amend`，不得回退无关文件。

## 3. 不允许接手者决定的事项

以下事项没有自由选择空间：

- 顶层测试目标、旧文件归属和固定子模块使用设计说明第 5.8 节。
- 当前必须处理的测试专用文件和源码侧测试使用第 5.9 节。
- 禁止测试专用 `mod.rs`、`tests.rs`、`test.rs`、`misc.rs`、`common.rs`、`integration.rs`、数字分片。
- 不新增依赖、测试框架、缓存、覆盖率门槛、标签系统、重试、夜间逃逸或长期测试台账。
- 不修改生产行为来让测试通过，不公开私有接口来方便测试。
- 纯移动和语义精简必须分开提交；工作流与多代理也必须分别提交。
- 默认 Nextest 配置是本机性能依据；远端配置和历史持续集成速度不能替代本机数据。
- `git push`、分支切换、合并和标签必须逐命令获得用户授权。

唯一允许由证据决定的事项只有两类：

1. 某个重复测试是否满足删除条件。证据不足就保留，不得猜测。
2. 某个具体测试是否确实争用全局资源。并行复现不足就不得加入串行组。

## 4. 固定执行序列

严格执行实施计划的 16 个提交批次：

1. 根测试规范。
2. `neo-ai` 纯移动。
3. `neo-ai` 语义精简。
4. 核心运行时与会话纯移动。
5. 核心运行时与会话语义精简。
6. 核心工具纯移动。
7. 核心工具语义精简。
8. 核心工作流纯移动。
9. 核心工作流语义精简。
10. 核心多代理纯移动。
11. 核心多代理语义精简。
12. `neo-tui` 纯移动。
13. `neo-tui` 语义精简。
14. `neo-agent` 纯移动。
15. `neo-agent` 语义精简。
16. Nextest 与持续集成调度。

不得让两个执行者同时编辑同一文件。每批结束后由协调者查看差异、运行精确测试、更新工作记录并提交，再开放下一批文件。

## 5. 每批状态机

### 纯移动批次

1. 保存移动前的目标清单、测试数量和三个关键完整测试路径。
2. 只改路径、模块声明、必要的 `use` 和夹具位置。
3. 保存移动后的目标清单；数量和完整名称集合必须一致。
4. 对三个完整测试路径分别执行一次 `cargo test --exact`，实际运行数必须为 1。
5. 确认没有旧入口、转发模块、断言变化或测试数据变化。
6. 提交纯移动。

### 语义精简批次

1. 为每个候选写一行退役记录：删除或合并测试、保留主要守护、共同生产故障、证明方式、精确命令、实际运行数。
2. 若共同故障不能由调用路径直接证明，临时破坏共同生产分支；保留测试必须失败。
3. 撤销临时故障，执行保留测试和当前目标的精确回归。
4. 将退役记录追加到 `docs/aegis/work/2026-08-07-test-suite-governance/90-evidence.md`，并用 `git add -f` 精确暂存该文件；仓库默认忽略 `docs/aegis/work/`，普通 `git add` 不会包含它。
5. 确认提交中没有临时故障，再提交语义精简。

缺任一证据列时必须保留测试。不得用“看起来重复”“年代久”“由 TDD 产生”“当前通过”作为删除依据。

## 6. 本机性能基线

只运行一次完整基线。用全新的独立构建目录，不删除或复用仓库现有 `target`：

```bash
mkdir -p target/test-governance
NEO_TEST_COLD_TARGET="$(mktemp -d "${TMPDIR:-/tmp}/neo-test-governance.XXXXXX")"
printf '%s\n' "$NEO_TEST_COLD_TARGET" > target/test-governance/cold-target-path.txt
export CARGO_TARGET_DIR="$NEO_TEST_COLD_TARGET"
/usr/bin/time -p -o target/test-governance/baseline-cold.time cargo nextest list --workspace --all-features > target/test-governance/baseline-list.txt 2>&1
/usr/bin/time -p -o target/test-governance/baseline-hot.time cargo nextest run --workspace --all-features --status-level all --final-status-level all > target/test-governance/baseline-hot.log 2>&1
/usr/bin/time -p -o target/test-governance/baseline-serial.time cargo nextest run --workspace --all-features --status-level all --final-status-level all -E 'binary(process_guard) | binary(shell_admission_runtime) | binary(tool_bash_guardian) | binary(tool_terminal_guardian) | binary(runtime_turn) | binary(tool_bash)' > target/test-governance/baseline-serial.log 2>&1
```

记录冷发现、热执行、当前串行组、测试发现数、顶层目标数、超过 20 秒项、超时、失败和资源泄漏。命令失败时记录退出码，不用缺失数字计算性能。

最终性能验收重新创建另一个独立目录，重复同样三段。Task 10 必须把更新后串行组的完整过滤表达式写入 `target/test-governance/final-serial-filter.txt`，最终以该表达式生成 `final-serial.time` 和 `final-serial.log`。不得使用 `--profile ci`，不得复用基线目录。完整工作区只在基线和最终各运行一次。

## 7. 精确验证格式

先确认完整名称能发现，再精确运行：

```bash
cargo nextest list -p <crate> --test <target> -E 'test(=<module::test_name>)'
cargo test --package <crate> --test <target> -- <module::test_name> --exact --nocapture
```

源码侧测试使用 `--lib` 或 `--bin neo`，仍必须给完整模块路径和 `--exact`。宽泛子串、包级运行和 `0 tests run` 不算证据。

固定关键守护包括：

- `neo-ai`：`openai_responses::openai_responses_client_posts_responses_payload_and_streams_events`。
- 核心运行时：`context::unchanged_session_keeps_cache_prefix_and_new_context_appends`。
- 核心会话：`jsonl_recovery::jsonl_session_drops_torn_final_line_on_replay`。
- 核心工作流：`runtime_effects::invoke_persists_start_before_effect_and_finish_after_effect`、`runtime_recovery::incomplete_invocation_is_interrupted_and_never_reexecuted`。
- 核心多代理：`progress::older_terminal_progress_cannot_clear_a_newer_outcome`。
- 终端界面：`fullscreen::logical_anchor_survives_growth_removal_resize_and_wrap`、`background_updates::background_delegate_group_updates_offscreen_and_latest_state_is_reachable`。
- 命令入口：`sessions::resume_specific_session_uses_indexed_workspace`、`streaming::rpc_responds_before_stdin_eof_and_accepts_next_request`。

移动后模块路径必须与设计说明一致；不要为保留旧测试名而增加别名入口。

## 8. 不可删除边界

- `neo-ai`：请求形状、敏感认证信息、协议流顺序、工具生命周期、错误分类、缓存前缀和平台环境差异。
- `neo-agent-core`：上下文只追加、会话追加与恢复、权限、路径边界、Shell 等待、工作流持久化与恢复、多代理终态和事件路由。
- `neo-tui`：输入焦点、阻塞弹窗、全屏生命周期、滚动选择、后台更新、终端模式和 Delegate 系列卡片现有展示。
- `neo-agent`：真实命令入口、跨工作区恢复、假提供方端到端、RPC 流、Unix 进程树、Windows 作业对象和上下文追加。

生产行为退役时，测试必须与生产路径同批退役。本任务没有生产路径退役授权，因此旧配置名等兼容测试不能单独删除。

## 9. 已确认的语义候选

候选仍需退役记录，不代表自动删除：

- `neo-ai`：API 类型推断、`Retry-After`、错误代码、推理强度名称可表驱动；固定五秒服务器等待应改为就绪信号。
- 核心：主题保存权限可收敛为权限矩阵加一条运行链路；技能路径拒绝和 MCP 状态可表驱动；仅验证文本非空是弱候选。
- 终端界面：无断言 Markdown 测试、只验证读取辅助接口的测试可删；代码框、主题错误和真实等待存在重复。
- 命令入口：推理配置、权限模式、令牌上限可表驱动；RPC 派生往返是弱候选；四套模拟服务器收敛为一个领域夹具。

两条资源测试预先判定为保留：10,000 条工作流分页守护真实规模边界；12 MiB 输出跨越 10 MiB 旧日志上限。不得缩小、忽略或移到夜间。

## 10. Nextest 与持续集成

- `retries = 0` 和默认 20 秒慢测阈值保持不变。
- 串行组只能列完整测试名，不允许 `binary(...)` 粗粒度成员。
- 每个串行成员必须有共享资源和并行失败证据；没有证据就保持普通并行。
- 远端命令显式使用 `--profile ci`。
- 在新构建目录中不预先构建，精确运行依赖 `CARGO_BIN_EXE_neo` 的命令测试；成功后才删除持续集成中的独立构建步骤，失败则保留并记录原因。
- 不新增缓存、重试、忽略、夜间任务或放宽超时。

## 11. 三平台原生验证

先运行 `vm_stat` 和 `prlctl list`。可用内存不足 8 GiB 不启动虚拟机；一次只运行一个虚拟机。每个平台先记录 `git rev-parse HEAD`，无法证明同一提交就停止。

1. macOS 主机：精确运行 Unix 进程树和全屏转录生命周期主守护。
2. 正常关闭其他虚拟机后启动 `Fedora Linux`，在共享检出中精确运行 Unix 进程树和真实伪终端主守护。
3. 停止 Fedora 并确认状态后启动 `Windows 11`，精确运行作业对象停止、父输入结束回收和 Windows 终端会话主守护。
4. 停止 Windows 并确认状态。记录每台机器的提交、命令、实际运行数、结果和启动停止状态。

不得用交叉编译、Linux 子系统、`prlctl exec` 的非终端行为或旧持续集成结果冒充对应原生语义。

## 12. 停止条件

出现任一情况，停止当前批次并向协调者报告：

- 需要修改生产行为、公开接口、持久化格式或 Delegate 系列展示。
- 需要新增依赖、缓存、重试、兼容分支或新测试框架。
- 删除候选没有更强主要守护，或临时故障注入不能证明共同故障。
- 纯移动前后测试发现集合不一致且无法由条件编译解释。
- 性能目标只能通过删除高风险守护达到。
- 同一文件存在无法安全分离的他人改动。
- 原生平台未运行却准备声称跨平台完成。

## 13. 最终报告

最终报告必须包含：

- 四个 crate 前后的测试定义数、顶层目标数和测试代码行数。
- 每个语义提交的退役短表，以及删除、合并、重写总数。
- 本机冷发现、热执行、资源组前后时间和热执行降幅。
- 所有精确命令、完整测试路径和实际运行数。
- 三个平台的提交与原生结果。
- 每个提交哈希和文件边界。
- 若用户未授权推送，明确写“当前提交远端未验证”；不得复用旧绿色结果。

不能只写“测试变少了”“全量通过了”“远端很快”或“跨平台应该没问题”。
