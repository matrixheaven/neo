# Neo TUI 全界面鼠标文本选取实施计划

## Goal

让 Neo 当前所有可见且非敏感的 TUI 文本都能由应用内左键长按或拖动选取，并保证高亮内容与复制内容完全一致。保留正文和聊天输入框的既有语义，其余普通界面、弹窗、任务浏览器和主题管理器统一使用最终画面选区，不再逐界面补丁。

## Architecture

保留三个互不重叠的选区所有者：`TranscriptPane` 负责正文文档坐标，`PromptState` 负责聊天输入框可编辑字符范围，`NeoTui` 负责其余最终可见画面的屏幕坐标。鼠标按下时确定手势所有者，拖动和释放始终交回该所有者。

最终画面选区由一个小型 `frame_selection.rs` 模块持有。普通画面和全屏覆盖层都在加入左侧留白、提取硬件光标标记之后进入同一个画面收尾步骤；该步骤记录可见文本映射、执行失效判断并绘制高亮。它只处理派生显示数据，不进入正文存储、会话、模型上下文或提供方请求。

## Tech Stack

Rust 2024、`crossterm 0.29`、现有 Unicode 分词与终端单元格辅助函数、`cargo nextest`、现有 `NeoTui` 和 `InteractiveController` 测试框架。不得增加依赖。

## Baseline/Authority Refs

