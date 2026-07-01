# 粘贴占位符与 Session Blob 存储设计

## 目标

为 Neo TUI 实现两类粘贴增强功能：

1. **多行文本折叠**：粘贴大段文本时，输入框显示 `[paste +N lines]` 或 `[paste N chars]` 占位符。
2. **图片粘贴**：当当前模型支持识图时，通过 `Ctrl+V`（macOS/Linux）或 `Alt+V`（Windows）读取系统剪贴板图片，插入 `[image #N (WxH)]` 占位符；提交后把图片作为 `Content::Image` 进入对话；发送后在 transcript 中根据终端能力显示图片或占位符文本。当前模型不支持识图时，在 footer 显示一次性状态提示。

为此需要先对 session 存储结构做一次重构，使每个 session 拥有独立目录，附件（图片 blob、plan 文件）按 session 存放。

## 设计范围

- **Phase 1：Session 目录化重构**
  - 新 session 的 transcript 路径从 `~/.neo/sessions/<bucket>/<session_id>.jsonl` 改为 `~/.neo/sessions/<bucket>/<session_id>/transcript.jsonl`。
  - `plans/` 从 `<bucket>/plans/` 迁移到 `<session_id>/plans/`。
  - `prompt-history.jsonl` 仍保留在 `<bucket>/prompt-history.jsonl`，但收集逻辑需要适配新的 transcript 路径。
  - plan mode 的提示词/指引路径同步迁移到 `session_id/plans/`。
  - 提供一次性 Python 迁移脚本 `scripts/migrate-sessions.py`，由用户手动运行；Neo 运行时不兼容旧布局。

- **Phase 2：粘贴占位符**
  - 在 `PromptState` 中引入「可整体删除的占位 token」机制，用于 `[paste +N lines]`、`[paste N chars]`、`[image #N (WxH)]`。
  - 图片作为 blob 存储在 `~/.neo/sessions/<bucket>/<session_id>/blobs/<sha256>.<ext>`，按内容去重。
  - 提交时把占位符展开为 `AgentMessage::User { content: Vec<Content> }`，其中图片为 `Content::Image`。
  - transcript 渲染：输入框只显示占位符；用户消息在 transcript 中支持文本+图片混合渲染，终端支持时显示图片，不支持时回退到占位符文本。

## 需求来源

- 用户期望输入框粘贴体验与 `docs/kimi-code` / `docs/pi` 对齐。
- 当前 `PromptState` 只保存纯 `String`，`TurnRequest.prompt` 只传递 `Vec<String>`，无法携带图片等多模态内容。
- 当前 session 文件平铺在 workspace bucket 下，没有为单条 session 存放附件的位置。

## 相关代码

- `crates/neo-tui/src/chrome.rs`：
  - `PromptState` / `PromptEdit` / `apply_edit` 负责输入框文本与光标。
  - `apply_delete` / `delete_range` 负责 backspace / delete。
  - `handle_prompt_edit_event` 把 `InputEvent::Paste` 直接当成普通文本插入。
- `crates/neo-tui/src/transcript/pane.rs`：
  - `render_prompt_lines` / `build_prompt_logical_lines` 渲染输入框。
  - `push_user_message` 把用户消息以纯文本推进 transcript。
- `crates/neo-agent/src/modes/interactive.rs`：
  - `handle_input_event` / `handle_prompt_edit_event` 处理键盘/粘贴事件。
  - `submit_current_prompt` / `start_turn_with_prompt` / `PromptSubmission::from_text` 组装并提交 turn。
  - `controller_for_config` 构造 `TurnDriver`，把 `Vec<String>` 传给 `run_prompt_streaming` / `run_prompt_in_session_streaming`。
- `crates/neo-agent/src/modes/run.rs`：
  - `run_prompt_streaming` / `prepare_new_streaming_turn` / `append_user_event_jsonl` 负责创建 session、写入初始 user event。
- `crates/neo-agent-core/src/messages.rs`：
  - `AgentMessage::User { content: Vec<Content> }`
  - `Content::Image { mime_type, data: ImageRef }`
  - `to_chat_message` 已能把 `Content::Image` 转成 `neo_ai::ContentPart::Image`。
- `crates/neo-ai/src/types.rs`：
  - `ModelCapabilities.images` 用于判断模型是否支持识图。
  - `ContentPart::Image { mime_type, data: ImageData }`。
- `crates/neo-tui/src/transcript/entry.rs`：
  - `TranscriptEntry::UserMessage(String)` 目前只能存文本。
  - `TranscriptEntry::Image { ... }` 已存在，可用于 transcript 中的图片渲染。
- `crates/neo-tui/src/image.rs`：
  - `InlineImage` / `ImageSource` / `ImageRenderPolicy` / `TerminalImageCapabilities` 已支持 Kitty/iTerm2/Sixel 协议。

