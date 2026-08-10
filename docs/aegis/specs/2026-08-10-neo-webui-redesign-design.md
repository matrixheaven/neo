# Neo WebUI 重设计说明

日期：2026-08-10
状态：待用户书面评审
上游：`docs/aegis/specs/2026-08-09-neo-webui-design.md`（产品范围不变，本说明替换其网页呈现层）；调研证据 `docs/aegis/work/2026-08-10-neo-webui-redesign/reference/`（实机截图、DOM/computed-style 提取、设计令牌、交互观察）

## 1. 背景与已锁定决策

当前前端被用户判定不可用：转录是卡片堆叠（丑）、Delegate/Swarm 事件显示「未识别」（`DelegateProgressUpdated`/`DelegateSwarmProgressUpdated` 前端无 case）、侧栏拖拽卡顿且无工作区维度且字体过大、composer 选择器无设计、无上下文占用、无附件。

调研结论（参照产品的成熟 WebUI，截图与 DOM/样式证据见调研目录）：其核心是「排版层级而非容器」——只有用户消息是气泡；工具调用是 24px 单行（图标+名称+淡化参数摘要+状态丸），点击就地展开；思考块单行折叠带呼吸动画；整段回合可折叠；折叠动画统一纯 CSS `grid-template-rows 0fr→1fr`；转录用 `content-visibility` 虚拟化；侧栏按工作区分组；composer 有 pill 选择器、上下文用量环、附件按钮；子代理为「转录内入口 → 右侧详情面板」钻孔模型。

用户已锁定：

1. **范围**：前后端一起改（含新协议面）。
2. **主题**：**亮色/深色双主题，可切换**。深色借参照产品的「形」（排版层级、动画体系、间距令牌、组件结构）；亮色参照其浅灰蓝体系（表面 `#f9fbfc`、文字 `rgba(0,0,0,.9/.6/.45)` 三级、强调 `#1783ff`）。
3. **附件**：本轮做（含上传协议）。
4. **侧栏**：跨工作区聚合（后端读全局会话索引，按工作区分组，可切换任意工作区会话）。
5. **子代理**：钻孔式详情面板（转录内入口 → 右侧完整子代理转录面板），配特色动画。

用户补充的界面事实（截图 03a/03b/03c/04d，并更正 04c 为用户对话+助手最终回答而非思考块）：

- **用户长消息折叠**：transcript 里用户输入超过若干行时，气泡底部渐变省略 + 「展开」按钮。
- **助手最终回答页脚**：最终回答底部显示本回合文件修改列表（路径 + 增删行），并有复制按钮把最终回答以 Markdown 复制。
- **新会话欢迎 banner**：新会话时输入框上方出现欢迎横幅。
- **工具展开详情**：工具行展开后除参数/输出外还有命令回显与状态元信息区。

## 2. 设计原则

1. **排版即层级**：不用卡片承载过程信息。语义靠字色三级衰减、等宽/比例字体、缩进连接线表达。
2. **行而非卡**：工具/终端/思考/子代理默认都是单行（24-28px），点击就地展开，展开区无背景色块或仅 2-4% 亮度差。
3. **动画纯 CSS**：统一 `cubic-bezier(.16,1,.3,1)`，快 120ms / 常规 200ms；折叠统一 `grid-template-rows 0fr→1fr + overflow:hidden`；流式文本增量淡入；运行态脉冲/呼吸；禁止 JS 动画库。`prefers-reduced-motion` 全部退化为即时切换。
4. **性能即设计**：转录项 `content-visibility: auto` + `contain-intrinsic-size` 原生虚拟化；侧栏拖拽经 rAF 节流写 CSS 变量；长转录不阻塞拖拽是验收项。
5. **双主题令牌**：`:root` 语义令牌分两层——原始层（色板）与语义层（surface/text/accent/status/shadow）；`html[data-theme="light"|"dark"]` 切换语义值，组件只引用语义层。初始值跟 `prefers-color-scheme`，用户切换后持久化；localStorage 允许清单扩充为：侧栏宽度、主题偏好（仍禁令牌/转录/草稿以外状态）。
6. 既有安全与协议不变量全部保留：一次性令牌、同源、无 dangerouslySetInnerHTML、Markdown 链接白名单、不透明 output 引用、工作区相对路径、无乐观消息。