- `AGENTS.md`
- `docs/aegis/specs/2026-08-07-tui-mouse-text-selection-design.md`
- `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`
- `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
- 用户于 2026-08-07 对书面设计的明确批准

## Compatibility Boundary

- 不修改 Delegate、DelegateGroup、DelegateSwarm、Workflow 卡片本体、层级、进度、展开语义和正文条目布局。
- 不修改工具授权、执行、审批顺序、会话持久化、模型上下文、提供方请求、压缩输入或缓存前缀。
- 不修改原始鼠标序列解析、输入队列、终端鼠标捕获、备用屏幕进入和恢复生命周期。
- 不增加按钮点击、列表点击、页签点击、弹窗鼠标编辑或隐藏内容复制。
- 不建立第二套正文选区、第二视口、第二渲染器或兼容回退路径。
- `Shift` 拖选继续交给终端；滚轮继续由现有焦点界面处理。
- 打印、管道、导出、`neo run` 和非 TTY 恢复继续使用静态路径，不持有画面选区。

## TDD Route

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: 用户与项目都没有要求严格测试先行；设计、根因和验收边界已经批准，采用最小实现后精确回归。
- Verification: 每条 Rust 验证命令必须指定一个包、一个目标选择器和一个测试过滤词；先用 `cargo nextest list` 确认过滤词至少命中一个测试，再运行同过滤词的 `cargo nextest run`。

## Aegis Visibility

这次变更跨越最终画面、鼠标路由、复制优先级和旧所有者退役；计划用于固定唯一所有者、阻止逐弹窗分支增长，并把跨平台与隐私负面证明写成完成条件。

## Plan Basis

- 规划开始时的历史快照：`main`，`HEAD` 为 `4c722d95`，相对 `origin/main` 领先 8 个提交；规划期间共享工作树继续变化，因此这不是执行基线。
- 规划开始时存在外部未提交改动：`crates/neo-tui/tests/transcript_selection.rs` 和 `docs/aegis/INDEX.md`；随后还出现其他任务的源码与文档改动。
- 执行时必须重新读取实际 `HEAD` 和工作树；上述快照只能用于识别漂移，不能用于覆盖或回退用户改动。
- `transcript_selection.rs` 的并行改动把错误的选区提示行期望改为“不减少正文高度”；无论执行时它已提交还是仍在工作树，都必须保留并在任务二中继续使用。

## Requirement Ready Check

- Requirement source refs: 用户要求统筹 Neo 所有界面的鼠标文本选取，避免逐界面散补。
- Goals and scope refs: 已批准设计的 Goals、Non-goals 和 Acceptance Matrix。
- User / scenario refs: 正文、聊天输入框、待办、普通外框、富弹窗、任务浏览器、主题管理器、掩码字段。
- Requirement item refs: 三个选区所有者、按下时锁定手势、最终画面映射、复制与失效、旧待办路径退役。
- Acceptance / verification criteria refs: 本计划各任务的 Verification 和 Final Acceptance。
- Open blocker questions: 无。
- Decision: ready

## BaselineUsageDraft

- Required baseline refs: `AGENTS.md`、ADR-0012、2026-08-04 已落地基线、2026-08-07 已批准设计。
- Acknowledged before plan refs: 全部已查看。
- Cited in plan refs: 全部列于 Baseline/Authority Refs。
- Missing refs: 无。
- Decision: continue

## Change Necessity

- User-visible need: 鼠标被 Neo 捕获后，大量可见文本无法选取，正文手势跨区域释放和向下自动滚动也失效。
- No-change / non-code option: 使用说明、终端原生选择或逐界面登记都不能修复 Neo 已接管的事件路由和最终画面缺口。
- Why code change is necessary: 根因位于生产事件顺序、最终画面记录、选区所有权和旧待办专用路径。
- Minimum change boundary: 一个小型画面选区模块，`NeoTui` 和输入控制器接线，删除旧待办选区状态与绘制复制分支，并补定点测试。
- Decision: code-change

## Existence Check

- Proposed new surface: `FrameSelection` 和 `FrameTextMap`。
- Existing owner / reuse candidate: `NeoTui` 最终画面组合、`OverlayId`、`TranscriptPane` 单元格切片和 ANSI 高亮辅助、旧待办屏幕坐标选择、现有剪贴板写入。
- Why existing surface is insufficient: 旧待办状态只能描述待办局部行，正文与聊天输入框又有不同坐标和编辑语义。
- Creation proof: 一个与弹窗类型无关的画面模块替换旧待办所有者，并让所有最终渲染路径自动获得选区。
- Entropy / retirement impact: 删除 `TodoSelection`、冻结文本、专用绘制、专用物化和复制分支；不增加弹窗登记表。
- Decision: add-with-proof

## Architecture Integrity Lens

- Invariant: 同一时刻最多一个选区；高亮和复制必须来自同一组可见单元格；隐藏值绝不进入派生映射。
- Canonical owner: 正文为 `TranscriptPane`，聊天输入框为 `PromptState`，其余最终画面为 `NeoTui`。
- Responsibility overlap: `NeoChromeState` 不再保存待办选区；任何弹窗组件都不实现自己的文本选区。
- Higher-level simplification: 选择事件在任务浏览器和富弹窗之前进入 `NeoTui`；滚轮仍走原有路径。
- Retirement / falsifier: 生产代码仍有 `TodoSelection`，或新增 `OverlayKind` 选区匹配，均视为架构失败。
- Verdict: proceed

## Ripple Signal Triage

- Upstream: `InteractiveController::handle_input_event` 的事件优先级。
- Shared owner: `NeoTui::render_terminal_frame_at`、`NeoTui::handle_mouse_event`、`TranscriptPane::handle_mouse_event`。
- Downstream: `Ctrl+C` 退出保护、右键复制、审批与提问焦点、任务浏览器键盘和滚轮、富弹窗键盘和滚轮。
- Required expanded verification: `neo-tui` 的画面与正文目标、`neo-agent` 二进制内的选择控制器目标、冻结的 Delegate 家族回归。
- Decision: bounded expansion inside this plan

## Complexity Budget

- Artifact class: Source Complexity、Test Complexity。
- Target files / artifacts: `app.rs` 636 行、正文 `selection.rs` 873 行、交互 `input.rs` 1658 行、控制器选择测试 764 行。
- Current pressure: 三个生产文件和控制器测试已超过软阈值，不能继续承载新的纯选择算法或大量场景构造。
- Projected post-change pressure: `app.rs` 和 `input.rs` 仅增加接线并删除旧分支；新算法进入一个小模块；现有 `chrome_selection.rs` 直接升级为画面选区测试目标。
- Budget result: within-budget
- Planned governance: 不新增平行测试目标，不把画面算法塞进 `app.rs`，不移动正文选区；任务三只在控制器文件添加一个小型前置路由。

## Plan-Time Complexity Check

- Target files: `app.rs`、`transcript/selection.rs`、`input.rs`、`selection_tests.rs`。
- Existing size / shape signals: 路由、渲染和测试夹具已集中，继续内联算法会增加混合职责。
- Owner fit: `frame_selection.rs` 只持有最终画面纯状态和单元格操作；`app.rs` 保持最终画面收尾与所有者路由。
- Add-in-place risk: 在正文选区或输入控制器中实现画面选择会形成第二职责和重复所有者。
- Better file boundary: 新建一个内部模块，其他文件只接线或删除旧路径。
- Recommendation: add owner file

## Anti-Entropy Declaration

- Deletion Class: code-retirement
- Old Path/Object: `TodoSelection`、`todo_selection_text`、待办专用高亮、物化、手势和复制优先级。
- New Canonical Owner: `NeoTui` 持有的最终画面选区。
- Expected Preserved Behavior: 待办可拖选、高亮、右键复制和 `Ctrl+C` 复制。
- Expected Retired Behavior: `NeoChromeState` 保存待办选区以及 `ChromeRowKind::Todo` 的专用分派。
- External Boundary Touched: no
- Source-of-Truth Data Risk: none
- User Confirmation Required: no

## Retirement Decision

- Path: delete-first
- Why: 全部目标都是内部派生显示代码，没有外部依赖或持久化数据。
- Non-edits: 不删除任何会话、主题、任务或用户数据；不恢复不可达旧覆盖层。

## Files

### 新建

- `crates/neo-tui/src/frame_selection.rs`

### 修改

- `crates/neo-tui/src/lib.rs`
- `crates/neo-tui/src/app.rs`
- `crates/neo-tui/src/shell/state.rs`
- `crates/neo-tui/src/shell/mod.rs`
- `crates/neo-tui/src/transcript/chrome_render.rs`
- `crates/neo-tui/src/transcript/mod.rs`
- `crates/neo-tui/src/transcript/selection.rs`，仅在复用纯单元格辅助函数需要调整可见性时修改
- `crates/neo-agent/src/modes/interactive/input.rs`
- `crates/neo-agent/src/modes/interactive/prompt_edit.rs`
- `crates/neo-tui/tests/chrome_selection.rs`
- `crates/neo-tui/tests/transcript_selection.rs`
- `crates/neo-agent/src/modes/interactive/selection_tests.rs`
- `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`，仅在实现与验证完成后追加
- `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`，仅在实现与验证完成后更新
- `docs/aegis/INDEX.md`，仅追加计划和交接条目；保留已有外部改动

### 明确不修改

- `crates/neo-tui/src/transcript/child_activity.rs`
- `crates/neo-tui/src/transcript/delegate_card.rs`
- `crates/neo-tui/src/transcript/delegate_group.rs`
- `crates/neo-tui/src/transcript/swarm_card.rs`
- `crates/neo-tui/src/transcript/workflow_card.rs`
- `crates/neo-tui/src/input/raw_input.rs`
- `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- 任意模型上下文、会话、提供方、压缩或工具执行文件

