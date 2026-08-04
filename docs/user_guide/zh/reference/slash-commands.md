# 斜杠命令参考

交互模式下，以 `/` 开头的输入由 `InteractiveController::handle_slash_command` 解析。本文列出全部内置斜杠命令。

源码位置：[`crates/neo-agent/src/modes/interactive/slash_commands.rs`](../../../crates/neo-agent/src/modes/interactive/slash_commands.rs) 与 `prompt_completion.rs` 中的 `STATIC_SLASH_COMMANDS`。

## 会话管理

| 命令 | 别名 | 说明 |
| --- | --- | --- |
| `/new` | — | 开启一个新的本地会话。 |
| `/clear` | `/new` | `/new` 的别名。 |
| `/resume` | — | 打开会话选择器，恢复某个本地会话。 |
| `/compact` | — | 手动压缩上下文；可附加指令，如 `/compact <instruction>`。 |
| `/tasks` | — | 打开任务浏览器：后台任务与工作流（Workflow）运行（phase、准入等待、等待输入、用量）。 |
| `/workflow` | — | 打开可搜索的有效工作流选择器。 |
| `/workflow <task>` | — | 开始一次普通模型回合，把完整有效的工作流目录交给模型选择。 |
| `/workflow:<name> <task>` | — | 开始一次普通模型回合，提供指定工作流定义与完整输入 schema。 |
| `/skill:create-workflow <request>` | — | 通过现有技能路径编写或修改工作流。 |
| `/fork` | — | 为当前会话创建一个新的分支并跳转。 |
| `/init [instruction]` | — | 只创建或刷新工作区根目录的 `AGENTS.md`；嵌套的 `AGENTS.md` 由用户自行编写，`/init` 绝不生成或修改。后续文本会作为自然语言指导传入 init 工作流。 |

`/init` 仅支持 TUI 交互模式。`/init`、`/skill:self-evo`、`/skill:create-skill` 等交互流在 Auto 模式下可能会在开始前打开本地预检。Neo 会根据解析到的斜杠命令机械地触发该预检；模型不能自行决定切换权限模式。

### `/workflow` 形态

`/workflow` 有四种形态，覆盖从选择器到直接执行的工作流交互：

| 形态 | 行为 |
| --- | --- |
| `/workflow` | 打开可搜索选择器。选择后只把 `/workflow:<name> ` 写入输入框，不启动回合。 |
| `/workflow <自然语言任务>` | 把完整有效目录交给一次可见模型回合。没有合适定义时，assistant 会先询问是否编写或改为普通执行。 |
| `/workflow:<name> <自然语言任务>` | 把指定定义和完整输入 schema 交给一次可见模型回合，由模型把任务转换为工作流输入。 |
| `/skill:create-workflow <编写请求>` | 单独进入工作流编写路径；使用已有保存工作流不需要先激活它。 |

斜杠匹配是精确的：`/workflowish` 和正文中的 `/workflow` 都只是普通提示词。语法错误或本地 registry 错误会保留原输入，不启动模型回合。模型选择工作流后，现有 Ask / Auto / YOLO 权限和工作流卡片继续生效。无界面模式（headless）下的 `neo workflow` 命令仍只供人类和脚本使用。见 [Workflows](../guides/workflows.md)。

## 模式控制

| 命令 | 别名 | 说明 |
| --- | --- | --- |
| `/plan` | — | 切换计划模式；参数：`on` / `off` / `clear`。 |
| `/goal` | — | 目标模式入口；参数如 `replace <obj>`、`next <obj>`。 |
| `/ask` | — | 切到 **Ask** 权限模式（每个风险操作前询问）。 |
| `/auto` | — | 切到 **Auto** 权限模式（非交互运行）。 |
| `/yolo` | — | 切到 **YOLO** 权限模式（跳过确认）。 |
| `/permissions` | `/permission` | 打开权限模式选择器。 |

> `/ask`、`/auto`、`/yolo` 即使在回合运行中也可以即时切换（实时生效）；`/theme <name-or-id>` 也可以在回合运行期间应用。其余斜杠命令需要先打断当前回合。

## 主题管理

| 命令 | 行为 |
| --- | --- |
| `/theme` | 打开主题管理器。需要主回合空闲；忙碌时 Neo 会保持回合继续，并提示「需要空闲」。 |
| `/theme <name-or-id>` | 立即应用到当前会话，回合运行期间也可用。解析是精确的：先按逻辑 `ThemeId`，再按唯一的精确显示名；不做模糊匹配。 |
| `/theme reload` | 清除当前会话的临时覆盖，重新应用由 `[tui].theme` 解析出的主题。 |
| `/skill:custom-theme` | 显式调用的 AI 辅助主题创建技能；先预览再保存，绝不自动应用。 |

管理器支持列表、过滤、预览、应用到会话、设为启动默认、导入（重名时选择 Overwrite / Save as new）、复制、删除（活动主题与启动默认主题受保护）和刷新。`/theme <name-or-id>` 只影响当前会话——不改变启动默认主题。见 [主题（Themes）](../customization/themes.md)。

## 信息与状态

| 命令 | 说明 |
| --- | --- |
| `/help` | 打开帮助面板，列出所有可用命令与技能。 |
| `/model [alias]` | 不带参数打开模型选择器；带参数切换到指定 alias。 |
| `/provider` | 打开服务商（provider）选择器，查看已配置的服务商。 |
| `/mcp` | 打开 MCP 管理面板，查看 / 管理 MCP server。 |
| `/btw [question]` | 打开临时侧边问答面板（"by the way" 旁路提问）。 |

## 退出

Neo 交互模式**没有** `/exit` 或 `/quit` 斜杠命令。退出方式见 [键盘快捷键 · 通用](keyboard.md)：

| 操作 | 快捷键 |
| --- | --- |
| 退出应用（提示词为空时） | `Ctrl+D`（500 ms 内再按一次确认） |
| 清空编辑器 / 中断轮次 | `Ctrl+C` |
| 挂起到后台 | `Ctrl+Z` |

## 内置技能

| 命令 | 说明 |
| --- | --- |
| `/skill:<name> [args]` | 激活名为 `<name>` 的技能，可接参数；支持同一行多个 `/skill:` 指令。 |

激活后会把技能内容作为上下文注入，并在对话记录（transcript）中显示 `SkillActivation` 卡片。可用技能列表可通过 `/help` 或提示词自动补全查看。

## 命令面板（非斜杠）

按 `Ctrl+P` 打开命令面板，内含未暴露为斜杠的命令，例如：`session.exportHtml`（导出 HTML）、`fork`（分叉会话）、`copy-prompt`、`select-transcript` 等。详见 [键盘快捷键](keyboard.md)。
