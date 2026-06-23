# 粘贴占位符与 Session Blob 存储实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 Neo TUI 的多行文本粘贴折叠占位符 `[paste +N lines]` / `[paste N chars]` 和图片粘贴占位符 `[image #N (WxH)]`，占位符在输入框中作为整体被 backspace 删除；同时完成 session 目录化重构，使每条 session 拥有独立目录（`transcript.jsonl`、`plans/`、`blobs/`），并提供一次性迁移脚本。

**Architecture:** 先重构 session 存储路径（从平铺 `.jsonl` 改为 `session_<uuid>/transcript.jsonl`，并把 `plans/` 迁到 session 目录），再基于新目录引入 blob 存储。输入框仍用纯字符串，通过 marker 正则把占位符识别为单个 grapheme；图片粘贴键触发时读取系统剪贴板，支持识图的模型保存 blob 并插入占位符，否则走普通粘贴或 footer 提示。

**Tech Stack:** Rust 2024, `tokio`, `crossterm`, `serde_json`, `neo-tui`, `neo-agent-core`, `neo-ai`, `xtask`/`nextest`。

---

## 前置阅读

- `AGENTS.md`（尤其 Git 变更禁令、测试策略、验证层级）
- `docs/superpowers/specs/2026-06-23-paste-markers-and-session-blobs-design.md`
- `docs/kimi-code/apps/kimi-code/src/tui/components/editor/custom-editor.ts`（pi-tui 粘贴 marker 参考）
- `docs/kimi-code/apps/kimi-code/src/tui/utils/image-attachment-store.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/utils/image-placeholder.ts`

## 项目规则

- 不要运行 `git reset`, `git checkout`, `git restore`, `git stash`, `git clean`, `git rebase`, `git rm` 等会修改 worktree 的命令。
- `git add`/`git commit` 必须得到用户逐条授权。
- 测试统一通过 `cargo run -p xtask -- test ...` 运行，禁止用裸 `cargo test` 作为完成证据。
- 运行测试前先 `icm recall-context "paste markers session blobs" --limit 5`。
- 如果解决了一个有意义的错误，先用 `icm store -t errors-resolved ...` 保存。
- 任务完成后用 `icm store -t context-neo ...` 保存总结。

## 当前代码地图

### Session 存储

- `crates/neo-agent/src/modes/run.rs`
  - `create_session_path`：生成 `.jsonl` 文件路径
  - `session_id_from_path`：从 `.jsonl` 文件名解析 session id
  - `run_prompt_streaming` / `run_prompt_in_session_streaming`
  - `prepare_new_streaming_turn`：`prompt.join(" ")`
  - `append_user_event_jsonl` / `append_user_event`
  - `load_session_transcript` / `fork_session_transcript`
- `crates/neo-agent-core/src/session/`（查找所有使用 `.jsonl` 路径的函数）
- `crates/neo-agent/src/modes/interactive.rs`
  - CLI session 管理调用：session picker、fork、rename、export

### Plan Mode

- `crates/neo-agent-core/src/mode/plan.rs`
- `crates/neo-agent-core/src/tools/plan_mode.rs`
- Plan 文件当前路径假设为 `<bucket>/plans/<name>.md`

### Prompt History

- `crates/neo-agent/src/modes/interactive.rs` 中 prompt-history 收集逻辑
- `append_prompt_history` 写入 `<bucket>/prompt-history.jsonl`
- 扫描旧 `.jsonl` 文件的地方需要改为扫描 `session_*/transcript.jsonl`

### TUI 输入框

- `crates/neo-tui/src/chrome.rs`
  - `PromptState`（text + cursor + scroll_offset + history + undo）
  - `PromptEdit` 变体
  - `apply_edit` / `apply_edit_with_width`
  - `apply_delete` / `delete_range`
- `crates/neo-tui/src/transcript/pane.rs`
  - `render_prompt_lines` / `build_prompt_logical_lines`
  - `push_user_message`
- `crates/neo-tui/src/input.rs`
  - `InputEvent::Paste(String)`
  - keybindings 系统

### 交互控制器

- `crates/neo-agent/src/modes/interactive.rs`
  - `handle_input_event`
  - `handle_prompt_edit_event`
  - `submit_current_prompt`
  - `start_turn_with_prompt`
  - `TurnRequest.prompt: Vec<String>`
  - `controller_for_config` 构造 `TurnDriver`

### 消息模型

- `crates/neo-agent-core/src/messages.rs`
  - `AgentMessage::User { content: Vec<Content> }`
  - `Content::Image { mime_type, data: ImageRef }`
  - `ImageRef::Base64` / `Url`
  - `to_chat_message` / `to_content_part`
- `crates/neo-ai/src/types.rs`
  - `ModelCapabilities.images`
  - `ContentPart::Image`

### Transcript 渲染

- `crates/neo-tui/src/transcript/entry.rs`
  - `TranscriptEntry::UserMessage(String)`
  - `TranscriptEntry::Image { ... }`
- `crates/neo-tui/src/transcript/pane.rs`
  - `apply_agent_event`
  - `push_user_message`

### 图片协议

- `crates/neo-tui/src/image.rs`
  - `InlineImage`
  - `ImageRenderPolicy`
  - `TerminalImageCapabilities`
  - `render_inline_image`

---

## Phase 1：Session 目录化重构

### Task 1.1：创建一次性迁移脚本

**Files:**
- Create: `scripts/migrate-sessions.py`

- [ ] **Step 1: 设计迁移脚本结构**

