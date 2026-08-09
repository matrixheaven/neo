# Neo WebUI 底层实施交接

把本文件、`docs/aegis/specs/2026-08-09-neo-webui-design.md` 与 `docs/aegis/plans/2026-08-09-neo-webui.md` 一起交给负责 Rust、会话调度和网页服务的执行者。本文件补全了实施时不能自行猜测的并发、鉴权、持久化和失败边界；三个文件冲突时，以本文件的技术时序和边界为准，产品范围仍以设计说明为准。

## 1. 授权、范围与工作方式

本文件冻结底层范围，但不单独授权实现。只有协调者明确提供已批准的实施计划并要求开始后，才可写入代码。

工作目录：

```text
/Users/chenyuanhao/Workspace/neo
```

开始前按顺序查看：

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-08-09-neo-webui-design.md`
5. `docs/aegis/plans/2026-08-09-neo-webui.md`
6. 本文件

然后执行：

```bash
rtk icm recall-context "neo-webui loopback token session host TurnChannels event replay" --limit 5
rtk git status --short --branch
rtk cx definition --name TurnChannels --from crates/neo-agent/src/modes/interactive/mod.rs
rtk cx definition --name run_prompt_streaming --from crates/neo-agent/src/modes/run/mod.rs
rtk cx definition --name run_prompt_in_session_streaming --from crates/neo-agent/src/modes/run/mod.rs
rtk cx definition --name append_streaming_event --from crates/neo-agent/src/modes/run/mod.rs
```

以执行时工作树为准，保留全部既有改动。不得自行创建分支、工作树、提交、推送、恢复、暂存或清理文件。

允许修改：

```text
Cargo.toml
Cargo.lock
crates/neo-webui/**
crates/neo-agent/Cargo.toml
crates/neo-agent/src/cli.rs
crates/neo-agent/src/main.rs
crates/neo-agent/src/modes/mod.rs
crates/neo-agent/src/modes/webui/**
crates/neo-agent-core/src/session/**
crates/neo-agent/tests/webui_behavior/**
crates/neo-webui/tests/**
```

实际命令行分发文件若与上述入口不同，先用 `cx` 找到唯一调用路径；只做最小扩展。

禁止修改：

- `crates/neo-tui/**`。
- 旧 `crates/neo-agent/src/rpc/**` 的行为，或任何网页到旧 RPC 的包装、转发、别名与兼容分支。
- `AgentMessage`、提供方请求、系统提示、缓存前缀、历史顺序和追加式会话记录语义。
- Delegate、DelegateGroup、DelegateSwarm 的卡片、执行语义或工具语义。
- Bash、Terminal 的准入等待、无限等待、取消和输出捕获语义。
- `crates/neo-webui/web/**`。前端未交付时不得创建占位网页、占位资源、模拟界面或修改前端构建文件。

## 2. 已证实的事实，不要重新发明运行路径

1. 旧 `neo rpc` 按行读取标准输入并等待一个请求结束后才读取下一条；`prompt` 只收文本且总是启动新会话，不能作为网页后端。
2. `run_prompt_streaming` 用于新会话首条消息；`run_prompt_in_session_streaming` 用于已有会话。它们必须是网页启动回合的唯一入口。
3. `TurnChannels` 只持有事件、审批、会话编号、提问的发送端，以及取消令牌与 `SteerInputHandle`。现有交互层的 `RunningTurn` 才持有四个接收端。
4. 网页宿主必须自行按现有交互层的形状创建四组通道，并由后台任务持续排空 `events`、`approvals`、`session_ids`、`questions` 接收端。不得把网页连接或一个被丢弃的 `AgentEventStream` 作为回合的动态持有者。
5. `append_streaming_event` 先沿现有路径写入 JSONL，再把可转发的 `AgentEvent` 发到通道。因此网页只投影收到的事件，绝不第二次写 JSONL、重排事件或伪造撤回事件。
6. 同一会话在取得写锁前会恢复上下文，且工作流事件路由按会话目录持有动态路径。同会话绝不能并行启动两个回合；不同会话可以并行。
7. `TurnRequest` 中的动态权限、工作区策略、计划模式、手动压缩、指令注册表和主题草稿都是可变状态。每个 `WebSessionState` 必须新建自己的状态容器，不能共享或克隆已有控制器、`AppConfig` 中的 `Arc` 状态。
8. `PendingApproval` 由 `request.id + oneshot response_tx` 构成；`PendingQuestion` 由 `id + oneshot response_tx` 构成。它们不是可重试的普通请求。
9. `ToolOutputRef` 不含会话标识，不能仅凭引用字段授权读取。必须由该会话的规范历史和实时事件建立归属映射。

## 3. 固定架构和唯一持有位置

```text
neo-agent
  -> neo-webui
       -> neo-agent-core
```

### `neo-webui`

- 只持有 Axum 路由、静态资源、Cookie 鉴权、网页长连接、请求体限制、序列化协议和有界中继。
- 定义 `WebUiHost`、`WebUiCommand`、`WebUiReply`、`WebUiSnapshot`、`WebUiEventEnvelope` 与网页错误形状。
- `WebUiHost` 只做两件事：执行强类型网页命令；为一个会话建立“快照加续接”订阅。不得发展成通用服务定位器。
- 不读取 JSONL、不创建 `AgentRuntime`、不执行工具、不解析 `AppConfig`、不接收文件系统路径。

### `neo-agent`

- `modes/webui/` 中的 `WebSessionHost` 是网页回合、审批、提问、取消、输入、会话状态和事件中继的唯一动态持有者。
- 它复用 `TurnRequest`、`TurnChannels`、`SteerInputHandle`、`run_prompt_*_streaming` 和现有会话元数据位置；不得复制、包裹或重写 `AgentRuntime`。
- 每个已知会话有一个 `WebSessionState`。它包含当前回合、当前 `turn_id`、通道接收端、取消令牌、`SteerInputHandle`、待审批、待提问、每会话可变状态、最后的 `TodoUpdated`、输出引用归属和到 `neo-webui` 的发布器。
- 没有活动回合、待审批、待提问和订阅者的会话可释放纯内存的执行与中继状态；下一次读取必须从既有元数据和规范历史重建，不能创建网页数据库。

### `neo-agent-core`

- `AgentEvent`、JSONL、`ToolOutputRef`、任务事件、会话标识验证与会话元数据继续是唯一来源。
- 在现有会话元数据中新增 `pinned`、`archived`；旧数据使用 `#[serde(default)]` 得到 `false`。
- 不向核心层传入 Cookie、URL、网页请求、Axum 类型或网页路径。

## 4. 回合启动与通道排空：严格顺序

### 4.1 所有回合的创建形状

每次回合都必须创建独立的：

```text
event_tx / event_rx
approval_tx / approval_rx
session_id_tx / session_id_rx
question_tx / question_rx
CancellationToken
SteerInputHandle
```

将发送端、取消令牌和输入句柄放入 `TurnChannels`，把接收端和 `JoinHandle` 放入当前 `WebSessionState` 的活动回合。一个后台排空循环负责所有接收端；不得由 HTTP 请求、网页长连接或前端可见性控制它的生命周期。

排空循环每次唤醒都先处理至多 `256` 个事件，再检查审批、提问、会话编号和任务完成状态，防止大量文本增量饿死审批或提问。任何通道关闭只表示该通道不再有新值，不能立即丢弃其他仍可能有值的通道。

### 4.2 首条消息创建新会话

`POST /api/sessions` 的唯一流程如下：

1. 验证输入后，建立一个不可被网页按会话标识访问的临时启动记录，立即生成唯一 `turn_id`，再启动后台任务。
2. 后台任务调用 `run_prompt_streaming`，同时持续排空四个接收端。它只等待 `session_id_rx` 送来第一个合法会话标识，不等待模型、工具或整个回合完成。
3. 收到合法标识后，原子地把临时记录移入该 `session_id` 的 `WebSessionState`，发布 `phase: starting`，并让 HTTP 返回 `201`：`session_id`、`turn_id`、当前状态和初始续接信息。
4. 在取得会话标识前，若任务完成、通道关闭或初始化失败，先排空已经入队的会话标识；仍没有合法标识时才返回不含会话标识的通用失败。不得等待完整模型回合来确认失败。
5. 在取得会话标识后，任何初始化、模型、工具或持久化错误都转为该会话的 `phase: failed` 与状态事件；不得撤销已返回的会话标识，也不得删除已有追加式记录。
6. 收到空、非法、第二个或与已有会话不一致的标识时，不得替换当前键。将当前回合标记失败并取消，保留已存在的规范记录。
7. 客户端在 HTTP 响应丢失后不得自动重发创建请求。服务端不承诺未持久化的请求去重；客户端必须先刷新会话列表或快照，再由用户决定是否重发，避免生成两个新会话。

### 4.3 已有会话的新回合

`POST /api/sessions/<session_id>/turns` 只在同一把会话状态锁内确认该会话没有活动回合、没有结束清理、没有待响应的一次性通道后才可进入 `starting`。锁内先登记 `turn_id` 和活动记录，再在锁外启动 `run_prompt_in_session_streaming`。返回 `202` 表示任务已登记，不表示模型已经完成。

任何并发的第二次启动必须得到 `409 session_busy`，而不是在写锁、恢复上下文或工作流路由处碰运气。`failed` 与 `cancelled` 在完整清理后才可再次启动；`cancelling`、`finishing` 期间一律返回 `409 turn_transition`。

### 4.4 逐会话可变状态

新建 `WebSessionState` 时，分别新建下列状态容器：

- `live_permission_mode`
- `workspace_policy`
- `plan_mode`
- `manual_compact_request`
- `theme_draft_store`
- 当前会话的指令注册表和逐回合配置快照

模型、推理、权限和模式选择只构成该会话下一回合的 `TurnRequest` 覆盖；不得写回全局 `AppConfig`，不得让会话 A 的操作影响会话 B 正在运行的工具授权、工作区策略、计划模式、手动压缩或主题草稿。

## 5. 状态机、输入与竞争规则

网页状态必须是下列可组合形状，不能把“正在等人”和“回合是否结束”混为一个互斥枚举：

```text
phase: starting | running | finishing | idle | cancelled | failed
waiting_approval: bool
waiting_question: bool
current_turn_id: Option<turn_id>
```

前端展示“等待确认”或“等待回答”时来自两个布尔值；两者同时为真时必须都保留，不能覆盖其中一个。只要 `phase` 为 `starting`、`running` 或 `finishing`，就仍是活动回合。

| 情形 | 唯一行为 |
| --- | --- |
| 普通后续输入 | 仅在当前 `turn_id` 仍是活动回合时，以 `ActiveTurnInput::FollowUp` 调用 `SteerInputHandle::try_push`。成功后返回 `202`；真实 `FollowUpQueued` 仍只由运行时发出。 |
| 立即引导 | 仅在当前 `turn_id` 仍是活动回合时，以 `ActiveTurnInput::SteerNow` 调用同一输入句柄。不得把它降级为普通后续输入，也不得中断当前步骤。 |
| 输入句柄关闭竞争 | `try_push` 失败时，在同一会话锁内复核 `turn_id` 与阶段。若回合正在结束，返回 `409 turn_transition` 并保留前端草稿；不得静默丢弃、伪造已排队事件或另开并行回合。 |
| 空闲会话收到 `input` | 返回 `409 no_active_turn`；前端应改用 `turns` 启动新回合。 |
| 运行中收到第二个 `turns` | 返回 `409 session_busy`；绝不创建第二个写入器。 |
| 取消 | 先在锁内把同一 `turn_id` 标记为 `finishing`，移除待审批和待提问记录，再在锁外调用取消令牌。相同当前 `turn_id` 的重复取消返回 `202 cancelling`；旧回合标识返回 `409 stale_turn`。 |
| 审批或提问解析 | 在锁内同时验证会话标识、`turn_id`、待处理项标识和未关闭的 `response_tx`；用 `remove` 取得一次性发送端后才在锁外发送。只有一个竞争者可成功，其余一律 `409 stale_control`。 |
| 任务完成 | 先进入 `finishing`，停止接收新的回合启动与输入；继续排空四个接收端直到它们关闭并完成任务连接，再清空 `turn_id`、派发最终状态并进入 `idle`、`cancelled` 或 `failed`。不得在 `MessageEnd`、网页断开或第一个完成信号到达时提前结束。 |

取消不得调用任意时限后的强制 `abort`，不得为网页入口引入工具或终端超时。进程退出是唯一全局终止边界；进程停止时已有 JSONL 保持追加式，后续恢复仍走现有恢复路径。

### 审批、提问与关闭的细节

1. 收到 `PendingApproval` 或 `PendingQuestion` 时，先检查 `response_tx.is_closed()`；已关闭则不向网页暴露，也不保留失效记录。
2. 未关闭的待处理项必须在同一临界区内存入“当前会话加当前回合”的映射，再发布状态与事件。网页断开绝不能清掉该映射；重连快照必须带回它。
3. 相同待处理项标识第二次到达时不得覆盖第一个一次性发送端。将其视为运行时异常，标记当前回合失败并取消；不得让第一个等待者永久悬挂。
4. 解析请求与取消、任务结束竞争时，先从映射移除者获胜。发送失败或接收端已经关闭时返回 `409 stale_control`，不重试一次性发送，也不伪造“已处理”。
5. 取消和最终清理只丢弃仍未解析的发送端，让既有运行时按其取消路径结束；不得把取消伪造成批准、拒绝或问题答案。

## 6. 事件、快照、重试与输出归属

### 6.1 事件语义和序列

`AgentEvent` 是唯一转录语义来源。网页只附加：

```json
{
  "type": "session_event",
  "stream_id": "本次服务启动标识",
  "session_id": "...",
  "sequence": 43,
  "event": {}
}
```

`session_state` 与 `session_metadata_changed` 是传输状态，不是伪造的 `AgentEvent`，不得写入 JSONL 或进入模型上下文；它们也使用同一会话、同一 `stream_id` 的递增 `sequence`。前端以 `stream_id + sequence` 去重。

实时事件到达时，先更新输出引用归属和会话内存投影，再发入有界中继。不得把网页事件重新写进 JSONL。`RetryScheduled`、`RetryResumed`、`RetryExhausted` 的临时输出处理必须复用现有转录语义：失败尝试不能出现在重连后的最终投影中，也不能以新 JSONL 事件“撤回”。

### 6.2 快照与续接的无丢失边界

一个网页长连接只能同时观察一个会话。新 `watch_session` 会原子地注销旧观察者、建立新会话边界；它从不取消后台回合。

对每次订阅执行下列不可拆分的顺序：

1. 验证会话标识，取得该会话状态锁和中继锁。
2. 建立观察者，记录一个 `snapshot_sequence` 水位；快照必须代表不晚于该水位的规范历史加当前有效投影。
3. 在不持有锁等待网络的前提下发送快照及其水位。
4. 从 `snapshot_sequence + 1` 开始补发连续事件。重复序号由前端去重，但服务端不得制造缺口或重新排序。

续接规则固定如下：

- `stream_id` 不同、`after.sequence` 大于当前序号、序号不连续、请求游标落在已淘汰区间，均发送完整快照。
- 缓存中存在从 `after.sequence + 1` 开始的连续尾部时，只发送缺失事件。
- 快照、缓存和实时流都不得静默省略规范历史。完整工具与终端输出例外：它们始终只给不透明引用，正文按范围读取。
- 服务重启会使令牌、Cookie、`stream_id` 和所有纯内存中继失效。旧网页得到未授权或新流标识后必须回到终端取得新的完整地址；不得自动沿用旧凭据或伪造续接。

### 6.3 有界内存与慢连接

固定常量必须集中定义并有测试，不得散落魔法数字：

```text
单会话事件缓存：256 KiB
服务全部事件缓存：4 MiB
单个网页命令正文：256 KiB
单个网页长连接入站消息：64 KiB
单连接待发送数据：512 KiB 且最多 256 条消息
工具输出单次读取：最多 1,000 行
会话列表单页：最多 100 项
```

规则：

1. 单个事件过大或服务总缓存需要淘汰时，仍向已连接观察者发送该事件，但把无法连续续接的游标标记为“必须快照”。不得为了缓存它复制完整工具或终端输出。
2. 全服务缓存达到上限时，按最旧事件淘汰，且每个会话保留连续尾部；任何被淘汰区间只能用快照恢复。
3. 向连接队列投递必须是非等待式。达到消息或字节上限即注销观察者并以 `1013` 关闭长连接；不得在持锁状态等待网络，不得扩大队列。
4. 空闲且没有活动回合、待处理项和观察者的会话释放内存中继、临时投影和输出归属索引；下一次读取从规范历史重建。
5. 不设置模型、工具、子代理、Bash 或 Terminal 的网页专用超时。网页长连接只允许对“首次订阅帧”设置 `5` 秒接入期限，超时直接关闭空连接，不影响任何回合。

### 6.4 `ToolOutputRef` 归属

范围读取只接受强类型、不透明的 `ToolOutputRef` 与数值行范围；绝不接受路径、URL、文件名、`cwd` 或自由字符串。

处理顺序固定为：

1. 验证路径中的 `session_id`。
2. 从该会话的规范历史和当前有效实时事件建立或重建输出引用集合；服务重启后必须能够从历史重建，不能仅依赖内存。
3. 精确确认请求的引用属于该集合；失败即 `404 output_not_in_session`，不泄露其他会话是否存在。
4. 验证 `start_line` 不溢出，`max_lines` 在 `1..=1000`；再调用既有 `ToolOutputStore::read_range`。

不得根据 `agent_id`、`task_id` 或引用里看似相关的字段放行，不能跨会话扫描输出存储，也不能把输出文件路径返回给网页。

## 7. 监听、令牌与网页防护

### 7.1 启动顺序

1. 只绑定 `127.0.0.1:0`；不得支持 `localhost`、`0.0.0.0`、IPv6、局域网地址、反向代理、隧道或自定义主机。
2. 成功取得实际端口后，生成仅驻留内存的 32 字节随机一次性令牌和本次 `stream_id`。
3. 完成路由与服务任务安装后，构造完整地址 `http://127.0.0.1:<port>/#access=<token>`。
4. 当且仅当标准输出是交互终端时，默认和 `--no-open` 都向标准输出打印完整地址。`--no-open` 只禁止自动打开浏览器。
5. 默认在打印后调用已有 `webbrowser::open`。打开失败只报告不含地址、令牌或库原始错误的通用提示，服务继续运行。
6. 标准输出被重定向时，标准输出、标准错误和普通日志都不得输出完整地址、令牌、Cookie 或认证正文；默认打开浏览器的行为不因重定向改成泄露凭据。

### 7.2 令牌兑换与 Cookie

- 地址片段不会发往服务端。前端仅用内存中的令牌请求 `POST /api/auth/claim`，成功或失败后都立即清除片段。
- 兑换请求只接受 `application/json`，正确 `Host` 和正确 `Origin`。令牌必须按 `URL_SAFE_NO_PAD` 解码为恰好 `32` 字节；以固定时间字节比较，并在同一把锁内完成“比较加已消费”转换，两个并发兑换中恰好一个可以成功。
- 无效、已消费、长度不符的令牌一律返回相同的通用 `401`，不设置 Cookie，不回显令牌。不得从错误差异暴露令牌状态。
- 成功时消费令牌，再签发独立随机服务内会话凭据，使用固定 Cookie 名 `neo_webui_session` 并设置 `HttpOnly; SameSite=Strict; Path=/`，不设置虚假的 `Secure`。不设置持久化 `Max-Age`。
- 服务端只在内存中保存当前 Cookie 凭据；服务停止、重启或主动清除时立刻失效。带旧 Cookie 的请求返回通用 `401` 并清除该 Cookie，不泄露重启细节。
- 令牌、Cookie、认证请求体、网页帧和 `Set-Cookie` 不得写进日志、错误、JSONL、工具输出、浏览器本地存储、会话存储、控制台或遥测。

### 7.3 主机、来源、静态资源和错误

- 每个请求都精确要求唯一 `Host: 127.0.0.1:<实际端口>`。不得接受大小写变体、尾随域名、`localhost`、IPv6、多个 Host 值或转发头；不得信任 `Forwarded`、`X-Forwarded-Host`、`X-Forwarded-Proto`。
- 每个写请求、令牌兑换和网页长连接都额外精确要求唯一 `Origin: http://127.0.0.1:<实际端口>`；缺失、`null`、多个值、斜杠后缀或其他来源一律拒绝。读取请求不要求 `Origin`，但仍要求正确 `Host` 与有效 Cookie。
- 不设置跨域响应头。所有页面与接口设置 `Cache-Control: no-store`、`Referrer-Policy: no-referrer`、`X-Content-Type-Options: nosniff`。
- 内容安全策略固定为 `default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'`；禁止外部资源、内联脚本、对象、嵌入框架和基础地址改写。
- 静态资源必须是编译期嵌入的固定允许列表。不得用当前工作目录、`ServeDir`、用户路径或回退到任意 `index.html` 提供资源。
- API 错误只返回稳定状态码和下列短错误代码：`invalid_request`、`unauthorized`、`not_found`、`session_busy`、`turn_transition`、`no_active_turn`、`stale_turn`、`stale_control`、`too_large`、`output_not_in_session`、`internal`；不得回显文件路径、会话目录、令牌、Cookie、提示词或底层错误文本。

## 8. 固定接口细节

路径、字段名与设计说明第 7 节一致，全部使用 `snake_case`，不增加历史路径、别名或网页到 RPC 的桥接。

| 请求 | 成功语义 | 明确拒绝 |
| --- | --- | --- |
| `POST /api/sessions` | 收到首个合法会话标识后返回 `201`，不等待回合结束。 | 空白消息、过大正文、预标识失败。 |
| `POST /turns` | 原子登记新 `turn_id` 后返回 `202`。 | 活动、结束清理、待处理回合。 |
| `POST /input` | 当前回合接受 `follow_up` 或 `steer` 后返回 `202`。 | 空闲、旧回合、输入句柄关闭竞争。 |
| `POST /cancel` | 当前回合首次或重复取消返回 `202 cancelling`。 | 非当前 `turn_id`。 |
| `POST /approval`、`POST /question` | 单次成功发送响应后返回 `204`。 | 会话、回合、待处理项不匹配，或发送端关闭。 |
| `PATCH /api/sessions/<session_id>` | 只改标题、`pinned`、`archived` 并发布元数据变化。 | 未知字段、任意路径、非法会话标识。 |
| `GET tool-output` | 已验证归属的限定行范围。 | 非归属引用、越界范围、路径形式输入。 |

会话列表只匹配标题。归档、置顶、重命名不影响活动回合；归档运行中的会话仍发布状态。网页没有删除会话能力。

## 9. 前端资源与固定样本的交接时序

底层和前端的文件边界不可重叠，按下列顺序交付：

1. 底层先完成 `protocol.rs`、认证、中继、内存假宿主测试和 `crates/neo-webui/fixtures/webui-events.json`。固定样本必须可被 `serde_json` 验证，包含：两个会话、快照水位与续接、不同 `stream_id` 的快照替换、普通正文、思考、排队工具、终端输出引用、审批、提问、重试撤回、任务更新、三个 Delegate 系列事件、变更统计、慢连接关闭和过期控制拒绝。
2. 在前端交付 `crates/neo-webui/web/dist` 前，底层只完成 API 与长连接能力。不得创建 `web/**`，不得伪造根页面，也不得为了让编译通过做临时网页回退。
3. 前端交付固定文件名的 `dist` 后，底层才添加 `assets.rs` 和生产静态路由，以编译期字节嵌入固定资源。最终 `neo webui` 只运行带真实嵌入资源的二进制。
4. 固定样本一旦交给前端即由前端只读使用。字段、顺序或边界不足时由底层增补样本和接口；前端不得猜测、兼容或自己改样本。

## 10. 必须新增的精确测试

使用假模型、同步屏障、随机端口和隔离临时目录。不要使用固定端口、固定等待、真实提供方、共享工作目录或 Shell 平台分支。

至少有下列可独立失败的测试：

1. 首条消息在取得会话标识后返回而不等待模型完成；标识前失败不创建网页可访问状态；标识后失败成为会话失败状态。
2. 两个不同会话并行；同会话第二个回合被拒绝；切换、归档、取消订阅和网页长连接断开都不取消后台回合。
3. 事件、审批、提问、会话标识通道都被持续排空；高频事件不饿死审批或提问；任务完成后晚到接收端值不会被丢弃。
4. 普通后续输入、立即引导、输入句柄关闭竞争、取消与回合结束的胜负顺序符合第 5 节，且不会并行启动回合或静默丢输入。
5. 审批、提问、取消的会话标识、回合标识、待处理项三重匹配；两个并发响应只有一个成功；关闭的一次性通道与重复标识安全失败。
6. 一次性令牌并发兑换只有一个成功；错误或已消费令牌、错误 Host、错误或缺失 Origin、无 Cookie 长连接、过大请求和敏感日志均被拒绝或脱敏。
7. 网页断线、慢客户端、缓存淘汰、不同 `stream_id`、非法游标和快照切换都不丢失或重复最终投影；重试后的快照不带失败尝试文本。
8. 置顶归档跨 `SessionMetadataStore` 重开保存，旧元数据默认为 `false`；工具输出范围读取拒绝跨会话引用和路径输入。
9. macOS、Linux、Windows 都只走回环监听与 `webbrowser` 库，不调用 `open`、`xdg-open`、`cmd /c start` 或 Shell。

完成每个片段后运行单包、单目标、精确测试名与对应静态检查；不要用宽泛全仓测试替代。前端资源到位前，不把静态资源未交付误报为运行时失败。

## 11. 立即停止并报告的条件

出现以下任一情况时停止对应任务并报告最小缺口；不得自行扩大架构：

- 现有逐回合配置无法让会话状态隔离。
- 无法在不新增第二份记录的前提下建立“快照加续接”切换点。
- `ToolOutputRef` 无法从该会话历史和实时事件建立可靠归属。
- 实现要求修改提供方请求、系统提示、缓存前缀、规范历史、旧 RPC 或 TUI。
- 实现要求开放非回环地址、持久化令牌、前端读取文件、增加浏览器数据库或无限中继队列。
- 前端资源尚未交付，却被要求用占位网页或修改 `web/**` 绕过资源边界。

交付报告只包含：修改文件、每项验收的精确命令和结果、固定事件样本位置、未解决阻断和剩余风险。不要自行提交。
