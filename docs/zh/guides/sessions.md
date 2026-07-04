# 会话管理

Neo 把每一次对话都保存为一份**本地 JSONL 记录**——可恢复、可分叉、可压缩、可导出。本页介绍会话持久化模型与常用操作。

## 会话持久化概念

| 概念 | 说明 |
| --- | --- |
| **会话（session）** | 一条 JSONL 事件流，包含 system / user / assistant / tool / shell 消息 |
| **存储位置** | 按工作区隔离：`~/.neo/sessions/wd_<slug>_<hash12>/agents/main/wire.jsonl` |
| **元数据** | 名称、摘要、父/子关系等存于会话目录下的元数据文件 |
| **会话 ID** | 形如 `session_<uuid>`，可用全名或 JSONL 路径引用 |

会话记录与供应商无关——所有事件都被归一化为 `AgentEvent`，JSONL 永不依赖具体的供应商协议。

> 工作区隔离意味着不同项目目录的会话互不干扰；切换目录即切换可见的会话池。

## 恢复会话

### 命令行

```bash
neo -c                          # 继续当前工作区最近的一次会话
neo -r                          # 打开会话选择器（仅 TTY）
neo resume <session-id>         # 恢复指定会话；非 TTY 时打印其 transcript
```

| flag | 行为 |
| --- | --- |
| `-c` / `--continue` | 直接恢复最近会话 |
| `-r` / `--resume` | 启动时打开会话选择器 |
| `--no-session` | 本次不落盘为新会话（适合一次性脚本） |

`-c`、`-r`、`--no-session` 三者互斥。

### TUI 内

在交互界面输入 `/resume` 或按会话选择器快捷键即可打开选择器；可在「当前工作区」与「所有工作区」之间切换作用域，选中后回车加载。

## 分叉会话 /fork

分叉会从某个已有会话的当前状态复制出一条新的独立分支，原会话保持不变，适合「在这一点试两条路」。

```bash
neo sessions fork <session-id> --name "experiment-A"
```

在 TUI 会话选择器中，选中目标会话后按会话分叉快捷键即可分叉。

分叉后的会话在列表里会带上 `parent=<id>` 标记，原会话的 `children` 字段会记录新会话 ID。

## 压缩会话 /compact

长会话会逼近上下文窗口。`/compact` 用 LLM 把旧消息总结为一段压缩摘要，保留最近的若干条原始消息。

```bash
# 命令行：压缩指定会话，保留最近 20 条
neo sessions compact <session-id> --keep-recent 20
```

```text
# TUI 内
/compact                      # 用默认策略压缩
/compact 只保留与认证模块相关的部分   # 附带自然语言指令
```

压缩后会话文件被改写：被压缩的消息替换为一条 `CompactionSummary`，新生成的对话继续追加在其后。`neo resume` 读取时会自动还原压缩摘要。

## 导出会话

Neo 当前支持 HTML 与 JSON 两种导出格式（Markdown 导出可通过 JSON 自行转换）：

| 命令 | 输出 |
| --- | --- |
| `neo sessions export-html <session-id>` | 带样式的可读 HTML |
| `neo sessions export-json <session-id>` | 结构化 JSON（schema `neo.session.export_json`，v1） |

```bash
neo sessions export-html session_abc123 > talk.html
neo sessions export-json session_abc123 > talk.json
```

JSON 产物包含会话元数据（`id` / `name` / `summary` / `parent_id` / `children` / `message_count`）与完整消息列表，便于归档或二次处理。

## 其他会话命令

| 命令 | 作用 |
| --- | --- |
| `neo sessions list` | 列出当前工作区会话（含名称、父/子、摘要） |
| `neo sessions show <id>` | 打印会话原始 JSONL |
| `neo sessions rename <id> <name>` | 给会话命名 |

会话的 `summary` 由 Neo 在运行过程中自动生成；可通过命名与摘要快速在会话选择器里识别历史会话。

## 存储位置一览

| 内容 | 路径 |
| --- | --- |
| 主配置 | `~/.neo/config.toml` |
| 会话根目录 | `~/.neo/sessions/` |
| 工作区会话桶 | `~/.neo/sessions/wd_<slug>_<hash12>/` |
| 主 Agent 记录 | `<桶>/agents/main/wire.jsonl` |
| 目标/计划/任务 | `<桶>/agents/main/{goals,plans,tasks}/` |

设置 `NEO_HOME` 环境变量即可整体迁移这些数据的位置。详见 [数据位置参考](../configuration/data-locations.md)。

## 下一步

- [交互模式](interaction.md) — `/resume`、`/fork`、`/compact` 的交互用法
- [目标模式](goals.md) — 会话级目标与阶段产物
- [数据位置参考](../configuration/data-locations.md)