```python
#!/usr/bin/env python3
"""Migrate Neo sessions from flat .jsonl layout to per-session directories.

Old layout:
    ~/.neo/sessions/<bucket>/session_<uuid>.jsonl
    ~/.neo/sessions/<bucket>/plans/

New layout:
    ~/.neo/sessions/<bucket>/session_<uuid>/transcript.jsonl
    ~/.neo/sessions/<bucket>/session_<uuid>/plans/

Usage:
    python scripts/migrate-sessions.py ~/.neo/sessions --dry-run
    python scripts/migrate-sessions.py ~/.neo/sessions
"""

import argparse
import json
import os
import re
import shutil
import sys
from pathlib import Path

SESSION_RE = re.compile(r"^(session_[0-9a-fA-F-]+)\.jsonl$")


def migrate_sessions(sessions_root: Path, dry_run: bool) -> dict:
    stats = {"buckets": 0, "sessions_moved": 0, "plans_moved": 0, "plans_unclaimed": 0}
    for bucket in sorted(sessions_root.iterdir()):
        if not bucket.is_dir():
            continue
        if bucket.name == "session_index.jsonl":
            continue
        stats["buckets"] += 1
        # 1. Move session .jsonl files into per-session directories.
        for entry in sorted(bucket.iterdir()):
            if not entry.is_file():
                continue
            match = SESSION_RE.match(entry.name)
            if not match:
                continue
            session_id = match.group(1)
            session_dir = bucket / session_id
            target = session_dir / "transcript.jsonl"
            if dry_run:
                print(f"[dry-run] would move {entry} -> {target}")
            else:
                session_dir.mkdir(parents=True, exist_ok=True)
                shutil.move(str(entry), str(target))
            stats["sessions_moved"] += 1
        # 2. Move plans into the session they belong to.
        plans_dir = bucket / "plans"
        if plans_dir.is_dir():
            # Strategy: a plan name such as "<session_id>-<slug>.md" belongs to that session.
            for plan_file in sorted(plans_dir.iterdir()):
                if not plan_file.is_file():
                    continue
                # Try to find owning session by prefix.
                owner = None
                for session_dir in sorted(bucket.glob("session_*")):
                    prefix = session_dir.name
                    if plan_file.name.startswith(prefix):
                        owner = session_dir
                        break
                if owner is None:
                    if dry_run:
                        print(f"[dry-run] would leave unclaimed plan {plan_file}")
                    else:
                        print(f"warning: unclaimed plan {plan_file}, leaving in {plans_dir}")
                    stats["plans_unclaimed"] += 1
                    continue
                target_dir = owner / "plans"
                target = target_dir / plan_file.name
                if dry_run:
                    print(f"[dry-run] would move {plan_file} -> {target}")
                else:
                    target_dir.mkdir(parents=True, exist_ok=True)
                    shutil.move(str(plan_file), str(target))
                stats["plans_moved"] += 1
            # Optionally remove empty plans dir in non-dry-run mode.
            if not dry_run:
                try:
                    plans_dir.rmdir()
                except OSError:
                    pass
    return stats


def main() -> int:
    parser = argparse.ArgumentParser(description="Migrate Neo session layout")
    parser.add_argument("sessions_root", type=Path, help="Path to ~/.neo/sessions")
    parser.add_argument("--dry-run", action="store_true", help="Print actions without moving files")
    args = parser.parse_args()

    if not args.sessions_root.exists():
        print(f"error: {args.sessions_root} does not exist", file=sys.stderr)
        return 1

    stats = migrate_sessions(args.sessions_root, args.dry_run)
    print(json.dumps(stats, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 2: 验证脚本语法**

Run: `python -m py_compile scripts/migrate-sessions.py`
Expected: no output, exit 0

- [ ] **Step 3: 提交脚本**

```bash
git add scripts/migrate-sessions.py
# 需要用户授权后再 commit
```

### Task 1.2：重构 `create_session_path` 返回目录

**Files:**
- Modify: `crates/neo-agent/src/modes/run.rs`

- [ ] **Step 1: 找到 `create_session_path` 函数**

Run: `cx definition --name create_session_path`

- [ ] **Step 2: 修改函数返回 `PathBuf` 指向 session 目录，并创建 `transcript.jsonl`**

把原来的代码从类似：

```rust
let path = sessions_dir.join(format!("session_{}.jsonl", id));
```

改为：

```rust
let session_dir = sessions_dir.join(format!("session_{}", id));
let transcript_path = session_dir.join("transcript.jsonl");
fs::create_dir_all(&session_dir).await?;
Ok(transcript_path)
```

确保调用者仍然拿到的是 `transcript.jsonl` 路径。

- [ ] **Step 3: 运行相关测试**

Run: `cargo run -p xtask -- test -p neo-agent sessions 2>&1 | tail -40`
Expected: 已有测试需要同步更新，可能会失败，先记录失败点。

### Task 1.3：更新 `session_id_from_path`

**Files:**
- Modify: `crates/neo-agent/src/modes/run.rs`

- [ ] **Step 1: 找到 `session_id_from_path` 函数**

Run: `cx definition --name session_id_from_path`

- [ ] **Step 2: 修改解析逻辑**

旧逻辑通常从文件名 `session_<uuid>.jsonl` 解析。新逻辑应：

1. 取路径的父目录名。
2. 检查是否为 `session_<uuid>` 格式。
3. 同时保留对旧文件名 `session_<uuid>.jsonl` 的解析能力，仅用于迁移脚本验证或错误信息，Neoo 运行时不应依赖旧路径。

```rust
fn session_id_from_path(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    if file_name == "transcript.jsonl" {
        let session_dir = path.parent()?;
        let dir_name = session_dir.file_name()?.to_str()?;
        if dir_name.starts_with("session_") {
            return Some(dir_name.to_owned());
        }
    }
    // Legacy fallback for diagnostics only.
    if file_name.starts_with("session_") && file_name.ends_with(".jsonl") {
        return Some(file_name.strip_suffix(".jsonl")?.to_owned());
    }
    None
}
```

- [ ] **Step 3: 运行 focused 测试**

Run: `cargo run -p xtask -- test -p neo-agent session_jsonl 2>&1 | tail -40`

### Task 1.4：更新 `load_session_transcript` / `fork_session_transcript`

**Files:**
- Modify: `crates/neo-agent/src/modes/run.rs`

- [ ] **Step 1: 找到这两个函数**

Run:
```bash
cx definition --name load_session_transcript
cx definition --name fork_session_transcript
```

- [ ] **Step 2: 修改路径构造**

把 `session_path = bucket_dir.join(format!("{session_id}.jsonl"))` 改为：

```rust
let session_path = bucket_dir.join(&session_id).join("transcript.jsonl");
```

- [ ] **Step 3: 运行 focused 测试**

Run: `cargo run -p xtask -- test -p neo-agent sessions 2>&1 | tail -60`

### Task 1.5：更新 prompt-history 收集逻辑

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: 找到所有扫描 `.jsonl` 的代码**

Run:
```bash
cx grep --pattern "\.jsonl" --path crates/neo-agent/src/modes/interactive.rs --output-mode content -n
```

- [ ] **Step 2: 修改为扫描 `session_*/transcript.jsonl`**

例如把 `bucket.read_dir()` 后检查 `.ends_with(".jsonl")` 改为：

```rust
if entry.is_dir() && name.starts_with("session_") {
    let transcript = entry.path().join("transcript.jsonl");
    if transcript.exists() {
        // process transcript
    }
}
```

- [ ] **Step 3: 运行相关测试**

Run: `cargo run -p xtask -- test -p neo-agent prompt_history 2>&1 | tail -40`

### Task 1.6：更新 Plan Mode 路径

**Files:**
- Modify: `crates/neo-agent-core/src/mode/plan.rs`
- Modify: `crates/neo-agent-core/src/tools/plan_mode.rs`
- Modify: 其他引用 `<bucket>/plans/` 的地方

- [ ] **Step 1: 找到 plan 文件路径生成代码**

Run:
```bash
cx grep --pattern "plans/" --path crates/neo-agent-core/src --output-mode files_with_matches
cx grep --pattern "plans" --path crates/neo-agent-core/src/mode/plan.rs --output-mode content -n
```

- [ ] **Step 2: 设计新的 plan 路径获取方式**

Plan mode 需要知道当前 session 的目录。`AgentConfig` 或 plan tool context 中需要传入 session 目录。

推荐：
- 在 `AgentConfig` 中新增 `session_directory: Option<PathBuf>`。
- Plan tool 使用 `config.session_directory.as_ref()?.join("plans")`。

- [ ] **Step 3: 修改 `enter_plan_mode` / `exit_plan_mode` 工具**

把原来基于 `config.workspace_root` 或硬编码 bucket 路径的 plan 目录改为从 `session_directory` 拼接。

- [ ] **Step 4: 运行 plan mode 测试**

Run: `cargo run -p xtask -- test -p neo-agent-core plan_mode 2>&1 | tail -60`

### Task 1.7：更新 CLI session 管理命令

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs` 中的 session picker / fork / rename / export 相关代码
- Modify: `crates/neo-agent/src/cli/sessions.rs`（如果存在）