## Plan Pressure Test

- Owner / retirement: 三个所有者固定；旧待办路径在任务一直接删除，不保留兼容分支。
- Architecture integrity / higher-level path: 最终画面统一收尾，弹窗无感知，不逐变体适配。
- Verification scope: 纯状态、普通画面、三类覆盖层、手势跨区、控制器、复制、失效、Unicode、掩码、旧路径清零和冻结卡片。
- Task executability: 三个实现任务严格串行，随后独立复查和原生验证。
- Pressure result: proceed

## Execution Readiness View

- Intent Lock: 覆盖所有当前可见非敏感文本，不扩展为鼠标控件操作或弹窗鼠标编辑。
- Scope Fence: 只改最终画面选区、手势路由、复制查询、旧待办路径、定点测试和落地后的现有架构记录。
- Baseline Lock: 继续使用 ADR-0012 的单一全屏正文和终端生命周期，不创建平行架构记录。
- Approved Behavior: 三个选区所有者；按下锁定手势；可见画面复制；掩码安全；选中行变化才失效。
- Owner Constraints: `TranscriptPane` 和 `PromptState` 保持现有语义；`NeoTui` 不接管正文文档坐标或输入框编辑范围。
- Compatibility Boundary: 见上文。
- Retirement Boundary: `TodoSelection` 及全部生产引用必须为零。
- Task Batches: Task 1 至 Task 5 严格串行。
- Test Obligations: 每个新过滤词先枚举后运行；每任务独立验证和提交；最终三平台证据分开报告。
- Review Gates: 每个实现任务先做设计符合性复查，再做代码质量复查；存在开放问题不得提交或进入下一任务。
- Drift / Rewind Rules: 若需要逐弹窗选区分支、第二正文选区、卡片改动、输入解析改动或上下文改动，立即停止并回到已批准设计。
- Evidence Required Before Completion: 精确自动化、旧引用负面检查、差异检查、macOS 图形终端、Fedora 原生、Windows 原生，以及未完成图形验证的明确残余风险。
- Advisory Boundary: 本视图只用于实施约束，不授予完成结论。

