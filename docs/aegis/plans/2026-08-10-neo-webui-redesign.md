# Neo WebUI 重设计实施计划

- Goal：按 `docs/aegis/specs/2026-08-10-neo-webui-redesign-design.md` 重写 WebUI 呈现层并补后端数据面：去卡片化 transcript、子代理钻孔面板、跨工作区侧栏、composer pill/ContextRing/附件、亮暗双主题。
- Architecture：`neo-webui`（协议/中继/HTTP/嵌入资源）+ `neo-agent` modes/webui（宿主/会话投影）+ `web/`（Vite+React+TS，纯 reducer + 分片 store）。不改 TUI/旧 RPC/JSONL 语义/缓存前缀。
- Tech Stack：Rust（axum 既有）、React 19 + TS、单一全局 CSS（语义令牌，无框架）、Vitest + Playwright。
- Baseline/Authority Refs：重设计 spec（b77a1c1c）；`2026-08-09-neo-webui-design.md`；两份 remaining 交接（协议与安全不变量）；调研证据 `docs/aegis/work/2026-08-10-neo-webui-redesign/reference/`；media-input 上游设计 `docs/aegis/specs/2026-08-10-media-input-model-cache-lanes-design.md`（仅依赖形状）。
- Compatibility Boundary：最终协议直接替换未发布线形，不留兼容分支；规范事件/JSONL/历史顺序只读；监听 127.0.0.1；令牌语义不变；网页只见工作区相对路径与 workspace label；`web/dist` 仍恰三文件。
- TDD Route：Mode off；Decision: skipped（无显式 TDD 请求）；Test posture: post-change regression；Verification：每任务精确测试 + 评审 + 协调者提交。
- Verification：每任务自带精确命令；批次末 `rtk cargo nextest run -p neo-webui --test webui_behavior`、`-p neo-agent --test webui_behavior`、`rtk npm --prefix crates/neo-webui/web run test`、Playwright 截图、fmt/clippy。

## Execution Readiness View

- Intent Lock：只实现 spec §1 锁定的五项决策；不引入浅色以外的主题机制扩张、不做 dock、不做子代理输入。
- Scope Fence：后端仅 `crates/neo-webui/{src,tests,fixtures}`、`crates/neo-agent/src/modes/webui`、`crates/neo-agent-core/src/session`（只读面）；前端仅 `crates/neo-webui/web/**`。
- Baseline Lock：上述 Baseline refs；冲突时以重设计 spec 为准，停止并报告而非自行扩张。
- Compatibility Boundary / Retirement Boundary：见文头；旧卡片组件与旧令牌随重写删除，无双轨。
- Task Batches：R0 spike → R1 后端 ∥ R2 主题底座 → R3 transcript → R4 子代理 ∥ R5 侧栏 ∥ R6 composer（同树串行分派）→ R7 验收。
- Test Obligations：每个协议/行为改动一条精确回归；前端 reducer/组件 Vitest；Playwright 截图双主题。
- Review Gates：每任务 spec 评审 + 质量评审（用户已豁免时可简化为协调者抽查，记录豁免）。
- Drift / Rewind Rules：遇 spec 未定的新 owner/协议面 → 停止报告；禁用 git 历史重写；每任务一提交。
- Evidence Required：精确测试非零结果、截图路径、dist 三文件核对、性能验收记录。

## 任务 R0：子代理事件可得性 spike（后端，先于 R1d）

- Files：`crates/neo-agent-core/src/events.rs`、`crates/neo-agent/src/modes/webui/{host,session}.rs`（只读调研 + 一页结论）。
- 目标：确认 Delegate 子代理的逐条 `AgentEvent` 是否在运行时可观测（子代理是否共享/可挂接事件流、快照是否可重建其历史）。
- 产出（写入 `docs/aegis/work/2026-08-10-neo-webui-redesign/30-agent-spike.md`）：
  1. 子代理事件流的可挂接点（有/无，file:line 证据）；
  2. 结论 A「可做完整面板」：给出 watch_agent 的事件来源；或结论 B「降级面板」：快照+进度+结果+用量。
