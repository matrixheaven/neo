# 全屏转录文档后续修复实施计划

## Goal

修复全屏转录文档落地后的确定性回归：动态工具组几何失真、审批与提问不可完整操作、并行工具过早显示、鼠标选择失效、选区无视觉反馈、锁定视口没有新内容提示，以及短终端弹窗被删除顶部内容。

## Architecture

继续使用唯一的 `TranscriptStore`、`DocumentLayout`、`TranscriptPane`、`FullscreenTerminal` 和 `LiveRenderer`。修复落在现有数据、几何、输入和外层帧组合所有者中，不增加第二份转录、第二个视口、兼容渲染器或新的终端依赖。

## Tech Stack

Rust 2024、`crossterm 0.29`、Neo 自有转录组件、现有 `cargo nextest` 与精确 `cargo test` 验证路径。

## Baseline/Authority Refs

- `AGENTS.md`
- `docs/aegis/specs/2026-08-04-fullscreen-transcript-document-design.md`
- `docs/aegis/plans/2026-08-04-fullscreen-transcript-document.md`
- `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`
- `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
- 用户于 2026-08-06 提供的两张真实终端截图与完成报告

## Compatibility Boundary

- 不改变模型可见上下文、提供方请求、压缩输入、缓存前缀或会话记录顺序。
- 微压缩与 snip/dedup 继续默认关闭且互相独立。
- 不改变 Bash/Terminal 准入等待、超时、取消、长任务和输出捕获语义。
- 不改变 Workflow 执行、恢复、日志、结果或模型可见输出。
- Delegate、DelegateGroup、DelegateSwarm 卡片本体、层级、进度、展开和内容必须保持不变。
- 不恢复历史区/动态区、原生历史写入、旧 `Ctrl+O` 浏览器或内联渲染路径。
- 不以省略、截断、压缩、折叠或丢弃条目解决任何高度问题。

## TDD Route

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: diagnostic reproduction and post-change regression
- Reason: 项目未要求严格测试先行；每个根因已有明确代码证据，采用最小修复和精确回归。
- Verification: 每个任务使用一个包、一个目标选择器和精确测试过滤器；任务完成后再执行跨边界定点验证。

## Aegis Visibility

本计划涉及共享转录几何、阻塞焦点、鼠标协议和终端帧边界，必须固定唯一所有者、逐任务复查并防止用局部视觉补丁掩盖数据或输入错误。

## Plan Basis

审查基线为 `main` 的 `f4f2e19f`，审查时工作树干净，分支相对远端领先 17 个提交。执行代理必须把它视为历史快照，并在每个任务前重新读取实际 `HEAD` 与工作树。

## Requirement Ready Check

- Requirement source refs: 用户截图、用户问题清单、2026-08-04 已批准设计。
- Goals and scope refs: 本计划的 Goal 与 Compatibility Boundary。
- User / scenario refs: 动态 `Preparing Bash`、长审批、并行工具调用、普通鼠标拖动、上滚锁定、短终端弹窗。
- Requirement item refs: 下方“问题清单”和七个任务。
- Acceptance / verification criteria refs: 每个任务的 Verification 与最终验收。
- Open blocker questions: 无。并行工具行为按“首个审批处理完成后再显示下一项”解释，只改变显示顺序，不改变工具正文执行语义。
- Decision: ready

## BaselineUsageDraft

- Required baseline refs: 2026-08-04 设计、计划、ADR、已落地基线。
- Acknowledged before plan refs: 全部已查看。
- Cited in plan refs: 全部列于 Baseline/Authority Refs。
- Missing refs: 无。
- Decision: continue

## Change Necessity

- User-visible need: 当前实现会丢失空行、裁掉审批、吞掉拖动、误判鼠标移动，并静默隐藏新内容状态。
- No-change / non-code option: 文档或使用说明无法修复终端输入、布局和绘制错误。
- Why code change is necessary: 缺陷位于生产代码的修订失效、视口求解、输入队列、鼠标解析和帧组合。
- Minimum change boundary: 仅修改现有转录、输入、交互路由和外层帧所有者及其定点测试。
- Decision: code-change

## Existence Check

- Proposed new surface: 无新的转录或终端子系统。
- Existing owner / reuse candidate: `TranscriptStore`、`DocumentLayout`、`TranscriptPane`、`InteractiveController`、`NeoTui`、`FullscreenTerminal`。
- Why existing surface is insufficient: 不是所有者不足，而是现有所有者漏掉了必要的失效、焦点、绘制和提示逻辑。
- Creation proof: 仅允许增加很小的纯辅助函数或状态字段，且必须由现有所有者直接持有。
- Entropy / retirement impact: 不保留旧行为分支；错误测试语义必须删除或改写。
- Decision: reuse-existing

## Architecture Integrity Lens

- Invariant: 所有展示输入完整保留；物理帧受终端高度约束；当前可见窗口可滚动；动态内容能原位增长。
- Canonical owner: 工具组形状由 `TranscriptStore` 失效；虚拟行由 `DocumentLayout` 求解；选区由 `TranscriptPane` 绘制；终端事件由 `RawStdinEvents` 和 `parse_sgr_mouse` 解释；底部提示由外层帧组合。
- Responsibility overlap: 禁止在 `Preparing Bash`、审批卡片或 Delegate 卡片内部手工添加空行、截断或局部滚动。
- Higher-level simplification: 后续工具不需要删除或重排规范事件，只需让当前阻塞焦点限制可见窗口。
- Retirement / falsifier: 任何新增第二视口、第二选择系统、兼容鼠标解析支线或卡片级补丁都说明实现偏离。
- Verdict: proceed

## 问题清单

1. **P1：连续工具组高度没有随追加和成员更新重新计算。** `ToolRun` 组的完整绘制归在首条，但追加、普通更新、插入和删除只使局部条目失效，导致总高度、分隔空行和后续起始行过期。
2. **P1：阻塞条目只接管输入，不接管可见焦点。** 审批或提问可能位于旧锁定视口之外；长卡片的可操作选项可能落在屏幕外。
3. **P1：并行批次的后续 `Preparing` 提前显示在当前审批下方。** 规范事件在模型流阶段已经发布，审批后来插入对应工具之后，当前可见顺序因此显得像同时处理多个阻塞项。
4. **P1：鼠标释放会删除尚未派发的拖动事件。** 同一输入批次的“按下、拖动、释放”会退化为“按下、释放”，选择状态按普通点击清空。
5. **P1：SGR 编码 35 的无按键移动被误判为释放。** `EnableMouseCapture` 开启全移动上报后，普通移动可能清除已经建立的选区。
6. **P1：选区没有任何视觉高亮。** 选择端点只用于生成复制文本，`compose_rows` 从未应用 `theme.selection_bg` 或选区前景色。
7. **P1：审批和提问路由吞掉非滚轮鼠标事件。** 阻塞键盘输入拥有者会提前返回，鼠标选择无法进入转录窗格。
8. **P2：已批准的新内容提示没有产品实现。** `new_activity` 只在文档状态和测试中存在，没有生产渲染调用者。
9. **P2：复制能力缺少可见反馈。** `Ctrl+C` 和命令面板已有复制动作，但选区不可见，也没有“已选择、可复制”的屏幕提示。
10. **P2：短终端富弹窗通过删除顶部行来适配高度。** `fit_chrome_to_height` 会直接删除顶部内容，标题、说明或选项可能消失。
11. **残余兼容风险：** 旧式非 SGR 鼠标序列没有转换路径；Shift 绕过依赖终端在上报前自行处理。先完成现代 SGR 主路径与真实终端验收，只有目标终端确实失败时才扩展协议支持。

## Files

主要修改候选：

- `crates/neo-tui/src/transcript/store.rs`
- `crates/neo-tui/src/transcript/document.rs`
- `crates/neo-tui/src/transcript/pane.rs`
- `crates/neo-tui/src/transcript/selection.rs`
- `crates/neo-tui/src/input/raw_input.rs`
- `crates/neo-tui/src/app.rs`
- `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`
- `crates/neo-tui/src/dialogs/help_panel.rs`
- `crates/neo-tui/src/dialogs/confirm_dialog.rs`

主要测试候选：

- `crates/neo-tui/tests/fullscreen_transcript.rs`
- `crates/neo-tui/tests/transcript_selection.rs`
- `crates/neo-tui/tests/terminal_frame.rs`
- `crates/neo-agent/src/modes/interactive/selection_tests.rs`
- `crates/neo-agent/src/modes/interactive/terminal_io.rs` 内部测试
- `crates/neo-tui/src/input/raw_input.rs` 内部测试

只有证据表明这些文件不足时才能扩大范围。

## Plan Pressure Test

- Owner / retirement: 所有修复都进入现有所有者；错误测试语义替换，不保留兼容分支。
- Architecture integrity: 不修改卡片本体，不改变工具执行，只修复外层文档、输入和帧。
- Verification scope: 每个任务有精确回归，最终有组合序列和真实图形终端验收。
- Task executability: 七个任务按共享所有者依赖顺序串行执行。
- Pressure result: proceed

## Execution Readiness View

- Intent Lock: 修复可见性、选择和高度回归，不重新设计全屏架构。
- Scope Fence: 仅限问题清单对应的生产路径、定点测试和最后的基线同步。
- Baseline Lock: 2026-08-04 设计、ADR 和已落地基线继续有效。
- Approved Behavior: 底部自动跟随；上滚锁定；新内容提示；普通拖动选择；阻塞项保持可操作；所有卡片信息完整。
- Owner Constraints: 卡片本体不是修复所有者；`TranscriptStore`、`DocumentLayout`、`TranscriptPane` 和输入链各自只处理自己的职责。
- Compatibility Boundary: 见上文，不改变上下文、工具执行、Workflow 或 Delegate 家族。
- Retirement Boundary: 删除错误测试期望，不增加旧路径或回退。
- Task Batches: Task 1 至 Task 7 严格串行。
- Test Obligations: 每个任务一组定点回归；最终组合验收；真实 macOS 图形终端鼠标和剪贴板人工验证。
- Review Gates: 每任务实现后先设计符合性复查，再代码质量复查；两轮无开放问题后协调代理才能验证和提交。
- Drift / Rewind Rules: 出现卡片内部省略、第二视口、执行调度变化、上下文变化或无关文件修改时立即停止并回到本计划。

## Task 1：修复连续工具组的统一失效和几何

### Files

- Modify: `crates/neo-tui/src/transcript/store.rs`
- Modify only if geometry assertions require it: `crates/neo-tui/src/transcript/document.rs`
- Test: `crates/neo-tui/tests/fullscreen_transcript.rs`

### Why

这是截图中间距缺失和部分裁切的共同根因。给 `Preparing Bash` 单独添加空行会留下所有其他工具状态和组变化仍然错误。

### Repair Track

- 让连续 `ToolRun` 组的任何形状变化或成员变化都使组首条重新测高。
- 统一覆盖追加工具、更新任意成员、在工具之间插入审批、移除条目、隐藏和恢复隐藏。
- 优先在 `append_entry`、`insert_entry`、`remove`、`mutate_entry` 或一个被这些入口共同调用的私有辅助函数中解决。
- `touch_tool_run_span` 是现有复用候选，不创建第二套组查找。
- 分隔行仍由文档层统一拥有；禁止在卡片渲染器中添加空白行。

### Retirement Track

- 改写只接受首次绘制前一次性构造工具组的测试假设。
- 不保留“普通更新只失效单个成员”的旧语义。

### Steps

1. 在 `fullscreen_transcript.rs` 增加动态序列：先绘制一个工具，再追加第二个 `Preparing` 工具，再更新第二个成员，再在两者之间插入审批或状态边界。
2. 断言 `document.total_rows()` 等于完整组合行数，组前后恰好一行空白，后续条目的 `start_row` 按组高度差移动，尾部仍可滚动到达。
3. 修改 `TranscriptStore` 的共同变更入口，使连续工具组首条在所有组变化后修订并清除缓存。
4. 运行：

```bash
cargo nextest run -p neo-tui --test fullscreen_transcript dynamic_tool_group_remeasures_after_append_and_member_update
```

5. 运行 `git diff --check`，完成两阶段复查后由协调代理提交：

```text
fix(tui): remeasure dynamic tool groups
```

## Task 2：修复鼠标事件解析与队列顺序

### Files

- Modify: `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- Modify: `crates/neo-tui/src/input/raw_input.rs`
- Test in the same files

