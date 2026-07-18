# 子 Agent（Sub-agents）

Neo 可以把一个任务派生（delegate）给一个或多个独立的子 Agent 并发执行。子 Agent 拥有自己的角色、工具集、上下文窗口和对话历史，完成后向主 Agent 返回摘要。核心实现见 `crates/neo-agent-core/src/multi_agent/` 与 `crates/neo-agent-core/src/tools/delegate.rs`。

## 子 Agent 概念

| 概念 | 说明 |
| --- | --- |
| **主 Agent** | 当前与用户对话的顶层 Agent |
| **子 Agent** | 由 `Delegate` / `DelegateSwarm` 工具派生，拥有独立 `AgentId`、角色和工具策略 |
| **角色（Role）** | 决定子 Agent 的工具白名单、system 提示补丁、权限策略 |
| **Swarm** | 一次性派生多个子 Agent 的批量调度，支持 `{{item}}` 模板与并发上限 |
| **上下文模式** | 子 Agent 能看到多少父级上下文：`inherit` / `summary` / `none` |

子 Agent 的生命周期状态由 `AgentLifecycleState` 管理：`queued → running → completed / failed / cancelled / timed_out / interrupted`。主 Agent 默认前台等待子 Agent 返回；也可设为 `background` 让主 Agent 继续推进。

## Delegate / DelegateSwarm

| 工具 | 说明 |
| --- | --- |
| `Delegate` | 派生单个子 Agent；支持 `resume` 续跑已存在的 agent_id |
| `DelegateSwarm` | 用 `prompt_template` + `items` 批量派生；支持 `resume_agent_ids` 续跑、`max_concurrency` 限流 |

### Delegate 关键参数

| 参数 | 默认 | 说明 |
| --- | --- | --- |
| `task` | 必填 | 子 Agent 任务描述 |
| `role` | `coder` | 子 Agent 角色 |
| `mode` | `foreground` | `foreground` 等待 / `background` 后台并发 |
| `context` | `inherit` | 父级上下文传递方式：`inherit` / `summary` / `none` |
| `resume` | — | 已存在的 `agent_xxx` 续跑；此时 `role` 必须省略 |
| `title` | 自动派生 | UI 显示名 |

### DelegateSwarm 关键参数

| 参数 | 说明 |
| --- | --- |
| `description` | swarm 标题 |
| `items` | 子任务数组，每项含 `title` 与插入模板的 `value` |
| `prompt_template` | 支持 `{{item}}` 与 `{{description}}` |
| `resume_agent_ids` | `{ "agent_xxx": "继续提示词" }` 续跑映射 |
| `max_concurrency` | 最大并发数（>0） |

## 角色类型

四种内置角色由 `AgentProfile::for_role` 定义，各自有独立工具白名单与权限策略：

| 角色 | 字符串 | 工具集 | 权限策略 | 何时用 |
| --- | --- | --- | --- | --- |
| **Coder** | `coder` | Read/List/Grep/Find/Glob/Bash/Write/Edit/TodoList | 完全访问（shell + 写文件） | 实现类任务，默认选择 |
| **Explorer** | `explorer` | Read/List/Grep/Find/Glob/Bash（只读） | shell 只读、禁写 | 只读代码勘探，可并发多个 |
| **Planner** | `planner` | Read/List/Grep/Find/Glob | 无 shell、禁写 | 写代码前的实现规划 |
| **Reviewer** | `reviewer` | Read/List/Grep/Find/Glob/Bash（只读） | shell 只读、禁写 | 改动后的只读评审 |

Explorer / Reviewer 的 Bash 仅允许只读命令（`ls`、`rg`、`git status/diff/log/show` 等），写文件类工具不可用。角色的"何时用"提示会拼到工具 schema 里，引导模型正确选型。

## 上下文隔离

`context` 字段决定子 Agent 拿到多少父级上下文，是控制 token 成本与隔离度的关键：

| 模式 | 行为 |
| --- | --- |
| `inherit`（默认） | 传入精选的父级上下文 |
| `summary` | 只传入父级的紧凑摘要 |
| `none` | 仅任务文本 + 角色提示，完全隔离 |

子 Agent 拥有独立对话历史，返回主 Agent 的只是结果摘要，不会把整段历史回灌。续跑（`resume`）会在同一子 Agent 的历史之上继续。

## 权限继承

- 子 Agent 的工具白名单由角色决定（见上表），`ToolPolicy` 作为防御性兜底；
- 写文件、shell 变更类操作仅 Coder 角色可用；Explorer / Reviewer 的 Bash 在提示层强制只读；
- 主 Agent 的审批规则不自动透传，子 Agent 工具调用按自身角色策略评估；
- 子 Agent 默认禁止执行 git 变更（commit/push/reset 等），除非父 Agent 显式要求。

## AGENTS.md

`AGENTS.md` 是 Neo 唯一的项目指令文件名。匹配不区分大小写（跨平台行为），但同一目录下存在多个仅大小写不同的变体属于阻塞性歧义（`Blocked: ambiguous AGENTS.md`）。任何地方都没有 `CLAUDE.md` 回退。项目指令是会话级状态：Neo 以持久化指令 epoch 的形式将其写入会话的 JSONL 事件流，绝不改写系统 prompt 或此前的请求字节。

