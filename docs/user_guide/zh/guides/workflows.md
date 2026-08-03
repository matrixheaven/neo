# 本地工作流（Workflow）

Neo 把可持久化的 Lua 工作流作为一类一等后台任务来运行。一个工作流由「已审查的脚本 + 结构化元数据」组成：它可以并行派发子 Agent、调用普通工具、等待类型化的用户输入，并留下可检查、可暂停、可恢复、可停止的日志轨迹。

本页介绍工作流的定义编写、触发方式、Lua 宿主 API、资源上限与运维操作，只描述已落地行为。

## 工作流是什么

| 组成 | 说明 |
| --- | --- |
| **定义（Definition）** | 配套的 `<name>.lua` + `<name>.workflow.toml` 文件，或由模型动态编写的脚本 |
| **运行（Run）** | 会话下的一次持久化执行，位于 `workflows/<run_id>/` |
| **日志（Journal）** | 状态、调用记录、用户回答、产物、最终结果与实际用量的追加式记录，是唯一真相来源 |
| **任务 ID** | 与 `run_id` 相同；出现在 `/tasks`、`TaskOutput` 与 CLI 中 |

工作流**始终在后台运行**。启动时的审批只授权编排本身；之后每个子 Agent 或工具调用仍走 Ask / Auto / YOLO 的普通权限路径。

Neo **不会**预测 token 成本、耗时、Agent 数量或项目规模来决定暂停或降级运行。准入与上限只看**实际占用与存储**，不做预测。脚本引擎只有 Lua 一种（Rhai 或双引擎不在规划内）。

## 定义文件（配套的一对文件）

文件型定义是两个同名的普通文件：

```text
<name>.lua
<name>.workflow.toml
```

- 文件名的主干（stem）就是规范的查找名。
- TOML 清单持有结构化元数据；Neo 不会执行顶层 Lua 来发现 name / phases / schemas。
- Lua 文件是沙箱中的脚本本体。

### 清单字段

```toml
name = "my-workflow"          # 必须与文件名主干一致
display_name = "My Workflow"
description = "这次运行要编排什么"
source_sha256 = "<Lua 文件精确字节的小写十六进制>"

[[phases]]
id = "plan"
description = "范围与路径"

[[phases]]
id = "execute"
description = "执行工作"

# 仅对已保存的配对定义可选：省略表示该定义不接受参数。
# 内联的 Workflow(validate_inline)、Workflow(save) 与 Workflow(run_inline)
# 始终要求显式 input_schema；无参数的内联工作流使用
# {"type":"object","additionalProperties":false}。
[input_schema]
type = "object"
additionalProperties = false
required = ["topic"]
[input_schema.properties.topic]
type = "string"
minLength = 1

# 必需的最终输出 JSON Schema
[output_schema]
type = "object"
additionalProperties = false
required = ["summary", "ok"]
[output_schema.properties.summary]
type = "string"
[output_schema.properties.ok]
type = "boolean"
```

`source_sha256` 必须与 Lua 文件的精确字节一致。清单与源码的大小受 `runtime.workflow` 的 `manifest_bytes`、`lua_source_bytes` 约束。

### 内容版本（content revision）

每个定义都有一个内容版本：对规范化的清单 JSON 与精确的 Lua 源码做固定字节拼接后计算 SHA-256。路径、修改时间、注册表作用域**不是**哈希输入。运行开始时固定使用当时的版本；之后编辑或覆盖同名定义不会改写已有运行。

## 注册表作用域与信任

发现作用域只有三处：

```text
builtin                              # 编译进 Neo
$NEO_HOME/workflows                  # 用户定义
<trusted-workspace>/.neo/workflows   # 项目定义
```

**优先级：** `builtin < user < trusted project`。同名时高作用域覆盖低作用域；同一作用域内出现两个同名候选会使该名字失效。高作用域内容无效时**不会**静默回落到低作用域。

项目发现与项目保存复用 Neo 已有的工作区信任机制（`trust.json`）。未信任或禁用了项目发现时，不会出现项目候选。符号链接 / 重解析点定义文件与父路径逃逸会被拒绝；不跟随目录链接。

助手通过 `Workflow(save)` 保存定义。builtin 作用域不可写。

## 手动触发入口

```text
/workflow
/workflow <自然语言任务>
/workflow:<name> <自然语言任务>
/skill:create-workflow <编写请求>
```

- `/workflow` 打开可搜索的选择器。选中一行只会把 `/workflow:<name> ` 写入输入框，不会直接启动。
- `/workflow <任务>` 与 `/workflow:<name> <任务>` 各自只启动一次可见的模型轮次，并在对话记录中保留完整的原始斜杠输入。自动形式会收到完整的有效定义目录；指定名称的形式会收到所选定义和完整输入 schema。两种形式都不接受工作流参数 JSON，也不会由宿主直接启动。
- 编写、修改或适配工作流使用 `/skill:create-workflow <编写请求>`。

