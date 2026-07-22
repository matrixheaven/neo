# 配置文件

Neo 使用**单一配置文件** `~/.neo/config.toml`（TOML 格式）管理所有全局设置、provider、model、运行时参数和 MCP 服务器。所有 workspace 共享同一份配置——Neo 不再读取项目级配置文件。

## 配置文件位置

| 位置 | 说明 |
| --- | --- |
| `$NEO_HOME/config.toml` | 当设置了 `NEO_HOME` 环境变量时优先使用 |
| `~/.neo/config.toml` | 默认路径（推荐） |
| `--config <path>` | CLI 参数，临时覆盖路径（见 `neo --help`） |

> 没有 `.neo/config.toml` 也能启动——所有字段都有默认值。首次运行 `neo` 时，按需创建即可。

## 顶层字段总览

`config.toml` 的顶层字段来自 `FileConfig`：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `default_model` | string | `"gpt-4.1"` | 默认模型别名；可填 `[models.<alias>]` 的别名，或直接的 `<provider>/<model>` |
| `default_provider` | string | `"openai"` | 默认 provider id，当 `default_model` 不含 `/` 时用于拼接显示标签 |
| `api_key_env` | string | — | 全局 API key 环境变量名（provider 自身的 `api_key_env` 会覆盖此值） |
| `permission_mode` | `"ask"` \| `"auto"` \| `"yolo"` | `"ask"` | 默认权限模式，详见 [权限模式](permissions.md) |
| `sessions_dir` | path | `~/.neo/sessions` | 会话存储根目录，支持 `~` 展开 |
| `model_scope` | string[] | `[]`（即全部） | 限制可用的 model glob 列表，例如 `["openai/gpt-*", "claude-sonnet-4:high"]` |
| `skill_path` | string \| string[] | `[]` | 额外的技能目录；可写成单字符串或字符串数组 |
| `extra_skill_dirs` | string[] | `[]` | 额外技能目录（与 `skill_path` 等价，列表写法） |
| `prompt_templates` | string[] | `[]` | 自定义 prompt 模板目录列表 |
| `system_prompt_file` | path | 存在时为 `~/.neo/SYSTEM.md` | 自定义系统 prompt 文件。等效于 `~/.neo/SYSTEM.md`：会替换 Neo 内置系统 prompt，支持 `~` 展开 |
| `providers` | table | — | `[providers.<id>]` 表，详见 [Provider 配置](providers.md) |
| `models` | table | — | `[models.<alias>]` 表 |
| `runtime` | table | — | `[runtime]` 推理参数 |
| `tui` | table | — | `[tui]` 终端 UI 设置 |
| `mcp` | table | — | MCP 服务器配置 |

```toml
# config.toml 顶层示例
default_model = "openai/gpt-4.1"
default_provider = "openai"
permission_mode = "ask"
sessions_dir = "~/.neo/sessions"
system_prompt_file = "~/.neo/SYSTEM.md"
```

## 系统 Prompt 文件

Neo 按以下顺序构造模型系统消息：

1. 基础系统 prompt：优先使用 `system_prompt_file`，未配置时使用存在的 `~/.neo/SYSTEM.md`，两者都没有时使用 Neo 内置 prompt。
2. 存在时追加 `~/.neo/APPEND_SYSTEM.md`。
3. 可用 skill 元数据。

`SYSTEM.md` 和 `system_prompt_file` 会替换内置基础 prompt。`APPEND_SYSTEM.md` 是只追加的入口，适合保留 Neo 内置 prompt 并在其后补充用户指令。

