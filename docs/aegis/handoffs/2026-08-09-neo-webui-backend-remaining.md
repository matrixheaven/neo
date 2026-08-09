# Neo WebUI 后端剩余实施交接

本文件交给负责 Rust、网页协议与最终联调的执行者。它基于当前未提交源码，而不是“底层已完成”的交付报告；产品范围仍以 `docs/aegis/specs/2026-08-09-neo-webui-design.md` 为准。本文件冻结当前剩余修复及接口收口，取代初始底层交接中尚未完成的执行步骤，不改变已批准的本机回环、多会话、追加式记录和悬浮网页设计。

## 1. 当前状态与唯一目标

已经落地的部分：

- `neo-webui` 已有协议、一次性令牌兑换、Cookie、回环 HTTP、网页长连接和有界中继。
- `WebSessionHost` 已复用既有 `run_prompt_*_streaming`、`TurnChannels`、会话元数据和 `ToolOutputStore`。
- `pinned`、`archived` 已进入唯一会话元数据存储，旧记录默认 `false`。
- 固定样本已能驱动前端静态开发。

尚未完成的部分不是“只缺根页面”。当前代码存在真实接口和传输缺口：空闲会话启动新回合时丢失用户消息；一个网页连接只能看一个会话；输出引用未暴露为网页可用的不透明值；慢连接不会把 `1013` 真正送到浏览器；未知会话可得到空重放；网页长连接没有消息总长度限制；请求结构会静默接受未知字段；没有静态资源；顶栏改动和分支数据也没有结构化来源。

目标是把这些缺口修到前端可以同源接入且不会靠浏览器补偿。不要重新设计运行时，不要把网页接到旧 RPC，不要新建第二事件存储、网页数据库或第二个 `AgentRuntime`。

## 2. 执行范围与前置阅读

工作目录：

```text
/Users/chenyuanhao/Workspace/neo
```