## 方案概述

采用 **方案 C**：先进行 session 目录化，再引入 blob 存储和粘贴占位符。图片按内容 sha256 命名后存入 `session_id/blobs/`，在 JSONL 中记录 sha256 与占位符信息；占位符在输入框中被当作一个整体 grapheme 处理，backspace 第一次选中、第二次删除。

## 详细设计

### Phase 1：Session 目录化

#### 新目录结构

```text
~/.neo/sessions/
├── session_index.jsonl
└── <bucket>/
    ├── prompt-history.jsonl          # workspace 级别，保留
    ├── session_<uuid>/
    │   ├── transcript.jsonl
    │   ├── plans/                    # 从 bucket/plans/ 迁移
    │   │   └── <plan-name>.md
    │   └── blobs/                    # 新增
    │       └── <sha256>.<ext>
    └── ...
```

#### 受影响的代码路径

| 区域 | 当前行为 | 修改后 |
|------|----------|--------|
| `create_session_path` | 返回 `.jsonl` 文件路径 | 返回 `session_id/` 目录，并创建 `transcript.jsonl` |
| `session_id_from_path` | 从 `.jsonl` 文件名解析 id | 从目录名解析 id |
| `load_session_transcript` / `fork_session_transcript` | 读 `.jsonl` | 读 `session_id/transcript.jsonl` |
| plan mode | 读写 `<bucket>/plans/<name>.md` | 读写 `<session_id>/plans/<name>.md` |
| prompt-history 收集 | 监听/扫描 bucket 下的 `.jsonl` | 监听/扫描 `<session_id>/transcript.jsonl` |
| CLI `sessions list/show/rename/fork/export` | 枚举 `.jsonl` | 枚举 `session_*/transcript.jsonl` |

#### 迁移脚本

- 路径：`scripts/migrate-sessions.py`
- 行为：
  1. 扫描每个 bucket 下的旧 `session_<uuid>.jsonl`。
  2. 创建 `session_<uuid>/` 目录。
  3. 把 `.jsonl` 移动到 `session_<uuid>/transcript.jsonl`。
  4. 把 bucket 下的 `plans/` 移动到每个 session 下（按 plan 文件名归属，若无法精确归属则保留在 bucket 下并生成警告）。
  5. 支持 `--dry-run`。
  6. 迁移完成后在原位置写入 `MIGRATED` 标记文件或 stdout 报告，但不影响 Neo 运行（Neo 不兼容旧路径，用户需确认迁移成功后再启动）。

> 注意：迁移脚本由用户手动运行，Neo 启动时不自动调用，也不兼容旧路径。

### Phase 2：粘贴占位符

#### 占位符语法

| 类型 | 显示文本 | 触发条件 |
|------|----------|----------|
| 多行文本 | `[paste +N lines]` | 粘贴文本行数 > 10 |
| 长文本 | `[paste N chars]` | 粘贴文本字符数 > 1000 且行数 <= 10 |
| 图片 | `[image #N (WxH)]` | 图片粘贴成功且模型支持识图 |

#### 输入框 token 机制

在 `PromptState` 中引入 `InlineToken` 概念：

```rust
enum InlineToken {
    Text(String),
    PasteMarker { id: usize, selected: bool },
    ImageMarker { id: usize, selected: bool },
}
```

- `PromptState.text` 保持为 `String`，但其中的占位符用不可编辑的 token 表示。
- 渲染输入框时，把 token 序列渲染成占位符文本；`selected` 为 true 时显示高亮/反色。
- `apply_edit(PromptEdit::Backspace)` 时：
  - 如果光标前一个 token 是占位符且未被选中，则把它标记为 `selected`，不删除。
  - 如果已经被选中，则删除整个占位符。
- 占位符在光标移动、选中等场景下都作为一个整体处理。

为降低改动风险，也可以保持 `PromptState.text` 为普通字符串，但在 `segment` / `delete_range` 里识别占位符正则，把它当作一个 grapheme。这是更轻量的实现，参考 pi-tui 的 `segmentWithMarkers`。

**推荐**：先用字符串 + 正则识别的方式实现，保持 `PromptState` 结构不变，减少输入框渲染和编辑的侵入性。

#### 粘贴事件处理

在 `InteractiveController::handle_prompt_edit_event` 中：

1. 收到 `InputEvent::Paste(text)` 时：
   - 清理文本（过滤控制字符、统一换行）。
   - 如果行数 > 10 或字符数 > 1000，生成 paste marker，调用 `PromptEdit::Insert(marker)`。
   - 否则直接 `PromptEdit::Insert(text)`。