- Verification：结论中引用的符号存在性（`cx references` / graph trace）。
- Stop：需要改动运行时语义才能得到事件流 → 直接结论 B。

## 任务 R1：后端数据面（四个独立子任务，可串行一人）

### R1a 用量与上下文入快照/状态

- Files：`crates/neo-webui/src/protocol.rs`、`crates/neo-agent/src/modes/webui/session.rs`、`crates/neo-agent/src/modes/webui/test_cases/`、`crates/neo-webui/tests/webui_behavior/**`。
- 协议新增（deny_unknown_fields 不变）：
```rust
pub struct WebUiContextWindow { pub used_tokens: u64, pub projected_tokens: Option<u64>,
  pub max_tokens: Option<u64>, pub remaining_tokens: Option<u64> }
// WebUiSessionState 增加：pub token_usage: Option<AgentTokenUsage>,
//                          pub context_window: Option<WebUiContextWindow>
```
- session.rs：事件摄入时缓存最新 `TokenUsage`/`ContextWindowUpdated`（同 `last_todos` 模式），快照与 `session_state` 携带。
- 测试：`usage_and_context_window_survive_snapshot_and_reconnect`（发事件→快照断言字段→重连快照仍携带）。
- Verification：`rtk cargo nextest run -p neo-agent --test webui_behavior usage_and_context_window` + 全量。

### R1b 跨工作区聚合

- Files：`crates/neo-agent/src/modes/webui/host.rs`、`crates/neo-webui/src/protocol.rs`、`crates/neo-agent/tests/webui_behavior/**`。
- 协议：`WebUiSessionSummary` 加 `workspace_label: String`；`workspace_snapshot` 加 `workspaces: [{ label, current: bool, sessions: [...] }]` 分组视图（保留平铺 sessions 字段与否由实现取简，不留双轨——选分组为唯一形状）。
- host：读全局 `session_index.jsonl` 列出各工作区桶；label = 目录基名，重名加 `<hash8>`；绝不输出绝对路径。`watch_session`/快照/读取对非当前工作区会话以其自身工作区装载（与 CLI resume 语义一致）；工具执行上下文随会话。
- 测试：`cross_workspace_listing_groups_by_label_without_absolute_paths`（构造两个工作区桶，断言分组、label、响应无 `/` 绝对路径）；`watch_session_loads_session_from_another_workspace`。
- 边界：聚合列表失败降级为仅当前工作区（不报错泄路径）。

### R1c 附件上传 + 模型目录