## 3. 间距与尺寸骨架

```text
侧栏 264px（min 220 / max 400，拖拽 + 键盘），顶栏 48px
内容列 max 760px 居中；composer max 728px
--chat-turn-gap:20px  --chat-block-gap:10px  --chat-section-gap:18px
字号：正文 14px / 1.6；辅助 12.5px；微标 11.5px；等宽 12.5px
圆角：用户气泡 14px；展开体 10px；composer 24px；菜单 12px；pill 999px
阴影：菜单/覆盖层 elevation-2
主题切换入口：顶栏右侧图标按钮（日/月），即时切换无刷新
```

## 4. Transcript 重设计

### 4.1 结构

```text
.u-turn            用户回合：右对齐气泡（radius 14px，pre-wrap）
                   超过约 8 行：底部渐变遮罩省略 +「展开/收起」按钮；下方 11.5px 淡化时间戳
.a-msg             助手消息：左对齐通栏 prose（max 760px）
  .think           思考单行：图标 +「思考」+ 计时；流式时标题呼吸；点击展开（caret 旋转 90°，0fr→1fr）
  .tool-line       工具单行：状态图标 + 工具名 + 淡化参数摘要（等宽、截断）+ 右侧状态丸/耗时/排队位置
                   展开体：命令回显（等宽）+ 参数 + 输出（文本节点/<pre><code>，output.id 透传读取完整输出）+ 状态元信息
  .agent-line      Delegate 单行（见 §5）
  .swarm-block     Swarm 成员列表块（见 §5）
  .approval-row    审批/提问原位行：无卡片，左侧 2px accent 竖条 + 行内按钮/chip
  TurnFold         已完成回合的过程合集自动收一行「工作了 Ns · K 个步骤」，展开看全部过程行
  .answer-ft       助手最终回答页脚：本回合文件修改列表（相对路径 + 增删行数，点击可打开改动详情）
                   + 复制按钮（最终回答以 Markdown 写入剪贴板，带成功反馈）
```

页脚的文件修改列表由前端从本回合 Edit/Write 等工具事件推导（事件已携带工作区相对路径），不新增后端面；无文件修改时不渲染。

规则：增量 TextDelta 原位淡入不成为独立气泡；Retry 系列撤去临时输出的既有 reducer 语义不变；未知事件标签仍保留可折叠原始 JSON（样式同 tool-line 降级）。

### 4.2 滚动与性能

- `content-visibility:auto; contain-intrinsic-size:auto 120px` 作用于每个转录项；长会话（10 万字符级）滚动与侧栏拖拽 60fps 为验收标准。
- 保持现有「离开底部不强制滚动 + 回到最新内容」行为；流式期间自动跟随。
- 侧栏拖拽：rAF 节流写 `--sidebar-w`；转录列尽量布局隔离，拖拽期间禁用转录过渡动画。

### 4.3 审批与提问

原位行（非卡片）：细竖条 + 标题 + 描述（淡化）+ 主/次按钮或选项 chip + Other 输入。提交一次后禁用待服务端确认；快照裁决 `no_longer_pending` 时显示淡化「已失效」。不依赖颜色单独传达状态（带文字）。

## 5. Subagent（Delegate/Swarm）UI

数据现状：8 个 Delegate 事件后端已透传；`AgentSnapshot`/`AgentProgressSnapshot`/`SwarmSnapshot` 含状态、进度摘要、结果；**不含子代理逐条事件转录**。钻孔面板需要新订阅面（§7.4）。

### 5.1 转录内呈现（内联层）

- **Delegate**：`.agent-line` 单行 = 代理图标 + 标题 + 状态丸（running 脉冲/done ✓/failed ✕/timed_out）+ 耗时。运行中标题左侧 6px 脉冲圆点。点击：有详情数据 → 打开右侧面板；否则就地展开进度摘要。
- **Swarm**：`.swarm-block` 头部单行（图标 + 标题 + `完成 x/N` + 聚合进度细条）；成员列表每行同 agent-line 样式，`border-l` 连接线缩进，成员可单独点开面板。成员行 stagger-in（每行延迟 30ms 淡入上移 4px）。
- 修复 `DelegateProgressUpdated`/`DelegateSwarmProgressUpdated`：progress 载荷按 `progress`/`child_progress` 字段归并进现有 Delegate/Swarm item（更新进度文本与状态），fixture 补样本。

