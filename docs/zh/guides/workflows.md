# 本地 Workflow 平台

Neo 将 **可持久化的 Lua workflow** 作为一等本地后台任务运行。一个 workflow 是「已审查脚本 + 结构化元数据」：可以扇出子 agent、调用普通工具、等待类型化用户回答，并留下可检查、暂停、恢复或停止的 journal 轨迹。

本指南覆盖定义编写、启动方式、Lua 宿主 API、schema、机器上限与运维面。**只描述已落地行为**。

## Workflow 是什么

| 组成 | 作用 |
| --- | --- |
| **Definition** | 成对的 `<name>.lua` + `<name>.workflow.toml`，或模型动态编写的脚本 |
| **Run** | 会话下的一次持久化执行：`workflows/<run_id>/` |
| **Journal** | 状态、invocation、回答、artifact、最终结果与实际用量的 append-only 真相源 |
| **Task ID** | 与 `run_id` 相同；出现在 `/tasks`、`TaskOutput` 与 CLI 中 |

Workflow **始终后台**运行。Launch 审批只授权编排；之后的每个 child 或 tool effect 仍走 Ask / Auto / Yolo 的普通权限路径。

Neo **不会**预测 token 成本、耗时、agent 数量或项目规模来暂停/降级 run。准入与上限只看 **实际占用与存储**，不做预测。没有第二套脚本引擎（仅 Lua；Rhai / 双引擎为非目标）。

## 定义文件（成对）

文件型定义是两个同 stem 的普通文件：

```text
<name>.lua
<name>.workflow.toml
```

- 文件名 stem 是规范查找名。
- TOML manifest 拥有结构化元数据；Neo 不会执行顶层 Lua 来发现 name/phases/schemas。
- Lua 文件是沙箱脚本本体。

### Manifest 字段

```toml
name = "my-workflow"          # 必须与文件名 stem 一致
display_name = "My Workflow"
description = "这次 run 编排什么"
source_sha256 = "<Lua 精确字节的小写十六进制>"

[[phases]]
id = "plan"
description = "范围与路径"

[[phases]]
id = "execute"
description = "执行工作"

# 仅对存储的配对定义可选：省略表示该已保存定义不接受参数。
# 内联 Workflow(validate_inline)、Workflow(save) 与 Workflow(run_inline)
# 始终要求显式 input_schema；无参数内联 workflow 使用
# {"type":"object","additionalProperties":false}。
[input_schema]
type = "object"
additionalProperties = false
required = ["topic"]
[input_schema.properties.topic]
type = "string"
minLength = 1

# 必需的最终 output JSON Schema
[output_schema]
type = "object"
additionalProperties = false
required = ["summary", "ok"]
[output_schema.properties.summary]
type = "string"
[output_schema.properties.ok]
type = "boolean"
```

`source_sha256` 必须与 Lua 文件精确字节一致。manifest 与 source 大小受 `runtime.workflow`（`manifest_bytes`、`lua_source_bytes`）约束。

### 内容 revision

每个定义有 **content revision**：对 canonical manifest JSON 与精确 Lua source 的固定字节 framing 做 SHA-256。路径、mtime、registry scope **不是** 哈希输入。Run 钉住启动时的 revision；编辑或 shadow 定义不会改写已有 run。

## 注册表 scope 与信任

发现 scope 仅此三者：

```text
builtin                              # 编译进 Neo
$NEO_HOME/workflows                  # 用户定义
<trusted-workspace>/.neo/workflows   # 项目定义
```

**优先级：** `builtin < user < trusted project`。同名时高 scope 覆盖低 scope。同一 scope 内两个同名候选会使该名无效。高 scope 内容无效时 **不会** 静默回落到低 scope。

项目发现与项目保存复用 Neo 已有的 **工作区信任**（`trust.json`）。未信任或禁用项目发现时不会出现 project 候选。符号链接/reparse point 定义文件与父路径逃逸会被拒绝；不跟随目录链接。

assistant 通过 `Workflow(save)` 保存。builtin scope 不可写。

## Assistant-native workflow 路径

需要 inline 编写、新建已保存定义或一次性测试/评测时，assistant 可在需要编写
指导时激活 `create-workflow`。对于已知的已保存 workflow，可直接使用
`Workflow(list|show|run_saved)`
发现或运行。全部生命周期 action 仍由 `Workflow` 统一拥有：`list`、
`show`、`validate_inline`、`validate_saved`、`save`、`run_inline`、
`run_saved`。

一次性评测在完成定义后可直接通过 `Workflow(run_inline)` 启动。只有用户明确
要求只检查、不运行时，才先调用 `Workflow(validate_inline)`；它不会创建任务。
正常产品路径不需要插入源码检查、shell/CLI、Cargo、TodoList 或已保存 workflow
发现：

