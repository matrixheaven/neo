# 斜杠命令参考

交互模式下，以 `/` 开头的输入由 `InteractiveController::handle_slash_command` 解析。本文列出全部内置斜杠命令。

源码位置：[`crates/neo-agent/src/modes/interactive/slash_commands.rs`](../../../crates/neo-agent/src/modes/interactive/slash_commands.rs) 与 `prompt_completion.rs` 中的 `STATIC_SLASH_COMMANDS`。

## 会话管理

| 命令 | 别名 | 说明 |
| --- | --- | --- |
| `/new` | — | 开启一个新的本地会话。 |
| `/clear` | `/new` | `/new` 的别名。 |
| `/resume` | — | 打开 session 选择器，恢复某个本地会话。 |
| `/compact` | — | 请求手动压缩上下文；可附加指令 `/compact <instruction>`。 |
| `/tasks` | — | 打开任务浏览器：后台任务与 workflow run（phase、准入等待、等待输入、用量）。 |
| `/workflow` | — | 打开可搜索的有效工作流选择器。 |
| `/workflow <task>` | — | 开始普通模型回合，并提供完整有效工作流目录，由模型选择已有工作流。 |
| `/workflow:<name> <task>` | — | 开始普通模型回合，并提供指定工作流定义和完整输入 schema。 |
| `/skill:create-workflow <request>` | — | 通过现有 skill 路径编写或修改工作流。 |
| `/fork` | — | 为当前会话创建一个新的分支并跳转。 |
| `/init [instruction]` | — | 仅创建或刷新工作区根目录的 `AGENTS.md`；嵌套的 `AGENTS.md` 由用户自行编写，`/init` 绝不生成或修改。后续文本会作为自然语言指导传入 init 工作流。 |

`/init` 仅支持 TUI 交互模式。`/init`、`/skill:self-evo`、`/skill:create-skill` 等交互流在 Auto 模式下可能会在开始前打开本地预检。Neo 会根据已解析的 slash command 机械触发该预检；模型不能自行决定切换权限模式。

### `/workflow` 形态

| 形态 | 行为 |
| --- | --- |
| `/workflow` | 打开可搜索选择器。选择后只把 `/workflow:<name> ` 写入 composer，不启动回合。 |
| `/workflow <自然语言任务>` | 把完整有效目录交给一次可见模型回合。没有合适定义时，assistant 会先询问是否编写或改为普通执行。 |
| `/workflow:<name> <自然语言任务>` | 把指定定义和完整输入 schema 交给一次可见模型回合，由模型把任务转换为工作流输入。 |
| `/skill:create-workflow <编写请求>` | 单独进入工作流编写路径；使用已有保存工作流不需要先激活它。 |

Slash 匹配是精确的：`/workflowish` 和正文中的 `/workflow` 都是普通提示。语法或 registry 本地错误会保留原输入，不启动模型回合。模型选择工作流后，现有 Ask / Auto / Yolo 权限和工作流卡片继续生效。headless `neo workflow` 命令仍只供人类和脚本使用。见 [Workflows](../guides/workflows.md)。

## 模式控制

| 命令 | 别名 | 说明 |
| --- | --- | --- |
| `/plan` | — | 切换计划模式；参数：`on` / `off` / `clear`。 |
| `/goal` | — | 目标模式入口；参数如 `replace <obj>`、`next <obj>`。 |
| `/ask` | — | 切到 **Ask** 权限模式（每个风险操作前询问）。 |
| `/auto` | — | 切到 **Auto** 权限模式（非交互运行）。 |
| `/yolo` | — | 切到 **Yolo** 权限模式（跳过确认）。 |
| `/permissions` | `/permission` | 打开权限模式选择器。 |

> `/ask`、`/auto`、`/yolo` 即便在 turn 运行中也可即时切换（实时生效），其余斜杠命令需要先打断当前 turn。

## 信息与状态

| 命令 | 说明 |
| --- | --- |
| `/help` | 打开帮助面板，列出所有可用命令与 skill。 |
| `/model [alias]` | 不带参数打开模型选择器；带参数切换到指定 alias。 |
| `/provider` | 打开 provider 选择器，查看已配置的提供方。 |
| `/mcp` | 打开 MCP 管理面板，查看 / 管理 MCP server。 |
| `/btw [question]` | 打开临时侧边问答面板（"by the way" 旁路提问）。 |

## 退出

Neo 交互模式**没有** `/exit` 或 `/quit` 斜杠命令。退出方式见 [键盘快捷键 · 通用](keyboard.md)：

| 操作 | 快捷键 |
| --- | --- |
| 退出应用（prompt 为空时） | `Ctrl+D`（500 ms 内再按一次确认） |
| 清空编辑器 / 中断 turn | `Ctrl+C` |
| 挂起到后台 | `Ctrl+Z` |

## 内置 Skill

| 命令 | 说明 |
| --- | --- |
| `/skill:<name> [args]` | 激活名为 `<name>` 的 skill，可接参数；支持同一行多个 `/skill:` 指令。 |

激活后会把 skill 内容作为上下文注入，并在 transcript 中显示 `SkillActivation` 卡片。可用 skill 列表通过 `/help` 或 prompt 自动补全查看。

## 命令面板（非斜杠）

按 `Ctrl+P` 打开命令面板，内含未暴露为斜杠的命令，例如：`session.exportHtml`（导出 HTML）、`fork`（分叉会话）、`copy-prompt`、`select-transcript` 等。详见 [键盘快捷键](keyboard.md)。