`/workflowish` 这类前缀和正文中出现的 `/workflow` 仍是普通提示。模型完成选择后，现有的权限、工作流卡片、任务控制和 headless CLI 行为保持不变。

### 无界面 CLI（仅限人与脚本）

```text
neo workflow list [--output text|json]
neo workflow check <name-or-path> [--json]
neo workflow test <name-or-path> --case <fixture> [--json]
neo workflow run <name> [--args <object> | --args-file <path>]
                  [--output text|json|jsonl]
```

规则：

- `list`、`check`、`test` 为只读操作。
- `run` 会等待运行进入终态。
- `--args` 与 `--args-file` 互斥。

这些命令只说明人与脚本的操作方式，不是助手的工作流路径。

## 助手侧路径

需要内联编写、新建已保存定义或做一次性测试时，助手可以在需要编写指导时激活 `create-workflow` 技能。对于已知的已保存工作流，直接使用 `Workflow(list|show|run_saved)` 发现或运行。全部生命周期动作统一由 `Workflow` 工具持有：`list`、`show`、`validate_inline`、`validate_saved`、`save`、`run_inline`、`run_saved`。

一次性评测在写完定义后可直接用 `Workflow(run_inline)` 启动。只有用户明确要求「只检查、不运行」时，才先调用 `Workflow(validate_inline)`——它不会创建任务。正常产品路径不需要插入源码检查、shell/CLI、Cargo、TodoList 或已保存工作流发现：

```text
Skill(create-workflow) -> Workflow(run_inline)
```

创建并测试则走 `Workflow(save) -> Workflow(run_saved)`。运行动作返回任务 ID，由工作流运行时持续执行。`TaskOutput` 是读取与等待工作流任务的唯一入口：用该任务 ID 获取状态、有界结果或日志分页、产物内容或待回答的输入。`WaitDelegate` 只处理 delegate 与 swarm ID，不处理工作流任务 ID。这些路径都不需要斜杠命令、能力开关、手工清单/哈希操作或 `neo workflow` CLI 调用。

工作流等待输入时，每个 `TaskOutput` 视图都会暴露可执行的 `pending_user` 字段：`request_id`、`prompt`、`answer_schema`、可选的 `default`、`answer_policy` 与 `next_action`。仅当 `next_action` 为 `TaskAnswer` 时，助手才用这些精确 ID 调用 `TaskAnswer(task_id, request_id, answer)`；`wait_for_human` 表示必须由用户在 TUI 或 CLI 中回答。

## Lua 宿主 API

沙箱只使用 mlua。没有文件系统、进程、网络、package、debug、time、random 或环境类标准库。参数（`neo.args`）递归只读。

| API | 用途 |
| --- | --- |
| `neo.args` | 只读的启动参数对象 |
| `neo.phase(id)` | 切换到已声明的阶段（写入日志） |
| `neo.log(message)` | 有界的进度日志 |
| `neo.delegate(input)` | 派生单个子 Agent；**必须**提供 `output_schema` |
| `neo.swarm(input)` | 直接派发一批子 Agent；包括同构批量派发在内，**每项**都需要 `output_schema` |
| `neo.tool({ name, input })` | 通过规范 `ToolRegistry` 调用合格工具；只接受 `{ name, input }` 形状。调用形状解码失败会中止宿主操作；已执行的工具失败返回 `ok = false` |
| `neo.await_user(input)` | 持久化的类型化用户输入；返回原始只读 answer 值（见下文） |
| `neo.verify(condition, message)` | 返回不可变结果，直接检查 `outcome.ok` |
| `neo.verify_command({ command, cwd?, failure_message? })` | 经 Bash 执行；成功和普通失败都返回结果 |
| `neo.report(value)` | 中间报告；不返回任何值——只能作为语句使用 |
| `neo.fail(message)` | 显式终态失败；`pcall` 无法撤销或恢复 |
| `neo.json_array(table)` | 要求传表；返回标记表（绝不返回字符串）；`nil` 无效 |
| `neo.json_object(table)` | 要求传表；返回标记表（绝不返回字符串）；`nil` 无效 |

没有 `neo.parallel`、递归工作流启动、脱离式工作流任务、裸 shell 逃逸或引擎选择 API。

### 效果的结果形态

宿主效果按返回方式分为三组：

- 返回结果表的调用（`neo.delegate`、`neo.swarm`、`neo.tool`、`neo.verify`、`neo.verify_command`）返回同一个不可变表形态：

  ```text
  ok, status, summary, details?, actual_usage?, agent_id?, swarm_id?, task_id?
  ```