- [ ] **Step 1: 找到 session 枚举代码**

Run:
```bash
cx grep --pattern "session_" --path crates/neo-agent/src --output-mode files_with_matches
```

- [ ] **Step 2: 统一使用新的 transcript 路径**

所有 `session_<uuid>.jsonl` 改为 `session_<uuid>/transcript.jsonl`。

- [ ] **Step 3: 运行 CLI 测试**

Run: `cargo run -p xtask -- test -p neo-agent cli_commands 2>&1 | tail -60`

### Task 1.8：Session 目录化集成测试

**Files:**
- Modify: `crates/neo-agent/tests/cli_commands.rs`（或新增测试文件）

- [ ] **Step 1: 编写测试创建新 layout 的 session**

```rust
#[tokio::test]
async fn new_session_uses_directory_layout() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config_in(&temp);
    // 运行一次简单 turn，触发 session 创建
    // 验证 transcript.jsonl 位于 session_<uuid>/transcript.jsonl
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo run -p xtask -- test -p neo-agent sessions 2>&1 | tail -60`

---

## Phase 2：粘贴占位符

### Task 2.1：定义 marker 正则与 attachment store 类型

**Files:**
- Create: `crates/neo-tui/src/paste.rs`

- [ ] **Step 1: 创建模块并定义正则与占位符类型**

```rust
//! Input composer paste markers and image attachments.

use regex::Regex;
use std::sync::OnceLock;

/// Matches `[paste +N lines]`, `[paste N chars]`, or `[image #N (WxH)]`.
pub fn marker_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[(?:(paste)\s+(?:\+(\d+)\s+lines|(\d+)\s+chars)|(image)\s+#(\d+)\s+\((\d+)x(\d+)\))\]")
            .expect("marker regex is valid")
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    Paste { id: usize, lines: Option<usize> },
    Image { id: usize, width: u32, height: u32 },
}

impl Marker {
    pub fn id(&self) -> usize {
        match self {
            Self::Paste { id, .. } | Self::Image { id, .. } => *id,
        }
    }

    pub fn as_placeholder(&self) -> String {
        match self {
            Self::Paste { id, lines: Some(n) } => format!("[paste +{n} lines]"),
            Self::Paste { id, lines: None } => {
                // chars case: caller must provide original char count.
                // For placeholder identity we keep the id hidden in rendering.
                format!("[paste #{} chars]", id)
            }
            Self::Image { id, width, height } => {
                format!("[image #{id} ({width}x{height})]")
            }
        }
    }
}

/// Parse markers in source order.
pub fn parse_markers(text: &str) -> Vec<(usize, Marker)> {
    let mut out = Vec::new();
    for cap in marker_regex().captures_iter(text) {
        let start = cap.get(0).unwrap().start();
        if let Some(lines) = cap.get(2).and_then(|m| m.as_str().parse().ok()) {
            out.push((start, Marker::Paste { id: out.len() + 1, lines: Some(lines) }));
        } else if let Some(chars) = cap.get(3).and_then(|m| m.as_str().parse().ok()) {
            out.push((start, Marker::Paste { id: out.len() + 1, lines: None }));
        } else if cap.get(4).is_some() {
            let id = cap[5].parse().unwrap_or(0);
            let width = cap[6].parse().unwrap_or(0);
            let height = cap[7].parse().unwrap_or(0);
            out.push((start, Marker::Image { id, width, height }));
        }
    }
    out
}
```

- [ ] **Step 2: 添加单元测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paste_lines_marker() {
        let text = "hello [paste +15 lines] world";
        let markers = parse_markers(text);
        assert_eq!(markers.len(), 1);
        assert!(matches!(markers[0].1, Marker::Paste { lines: Some(15), .. }));
    }

    #[test]
    fn parses_image_marker() {
        let text = "look [image #3 (640x480)] here";
        let markers = parse_markers(text);
        assert_eq!(markers.len(), 1);
        assert!(matches!(markers[0].1, Marker::Image { id: 3, width: 640, height: 480 }));
    }
}
```

- [ ] **Step 3: 运行测试**

Run: `cargo run -p xtask -- test -p neo-tui paste 2>&1 | tail -40`

### Task 2.2：把 `neo-tui/src/paste.rs` 接入 `lib.rs`

**Files:**
- Modify: `crates/neo-tui/src/lib.rs`

- [ ] **Step 1: 添加模块声明**

```rust
pub mod paste;
```

- [ ] **Step 2: 运行编译检查**

Run: `cargo run -p xtask -- check --workspace 2>&1 | tail -60`

### Task 2.3：实现 `ImageAttachmentStore`

**Files:**
- Modify: `crates/neo-tui/src/paste.rs`（或创建 `crates/neo-agent/src/image_attachment.rs`）

推荐把 `ImageAttachmentStore` 放在 `neo-agent` 而不是 `neo-tui`，因为它依赖 session 目录和文件系统；但也可以放在 `neo-tui` 保持与输入框 close。这里我们选择 `neo-tui/src/paste.rs` 先定义核心结构，在 `neo-agent` 中做文件 IO。

- [ ] **Step 1: 定义 attachment 结构**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    pub id: usize,
    pub sha256: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Default, Clone)]
pub struct ImageAttachmentStore {
    next_id: usize,
    by_id: std::collections::BTreeMap<usize, ImageAttachment>,
}

impl ImageAttachmentStore {
    pub fn new() -> Self {
        Self { next_id: 1, by_id: BTreeMap::new() }
    }