## Task 1：建立最终画面选区并退役待办专用路径

### Files

- Create: `crates/neo-tui/src/frame_selection.rs`
- Modify: `crates/neo-tui/src/lib.rs`
- Modify: `crates/neo-tui/src/app.rs`
- Modify: `crates/neo-tui/src/shell/state.rs`
- Modify: `crates/neo-tui/src/shell/mod.rs`
- Modify: `crates/neo-tui/src/transcript/chrome_render.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`
- Modify only for helper visibility: `crates/neo-tui/src/transcript/selection.rs`
- Test: `crates/neo-tui/tests/chrome_selection.rs`

### Why

这是所有普通界面和覆盖层漏选取的共同所有者，也是删除旧待办双轨的最小边界。

### Change Necessity

最终画面当前只返回字符串和光标，没有可选择的可见文本映射；非代码方案无法让已捕获的鼠标事件命中这些单元格。

### Repair Track

- 在新模块中实现内部 `FrameTextMap`、`FrameSelection`、画面端点、选中行快照和表面身份。
- 表面身份直接复用 `NeoChromeState::focused_overlay_id()`；无覆盖层时使用主画面身份，不新增覆盖层类型登记表。
- 普通画面把正文行标记为 `Transcript`、聊天输入正文行为 `Prompt`，其余行为 `Frame`；全屏覆盖层所有行为 `Frame`。
- 普通 `render_frame` 和 `render_terminal_frame_at` 都调用同一个最终画面收尾函数。映射在左侧留白和光标标记处理后、选区着色前生成。
- 复制从映射中的纯可见行与同一显示单元格范围物化；保留可见换行和空白行，不重建源制表符，不复制 ANSI 或终端协议字节。
- 复用现有移动阈值、长按时限、Unicode 字素单元格切片和 ANSI 背景恢复逻辑；不得复制算法。
- 将画面选区的待确认长按状态接入现有 100 毫秒帧调度；静止按住超过既有时限后必须在没有鼠标移动事件时激活，高亮不能依赖新的定时器或输入线程。
- 终端宽高、`OverlayId`、选中行内容或单元格映射变化时清除；未选行变化不得清除。
- 删除 `TodoSelection`、`todo_selection_text`、`materialize_todo_selection`、待办专用绘制与 `ChromeRowKind::Todo` 分派；待办改由 `Frame` 行自然覆盖。
- 不增加选区提示行。

### Retirement Track

- 删除旧待办状态、导出、测试访问器和复制分支，不保留别名。
- `rg` 发现任何生产 `TodoSelection` 或 `todo_selection` 引用都阻止任务提交。

### Steps

1. 记录 `TaskStartSnapshot`，确认当前工作树并保留外部 `transcript_selection.rs` 与 `INDEX.md` 改动。
2. 添加小型画面选区模块，只实现最终画面需要的状态、失效、绘制和物化。
3. 把两个 `NeoTui` 渲染入口接到同一个收尾函数，并使用现有 `OverlayId` 作为覆盖层身份。
4. 将普通外框非聊天输入行和全部全屏覆盖层行路由为画面选区。
5. 删除全部待办专用状态、绘制、物化和导出；改写 `chrome_selection.rs` 的待办测试为画面所有者测试。
6. 在同一测试目标增加一个组合场景，覆盖普通外框、独立选择器、富弹窗、任务浏览器和主题管理器的最终画面路径；不要为每个弹窗重复建测试。
7. 增加 Unicode 与 ANSI 测试，至少包含 ASCII、中文宽字符、组合附加符、表情变体选择符和零宽连接序列，断言高亮与复制范围一致。
8. 增加点击、拖动和静止长按测试，证明普通点击不留下选区、跨过既有阈值才成为拖选、静止长按通过现有帧调度激活。
9. 增加失效测试，分别证明尺寸变化、覆盖层替换、选中行变化会清除，未选行刷新不会清除。
10. 增加掩码负面测试，从真实掩码输入渲染开始拖选，断言复制值只含屏幕掩码且不含原始密钥。
11. 先确认过滤词命中，再运行精确回归：

