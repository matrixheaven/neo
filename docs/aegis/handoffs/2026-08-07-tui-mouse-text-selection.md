# 交接提示：Neo TUI 全界面鼠标文本选取

把分隔线下方的全部内容原样交给负责实施的代码代理。

---

你是 Neo TUI 全界面鼠标文本选取的协调实施代理。工作目录：

```text
/Users/chenyuanhao/Workspace/neo
```

## 一、授权边界

当前已批准的是书面设计、实施计划和本交接文档。若用户只是让你查看、审查或继续规划，禁止修改产品代码。只有用户把本交接交给你并明确要求“实施、执行、修复”时，才可进入产品实现。

实施依据：

```text
docs/aegis/specs/2026-08-07-tui-mouse-text-selection-design.md
docs/aegis/plans/2026-08-07-tui-mouse-text-selection.md
docs/aegis/handoffs/2026-08-07-tui-mouse-text-selection.md
```

设计已经批准。不要重新做全仓探索，不要重新比较架构，不要逐个盘点 23 个覆盖层，也不要把任务改写成另一套产品方案。只有当前源码与计划出现决定性冲突时，才允许做最小补证并暂停报告。

## 二、开始前必须读取

按顺序完整查看：

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-08-07-tui-mouse-text-selection-design.md`
5. `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`
6. `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
7. `docs/aegis/plans/2026-08-07-tui-mouse-text-selection.md`
8. 本交接文件
9. `docs/aegis/work/2026-08-07-tui-mouse-text-selection/10-intent.md`
10. `docs/aegis/work/2026-08-07-tui-mouse-text-selection/20-checkpoint.md`

然后运行：

```bash
icm recall-context "TUI 鼠标文本选取 TranscriptPane PromptState NeoTui TodoSelection 覆盖层" --limit 5
git status --short --branch
git log -5 --oneline
```

规划开始时的历史快照：

```text
branch: main
HEAD: 4c722d95
remote relation: ahead 8
initial external worktree changes:
  crates/neo-tui/tests/transcript_selection.rs
  docs/aegis/INDEX.md
```

规划期间共享工作树继续发生变化，因此这只是历史证据，不是执行基线。以执行时实际状态为准，保留用户与其他代理的全部改动。`transcript_selection.rs` 的并行改动修正了“选区提示行占用正文高度”的错误测试；无论它已提交还是仍在工作树，都不得覆盖或删除。

## 三、已达成的唯一实现共识

只有三个互不重叠的选区所有者：

```text
TranscriptPane -> 正文文档坐标、跨条目、双击、长按、自动滚动
PromptState    -> 聊天输入框字符坐标、光标、删除、替换
NeoTui         -> 其余最终可见画面的屏幕坐标
```

按下时确定所有者，拖动和释放始终交回同一所有者。普通界面、独立选择器、富弹窗、任务浏览器和主题管理器统一从最终画面生成文本映射，不维护覆盖层登记表。

最终画面映射必须在加入左侧留白、提取硬件光标标记之后生成。只复制当前可见单元格。掩码字段只能复制掩码；隐藏原值不得进入映射。终端尺寸、`OverlayId`、选中行内容或单元格映射变化时清除画面选区，未选行刷新不得清除。

覆盖层输入字段只支持选取和复制可见文字，不支持鼠标编辑。滚轮保持原有分发。`Shift` 拖选继续交给终端。

## 四、已确认根因，不要重复诊断

1. `InteractiveController::handle_input_event` 先把事件交给任务浏览器和富弹窗，它们会吞掉未处理的左键选择事件。
2. `NeoTui::handle_mouse_event` 对大多数阻断覆盖层提前返回。
3. `NeoTui::render_terminal_frame_at` 的全屏覆盖层分支提前返回，没有记录最终画面布局。
4. `ChromeRowKind::Other` 当前不可选。
5. 路由按指针当前行选择接收者；正文拖到外框后释放无法回到 `TranscriptPane`。
6. 同一原因使正文向下拖出可见区时无法触发 `body_row >= body_height` 自动滚动。
7. `TodoSelection` 是只覆盖一个面板的重复屏幕坐标所有者，继续保留会形成双轨。

修复必须落在共享最终画面和共享事件入口。若某个弹窗仍需要选区专用分支，说明实现偏离，必须停止而不是加例外。

## 五、绝对禁止事项

- 禁止修改 Delegate、DelegateGroup、DelegateSwarm、Workflow 卡片本体、层级、进度、展开语义和正文条目布局。
- 禁止修改工具执行、审批顺序、Workflow 运行、会话持久化、模型上下文、提供方请求、压缩输入或缓存前缀。
- 禁止修改 `parse_sgr_mouse`、鼠标队列合并、输入读取器、备用屏幕、鼠标捕获或终端恢复生命周期。
- 禁止新增第二正文选区、第二视口、第二渲染器、兼容分支、回退路径或覆盖层选区登记表。
- 禁止新增按钮点击、列表点击、页签点击、弹窗鼠标编辑、隐藏内容复制或覆盖层自动滚动。
- 禁止增加“已选择、复制”提示行。
- 禁止保留 `TodoSelection` 别名、兼容字段、双重绘制或回退。
- 禁止创建新 ADR；最终只更新现有 ADR-0012 和对应基线。
- 禁止修复无关失败或格式化无关文件。
- 禁止 `git reset`、`git checkout --`、`git restore`、`git stash`、`git clean`、`git rebase`、`git rm`、amend、强制推送、切分支和工作树增删。
- 未授权推送、发布、打标签、创建分支或创建工作树。

