# 权限模式

Neo 在执行工具调用前会根据当前权限模式决定是否需要用户审批。权限模式由 `config.toml` 的 `permission_mode` 字段、CLI 标志（`--auto` / `--yolo`）以及交互式 TUI 中的 `/ask`、`/auto`、`/yolo`、`/permissions` 命令控制。

## 四种权限模式

Neo 实际运行时有 `Ask`、`Auto`、`Yolo` 三种命名模式，加上由 `EnterPlanMode` 工具激活的**计划模式**（Plan）。三者由 `PermissionMode` 枚举定义，计划模式作为额外的硬保护叠加在其上。

| 模式 | 字符串值 | 行为 |
| --- | --- | --- |
| **Ask** | `"ask"` | 默认。读类工具（`Read`/`List`/`Grep`/`Glob` 等）和已知安全命令自动放行；写、shell、tool 调用一律弹出审批对话框 |
| **Auto** | `"auto"` | 自动批准所有工具调用（包括 shell、写入、`ExitPlanMode`、`ExitGoalMode`）。`AskUserQuestion` 会被硬拒 |
| **Yolo** | `"yolo"` | 放行普通工具调用（含危险命令），并跳过项目信任检查，但仍会弹出 Plan/Goal 审阅对话框。仅在受控环境使用 |
| **Plan** | — | 由模型调用 `EnterPlanMode` 进入；只允许只读工具和写当前计划文件，`ExitPlanMode` 退出时需用户审批（Auto 除外） |

> 三种模式的优先级：CLI 标志 `--yolo` / `--auto` 覆盖配置文件；两者不能同时使用。运行中可用斜杠命令实时切换，正在进行的 turn 也会立即生效。

```toml
# config.toml
permission_mode = "ask"
```

```shell
# CLI 标志
neo --auto
neo --yolo
```

## 动态审批选项

需要审批时，对话框由运行时拥有的**单一请求**构建。**选项是动态的**：列表只包含运行时对该调用能兑现的动作——标签只是展示文案，不是另一套语义来源。

### 普通 Tool / Shell 审批

普通 Tool 与 Shell 审批可提供的动作：

| 选项 | 说明 | 存储位置 |
| --- | --- | --- |
| **Approve once**（单次） | 只放行这一次，下次仍需审批 | 不持久化 |
| **会话授权**（若可用） | 当前会话内匹配的操作自动放行 | 内存（`session_approvals`） |
| **前缀授权**（若可用） | 以后所有以该前缀开头的 shell 命令自动放行 | 磁盘（`~/.neo/approval_rules.json`） |
| **Reject** | 拒绝，返回 `approval denied` 给模型 | — |

普通 Tool/Shell 审批**不提供修订反馈**。仅当运行时能推导出安全的可复用范围时才会出现会话/前缀授权；两者都不可用时，对话框只显示单次批准与拒绝。

**后台 Bash** 永不提供可复用授权：会话与前缀选项会被省略，只剩单次 Approve 与 Reject。

### Plan 与 Goal 审阅

- **Plan（`ExitPlanMode`）**：Approve（可选带备选方案）退出计划模式并继续执行；**Reject** 与 **Revise** 保持计划模式激活，以便 Neo 修订方案。
- **Goal（`ExitGoalMode`）**：审阅界面展示 **objective**、**完成判据** 与 **phases**。Approve 启动目标；**Reject** 与 **Revise 不会创建目标**。
- **Ask** 与 **Yolo** 会弹出 Plan/Goal 审阅对话框；**Auto** 会跳过审阅，直接继续。

### 会话级（Layer 1）

会话级审批按**精确规范化键**记录，绝不按工具名缓存：

- **Shell**：`<workspace> + <cwd> + <argv>`。`git status` 和 `git log` 是两个不同的键；`git status && git push` 这类复合命令会作为整体不透明键记录，不会泄漏成单独的 `git status`。
- **File write/edit**：`<workspace> + <path> + <operation>`。Write 和 Edit 是两套独立键。
- **Tool**：`<workspace> + <fully-qualified tool name>`（主要针对 MCP 工具）。

> 跨 workspace 隔离：所有键都带 workspace 根路径，session 复用时不会泄漏授权。

### 前缀级（Layer 2）

前缀级规则以 token 前缀匹配（不是子串），持久化在 `~/.neo/approval_rules.json`，重启后仍生效：

```json
{
  "prefix_rules": [
    { "prefix": ["git"], "label": "git" },
    { "prefix": ["cargo", "test"], "label": "cargo test" }
  ]
}
```

- 空前缀会被拒绝（防止「批准所有命令」）；
- 复合命令（含 `&&`、`|`、`;` 等）不会生成前缀规则，因为其前缀不是稳定的 argv 前缀；
- 危险命令（`rm -rf`、`sudo`、`curl | sh` 等）永远强制弹审批，不会生成任何可复用授权。

### 命令安全分级（Layer 3）

在 Ask 模式下，Neo 会先做命令分级，决定是否跳过审批：

- **已知安全**：`ls`、`cat`、`git status`、`git log`、`cargo test` 等只读子命令——自动放行。
- **危险命令**：`rm -rf`、`sudo`、`chmod`、`curl ... | sh` 等——强制弹审批，即使有前缀规则也必须确认。
- **其他**：常规弹审批。

## 权限决策流程（用户视角）

从工具调用发起到执行，Neo 按以下顺序短路判断（任何一层命中就返回）：

1. **Plan 模式硬保护**：若处于 Plan 模式且该工具不在只读白名单内 → 直接拒绝。
2. **Auto / 背景 AskUser**：Auto 模式拒绝 `AskUserQuestion`、批准其余（包括 Plan/Goal 退出）；后台 `AskUserQuestion` 永不弹窗；`EnterPlanMode` 在所有模式下自动放行。
3. **前缀规则（Layer 2）**：命中持久化前缀 → 放行。
4. **会话缓存（Layer 1）**：命中本会话已批准的精确键 → 放行。
5. **状态转换工具**：`ExitPlanMode` / `ExitGoalMode` 在 Ask 与 Yolo 下弹出审阅对话框（Auto 已在第 2 步放行）。
6. **Yolo 模式**：放行剩余普通工具调用（Plan/Goal 审阅已在上面处理）。
7. **安全分级**：安全命令放行；危险命令强制弹窗；默认审批工具（`Read`/`List`/`Grep`/`Find`/`Glob`/`TodoList`/`TaskList`/`TaskOutput`/`Skill`/`AskUserQuestion`/`Sleep`）放行。
8. **兜底**：弹出审批对话框，仅包含运行时对该调用能兑现的动作（单次批准、可选的会话/前缀授权、拒绝——普通工具无修订反馈）。

> 实时性：`/ask`、`/auto`、`/yolo`、`/permissions` 切换的模式会立刻生效——不需要取消当前 turn，下一次工具调用就会按新模式评估。

## 下一步

- [配置文件总览](config-files.md) — `permission_mode` 字段位置
- [Provider 配置](providers.md) — 模型与端点
- [数据存储位置](data-locations.md) — `approval_rules.json` 在哪