```text
Skill(create-workflow) -> Workflow(run_inline)
```

创建并测试则走 `Workflow(save) -> Workflow(run_saved)`。run action 返回 task
ID，并由 workflow runtime 持续执行。`TaskOutput` 是 workflow 任务唯一的读取与等待
入口：用该 task ID 获取状态、有界结果或 journal 页、artifact 内容，或待回答输入。
`WaitDelegate` 只处理 delegate 和 swarm ID，不处理 workflow task ID。这些路径均不
需要 slash、capability、手工 manifest/hash 操作或 `neo workflow` CLI 调用。

workflow 等待输入时，每个 `TaskOutput` view 都会暴露可执行的
`pending_user`：`request_id`、`prompt`、`answer_schema`、可选 `default`、
`answer_policy` 与 `next_action`。仅当 `next_action` 为 `TaskAnswer` 时，
assistant 才以这些精确 ID 调用 `TaskAnswer(task_id, request_id, answer)`；
`wait_for_human` 表示必须由用户在 TUI 或 CLI 中回答。

## 手动触发工作流入口

```text
/workflow
/workflow <自然语言任务>
/workflow:<name> <自然语言任务>
/skill:create-workflow <编写请求>
```

`/workflow` 打开可搜索选择器。选择一行只会把
`/workflow:<name> ` 写入 composer。自动选择和指定名称两种形式各自只
启动一次可见模型回合，并在 transcript 中保留完整原始斜杠输入。自动形式
收到完整有效目录；指定形式收到所选定义和完整输入 schema。两种形式都不
接受工作流参数 JSON，也不会由宿主直接启动。

创建、修改、适配或确认后的一次性编写使用 `/skill:create-workflow`。
`/workflowish` 和正文中的 `/workflow` 仍是普通提示。模型完成选择后，现有
权限、工作流卡片、任务控制和 headless CLI 行为保持不变。

### Headless CLI（仅人类和脚本）

```text
neo workflow list [--output text|json]
neo workflow check <name-or-path> [--json]
neo workflow test <name-or-path> --case <fixture> [--json]
neo workflow run <name> [--args <object> | --args-file <path>]
                  [--output text|json|jsonl]
```

规则：

- `list`、`check`、`test` 为只读。
- `run` 等待终态。
- `--args` 与 `--args-file` 互斥。

这些命令仅说明人类与脚本的操作方式，不是 assistant workflow 路径。

## Lua 宿主 API

沙箱为 **仅 mlua**。无文件系统、进程、网络、package、debug、time、random 或环境类标准库。参数（`neo.args`）递归只读。

| API | 用途 |
| --- | --- |
| `neo.args` | 只读 launch 参数对象 |
| `neo.phase(id)` | 选择已声明 phase（写入 journal） |
| `neo.log(message)` | 有界进度日志 |
| `neo.delegate(input)` | 单个子 agent；**必须**提供 `output_schema` |
| `neo.swarm(input)` | 直接 child spec 批；包括同构 fan-out 在内，**每项** `output_schema` 都必需 |
| `neo.tool({ name, input })` | 通过规范 `ToolRegistry` 调用合格工具；仅接受 `{ name, input }`。调用形状解码失败会中止宿主操作；已执行工具失败返回 `ok = false`。 |
| `neo.await_user(input)` | 持久化类型化用户输入；返回原始只读 answer 值（见下） |
| `neo.verify(condition, message)` | 返回不可变结果，直接检查 `outcome.ok` |
| `neo.verify_command({ command, cwd?, failure_message? })` | 经 Bash 执行，成功和普通失败都返回结果 |
| `neo.report(value)` | 中间报告；不返回任何值——仅作语句使用 |
| `neo.fail(message)` | 显式终态失败；`pcall` 无法撤销或恢复 |
| `neo.json_array(table)` | 要求传表；返回标记表（绝不返回字符串）；`nil` 无效 |
| `neo.json_object(table)` | 要求传表；返回标记表（绝不返回字符串）；`nil` 无效 |

没有 `neo.parallel`、递归 workflow 启动、detached workflow 任务、裸 shell 逃逸或引擎选择 API。

### Effect 结果形态

宿主效果分为三种返回分组：

- 返回结果表的调用（`neo.delegate`、`neo.swarm`、`neo.tool`、
  `neo.verify`、`neo.verify_command`）返回同一不可变表形态：

  ```text
  ok, status, summary, details?, actual_usage?, agent_id?, swarm_id?, task_id?
  ```

- `neo.await_user` 返回原始只读 answer 值，而非结果表。
- `neo.report` 记录中间报告且不返回任何值；仅作语句使用。