内容建议写项目规范、约定、构建命令等稳定信息，而不是频繁变化的细节。它与技能互补：`AGENTS.md` 是"项目对所有 Agent 的全局说明"，技能是"可复用的任务流程"。

### 基线：全局、受信祖先与工作区根

新会话初始化时、首个用户消息之前，Neo 解析出一个基线指令 epoch，来源为：

1. `$NEO_HOME/AGENTS.md`（用户全局，始终受信）；
2. 主工作区文件系统祖先链上受信的 `AGENTS.md`，按从外到内排序，终止于工作区根。

项目指令仅在项目受信（`~/.neo/trust.json`）时加载。恢复自本特性之前的会话时，Neo 会在下一个活动轮次按当前磁盘状态建立全新基线，不会重建旧行为。

### 从工具路径发现的嵌套作用域

Neo 在工具运行前，仅从类型化工具参数推导嵌套 `AGENTS.md` 的发现范围——绝不解析 shell 命令字符串：

| 工具类别 | 作用域探针 |
| --- | --- |
| `Read`、`Write`、`Edit` | 目标文件的父目录 |
| `List`、`Grep`、`Find`、`Glob` | 显式 root 或 path 目录 |
| `Bash`、`Terminal` | 显式 `cwd`，否则为主工作区 |
| 其他工具 | 无指令作用域探针 |

打算在嵌套子树中执行命令时，必须设置工具的 `cwd` 字段；Neo 不会从命令字符串推断路径。对主工作区内的每个目标目录，Neo 只扫描从工作区根到目标目录的目录链，不遍历兄弟或后代目录：