```bash
cargo nextest list -p neo-tui --test chrome_selection frame_selection_covers_normal_and_overlay_frames
cargo nextest run -p neo-tui --test chrome_selection frame_selection_covers_normal_and_overlay_frames
cargo nextest list -p neo-tui --test chrome_selection frame_selection_preserves_unicode_and_ansi_cells
cargo nextest run -p neo-tui --test chrome_selection frame_selection_preserves_unicode_and_ansi_cells
cargo nextest list -p neo-tui --test chrome_selection frame_selection_click_drag_and_long_press_share_thresholds
cargo nextest run -p neo-tui --test chrome_selection frame_selection_click_drag_and_long_press_share_thresholds
cargo nextest list -p neo-tui --test chrome_selection frame_selection_invalidates_only_for_selected_visual_state
cargo nextest run -p neo-tui --test chrome_selection frame_selection_invalidates_only_for_selected_visual_state
cargo nextest list -p neo-tui --test chrome_selection masked_overlay_selection_exposes_only_rendered_mask
cargo nextest run -p neo-tui --test chrome_selection masked_overlay_selection_exposes_only_rendered_mask
```

12. 运行负面和机械检查：

```bash
rg -n "TodoSelection|todo_selection|materialize_todo_selection" crates/neo-tui/src crates/neo-agent/src
rustfmt --edition 2024 --check crates/neo-tui/src/frame_selection.rs crates/neo-tui/src/app.rs crates/neo-tui/src/shell/state.rs crates/neo-tui/src/shell/mod.rs crates/neo-tui/src/transcript/chrome_render.rs crates/neo-tui/src/transcript/mod.rs crates/neo-tui/src/transcript/selection.rs crates/neo-tui/tests/chrome_selection.rs
git diff --check
```

`rg` 必须无输出。完成两阶段复查后只提交本任务文件：

```text
feat(tui): select text from final frames
```

## Task 2：锁定手势所有者并修复正文与聊天输入跨区边界

### Files

- Modify: `crates/neo-tui/src/app.rs`
- Modify only if the existing public query is insufficient: `crates/neo-tui/src/transcript/pane.rs`
- Test: `crates/neo-tui/tests/transcript_selection.rs`
- Test: `crates/neo-tui/tests/chrome_selection.rs`

### Why

当前路由按指针所在行决定接收者，正文拖入外框后释放无法关闭手势，向下自动滚动条件也无法通过真实 `NeoTui` 路径触发。

### Change Necessity

只有生产路由保存按下所有者并转发后续事件，正文现有自动滚动与释放逻辑才能真正运行；在正文状态机内增加局部补丁无效。

### Repair Track

- 用一个小型活动手势枚举记录 `Transcript`、`Prompt` 或 `Frame`；按下时选定，释放后清除。
- 新按下清除另两个所有者的选区；同一手势的拖动和释放不得重新命中其他区域。
- 正文手势拖到正文下方时，把真实屏幕行继续传给 `TranscriptPane`，让 `body_row >= body_height` 的向下自动滚动生效。
- 正文在待办、聊天输入、页脚、覆盖层边缘或终端下边界释放时都必须关闭。
- 聊天输入手势拖出输入正文行时，端点夹到首尾可见字符边界；释放始终关闭手势，保留现有光标、删除和替换语义。
- 首帧之前的选择事件直接忽略，不猜测为正文。
- `Shift` 事件继续忽略；滚轮不进入手势所有者。

### Retirement Track

- 删除当前按指针区域结束 `widget_gesture` 的隐式语义。
- 不在 `TranscriptPane` 增加第二手势状态或外层自动滚动器。

### Steps

1. 记录 `TaskStartSnapshot` 并重读任务一提交后的 `NeoTui` 路由。
2. 在 `app.rs` 用按下所有者替换指针当前位置路由；`FrameSelection` 和 `PromptState` 仍分别保存自己的端点。
3. 让正文拖动和释放穿过所有外框行；只传坐标，不把外框内容加入正文复制。
4. 对聊天输入跨区拖动做首尾夹取，并确保释放关闭手势。
5. 在现有外部改动基础上增加真实 `NeoTui` 回归，不能覆盖 `active_selection_does_not_reduce_visible_body_height`。
6. 先确认过滤词命中，再运行：