项目指令（`AGENTS.md`）不再是系统消息的一部分。Neo 把受信任门控、按路径发现作用域的指令链加载为持久化的会话级指令 epoch，存入会话事件流，因此它们绝不改写 `system_prompt` 或此前的请求字节。`CLAUDE.md` 不再是回退文件名。详见 [AGENTS.md](../customization/agents.md#agentsmd)。

## `[providers.<id>]` 表

每个 provider 用一个 `[providers.<id>]` 子表声明。`<id>` 由你命名，会被 `default_provider` 和每个 model 的 `provider` 字段引用。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `type` | `openai` \| `openai_response` \| `anthropic` \| `google` | `openai` | Provider 协议类型，决定走哪条 wire 客户端 |
| `base_url` | string | — | API 基址，如 `https://api.openai.com/v1` |
| `api_key` | string | — | 内联 API key（明文存于配置文件中） |
| `api_key_env` | string | — | 承载 API key 的环境变量名，如 `OPENAI_API_KEY` |

> `api_key_env` 与 `api_key` 可同时存在；运行时优先读取环境变量，取不到才回落到内联值。具体策略见 [Provider 配置](providers.md#环境变量优先级)。

## `[models.<alias>]` 表

每个 model 用 `[models."<alias>"]` 声明。别名通常约定为 `<provider>/<model-name>`，但并不强制。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `provider` | string | **必填** | 引用的 provider id（必须已存在） |
| `model` | string | **必填** | 实际发给 API 的模型 id，如 `gpt-4.1`、`claude-sonnet-4-5-20250514` |
| `max_context_tokens` | u32 | — | 上下文窗口大小（token 数） |
| `max_output_tokens` | u32 | — | 单次最大输出 token；未设时使用模型自带值 |
| `capabilities` | string[] | `[]` | 能力标签：`streaming` / `tools` / `images` / `reasoning` |
| `display_name` | string | — | 在 picker 中展示的友好名称 |

```toml
[models."openai/gpt-4.1"]
provider = "openai"
model = "gpt-4.1"
max_context_tokens = 1047576
capabilities = ["streaming", "tools", "images", "reasoning"]
display_name = "GPT-4.1"
```

`capabilities` 标签与协议无关，仅用于 UI 提示和能力路由；缺省时 Neo 按模型默认能力推断。

## `[runtime]` 表

控制推理请求参数：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `temperature` | f64 | — | 采样温度，必须为有限且非负的数 |
| `max_tokens` | u32 | — | 最大输出 token，必须 > 0 |
| `reasoning` | table | `mode = "off"` | 结构化 reasoning 控制（仅对支持 reasoning 的模型生效） |
| `replay_reasoning` | bool | `true` | 回放历史时是否包含 reasoning 片段 |
| `steering_queue_mode` | `all`\|`one_at_a_time` | `all` | Steering 消息队列模式 |
| `follow_up_queue_mode` | `all`\|`one_at_a_time` | `all` | Follow-up 消息队列模式 |
| `tool_execution_mode` | `sequential`\|`parallel` | `parallel` | 同一轮内多个 tool call 的执行方式 |

```toml
[runtime]
temperature = 0.2
max_tokens = 4096

[runtime.reasoning]
mode = "effort"
effort = "high"
```

### `[runtime.reasoning]` 子表

`mode = "off"` 关闭 reasoning（默认），`mode = "on"` 启用 provider/model 默认 reasoning，`mode = "effort"` 在支持时发送 provider 定义的显式 `effort`，`mode = "budget_tokens"` 在支持时发送显式 `budget_tokens` 数量。常见 effort 包括 `minimal`、`low`、`medium`、`high`、`xhigh` 和 `max`；provider 也可以声明其他非空且区分大小写的值。

### `[runtime.retry]` 子表

Neo 会在 runtime 层重试可重试的 `Transport`、`RateLimit` 和 `Server` 故障；永久性的 `QuotaExhausted` 是 terminal：

```toml
[runtime.retry]
max_retries = 5
first_event_timeout_secs = 60
stream_idle_timeout_secs = 120
```

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `max_retries` | u32 | `5` | 首次请求之后允许的重试请求次数 |
| `first_event_timeout_secs` | u64 | `60` | 等待首个规范化 stream event 的 deadline |
| `stream_idle_timeout_secs` | u64 | `120` | 后续规范化 stream event 之间允许的最长静默时间 |

三个 `0` 的语义彼此独立：`max_retries = 0` 只禁用重试，`first_event_timeout_secs = 0` 只禁用首事件 deadline，`stream_idle_timeout_secs = 0` 只禁用 idle deadline。Neo 始终会发出首次请求；`max_retries` 只计算额外请求，因此 `max_retries = 100` 最多允许 101 次总请求。

首事件 deadline 持续到 Neo 收到第一个规范化 stream event。之后 idle deadline 衡量后续规范化 event 之间的静默时间。provider 的 keepalive 注释不会重置任一 deadline。deadline 到期会被归类为可重试的 `Transport` failure。

普通重试会重新发送同一个冻结请求，因此 prompt 与 cache identity 保持稳定。失败尝试产生的 delta 不会持久化到 canonical context，也不会进入 replay。有效的 `Retry-After` 会覆盖本地 backoff，并以 24 小时为上限。永久性的 `QuotaExhausted` 是 terminal：Neo 不会重试，也不会显示重连 Card。

按 `Esc` 可取消正在进行的 stream 或 retry wait。内联 Card 会在 waiting 或 connecting 时动画显示；replay 只恢复 exhausted state，绝不恢复进行中的动画。

### `[runtime.shell]` 子表

`Bash` 与 `Terminal` 共享的 shell 准入调度与单命令资源上限：

```toml
[runtime.shell]
max_active_commands = 8
max_command_parallelism = 4
max_command_descendant_processes = 32
max_command_memory_percent = 25
max_output_bytes = 65536
max_background_log_bytes = 10485760
```

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `max_active_commands` | usize | `8` | 共享调度器上同时运行的 shell 命令上限 |
| `max_command_parallelism` | usize | `4` | 单命令建议并行度预算（例如环境未设置时的 `CARGO_BUILD_JOBS`） |
| `max_command_descendant_processes` | usize | `32` | 每个命令进程树允许的最大观测后代进程数 |
| `max_command_memory_percent` | u8 | `25` | 每个命令进程树允许的最大常驻内存百分比（`1..=100`） |
| `max_output_bytes` | usize | `65536` | 工具结果中保留的最大 shell 输出字节数 |
| `max_background_log_bytes` | u64 | `10485760` | 后台命令磁盘日志上限 |

`max_active_commands` 只控制调度容量。容量满时，新的 shell 调用会透明等待，而不是返回容量错误。Agent 发起的后台 Bash 与 Terminal 共享固定的 3 个槽上限，因此默认仍有 5 个槽可供用户与前台 Agent 工作使用。三个 `max_command_*` 是直接的单命令预算，不会按容量再分摊。所有整数限制必须为正。

### `[runtime.compaction]` 子表

上下文压缩默认开启。首次写入配置时会包含此表；如果旧配置缺少该表，Neo 仍使用开启状态的默认值。需要关闭时必须显式设置 `enabled = false`。其余子字段都可选：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | 是否开启自动压缩 |
| `max_estimated_tokens` | usize | `32000` | 压缩后目标 token 上限 |
| `keep_recent_messages` | usize | `20` | 压缩时保留的最近消息数 |
| `trigger_ratio` | f64 | `0.85` | 触发压缩的上下文占比阈值 |
| `reserved_context_tokens` | usize | `50000` | 预留的尾部 token 余量 |
| `max_recent_messages` | usize | `4` | 自动压缩保留的极近消息数 |
| `micro_enabled` | bool | `false` | 是否启用 micro compaction（旧 tool-result 截断） |
| `micro_keep_recent` | usize | `20` | micro compaction 豁免的最近消息数 |
| `max_rounds` | usize | `5` | 单次压缩最大轮数 |
| `max_retry_attempts` | u32 | `5` | 空/截断摘要的最大重试次数 |

## `[tui]` 表

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `image_protocol` | `auto`\|`kitty`\|`iterm2`\|`sixel`\|`none` | `auto` | 图片渲染协议偏好 |
| `keybindings` | map<string, string[]> | `{}` | 自定义键位绑定（action → 按键列表） |
| `completion_notification` | `none`\|`bell`\|`system`\|`all` | `bell` | 任务完成通知方式 |
| `question_notification` | `none`\|`bell`\|`system`\|`all` | `none` | `AskUserQuestion` 触发通知方式 |

## `[defaults]` 表

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `mode` | string | `"interactive"` | 默认启动模式（`interactive` / `run` 等） |

## 关于项目级配置

Neo **不再支持**项目级的 `.neo/config.toml` 或 `local.toml`。所有 provider、model、设置、技能、prompt、主题都统一放在 `~/.neo/` 下，跨 workspace 共享。如果你希望按项目区分模型或权限模式，可以：

- 在 shell 启动脚本里 `export NEO_HOME=/path/to/project-neo`，让每个项目指向不同的 neo home；
- 或用 `neo --config /path/to/custom.toml` 显式指定配置文件。

## 完整示例

仓库 `examples/config/` 目录提供了可直接复制的模板：

- [`examples/config/providers-models.toml`](../../../examples/config/providers-models.toml) — 覆盖 OpenAI、Anthropic、Google、OpenRouter、Ollama 全部 provider/model 写法
- [`examples/config/mcp-server.toml`](../../../examples/config/mcp-server.toml) — MCP 服务器配置参考

```toml
# ~/.neo/config.toml —— 最小可用配置
default_model = "openai/gpt-4.1"

[providers.openai]
type = "openai_response"
api_key_env = "OPENAI_API_KEY"

[models."openai/gpt-4.1"]
provider = "openai"
model = "gpt-4.1"
max_context_tokens = 1047576
capabilities = ["streaming", "tools", "images", "reasoning"]
```

## 下一步

- [Provider 配置](providers.md) — 四种 provider 类型与自定义端点的完整写法
- [权限模式](permissions.md) — Ask / Auto / Yolo 模式与审批粒度
- [数据存储位置](data-locations.md) — `~/.neo/` 目录结构与清理指南