- `neo.await_user` 返回原始只读 answer 值，不是结果表。
- `neo.report` 只记录中间报告，不返回任何值；只能作为语句使用。

`status` 取值为：`completed` | `failed` | `denied` | `cancelled` | `resource_limited` | `interrupted`。

普通校验和工具失败会返回 `ok = false` 的结果值，脚本可以直接分支处理，不需要 `pcall`。`neo.fail`、未捕获的 Lua 错误、资源耗尽、取消以及最终结果无效都会终止工作流。`neo.fail` 是终态的运行决定，`pcall` 无法撤销或恢复。工作流任务 ID 一律通过 `TaskOutput` 读取与等待；绝不把工作流 ID 传给 `WaitDelegate`。

### 最终结果

顶层 Lua 返回值（至多一个）是**唯一**的最终结果。零返回或单个 `nil` 变成 JSON `null`。混合键或稀疏表转换失败。`neo.report` 绝不能替代最终结果。

### `neo.delegate` / `neo.swarm`

新子 Agent 的输入字段：

```text
task（必填）, title?, role?, model?, provider?, context?, worktree?,
tool_allow?, output_schema（必填的 JSON Schema）
```

成功时，通过 schema 校验的子 Agent JSON 位于 `outcome.details.structured_output`。

直接 swarm 形态：

```lua
neo.swarm({
  description = "review each subsystem",
  items = {
    { task = "review runtime", role = "reviewer", output_schema = runtime_schema },
    { task = "review persistence", role = "reviewer", output_schema = persistence_schema },
  },
})
```

每个 item 都是完整的子 Agent 规格。工作流 Lua 不接受面向模型的独立 `DelegateSwarm` 简写：禁止 `prompt_template`、`title`/`value` 形式的 item、`resume_agent_ids` 与顶层 `output_schema`。

JSON 标记不可变。先构建并修改普通 Lua 表，收集完成后再调用 `neo.json_array(table)` 或 `neo.json_object(table)`；标记之后不得继续修改。

恢复型子 Agent 只接受 `resume`、`task`、`output_schema`。新子 Agent 的 worktree 默认为 `shared`；`isolated` 需要显式指定。**没有**模型/脚本字段可以设置 `max_concurrency`、token 预算、Agent 预算或墙钟超时。宿主 `runtime.workflow.swarm_concurrency` 提供默认的 swarm 并发（不是子 Agent 总数上限）。没有硬编码的 `MAX_SWARM_CHILDREN`；真实的字节、内存、日志与准入上限仍然生效。

### 恰好一次的 schema 修复

对每个子 Agent 的结构化输出：

1. 通过规范 Agent 运行时运行子 Agent。
2. 解析**一个**严格 JSON 值（不剥离代码围栏、不做模糊提取）。
3. 无效时：写入 `SchemaRepairStarted`，在**同一个**子 Agent 会话上**禁用工具**继续，只要求替换 JSON。
4. 只有一次自动纠正轮次；之后要么成功，要么进入终态 `schema_invalid`。

修复期间的工具调用以 `schema_repair_tool_forbidden` 失败。不确定的外部副作用永不自动重试。最终结果 schema 校验失败（当附着了最终 schema 时）不会启动模型修复轮次。

### `neo.tool` 拒绝集

已注册工具默认合格，但集中拒绝编排/控制类工具，包括（不限于）：`Workflow`、`Delegate`、`DelegateSwarm`、`TaskPause` / `TaskResume` / `TaskStop` / `TaskAnswer`、计划/目标工具、多 Agent 控制工具。子 Agent、用户输入与工作流控制由专用 API 持有。指向**当前**运行的 `TaskOutput` 会被拒绝，避免递归锁与路径重入。Shell 准入保持 pending，无隐式超时。

### `neo.await_user`（禁止索取密钥）

```text
prompt（必填）, answer_schema（必填）, default?, title?,
answer_policy?  # human | human_or_model；默认 human
```

**不要通过该接口索取密码、API key 或其他密钥。** 回答会写入本地日志，重启后仍可读取。运行进入持久化的 `awaiting_user` 状态，释放活动 VM/worker 准入，并继续在 `/tasks` 与 CLI 中可见。助手只对 `human_or_model` 使用 `TaskAnswer`；仅手动请求由用户在 `/tasks` 中回答。

没有回答的 `TaskResume` **不能**解除 `awaiting_user`。

## 资源上限与准入

在 `~/.neo/config.toml` 的 `[runtime.workflow]` 下配置。脚本、模型工具输入与定义**不能**设置或提高这些值。被拒绝的键包括 `token_cap`、`max_concurrency`（作为工作流的模型限额）与 `projected_usage` 等预测性字段。