2. 收到图片粘贴键（`Ctrl+V` 或 `Alt+V`）时：
   - 先检查当前模型是否 `capabilities.images == true`。
   - 不支持：调用 `push_status("此模型不支持图片识别")` 或等价的英文提示，消耗事件。
   - 支持：异步读取系统剪贴板。
     - 如果读到图片：保存 blob，生成 `[image #N (WxH)]` 插入输入框。
     - 如果读到文本或失败：把按键事件转换成普通 paste 事件继续处理。

#### 图片粘贴键绑定

- 默认：
  - 非 Windows：`Ctrl+V`
  - Windows：`Alt+V`
- 允许用户在 `config.toml` 的 `[tui.keybindings]` 中自定义，例如 `paste_image = "ctrl+shift+v"`。
- 输入框事件处理顺序：
  1. 如果当前光标位于已有的 paste/image marker 上，先尝试展开 marker（仅对 paste marker 有效，image marker 不展开），消耗事件。
  2. 否则触发图片粘贴流程。
  3. 若未读到图片，fallback 为普通文本粘贴（生成 `InputEvent::Paste` 或模拟 bracketed paste 内容）。

#### Blob 存储与附件管理

- `ImageAttachmentStore` 每个 `NeoChromeState` / `InteractiveController` 实例一个：
  - 维护 `id -> Attachment { sha256, mime, width, height, placeholder }`。
  - id 在每个进程实例内从 1 开始递增（显示编号）。
  - 同一个 session resume 后，从 JSONL 反序列化时重建 attachment 映射（id 按出现顺序重新分配）。
- 保存图片时：
  1. 计算 sha256。
  2. 文件名：`blobs/<sha256>.<ext>`（ext 由 mime 推断，如 png/jpg/webp/gif）。
  3. 如果 blob 已存在则跳过写入。
- 删除 marker 时不删除 blob；session 被删除时由外部清理整个 `session_<uuid>/` 目录。

#### 占位符展开为消息内容

`submit_current_prompt` 时：

1. 从 `PromptState.text` 中提取所有占位符。
2. 把文本切分为 `Vec<Content>`：
   - 普通文本段 -> `Content::Text`
   - paste marker -> 从 store 取原文本 -> `Content::Text`
   - image marker -> 从 store 取 sha256/mime，构造 `Content::Image { mime_type, data: ImageRef::Blob(sha256) }`
3. 构造 `AgentMessage::User { content: Vec<Content> }`。
4. 把 `TurnRequest.prompt` 从 `Vec<String>` 改为 `Vec<PromptPart>` 或 `Vec<Content>`，使 runtime 能直接接收多模态消息。

#### Runtime 与 JSONL

- `AgentMessage::User { content }` 已支持 `Content::Image { mime_type, data: ImageRef::Base64(...) }` 或 `Url(...)`。
- 新增 `ImageRef::Blob(String)` 表示指向 session blob 的 sha256。
- JSONL 序列化时：
  - `ImageRef::Blob(sha256)` 序列化为 `{ "blob": sha256, "mime_type": "image/png" }`。
  - `ImageRef::Base64` / `Url` 保持原行为。
- 反序列化时：
  - 如果 blob 文件存在，渲染图片。
  - 如果不存在，渲染为占位符文本 `[image #N (WxH)]`（占位符信息随 JSONL 一起保存，或在反序列化时根据 attachment 映射生成）。

#### Transcript 渲染

- `TranscriptEntry::UserMessage(String)` 需要扩展为能携带图片。
- 推荐做法：新增 `TranscriptEntry::UserMessageParts(Vec<UserMessagePart>)`，其中：
  ```rust
  enum UserMessagePart {
      Text(String),
      Image { sha256: String, mime_type: String, width: u32, height: u32 },
  }
  ```
- 或者复用 `AgentMessage::Content` 的转换结果直接生成渲染行。
- 渲染时：
  - 文本部分按现有用户消息样式换行、折行。
  - 图片部分调用 `InlineImage::bytes(...)` + `ImageRenderPolicy` + `TerminalImageCapabilities`，生成 Kitty/iTerm2/Sixel 序列；不支持时渲染为 `[image #N (WxH)]` 或 `[image (WxH)]`。

#### 模型能力检查

- 当前活动模型：`InteractiveController.active_model`（`Option<SelectedModel>`）。
- 在 `controller_for_config` 中初始化时，以及每次切换模型时，把 `capabilities.images` 缓存到 `InteractiveController`。
- 图片粘贴键触发时直接查缓存，不阻塞输入循环。
- 如果当前没有模型或无法解析能力，保守地视为不支持，提示用户。

#### 状态提示

- 当 model 不支持图片时，调用 `self.push_status("此模型不支持图片识别")`（或英文文案 `This model does not support image input`）。
- 提示一次性显示，不进入 transcript，几秒后消失（复用现有 footer status 机制）。