```text
workspace/
|-- AGENTS.md
|-- crates/
|   |-- AGENTS.md
|   `-- neo-tui/
|       |-- AGENTS.md
|       `-- src/lib.rs
`-- docs/AGENTS.md   # not loaded for crates/neo-tui/src/lib.rs
```

当批次触及新的或已变化的作用域时，预检会推迟整个工具批次——绝不部分执行——追加一个指令 epoch，模型在同一轮内重新规划。规则按从一般到具体渲染：先全局，再受信祖先（由远及近），再工作区根，最后嵌套作用域（由浅到深），因此更深层的文件覆盖更宽泛的指导。目录中没有 `AGENTS.md` 不是错误，且缺失结果不跨轮缓存，因此新建文件会在其作用域内下一个工具执行前被发现。修改活动来源的工具仍受旧修订管辖；该工具完成后，Neo 追加更新或移除 epoch。重写相同内容不产生 epoch。

### 指令导入

只有位于围栏代码块之外、单独成行且只有一个前导 `@` 的行才是导入：

```md
@./docs/project-rules.md
@~/.neo/shared-rules.md
```

指向 `.md` 文件的本地 Markdown 链接同样是导入：

```md
执行前阅读[项目规则](./docs/project-rules.md)。
```

`[项目规则](./docs/project-rules.md)` 与独立成行的 `@./docs/project-rules.md` 会读取同一个文件，并遵循完全相同的递归、信任、去重和大小限制。两者只有呈现差异：Neo 会保留 Markdown 链接，而 `@` 指令会被导入正文替换。

Neo 保留原始链接，并紧随其后插入导入正文。图片、行内或围栏代码中的链接、URL、纯锚点链接都不会导入。对 `@` 而言，`@@./rules.md`、行内提及（如 `See @docs/rules.md`）、URL 和环境变量表达式也仍是普通文本。

解析规则：

- 相对路径从导入文件所在目录解析；`~` 使用平台 home 目录。
- 项目和祖先 bundle 只能从主工作区或 `$NEO_HOME` 导入。
- 用户全局 `$NEO_HOME/AGENTS.md` bundle 还可以导入平台 home 下的 Markdown；若项目不受信，其工作区子树仍被排除。
- 导入源必须是常规 UTF-8 `.md` 文件；目录、设备、套接字、URL 和其他特殊文件都会被拒绝。
- 规范化路径驱动循环检测与去重；同一来源被多次导入时，仅在首次出现处展开。
- 导入内容包裹在 `<included_instructions path="...">` 来源标记中；它会替换 `@` 指令，或跟在保留的 Markdown 链接之后。源内容保持精确的 UTF-8 文本。

结构性安全上限（最大递归导入深度 5、单个导入图最多 32 个来源、单来源最大 1 MiB、完整图最大 8 MiB）是宿主安全限制，不是模型上下文预算：

| 限制 | 值 |
| --- | ---: |
| 最大递归导入深度 | 5 |
| 单个导入图最大来源数 | 32 |
| 单来源最大字节数 | 1 MiB |
| 完整图最大字节数 | 8 MiB |

一个 `AGENTS.md` 加上其完整递归导入图构成一个原子 bundle：要么整体激活，要么完全不激活；Neo 绝不会把部分解析的导入图当作完整图呈现。

### 信任与文件系统边界

- `$NEO_HOME/AGENTS.md` 是始终受信的用户全局指令；它可以导入 `$NEO_HOME` 或平台 home 下的 Markdown，但不能进入未受信的工作区子树。
- 项目 `AGENTS.md` 及其工作区内导入仅在主项目受信时加载。
- 向下发现绝不越过主工作区边界；工作区外的绝对路径 `Read` 和额外工作区根不触发作用域发现。
- 规范化包含性判断使用 `Path`/`PathBuf` 语义，而非字符串前缀比较。

### 失败语义与被阻塞的作用域

| 条件 | 结果 |
| --- | --- |
| 目录中没有 `AGENTS.md` | 该目录无作用域；不是错误 |
| 导入缺失 | `Blocked: missing import` |
| 权限或 I/O 故障 | `Blocked: unreadable source` |
| 非法 UTF-8 | `Blocked: invalid encoding` |
| 导入循环 | `Blocked: include cycle` |
| 超过结构上限 | `Blocked: instruction limit exceeded` |
| 规范化路径离开允许的根 | `Blocked: untrusted import` |
| 多个仅大小写不同的 `AGENTS.md` 变体 | `Blocked: ambiguous AGENTS.md` |
| 读取期间来源反复变化 | `Blocked: unstable source` |

失败的 bundle 不会注入已成功读取的子集；模型只收到一条包含路径与原因的紧凑失败通知。作用域被阻塞期间，只读的 `Read`、`List`、`Grep`、`Find`、`Glob` 可继续用于诊断，但 `Write`、`Edit`、`Bash`、`Terminal` 保持阻塞；含有其中任何一个的混合批次会整体阻塞。来源指纹变化后 Neo 自动重试解析；完整成功的 bundle 会按普通激活流程替换失败状态。

### 动态指令预算

指令内容是固定的请求上下文，由现有上下文估算器计数：

```text
nominal_instruction_budget = max(65_536, effective_max_tokens / 8)
actual_instruction_budget  = min(nominal_instruction_budget, 请求中可安全使用的 token)
```

`effective_max_tokens` 是 Neo 的有效模型上限（含观测到的 provider 溢出修正）。准入优先级依次为：全局 bundle、工作区根、嵌套 bundle（由深到浅）、受信祖先（由近及远）；渲染仍把更深层作用域放在最后，使其在项目级冲突中胜出。

若完整选择无法安全容纳，Neo 先压缩普通历史，再执行确定性的整 bundle 省略：能容纳的 bundle 激活，其余整体忽略，工作流在一次指令感知的模型重新规划后继续。Transcript 显示 `⚠ 指令部分加载` 警告，列出已加载与被忽略的 bundle 及 token 估算；模型不得声称遵守了被忽略的规则。相同选择与来源哈希不会重复告警；之后模型窗口、来源或作用域变化可能使被忽略的 bundle 变为可准入，此时 Neo 发出新的激活 epoch。

### 前缀缓存稳定性与压缩

指令变更是只追加的 epoch，绝不改写此前请求字节；因此在完整压缩之前，上一个 provider 请求始终是下一个请求的精确前缀。压缩摘要不包含指令正文，压缩后 Neo 从注册表逐字节恢复当前规则：全局指令、工作区基线、当前嵌套作用域链。重新进入此前被丢弃的兄弟作用域会发出 `Reactivated` epoch。Transcript 卡片（`◆ ready/loaded`、`↻ updated`、`− removed`、`⚠ 部分加载`、`✕ blocked`）只显示元数据——绝不显示指令正文，也绝不显示 home 目录绝对路径。

### 恢复与多 Agent 可见性

指令 epoch 是持久化的 JSONL 事件。恢复会话时，Neo 先回放历史事件、重建注册表与各 Agent 可见性状态，再在首个活动边界核对当前磁盘状态；无变化的会话不会产生重复 epoch 或卡片，来源有变化则追加替换或移除 epoch。

来源字节与修订图在会话内共享，但可见性按 Agent 隔离：主 Agent 与每个 Delegate 子 Agent 各自独立记录其模型已见过的修订。派生子 Agent 时，会在其首个模型请求前物化一个子级基线 epoch（全局、工作区与适用的父级作用域）；一个 Agent 的激活不代表另一个 Agent 可见，指令卡片也只留在各自 Agent 的 transcript 中。

### `/init` 与已移除的 `CLAUDE.md` 回退

`/init` 只创建或刷新工作区根目录的 `AGENTS.md`；嵌套的 `AGENTS.md` 由用户自行编写，`/init` 绝不生成或修改它们。旧的"仅启动时"加载器与 `CLAUDE.md` 兼容候选已被移除——`AGENTS.md` 是唯一规范的指令文件名。

## 下一步

- [技能系统](skills.md) — 用技能固化子 Agent 的工作流程
- [MCP 服务器](mcp.md) — 子 Agent 也可调用 MCP 工具
- [权限模式](../configuration/permissions.md) — 工具审批粒度
