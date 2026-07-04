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

项目根目录下的 `AGENTS.md`（大小写不敏感，`CLAUDE.md` 作为兼容候选）是项目级上下文文件，Neo 在受信任目录下会自动读取并注入主 Agent 上下文。要点：

- 优先级：`AGENTS.md` 高于 `CLAUDE.md`；
- 扫描范围：当前工作目录及其受信祖先目录，只取首个匹配；
- 内容建议写项目规范、约定、构建命令等稳定信息，而不是会重复变化的细节；
- 与技能互补：`AGENTS.md` 是"项目对所有 Agent 的全局说明"，技能是"可复用的任务流程"。

子 Agent 默认不直接读 `AGENTS.md`，但父级上下文（`inherit` / `summary`）会把它作为项目背景带上。

## 下一步

- [技能系统](skills.md) — 用技能固化子 Agent 的工作流程
- [MCP 服务器](mcp.md) — 子 Agent 也可调用 MCP 工具
- [权限模式](../configuration/permissions.md) — 工具审批粒度