## 关键接口变更

### `crates/neo-tui/src/chrome.rs`

- `PromptState` 增加 `attachments: ImageAttachmentStore`（或保持 store 在 controller，只在渲染时传引用）。
- `PromptEdit` 保持当前变体，不需要新增；占位符作为普通字符串插入和删除。
- 新增 `PromptState::select_marker_at_cursor()` / `delete_selected_marker()` 等辅助方法。

### `crates/neo-agent/src/modes/interactive.rs`

- `TurnRequest.prompt` 从 `Vec<String>` 改为 `Vec<Content>`（或新增 `PromptPart`）。
- `start_turn_with_prompt` 接受 `Vec<Content>`。
- `handle_input_event` / `handle_prompt_edit_event` 处理图片粘贴键和图片读取结果。
- 新增 `read_clipboard_image()` 异步方法（跨平台实现）。

### `crates/neo-agent/src/modes/run.rs`

- `run_prompt_streaming` / `run_prompt_in_session_streaming` 的 `prompt` 参数改为 `Vec<Content>`。
- `prepare_new_streaming_turn` 不再 `prompt.join(" ")`；直接把 `Vec<Content>` 写入 `AgentEvent::MessageAppended`。
- `create_session_path` 返回目录路径，并创建 `transcript.jsonl`。

### `crates/neo-agent-core/src/messages.rs`

- `ImageRef` 新增 `Blob(String)` 变体。
- `AgentMessage::user_text` 之外新增 `AgentMessage::user_parts(Vec<Content>)`。

### `crates/neo-agent-core/src/runtime.rs`

- `chat_request` 已支持 `Content::Image`，无需大改；需要确保 `ImageRef::Blob` 在发给 provider 前解析为 base64 或 URL。

### `crates/neo-tui/src/transcript/entry.rs`

- 新增 `TranscriptEntry::UserMessageParts` 或扩展 `UserMessage` 变体。
- `render_user_message` 支持混合文本和图片。

## 测试策略

- **单元测试**：
  - `PromptState` 中占位符的 backspace 删除逻辑（第一次选中，第二次删除）。
  - 占位符正则解析与展开（文本、图片混合）。
  - `ImageAttachmentStore` 的 id 分配、sha256 去重、blob 路径生成。
- **集成测试**：
  - 模拟 `Event::Paste` 大段文本，验证输入框显示 `[paste +N lines]`。
  - 提交后验证 `AgentMessage::User` 内容被正确展开。
  - 模拟不支持图片的模型，验证 footer status 提示。
  - 模拟图片粘贴并构造假 blob，验证 transcript 渲染包含图片序列或占位符。
- **迁移脚本测试**：
  - 构造旧布局 fixture，运行 `scripts/migrate-sessions.py --dry-run` 和实际迁移，验证目录结构和文件内容。

## 风险与回退

- **迁移脚本是破坏性的**：用户必须先停止所有 Neo 进程并备份 `~/.neo/sessions/`。
- **剪贴板读取跨平台差异大**：优先实现 macOS（`osascript`/AppKit）和 Linux（`wl-paste`/`xclip`）；Windows 使用 PowerShell 或 `Get-Clipboard`。如果某平台读取失败，fallthrough 到普通文本粘贴。
- **占位符与现有正则冲突**：`[paste ...]` 和 `[image ...]` 不是常见用户输入，冲突概率低；用户若真的需要输入这些字面量，可通过删除 marker 后展开原文本再编辑（paste marker），图片 marker 不支持字面量输入。
- **Blob 文件清理**：删除 session 时连带删除整个 `session_<uuid>/` 目录即可；单个 blob 不被删除以避免误删，但未来可考虑垃圾回收脚本。

## 未包含在本设计中的内容

- 视频粘贴（kimi-code 支持 `[video #N ...]`）。
- 图片拖拽、文件路径粘贴自动转图片。
- 图片在输入框内预览（只显示占位符）。
- 在 resume 时通过 UI 重新定位丢失的 blob 文件。

## 待实现检查清单

- [ ] `scripts/migrate-sessions.py` 迁移脚本。
- [ ] Session 路径、plan 路径、prompt-history 扫描逻辑迁移。
- [ ] `PromptState` 占位符识别与整体删除。
- [ ] `ImageAttachmentStore` 与 blob 存储。
- [ ] 跨平台剪贴板图片读取。
- [ ] 图片粘贴键绑定（`Ctrl+V` / `Alt+V`）。
- [ ] Model 能力检查与 footer 提示。
- [ ] `TurnRequest.prompt` / `AgentMessage::User` 多模态改造。
- [ ] JSONL 序列化 `ImageRef::Blob`。
- [ ] Transcript 混合文本+图片渲染。
- [ ] 单元测试与集成测试。