### Why

当前真实拖动在进入选择状态机前已经丢失或被误判；直接修复选择组件无法解决问题。

### Repair Track

- 非运动事件不得无条件删除拖动。释放到来时必须保留该次手势最后一个拖动，然后再派发释放。
- 键盘输入和无关阻塞事件仍可清理陈旧滚轮，但不得破坏同一鼠标手势。
- `parse_sgr_mouse` 必须先识别移动位，再处理无按键移动；编码 35 应得到不触发释放的移动种类，或被安全忽略，绝不能得到 `Release`。
- 不引入第二个 crossterm 事件读取器。

### Retirement Track

- 删除或改写当前把“释放删除最后拖动”固定为正确行为的测试。
- 不保留旧误判作为回退。

### Steps

1. 增加一个原始字节批次包含按下、多个拖动、释放的回归，断言派发顺序至少保留按下、最后拖动、释放。
2. 增加 SGR `35M` 回归，断言它不是释放。
3. 用最小队列规则修复手势内顺序，并修正 SGR 分类顺序。
4. 运行：

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::terminal_io::tests::drag_release_preserves_last_motion_in_same_batch --exact --nocapture --include-ignored
cargo nextest run -p neo-tui --lib sgr_mouse_no_button_motion_is_not_release
```

5. 运行 `git diff --check`，完成两阶段复查后提交：

```text
fix(tui): preserve mouse drag gestures
```

## Task 3：绘制选区并让阻塞状态下仍可选择和复制

### Files

- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Modify only if a small style helper is required: `crates/neo-tui/src/transcript/selection.rs`
- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Modify: `crates/neo-tui/src/app.rs`
- Test: `crates/neo-tui/tests/transcript_selection.rs`
- Test: `crates/neo-agent/src/modes/interactive/selection_tests.rs`

### Why

即使事件链偶尔成功，用户也看不到选区；审批和提问还会吞掉鼠标。复制动作虽存在，但没有可见状态。

### Repair Track

- 在 `compose_rows` 输出最终可见行之前，根据文档坐标把选区样式应用到相交单元格。
- 复用 `TuiTheme` 的选择颜色；必须保留原 ANSI 样式、宽字符和跨条目选择。
- 空白分隔行可以被选择为换行，但不需要绘制整行背景。
- 审批和提问只独占键盘选择与提交；普通左键选择事件应继续进入 `NeoTui::handle_mouse_event`，滚轮保持现有导航语义。
- 选区活动时，在外层帧显示简短可见提示，例如 `selected · ctrl+c copy · ctrl+shift+space clear`；提示不得覆盖审批或正文。
- 松开鼠标不自动复制，继续由现有 `Ctrl+C` 和系统剪贴板路径负责，避免意外覆盖剪贴板。

### Retirement Track

- 不增加第二套选择状态或终端原生选择模拟。
- 不把 Shift 绕过宣传为所有终端必然可用；保留“受终端支持时”的边界。

### Steps

1. 增加同一行、跨行、跨条目和宽字符选区的帧断言，检查选区 ANSI 背景范围。
2. 增加审批与提问待处理时的鼠标拖动路由测试，断言转录选区建立，审批键盘选择不变。
3. 在 `TranscriptPane` 最终行组合阶段应用高亮，并在外层帧增加选区提示。
4. 调整阻塞输入路由，只放行选择鼠标事件，不放行审批键盘事件。
5. 运行：

```bash
cargo nextest run -p neo-tui --test transcript_selection rendered_selection_highlights_exact_document_cells
cargo test --package neo-agent --bin neo -- modes::interactive::selection_tests::mouse_selection_works_while_approval_owns_keyboard --exact --nocapture --include-ignored
```

6. 运行 `git diff --check`，完成两阶段复查后提交：

```text
fix(tui): render and route transcript selection
```

## Task 4：让最早阻塞条目拥有可见焦点并顺序显露后续工具

### Files

- Modify: `crates/neo-tui/src/transcript/document.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Modify only if event order assertions require it: `crates/neo-tui/src/transcript/approval_data.rs`
- Test: `crates/neo-tui/tests/fullscreen_transcript.rs`
- Test: `crates/neo-tui/tests/progressive_transcript.rs`
- Test: `crates/neo-tui/tests/workflow_transcript.rs`