    pub fn add(&mut self, sha256: String, mime_type: String, width: u32, height: u32) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.by_id.insert(id, ImageAttachment { id, sha256, mime_type, width, height });
        id
    }

    pub fn get(&self, id: usize) -> Option<&ImageAttachment> {
        self.by_id.get(&id)
    }

    pub fn remove(&mut self, id: usize) -> Option<ImageAttachment> {
        self.by_id.remove(&id)
    }

    pub fn clear(&mut self) {
        self.by_id.clear();
        self.next_id = 1;
    }
}
```

- [ ] **Step 2: 添加测试**

```rust
#[test]
fn attachment_store_assigns_incrementing_ids() {
    let mut store = ImageAttachmentStore::new();
    let id1 = store.add("a".into(), "image/png".into(), 100, 100);
    let id2 = store.add("b".into(), "image/jpeg".into(), 200, 200);
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(store.get(id1).unwrap().sha256, "a");
}
```

- [ ] **Step 3: 运行测试**

Run: `cargo run -p xtask -- test -p neo-tui paste 2>&1 | tail -40`

### Task 2.4：修改 `PromptState` 支持 marker 整体删除

**Files:**
- Modify: `crates/neo-tui/src/chrome.rs`

- [ ] **Step 1: 在 `apply_edit` 的 Backspace 分支中识别 marker**

当前 `PromptEdit::Backspace` 直接 `apply_delete(cursor-1, cursor, Backward, false)`。需要改为：

1. 获取光标前一个 marker（如果光标刚好在 marker 末尾或内部）。
2. 如果是 marker 且未被选中，则把该 marker 的文本替换为「selected」版本（例如加 ANSI 反色），并把 marker 标记为选中。
3. 如果已经被选中，则删除整个 marker。

推荐：在 `PromptState` 中新增字段 `selected_marker: Option<Range<usize>>` 或 `selected_marker_text: Option<String>`。

简化实现：

```rust
pub struct PromptState {
    pub text: String,
    pub cursor: usize,
    scroll_offset: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<PromptSnapshot>,
    undo_stack: Vec<PromptSnapshot>,
    kill_ring: Vec<String>,
    selected_marker: Option<(usize, usize)>, // byte range
}
```

- [ ] **Step 2: 实现 `marker_at_cursor` 辅助函数**

```rust
impl PromptState {
    fn marker_before_cursor(&self) -> Option<(usize, usize)> {
        let text = &self.text;
        for cap in marker_regex().captures_iter(text) {
            let m = cap.get(0).unwrap();
            if m.end() == self.byte_index(self.cursor) {
                return Some((m.start(), m.end()));
            }
            if m.start() <= self.byte_index(self.cursor) && m.end() >= self.byte_index(self.cursor) {
                return Some((m.start(), m.end()));
            }
        }
        None
    }
}
```

- [ ] **Step 3: 修改 Backspace 行为**

```rust
PromptEdit::Backspace => {
    if let Some((start, end)) = self.marker_before_cursor() {
        if self.selected_marker == Some((start, end)) {
            self.apply_delete_with_byte_range(start, end, DeleteDirection::Backward, false)
        } else {
            self.selected_marker = Some((start, end));
            None
        }
    } else {
        self.selected_marker = None;
        self.apply_delete(self.cursor.saturating_sub(1), self.cursor, DeleteDirection::Backward, false)
    }
}
```

- [ ] **Step 4: 修改 Delete 行为**

光标后的 marker 同样处理。

- [ ] **Step 5: 光标移动时清除选中**

在 `MoveLeft` / `MoveRight` / `MoveHome` / `MoveEnd` 等分支中设置 `self.selected_marker = None`。

- [ ] **Step 6: 添加单元测试**

```rust
#[test]
fn backspace_selects_marker_first_then_deletes() {
    let mut prompt = PromptState::new("hello [paste +5 lines]");
    prompt.cursor = prompt.char_len();
    // First backspace selects marker, text unchanged.
    prompt.apply_edit(PromptEdit::Backspace);
    assert!(prompt.text.contains("[paste +5 lines]"));
    // Second backspace deletes marker.
    prompt.apply_edit(PromptEdit::Backspace);
    assert!(!prompt.text.contains("[paste +5 lines]"));
    assert_eq!(prompt.text, "hello ");
}
```

- [ ] **Step 7: 运行测试**

Run: `cargo run -p xtask -- test -p neo-tui chrome 2>&1 | tail -60`

### Task 2.5：输入框渲染 marker 高亮

**Files:**
- Modify: `crates/neo-tui/src/transcript/pane.rs` 中的 `build_prompt_logical_lines`

- [ ] **Step 1: 根据 `selected_marker` 给 marker 加 ANSI 反色**

在插入 `CURSOR_MARKER` 后、wrap 前，对 `selected_marker` 范围内的文本包裹 ANSI 反色序列（使用 theme 的 `selection` 或 `brand` 颜色）。

```rust
let highlighted = if let Some((start, end)) = prompt.selected_marker {
    let before = &marked[..start];
    let selected = &marked[start..end];
    let after = &marked[end..];
    format!("{before}{selected}{after}")
} else {
    marked
};
```

> 注意：这里需要处理 byte/char 索引转换，确保 `selected_marker` 是 byte 范围。

- [ ] **Step 2: 运行渲染测试**

Run: `cargo run -p xtask -- test -p neo-tui transcript_pane 2>&1 | tail -60`

### Task 2.6：多行文本粘贴折叠

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-tui/src/chrome.rs`（可选，如果需要辅助函数）

- [ ] **Step 1: 修改 `handle_prompt_edit_event` 中的 Paste 处理**

当前：

```rust
InputEvent::Paste(text) => {
    self.apply_prompt_edit(PromptEdit::Insert(text));
}
```

改为：

```rust
InputEvent::Paste(text) => {
    let cleaned = clean_pasted_text(&text);
    let lines: Vec<&str> = cleaned.split('\n').collect();
    if lines.len() > 10 || cleaned.len() > 1000 {
        let id = self.next_paste_id();
        self.paste_store.insert(id, cleaned.clone());
        let marker = if lines.len() > 10 {
            format!("[paste +{} lines]", lines.len())
        } else {
            format!("[paste {} chars]", cleaned.len())
        };
        self.apply_prompt_edit(PromptEdit::Insert(&marker));
    } else {
        self.apply_prompt_edit(PromptEdit::Insert(&cleaned));
    }
}
```

- [ ] **Step 2: 定义 `clean_pasted_text`**

```rust
fn clean_pasted_text(text: &str) -> String {
    text.replace('\r', "")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}
```

- [ ] **Step 3: 在 `InteractiveController` 中新增 paste store**

```rust
struct InteractiveController {
    // ... existing fields
    paste_store: std::collections::HashMap<usize, String>,
    next_paste_id: usize,
}
```

初始化 `next_paste_id: 1`。

- [ ] **Step 4: 添加集成测试**

```rust
#[tokio::test]
async fn large_paste_becomes_paste_marker() {
    let mut controller = InteractiveController::new_for_test(...);
    let large = "line\n".repeat(15);
    controller.handle_input_event(InputEvent::Paste(large)).await.unwrap();
    assert!(controller.chrome().prompt().text.contains("[paste +16 lines]"));
}
```

- [ ] **Step 5: 运行测试**

Run: `cargo run -p xtask -- test -p neo-agent interactive 2>&1 | tail -60`

### Task 2.7：展开 paste marker 再次粘贴

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: 处理图片粘贴键时的 marker 展开**

在 `Ctrl+V` / `Alt+V` 处理中，先检查光标是否在 paste marker 上：

