# 命令行参考

本页列出 Neo 的全部 CLI 命令。每条命令都可用 `neo <command> --help` 查看实时帮助，本文表格中的用法以当前版本的输出为准。

顶层命令有：`run`、`resume`、`sessions`、`provider`、`models`、`mcp`、`rpc`、`trust`、`workflow`、`update`、`uninstall` 与 `help`。不带子命令直接运行 `neo` 会进入交互式 TUI。

## 全局参数

用法：`neo [OPTIONS] [COMMAND]`

| 参数 | 说明 |
| --- | --- |
| `-r, --resume` | 启动时打开会话选择器 |
| `-c, --continue` | 继续最近一次会话 |
| `--no-session` | 本次运行不创建新会话 |
| `--yolo` | YOLO 权限模式 |
| `--auto` | Auto 权限模式 |
| `--config <CONFIG>` | 指定配置文件，也可用环境变量 `NEO_CONFIG` |
| `--verbose` | 详细启动诊断 |
| `-h, --help` | 显示帮助信息 |
| `-V, --version` | 显示版本号 |

## neo run

用途：执行一次性的提示词任务，把结果输出到 stdout，适合脚本化调用。

用法：`neo run [OPTIONS] [PROMPT]...`

| 参数 | 说明 |
| --- | --- |
| `PROMPT...` | 要发送的提示词文本，可传多个 |
| `--output <OUTPUT>` | 输出格式：`events`（原始事件流）、`json`（JSON 输出）、`text`（纯文本输出） |

## neo resume

用途：恢复指定会话并进入交互模式；非 TTY 环境打印该会话的对话记录。

用法：`neo resume [SESSION_ID]`

| 参数 | 说明 |
| --- | --- |
| `SESSION_ID` | 要恢复的会话 ID |

## neo sessions

用途：管理会话——列出、查看、重命名、分叉、压缩与导出。

用法：`neo sessions <SUBCOMMAND>`

| 子命令 | 说明 |
| --- | --- |
| `list` | 列出当前工作区的会话 |
| `show` | 显示会话详情 |
| `rename` | 重命名会话 |
| `fork` | 分叉会话 |
| `compact` | 压缩会话历史，用 `--keep-recent <N>` 保留最近 N 条消息（默认 20） |
| `export-html` | 导出为 HTML |
| `export-json` | 导出为 JSON |

## neo models

用途：管理模型别名。

用法：`neo models <SUBCOMMAND>`

| 子命令 | 说明 |
| --- | --- |
| `list` | 列出可用模型 |
| `add` | 添加模型别名；参数：`--provider`、`--model`、`--max-context-tokens`、`--capabilities`、`--display-name` |
| `remove` | 移除模型别名 |
| `set` | 设为默认模型 |

## neo provider

用途：管理服务商（provider）。

用法：`neo provider <SUBCOMMAND>`

| 子命令 | 说明 |
| --- | --- |
| `list` | 列出已配置或可用的服务商 |
| `add` | 添加自定义服务商；参数：`--type`、`--base-url`、`--api-key`、`--api-key-env` |
| `remove` | 移除服务商 |
| `catalog` | models.dev 目录管理，含 `list`、`add` 等子命令 |

## neo mcp

用途：管理 MCP 服务器——添加、删除、启停、状态检查与资源读取。

用法：`neo mcp <SUBCOMMAND>`

| 子命令 | 说明 |
| --- | --- |
| `list` | 列出所有已配置的 MCP 服务器及工具名 |
| `add` | 添加并测试服务器 |
| `del` | 删除服务器 |
| `enable` | 启用服务器 |
| `disable` | 停用服务器 |
| `status` | 显示连接状态、工具数与最近错误 |
| `resources` | 列出资源 |
| `read-resource` | 读取资源内容 |
| `auth` | 启动 OAuth 授权流程 |

`add` 的参数：

| 参数 | 说明 |
| --- | --- |
| `-t, --type <TYPE>` | 服务器类型：`studio` / `remote-http` / `remote-sse` |
| `-C, --command <CMD>` | 启动命令 |
| `--arg <ARG>` | 启动参数，可重复 |
| `-u, --url <URL>` | 服务器地址 |
| `-e, --env <KEY=VALUE>` | 环境变量，可重复 |
| `-H, --header <KEY=VALUE>` | 请求头 |
| `--cwd <PATH>` | 工作目录 |
| `--enabled-tools <LIST>` | 启用的工具列表（逗号分隔） |
| `--disabled-tools <LIST>` | 停用的工具列表（逗号分隔） |
| `--startup-timeout-ms <MS>` | 启动超时（毫秒） |
| `--tool-timeout-ms <MS>` | 单次工具调用超时（毫秒） |
| `--enable` / `--disable` | 启用 / 停用该服务器 |

## neo trust

用途：管理当前工作区的信任状态。

用法：`neo trust <SUBCOMMAND>`

| 子命令 | 说明 |
| --- | --- |
| `status` | 查看当前工作区的信任状态 |
| `approve` | 信任当前工作区 |
| `deny` | 拒绝信任当前工作区 |
| `clear` | 清除信任决定 |

## neo workflow

用途：管理工作流（Workflow）——列出、校验、试运行与运行。

用法：`neo workflow <SUBCOMMAND>`

| 子命令 | 说明 |
| --- | --- |
| `list` | 列出可用工作流及说明 |
| `check` | 校验定义而不运行；`--json` 以 JSON 输出 |
| `test` | 用录制结果安全试运行；`--case <fixture>` 指定用例，`--json` 以 JSON 输出 |
| `run` | 运行并等待结果；`--args <JSON>` 或 `--args-file <path>`（两者互斥）；`--output text\|json\|jsonl`，默认 `text` |

## neo update

用途：更新 Neo 到最新版本。

用法：`neo update [OPTIONS]`

| 参数 | 说明 |
| --- | --- |
| `--unstable` | 安装最新预发布版 |
| `--stable` | 从预发布版切回最新稳定版 |
| `--rollback` | 离线恢复相邻的 `.bak` 备份 |

## neo uninstall

用途：卸载 Neo。

用法：`neo uninstall [OPTIONS]`

| 参数 | 说明 |
| --- | --- |
| `-y, --yes` | 跳过数据删除确认 |

## neo rpc

用途：以 JSONL RPC 服务器模式运行。

用法：`neo rpc`（无参数）

## 下一步

- [快速开始](../quickstart.md) — 安装、配置与第一个对话
- [会话管理](../guides/sessions.md) — 恢复、分叉、压缩与导出