- Files：`crates/neo-webui/src/{protocol,server}.rs`、`crates/neo-agent/src/modes/webui/host.rs`、`crates/neo-webui/tests/webui_behavior/**`。
- 路由：`POST /api/attachments`（JSON `{mime, base64}`；8MiB/个上限，413；类型白名单 image/* 起步）→ `201 {id, mime, byte_len}`，字节按摘要写入会话级 Blob 暂存；`CreateSession/StartTurn/SendInput` body 加 `attachments: Option<Vec<String>>`（最多 4，未知 id 400）。发送时按有效能力裁剪，不可发送→确定性文本替换（依 media-input 设计），绝不写回历史。
- bootstrap 增加 `models: [{ alias, provider, context_window, capabilities }]`（来自 ModelRegistry 只读投影，不含密钥/base url 之外敏感信息——只含展示字段）。
- 测试：`attachment_upload_enforces_size_type_and_count_limits`、`message_with_attachments_projects_per_capability`、`bootstrap_models_catalog_has_display_fields_only`。
- 依赖：media-input 设计的 MediaRef/Blob 形状；若其类型未落地，webui 侧先以自己的暂存 id 传递，接入点标注。

### R1d 子代理订阅（形状依 R0 结论）

- 结论 A：入站 `watch_agent { agent_id, after? }`；出站 `agent_snapshot { stream_id, agent_id, snapshot }`、`agent_event { stream_id, agent_id, sequence, event }`；复用每连接有界队列与 1013 语义；未知 agent_id → `not_found`。
- 结论 B：`GET /api/agents/<agent_id>` 一次性读快照（进度+结果+用量），无长订阅。
- 测试：A=`watch_agent_streams_child_events_with_cursor_dedup` + `unknown_agent_gets_not_found`；B=`agent_snapshot_read_is_scoped_and_bounded`。
- fixture：补 `DelegateProgressUpdated`、`DelegateSwarmProgressUpdated`、`ContextWindowUpdated`、`TokenUsage`、带附件用户消息、多工作区摘要样本。

R1 验证（全绿才提交）：`rtk cargo nextest run -p neo-webui --test webui_behavior`、`rtk cargo nextest run -p neo-agent --test webui_behavior`、`rtk cargo fmt --all --check`、`rtk cargo clippy -p neo-webui --lib -- -D clippy::all`、`rtk cargo clippy -p neo-agent --test webui_behavior -- -D clippy::all`、`rtk cargo build -p neo-agent`。

## 任务 R2：主题令牌底座（前端，先行）

- Files：`web/src/styles.css`（重写头部令牌区）、`web/src/state/appState.ts`（`theme`）、`web/src/components/topBar.tsx`（切换按钮）、`web/src/main.tsx`（初始化）。
- 结构：原始色板（dark/light）→ 语义令牌；`html[data-theme]` 切换；初始 `prefers-color-scheme`，切换写 localStorage（允许清单：侧栏宽度、主题）；组件只引语义令牌。
- 关键令牌（dark 沿用现值微调，light 用调研 tokens.md 浅灰蓝体系）：
```css
[data-theme="light"]{ --bg:#f9fbfc; --surface:#ffffff; --surface-2:#f5f5f5;
  --text:rgba(0,0,0,.9); --text-dim:rgba(0,0,0,.6); --text-faint:rgba(0,0,0,.45);
  --accent:#1783ff; --accent-soft:rgba(23,131,255,.12); --border:rgba(0,0,0,.08); }
[data-theme="dark"]{ /* 现 --bg/--bg-raised/--text 等映射到同一语义名 */ }
```
- 间距/字号/圆角/时长/缓动令牌按 spec §3 全量定义（`--chat-turn-gap:20px` 等）。
- 测试：`theme_toggle_persists_and_defaults_to_system`（Vitest + matchMedia mock）。
- Verification：`rtk npm --prefix crates/neo-webui/web run test`。

## 任务 R3：Transcript 重写（前端核心）

- Files：`web/src/components/{transcript,transcriptItems}.tsx`（重写）、新增 `userTurn.tsx / thinkRow.tsx / toolRow.tsx / turnFold.tsx / answerFooter.tsx / approvalRow.tsx`、`web/src/state/transcript.ts`（补 Progress 两 tag、usage 投影）、`web/src/styles.css` 转录区。
- 结构按 spec §4.1：用户气泡（>8 行渐变省略+展开/收起）；`.tool-line` 单行+展开体（命令回显/参数/输出/元信息）；`.think` 呼吸动画单行；TurnFold（完成回合过程聚合「工作了 Ns · K 步」）；`.answer-ft`（本回合文件修改列表=从 Edit/Write 工具事件推导相对路径+增删行；复制按钮写 Markdown，成功反馈）；审批/提问原位行（2px 竖条，非卡片）。
- 动画：全部 `grid-template-rows 0fr→1fr` + `cubic-bezier(.16,1,.3,1)`；TextDelta 原位淡入；`prefers-reduced-motion` 退化。
- 性能：转录项 `content-visibility:auto; contain-intrinsic-size:auto 120px`；保持回到底部/跟随语义。
- reducer：新增 `DelegateProgressUpdated`/`DelegateSwarmProgressUpdated` case（按 `progress`/`child_progress` 归并）；`TokenUsage`/`ContextWindowUpdated` 移出 KNOWN_SILENT_TAGS → projection `latestUsage/contextWindow`。
- 测试：每组件 Vitest（折叠展开、渐变省略、TurnFold 聚合、answer-ft 推导与复制、Progress 归并、usage 投影）。
- Verification：`rtk npm --prefix crates/neo-webui/web run test` 全量。

## 任务 R4：Subagent UI（前端）

- Files：`web/src/components/agentLine.tsx / swarmBlock.tsx / agentPanel.tsx`（新）、`web/src/state/{appState,store}.tsx`（面板状态 + watch_agent 订阅或一次性读取）、样式区。
- agent-line：图标+标题+状态丸+耗时+脉冲点；swarm-block：头部聚合进度条 + 成员行 stagger-in + 连接线缩进。
- 详情面板：右侧 overlay `min(560px,45%)`，transform 滑入 200ms，Esc/遮罩关闭还焦；内容复用 R3 渲染树（数据源=agent_snapshot/agent_event 或降级一次性读取）；顶部标题/模型/状态/耗时/用量；只读。
- 测试：面板开关/还焦、swarm 成员行渲染与进度、降级形状（若 R0=B）。
- 依赖：R1d 协议 + R3 组件。

## 任务 R5：Sidebar 重写（前端）

- Files：`web/src/components/{sidebar,sidebarResizer}.tsx`、`web/src/state/appState.ts`、样式区。
- 工作区分组（当前组默认展开+新建按钮；他组折叠）、Pinned 区、归档折叠入口；会话行 13.5px 标题/11.5px 副行、运行脉冲点、审批/提问计数徽章、悬停按钮、右键/菜单键/Shift+F10 同菜单（不切换会话、关闭还焦）。
- 拖拽：rAF 节流写 `--sidebar-w`，拖拽中禁用转录过渡；220-400px。
- 测试：分组折叠、徽章、拖拽宽度写回 localStorage、菜单键盘路径。

## 任务 R6：Composer 重写（前端）

- Files：`web/src/components/composer.tsx`（重写）+ `modelPicker.tsx / contextRing.tsx / attachmentTray.tsx / welcomeBanner.tsx`（新）、store/api（attachments 上传、bootstrap models）。
- pill 行：附件📎（36px 圆钮+拖拽高亮+队列 chip 可移除）、模型 pill（覆盖层：搜索+星标+行内 `名称·provider·上下文·能力chip`）、权限 pill（灰/警告/危险三态，仅下一回合）、模式 pill（有能力时）、推理 pill（有能力时）；ContextRing（16px SVG 圆环+百分比+tooltip `used / max (pct%)`）。
- 欢迎 banner：仅新会话显示于输入框正上方，克制内容，首条规范用户消息后消失。
- 发送语义不变（创建/turns/follow_up/steer/停止独立）；附件随消息 body 传 id；上传失败保留草稿+错误提示。
- 测试：pill 交互、ContextRing 数值、附件队列与失败态、欢迎 banner 显隐。

## 任务 R7：验收（前端+协调者）

- 截图（mock 服务，≥14 张）：spec §9 清单 + 亮色对照 ≥4 张；存 `web/screenshots/`（不入库）。
- 性能：10 万字符会话滚动/侧栏拖拽流畅（content-visibility 生效：离屏项不渲染的证据截图/计数）。
- `rtk npm --prefix crates/neo-webui/web run build` 后核对 dist 恰三文件、无外部 URL、无 source map；`git diff --exit-code -- crates/neo-webui/web/dist`（提交后）。
- 更新 fixture 驱动测试至最终样本；删除所有旧卡片类名死代码。

## 风险与回退

1. R0 结论 B 时 R4 降级（面板=快照+结果+用量），spec 已允。
2. 跨工作区装载（R1b）是最大后端风险：遇宿主上下文切换困难 → 停止报告，不绕过权限/工作区语义。
3. 附件依赖 media-input 形状：若上游未落地，R1c 以 webui 暂存 id 自成闭环，接入点留注。
4. 回退面：每任务独立提交；前端重写为 web/** 内替换，可由 dist 三文件 + 源码目录整体回退（git revert 对应提交）。

## 提交纪律

每任务（R1a-d 各一、R2-R7 各一）验证+评审后由协调者精确提交；实现者不碰 git。提交信息不出现参照产品名。