## 六、固定文件边界

允许新建：

```text
crates/neo-tui/src/frame_selection.rs
```

允许按任务修改：

```text
crates/neo-tui/src/lib.rs
crates/neo-tui/src/app.rs
crates/neo-tui/src/shell/state.rs
crates/neo-tui/src/shell/mod.rs
crates/neo-tui/src/transcript/chrome_render.rs
crates/neo-tui/src/transcript/mod.rs
crates/neo-tui/src/transcript/selection.rs
crates/neo-tui/src/transcript/pane.rs
crates/neo-agent/src/modes/interactive/input.rs
crates/neo-agent/src/modes/interactive/prompt_edit.rs
crates/neo-tui/tests/chrome_selection.rs
crates/neo-tui/tests/transcript_selection.rs
crates/neo-agent/src/modes/interactive/selection_tests.rs
docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md
docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md
docs/aegis/INDEX.md
docs/aegis/work/2026-08-07-tui-mouse-text-selection/
```

`transcript/selection.rs` 只允许调整现有纯单元格辅助函数的可见性或消除真正重复；不得把正文状态迁入新模块。`transcript/pane.rs` 只在现有公开查询不足时修改。超出文件边界先返回 `BLOCKED`，说明决定性原因，不得自行扩大范围。

## 七、固定实施顺序

严格执行计划中的五个任务：

1. 建立最终画面选区，并在同一提交退役待办专用路径。
2. 锁定按下时的手势所有者，修复正文向下自动滚动、跨区域释放和聊天输入跨区释放。
3. 在控制器中把选择事件放到任务浏览器与富弹窗之前，统一 `Ctrl+C` 和右键复制。
4. 完成组合回归和两阶段独立审查。
5. 完成 macOS、Fedora、Windows 分层验证，只在证据完成后更新 ADR-0012 与基线。

不得调换 Task 1 至 Task 3。Task 2 依赖 Task 1 的画面所有者，Task 3 依赖前两项已在 `NeoTui` 内正确处理。

每个实现任务一个提交：

```text
Task 1: feat(tui): select text from final frames
Task 2: fix(tui): keep mouse selection gesture ownership
Task 3: fix(tui): route selection before overlays
Task 4: 只有确认缺陷并修复时才提交，不制造空提交
Task 5: docs: record final-frame text selection
```

## 八、最小实现形状

`frame_selection.rs` 只负责：

- 最终画面行、纯可见行、行所有者和 `OverlayId` 身份；
- 画面端点、按下时间、拖动状态、物化文本和选中行快照；
- Unicode 显示单元格范围、ANSI 选择背景、复制和失效。
- 静止长按通过现有 100 毫秒帧调度激活，不增加定时器或输入线程。

`app.rs` 只负责：

- 普通画面与全屏覆盖层调用同一收尾函数；
- 把正文、聊天输入正文和其他画面行分给三个所有者；
- 保存活动手势所有者并转发拖动和释放；
- 暴露唯一当前选区文本查询与右键待复制文本。

`input.rs` 只负责：

- 非 `Shift` 的左右键选择事件在阻塞条目、任务浏览器和富弹窗之前进入 `NeoTui`；
- 滚轮与其他键盘事件保持原顺序；
- 排空现有 `pending_copy`。

这三个边界之外的算法或分支一律拒绝。

## 九、每任务执行协议

每个任务开始前记录：

```text
TaskStartSnapshot
- 当前分支和 HEAD
- 现有修改与未跟踪文件
- 当前任务允许修改的路径
- 当前任务基线 SHA
- 与外部改动重叠时的保留方式
```

执行流程：

```text
按计划完成最小实现
  -> 先用 cargo nextest list 确认每个过滤词至少命中一项
  -> 运行同过滤词的 cargo nextest run
  -> 设计符合性独立复查
  -> 有问题则在原所有者最小修复并重跑
  -> 代码质量独立复查
  -> 有问题则在原所有者最小修复并重跑
  -> rustfmt 精确文件检查
  -> git diff --check
  -> 只暂存本任务文件
  -> 提交
  -> git show --stat --oneline HEAD
  -> git status --short --branch
  -> 更新检查点
```

复查者不得写文件或执行 Git 写入。任何开放问题未关闭时禁止进入下一任务。

## 十、测试硬要求

必须直接使用实施计划中的完整命令，不得改成包级、工作区级或模糊过滤。每条 Rust 命令必须包含：

```text
一个包
一个目标选择器
一个测试过滤词
```

核心新回归过滤词：

