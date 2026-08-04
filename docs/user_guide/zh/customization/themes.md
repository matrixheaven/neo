# 主题（Themes）

Neo TUI 的配色由 `TuiTheme` 结构定义（见 `crates/neo-tui/src/primitive/theme.rs`），可以通过 JSON 主题文件覆盖默认配色。`$NEO_HOME/themes/`（默认 `~/.neo/themes/`）是**唯一**受管理的主题目录：把 `.json` 文件放进去，它就会成为目录条目，可被主题管理器、`/theme` 和启动解析使用。示例：[`examples/config/magenta-dark.json`](../../../../examples/config/magenta-dark.json) 展示了文件结构——注意该示例使用的是旧版键名，详见下方 token 表后的说明。

## JSON 主题格式

主题文件是一个顶层对象，`colors` 下每个键对应一个语义颜色 token，值是一个颜色字符串：

```json
{
  "name": "magenta-dark",
  "colors": {
    "brand": "#C678DD",
    "status_ok": "#4EC87E",
    "status_error": "#E85454"
  }
}
```

| 字段 | 说明 |
| --- | --- |
| `name` | 可选；缺省时取文件名 stem |
| `colors` | 颜色 token 表，所有键可选，缺省沿用默认主题 |

颜色值支持三种写法：

| 写法 | 示例 | 说明 |
| --- | --- | --- |
| `#RRGGBB` | `"#C678DD"` | 24 位真彩色，推荐 |
| 命名色 | `"darkgray"` | ANSI 命名色 |
| `reset` | `"reset"` | 跟随终端默认 |

> 加载器对未知键严格报错（`deny_unknown_fields`），写错 token 名会直接加载失败。请按下表精确对齐。

## 颜色 Token 表

| Token | 默认用途 |
| --- | --- |
| `text_primary` | 正文文字 |
| `prompt` | 提示符 / 输入框前景 |
| `brand` | 品牌主色（浮层边框、选中高亮） |
| `status_ok` | 成功状态 |
| `status_error` | 错误 / 危险 |
| `status_warn` | 警告 / 审批标题 |
| `status_pending` | 待定状态 |
| `status_cancelled` | 已取消状态 |
| `text_muted` | 次要 / 灰色文字 |
| `user_message` | 用户消息颜色 |
| `diff_added` | diff 新增行 |
| `diff_removed` | diff 删除行 |
| `diff_hunk` | diff hunk 头 |
| `diff_context` | diff 上下文行 |
| `selection_bg` | 选择背景 |
| `approval_border` | 审批对话框边框 |
| `selected_fg` / `selected_bg` | 选中项前景 / 背景 |
| `overlay_border` | 浮层边框 |
| `footer_permission_allow` | 底栏：允许 |
| `footer_permission_ask` | 底栏：询问 |
| `footer_permission_deny` | 底栏：拒绝 |
| `footer_working` | 底栏：工作中 |
| `footer_context_ok` | 底栏：上下文充足 |
| `footer_context_warn` | 底栏：上下文告警 |
| `footer_context_critical` | 底栏：上下文临界 |
| `shell_mode` | shell 模式标识色 |

> 注意：`examples/config/magenta-dark.json` 使用的 `accent` / `success` / `danger` 等是旧版别名，**当前加载器不再识别**。请使用上表的 `brand` / `status_ok` / `status_error` 等新键。下方示例已用新 schema。

## 示例

一份完整的暗色主题（`~/.neo/themes/magenta-dark.json`）：

```json
{
  "name": "magenta-dark",
  "colors": {
    "brand": "#C678DD",
    "status_ok": "#4EC87E",
    "status_error": "#E85454",
    "status_warn": "#E8A838",
    "text_muted": "#8B949A",
    "text_primary": "#C6D0F5",
    "prompt": "#C6D0F5",
    "user_message": "#E5C890",
    "diff_added": "#4EC87E",
    "diff_removed": "#E85454",
    "diff_hunk": "#E8A838",
    "diff_context": "#8B949A",
    "footer_permission_ask": "#C678DD",
    "footer_working": "#C678DD"
  }
}
```

主题仓库（`crates/neo-agent/src/themes.rs`）：

- `$NEO_HOME/themes/` 是唯一主题位置；其中的每个 `*.json` 都是目录条目，格式错误的文件只是无效条目，不会隐藏其他有效主题。
- `[tui].theme` 是相对 `$NEO_HOME/themes/` 的**逻辑 id**，绝不是绝对路径。启动时 Neo 解析该精确 id；如果缺失或无效，Neo 使用内置 `TuiTheme::default()` 启动并给出可见诊断——不会静默选择其他 JSON 文件，也不会自动改写配置。
- 未设置 `[tui].theme` 时，Neo 保留旧版「按文件名排序取第一个」的发现逻辑，作为有边界的兼容回退。
- 解析失败会明确报错，绝不静默回退。

更多示例参见 [`examples/config/`](../../../../examples/config/) 目录。

## /theme 命令

| 形式 | 行为 |
| --- | --- |
| `/theme` | 打开主题管理器。需要主回合处于空闲状态；回合运行中时 Neo 会保持回合继续，并提示「需要空闲」。 |
| `/theme <name-or-id>` | 立即应用到**当前会话**，包括模型回合运行期间。解析是精确的：先按逻辑 `ThemeId`，再按唯一的精确显示名。不做模糊或前缀匹配——未知或歧义名称会给出本地错误。 |
| `/theme reload` | 清除当前会话的临时覆盖，重新应用由 `[tui].theme` 解析出的主题。 |

`/theme <name-or-id>` 只改变当前会话——不写 `config.toml`，也不改变启动默认主题。

### 主题管理器

裸 `/theme` 打开的管理器包含列表、过滤和预览面板。选中条目只预览，不做任何应用，直到你选择某个动作：

| 动作 | 效果 |
| --- | --- |
| 应用到会话（Apply for session） | 把当前 TUI 会话切换到所选主题；不写配置。会话的临时覆盖在无关的配置刷新后仍然保留。 |
| 设为启动默认（Set startup default） | 把逻辑 id 写入 `[tui].theme`；当前会话不变。 |
| 导入（Import） | 校验并把外部主题文件复制进 `$NEO_HOME/themes/`。目标重名时必须显式选择——覆盖（Overwrite）或另存为新主题（Save as new）；不存在静默覆盖。 |
| 复制（Copy） | 以新的显示名复制所选主题。 |
| 删除（Delete） | 确认后删除受管理的主题。当前活动主题和启动默认主题受保护，直到你应用或设置其他主题。 |
| 刷新（Refresh） | 重新扫描 `$NEO_HOME/themes/` 并重新解析目录。 |

管理器适配窄终端：一次只渲染一个聚焦面板，焦点显示在标题行。

### 启动默认

启动时主题由 `[tui].theme` 解析（见 [配置文件总览](../configuration/config-files.md)）；字段缺失且不存在任何 JSON 文件时，使用内置 `TuiTheme::default()`（magenta 暗色调）。想用 AI 辅助创建主题，请使用显式调用的 `custom-theme` 技能——它先预览再保存，绝不自动应用，应用交由 `/theme` 完成。见 [技能系统](skills.md)。

## 下一步

- [技能系统](skills.md) — `custom-theme` 技能的完整流程
- [配置文件总览](../configuration/config-files.md) — 主题目录位置
- [交互指南](../guides/interaction.md) — TUI 各区域与颜色含义
