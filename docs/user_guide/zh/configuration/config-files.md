# 配置文件

Neo 使用**单一配置文件** `~/.neo/config.toml`（TOML 格式）管理全部设置：服务商、模型、运行时参数和 MCP 服务器。所有工作区共享同一份配置——Neo 不读取项目级配置文件。

## 配置文件位置

| 位置 | 说明 |
| --- | --- |
| `$NEO_HOME/config.toml` | 设置了 `NEO_HOME` 环境变量时优先使用 |
| `~/.neo/config.toml` | 默认路径 |
| `--config <path>` | CLI 参数，临时覆盖路径（见 `neo --help`） |

> 没有配置文件也能启动——所有字段都有默认值。Neo 会在需要时自动创建。

## 顶层字段总览

`config.toml` 的顶层字段来自 `FileConfig`：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `default_model` | string | `"gpt-4.1"` | 默认模型别名；可填 `[models.<alias>]` 的别名，或直接的 `<provider>/<model>` |
| `default_provider` | string | `"openai"` | 默认服务商 id；当 `default_model` 不含 `/` 时用于拼接显示标签 |
| `permission_mode` | `"ask"` \| `"auto"` \| `"yolo"` | `"ask"` | 默认权限模式，详见 [权限模式](permissions.md) |
| `sessions_dir` | path | `~/.neo/sessions` | 会话存储根目录，支持 `~` 展开 |
| `model_scope` | string[] | `[]`（即全部） | 限制可用模型的 glob 列表，例如 `["openai/gpt-*", "claude-sonnet-4:high"]` |
| `skill_path` | string \| string[] | `[]` | 额外的技能目录；可写成单字符串或字符串数组 |
| `extra_skill_dirs` | string[] | `[]` | 额外技能目录（与 `skill_path` 等价，列表写法） |
| `prompt_templates` | string[] | `[]` | 自定义提示词模板目录列表 |
| `system_prompt_file` | path | 存在时为 `~/.neo/SYSTEM.md` | 自定义系统提示词文件；会替换 Neo 内置系统提示词，支持 `~` 展开 |
| `providers` | table | — | `[providers.<id>]` 表，详见 [服务商配置](providers.md) |
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

## 系统提示词文件

Neo 按以下顺序构造模型的系统消息：

1. 基础系统提示词：优先使用 `system_prompt_file`；未配置时使用存在的 `~/.neo/SYSTEM.md`；两者都没有时使用 Neo 内置提示词。
2. 存在时追加 `~/.neo/APPEND_SYSTEM.md`。
3. 追加可用技能元数据。

`SYSTEM.md` 与 `system_prompt_file` 会替换内置基础提示词。`APPEND_SYSTEM.md` 是只追加的入口：保留 Neo 内置提示词，并在其后补充你的指令。

项目指令（`AGENTS.md`）不再是系统消息的一部分。Neo 把受信任门控、按路径发现作用域的指令链加载为持久化的会话级指令版本（epoch），写入会话事件流，因此它们绝不改写 `system_prompt` 或此前的请求字节。`CLAUDE.md` 不再是回退文件名。详见 [项目指令](../customization/instructions.md)。

## `[providers.<id>]` 表

每个服务商用 `[providers.<id>]` 子表声明。`<id>` 由你命名，会被 `default_provider` 和每个模型的 `provider` 字段引用。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `type` | `openai` \| `openai_response` \| `anthropic` \| `google` | `openai` | 服务商协议类型，决定走哪条协议客户端 |
| `base_url` | string | — | API 基址，如 `https://api.openai.com/v1` |
| `api_key` | string | — | 内联 API key（明文存于配置文件中） |
| `api_key_env` | string | — | 承载 API key 的环境变量名，如 `OPENAI_API_KEY` |

