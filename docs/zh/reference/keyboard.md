# 键盘快捷键参考

Neo TUI 的快捷键由 `KeybindingsManager` 统一管理，支持用户在配置里覆盖默认绑定。本文列出默认快捷键。

源码位置：[`crates/neo-tui/src/input/keybinding.rs`](../../../crates/neo-tui/src/input/keybinding.rs)（`default_keybinding_definitions`）与 [`crates/neo-agent/src/modes/interactive/input.rs`](../../../crates/neo-agent/src/modes/interactive/input.rs)（事件分发）。

每个动作对应一个稳定的配置 ID（形如 `tui.editor.cursorUp`），可在用户配置里通过该 ID 重绑按键。

## 通用

| 快捷键 | 动作 | 说明 |
| --- | --- | --- |
| `Enter` | `InputSubmit` | 提交当前 prompt。 |
| `Tab` | `InputTab` | 触发自动补全。 |
| `Ctrl+C` | `AppClear` / `InputCopy` | 编辑器有选区时复制；否则清空编辑器 / 中断 turn / 拒绝审批。 |
| `Ctrl+D` | `AppExit` | prompt 为空时退出；500 ms 内再按一次确认退出。 |
| `Ctrl+Z` | `AppSuspend` | 将 Neo 挂起到 shell 后台（`fg` 恢复）。 |
| `Esc` | `SelectCancel` | 关闭弹层 / 取消选择。 |

## 模式切换

| 快捷键 | 动作 | 说明 |
| --- | --- | --- |
| `Shift+Tab` | `CycleDevelopmentMode` | 在 normal → plan → goal 模式间循环切换。 |
| `Ctrl+P` | `PromptCompletionToggle` | 打开 `/` 命令候选栏；候选栏已打开时关闭。 |
| `Ctrl+R` | `SessionPickerOpen` | 打开会话选择器。 |
| `Ctrl+N` | `SessionFork` | 有会话选择器时分叉选中会话；否则分叉当前会话。 |
| `Ctrl+A` | `SessionPickerToggleScope` | 在会话选择器中切换当前工作区 / 全部会话。 |

> Plan / Goal / Model picker 也有 `TogglePlanMode` / `ModelPickerOpen` 动作，但默认未绑定按键，通过命令面板或斜杠命令触发。

## 输入编辑（Emacs 风格）

| 快捷键 | 动作 |
| --- | --- |
| `←` / `Ctrl+B` | 光标左移 |
| `→` / `Ctrl+F` | 光标右移 |
| `Alt+←` / `Ctrl+←` / `Alt+B` | 向左移一个词 |
| `Alt+→` / `Ctrl+→` / `Alt+F` | 向右移一个词 |
| `Home` / `Ctrl+A` | 移至行首 |
| `End` / `Ctrl+E` | 移至行尾 |
| `PageUp` / `PageDown` | 上下翻页 |
| `Backspace` | 向前删除一个字符 |
| `Delete` / `Ctrl+D` | 向后删除一个字符 |
| `Ctrl+W` / `Alt+Backspace` | 向前删除一个词 |
| `Alt+D` / `Alt+Delete` | 向后删除一个词 |
| `Ctrl+U` | 删至行首 |
| `Ctrl+K` | 删至行尾 |
| `Ctrl+Y` | Yank（粘贴已删除内容） |
| `Ctrl+-` / `Ctrl+_` | 撤销 |
| `Alt+Enter` / `Ctrl+J` / `Shift+Enter` | 插入换行 |
| `Ctrl+V`（Windows: `Alt+V`） | 从剪贴板粘贴图片 |

## 流式控制

| 快捷键 | 动作 | 说明 |
| --- | --- | --- |
| `Ctrl+S` | `PromptSteer` | 在下一个自然断点用当前编辑器文本 steer 运行中的 turn；无 turn 时排队为后续消息。需 `stty -ixon`。 |
| `Alt+Up` | `EditNextQueuedMessage` | 把下一条排队消息取回编辑器编辑。 |
| `Ctrl+C` | `Cancel` / `Interrupt` | 取消 / 中断当前 turn；若有审批待处理则全部拒绝。 |
| `Ctrl+T` | `TodoPanelToggle` | 展开 / 折叠 Todo 面板。 |

## 工具输出与 Transcript

| 快捷键 | 动作 | 说明 |
| --- | --- | --- |
| `Ctrl+O` | `ToolOutputToggle` | 展开 / 折叠工具调用输出。 |
| `Ctrl+Space` | `TranscriptSelectionStart` | 进入 transcript 条目选择。 |
| `Ctrl+Shift+Space` | `TranscriptSelectionClear` | 清除 transcript 选择。 |
| `Shift+Up` / `Shift+Down` | 向上 / 向下扩展选择 | |
| `Shift+PageUp` / `Shift+PageDown` | 向上 / 向下翻页扩展选择 | |
| `Ctrl+C`（有选区时） | `TranscriptCopySelection` | 复制 transcript 选区。 |

## 审批面板（Approval Modal）

审批面板打开时，输入由 `handle_pending_approval_input` 处理：

| 按键 | 动作 |
| --- | --- |
| `↑` / `↓` | 在 Approve / Always approve / Reject / Revise 选项间移动。 |
| `1` – `4` | 直接选择第 N 个选项。 |
| `Enter` | 确认当前选项；选 Revise 时第一次 Enter 进入反馈输入，再按 Enter 提交。 |
| `Esc` / `Ctrl+C` | 拒绝（等同 Reject）/ 关闭。 |
| `Backspace` / `Delete` | 在 Revise 反馈输入框中删除。 |
| 其他字符 | 追加到 Revise 反馈输入框。 |

## 选择器 / 弹层通用

| 快捷键 | 动作 |
| --- | --- |
| `↑` / `↓` | `SelectUp` / `SelectDown` |
| `PageUp` / `PageDown` | `SelectPageUp` / `SelectPageDown` |
| `Enter` | `SelectConfirm` |
| `Esc` / `Ctrl+C` | `SelectCancel` |

## 自定义

所有动作都暴露了稳定 ID（见各表"动作"列对应的配置 ID，如 `tui.input.submit`、`app.exit`）。在 `~/.neo/config.*` 中按下表键路径绑定即可覆盖默认值；冲突由 `KeybindingsManager` 检测并报告。
