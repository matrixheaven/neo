# Neo WebUI 底层实施交接

把本文件和 `docs/aegis/specs/2026-08-09-neo-webui-design.md` 一起交给负责 Rust、会话调度和网页服务的执行者。

## 1. 授权边界

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
5. 本文件

然后执行：

```bash
icm recall-context "neo-webui loopback token session host event replay" --limit 5
rtk git status --short --branch
rtk cx definition --name run_prompt_streaming --from crates/neo-agent/src/modes/run/mod.rs
rtk cx definition --name run_prompt_in_session_streaming --from crates/neo-agent/src/modes/run/mod.rs
```

以执行时工作树为准，保留全部既有改动。不得自行创建分支、工作树、提交、推送、恢复、暂存或清理文件。

## 2. 已证实的事实，不要重复做无边界探索

1. `crates/neo-agent/src/rpc/server.rs::execute` 逐行读取标准输入，并等待一个请求处理完成后才读取下一条；不能直接承担网页并发。
2. `handle_prompt` 只有 `message`，总是走新会话流式路径，没有指定会话、审批、提问、续接或网页鉴权。
3. `run_prompt_streaming` 与 `run_prompt_in_session_streaming` 已分别实现新会话和指定会话的流式路径，必须复用。
4. 同一会话不能并发启动两个回合；现有会话写入、上下文恢复和工作流事件路由均要求服务层先串行化。
5. `AgentEventStream` 若没有持续消费者，回合可能被取消；网页连接不能拥有它。
6. `AgentEvent` 已经是思考、工具、终端、审批、提问、重试、任务和子代理的完整语义来源。
7. `TodoUpdated` 是会话任务清单唯一来源；`ToolOutputRef` 与 `ToolOutputStore::read_range` 是完整工具输出唯一来源。
8. 工作区已有 `axum`、`base64`、`rand` 与 `webbrowser`，首版不得为服务、令牌或打开浏览器重复引入依赖。

## 3. 允许范围

底层执行者可以在实施计划允许的前提下修改：

```text
Cargo.toml
crates/neo-webui/**
crates/neo-agent/Cargo.toml
crates/neo-agent/src/cli.rs
crates/neo-agent/src/main.rs
crates/neo-agent/src/modes/webui/**
crates/neo-agent-core/src/session/**
crates/neo-agent-core/src/events.rs
crates/neo-agent/tests/webui_behavior/**
crates/neo-webui/tests/**
```

实际命令行分发文件若与上述入口不同，先用 `cx` 找到唯一调用路径；只做最小扩展。

禁止修改：

- `crates/neo-tui/**`。
- 旧 `crates/neo-agent/src/rpc/**` 的行为或任何网页兼容转发。
- `AgentMessage`、提供方请求、系统提示、缓存前缀、历史顺序和追加式会话记录语义。
- Delegate、DelegateGroup、DelegateSwarm 的卡片、执行语义或工具语义。
- Bash、Terminal 的准入等待、无限等待、取消和输出捕获语义。
- 前端目录 `crates/neo-webui/web/**`，除非协调者明确授权处理纯打包接口且前端执行者已停止。

## 4. 固定架构

```text
neo-agent
  -> neo-webui
       -> neo-agent-core
```

### `neo-webui`

- 新 Rust 包，只持有 Axum 路由、静态资源、Cookie 鉴权、网页长连接、请求大小限制和序列化协议。
- 定义 `WebUiHost`、`WebUiCommand`、`WebUiReply`、`WebUiSnapshot`、`WebUiEventEnvelope`。
- `WebUiHost` 只能有两个职责：执行强类型网页命令，以及订阅强类型会话事件。不得发展成通用服务定位器。
- 不读取 JSONL、不创建 `AgentRuntime`、不执行工具、不解析 `AppConfig`。

### `neo-agent`

- 新增 `modes/webui/`，其中的 `WebSessionHost` 是网页回合、审批、提问、取消、队列和事件中继的唯一持有者。
- 它使用现有 `TurnChannels` 和 `run_prompt_*_streaming`。不得复制、包裹或重写 `AgentRuntime`。
- 每会话一个执行器。同会话的普通输入走既有后续输入队列；明确引导才走引导句柄；不同会话互不阻塞。
- 后台宿主持续消费 `AgentEventStream`、更新会话状态、持久化和广播。浏览器离线只移除订阅者。

### `neo-agent-core`

- `AgentEvent`、JSONL、`ToolOutputRef`、任务事件和会话标识校验仍保持唯一来源。
- 在现有会话元数据中增加 `pinned`、`archived`；缺失旧字段使用 `#[serde(default)]` 得到 `false`。
- 不向核心层传入网页 Cookie、URL、网页请求或 Axum 类型。

## 5. 监听与安全要求