```bash
cargo nextest list -p neo-tui --test transcript_selection transcript_gesture_crosses_chrome_autoscrolls_down_and_releases
cargo nextest run -p neo-tui --test transcript_selection transcript_gesture_crosses_chrome_autoscrolls_down_and_releases
cargo nextest list -p neo-tui --test chrome_selection prompt_gesture_releases_outside_prompt_without_switching_owner
cargo nextest run -p neo-tui --test chrome_selection prompt_gesture_releases_outside_prompt_without_switching_owner
cargo nextest list -p neo-tui --test chrome_selection selection_before_first_frame_is_ignored
cargo nextest run -p neo-tui --test chrome_selection selection_before_first_frame_is_ignored
cargo nextest list -p neo-tui --test chrome_selection prompt_click_places_caret_and_drag_selects_and_highlights
cargo nextest run -p neo-tui --test chrome_selection prompt_click_places_caret_and_drag_selects_and_highlights
```

7. 运行机械检查和两阶段复查后提交：

```bash
rustfmt --edition 2024 --check crates/neo-tui/src/app.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/tests/transcript_selection.rs crates/neo-tui/tests/chrome_selection.rs
git diff --check
```

```text
fix(tui): keep mouse selection gesture ownership
```

## Task 3：把选择事件放到覆盖层输入之前并统一复制

### Files

- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Modify: `crates/neo-agent/src/modes/interactive/prompt_edit.rs`
- Test: `crates/neo-agent/src/modes/interactive/selection_tests.rs`

### Why

任务浏览器和富弹窗当前先吞掉按下、拖动和释放；即使 `NeoTui` 已支持最终画面，真实控制器仍无法到达它。

### Change Necessity

事件顺序必须在共享控制器入口修复一次；逐弹窗改 `InputResult` 会继续让新界面漏适配。

### Repair Track

- 在阻塞条目、任务浏览器和富弹窗输入之前，只预处理非 `Shift` 的左键或右键选择事件。
- 滚轮不进入前置路由，继续由任务浏览器、帮助面板、选择器或普通正文现有路径处理。
- `NeoTui` 处理选择事件后立即排空 `pending_copy`；事件不得再交给覆盖层动作处理。
- 将 `Ctrl+C` 改为调用 `NeoTui` 的唯一当前选区查询：正文、画面、聊天输入。由于新按下会清除其他所有者，该顺序仅作防御。
- 保留聊天输入框右键在无选区时复制全部输入文本的既有行为；画面和正文无选区时右键不复制。
- 有任何选区时 `Ctrl+C` 不清空输入、不触发退出确认；剪贴板失败不改变已物化文本。
- 审批和提问键盘输入、任务浏览器键盘输入、富弹窗键盘输入保持原样。

### Retirement Track

- 删除 `prompt_edit.rs` 的待办复制分支。
- 改写旧的“任务浏览器吞掉鼠标”测试期望；保留其键盘和滚轮优先级断言。

### Steps

1. 记录 `TaskStartSnapshot`，确认只改控制器路由、复制查询和测试。
2. 在 `handle_input_event` 最前部增加小型选择事件前置路由；不要重排非选择事件。
3. 用 `NeoTui` 的当前选区查询替换控制器中的正文、待办、聊天输入多分支复制。
4. 把 `ctrl_c_copies_todo_selection` 改为 `ctrl_c_copies_frame_selection_from_todo_rows`，不得访问旧待办状态。
5. 改写 `selection_and_task_browser_preserve_input_priority`：任务浏览器文本可画面选取，键盘和滚轮仍归任务浏览器。
6. 增加一个富弹窗场景，证明选择事件前置、普通键盘仍进弹窗、滚轮仍按原路径。
7. 保留并重新运行审批和提问下正文选择测试。
8. 先确认过滤词命中，再运行：

```bash
cargo nextest list -p neo-agent --bin neo selection_routes_before_task_browser_and_rich_dialog_without_stealing_input
cargo nextest run -p neo-agent --bin neo selection_routes_before_task_browser_and_rich_dialog_without_stealing_input
cargo nextest list -p neo-agent --bin neo ctrl_c_copies_frame_selection_from_todo_rows
cargo nextest run -p neo-agent --bin neo ctrl_c_copies_frame_selection_from_todo_rows
cargo nextest list -p neo-agent --bin neo right_click_copies_current_frame_selection
cargo nextest run -p neo-agent --bin neo right_click_copies_current_frame_selection
cargo nextest list -p neo-agent --bin neo mouse_selection_works_while_approval_owns_keyboard
cargo nextest run -p neo-agent --bin neo mouse_selection_works_while_approval_owns_keyboard
cargo nextest list -p neo-agent --bin neo mouse_selection_works_while_question_owns_keyboard
cargo nextest run -p neo-agent --bin neo mouse_selection_works_while_question_owns_keyboard
```