### 5.2 钻孔详情面板（右侧 overlay）

- 宽度 `min(560px, 主区 45%)`，从右侧滑入（transform 200ms），不压缩聊天列；Esc/点击遮罩关闭并还焦。
- 内容是**该子代理的完整转录**，复用 §4 全部组件（同一渲染树，不同数据源）；顶部显示标题、模型、状态、累计耗时、Token 用量（若有）；底部只读（不向子代理发输入）。
- 动画：打开滑入 + 内容 120ms 淡入；关闭反向。

## 6. Sidebar 重设计

### 6.1 结构

```text
搜索框（请求后端标题搜索）
Pinned（置顶，跨工作区）
工作区分组（可折叠）：
  ▸ 当前工作区（默认展开，组头右侧 + 新建会话）
  ▸ 其他工作区（按最近活跃排序，默认折叠）
    会话行：标题 13.5px/450 字重（截断）+ 副行 11.5px 淡化（相对时间 · 状态徽章）
    运行中：标题左 6px 脉冲圆点；等待审批/提问：琥珀徽章计数
    悬停：置顶/归档图标按钮；右键/菜单键/Shift+F10：重命名/置顶/归档
归档分组（每组底部「已归档 n」折叠入口）
```

字体整体降一档；行高紧凑（40-44px/行）。选中态 = accent-soft 底 + 内描边。

### 6.2 跨工作区聚合（后端）

- webui host 增加读全局 `session_index.jsonl` 的列表面：每个工作区桶给出 `workspace_label`（目录基名，重名加短哈希后缀），**绝不暴露绝对路径**。
- 摘要 `WebUiSessionSummary` 增加 `workspace_label` 字段；`workspace_snapshot` 按工作区分组返回。
- 切换其他工作区会话：该会话以其**自身记录的工作区**为运行上下文装载（与 CLI 跨目录 resume 语义一致）；网页只看到 label。权限与工具执行语义不变（会话内工具仍按其会话工作区判定）。
- 非当前工作区的会话同样遵守「摘要更新、不传完整转录直到选中」。

## 7. Composer 重设计

### 7.1 结构与 pill 行

保持悬浮半透明容器（24px 圆角、blur、细边框），内部重排：

```text
[新会话：欢迎 banner（输入框正上方，会话开始后消失）]
[附件队列区（有附件时）]
[textarea 52-180px]
[ pill 行：📎附件 | 模型 pill | 权限 pill | 模式 pill(plan/goal) | 推理 pill(有能力时) ⫽ ContextRing + 发送/停止 ]
```

- **欢迎 banner**：仅新会话（无规范用户消息）显示，置于输入框正上方，内容克制（标识 + 一句话），不做营销首屏。
- **模型 pill**：`max-width 200px` 截断；点击开覆盖层：搜索框 + 星标 + 每行 `名称 · provider · 上下文大小 · 能力 chip`。数据来自 bootstrap（后端补模型目录面，见 §7.3）。
- **权限 pill**：ask/auto/yolo 三态，色阶灰/警告/危险；仅作用于当前会话下一回合（不写回全局设置）。
- **模式 pill**：plan/goal 开关（后端能力提供时显示）。
- **ContextRing**：16px SVG 圆环 + 百分比数字；数据消费已在 wire 上的 `ContextWindowUpdated`/`TokenUsage`（前端从 KNOWN_SILENT_TAGS 移出并投影 `latestUsage/contextWindow`）；快照/状态消息补字段（§7.2）保证刷新/切换后可恢复。tooltip 显示 `83.7k / 256k tokens (33%)`。
- **附件**：圆形 36px 📎 按钮 + 拖拽悬停高亮；附件队列 chip（缩略/文件名 + 移除）；上传协议见 §7.3。
- 发送/停止/立即引导：保持独立可辨识；steer 仅运行中显示。

### 7.2 用量/上下文的协议补充

`WebUiSessionState` 增加 `token_usage` 与 `context_window` 可选字段（`used_tokens/max_tokens/remaining_tokens`），由 session.rs 在事件摄入时缓存最新值（同 last_todos 模式），快照携带，保证重连/切换后立即可读。

### 7.3 附件上传协议与模型目录（新）