开始前按顺序查看：

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-08-09-neo-webui-design.md`
5. `docs/aegis/handoffs/2026-08-09-neo-webui-runtime.md`
6. 本文件
7. `docs/aegis/handoffs/2026-08-09-neo-webui-frontend-remaining.md`
8. `crates/neo-webui/src/{protocol,relay,server,auth}.rs`
9. `crates/neo-agent/src/modes/webui/{host,session,mod}.rs`

然后执行：

```bash
icm recall-context "neo-webui session host websocket snapshot output reference security" --limit 5
rtk git status --short --branch
```

允许修改：

```text
Cargo.toml
Cargo.lock
crates/neo-webui/src/**
crates/neo-webui/fixtures/webui-events.json
crates/neo-webui/tests/**
crates/neo-agent/Cargo.toml
crates/neo-agent/src/modes/webui/**
crates/neo-agent/src/modes/interactive/git_status.rs
crates/neo-agent/src/modes/**
crates/neo-agent/tests/webui_behavior/**
crates/neo-agent-core/src/session/**
crates/neo-agent-core/tests/session_behavior/**
```

`crates/neo-webui/web/**` 只在前端交付者写完固定资源后允许读取；后端不得创建、修改、删除或格式化该目录。若共享的 Git 状态收集器需要移动，优先在 `neo-agent` 内抽到一个唯一的非界面模块，并让交互模式复用它；不得复制一份 Git 解析逻辑，也不得改 `neo-tui`。

禁止修改：

- `crates/neo-tui/**`、旧 `crates/neo-agent/src/rpc/**`、提供方请求、系统提示、缓存前缀、会话历史顺序和规范 JSONL 语义。
- Delegate、DelegateSwarm、Bash、Terminal、工具权限、无限等待和取消的既有运行语义。
- 任何会话记录的重写、删除、迁移或网页专用持久化。
- 监听地址、端口配置、跨域、远程访问、反向代理或令牌持久化。

以当前工作树为准，保留其他人的改动。不得自行暂存、提交、推送、创建分支、创建工作树、恢复或清理文件。

## 3. 不可改变的边界

1. 只监听 `127.0.0.1:0`。终端交互输出完整的一次性令牌地址；标准输出重定向时绝不输出令牌。`--no-open` 只禁止自动打开浏览器，不能改变地址或认证语义。
2. 地址片段令牌只兑换一次，得到内存内 `HttpOnly; SameSite=Strict; Path=/` Cookie；主机校验适用于所有请求，来源校验适用于写请求和网页长连接。令牌、Cookie、认证正文和长连接帧不得进入日志、错误或 JSONL。
3. 同一会话永远只有一个活动回合；不同会话可并行。切换、归档、网页关闭和长连接断开只移除观察者，绝不取消模型、工具、终端、Delegate 或 DelegateSwarm。
4. `AgentEvent` 和现有 JSONL 是规范事实。网页只使用派生的传输信封、显示元数据和有界缓存；不得再次写 JSONL、改写历史、伪造重试撤回或复制运行时。
5. `WebUiHost` 是 `neo-webui` 与 `neo-agent` 的唯一跨包入口。`neo-webui` 不读取 JSONL、不构造 `AgentRuntime`、不解析 `AppConfig`、不执行工具、不接受路径。
6. 只允许网页看到工作区相对路径。会话目录、配置路径、全局索引、提供方凭据和任意绝对路径不得通过元数据、输出引用或新接口发往浏览器。
7. 事件缓存为每会话 `256KiB`、全服务 `4MiB`；每连接实时队列为 `512KiB` 且有消息数上限。完整工具和终端输出始终按引用分页读取。

## 4. 已确认的根因与固定处置

| 优先级 | 根因 | 当前证据 | 必须的处置 |
| --- | --- | --- | --- |
| P0 | 空闲会话的新回合构造 `Vec::new()`，HTTP `POST /turns` 也没有 `message` | `host.rs` 的 `start_turn` 与 `protocol.rs` 的 `WebUiStartTurnBody` | 让新回合和新会话都要求非空 `message`，沿用创建首回合的 `Content::text(message)` 与显示文本路径；不能启动空提示回合。 |
| P0 | 一个连接重新订阅时会清除旧会话观察者，后台会话没有摘要推送 | `relay.rs::subscribe` 只有一个 `session_id` | 一个连接保留工作区摘要订阅加一个当前会话完整订阅；其他会话只发小型摘要，绝不传完整转录。 |
| P0 | 事件携带结构化 `ToolOutputRef`，读取路由却要求私有编码的字符串 | `session.rs::encode_output_ref` 与固定样本 | 在快照和实时事件公开不透明 `output.id`，前端只透传该值；不得让前端编码或解码引用。 |
| P1 | 队列溢出后先标记关闭，网页循环随即返回，队列中的关闭帧从未发送 | `relay.rs::try_push` 与 `server.rs::handle_events_socket` | 关闭时先排出唯一 `Close { code: 1013 }`，再注销连接；必须用真实网页长连接断言客户端收到 `1013`。 |
| P1 | 未知会话带相同服务标识和 `sequence: 0` 时被中继当作空重放 | `relay.rs::subscribe` 在宿主存在性校验前创建会话 | 先向宿主确认会话存在，或强制无缓存会话走快照；未知会话必须得到 `not_found`，已持久化但尚未装载的会话必须得到快照。 |
| P1 | 网页长连接只限制帧大小，分片消息仍可达到默认 `64MiB` | `server.rs::events_ws` | 同时设置 `max_frame_size` 与 `max_message_size` 为 `WS_FRAME_LIMIT_BYTES`，并补分片超限回归。 |
| P1 | 所有入站结构和查询会静默忽略拼错字段 | `protocol.rs` 的反序列化结构与查询 `HashMap` | 所有入站 JSON 结构、嵌套 composer、问题答案和网页订阅都拒绝未知字段；列表与工具输出查询只接受明确白名单。 |
| P1 | 原始事件携带 `cwd` 等路径 | `AgentEvent` 的 Shell、Terminal 和审批展示字段 | 显示层只传工作区相对路径或 `.`；不改变规范事件或 JSONL。输出正文保持原文，但新增元数据与路径字段不得暴露绝对路径。 |
| P2 | `RunningWebUi.stream_id` 与 `Relay::stream_id` 重复生成 | `modes/webui/mod.rs` 与 `server.rs::start` | 删除无用字段和重复生成，只以中继标识作为网页服务标识。 |
| P2 | 默认自动打开浏览器被错误地限制为交互标准输出 | `modes/webui/mod.rs::execute` | 只由 `--no-open` 控制打开；重定向时仍不打印令牌，打开失败只输出通用非敏感提示。 |

## 5. 固定协议收口

当前没有已发布网页客户端，所以直接替换当前不完整网页线形，不保留旧字段、旧消息或兼容分支。

### 5.1 回合消息

将 `WebUiStartTurnBody` 与 `WebUiCommand::StartTurn` 增加必填 `message: String`。服务端对新会话、空闲会话新回合和活跃回合输入统一拒绝空白消息。创建和新回合都必须走已有的用户消息构造路径，确保规范 `MessageAppended` 才是网页中用户气泡的唯一来源。

活动回合的 `follow_up`、`steer`、取消、审批和提问保持现有回合标识竞争规则。第二个 `turns` 仍返回 `409 session_busy`；输入句柄关闭竞争仍返回 `409 turn_transition` 并保留浏览器草稿。

### 5.2 一个连接的两层订阅

把当前单一观察者改为一个连接的一条有界发送队列、两类订阅：

```text
工作区摘要订阅：所有会话的 WebUiSessionSummary 与小型工作区状态更新
当前会话订阅：一个 session_id 的完整快照、AgentEvent、状态与元数据更新
```

网页长连接的强类型收发统一使用一个带 `type` 的服务消息枚举。至少包含：

```text
watch_workspace { after? }
watch_session { session_id, after? }
workspace_snapshot { stream_id, workspace_sequence, sessions }
session_snapshot { snapshot }
session_summary_changed { stream_id, workspace_sequence, event }
session_event { stream_id, session_id, sequence, event, output? }
session_state { stream_id, session_id, sequence, event }
session_metadata_changed { stream_id, session_id, sequence, event }
```

`watch_workspace` 的快照由宿主从会话元数据和当前动态状态构建；中继只缓存摘要更新。`watch_session` 切换时只清除该连接的完整会话观察者，不清除工作区摘要观察者。摘要缓存不足或服务标识改变时发送新的工作区快照；会话缓存不足、未知水位或服务标识改变时发送新的会话快照。

不要为每个后台会话建立完整网页连接，不要向摘要订阅发送 `AgentEvent`，不要让前端靠定时拉取全量会话列表维持状态。所有队列共享同一个连接上限；慢消费者仍只收到一次 `1013` 后被移除。

### 5.3 不透明工具输出引用

定义网页专用显示元数据，例如：

```text
WebUiOutputRef {
  id: String,
  byte_len: u64,
  line_count: u64,
  complete: bool
}
```

对携带 `ToolOutputRef` 的工具、Shell 和 Terminal 事件，传输层生成该元数据。`id` 可复用当前 URL 安全编码，但只能由服务端生成，浏览器只能逐字透传。原始结构化 `ToolOutputRef` 不得出现在网页 JSON 中；它继续只存在于核心事件与规范记录。

快照历史项和实时 `session_event` 都必须带相同的可选 `output`。宿主在事件进入中继前建立“会话拥有的引用”映射；读取接口先验证映射和会话，再调用现有 `ToolOutputStore::read_range`。会话空闲后释放派生状态时，下一次读取仍从规范历史重建归属。路径形式、伪造、跨会话、过期或不存在的引用一律返回同一个 `404 output_not_in_session`。

### 5.4 工作区改动入口

用户已确认顶栏右侧用于改动和分支，不用于模型、逐条确认或开发模式。为此提供一个小型、结构化、只读工作区改动读取面：

```text
GET /api/workspace/changes
GET /api/workspace/changes/<change_id>
```

摘要至少包含分支标签、是否有改动、每个改动的工作区相对路径、状态、增加行和删除行；详情只返回经长度限制的统一差异预览。`change_id` 必须是不透明值，服务端验证它只对应当前工作区内的相对路径。不得接受任意路径、Shell 字符串、绝对路径或浏览器提供的 Git 参数。

复用现有 `neo-agent` Git 状态收集的纯逻辑，必要时将其移到一个由交互模式和网页模式共同使用的唯一模块；不要复制解析器。命令只能用跨平台的 `std::process::Command` 参数形式，禁用外部差异程序，失败时返回“无状态”而不是错误文本或路径。改动接口按用户打开覆盖层时读取，并在已有工具完成事件后发布小型工作区状态失效通知；不要持续运行 Git 轮询。

### 5.5 静态资源

前端交付固定的 `web/dist` 后，新增 `crates/neo-webui/src/assets.rs`：

```text
/                 -> index.html, text/html; charset=utf-8
/index.html       -> index.html, text/html; charset=utf-8
/assets/neo-webui.js  -> JavaScript
/assets/neo-webui.css -> CSS
其他所有路径       -> 404
```

资源必须编译期嵌入，不能在运行时读取磁盘，不能递归枚举目录，不能提供单页路由回退，不能接受路径穿越。静态读取可以匿名，但仍经过精确主机检查和既有安全响应头；API、Cookie 和长连接保持原有认证规则。不要为资源托管加入新服务器框架或运行时依赖。

## 6. 实施批次

### 批次 B1：先修协议与安全根因

文件：

```text
crates/neo-webui/src/protocol.rs
crates/neo-webui/src/relay.rs
crates/neo-webui/src/server.rs
crates/neo-agent/src/modes/webui/{host,session}.rs
crates/neo-webui/tests/webui_behavior/{auth,relay,http_server}.rs
crates/neo-agent/tests/webui_behavior/{http,ws,session_runtime}.rs
```

完成 5.1、5.2、5.3 的协议替换，并修复真实 `1013`、未知会话、消息总长度和未知字段问题。建立摘要中继时只保存会话摘要与工作区状态，不能持有其他会话完整转录。更新固定样本和前端说明后再让前端连真实协议。

至少新增精确回归：

```text
idle_session_turn_persists_the_submitted_user_message
workspace_subscription_updates_background_session_without_full_transcript
opaque_output_reference_reads_only_its_own_session
slow_websocket_client_receives_1013_and_is_deregistered
unknown_or_unloaded_session_never_receives_an_empty_replay
fragmented_websocket_message_over_limit_is_rejected
unknown_json_or_query_fields_are_rejected
```

每个测试只覆盖新增边界，不重复既有认证、并发或元数据测试。

### 批次 B2：补真实运行时正向链路

文件：

```text
crates/neo-agent/src/modes/webui/{host,session}.rs
crates/neo-agent/tests/webui_behavior/{provider,pty,session_runtime,ws}.rs
```

用现有可编程提供方、跨平台终端夹具和真实 `WebSessionHost` 验证：两个会话并行、切换不取消、空闲会话新回合带用户消息、工具输出在释放投影后仍能读取、重试后重连没有失败尝试文本、审批和提问拒绝过期控制。不要以假宿主或直接读取中继队列替代这些产品边界。

必须覆盖：

```text
owned_tool_output_range_reads_after_idle_projection_rebuild
retried_provider_session_reconnects_without_failed_attempt_text
different_sessions_run_concurrently_without_cross_cancellation
dropping_the_web_subscription_does_not_cancel_the_background_turn
```

### 批次 B3：接入前端资源与顶栏数据

等待前端提交固定 `web/dist` 后，才修改：

```text
crates/neo-webui/src/{assets,lib,server}.rs
crates/neo-webui/tests/webui_behavior/**
crates/neo-agent/src/modes/webui/**
crates/neo-agent/src/modes/interactive/git_status.rs
```

实现 5.4 与 5.5。`assets.rs` 的允许列表必须与前端构建产物逐字一致。补充静态资源、工作区相对路径、差异大小、未知路径和不触发运行时磁盘读取的测试。

建议精确回归：

```text
embedded_assets_are_allowlisted_anonymous_and_non_fallback
workspace_change_detail_rejects_forged_or_outside_reference
workspace_status_reuses_the_shared_git_collector
```

### 批次 B4：启动收尾与集成

删除重复 `stream_id`，修正 `--no-open` 之外的浏览器打开行为。终端交互时仍打印含一次性令牌的地址；重定向时不打印；浏览器打开失败不泄露地址、令牌或底层错误。

启动 `neo webui --no-open` 做同源浏览器验收：首次地址片段兑换后被清除、根页面加载已嵌入资源、两个会话后台同时运行、切换不停止、归档不停止、摘要更新不断、重连正确、审批提问工具输出和改动覆盖层可用。

## 7. 快照与内存边界

当前已批准设计要求会话快照能恢复完整转录；因此一份恢复快照可以大于实时队列 `512KiB`，这是唯一例外，不得把它描述成实时队列仍有该上限。该例外只能在一个连接上有一份待发送快照，快照必须在发送后立即从队列释放，后续实时事件继续按 `512KiB` 计账。

不要为了修复该例外截断、总结、重排或改写规范历史。若实现发现单次完整快照会使当前会话常态内存突破 Neo 的资源目标，停止在此处并报告；需要另行设计基于规范 JSONL 的分页转录投影，不能偷偷把全量历史长期留在中继、浏览器或网页数据库中。

## 8. 精确验证

每个批次完成后运行最小相关命令。示例：

```bash
rtk cargo nextest run -p neo-webui --test webui_behavior webui_behavior::relay::slow_websocket_client_receives_1013_and_is_deregistered
rtk cargo nextest run -p neo-webui --test webui_behavior webui_behavior::auth::unknown_json_or_query_fields_are_rejected
rtk cargo nextest run -p neo-agent --test webui_behavior webui_behavior::session_runtime::idle_session_turn_persists_the_submitted_user_message
rtk cargo nextest run -p neo-agent --test webui_behavior webui_behavior::session_runtime::owned_tool_output_range_reads_after_idle_projection_rebuild
rtk cargo nextest run -p neo-agent --test webui_behavior webui_behavior::session_runtime::retried_provider_session_reconnects_without_failed_attempt_text
rtk cargo fmt --all --check
rtk cargo clippy -p neo-webui --lib -- -D clippy::all
rtk cargo clippy -p neo-webui --test webui_behavior -- -D clippy::all
rtk cargo clippy -p neo-agent --test webui_behavior -- -D clippy::all
rtk cargo build -p neo-agent
rtk git diff --check
```

本机验证不替代 Windows、Linux、macOS 原生验证。三个系统至少各运行一组 `neo-webui` 精确网页长连接测试和一组 `neo-agent` 精确会话宿主测试；不要把交叉编译结果说成原生运行证据。

## 9. 停止条件与交付

遇到以下任一情况立即停止并报告：需要改写 JSONL 或缓存前缀；需要复制 `AgentRuntime`；需要网页读磁盘、解析路径或执行 Git；需要开放非回环地址；需要为旧网页协议保留兼容；需要改变 TUI 的 Delegate 系列呈现；或需要通过超时强制结束工具和终端。

交付时只报告：修改文件、最终协议和样本差异、每个精确测试的非零结果、浏览器同源验收、跨平台证据与残余风险。不得自行提交；协调者在前端接入、全部证据新鲜且文件范围已核对后统一精确提交。