```rust
fn expand_marker_at_cursor(&mut self) -> bool {
    let prompt = self.tui.chrome().prompt();
    let text = &prompt.text;
    let cursor_byte = prompt.byte_index(prompt.cursor);
    for cap in marker_regex().captures_iter(text) {
        let m = cap.get(0).unwrap();
        if m.start() <= cursor_byte && m.end() >= cursor_byte {
            // Extract id and replace marker with original text.
            let id = cap[5].parse::<usize>().ok(); // or cap[2] for paste
            if let Some(original) = id.and_then(|id| self.paste_store.get(&id)).cloned() {
                let before = text[..m.start()].to_owned();
                let after = text[m.end()..].to_owned();
                let new_text = format!("{}{}{}", before, original, after);
                self.tui.chrome_mut().prompt_mut().set_text(new_text);
                return true;
            }
        }
    }
    false
}
```

- [ ] **Step 2: 在图片粘贴键处理中调用**

```rust
if self.expand_marker_at_cursor() {
    return Ok(false);
}
```

- [ ] **Step 3: 添加测试**

```rust
#[tokio::test]
async fn repaste_expands_existing_marker() {
    let mut controller = InteractiveController::new_for_test(...);
    controller.handle_input_event(InputEvent::Paste("a\n".repeat(15))).await.unwrap();
    controller.handle_input_event(InputEvent::MoveEnd).await.unwrap();
    // simulate Ctrl+V on the marker
    controller.handle_input_event(InputEvent::Key(KeyId::new("ctrl+v").unwrap())).await.unwrap();
    assert!(controller.chrome().prompt().text.contains("a\na\n"));
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo run -p xtask -- test -p neo-agent interactive 2>&1 | tail -60`

### Task 2.8：图片粘贴键绑定

**Files:**
- Modify: `crates/neo-tui/src/input.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: 在 keybindings 中新增 `PasteImage` action**

```rust
pub enum KeybindingAction {
    // ... existing variants
    PasteImage,
}
```

- [ ] **Step 2: 配置默认绑定**

在 `default_keybinding_definitions` 或 `editor_keybinding_definitions` 中：

```rust
#[cfg(target_os = "windows")]
definition(KeybindingAction::PasteImage, &["alt+v"], "Paste image from clipboard"),
#[cfg(not(target_os = "windows"))]
definition(KeybindingAction::PasteImage, &["ctrl+v"], "Paste image from clipboard"),
```

- [ ] **Step 3: 在 `InteractiveController::handle_keybinding_action` 中处理 `PasteImage`**

```rust
KeybindingAction::PasteImage => {
    self.handle_paste_image().await?;
    return Ok(false);
}
```

- [ ] **Step 4: 运行 keybindings 测试**

Run: `cargo run -p xtask -- test -p neo-tui input 2>&1 | tail -60`

### Task 2.9：跨平台剪贴板图片读取

**Files:**
- Create: `crates/neo-agent/src/clipboard.rs`

- [ ] **Step 1: 定义 trait 与返回类型**

```rust
use std::path::PathBuf;

pub struct ClipboardImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("no image in clipboard")]
    NoImage,
    #[error("unsupported image format")]
    UnsupportedFormat,
    #[error("clipboard read failed: {0}")]
    ReadFailed(String),
}

pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
    #[cfg(target_os = "macos")]
    return macos::read_clipboard_image();
    #[cfg(target_os = "linux")]
    return linux::read_clipboard_image();
    #[cfg(target_os = "windows")]
    return windows::read_clipboard_image();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err(ClipboardError::ReadFailed("unsupported platform".into()));
}
```

- [ ] **Step 2: 实现 macOS 读取**

```rust
#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        // Use pngpaste if available, otherwise osascript/AppKit.
        if let Ok(out) = Command::new("pngpaste").arg("-").output() {
            if out.status.success() && !out.stdout.is_empty() {
                return Ok(ClipboardImage { bytes: out.stdout, mime_type: "image/png".into() });
            }
        }
        // Fallback: write to temp file with osascript.
        let tmp = std::env::temp_dir().join(format!("neo-clipboard-{}.png", std::process::id()));
        let script = format!(
            "ObjC.import('AppKit'); var pb = $.NSPasteboard.generalPasteboard; var data = pb.dataForType($.NSPasteboardTypePNG); data.writeToFileAtomically({:?}, true);",
            tmp.to_str().unwrap_or("")
        );
        let out = Command::new("osascript").arg("-e").arg(script).output()
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        if !out.status.success() {
            return Err(ClipboardError::NoImage);
        }
        let bytes = std::fs::read(&tmp).map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let _ = std::fs::remove_file(&tmp);
        if bytes.is_empty() {
            return Err(ClipboardError::NoImage);
        }
        Ok(ClipboardImage { bytes, mime_type: "image/png".into() })
    }
}
```

- [ ] **Step 3: 实现 Linux 读取**

```rust
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        // Try wl-paste first, then xclip.
        let candidates: Vec<(&str, Vec<&str>, &str)> = vec![
            ("wl-paste", vec!["--type", "image/png"], "image/png"),
            ("xclip", vec!["-selection", "clipboard", "-t", "image/png", "-o"], "image/png"),
        ];
        for (cmd, args, mime) in candidates {
            if let Ok(out) = Command::new(cmd).args(&args).output() {
                if out.status.success() && !out.stdout.is_empty() {
                    return Ok(ClipboardImage { bytes: out.stdout, mime_type: mime.into() });
                }
            }
        }
        Err(ClipboardError::NoImage)
    }
}
```

- [ ] **Step 4: 实现 Windows 读取**

```rust
#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::process::Command;

    pub fn read_clipboard_image() -> Result<ClipboardImage, ClipboardError> {
        let tmp = std::env::temp_dir().join(format!("neo-clipboard-{}.png", std::process::id()));
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $img = [Windows.Forms.Clipboard]::GetImage(); if ($img -eq $null) {{ exit 1 }}; $img.Save({:?}, [System.Drawing.Imaging.ImageFormat]::Png);",
            tmp.to_str().unwrap_or("")
        );
        let out = Command::new("powershell.exe").args(["-NoProfile", "-Command", &script]).output()
            .map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        if !out.status.success() {
            return Err(ClipboardError::NoImage);
        }
        let bytes = std::fs::read(&tmp).map_err(|e| ClipboardError::ReadFailed(e.to_string()))?;
        let _ = std::fs::remove_file(&tmp);
        Ok(ClipboardImage { bytes, mime_type: "image/png".into() })
    }
}
```

- [ ] **Step 5: 添加到 `neo-agent` crate**

在 `crates/neo-agent/src/lib.rs`（或 `main.rs`）中：

```rust
mod clipboard;
```

- [ ] **Step 6: 运行编译**

Run: `cargo run -p xtask -- check -p neo-agent 2>&1 | tail -60`

### Task 2.10：图片保存为 blob

**Files:**
- Create: `crates/neo-agent/src/image_blob.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: 定义 blob 保存函数**