9. 运行机械检查和两阶段复查后提交：

```bash
rustfmt --edition 2024 --check crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/prompt_edit.rs crates/neo-agent/src/modes/interactive/selection_tests.rs
git diff --check
```

```text
fix(tui): route selection before overlays
```

## Task 4：完成组合回归和独立审查

### Files

- Modify only for confirmed defects: Task 1 至 Task 3 已触及的文件
- Test: 现有精确目标，不新增平行测试文件

### Why

三个实现切片分别证明局部所有者；完成结论还需要证明组合路由、旧路径清零和冻结界面边界。

### Change Necessity

本任务默认不新增生产代码。只有组合回归或独立审查发现已批准范围内的真实缺陷时，才回到相应所有者最小修复并重新运行该任务全部验证。

### Steps

1. 对 Task 1 至 Task 3 的总差异做设计符合性复查，逐条核对已批准设计、三所有者、复制语义、失效规则、掩码安全和禁止事项。
2. 用新的独立复查者做代码质量复查，重点查看 Unicode 单元格边界、ANSI 重置、动态帧失效、越界坐标、剪贴板失败和跨平台整数转换。
3. 确认所有过滤词都能列出至少一个测试，再重新运行 Task 1 至 Task 3 的全部精确测试。
4. 运行现有边界回归：

```bash
cargo nextest list -p neo-tui --test transcript_selection selection_crosses_entries_autoscrolls_and_materializes_text
cargo nextest run -p neo-tui --test transcript_selection selection_crosses_entries_autoscrolls_and_materializes_text
cargo nextest list -p neo-tui --test terminal_frame blocking_overlays_render_inside_the_active_fullscreen_frame
cargo nextest run -p neo-tui --test terminal_frame blocking_overlays_render_inside_the_active_fullscreen_frame
cargo nextest list -p neo-tui --test workflow_transcript non_workflow_delegate_family_cards_remain_unchanged
cargo nextest run -p neo-tui --test workflow_transcript non_workflow_delegate_family_cards_remain_unchanged
cargo nextest list -p neo-agent --bin neo ctrl_c_prefers_prompt_selection_over_whole_text
cargo nextest run -p neo-agent --bin neo ctrl_c_prefers_prompt_selection_over_whole_text
```

5. 重新运行旧路径负面检查与机械检查：

```bash
rg -n "TodoSelection|todo_selection|materialize_todo_selection" crates/neo-tui/src crates/neo-agent/src
rustfmt --edition 2024 --check crates/neo-tui/src/frame_selection.rs crates/neo-tui/src/app.rs crates/neo-tui/src/shell/state.rs crates/neo-tui/src/shell/mod.rs crates/neo-tui/src/transcript/chrome_render.rs crates/neo-tui/src/transcript/mod.rs crates/neo-tui/src/transcript/selection.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/prompt_edit.rs crates/neo-tui/tests/chrome_selection.rs crates/neo-tui/tests/transcript_selection.rs crates/neo-agent/src/modes/interactive/selection_tests.rs
git diff --check
```

6. 如果复查或测试产生修复，只提交对应所有者的小型修复；若无需修改，不制造空提交。

## Task 5：三平台验证并同步现有 ADR 与基线

### Files