上限覆盖：源码/清单字节、Lua VM 内存与指令钩子、日志与产物大小、全局存储、TaskOutput 页大小、活动 VM/worker/executor，以及默认 swarm 并发。详见 [配置文件](../configuration/config-files.md#runtimeworkflow-子表)。

**全局准入**只跟踪**实际占用**（活动 VM、worker、executor、存储）。许可不可用时，运行保持持久化并进入 **queued** 状态；`/tasks` 与 `TaskOutput` 会展示等待原因。不推断工作流的墙钟超时。暂停与停止始终可用。

## 存储布局与不可变性

每个运行目录：

```text
<session_dir>/workflows/<run_id>/
  run.json                 # 不可变的启动元数据
  journal.jsonl            # 追加式的状态与调用记录
  artifacts/               # 内容寻址的不可变字节
  recovery-quarantine/     # 仅用于异常截断尾部的隔离
```

过大的最终结果、报告与 schema 原始输出可能以产物引用存储。读取时会重新校验大小与摘要。当工作流存储达到配置的高水位时，自动保留策略只会回收超过最小保留时间的终态运行，直到恢复到低水位。

终态运行不可变。再次运行同一工作流会创建独立的新运行，拥有自己的参数、结果、用量和日志。只有规范运行目录可读取和恢复；已废弃的日志布局不会迁移或投影。

## TaskOutput 游标

对工作流任务，`TaskOutput` 绝不会同步加载完整日志。支持的视图：

| 视图 | 内容 |
| --- | --- |
| `summary`（默认） | 有界的状态、阶段、用量、待回答请求、结果/产物引用、下一页游标 |
| `journal` | 升序连续的日志分页 |
| `result` | 规范最终结果的投影 |
| `artifacts` | 有界的产物元数据分页 |
| `artifact_content` | 按产物 id 的字节范围内容 |

每个非 summary 视图接受绑定运行、视图与查询哈希的稳定**游标**。错误游标会被拒绝。响应报告 `has_more` / `next_cursor` / 返回字节数。记录不会被静默中途截断。

使用返回的任务 ID 调用 `TaskOutput`，等待完成并读取真实的有界结果、日志分页或产物内容。`WaitDelegate` 不读取工作流任务。

## `/tasks` 面板

`/tasks` 已扩展工作流支持：可过滤的列表、阶段/进度、排队/准入原因、等待输入状态、实际用量、详情/输出，以及合法控制（暂停、恢复、回答、停止）。它仍是后台任务与工作流快照的投影——不是第二个状态所有者。Delegate / Bash / Terminal 卡片布局保持不变。

## 内置工作流

随 Neo 发布的普通定义（只使用公共 Lua API，无特权宿主函数）：

| 名称 | 用途 |
| --- | --- |
| `code-review` | 只读的多领域代码审查；从不改代码 |
| `deep-research` | 结构化的多步研究 |
| `large-refactor` | 分阶段的重构编排 |

助手用 `Workflow(list)`、`Workflow(show)` 与 `Workflow(run_saved)` 使用它们。人可以使用上面的斜杠入口或 headless CLI。

## 检查清单

### 助手路径

1. 通过 `Workflow` 编写；需要编写指导时再激活 `create-workflow`。
2. 只通过 `Workflow(save)` 持久化，并通过 `Workflow(run_inline)` 或 `Workflow(run_saved)` 运行。
3. 使用返回的任务 ID 调用 `TaskOutput`，读取状态、结果、产物、日志分页或待回答输入。
4. 只在 `human_or_model` 闸门使用 `TaskAnswer`；仅手动回答留给用户。
5. 不要要求用户先输入裸斜杠、调用 `neo workflow`，或手写清单/哈希。

### 手动编写脚本文件

1. 配套放置 `.lua` + `.workflow.toml`，主干与 `source_sha256` 一致。
2. 声明有序的 `phases` 与必需的最终 `output_schema`。
3. 每个 `neo.delegate` / `neo.swarm` 子 Agent 都提供 `output_schema`。
4. 绝不通过 `neo.await_user` 索取密钥。
5. 用 `neo workflow check` 校验；fixture 用 `neo workflow test --case`。
6. 浏览使用 `/workflow`，自动选择使用 `/workflow <task>`，明确指定使用 `/workflow:<name> <task>`；脚本化操作使用 headless CLI。
7. 用 `TaskOutput` 的视图/游标检查。

## 下一步

- [内置工具](../reference/tools.md) — `Workflow`、`TaskAnswer`、`TaskOutput`、pause/resume/stop
- [斜杠命令](../reference/slash-commands.md) — `/workflow`、`/tasks`
- [配置文件](../configuration/config-files.md) — `[runtime.workflow]`
- [数据路径](../configuration/data-locations.md) — 会话下的运行布局