> `api_key_env` 与 `api_key` 可同时存在；运行时优先读取内联值，仅在 `api_key` 未设置时回落到环境变量。具体策略见 [服务商配置](providers.md#环境变量优先级)。

## `[models.<alias>]` 表

每个模型用 `[models."<alias>"]` 声明。别名通常约定为 `<provider>/<model-name>`，但并不强制。

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `provider` | string | **必填** | 引用的服务商 id（必须已存在） |
| `model` | string | **必填** | 实际发给 API 的模型 id，如 `gpt-4.1`、`claude-sonnet-4-5-20250514` |
| `max_context_tokens` | u32 | — | 上下文窗口大小（token 数） |
| `max_output_tokens` | u32 | — | 单次最大输出 token；未设置时使用模型自带值 |
| `capabilities` | string[] | `[]` | 能力标签：`streaming` / `tools` / `images` / `reasoning` |
| `display_name` | string | — | 在选择器中展示的友好名称 |

```toml
[models."openai/gpt-4.1"]
provider = "openai"
model = "gpt-4.1"
max_context_tokens = 1047576
capabilities = ["streaming", "tools", "images", "reasoning"]
display_name = "GPT-4.1"
```

`capabilities` 标签与协议无关，只用于 UI 提示与能力路由；缺省时 Neo 按模型默认能力推断。

## `[runtime]` 表

控制推理请求参数：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `temperature` | f64 | — | 采样温度，必须为有限且非负的数 |
| `max_tokens` | u32 | — | 最大输出 token，必须 > 0 |
| `reasoning` | table | `mode = "off"` | 结构化 reasoning 控制（仅对支持 reasoning 的模型生效） |
| `replay_reasoning` | bool | `true` | 回放历史时是否包含 reasoning 片段 |
| `steering_queue_mode` | `all`\|`one_at_a_time` | `all` | 引导消息队列模式 |
| `follow_up_queue_mode` | `all`\|`one_at_a_time` | `all` | Follow-up 消息队列模式 |
| `tool_execution_mode` | `sequential`\|`parallel` | `parallel` | 同一轮内多个工具调用的执行方式 |

```toml
[runtime]
temperature = 0.2
max_tokens = 4096

[runtime.reasoning]
mode = "effort"
effort = "high"
```

### `[runtime.reasoning]` 子表

`mode = "off"` 关闭 reasoning（默认）；`mode = "on"` 启用服务商/模型的默认 reasoning；`mode = "effort"` 在支持时发送服务商定义的显式 `effort`；`mode = "budget_tokens"` 在支持时发送显式的 `budget_tokens` 数量。常见 effort 包括 `minimal`、`low`、`medium`、`high`、`xhigh` 与 `max`；服务商也可以声明其他非空且区分大小写的值。

### `[runtime.retry]` 子表

Neo 会在运行时层重试可重试的 `Transport`、`RateLimit` 与 `Server` 故障；永久性的 `QuotaExhausted` 是终态：

```toml
[runtime.retry]
max_retries = 5
first_event_timeout_secs = 60
stream_idle_timeout_secs = 120
```

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `max_retries` | u32 | `5` | 首次请求之后允许的重试请求次数 |
| `first_event_timeout_secs` | u64 | `60` | 等待首个规范化流事件的截止时间 |
| `stream_idle_timeout_secs` | u64 | `120` | 后续规范化流事件之间允许的最长静默时间 |

三个 `0` 的语义彼此独立：`max_retries = 0` 只禁用重试；`first_event_timeout_secs = 0` 只禁用首事件截止时间；`stream_idle_timeout_secs = 0` 只禁用空闲截止时间。Neo 始终会发出首次请求；`max_retries` 只计算额外请求，因此 `max_retries = 100` 最多允许 101 次总请求。

首事件截止时间持续到 Neo 收到第一个规范化流事件，之后空闲截止时间衡量后续规范化事件之间的静默时间。服务商的心跳注释不会重置任一截止时间。截止时间到期会被归类为可重试的 `Transport` 故障。

普通重试会重新发送同一个冻结请求，因此提示词与缓存身份保持稳定。失败尝试产生的增量不会持久化到规范上下文，也不会进入回放。有效的 `Retry-After` 会覆盖本地退避，并以 24 小时为上限。永久性的 `QuotaExhausted` 是终态：Neo 不会重试，也不会显示重连卡片。

按 `Esc` 可取消正在进行的流或重试等待。内联卡片会在 waiting 或 connecting 时显示动画；回放只恢复 exhausted 状态，绝不恢复进行中的动画。

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
| `max_command_parallelism` | usize | `4` | 单命令建议的并行度预算（例如环境未设置时的 `CARGO_BUILD_JOBS`） |
| `max_command_descendant_processes` | usize | `32` | 每个命令进程树允许的最大观测后代进程数 |
| `max_command_memory_percent` | u8 | `25` | 每个命令进程树允许的最大常驻内存百分比（`1..=100`） |
| `max_output_bytes` | usize | `65536` | 工具结果中保留的最大 shell 输出字节数 |
| `max_background_log_bytes` | u64 | `10485760` | 后台命令磁盘日志上限 |

`max_active_commands` 只控制调度容量。容量满时，新的 shell 调用会无需干预地等待，而不是返回容量错误。Agent 发起的后台 Bash 与 Terminal 共享固定的 3 个槽位上限，因此默认仍有 5 个槽位可供用户与前台 Agent 工作使用。三个 `max_command_*` 是直接的单命令预算，不会按容量再分摊。所有整数限制必须为正。

### `[runtime.workflow]` 子表

本地工作流平台的宿主安全上限（Lua VM、存储与实际占用）。脚本、定义与模型工具输入不能设置或提高这些值。预测性的未知键（如 `token_cap`、`max_concurrency`、`projected_usage`）会被拒绝。

```toml
[runtime.workflow]
lua_source_bytes = 1048576
manifest_bytes = 262144
lua_vm_memory_bytes = 268435456
pause_hook_interval = 10000
max_uninterrupted_instructions = 100000000
journal_record_bytes = 16777216
journal_total_bytes = 4294967296
artifact_record_bytes = 16777216
artifact_total_bytes = 4294967296
global_storage_bytes = 34359738368
pending_record_bytes = 268435456
task_output_page_bytes = 65536
max_active_vms = 8
max_active_workers = 8
max_active_executors = 32
swarm_concurrency = 4
```

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `lua_source_bytes` | u64 | `1048576`（1 MiB） | 每个定义/运行的 Lua 源码上限 |
| `manifest_bytes` | u64 | `262144`（256 KiB） | `.workflow.toml` 大小上限 |
| `lua_vm_memory_bytes` | u64 | `268435456`（256 MiB） | 每个 VM 内存上限；须适配平台 `usize` |
| `pause_hook_interval` | u64 | `10000` | pause/stop/资源检查之间的 Lua 指令数（`1..=u32::MAX`） |
| `max_uninterrupted_instructions` | u64 | `100000000` | 两次持久化宿主调用之间允许的最大 Lua 指令数 |
| `journal_record_bytes` | u64 | `16777216`（16 MiB） | 单条序列化日志记录上限 |
| `journal_total_bytes` | u64 | `4294967296`（4 GiB） | 每个运行的日志总上限 |
| `artifact_record_bytes` | u64 | `16777216`（16 MiB） | 单个产物载荷上限 |
| `artifact_total_bytes` | u64 | `4294967296`（4 GiB） | 每个运行的产物字节上限 |
| `global_storage_bytes` | u64 | `34359738368`（32 GiB） | 全局工作流存储上限（日志 + 产物 + 元数据） |
| `pending_record_bytes` | u64 | `268435456`（256 MiB） | 全局尚未持久化同步的 pending 记录字节 |
| `task_output_page_bytes` | u64 | `65536`（64 KiB） | TaskOutput 完整工具结果上限，不超过 `runtime.shell.output_bytes` |
| `max_active_vms` | usize | `8` | 同时活动的 Lua VM 数 |
| `max_active_workers` | usize | `8` | 同时活动的工作流 worker 数 |
| `max_active_executors` | usize | `32` | 同时活动的宿主执行器（子 Agent/工具槽位） |
| `swarm_concurrency` | usize | `4` | 工作流创建 swarm 时的默认并发（不是子 Agent 总数上限） |

所有字段必须为正；`pause_hook_interval` 还需符合上表范围。**没有**工作流 token 上限，也**没有**工作流墙钟超时。全局准入只看**实际占用**：许可不可用时运行保持持久化并排队。这些是机器安全控制，不是项目预算——Neo 不会预测 token、成本、时间或 Agent 数量来暂停或降级。Swarm 规模由这些物理上限与背压约束，而不是硬编码的 `MAX_SWARM_CHILDREN`。见 [工作流](../guides/workflows.md)。

### `[runtime.compaction]` 子表

上下文压缩默认开启。首次写入配置时会包含此表；旧配置缺少该表时，Neo 仍使用开启状态的默认值。需要关闭时必须显式设置 `enabled = false`。其余子字段都可选：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | 是否开启自动压缩 |
| `max_estimated_tokens` | usize | `32000` | 压缩后目标 token 上限 |
| `keep_recent_messages` | usize | `20` | 压缩时保留的最近消息数 |
| `trigger_ratio` | f64 | `0.85` | 触发压缩的上下文占比阈值 |
| `reserved_context_tokens` | usize | `50000` | 预留的尾部 token 余量 |
| `max_recent_messages` | usize | `4` | 自动压缩保留的极近消息数 |
| `micro_enabled` | bool | `false` | 是否启用微压缩（旧工具结果截断） |
| `micro_keep_recent` | usize | `20` | 微压缩豁免的最近消息数 |
| `snip_enabled` | bool | `false` | 是否把过时的大工具结果（如长 Read 输出）在模型输入中裁剪为头+尾片段。默认关闭：裁剪会改写旧结果，每个被改写结果会使前缀缓存失效一次——仅在能接受该代价时开启（如本地模型） |
| `snip_min_tokens` | usize | `1000` | 触发裁剪的最小工具结果大小 |
| `snip_keep_recent` | usize | `16` | 裁剪豁免的最近消息数 |
| `snip_trigger_ratio` | f64 | `0.6` | 裁剪进入占用带的窗口比例阈值。低于该比例时请求前缀保持纯追加（缓存稳定）；仅当会话累计增长进入该带后才执行裁剪 |
| `max_rounds` | usize | `5` | 单次压缩最大轮数 |
| `max_retry_attempts` | u32 | `5` | 空/截断摘要的最大重试次数 |

> 提示：`micro_enabled` 与 `snip_enabled` 都会改写上下文中间的历史工具结果，使前缀缓存从该消息起失效（每个被改写结果一次）。它们用暂时的命中率下降换取更小的历史体积；付费服务商建议两者都关闭。裁剪是较温和的变体（保留头尾、确定性改写、默认仅 Read），但仍然是前缀改写——仅建议本地或实验环境开启。

## `[tui]` 表

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `image_protocol` | `auto`\|`kitty`\|`iterm2`\|`none` | `auto` | 图片渲染协议偏好 |
| `keybindings` | map<string, string[]> | `{}` | 自定义键位绑定（动作 → 按键列表） |
| `completion_notification` | `none`\|`bell`\|`system`\|`all` | `bell` | 任务完成通知方式 |
| `question_notification` | `none`\|`bell`\|`system`\|`all` | `none` | `AskUserQuestion` 触发通知方式 |
| `theme` | string | — | 启动默认主题，为相对 `$NEO_HOME/themes/` 的逻辑 id（绝不是绝对路径）。未设置时保留旧版「排序取第一个」发现逻辑；设置了但缺失或无效时，使用内置默认主题并给出可见诊断 |

```toml
[tui]
theme = "my-theme.json"
```

主题目录与管理器行为见 [主题（Themes）](../customization/themes.md)。

## `[defaults]` 表

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `mode` | string | `"interactive"` | 默认启动模式（`interactive` / `run` 等） |

## 关于项目级配置

Neo **不再支持**项目级的 `.neo/config.toml` 或 `local.toml`。所有服务商、模型、设置、技能、提示词、主题都统一放在 `~/.neo/` 下，跨工作区共享。如果希望按项目区分模型或权限模式，可以：

- 在 shell 启动脚本里 `export NEO_HOME=/path/to/project-neo`，让每个项目指向不同的 neo home；
- 或用 `neo --config /path/to/custom.toml` 显式指定配置文件。

## 完整示例

仓库的 `examples/config/` 目录提供了可直接复制的模板：

- [`examples/config/providers-models.toml`](../../../../examples/config/providers-models.toml) — 覆盖 OpenAI、Anthropic、Google、OpenRouter、Ollama 全部服务商/模型写法
- [`examples/config/mcp-server.toml`](../../../../examples/config/mcp-server.toml) — MCP 服务器配置参考

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

- [服务商配置](providers.md) — 四种服务商类型与自定义端点的完整写法
- [权限模式](permissions.md) — Ask / Auto / YOLO 模式与审批粒度
- [数据存储位置](data-locations.md) — `~/.neo/` 目录结构与清理指南