- Modify after verified implementation only: `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`
- Modify after verified implementation only: `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
- Modify: 当前工作记录和 `docs/aegis/INDEX.md`

### Why

鼠标捕获、图形终端选区和系统剪贴板具有平台差异；架构记录只能写入已经落地并有证据的状态。

### Change Necessity

产品代码在本任务默认不变。文档更新用于把已经执行的最终画面所有者、旧路径退役和原生证据同步回现有 ADR 与基线；不创建新 ADR。

### Steps

1. 在 macOS 主机重新运行 Task 1 至 Task 4 的全部精确测试和机械检查。
2. 在真实图形终端人工验证：普通外框、富弹窗、任务浏览器、主题管理器、正文拖入聊天输入后释放、正文底边自动滚动、右键、`Ctrl+C`、调整终端尺寸、`Shift` 拖选交给终端。
3. 记录图形终端名称、系统剪贴板实际内容和未通过场景；无人工操作证据不得写“图形鼠标已验证”。
4. 运行 `vm_stat` 和 `prlctl list` 检查内存与现有虚拟机状态。一次只允许一台虚拟机运行，不得擅自停止用户正在使用且不属于本验证的虚拟机。
5. 使用 Fedora 虚拟机中已有的 Neo 原生检出执行相同精确测试和真实 PTY 生命周期检查；若不存在已知检出，记录 `needs-verification`，不得临时覆盖共享目录或伪造路径。验证结束后关闭本次启动的 Fedora 虚拟机。
6. 再次检查内存与虚拟机状态，然后使用 Windows 虚拟机中已有的 Neo 原生检出执行相同精确测试和终端生命周期检查；验证结束后关闭本次启动的 Windows 虚拟机。
7. Windows Terminal 图形鼠标和系统剪贴板需要登录桌面会话。若不可用，明确列为残余风险；远程命令、合成事件和普通 PTY 不得替代该结论。
8. 按 `aegis:recording-architecture-decisions` 更新现有 ADR-0012，追加已经验证的最终画面所有者、待办路径退役、提交和证据；同步 2026-08-04 基线。不得把未执行的平台验证写成已落地事实。
9. 使用 `aegis:verification-before-completion` 汇总目标关闭证据，运行工作区结构检查并提交文档：

```bash
python /Users/chenyuanhao/.codex/aegis/scripts/aegis-workspace.py check --root /Users/chenyuanhao/Workspace/neo
git diff --check
```

建议提交：

```text
docs: record final-frame text selection
```

## Verification Plan

- Main-path check: 普通外框、独立选择器、富弹窗、任务浏览器和主题管理器均通过同一个最终画面路径高亮与复制。
- Lingering-reference check: 生产目录中 `TodoSelection`、`todo_selection` 和 `materialize_todo_selection` 为零。
- Negative check: `Shift` 拖选不进入 Neo；隐藏密钥不出现在复制文本；首帧前事件不猜测区域；未选行刷新不清除选区。
- Boundary check: 正文、聊天输入、审批、提问、任务浏览器键盘和滚轮、Delegate 家族、静态模式、鼠标解析和终端生命周期保持不变。

## Risks

- 图形终端可能对 `Shift` 拖选、右键和鼠标捕获有不同默认行为，必须与确定性测试分开报告。
- 动态覆盖层在拖动期间更新选中行会主动清除选区，这是已批准的安全行为，不得增加旧文本回退。
- 最终画面复制按可见行保留换行和装饰，不重建源文本；这是已批准语义。
- 当前共享工作树已有外部改动，任何任务都必须精确暂存，不能用回退命令清理工作树。

## Retirement

- `TodoSelection` 和全部专用状态、绘制、物化、手势与复制分支必须在 Task 1 同一提交中删除。
- 不保留别名、兼容字段、双重绘制、待办回退或逐覆盖层登记表。
- 不复活当前不可达的旧覆盖层变体；它们的单独退役不属于本任务。

## Final Acceptance

- 所有可见非敏感区域通过三个固定所有者之一可选取，高亮与复制一致。
- 正文跨区域释放和向下自动滚动通过真实 `NeoTui` 与控制器路径验证。
- 普通画面、独立选择器、富弹窗、任务浏览器和主题管理器均有代表性证明。
- `Ctrl+C`、右键、失效、Unicode、ANSI、掩码和剪贴板失败行为通过精确回归。
- 旧待办选区生产引用为零，Delegate 家族冻结测试通过。
- 每个实现任务独立复查、验证和提交，未混入用户或其他代理改动。
- macOS、Fedora、Windows 自动化和人工结果分开记录；未完成的图形验证明确列为残余风险。
- ADR-0012 和 2026-08-04 基线只在实现与证据完成后同步，不创建平行 ADR。

## Execution Route

- Decision: inline
- Evidence: 三个实现任务按共享 `NeoTui` 和控制器依赖严格串行，当前工作树还有外部重叠改动；单协调代理更容易保持提交边界和手势所有权一致。
- Fallback: 若用户在实施时明确要求分派代理，只能按任务串行派发实现与两阶段只读复查，不允许并行写同一工作树。
- User confirmation required: yes，当前用户只授权计划和交接，尚未授权产品代码实施。

本计划是实施指导，不代表产品已经修复、验证完成或可发布。
