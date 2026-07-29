# 内置工具参考

Neo 通过 `ToolRegistry` 向模型暴露一组内置工具。本文按类别列出全部内置工具及其用途，供 Skill / prompt / 调试参考。

源码位置：[`crates/neo-agent-core/src/tools/`](../../../crates/neo-agent-core/src/tools/)，规范名来源 `Tool::name()`。

## 文件操作

| 工具 | 用途 |
| --- | --- |
| `Read` | 读取 UTF-8 文本文件，支持按行偏移分页读取。 |
| `Write` | 通过 `path` 和 `content` 创建或完整覆盖一个工作区内 UTF-8 文件。写入前完成 prepare 与重新检查，再执行一次原子安装。已有目标必须是 UTF-8 常规文件；拒绝二进制文件、链接、目录和无变更覆盖。多个独立文件由模型在同一响应中发出多个 `Write` 调用。 |
| `Edit` | 通过 `path`、`old`、`new` 与可选 `expected_matches`（默认 1）对一个已有 UTF-8 文件做一次精确文本替换。写入前完成 prepare 与重新检查，再执行一次原子替换。不创建文件；多个独立修改由模型在同一响应中发出多个 `Edit` 调用。 |
| `List` | 以两层树形列出目录内容。 |
| `Glob` | 按 glob 模式匹配文件/目录路径，按修改时间排序。 |
| `Find` | 按文件/目录名子串查找工作区路径。 |
| `Grep` | 基于正则搜索工作区文本文件内容。 |

### Edit 暂存与提交合同

`Edit` 只接收一个对象：`path`、`old`、`new` 与可选 `expected_matches`（默认
`1`）。写入前，Neo 解析并读取已有 UTF-8 常规文件且不跟随链接，验证精确匹配数，
暂存替换并生成审批 diff。Ask 模式中用户批准这份已验证 diff。随后 Neo 重新检查目标
与内容，再原子替换文件。prepare、stale 或提交前取消均保证零写入。
`durability_uncertain` 表示内容已安装但无法确认父目录持久化；再次调用前应重新读取文件。
创建文件或完整替换请使用 `Write`。

### Write 暂存与提交合同

`Write` 只接收一个包含 `path` 与完整 UTF-8 `content` 的对象。写入前，Neo 解析并
分类目标，拒绝不安全或无变更的覆盖，并生成审批投影。Ask 模式中用户批准已验证的完整
内容或 diff。随后 Neo 重新检查目标，再执行一次原子创建或替换。缺失父目录只在提交期间
创建。prepare、stale 或提交前取消均保证零写入。结果报告 `created_directories`；
`durability_uncertain` 表示内容已安装但无法确认父目录持久化。

## Shell

| 工具 | 用途 |
| --- | --- |
| `Bash` | 在工作区执行非交互式 `bash`（Windows 上为 Git Bash）命令；标准输入始终关闭，需要提示输入、终端状态、按键或控制字节时应使用 `Terminal`。支持管道、后台任务、可选 `timeout_secs` 与取消。省略 `timeout_secs` 表示不设超时；显式值必须在 `300..=3600`。超时后应增大或翻倍再重试；若已为 `3600` 或耗时无法确定，则省略。 |
| `Terminal` | 操作一个真实 PTY 会话：start / write / read / resize / stop，适合交互式长进程。`start` / `write` / `read` 共用可选 `yield_time_ms`（默认 250 / 250 / 3000 ms，范围 `0..=30000`），在 admission 成功且操作就绪后等待增量 **原始 PTY** 输出；到期仅返回当前输出且 `status: running`，绝不停止命令。admission 队列等待仍无限，原 Tool Use 保持 pending。`timeout_secs` 仅对 `mode=start` 有效；省略表示不设命令生命周期截止，否则必须在 `300..=3600`。超时后应增大或翻倍再重试；若已为 `3600` 或耗时无法确定，则省略。不过滤 echo、ANSI、CR、光标控制。`write` 的 `input` 是非空有序数组，例如 `[{"text":"command text"},{"control":3}]`：`text` 发送 UTF-8，并将 LF 和 CRLF 规范化为 CR；`control` 发送 `0..=31` 或 `127` 的精确字节（Ctrl+C `3`、Ctrl+D `4`、Ctrl+Z `26`、Escape `27`）。各项由一次工具调用按数组顺序发送；`{"text":"\\u0003"}` 会原样发送可打印的转义文本。精确 PTY 控制字节不保证可移植的 signal 行为：含义由接收程序决定，Windows ConPTY 行为取决于接收端；远程会话不确定是否分配 PTY 时应使用 `ssh -tt`。 |