`status` 为：`completed` | `failed` | `denied` | `cancelled` | `resource_limited` | `interrupted`。

普通校验和工具失败会返回 `ok = false` 的结果值，脚本可以直接分支处理，不需要
使用 `pcall`。`neo.fail`、未捕获的 Lua 错误、资源耗尽、取消以及最终结果无效会
终止 workflow。`neo.fail` 是终态运行决定，`pcall` 无法撤销或恢复。workflow
task ID 一律通过 `TaskOutput` 读取与等待；绝不把 workflow ID 传给
`WaitDelegate`。

### 最终结果

顶层 Lua 返回值（至多一个）是 **唯一** 最终结果。零返回 / 单个 `nil` 变为 JSON `null`。混合键或稀疏表转换失败。`neo.report` 绝不能替代最终结果。

### `neo.delegate` / `neo.swarm`

新 child 输入包括：

```text
task（必需）, title?, role?, model?, provider?, context?, worktree?,
tool_allow?, output_schema（必需的 JSON Schema）
```

成功时，通过 schema 的 child JSON 位于
`outcome.details.structured_output`。

Direct swarm 形态：

```lua
neo.swarm({
  description = "review each subsystem",
  items = {
    { task = "review runtime", role = "reviewer", output_schema = runtime_schema },
    { task = "review persistence", role = "reviewer", output_schema = persistence_schema },
  },
})
```

每个 item 都是完整 child spec。Workflow Lua 不接受独立 model-facing
`DelegateSwarm` 的简写：禁止 `prompt_template`、`title`/`value` item、
`resume_agent_ids` 与顶层 `output_schema`。

JSON marker 不可变。先构建并修改普通 Lua table，收集完成后再调用
`neo.json_array(table)` 或 `neo.json_object(table)`；标记后不得继续修改。

恢复型 child 只接受 `resume`、`task`、`output_schema`。新 child 的 worktree 默认为 `shared`；`isolated` 需显式指定。**没有** 模型/脚本字段可设置 `max_concurrency`、token budget、agent budget 或 wall-clock timeout。宿主 `runtime.workflow.swarm_concurrency` 提供默认 swarm 并发（不是总 child 数上限）。没有硬编码 `MAX_SWARM_CHILDREN`；真实字节、内存、journal 与准入上限仍然生效。

### 恰好一次 schema repair

对每个 child 结构化输出：

1. 通过规范 agent runtime 运行 child。
2. 解析 **一个** 严格 JSON 值（不剥 fence、不做模糊提取）。
3. 无效时：写入 `SchemaRepairStarted`，在 **同一** child session 上 **禁用工具** 继续，只要求替换 JSON。
4. 仅一次自动纠正回合；之后成功或终态 `schema_invalid`。

repair 期间的 tool call 以 `schema_repair_tool_forbidden` 失败。不确定的外部 effect 永不自动重试。最终结果 schema 失败（当附着了 final schema 时）不会启动模型 repair 回合。

### `neo.tool` 拒绝集

已注册工具默认合格，但集中拒绝编排/控制类工具，包括（不限于）：`Workflow`、`Delegate`、`DelegateSwarm`、`TaskPause` / `TaskResume` / `TaskStop` / `TaskAnswer`、plan/goal 工具、多 agent 控制工具。子 agent、用户输入与 workflow 控制由专用 API 拥有。指向 **当前** run 的 `TaskOutput` 会被拒绝，避免递归锁/路径重入。Shell 准入保持 pending，无隐式超时。

### `neo.await_user`（禁止密钥）

```text
prompt（必需）, answer_schema（必需）, default?, title?,
answer_policy?  # human | human_or_model；默认 human
```

**不要通过该接口索取密码、API key 或其他密钥。** 回答会写入本地 journal，重启后仍可读取。Run 进入持久化 `awaiting_user`，释放活动 VM/worker 准入，并继续在 `/tasks` 与 CLI 中可见。assistant 只对 `human_or_model` 使用 `TaskAnswer`；仅手动请求由用户通过 `/tasks` 回答。

没有 answer 的 `TaskResume` **不能** 解除 `awaiting_user`。

## 机器上限与准入

在 `~/.neo/config.toml` 的 `[runtime.workflow]` 下配置。脚本、模型工具输入与定义 **不能** 设置或提高这些值。拒绝的键包括 `token_cap`、`max_concurrency`（作为 workflow 模型限额）与 `projected_usage` 等预测性字段。

