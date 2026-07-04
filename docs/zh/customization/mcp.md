# MCP 服务器

MCP（Model Context Protocol）是一种让 LLM 通过标准协议调用外部工具、资源的机制。Neo 内置 MCP 客户端，可以把任意 MCP server 暴露的工具接入模型工具表，统一调度、统一鉴权、统一限流。

配置入口是 `~/.neo/config.toml` 下的 `[[mcp.servers]]` 数组（`$NEO_HOME` 优先）。Neo 在会话启动时拉起所有 `enabled = true` 的服务器，发现其工具并注册到工具表。参考示例：[`examples/config/mcp-server.toml`](../../../examples/config/mcp-server.toml)。

## MCP 概念

| 概念 | 说明 |
| --- | --- |
| **Server** | 一个独立的 MCP 进程或远程端点，通过 stdio / HTTP / SSE 与 Neo 通信 |
| **Tool** | Server 暴露的可调用函数，带 `name`、`description`、JSON Schema 输入 |
| **Resource** | Server 暴露的只读资源（URI + MIME 类型），可用 `neo mcp resources` 列出 |
| **Transport** | 底层传输方式：`stdio`（本地子进程）、`http`（Streamable HTTP）、`sse`（HTTP+SSE） |
| **OAuth** | 远程服务器所需的授权流程，token 持久化在 `~/.neo/mcp/` 下 |

Neo 的 MCP 客户端基于 [`rmcp`](https://crates.io/crates/rmcp) 实现（见 `crates/neo-agent-core/src/tools/mcp/`），连接、断线重连、OAuth 刷新都由 `McpConnectionManager` 统一托管。

## 配置 Server

所有 server 配置都写在 `[[mcp.servers]]` 表中，公共字段如下：

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `id` | string | 必填 | Server 标识，决定工具命名空间，不可重复 |
| `enabled` | bool | `true` | 是否启动该 server |
| `transport` | `"stdio"` / `"http"` / `"sse"` | 必填 | 传输方式 |
| `command` | string | stdio 必填 | 可执行文件名 |
| `args` | array | `[]` | stdio 子进程参数 |
| `env` | table | `{}` | stdio 子进程环境变量 |
| `cwd` | path | — | stdio 子进程工作目录 |
| `url` | string | http/sse 必填 | 远端 RPC 端点 |
| `headers` | table | `{}` | http/sse 自定义请求头 |
| `enabled_tools` | array | `[]` | 工具白名单，为空表示全部启用 |
| `disabled_tools` | array | `[]` | 工具黑名单 |
| `startup_timeout_ms` | int | `5000` | 建立连接超时 |
| `tool_timeout_ms` | int | — | 单次工具调用超时 |

### stdio（本地子进程）

```toml
[[mcp.servers]]
id = "filesystem"
enabled = true
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[mcp.servers.env]
RUST_LOG = "info"
```

stdio server 的 stderr 会被后台静默丢弃，不会污染 TUI；子进程随 Neo 退出而关闭。

### HTTP（Streamable HTTP）

```toml
[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
url = "https://mcp.example.test/rpc"

[mcp.servers.headers]
"x-neo-client" = "neo"
```

默认启动超时 5 秒，服务端需支持 Streamable HTTP 协议（`allow_stateless = true`）。

### SSE（HTTP + Server-Sent Events）

```toml
[[mcp.servers]]
id = "linear"
transport = "sse"
url = "https://mcp.linear.app/mcp"
enabled = true
```

`http` 与 `sse` 都走同一个 `OAuthStreamableHttpClient`，区别仅在传输握手。

## 工具命名 `mcp__<server>__<tool>`

为了让模型区分来自不同 server 的同名工具，Neo 把每个 MCP 工具重写为带命名空间的形式：

```
mcp__<server_id>__<remote_tool_name>
```

例如 `filesystem` server 暴露的 `read_file` 工具，在 Neo 工具表中注册为 `mcp__filesystem__read_file`。server_id 与工具名中的非法字符会被规范化。同名冲突时后注册的会被跳过并产出诊断。

## 权限

MCP 工具调用遵循与普通工具相同的权限模型（见 [权限模式](../configuration/permissions.md)）。在 Ask 模式下：

- **会话级审批**按全限定工具名缓存，例如 `mcp__filesystem__read_file`，同一会话内相同工具自动放行；
- 审批 key 带 workspace 根路径，跨 workspace 不会泄漏；
- 可通过 `enabled_tools` / `disabled_tools` 在配置层先做工具级收口。

## OAuth

远程 server 返回 `401 Unauthorized` 时，Neo 把状态置为 `needs_auth`，并暴露一个 `mcp__<server>__authenticate` 工具触发授权流程。完成登录有两条路径：

| 方式 | 命令 | 适用场景 |
| --- | --- | --- |
| TUI | `/mcp-config login <server_id>` 或 `/mcp` 打开管理面板 | 交互式 |
| CLI | `neo mcp auth <server_id>` | 脚本 / 无 TUI |

OAuth token 持久化在 `~/.neo/mcp/` 下，按 `<server_id> + <url>` 为键隔离；token 过期会自动刷新，刷新失败则回到 `needs_auth`。stdio server 不涉及 OAuth。

## 调试

| 命令 / 操作 | 作用 |
| --- | --- |
| `neo mcp list` | 列出所有配置的 server 及其发现的工具 |
| `neo mcp status` | 显示每个 server 的连接状态、工具数、最近错误 |
| `neo mcp add <name> -t <type> ...` | 添加并探测新 server（`--type` 取 `studio`/`remote-http`/`remote-sse`） |
| `neo mcp del <name>` / `enable` / `disable` | 管理 server |
| `neo mcp resources [--server <id>]` | 列出已连接 server 的资源 |
| `neo mcp read-resource <id> <uri>` | 读取单个资源内容 |
| `/mcp`（TUI） | 打开 MCP 管理面板，查看状态、触发重连、登录 |

> 配置变更后需重启 Neo 或开启新会话才能生效；`/mcp` 面板可在线刷新与重连单个 server。

断线重连由 `McpReconnectPolicy` 控制，默认启用指数退避（`initial_delay_ms = 500`，`max_delay_ms = 30_000`，最多 5 次）。

## 下一步

- [技能系统](skills.md) — 用 `mcp-config` 技能引导 MCP 配置
- [权限模式](../configuration/permissions.md) — MCP 工具的审批粒度
- [配置文件总览](../configuration/config-files.md) — `[[mcp.servers]]` 写在哪里