## 网络

| 工具 | 用途 |
| --- | --- |
| MCP 工具 | 动态注册，命名形如 `mcp__<server_id>__<tool_name>`，由 `mcp_manager.rs` 管理。非内置工具。 |

> Neo 内置工具集不直接提供 HTTP 抓取工具；联网能力通过 Bash（`curl`/`wget`）或用户配置的 MCP server 提供。

## 计划模式（Plan Mode）

| 工具 | 用途 |
| --- | --- |
| `EnterPlanMode` | 进入计划模式（只读调研 / 规划），不直接改代码。 |
| `ExitPlanMode` | 计划写完后退出计划模式并请求用户审批。 |

## 目标（Goal）

由 `GoalManager` 注册，启用目标模式时可用。

| 工具 | 用途 |
| --- | --- |
| `StartGoal` | 启动一个跨多轮持久化、结构化的目标。 |
| `ExitGoalMode` | 目标草稿评审完成，提交给用户审批。 |
| `UpdateGoalStatus` | 更新当前目标状态（resume / end / yield）。 |
| `GetGoalStatus` | 读取当前目标：objective、完成判据、状态、已消耗轮数。 |

## 多智能体协作（Delegate / Swarm）

| 工具 | 用途 |
| --- | --- |
| `Delegate` | 把有界子任务委派给一个子 agent；默认前台等待结果。 |
| `DelegateSwarm` | 并行派发多个相关子任务并汇总有序结果。 |
| `ListDelegates` | 列出子 agent / swarm 及其当前状态。 |
| `WaitDelegate` | 在一个全局超时内等待 `ids` 中的所有 delegate/swarm 达到终态；超时结果保留已完成结果和未完成目标的当前快照。 |
| `InterruptDelegate` | 中断并取消运行中的 delegate/swarm。 |
| `MessageDelegate` | 向运行中的 delegate 发送消息。 |

## 后台任务管理

| 工具 | 用途 |
| --- | --- |
| `TaskList` | 列出后台任务及其状态。Workflow 条目可包含 phase、准入等待原因与 awaiting-user 元数据。支持分页 cursor，而不是硬截断 50 条。 |
| `TaskOutput` | 取回运行中或已完成后台任务的输出。等待已知任务完成时优先 `block=true`。对 **workflow** 任务使用显式视图（`summary`、`journal`、`result`、`artifacts`、`artifact_content`）与不透明 cursor；Neo 绝不会把完整 journal 一次加载进结果。等待输入时，每个 view 都暴露可执行的 `pending_user` 字段：`request_id`、`prompt`、`answer_schema`、可选 `default`、`answer_policy` 与 `next_action`。 |
| `TaskStop` | 停止运行中的后台任务，或取消 workflow run。 |
| `TaskPause` | 请求运行中的 workflow 在下一个持久化 invocation 边界暂停；当前 child 会先完成。 |
| `TaskResume` | 恢复已暂停 workflow；先回放匹配的 journal invocation，再继续 live work。不能在没有类型化 answer 时解除 `awaiting_user`。 |
| `TaskAnswer` | 以 `task_id`、`request_id` 和类型化 `answer` 回答持久化 workflow `awaiting_user` 请求，仅在该请求的策略允许模型 actor 时可用。仅人类 gate 由用户通过 TUI 或人类 CLI 回答。 |

## 计时