对齐 `docs/aegis/specs/2026-08-10-media-input-model-cache-lanes-design.md`（待审）的 `MediaRef`/Blob 形状，本说明只定义 WebUI 传输面：

```text
POST /api/attachments            multipart 或 {mime, base64}，大小上限 8MiB/个、4 个/回合
  → 201 { id, mime, byte_len }    字节写入会话级 Blob 暂存（按摘要寻址），id 不透明
POST /api/sessions | /turns | /input   body 增加可选 attachments: ["<id>", ...]
bootstrap 增加 models 目录（别名、provider、上下文大小、能力），供模型 pill 覆盖层
```

- 服务端按有效能力（模型媒体能力 × 提供方传输）裁剪；不可发送时按媒体设计替换为确定性文本，绝不写回历史正文。
- 浏览器只做选择/预览/上传/透传 id，不读文件路径。
- 若 media-input 上游设计评审结论改变 Blob/引用形状，本传输面跟随调整（依赖已声明）。

### 7.4 子代理转录订阅（新）

```text
watch_agent { agent_id, after? }        入站
agent_snapshot { stream_id, agent_id, snapshot }   出站：子代理事件历史投影
agent_event { stream_id, agent_id, sequence, event } 出站
```

实施前先做数据可得性 spike：Delegate 子代理的逐条事件在运行时是否可观测/可持久化。若子代理事件不可得，降级方案 = 面板展示进度快照 + 结果 + 用量（仍需 `agent_snapshot` 一次性读取），在设计评审时如实标注。**不允许**为面板新建第二事件存储；若需持久化子代理事件，必须作为会话规范 JSONL 的既有追加语义的一部分。

## 8. 样式架构

`styles.css` 重写为令牌分组 + 区域分节（保持单一全局表、无框架）：原始色板（light/dark 两套）→ 语义令牌 → base → shell → sidebar → transcript（u-turn/a-msg/think/tool-line/agent/swarm/approval/answer-ft）→ composer → menus/popover → panel → responsive（980/720 断点保留）。类名契约随组件重写同步更新，不留旧类死代码。主题切换只写 `data-theme` 属性，组件零条件分支。

## 9. 测试与验收

- reducer：补两个 Progress tag case、usage/context 投影、附件队列（仅本地发送态，非消息气泡）；更新全部 fixture 驱动测试（fixture 由后端同步补样本）。
- 组件：pill 打开/选择、ContextRing 数值、附件上传失败态、侧栏分组折叠/拖拽、面板打开/关闭/还焦、主题切换持久化、用户长消息渐变展开、answer-ft 文件列表与复制、欢迎 banner 显隐。
- 浏览器截图（mock 服务）：宽屏新会话（含欢迎 banner）、宽屏运行会话（tool-line 行态 + TurnFold）、思考展开、工具展开详情、agent-line + 详情面板、swarm 块、用户长消息折叠、answer-ft、侧栏多工作区分组、composer pill 行 + ContextRing + 附件队列、右键菜单、窄抽屉、手机单列、**亮色全套对照**（关键页至少 4 张）。
- 性能验收：10 万字符会话滚动/侧栏拖拽无明显卡顿（content-visibility 生效证据：离屏项不渲染）。
- 后端精确测试：跨工作区列表不泄绝对路径、摘要带 label、附件大小/数量上限、用量字段入快照、watch_agent 未知 id 拒绝。
- dist 三文件约束不变。

## 10. 非目标与边界

- 不改 TUI、旧 RPC、JSONL 语义、缓存前缀、Delegate/Bash/Terminal 运行语义。
- 不做子代理输入（面板只读）；不做底部 dock 多 tab（任务清单保持现有悬浮设计，仅视觉并入新令牌）。
- 监听仍 127.0.0.1；无跨域；令牌语义不变。
- media-input 上游设计（agent-core 链路）不属于本说明，仅声明依赖。

## 11. 风险

1. 子代理逐条事件可得性未证实（§7.4 spike 先行，降级方案已备）。
2. 跨工作区装载的运行时上下文切换需要 host 层仔细处理（会话自带工作区）。
3. 附件依赖待审的 media-input 设计，形状可能微调。
4. content-visibility 与布局隔离的兼容性需实测（备选：仅 content-visibility）。
5. 双主题使视觉验收面翻倍（截图清单已含亮色对照）。