1. `neo webui` 只绑定 `127.0.0.1:0`。任何远程、局域网、通配符、反向代理和自定义主机入口均不实现。
2. 监听成功后生成 32 字节随机、仅内存、一次性令牌。交互式终端默认输出完整 `#access=` 地址并调用已有浏览器打开能力；输出重定向时不打印令牌。
3. 令牌只能通过 `POST /api/auth/claim` 兑换一次；兑换后生成独立随机 Cookie，设置 `HttpOnly; SameSite=Strict; Path=/`，服务退出失效。
4. 前端必须在兑换成功后清除地址片段。服务端、请求日志、错误、JSONL 和工具输出不得保存令牌或 Cookie。
5. 所有请求严格校验实际 `Host`，必须为 `127.0.0.1:<实际端口>`；所有写请求和网页长连接还严格校验 `Origin`。缺失或不匹配的主机拒绝，写请求或网页长连接的来源缺失、为 `null` 或不匹配时拒绝；不发送跨域响应头。
6. 网页长连接用 Cookie 鉴权；禁止从 URL 查询、子协议、普通日志或前端本地存储传递令牌。
7. 全部敏感页面与接口使用不缓存、无来源泄露、禁止内容嗅探和严格内容安全策略。

## 6. 网页路径与数据形状

严格实现设计说明第 7 节中的当前路径。不得增加旧路径、别名或网页到旧 RPC 的桥接。

关键写入语义：

```text
POST /api/sessions
  第一条消息创建会话并启动回合；空白新会话不落盘。

POST /api/sessions/<session_id>/turns
  仅空闲会话启动新回合。

POST /api/sessions/<session_id>/input
  follow_up 或 steer；必须绑定现有会话。

POST /api/sessions/<session_id>/cancel
POST /api/sessions/<session_id>/approval
POST /api/sessions/<session_id>/question
  都要求当前 turn_id 和待处理项标识。

PATCH /api/sessions/<session_id>
  仅 title、pinned、archived。
```

- 前端选择模型、推理、逐条确认和开发模式时，必须构造逐回合请求覆盖；不得修改全局配置或影响其他会话。
- `GET /api/sessions` 的搜索只匹配标题，不能扫描完整转录。
- 工具输出路由必须验证 `ToolOutputRef` 和会话归属，绝不接收文件系统路径。

## 7. 事件续接与有界内存

每条网页事件的最小形状：

```json
{
  "type": "session_event",
  "stream_id": "本次服务启动标识",
  "session_id": "...",
  "sequence": 43,
  "event": {}
}
```

- `sequence` 对每个会话、每次服务启动单调递增。
- 只有当前被网页查看的会话转发完整 `AgentEvent`；其他会话只传状态和元数据变化。
- `watch_session` 的正确顺序是：注册观察者并获得切换序号，读取快照，发送快照，补发序号之后的事件。
- `stream_id` 改变、缓存不足或续接失败时发送完整快照。不得猜测、跳过或重排历史。
- 每会话事件缓存最大 `256KiB`，全服务最大 `4MiB`；每连接发送队列有界。慢连接关闭后只能重连和取快照。
- 大型工具和终端输出不复制入缓存，只返回输出引用；范围读取必须复用 `ToolOutputStore::read_range`。
- 重试期间临时输出按既有事件清除，不写成新历史。

## 8. 必须新增的测试

使用最小、明确的 Rust 目标和真实浏览器边界，不要用固定端口、固定等待、共享工作目录或真实提供方。

至少覆盖：

1. 回环随机端口、一次性令牌兑换、终端输出脱敏、Cookie 与地址片段清理。
2. 主机和来源拒绝、无 Cookie 的网页长连接拒绝、令牌与 Cookie 不进入日志。
3. 两会话并行、同会话后续输入排队、切换和断线不取消回合。
4. 重连补发、缓存溢出后的快照、事件去重与重试临时输出清除。
5. 审批、提问、取消的会话、回合、待处理项三重匹配。
6. 会话置顶归档持久化与旧元数据默认值。
7. 工具输出引用分页、跨会话引用拒绝、任意路径拒绝。
8. macOS、Linux、Windows 的回环监听和浏览器打开路径不使用 Shell 平台分支。

前端需要固定事件样本。底层执行者必须提供规范 JSON 样本，至少包含：新会话、普通助手回复、思考、排队工具、终端输出、审批、提问、重试、任务更新、Delegate、DelegateGroup、DelegateSwarm、改动统计和断线快照。

## 9. 停止条件与报告

出现以下任一情况时停止并报告，不得自行扩展架构：

- 现有逐回合配置不能隔离会话设置。
- 会话 JSONL 无法同时提供一致快照与实时事件切换点。
- `ToolOutputRef` 没有足够信息验证会话归属。
- 需要修改提供方请求、系统提示、缓存前缀或规范历史才能实现网页。

交付报告只包含：修改文件、每项验收的精确命令和结果、固定事件样本位置、未解决阻断和剩余风险。不要自行提交。