### Why

输入焦点和可见焦点必须一致。当前审批位于屏幕外时，用户仍被迫回答它；后续 `Preparing` 还会出现在审批下方。

### Repair Track

- `TranscriptPane` 每帧从规范条目中派生最早未解决审批或提问，不新增第二份阻塞队列。
- 存在阻塞条目时，可见窗口的下边界不得超过该条目末尾；后续条目完整保留在文档中，但暂不进入当前可见窗口。
- 默认把阻塞卡片的操作区和当前选项放进视口。卡片高于视口时允许在该卡片范围内滚动，不能截短卡片。
- 阻塞条目解决后，立即显露下一最早阻塞条目；全部解决后恢复普通文档跟随或锁定行为。
- 第一项审批待处理时，第二个并行 `Preparing` 不显示；第一项处理后再显示。不得删除、重排或延迟规范事件持久化。
- 不改变 `authorize_tool_batch` 的整批审批和工具正文执行语义。本计划中的“一个处理完”指当前审批处理完成，不指工具正文执行完成。

### Retirement Track

- 不恢复审批弹窗覆盖层。
- 不在工具卡片内增加“隐藏后续工具”标志。

### Steps

1. 增加“用户上滚后到达长审批”的回归，断言当前选项和操作提示可见。
2. 增加“审批卡片高于视口”的回归，断言默认看到操作区，向上滚动可到达标题和完整命令，向下可回到操作区。
3. 增加“两个并行工具开始事件，第一项审批待处理”的回归，断言第二个 `Preparing` 在规范存储中存在但当前帧不可见；第一项解决后它出现。
4. 增加 QuestionPrompt 对称回归。
5. 在现有文档求解路径加入阻塞焦点范围，不建立第二视口所有者。
6. 运行：