| 工具 | 用途 |
| --- | --- |
| `Sleep` | 仅用于真正的时间等待（`duration_seconds` 1..=3600），不启动 shell 命令、不占用 shell 准入。已知 agent/swarm 优先 `WaitDelegate`；已知后台任务优先 `TaskOutput` 且 `block=true`。 |

## 其他

| 工具 | 用途 |
| --- | --- |
| `TodoList` | 维护结构化任务清单（pending / in_progress / done）。 |
| `Skill` | 按名称 + 参数调用一个可用 skill（由 `SkillStore` 提供）。 |
| `AskUserQuestion` | 执行中向用户提出带结构化选项的问题。 |
| `CreateSkill` | 在 `~/.neo/skills/<name>/SKILL.md` 创建新 skill。 |
| `MoveSkill` | 将 skill 目录移入父级 bundle，自动生成时间戳备份。 |
| `Workflow` | 规范的 assistant-native workflow 工具。平铺 action 为 `list`、`show`、`validate_inline`、`validate_saved`、`save`、`run_inline`、`run_saved`；inline 和 saved run 都返回 task ID。 |
| `ListSkills` | 列出所有可发现 skill（user / extra / builtin）。 |
| `SummarizeSessions` | 读取并总结本地 session transcript，便于沉淀为 skill。 |

### Workflow 工具与控制

assistant 的每个 workflow 生命周期动作都通过 `Workflow`。需要编写指导时可激活
`create-workflow`；已知的已保存 workflow 可直接 `list`/`show`/`run_saved`。
`run_inline`、`run_saved` 与 `save` 会在内部完成校验；只有用户明确要求
“只检查、不运行或不保存”时才使用 `validate_inline` 或 `validate_saved`。
这些路径均不需要 slash、capability 或 CLI。每次 run action 都在后台，返回
task ID（亦即 `run_id`）。

| 动作 | 方式 |
| --- | --- |
| 发现、校验、保存或运行 | `Workflow(list|show|validate_inline|validate_saved|save|run_inline|run_saved)` |
| 检查 | `TaskOutput` 的 workflow 视图/cursor；summary 从不内嵌完整 journal 或大 artifact |
| 暂停 / 恢复 / 停止 | `TaskPause`、`TaskResume`、`TaskStop`（持久化边界） |
| 回答 `awaiting_user` | 遵循 `TaskOutput.pending_user.next_action`；仅当其为 `TaskAnswer` 时才使用精确 ID 调用。仅 resume 不够。 |


Workflow Lua 创建的 child 必须带每 child 的 `output_schema`。无效 child JSON 在同一 child session 上获得 **恰好一次** 禁用工具的 repair 回合；无模糊 JSON 提取。Swarm 扇出支持异构且无硬编码总 child 上限；宿主 `swarm_concurrency` 只是默认并发。Ask / Auto / Yolo 控制每个 child 与 tool effect；launch 审批不能绕过它们。

用量统计 **只计 provider 实际用量**。没有用于暂停/降级的预测 token budget、agent budget 或 workflow wall-clock timeout。全局准入只看实际占用（VM、worker、executor、存储）。缺少持久化 `workflows/<run_id>/` 的历史卡片仍可阅读但不能恢复。完整编写指南：[Workflows](../guides/workflows.md)。

## 子 agent 工具集

派生 agent（`Delegate` / `DelegateSwarm`）默认仅注册子集，由 `ToolRegistry::with_builtin_child_tools()` 构建：

`Read` · `List` · `Grep` · `Find` · `Glob` · `TodoList` · `Write` · `Edit` · `Bash` · `TaskList` · `TaskOutput` · `TaskStop` · `Terminal` · `EnterPlanMode` · `ExitPlanMode` · `Sleep`

`Workflow` 和 `TaskAnswer` 仅属于 root agent，不在此工具集中。

外加 `AgentProfile::for_role` 按角色白名单过滤，调用方显式注册的自定义工具始终透传。

## 权限模型速查

工具执行由 `ToolAccess` 控制三类权限：`file_read` / `file_write` / `shell`；外部分发由 `ToolContext` 携带的 `PermissionMode`（Ask / Auto / Yolo）决定是否弹审批面板。