```text
frame_selection_covers_normal_and_overlay_frames
frame_selection_preserves_unicode_and_ansi_cells
frame_selection_click_drag_and_long_press_share_thresholds
frame_selection_invalidates_only_for_selected_visual_state
masked_overlay_selection_exposes_only_rendered_mask
transcript_gesture_crosses_chrome_autoscrolls_down_and_releases
prompt_gesture_releases_outside_prompt_without_switching_owner
selection_before_first_frame_is_ignored
selection_routes_before_task_browser_and_rich_dialog_without_stealing_input
ctrl_c_copies_frame_selection_from_todo_rows
right_click_copies_current_frame_selection
```

现有边界回归：

```text
prompt_click_places_caret_and_drag_selects_and_highlights
selection_crosses_entries_autoscrolls_and_materializes_text
blocking_overlays_render_inside_the_active_fullscreen_frame
mouse_selection_works_while_approval_owns_keyboard
mouse_selection_works_while_question_owns_keyboard
ctrl_c_prefers_prompt_selection_over_whole_text
non_workflow_delegate_family_cards_remain_unchanged
```

必须执行旧路径负面检查：

```bash
rg -n "TodoSelection|todo_selection|materialize_todo_selection" crates/neo-tui/src crates/neo-agent/src
```

成功结果是无输出。测试文件中可以保留描述退役行为的测试名，但生产代码不得有引用。

## 十一、Git 边界

共享工作树可能包含用户和其他代理改动。严禁为获得干净状态而回退、隐藏或删除它们。

- 每个任务只暂存计划列出的本任务文件。
- 若同一文件已有外部改动，逐段核对并保留；不能整文件覆盖。
- `docs/aegis/INDEX.md` 已有任务浏览器条目外部改动，只能追加本任务条目，提交时必须精确分离暂存。
- 提交前后都运行：

```bash
git diff --cached --check
git diff --cached --name-only
git show --stat --oneline HEAD
git status --short --branch
```

如果无法把同文件中的提交边界可靠分开，停止并报告，不得把外部改动混入提交。

## 十二、原生验证

先完成 macOS 主机精确自动化，再做真实图形终端人工操作。人工检查必须包含：

- 普通外框、富弹窗、任务浏览器和主题管理器拖选；
- 正文拖入聊天输入或页脚后释放；
- 正文底边向下自动滚动；
- 右键、`Ctrl+C` 和系统剪贴板实际内容；
- 调整终端尺寸后的失效；
- `Shift` 拖选仍由终端处理。

虚拟机规则：

1. 先运行 `vm_stat` 和 `prlctl list`。
2. 一次只运行一台虚拟机。
3. 不擅自停止用户正在使用且不属于本验证的虚拟机。
4. Fedora 和 Windows 都使用虚拟机中已有的 Neo 原生检出；不存在时报告 `needs-verification`，不临时覆盖共享目录。
5. 每台虚拟机只运行计划列出的精确测试和生命周期检查。
6. 验证结束后关闭本次启动的虚拟机，再启动下一台。

Windows Terminal 图形鼠标与系统剪贴板需要登录桌面。远程命令、合成鼠标和普通 PTY 只能证明自动化路径，不能写成图形终端已验证。

## 十三、架构记录

不要在实现前修改 ADR 或基线。只有 Task 1 至 Task 4 完成、精确证据齐全后，才按 `aegis:recording-architecture-decisions`：

- 追加 ADR-0012，记录最终画面选区所有者、旧待办路径退役、提交和证据；
- 更新 `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`；
- 区分 macOS、Fedora、Windows 自动化、图形终端人工结果和残余风险；
- 不创建平行 ADR，不把未执行验证写成已落地事实。

## 十四、立即停止条件

出现以下任一情况，返回 `BLOCKED` 或 `needs-verification`，不得自行扩大：

- 必须修改卡片本体、工具执行、上下文、会话、鼠标解析、输入队列或终端生命周期才能继续；
- 需要逐个 `OverlayKind` 增加选区分支；
- 需要保留 `TodoSelection` 兼容路径；
- 最终画面复制会接触弹窗原始密钥 getter；
- 高亮范围与物化范围无法共用同一单元格映射；
- 当前工作树外部改动无法安全保留或精确暂存；
- 同一虚拟机环境无法满足一次只运行一台的要求；
- 新过滤词无法枚举出测试；
- 独立复查仍有开放问题。

## 十五、完成报告

完成前使用 `aegis:verification-before-completion`。最终报告必须分别列出：

- 三个实现提交和每个提交的文件边界；
- 每个精确测试的命中与通过结果；
- 旧待办生产引用无输出；
- macOS 自动化与图形终端人工结果；
- Fedora 原生与真实 PTY 结果；
- Windows 原生自动化结果；
- Windows Terminal 登录桌面图形结果，或明确未执行原因；
- 工作树剩余外部改动；
- 最终残余风险。

不得用“本地测试通过”替代远端持续集成，也不得用 PTY 或合成事件替代图形终端鼠标与系统剪贴板。没有全部证据时，结论只能是部分完成或需要验证。本交接不授权推送、发布、标签或分支操作。