```bash
cargo nextest run -p neo-tui --test fullscreen_transcript blocking_entry_keeps_action_area_visible_and_defers_later_tools
cargo nextest run -p neo-tui --test progressive_transcript long_approval_scrolls_without_truncation
cargo nextest run -p neo-tui --test workflow_transcript workflow_approval_focus_defers_later_activity
```

7. 运行 `git diff --check`，完成两阶段复查后提交：

```text
fix(tui): keep blocking transcript entries visible
```

## Task 5：实现锁定视口的新内容提示

### Files

- Modify: `crates/neo-tui/src/app.rs`
- Modify only for read access: `crates/neo-tui/src/transcript/pane.rs`
- Test: `crates/neo-tui/tests/fullscreen_transcript.rs`
- Test: `crates/neo-tui/tests/terminal_frame.rs`

### Why

这是已批准行为，但当前只有无人读取的布尔状态，用户在上滚后无法知道后台已有更新。

### Repair Track

- 不使用 `consume_new_activity` 在单帧后清除提示。
- 只要视口锁定且有新活动，底部固定区域显示一行短提示，例如 `new activity · end to follow`。
- 回到底部后提示消失。
- 提示高度必须计入正文可用高度，不能覆盖正文、Todo、输入框或底栏。
- 窄终端使用更短文本，不做水平截断后仍不可读的长句。