```rust
use sha2::{Digest, Sha256};
use std::path::Path;

pub struct BlobRef {
    pub sha256: String,
    pub mime_type: String,
    pub path: std::path::PathBuf,
}

pub fn save_image_blob(
    session_dir: &Path,
    bytes: &[u8],
    mime_type: &str,
) -> anyhow::Result<BlobRef> {
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let ext = mime_to_extension(mime_type).unwrap_or("bin");
    let blob_dir = session_dir.join("blobs");
    std::fs::create_dir_all(&blob_dir)?;
    let path = blob_dir.join(format!("{}.{}", sha256, ext));
    if !path.exists() {
        std::fs::write(&path, bytes)?;
    }
    Ok(BlobRef { sha256, mime_type: mime_type.into(), path })
}

fn mime_to_extension(mime: &str) -> Option<&str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        _ => None,
    }
}
```

- [ ] **Step 2: 添加 sha2 依赖**

检查 `Cargo.toml` 是否已有 `sha2`；没有则添加：

```toml
sha2 = "0.10"
```

- [ ] **Step 3: 运行编译**

Run: `cargo run -p xtask -- check -p neo-agent 2>&1 | tail -60`

### Task 2.11：处理图片粘贴键流程

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: 实现 `handle_paste_image`**

```rust
async fn handle_paste_image(&mut self) -> Result<()> {
    // 1. Check model capability.
    if !self.model_supports_images() {
        self.push_status("此模型不支持图片识别");
        return Ok(());
    }

    // 2. Try to read image from clipboard.
    let image = match crate::clipboard::read_clipboard_image() {
        Ok(img) => img,
        Err(crate::clipboard::ClipboardError::NoImage) => {
            // Fall through to normal paste.
            return self.simulate_text_paste().await;
        }
        Err(e) => {
            self.push_status(&format!("Clipboard image read failed: {e}"));
            return Ok(());
        }
    };

    // 3. Detect image dimensions.
    let (width, height) = detect_image_dimensions(&image.bytes, &image.mime_type)
        .unwrap_or((0, 0));

    // 4. Save blob.
    let session_dir = self.active_session_dir()
        .ok_or_else(|| anyhow::anyhow!("no active session"))?;
    let blob = crate::image_blob::save_image_blob(&session_dir, &image.bytes, &image.mime_type)?;

    // 5. Register attachment and insert placeholder.
    let id = self.image_attachment_store.add(blob.sha256, image.mime_type, width, height);
    let placeholder = format!("[image #{} ({}x{})]", id, width, height);
    self.apply_prompt_edit(PromptEdit::Insert(&placeholder));

    Ok(())
}
```

- [ ] **Step 2: 实现 `model_supports_images`**

```rust
fn model_supports_images(&self) -> bool {
    self.active_model
        .as_ref()
        .and_then(|m| self.model_capabilities.get(&m.alias))
        .map(|c| c.images)
        .unwrap_or(false)
}
```

- [ ] **Step 3: 在 `controller_for_config` 中缓存模型能力**

构造 model registry 时，把每个模型的 capabilities 缓存到 `InteractiveController.model_capabilities: HashMap<String, ModelCapabilities>`。

- [ ] **Step 4: 运行编译与 focused 测试**

Run: `cargo run -p xtask -- check -p neo-agent 2>&1 | tail -60`
Run: `cargo run -p xtask -- test -p neo-agent interactive 2>&1 | tail -60`

### Task 2.12：图片尺寸检测

**Files:**
- Create: `crates/neo-agent/src/image_blob.rs` 或独立模块

- [ ] **Step 1: 实现最小尺寸检测**

PNG：

```rust
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 { return None; }
    if &bytes[0..8] != b"\x89PNG\r\n\x1a\n" { return None; }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}
```

JPEG：使用 `image` crate 或简单解析 SOF marker。

- [ ] **Step 2: 添加 image crate 依赖（可选）**

如果仓库已有 `image` crate，直接用；否则为了减小依赖，先实现 PNG/JPEG/WebP/GIF 的最小解析器。

- [ ] **Step 3: 运行测试**

Run: `cargo run -p xtask -- test -p neo-agent image_blob 2>&1 | tail -40`

### Task 2.13：扩展 `AgentMessage` / `ImageRef` 支持 Blob

**Files:**
- Modify: `crates/neo-agent-core/src/messages.rs`

- [ ] **Step 1: 给 `ImageRef` 新增 `Blob` 变体**

```rust
pub enum ImageRef {
    Base64(String),
    Url(String),
    Blob(String), // sha256
}
```

- [ ] **Step 2: 序列化/反序列化适配**

在 `Content` 的 serde 实现中，把 `ImageRef::Blob(sha256)` 序列化为：

```json
{
  "mime_type": "image/png",
  "data": { "blob": "<sha256>" }
}
```

- [ ] **Step 3: 添加 `AgentMessage::user_parts`**

```rust
impl AgentMessage {
    pub fn user_parts(content: Vec<Content>) -> Self {
        Self::User { content }
    }
}
```

- [ ] **Step 4: 运行编译**

Run: `cargo run -p xtask -- check -p neo-agent-core 2>&1 | tail -60`

### Task 2.14：把占位符展开为 `Vec<Content>`

**Files:**
- Create: `crates/neo-agent/src/prompt_parts.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: 定义 `expand_prompt_markers`**

```rust
use neo_agent_core::{Content, ImageRef};
use neo_tui::paste::{parse_markers, Marker};

