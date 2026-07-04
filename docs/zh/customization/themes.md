# 主题（Themes）

Neo TUI 的配色由 `TuiTheme` 结构定义（见 `crates/neo-tui/src/primitive/theme.rs`），可通过 JSON 主题文件覆盖默认配色。把 `.json` 放进 `~/.neo/themes/` 即被发现加载。示例：[`examples/config/magenta-dark.json`](../../../examples/config/magenta-dark.json)。

## JSON 主题格式

主题文件是一个顶层对象，`colors` 下每个键对应一个语义颜色 token，值是颜色字符串：

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
| `brand` | 品牌主色（覆盖层边框、选中高亮） |
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

加载机制（`crates/neo-agent/src/themes.rs`）：

- 扫描 `~/.neo/themes/` 下所有 `.json`，按文件名排序取首个；
- 相对路径以 `$NEO_HOME` 为基准，支持 `~/` 展开；
- 解析失败会在启动时报错，不会静默回退。

更多示例参见 [`examples/config/`](../../../examples/config/) 目录。

## /theme 命令

| 操作 | 说明 |
| --- | --- |
| `/theme <name>` | 切换到 `~/.neo/themes/<name>.json` |
| `custom-theme` 技能 | 交互式引导：选基础色 → 选 token → 预览 → 保存（`/skill:custom-theme`） |

主题切换在交互式 TUI 内即时生效；启动时默认主题由 `resolve_theme()` 决定，未发现任何 JSON 文件时使用内置 `TuiTheme::default()`（magenta 暗色调）。

## 下一步

- [技能系统](skills.md) — `custom-theme` 技能的完整流程
- [配置文件总览](../configuration/config-files.md) — 主题目录位置
- [交互指南](../guides/interaction.md) — TUI 各区域与颜色含义