### Retirement Track

- 删除无生产用途的消费接口，或把它改为实际需要的只读接口；不要保留两种提示清除语义。

### Steps

1. 增加上滚、追加、持续更新、回到底部的帧回归。
2. 在外层帧组合中读取文档状态并渲染单一提示行。
3. 断言帧总高不超过终端高度，提示存在时正文高度正确减少一行。
4. 运行：

```bash
cargo nextest run -p neo-tui --test terminal_frame locked_transcript_shows_new_activity_until_following_tail
cargo nextest run -p neo-tui --test fullscreen_transcript locked_view_keeps_anchor_and_exposes_activity_notice
```

5. 运行 `git diff --check`，完成两阶段复查后提交：

```text
fix(tui): show locked transcript activity
```

## Task 6：修复短终端富弹窗顶部内容丢失

### Files

- Modify: `crates/neo-tui/src/app.rs`
- Modify as required: `crates/neo-tui/src/dialogs/help_panel.rs`
- Modify as required: `crates/neo-tui/src/dialogs/confirm_dialog.rs`
- Test: `crates/neo-tui/tests/terminal_frame.rs`

### Why

当前 `fit_chrome_to_height` 从顶部删除行，满足了帧高度断言，却会丢失标题、说明或当前操作。

### Repair Track

- 富弹窗必须根据实际可用高度自行生成可滚动或受控切片。
- 当前选项、标题和必要操作提示必须同时可达。
- `fit_chrome_to_height` 不得继续作为静默删除顶部内容的正常路径；仅允许防御性保护，并在测试中证明所有生产弹窗已先适配高度。
- Task Browser 和主题管理器已有高度感知逻辑，不重复实现。

### Retirement Track

- 删除“非空且不超高就算通过”的弱测试。
- 不用省略弹窗正文替代滚动。

### Steps

1. 增加 8 行和 5 行终端的帮助、确认及至少一个富选择弹窗测试。
2. 断言标题、当前选项和操作提示可见或可通过现有滚动到达，且帧不超高。
3. 把实际高度传入需要它的弹窗渲染器，移除正常路径的顶部删除。
4. 运行：