pub fn expand_prompt_markers(
    text: &str,
    paste_store: &HashMap<usize, String>,
    image_store: &neo_tui::paste::ImageAttachmentStore,
) -> Vec<Content> {
    let mut parts = Vec::new();
    let mut cursor = 0;
    for (start, marker) in parse_markers(text) {
        if start > cursor {
            parts.push(Content::Text { text: text[cursor..start].into() });
        }
        match marker {
            Marker::Paste { id, .. } => {
                if let Some(original) = paste_store.get(&id) {
                    parts.push(Content::Text { text: original.clone() });
                }
            }
            Marker::Image { id, .. } => {
                if let Some(att) = image_store.get(id) {
                    parts.push(Content::Image {
                        mime_type: att.mime_type.clone(),
                        data: ImageRef::Blob(att.sha256.clone()),
                    });
                }
            }
        }
        cursor = start + marker.as_placeholder().len();
    }
    if cursor < text.len() {
        parts.push(Content::Text { text: text[cursor..].into() });
    }
    parts
}
```

- [ ] **Step 2: 合并相邻文本段**

```rust
fn coalesce_text_parts(parts: Vec<Content>) -> Vec<Content> {
    let mut out = Vec::new();
    for part in parts {
        match (out.last_mut(), &part) {
            (Some(Content::Text { text: last }), Content::Text { text: next }) => {
                last.push_str(next);
            }
            _ => out.push(part),
        }
    }
    out
}
```

- [ ] **Step 3: 添加测试**

```rust
#[test]
fn expand_mixed_text_and_image() {
    let mut image_store = ImageAttachmentStore::new();
    image_store.add("abc".into(), "image/png".into(), 100, 100);
    let parts = expand_prompt_markers("hello [image #1 (100x100)] world", &HashMap::new(), &image_store);
    assert_eq!(parts.len(), 3);
    assert!(matches!(parts[1], Content::Image { .. }));
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo run -p xtask -- test -p neo-agent prompt_parts 2>&1 | tail -40`

### Task 2.15：改造 `TurnRequest.prompt` 为 `Vec<Content>`

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-agent/src/modes/run.rs`

- [ ] **Step 1: 修改 `TurnRequest`**

```rust
pub(crate) struct TurnRequest {
    pub prompt: Vec<Content>,
    // ...
}
```

- [ ] **Step 2: 修改 `run_prompt_streaming` / `run_prompt_in_session_streaming` 签名**

```rust
pub async fn run_prompt_streaming(
    prompt: &[Content],
    // ...
) -> anyhow::Result<PromptTurn>
```

- [ ] **Step 3: 修改 `prepare_new_streaming_turn`**

不再 `prompt.join(" ")`；直接把 `Vec<Content>` 写入 `AgentEvent::MessageAppended`。

- [ ] **Step 4: 修改 `append_user_event`**

```rust
async fn append_user_event(
    content: Vec<Content>,
    writer: &mut SessionEventWriter<'_>,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let user_message = AgentMessage::User { content };
    // ...
}
```

- [ ] **Step 5: 运行编译**

Run: `cargo run -p xtask -- check -p neo-agent -p neo-agent-core 2>&1 | tail -80`

### Task 2.16：Runtime 中把 `ImageRef::Blob` 解析为 base64

**Files:**
- Modify: `crates/neo-agent-core/src/runtime.rs`
- Modify: `crates/neo-agent-core/src/messages.rs`

- [ ] **Step 1: 在 `AgentConfig` 中传入 session 目录**

新增：

```rust
pub struct AgentConfig {
    // ...
    pub session_directory: Option<PathBuf>,
}
```

- [ ] **Step 2: 在 `to_chat_message` 之前把 Blob 转成 Base64**

新增函数：

```rust
fn resolve_image_blobs(content: Vec<Content>, session_dir: Option<&Path>) -> Vec<Content> {
    content.into_iter().map(|c| match c {
        Content::Image { mime_type, data: ImageRef::Blob(sha256) } => {
            let path = session_dir.map(|d| d.join("blobs").join(format!("{}.*", sha256)));
            // read first matching file
            let bytes = path.and_then(read_blob_bytes).unwrap_or_default();
            Content::Image {
                mime_type,
                data: ImageRef::Base64(base64::encode(&bytes)),
            }
        }
        other => other,
    }).collect()
}
```

- [ ] **Step 3: 在 `chat_request` 中调用**

把 `context.messages` 中的 user/assistant/tool 消息在转 `ChatMessage` 前先 resolve blobs。

- [ ] **Step 4: 运行 runtime 测试**

Run: `cargo run -p xtask -- test -p neo-agent-core runtime 2>&1 | tail -80`

### Task 2.17：Transcript 支持图文混合用户消息

**Files:**
- Modify: `crates/neo-tui/src/transcript/entry.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`

- [ ] **Step 1: 新增 `UserMessagePart` 与 `TranscriptEntry::UserMessageParts`**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserMessagePart {
    Text(String),
    Image { sha256: String, mime_type: String, width: u32, height: u32 },
}

pub enum TranscriptEntry {
    // ...
    UserMessageParts(Vec<UserMessagePart>),
}
```

- [ ] **Step 2: 添加构造函数**

```rust
impl TranscriptEntry {
    pub fn user_message_parts(parts: Vec<UserMessagePart>) -> Self {
        Self::UserMessageParts(parts)
    }
}
```

- [ ] **Step 3: 修改 `push_user_message` 使其接受 parts**

```rust
pub fn push_user_message_parts(&mut self, parts: Vec<UserMessagePart>) {
    self.push_transcript(TranscriptEntry::user_message_parts(parts));
}
```

- [ ] **Step 4: 实现 `render_user_message_parts`**

文本部分按现有样式渲染；图片部分调用 `InlineImage::bytes(...)` 和 `render_inline_image`，生成 Kitty/iTerm2/Sixel 序列，不支持时渲染占位符。

- [ ] **Step 5: 运行测试**

Run: `cargo run -p xtask -- test -p neo-tui transcript 2>&1 | tail -80`

### Task 2.18：把用户消息以 parts 形式推入 transcript

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-agent/src/modes/run.rs`

- [ ] **Step 1: 在 `start_turn_with_prompt` 中把 `Vec<Content>` 转成 `Vec<UserMessagePart>`**

```rust
let parts: Vec<UserMessagePart> = prompt.iter().map(|c| match c {
    Content::Text { text } => UserMessagePart::Text(text.clone()),
    Content::Image { mime_type, data } => {
        let sha256 = match data {
            ImageRef::Blob(s) => s.clone(),
            _ => String::new(),
        };
        UserMessagePart::Image { sha256, mime_type: mime_type.clone(), width: 0, height: 0 }
    }
}).collect();
self.tui.transcript_mut().push_user_message_parts(parts);
```

> 实际 width/height 应从 attachment store 中获取。

- [ ] **Step 2: 修改 `submit_current_prompt` 中的 prompt 提交**

在调用 `start_turn_with_prompt` 之前，用 `expand_prompt_markers` 把 `PromptState.text` 展开为 `Vec<Content>`。

- [ ] **Step 3: 运行集成测试**

Run: `cargo run -p xtask -- test -p neo-agent interactive 2>&1 | tail -80`

### Task 2.19：JSONL 反序列化时恢复占位符

**Files:**
- Modify: `crates/neo-agent-core/src/messages.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`（apply_agent_event）

- [ ] **Step 1: 为 `AgentMessage::User` 增加占位符显示方法**

```rust
impl AgentMessage {
    pub fn to_user_message_parts(&self, attachment_store: &ImageAttachmentStore) -> Vec<UserMessagePart> {
        match self {
            Self::User { content } => content.iter().map(|c| match c {
                Content::Text { text } => UserMessagePart::Text(text.clone()),
                Content::Image { mime_type, data } => {
                    let sha256 = match data {
                        ImageRef::Blob(s) => s.clone(),
                        ImageRef::Base64(_) | ImageRef::Url(_) => String::new(),
                    };
                    // 从 store 找尺寸
                    let (width, height) = attachment_store.find_by_sha256(&sha256)
                        .map(|a| (a.width, a.height))
                        .unwrap_or((0, 0));
                    UserMessagePart::Image { sha256, mime_type: mime_type.clone(), width, height }
                }
                _ => UserMessagePart::Text(String::new()),
            }).collect(),
            _ => Vec::new(),
        }
    }
}
```

- [ ] **Step 2: 在 apply `MessageAppended` 时调用**

如果 user message 包含图片，用 `to_user_message_parts` 推到 transcript。

- [ ] **Step 3: 运行测试**

Run: `cargo run -p xtask -- test -p neo-tui session_jsonl 2>&1 | tail -80`

### Task 2.20：缺失 blob 时回退到占位符文本

**Files:**
- Modify: `crates/neo-tui/src/transcript/entry.rs`

- [ ] **Step 1: 在渲染 `UserMessagePart::Image` 时检查 blob 存在性**

由于 `entry.rs` 不应直接访问文件系统，在渲染前由 `pane.rs` 预先把 `UserMessagePart::Image` 转成渲染用的「InlineImage 或占位符文本」结构。

推荐新增 `RenderedUserMessagePart`：

```rust
enum RenderedUserMessagePart {
    Text(String),
    InlineImage(InlineImage),
    Placeholder(String),
}
```

- [ ] **Step 2: 在 `TranscriptPane` 中预处理**

```rust
fn resolve_user_parts(
    &self,
    parts: &[UserMessagePart],
    session_dir: Option<&Path>,
) -> Vec<RenderedUserMessagePart> {
    parts.iter().map(|p| match p {
        UserMessagePart::Text(t) => RenderedUserMessagePart::Text(t.clone()),
        UserMessagePart::Image { sha256, mime_type, width, height } => {
            let blob = session_dir.and_then(|d| find_blob_file(d, sha256));
            match blob.and_then(|path| std::fs::read(&path).ok()) {
                Some(bytes) => {
                    let image = InlineImage::bytes(sha256, mime_type, bytes, Some(format!("image # ({width}x{height})")), ImageSource::Clipboard);
                    RenderedUserMessagePart::InlineImage(image)
                }
                None => RenderedUserMessagePart::Placeholder(format!("[image ({width}x{height})]")),
            }
        }
    }).collect()
}
```

- [ ] **Step 3: 运行测试**

Run: `cargo run -p xtask -- test -p neo-tui image_protocols 2>&1 | tail -60`

### Task 2.21：集成测试覆盖图片粘贴

**Files:**
- Modify: `crates/neo-agent/tests/cli_commands.rs` 或新增 `crates/neo-agent/tests/paste_image.rs`

- [ ] **Step 1: 编写 mock 图片粘贴测试**

```rust
#[tokio::test]
async fn image_placeholder_submitted_as_user_image_content() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config_in(&temp);
    // 构造一个支持图片的模型
    let mut controller = InteractiveController::new_for_test(...);
    // 注入假剪贴板读取
    // 触发 PasteImage
    // 验证 prompt 包含 [image #1 ...]
    // 提交
    // 验证 runtime 收到 Content::Image
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo run -p xtask -- test -p neo-agent paste_image 2>&1 | tail -80`

---

## 验证与收尾

### Task 3.1：格式化与 clippy

- [ ] **Step 1: 运行 fmt**

Run: `cargo fmt --all`

- [ ] **Step 2: 运行 clippy**

Run: `cargo run -p xtask -- check --workspace 2>&1 | tail -80`

### Task 3.2：Focused 测试

- [ ] **Step 1: neo-tui**

Run: `cargo run -p xtask -- test -p neo-tui 2>&1 | tail -60`

- [ ] **Step 2: neo-agent-core**

Run: `cargo run -p xtask -- test -p neo-agent-core 2>&1 | tail -60`

- [ ] **Step 3: neo-agent**

Run: `cargo run -p xtask -- test -p neo-agent 2>&1 | tail -60`

### Task 3.3：覆盖率与 CRAP（仅当生产代码行为改变时）

- [ ] **Step 1: LCOV**

Run: `cargo run -p xtask -- coverage 2>&1 | tail -60`

- [ ] **Step 2: CRAP**

Run: `cargo run -p xtask -- crap 2>&1 | tail -60`

### Task 3.4：迁移脚本测试

- [ ] **Step 1: 构造旧布局 fixture**

```bash
mkdir -p /tmp/neo-migrate-fixture/wd_test_123/
echo '{"event":"MessageAppended"...}' > /tmp/neo-migrate-fixture/wd_test_123/session_00000000-0000-4000-8000-000000000001.jsonl
mkdir -p /tmp/neo-migrate-fixture/wd_test_123/plans
touch /tmp/neo-migrate-fixture/wd_test_123/plans/session_00000000-0000-4000-8000-000000000001-plan.md
```

- [ ] **Step 2: dry-run**

Run: `python scripts/migrate-sessions.py /tmp/neo-migrate-fixture --dry-run`
Expected: 输出 would move 计划

- [ ] **Step 3: 实际迁移**

Run: `python scripts/migrate-sessions.py /tmp/neo-migrate-fixture`
Expected: 输出 sessions_moved: 1, plans_moved: 1

- [ ] **Step 4: 验证目录结构**

Run: `find /tmp/neo-migrate-fixture -type f`
Expected: `wd_test_123/session_00000000-.../transcript.jsonl` 和 `wd_test_123/session_00000000-.../plans/...`

### Task 3.5：最终 CI Gate

- [ ] **Step 1: 运行 xtask ci**

Run: `cargo run -p xtask -- ci 2>&1 | tail -80`
Expected: 全部通过

---

## 自检

### Spec 覆盖检查

| Spec 要求 | 对应 Task |
|-----------|-----------|
| Session 目录化：`session_<uuid>/transcript.jsonl` | 1.2, 1.3, 1.4 |
| `plans/` 迁移到 session 目录 | 1.6 |
| `prompt-history.jsonl` 留在 bucket 并适配新路径 | 1.5 |
| 一次性迁移脚本 | 1.1 |
| 多行文本折叠 `[paste +N lines]` / `[paste N chars]` | 2.6 |
| 图片占位符 `[image #N (WxH)]` | 2.11 |
| 占位符整体删除（backspace 选中再删除） | 2.4 |
| 图片 blob 存储（sha256 去重） | 2.10 |
| 图片粘贴键 `Ctrl+V`/`Alt+V` | 2.8 |
| model 不支持图片时 footer 提示 | 2.11 |
| 提交时展开占位符为 `Vec<Content>` | 2.14, 2.15 |
| JSONL 序列化 `ImageRef::Blob` | 2.13 |
| transcript 图文混合渲染 | 2.17, 2.18, 2.20 |
| 缺失 blob 时回退占位符 | 2.20 |

### Placeholder 扫描

计划中未使用：TBD, TODO, implement later, fill in details, "Add appropriate error handling", "Write tests for the above", "Similar to Task N"。

### 类型一致性检查

- `ImageAttachmentStore` 在 `neo-tui/src/paste.rs` 中定义，后续在 `neo-agent` 中通过 `neo_tui::paste::ImageAttachmentStore` 引用。
- `ImageRef::Blob(String)` 在 `neo-agent-core` 中定义，String 表示 sha256。
- `TurnRequest.prompt` 统一为 `Vec<Content>`。
- `UserMessagePart` 在 `neo-tui` 中定义，用于 transcript 渲染。

## 执行交接

Plan complete and saved to `docs/superpowers/plans/2026-06-23-paste-markers-and-session-blobs-implementation-plan.md`.

Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
