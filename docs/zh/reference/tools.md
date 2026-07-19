# 内置工具参考

Neo 通过 `ToolRegistry` 向模型暴露一组内置工具。本文按类别列出全部内置工具及其用途，供 Skill / prompt / 调试参考。

源码位置：[`crates/neo-agent-core/src/tools/`](../../../crates/neo-agent-core/src/tools/)，规范名来源 `Tool::name()`。

## 文件操作

| 工具 | 用途 |
| --- | --- |
| `Read` | 读取 UTF-8 文本文件，支持按行偏移分页读取。 |
| `Write` | 创建或完整覆盖写入工作区内 UTF-8 文件。 |
| `Edit` | 对现有文件做精确字符串查找替换，返回 unified diff。 |
| `List` | 以两层树形列出目录内容。 |
| `Glob` | 按 glob 模式匹配文件/目录路径，按修改时间排序。 |
| `Find` | 按文件/目录名子串查找工作区路径。 |
| `Grep` | 基于正则搜索工作区文本文件内容。 |

## Shell

| 工具 | 用途 |
| --- | --- |
| `Bash` | 在工作区执行 `bash`（Windows 上为 Git Bash）命令，支持管道、后台任务、可选 `timeout_secs` 与取消。省略 `timeout_secs` 表示不设超时；显式值必须在 `300..=3600`。超时后应增大或翻倍再重试；若已为 `3600` 或耗时无法确定，则省略。 |
| `Terminal` | 操作一个真实 PTY 会话：start / write / read / resize / stop，适合交互式长进程。`start` / `write` / `read` 共用可选 `yield_time_ms`（默认 250 / 250 / 3000 ms，范围 `0..=30000`），在 admission 成功且操作就绪后等待增量 **原始 PTY** 输出；到期仅返回当前输出且 `status: running`，绝不停止命令。admission 队列等待仍无限，原 Tool Use 保持 pending。`timeout_secs` 仅对 `mode=start` 有效；省略表示不设命令生命周期截止，否则必须在 `300..=3600`。超时后应增大或翻倍再重试；若已为 `3600` 或耗时无法确定，则省略。不过滤 echo、ANSI、CR、光标控制。Write 可发送原始控制字节如 Ctrl+C（`\u0003`）、Ctrl+D（`\u0004`）、Ctrl+Z（`\u001a`），但不承诺跨平台 signal 语义。 |

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
| `TaskList` | 列出后台任务及其状态。 |
| `TaskOutput` | 取回一个运行中或已完成后台任务的输出。等待已知任务完成时优先使用 `block=true`。 |
| `TaskStop` | 停止运行中的后台任务。 |

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
| `RunWorkflow` | 运行 Lua 工作流脚本（可调用 `neo.delegate` / `neo.swarm` 等）。 |
| `ListSkills` | 列出所有可发现 skill（user / extra / builtin）。 |
| `SummarizeSessions` | 读取并总结本地 session transcript，便于沉淀为 skill。 |

## 子 agent 工具集

派生 agent（`Delegate` / `DelegateSwarm`）默认仅注册子集，由 `ToolRegistry::with_builtin_child_tools()` 构建：

`Read` · `List` · `Grep` · `Find` · `Glob` · `TodoList` · `Write` · `Edit` · `Bash` · `TaskList` · `TaskOutput` · `TaskStop` · `Terminal` · `EnterPlanMode` · `ExitPlanMode` · `RunWorkflow` · `Sleep`

外加 `AgentProfile::for_role` 按角色白名单过滤，调用方显式注册的自定义工具始终透传。

## 权限模型速查

工具执行由 `ToolAccess` 控制三类权限：`file_read` / `file_write` / `shell`；外部分发由 `ToolContext` 携带的 `PermissionMode`（Ask / Auto / Yolo）决定是否弹审批面板。