```bash
cargo nextest run -p neo-tui --test terminal_frame short_terminal_preserves_dialog_title_selection_and_actions
```

5. 运行 `git diff --check`，完成两阶段复查后提交：

```text
fix(tui): preserve dialogs on short terminals
```

## Task 7：组合回归、真实终端验收和基线同步

### Files

- Test-only changes only when a real uncovered case is found
- Modify after all evidence passes: `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
- Amend after all evidence passes: `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`

### Why

局部测试无法证明真实鼠标、剪贴板、长审批、动态组高度和底部固定区域组合后仍正确。

### Steps

1. 运行所有任务的精确回归，不扩大成包级测试。
2. 运行格式和差异检查：

```bash
cargo fmt --all --check
git diff --check
```

3. 在 macOS 图形终端人工验证：
   - 普通左键拖动实时高亮；跨卡片和跨屏自动滚动；松开后高亮保留。
   - `Ctrl+C` 复制选区，系统剪贴板内容正确；复制失败不清除选区。
   - 鼠标移动不会清除选区；单击不会误建立选区。
   - 上滚后新增内容不拉到底部，并持续显示新内容提示；回到底部清除提示。
   - 两个并行 Bash 调用中，第一项审批待处理时第二个 `Preparing` 不出现在其下方；处理后按顺序出现。
   - 长命令审批可滚动查看完整命令和操作区，无省略、无屏幕外不可达内容。
   - 动态工具组在 `Preparing`、`Using`、`Used` 变化间始终保留一行外层间距。
4. 若本机可用，再在 Fedora 图形终端或真实 PTY 验证鼠标序列和 resize；Windows 必须在登录桌面会话中验证 Windows Terminal 与系统剪贴板，ssh 结果不能替代。
5. 只有全部证据通过后，向 ADR-0012 和已落地基线追加本次修复与真实验证，不重写历史。
6. 完成最终全差异复查，重点确认没有改变 Delegate 家族、Workflow 执行、上下文或工具执行语义。
7. 完成两阶段复查后提交：

```text
docs: record fullscreen transcript follow-up fixes
```

## Subagent-Driven Execution

每个任务严格执行以下循环，禁止多个实现代理同时写代码：

```text
协调代理记录 TaskStartSnapshot
  -> 新实现代理只处理当前任务
  -> 设计符合性复查代理
  -> 有问题则原实现代理修复并重新复查
  -> 代码质量复查代理
  -> 有问题则原实现代理修复并重新复查
  -> 协调代理重新运行精确验证
  -> 协调代理只暂存当前任务文件并提交
  -> 读取提交文件列表、剩余差异和新 HEAD
  -> 更新检查点后进入下一任务
```

实现代理和复查代理不得执行 `git add`、`git commit`、切分支、创建工作树、推送或发布。协调代理是唯一 Git 写入所有者。

## Risks

- 选区高亮需要在保留 ANSI 样式的同时按显示单元格切分，宽字符边界是主要正确性风险。
- 阻塞焦点与用户锁定位置存在交互风险；实现必须让阻塞操作可达，同时不删除后续条目。
- 工具组失效若放在错误层级，会造成每帧全量重绘；只在组形状或成员修订变化时触发。
- 短终端弹窗修复可能影响多个弹窗；只修改真实超高路径，不重做所有弹窗布局。
- 真实图形终端鼠标和剪贴板仍是自动化无法完全覆盖的最终风险。

## Retirement

- 删除错误的“释放可以删除最后拖动”测试语义。
- 删除或收窄静默删除弹窗顶部行的正常路径。
- 删除无人使用的新活动消费接口或赋予唯一明确用途。
- 不新增任何旧行为兼容分支、第二视口、第二选择系统或卡片级高度补丁。

## Final Acceptance

只有同时满足以下条件才能声称完成：

- 问题清单 1 至 10 均有对应代码修复和定点证据。
- 所有动态工具组的虚拟高度与实际组合行数一致。
- 最早阻塞条目及其操作区始终可达，后续 `Preparing` 按审批顺序显露。
- 普通鼠标拖动在真实图形终端可见、可复制且不会被移动或释放破坏。
- 上滚锁定时新内容提示真实可见，回到底部后消失。
- 短终端弹窗不再静默丢失顶部内容。
- Delegate 家族卡片、Workflow 执行、工具运行语义、会话记录和模型上下文没有变化。
- 每任务两阶段复查均无开放问题，每任务一个独立提交，未推送。