上限覆盖 source/manifest 字节、Lua VM 内存与指令钩子、journal 与 artifact 大小、全局存储、TaskOutput 页大小、活动 VM/worker/executor，以及默认 swarm 并发。详见 [配置文件](../configuration/config-files.md#runtimeworkflow-子表)。

**全局准入**只跟踪 **实际占用**（活动 VM、worker、executor、存储）。许可不可用时 run 保持持久化并 **queued**；`/tasks` 与 `TaskOutput` 可展示等待原因。不推断 workflow wall-clock 超时。暂停与停止始终可用。

## Artifact 与存储布局

每个 run 目录：

```text
<session_dir>/workflows/<run_id>/
  run.json                 # 不可变 launch metadata
  journal.jsonl            # append-only 状态与 invocation
  artifacts/               # 内容寻址的不可变字节
  recovery-quarantine/     # 仅用于 torn-tail 隔离
```

过大的最终结果、报告与 schema 原始输出可能以 artifact 引用存储。读取会重新校验 size/digest。当 workflow 存储达到配置的高水位时，自动 retention 只会回收已超过最小保留时间的终态 run，直到恢复到低水位。

## Run 不可变性

终态 run 不可变。再次运行 workflow 会创建独立的新 run，拥有自己的参数、结果、用量和 journal。只有 canonical run 目录可读取和恢复；已废弃的 journal 布局不会迁移或投影。

## TaskOutput 游标

对 workflow 任务，`TaskOutput` 绝不会同步加载完整 journal。支持的视图：

| 视图 | 内容 |
| --- | --- |
| `summary`（默认） | 有界状态、phase、用量、待回答请求、结果/artifact 引用、下一页 cursor |
| `journal` | 升序连续 journal 页 |
| `result` | 规范最终结果投影 |
| `artifacts` | 有界 artifact 元数据页 |
| `artifact_content` | 按 artifact id 的字节范围内容 |

每个非 summary 视图接受绑定 run、view 与 query hash 的稳定 **cursor**。错误 cursor 会被拒绝。响应报告 `has_more` / `next_cursor` / 返回字节数。记录不会被静默中途截断。

使用返回的 task ID 调用 `TaskOutput`，等待完成并读取真实的有界结果、journal 页或
artifact 内容。`WaitDelegate` 不读取 workflow 任务。

## `/tasks` 面板

`/tasks` 已扩展 workflow 支持：可过滤列表、phase/进度、排队/准入原因、等待输入状态、实际用量、详情/输出，以及合法控制（暂停、恢复、回答、停止）。它仍是 background task 与 workflow 快照的投影 — 不是第二状态所有者。Delegate / Bash / Terminal 卡片布局保持不变。

## 内置 workflow

随 Neo 发布的普通定义（仅公共 Lua API，无特权宿主函数）：

| 名称 | 意图 |
| --- | --- |
| `code-review` | 只读多域代码审查；从不改代码 |
| `deep-research` | 结构化多步研究 |
| `large-refactor` | 分阶段重构编排 |

assistant 用 `Workflow(list)`、`Workflow(show)` 与 `Workflow(run_saved)`。人类可使用上面的 slash 入口或 headless CLI。

## Workflow检查清单

### Assistant 路径

1. 通过 `Workflow` 编写；需要编写指导时再激活 `create-workflow`。
2. 仅通过 `Workflow(save)` 持久化，并通过 `Workflow(run_inline)` 或 `Workflow(run_saved)` 运行。
3. 使用返回的 task ID 调用 `TaskOutput`，读取状态、结果、artifact、journal 页或待回答输入。
4. 只在 `human_or_model` gate 使用 `TaskAnswer`；仅手动回答留给用户。
5. 不要要求用户先输入裸 slash、调用 `neo workflow`，或手写 manifest/hash。

### 手动脚本文件编写

1. 成对放置 `.lua` + `.workflow.toml`，stem 与 `source_sha256` 一致。
2. 声明有序 `phases` 与必需的最终 `output_schema`。
3. 每个 `neo.delegate` / `neo.swarm` child 都给 `output_schema`。
4. 绝不通过 `neo.await_user` 索取密钥。
5. 用 `neo workflow check` 校验；fixture 用 `neo workflow test --case`。
6. 浏览使用 `/workflow`，自动选择使用 `/workflow <task>`，明确指定使用 `/workflow:<name> <task>`；脚本化操作使用 headless CLI。
7. 用 `TaskOutput` 视图/cursor 检查。

## 下一步

- [内置工具](../reference/tools.md) — `Workflow`、`TaskAnswer`、`TaskOutput`、pause/resume/stop
- [斜杠命令](../reference/slash-commands.md) — `/workflow`、`/tasks`
- [配置文件](../configuration/config-files.md) — `[runtime.workflow]`
- [数据路径](../configuration/data-locations.md) — 会话下的 run 布局
