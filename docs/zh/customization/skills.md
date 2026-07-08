# 技能（Skills）

技能是可复用的 Markdown 指令包，让 Neo 把"怎么做某类任务"沉淀成文件。技能由 `SKILL.md` 定义，运行时按四层优先级扫描，可由模型自动激活或用 `/skill:<name>` 手动触发。核心实现见 `crates/neo-agent-core/src/skills/`。

## 什么是技能

一个技能 = 一个目录 + 一份 `SKILL.md`。`SKILL.md` 顶部是 YAML frontmatter（元数据），下面是 Markdown 正文（给模型的指令）。技能不是代码，而是结构化的提示：

- **何时用**：模型根据 `whenToUse` 自动选择，或用户用斜杠命令显式调用；
- **怎么用**：正文在激活时注入上下文，引导模型按既定步骤完成任务；
- **能复用**：跨会话、跨项目，团队可共享目录。

技能扫描由 `SkillStore::load` 统一加载，源码定义在 `crates/neo-agent-core/src/skills/mod.rs`。

## SKILL.md 格式

```markdown
---
name: deploy-staging
description: Deploys the app to staging. Use when the user asks to deploy.
type: prompt
whenToUse: When the user asks to deploy to staging or update the staging environment.
---

# Deploy to Staging

## Steps
1. Run `cargo build --release`
2. ...
```

### Frontmatter 字段

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `name` | ✅ | 技能标识，需与目录名一致；嵌套目录会形成 `parent/child` 命名 |
| `description` | ✅ | 一句话功能说明，供模型选择时参考 |
| `type` | ✅ | `prompt`（注入为上下文消息，默认）/ `inline`（直接展开进提示）/ `flow`（多步交互式工作流） |
| `whenToUse` | 推荐 | 自然语言触发描述，用于自动激活 |
| `disableModelInvocation` | bool | `true` 时禁止模型自动调用，仅响应 `/skill:<name>` |
| `arguments` | array | 声明式参数（`name` / `description` / `required` / `default`） |
| `slashCommands` | array | 额外绑定的斜杠命令别名 |

> `type: flow` 的技能永远不参与自动激活；`disableModelInvocation: true` 同样会排除自动激活，二者其一为真即为手动型技能。

## 四层扫描优先级

Neo 启动时按以下顺序扫描技能，**同名技能高优先级覆盖低优先级**：

| 优先级 | 来源 | 路径 | 用途 |
| --- | --- | --- | --- |
| 1 | **user** | `~/.neo/skills/`、`~/.neo/.agents/skills/` | 用户私有技能，最高优先 |
| 2 | **extra** | 配置中 `extra_skill_dirs` / `skill_path` 指向的目录 | 团队共享目录 |
| 3 | **builtin** | `~/.neo/skills/.builtin/`（首次启动从二进制解压） | Neo 内置技能 |

加载顺序实际为：先解压内置技能到 `.builtin/`（已存在的用户编辑会被保留），再依次注入 extra、user 层；user 层最后写入 `HashMap`，因此**用户技能可覆盖同名内置技能**。目录支持嵌套，父目录有自己的 `SKILL.md` 时，子技能名会被前缀化为 `parent/child`。

```toml
# config.toml —— 追加团队共享技能目录
extra_skill_dirs = ["~/work/team-skills", "/srv/neo-skills"]
skill_path = ["~/work/more-skills"]
```

## 内置技能列表

Neo 自带以下技能（源码位于 `crates/neo-agent-core/src/skills/builtin/`）：

| 技能 | 类型 | 说明 |
| --- | --- | --- |
| `mcp-config` | prompt | 配置 MCP server、处理 OAuth 登录、编辑 `[[mcp.servers]]` |
| `sub-skill` | prompt | 审视、分组、重组技能库为层级子技能包 |
| `self-evo` | prompt | 把明确的当前、近期、会话或主题范围总结成可复用技能 |
| `create-skill` | prompt | 按用户需求创建 Neo skill，并包含验证说明 |

`self-evo` 和 `create-skill` 这类工作流创作型内置技能使用 `disableModelInvocation: true`，需要用户显式调用。Neo 会从当前二进制刷新 `~/.neo/skills/.builtin/` 下的内置技能；自定义副本应放在 `.builtin/` 之外。

`/skill:self-evo` 不带参数时会先询问蒸馏范围，再创建技能。在 Auto 权限模式下，Neo 会在模型回合开始前打开交互预检，避免无人值守运行中途才停下来等待用户回答。

`/skill:create-skill` 通过 `CreateSkill` 工具创建一个聚焦的 skill。如果没有提供需求，它会先询问要创建的能力再起草。创建出的 skill 会包含验证说明；`CreateSkill` 成功后会重新加载当前会话的 skill store。

## 激活方式

| 方式 | 触发者 | 行为 |
| --- | --- | --- |
| 模型自动调用 | 模型 | 命中 `whenToUse` 且未被禁用自动激活，正文注入上下文 |
| `/skill:<name>` | 用户 | 在 TUI 输入框直接调用，支持 `parent/child` 嵌套名 |
| `Skill` 工具 | 模型 | 编程式调用，常被其他技能编排 |
| `mcp__<server>__authenticate` | 模型 / 用户 | `mcp-config` 技能下处理 MCP OAuth 的专用工具 |

模型自动激活的前提：`disableModelInvocation` 为 false 且 `type` 非 `flow`（由 `SkillManifest::auto_invokable` 判定）。

## 创建自定义技能

### 用 `CreateSkill` 工具

模型可在对话中直接调用 `CreateSkill` 工具生成技能文件：

```jsonc
// 调用参数
{
  "name": "deploy-staging",
  "description": "Deploys the app to staging.",
  "skill_type": "prompt",        // prompt / inline / flow，默认 prompt
  "body": "# Deploy to Staging\n\n## Steps\n1. ..."  // 纯 Markdown，不要带 frontmatter
}
```

工具会自动生成 frontmatter，写入 `~/.neo/skills/<name>/SKILL.md`；同名旧文件会备份到 `~/.neo/backups/skills/<timestamp>/`。

### 手动创建

```bash
mkdir -p ~/.neo/skills/deploy-staging
$EDITOR ~/.neo/skills/deploy-staging/SKILL.md
```

文件需以 `---` 开头的 YAML frontmatter 开始，正文为 Markdown。下次启动 Neo 或重新扫描技能后即可用 `/skill:deploy-staging` 调用。

### 管理工具

| 工具 | 作用 |
| --- | --- |
| `ListSkills` | 按层级列出所有已发现技能（`include_builtin=true` 含内置） |
| `CreateSkill` | 创建新技能，自动备份旧文件 |
| `MoveSkill` | 把技能目录移动到父 bundle 下，重新分组 |

> 经验法则：把会重复出现的多步流程、踩过的坑、错误恢复流程写成技能；一次性的琐碎任务不必沉淀，已经写在 `AGENTS.md` 的内容也不必重复。

## 下一步

- [MCP 服务器](mcp.md) — `mcp-config` 技能配合 MCP 使用
- [子 Agent](agents.md) — 把技能与子 Agent 编排结合
- [配置文件总览](../configuration/config-files.md) — `extra_skill_dirs` 字段位置
