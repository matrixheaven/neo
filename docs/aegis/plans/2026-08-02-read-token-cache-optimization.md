# Read 工具 Token 缓存优化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use aegis:subagent-driven-development (recommended) or aegis:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 降低 Read 工具造成的 uncached input tokens 总量与 per-tool 方差：旧的大 Read 结果在模型输入中被确定性剪为头+尾片段（snip），同内容重复读取折叠为指向更早结果的短提示（dedup），模型被引导定点读且默认窗口降到 400 行，压缩输入投影保留头尾而非全删。基线数据见 spec §2（run `20260801-162932-d0877b`：Read 占 uncached 37%、variance 13.85M 全场最高）。

**Architecture:** 复用现有 `compaction/projection.rs` 的 ephemeral 变换管道（不写会话历史、不引入归档），把 `ProjectionPlan` 从"全删 omission"扩展为三态（dedup → snip → omission）。`Tool` trait 增加默认方法 `snip_hint()`，`ToolRegistry::register` 把几何登记进进程级 name→hint 表，投影按工具名查询（策略随工具定义走，同 reasonix 意图）。触发：`turn_loop::request_projection_plan` 的 `enabled = micro_enabled || snip_enabled`，`prepare_model_request` 的 `NoAction` 分支返回带 snip 的 request plan（controller 零改动）。

**Tech Stack:** Rust 2024 / edition 2024, `neo-agent-core` + `neo-agent`, `cargo nextest`, serde 兼容配置, 纯字符串/哈希实现（无平台相关代码，`unsafe` 禁）。

**Baseline/Authority Refs:**
- `docs/aegis/specs/2026-08-02-read-token-cache-optimization-design.md`（本 plan 的 spec，含证据与决策）
- `docs/aegis/specs/2026-08-01-deepseek-cache-probe-design.md`
- `target/cache-probe/20260801-162932-d0877b/report.json`
- `.references/reasonix/internal/tool/builtin/readfile.go`、`internal/agent/prune.go`、`internal/tool/tool.go`
- `crates/neo-agent-core/src/compaction/projection.rs`、`runtime/turn_loop.rs`、`runtime/config.rs`、`tools/read.rs`、`tools/mod.rs`

**Compatibility Boundary:**
- 不改：会话 JSONL 格式、wire 协议、tool schema、`AgentMessage` 变体、权限/plan 模式、`micro_enabled` 既有 omission 语义（非 hint 工具保持原样）。
- 不改：`.references/*`、`tools/cache_probe.py`。
- 新配置键带 `#[serde(default)]`，旧 `config.toml` 不破坏。
- projection 仍为 ephemeral：TUI/transcript 显示全文，模型输入显示压缩版。
- `config.compaction == None`（无上下文窗口）时 snip/dedup 不生效。

**TDD Route:**
```text
TDD Route:
- Mode: off
- Decision: skipped
- Strict authority: not applicable（无显式 strict 请求）
- Test posture: 每任务的最小回归/新增单测（post-change）
- Reason: 行为变更由纯函数承载，直接写最小实现 + 针对性单测即可
- Verification: 每任务给出精确 nextest 过滤命令
```

## Review Notes

- `request_projection_plan` 变更后 `NoAction` 分支返回的 plan 使 projection 在 `snip_enabled` 时每请求启用；snip/dedup 均为 `(messages, plan)` 纯函数，确定性输出，前缀只在首次改写时破坏一次（spec §4.1 经济学已论证）。
- `ProjectionPlan` 是 `Copy` 结构体，新增字段后所有字面量构造点由 `cargo check` 强制暴露；任务 B1 负责一次清点。
- 测试中的 hint 登记走真实注册路径（`ToolRegistry::register` + 进程级表），登记幂等；不同测试并行执行安全（`OnceLock` + `RwLock`）。
- 不新增 trigger 波段、不新增归档目录、不做 durable 历史改写。

## Policy Notes

- 禁止本 plan 直接执行 git 变更类命令（`reset`/`checkout --`/`stash`/`rebase`/`clean`/`rm`/`amend`/force push 等），除非用户对具体命令显式授权。`git add`/`git commit` 按仓库 AGENTS.md 工作循环在每任务验证通过后自主执行（conventional commit）。
- 验证必须用每任务给出的精确命令，禁止用宽泛 `cargo test` 作为完成证据。
- 共享 worktree 可能很脏：只改本 plan 列出的文件，不修无关失败。

## File Map

| 文件 | 动作 | 内容 |
| --- | --- | --- |
| `crates/neo-agent-core/src/tools/mod.rs` | 修改 | `SnipHint` 结构体、`Tool::snip_hint()` 默认方法、进程级 `snip_hint_for` 表 + `register` 挂钩、测试 |
| `crates/neo-agent-core/src/tools/read.rs` | 修改 | `ReadTool::snip_hint()`、`DEFAULT_LINES=400`、描述文本、测试 |
| `crates/neo-agent-core/src/compaction/projection.rs` | 修改 | `ProjectionPlan` 3 新字段、dedup/snip/omission 三态、测试 |
| `crates/neo-agent-core/src/runtime/turn_loop.rs` | 修改 | `request_projection_plan`、`prepare_model_request` NoAction、测试 |
| `crates/neo-agent-core/src/runtime/config.rs` | 修改 | `CompactionSettings` 3 新字段 + `new()` 默认值 |
| `crates/neo-agent-core/src/runtime/agent.rs` | 修改 | 测试字面量补齐 3 字段 |
| `crates/neo-agent/src/config/mod.rs` | 修改 | `RuntimeCompactionConfig` 3 新字段（serde default）+ `Default` |
| `crates/neo-agent/src/modes/run/runtime/agent.rs` | 修改 | `RuntimeCompactionConfig → CompactionSettings` 映射补齐 |
| `crates/neo-agent/src/modes/run/mod.rs` | 修改 | 测试字面量补齐 |
| `docs/en/configuration/config-files.md`、`docs/zh/configuration/config-files.md` | 修改 | `[runtime.compaction]` 新行 + micro 提示修订 |
| `docs/aegis/INDEX.md` | 修改 | 追加 spec + plan 两行 |

---

## Task A1 — `SnipHint` 能力 + 进程级登记表（tools/mod.rs）

**Files:** `crates/neo-agent-core/src/tools/mod.rs`（修改）
**Why:** 投影需要按工具名拿到"旧结果如何压缩"的几何，策略必须随工具定义走（rename 同步），且不把 registry 透传进 `chat_request`。
**Change Necessity:** 代码变更；最小边界 = `tools/mod.rs` 一个文件 + trait 默认方法（零破坏）。
**Impact/Compatibility:** `Tool` trait 增加带默认实现的方法，所有现有 implementer 无需改动。
**Verification:**
```bash
cargo nextest run -p neo-agent-core --lib tools::tests::snip_hint_registration_and_lookup --exact
cargo fmt --all --check
```

**Steps:**

1. 顶部导入补齐：
```rust
use std::collections::HashMap;
use std::sync::RwLock;
```
（`OnceLock` 已在 `tools/mod.rs:1006` 导入；如与现有导入重复则合并。）

2. 在 `Tool` trait（约 line 684）之前新增：
```rust
/// Geometry for shortening stale oversized tool results in the model input.
/// The policy travels with the tool definition: `ToolRegistry::register`
/// records it once, and the request-time projection looks it up by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnipHint {
    /// Lines kept from the start of the result.
    pub head_lines: usize,
    /// Lines kept from the end of the result.
    pub tail_lines: usize,
    /// Runes kept from the start when the result is one giant line.
    pub head_chars: usize,
    /// Runes kept from the end when the result is one giant line.
    pub tail_chars: usize,
}

/// Process-global name -> hint map, populated at registration so a tool
/// rename updates the policy in the same place (mirrors reasonix's
/// "policy travels with the tool" design without threading the registry
/// through request building).
static SNIP_HINTS: OnceLock<RwLock<HashMap<String, SnipHint>>> = OnceLock::new();

/// Snip geometry for `name`, if the tool opted in.
#[must_use]
pub fn snip_hint_for(name: &str) -> Option<SnipHint> {
    SNIP_HINTS
        .get()
        .and_then(|map| map.read().ok())
        .and_then(|map| map.get(name).copied())
}

fn register_snip_hint(name: &str, hint: Option<SnipHint>) {
    if let Some(hint) = hint {
        if let Ok(mut map) = SNIP_HINTS.get_or_init(|| RwLock::new(HashMap::new())).write() {
            map.insert(name.to_owned(), hint);
        }
    }
}
```

3. `Tool` trait 增加默认方法：
```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a>;

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_owned(),
            description: self.description().to_owned(),
            input_schema: self.input_schema(),
        }
    }

    /// Geometry for shortening stale oversized results this tool produced in
    /// the model input. `None` (default) keeps the historical full-omission
    /// marker behavior of micro compaction.
    fn snip_hint(&self) -> Option<SnipHint> {
        None
    }
}
```

4. `ToolRegistry::register`（约 line 776）在 `self.tools.insert(...)` 之前加一行：
```rust
        register_snip_hint(tool.name(), tool.snip_hint());
```

5. 在 `tools/mod.rs` 的 `#[cfg(test)] mod tests`（line 1096 起）新增测试：
```rust
    #[test]
    fn snip_hint_registration_and_lookup() {
        struct Hinted {
            hint: SnipHint,
        }
        impl Tool for Hinted {
            fn name(&self) -> &str { "Hinted" }
            fn description(&self) -> &str { "hinted" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
                Box::pin(async { Ok(ToolResult::ok("ok")) })
            }
            fn snip_hint(&self) -> Option<SnipHint> { Some(self.hint) }
        }
        struct Plain;
        impl Tool for Plain {
            fn name(&self) -> &str { "Plain" }
            fn description(&self) -> &str { "plain" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
                Box::pin(async { Ok(ToolResult::ok("ok")) })
            }
        }

        let hint = SnipHint { head_lines: 120, tail_lines: 12, head_chars: 12_000, tail_chars: 2_000 };
        let mut registry = ToolRegistry::default();
        registry.register(Hinted { hint });
        registry.register(Plain);

        assert_eq!(snip_hint_for("Hinted"), Some(hint));
        assert_eq!(snip_hint_for("Plain"), None);
        assert_eq!(snip_hint_for("Missing"), None);
    }
```
（`ToolContext`/`ToolResult`/`ToolFuture` 已在测试模块使用；如需补 `use super::*;` 已有。）

---

## Task A2 — `ReadTool::snip_hint()` 实现（read.rs）

**Files:** `crates/neo-agent-core/src/tools/read.rs`（修改）
**Why:** Read 是唯一声明几何的工具；几何 = reasonix readfile.go:70-72 同款。
**Change Necessity:** 代码变更；最小边界 = 一个方法。
**Impact/Compatibility:** 无。
**Verification:**
```bash
cargo nextest run -p neo-agent-core --lib tools::read::tests::reads_whole_small_file --exact
```

**Steps:**

1. `use super::{...}` 导入行加入 `SnipHint`：
```rust
use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, SnipHint, parse_input, schema};
```
2. `impl Tool for ReadTool` 内（`fn execute` 之后）新增：
```rust
    fn snip_hint(&self) -> Option<SnipHint> {
        Some(SnipHint {
            head_lines: 120,
            tail_lines: 12,
            head_chars: 12_000,
            tail_chars: 2_000,
        })
    }
```

---

## Task C1 — `CompactionSettings` 新字段（core config.rs）

**Files:** `crates/neo-agent-core/src/runtime/config.rs`（修改）
**Why:** `request_projection_plan`（Task B4）需要读 snip 开关/门槛/保护带。
**Change Necessity:** 代码变更；最小边界 = 结构体 + `new()`。
**Impact/Compatibility:** `CompactionSettings` 为 struct-literal 构造（无 `Default`），其余字面量由 Task C3 补齐；`new()` 默认 `snip_enabled=false`（修订 2026-08-02：前缀改写破坏 provider 缓存，付费模型默认关）。
**Verification:**
```bash
cargo nextest run -p neo-agent-core --lib runtime::config::tests::compaction_settings_new_enables_snip --exact
```

**Steps:**

1. `pub struct CompactionSettings`（line 667）在 `micro_keep_recent` 之后新增：
```rust
    /// Whether stale oversized tool results are shortened to head/tail in the
    /// model input (deterministic; the prefix is rewritten once per result,
    /// then stable). Independent of `micro_enabled`.
    pub snip_enabled: bool,
    /// Minimum estimated tool-result tokens before a stale result is snipped.
    pub snip_min_tokens: usize,
    /// Number of newest messages exempt from snip.
    pub snip_keep_recent: usize,
```
2. `CompactionSettings::new`（line 690）在 `micro_keep_recent: 20,` 之后新增：
```rust
            snip_enabled: false,  # 修订 2026-08-02: 前缀改写会破坏 provider 缓存, 付费模型默认关
            snip_min_tokens: 1_000,
            snip_keep_recent: 16,
```
3. `config.rs` 测试模块新增：
```rust
    #[test]
    fn compaction_settings_new_disables_snip_by_default() {
        let settings = CompactionSettings::new(100_000, 4);
        assert!(!settings.snip_enabled);
        assert_eq!(settings.snip_min_tokens, 1_000);
        assert_eq!(settings.snip_keep_recent, 16);
    }
```

---

## Task C2 — `RuntimeCompactionConfig` 新字段（neo-agent config）

**Files:** `crates/neo-agent/src/config/mod.rs`（修改）
**Why:** 用户 `config.toml` 的 `[runtime.compaction]` 载体。
**Change Necessity:** 代码变更；最小边界 = 结构体 + `Default`。
**Impact/Compatibility:** 新字段带 `#[serde(default)]`，旧配置省略新键可解析（既有字段无 default，行为不变）。
**Verification:**
```bash
cargo check -p neo-agent
```

**Steps:**

1. `pub struct RuntimeCompactionConfig`（line 230）在 `micro_keep_recent` 之后新增（每行带 serde default）：
```rust
    #[serde(default = "default_true")]
    pub snip_enabled: bool,
    #[serde(default = "default_snip_min_tokens")]
    pub snip_min_tokens: usize,
    #[serde(default = "default_snip_keep_recent")]
    pub snip_keep_recent: usize,
```
2. 同文件新增三个默认函数（放 `RuntimeCompactionConfig` 附近）：
```rust
fn default_true() -> bool { true }
fn default_snip_min_tokens() -> usize { 1_000 }
fn default_snip_keep_recent() -> usize { 16 }
```
3. `impl Default for RuntimeCompactionConfig`（line 240）在 `micro_keep_recent: 20,` 之后新增：
```rust
            snip_enabled: false,  # 修订 2026-08-02: 前缀改写会破坏 provider 缓存, 付费模型默认关
            snip_min_tokens: 1_000,
            snip_keep_recent: 16,
```

---

## Task C3 — 映射与字面量站点补齐

**Files:** `crates/neo-agent/src/modes/run/runtime/agent.rs`、`crates/neo-agent/src/modes/run/mod.rs`、`crates/neo-agent-core/src/runtime/agent.rs`（修改）
**Why:** `CompactionSettings`/`RuntimeCompactionConfig` 无 `Default`，全字段字面量必须补齐。
**Change Necessity:** 编译必需的机械补齐。
**Impact/Compatibility:** 无行为变化。
**Verification:**
```bash
cargo check -p neo-agent -p neo-agent-core
cargo nextest run -p neo-agent --bin neo modes::run::tests::agent_config_for_app_applies_runtime_config --exact
```

**Steps:**

1. `crates/neo-agent/src/modes/run/runtime/agent.rs` 的 `CompactionSettings { ... }` 映射（约 line 73-89）在 `micro_keep_recent: compaction.micro_keep_recent,` 之后新增：
```rust
            snip_enabled: compaction.snip_enabled,
            snip_min_tokens: compaction.snip_min_tokens,
            snip_keep_recent: compaction.snip_keep_recent,
```
2. `crates/neo-agent/src/modes/run/mod.rs` 中 `RuntimeCompactionConfig` 与 `CompactionSettings` 的全字段字面量（测试约 line 1256/1312/1655/1694/1701/1742/1781 及 `agent_config_for_app_scales_default_compaction_to_model_context_window` 相关字面量）各补三行：
```rust
                    snip_enabled: false,  # 修订 2026-08-02
                    snip_min_tokens: 1_000,
                    snip_keep_recent: 16,
```
3. `crates/neo-agent-core/src/runtime/agent.rs` 测试字面量（约 line 612）同样补齐（字段值与上同）。
4. 运行 `cargo check -p neo-agent -p neo-agent-core`；编译器会暴露任何遗漏的字面量站点，逐个补齐后再跑。

---

## Task B1 — `ProjectionPlan` 新增 snip 字段 + 全部构造点

**Files:** `crates/neo-agent-core/src/compaction/projection.rs`（及其测试）、`crates/neo-agent-core/src/runtime/context_budget.rs`、`crates/neo-agent-core/src/runtime/chat_request.rs`、`crates/neo-agent-core/src/compaction/summary.rs`（修改）
**Why:** 投影需要承载 snip 开关/门槛/保护带。
**Change Necessity:** 代码变更；`ProjectionPlan` 是 `Copy` 结构体，无 `Default`，字段一经添加所有字面量由编译器强制暴露。
**Impact/Compatibility:** `ProjectionPlan::disabled()` 全关（含 snip），既有语义不变。
**Verification:**
```bash
cargo check -p neo-agent-core
```

**Steps:**

1. `compaction/projection.rs` 的 `ProjectionPlan`（line 22）在 `keep_recent_messages` 之后新增：
```rust
    /// Whether the stale-result snip/dedup maintenance pass runs.
    pub snip_enabled: bool,
    /// Minimum estimated tool-result tokens before a stale result is snipped.
    pub snip_min_tokens: usize,
    /// Number of newest messages exempt from snip.
    pub snip_keep_recent: usize,
```
2. `ProjectionPlan::disabled()`（line 36）补齐：
```rust
            snip_enabled: false,
            snip_min_tokens: 0,
            snip_keep_recent: 0,
```
3. 更新全部字面量构造点，新增三行：
   - 测试/示例类（`projection.rs` 约 192/218/237/244/280、`context_budget.rs:306`、`summary.rs:219`、`chat_request.rs:251`）：`snip_enabled: false, snip_min_tokens: 0, snip_keep_recent: 0,`
   - `turn_loop.rs:1107` 的 `request_projection_plan`：在 Task B4 一并重写（见下），此处先补 `snip_enabled: false, snip_min_tokens: 0, snip_keep_recent: 0,` 占位保证编译。
4. `cargo check -p neo-agent-core` 直到零错误（编译器是遗漏站点的权威清单）。

---

## Task B2 — 投影三态：dedup → snip → omission

**Files:** `crates/neo-agent-core/src/compaction/projection.rs`（修改）
**Why:** 核心行为：旧大 Read 结果压缩为头+尾；同内容重复读取折叠为短提示；非 hint 工具保持既有全删 omission。
**Change Necessity:** 代码变更；最小边界 = 该文件内的纯函数。
**Impact/Compatibility:** 见 spec §4.2（dedup 只在更早副本"本次请求仍完整可见"时生效，压缩/恢复后天然不去重）。
**Verification:**
```bash
cargo nextest run -p neo-agent-core --lib projection::tests -- 
cargo clippy -p neo-agent-core --lib -- -D clippy::all
```

**Steps:**

1. 顶部导入新增：
```rust
use std::collections::HashMap;

use crate::tools::{SnipHint, snip_hint_for};
```
2. 新增常量与纯函数（放在 `omission_marker` 附近）：
```rust
/// Byte threshold before a tool result joins the dedup index / dedup check.
const DEDUP_MIN_BYTES: usize = 1024;

fn text_only(content: &[Content]) -> Option<&str> {
    let mut texts = content.iter().filter_map(Content::as_text);
    let first = texts.next()?;
    if texts.next().is_some() {
        // Mixed or multi-part content (e.g. images): keep verbatim.
        return None;
    }
    Some(first)
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Head/tail snippet of a stale tool result. Returns `None` when the text is
/// not single-part text. The returned body is strictly smaller than the input.
fn snip_text(text: Option<&str>, hint: SnipHint) -> Option<String> {
    let text = text?;
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= hint.head_lines + hint.tail_lines {
        // A giant single line (or few lines): keep rune windows from both ends.
        let total = text.chars().count();
        if total <= hint.head_chars + hint.tail_chars {
            return Some(text.to_owned());
        }
        let head_end = text
            .char_indices()
            .nth(hint.head_chars)
            .map_or(text.len(), |(i, _)| i);
        let tail_start = text
            .char_indices()
            .nth(total - hint.tail_chars)
            .map_or(text.len(), |(i, _)| i);
        let head = &text[..head_end];
        let tail = &text[tail_start..];
        let omitted_chars = total - hint.head_chars - hint.tail_chars;
        return Some(format!("{head}\n[... {omitted_chars} chars omitted ...]\n{tail}"));
    }
    let head = lines[..hint.head_lines].join("\n");
    let tail = lines[lines.len() - hint.tail_lines..].join("\n");
    let omitted = lines.len() - hint.head_lines - hint.tail_lines;
    Some(format!("{head}\n[... {omitted} lines omitted ...]\n{tail}"))
}

fn dedup_marker(mode: ProjectionMode, tool_name: &str, first_index: usize) -> String {
    match mode {
        ProjectionMode::None => unreachable!("disabled projection must not build markers"),
        ProjectionMode::Request => format!(
            "[duplicate of an earlier {tool_name} result in this request (message index {first_index}); byte-identical content omitted; re-run {tool_name} to restore full content]"
        ),
        ProjectionMode::SummaryInput => format!("[duplicate {tool_name} {first_index}]"),
    }
}
```
3. 改写 `project_messages`（line 72）：
```rust
fn project_messages(
    messages: &[AgentMessage],
    plan: &ProjectionPlan,
    mode: ProjectionMode,
) -> ProjectionResult {
    if !plan.enabled || plan.mode == ProjectionMode::None || plan.mode != mode {
        return unchanged(messages);
    }
    let recent_start = messages.len().saturating_sub(plan.keep_recent_messages);
    let cutoff = plan.cutoff_index.min(messages.len());
    let snip_cutoff = messages.len().saturating_sub(plan.snip_keep_recent);
    let mut omitted_tokens = 0;
    // (tool name, content hash) -> (first visible full index, original text).
    let mut seen_full: HashMap<(String, u64), (usize, String)> = HashMap::new();
    let projected = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            if index >= cutoff || index >= recent_start {
                note_full_result(message, index, &mut seen_full);
                return message.clone();
            }
            project_tool_result(
                message,
                plan,
                mode,
                index,
                snip_cutoff,
                &mut omitted_tokens,
                &mut seen_full,
            )
        })
        .collect::<Vec<_>>();
    let projected_tokens = estimate_messages_tokens(&projected);
    ProjectionResult {
        messages: projected,
        omitted_tokens,
        projected_tokens,
    }
}
```
4. 改写 `project_tool_result`（line 118）并新增 `note_full_result`：
```rust
fn project_tool_result(
    message: &AgentMessage,
    plan: &ProjectionPlan,
    mode: ProjectionMode,
    index: usize,
    snip_cutoff: usize,
    omitted_tokens: &mut usize,
    seen_full: &mut HashMap<(String, u64), (usize, String)>,
) -> AgentMessage {
    let AgentMessage::ToolResult {
        tool_call_id,
        tool_name,
        content,
        is_error,
    } = message
    else {
        return message.clone();
    };
    if *is_error {
        return message.clone();
    }
    let content_tokens = estimate_content_tokens(content);
    let hint = snip_hint_for(tool_name);
    let text = text_only(content);

    // 1. Byte-identical duplicate of a still-visible earlier result -> note.
    if plan.snip_enabled
        && let Some(text) = text
        && text.len() >= DEDUP_MIN_BYTES
        && hint.is_some()
    {
        let key = (tool_name.to_string(), fnv1a(text));
        if let Some((first_index, first_text)) = seen_full.get(&key)
            && first_text == text
        {
            let marker = dedup_marker(mode, tool_name, *first_index);
            let replacement_tokens = marker.len().div_ceil(4);
            *omitted_tokens += content_tokens.saturating_sub(replacement_tokens);
            return AgentMessage::tool_result(
                tool_call_id.clone(),
                tool_name.clone(),
                vec![Content::text(marker)],
                *is_error,
            );
        }
    }

    // 2. Head/tail snip for stale oversized results from hinted tools.
    if plan.snip_enabled
        && index < snip_cutoff
        && content_tokens >= plan.snip_min_tokens
        && let Some(hint) = hint
        && let Some(body) = snip_text(text, hint)
        && body.len() < text.unwrap_or_default().len()
    {
        let marker = format!(
            "[tool result snipped: tool={tool_name}, approx_tokens={content_tokens}; \
             first {} and last {} lines shown; full content retained in session history; \
             re-run {tool_name} to restore]\n{body}",
            hint.head_lines, hint.tail_lines
        );
        let replacement_tokens = marker.len().div_ceil(4);
        *omitted_tokens += content_tokens.saturating_sub(replacement_tokens);
        return AgentMessage::tool_result(
            tool_call_id.clone(),
            tool_name.clone(),
            vec![Content::text(marker)],
            *is_error,
        );
    }

    // 3. Historical full-omission path (micro compaction, non-hinted tools).
    if content_tokens >= plan.min_tool_result_tokens {
        let marker = omission_marker(mode, tool_name, content_tokens);
        let replacement_tokens = marker.len().div_ceil(4);
        *omitted_tokens += content_tokens.saturating_sub(replacement_tokens);
        return AgentMessage::tool_result(
            tool_call_id.clone(),
            tool_name.clone(),
            vec![Content::text(marker)],
            *is_error,
        );
    }

    // 4. Kept verbatim: a full, visible result later duplicates can match.
    note_full_result(message, index, seen_full);
    message.clone()
}

/// Record a result that stays full and visible in this request, so a later
/// byte-identical duplicate can be collapsed to a short pointer note.
fn note_full_result(
    message: &AgentMessage,
    index: usize,
    seen_full: &mut HashMap<(String, u64), (usize, String)>,
) {
    let AgentMessage::ToolResult {
        tool_name, content, ..
    } = message
    else {
        return;
    };
    if snip_hint_for(tool_name).is_none() {
        return;
    }
    let Some(text) = text_only(content) else { return };
    if text.len() < DEDUP_MIN_BYTES {
        return;
    }
    seen_full
        .entry((tool_name.to_string(), fnv1a(text)))
        .or_insert_with(|| (index, text.to_owned()));
}
```
5. `cargo clippy -p neo-agent-core --lib -- -D clippy::all` 通过（如 `let chains` 触发 pedantic 提示，按提示重构为嵌套 `if let`）。

---

## Task B3 — projection 单测（snip + dedup）

**Files:** `crates/neo-agent-core/src/compaction/projection.rs`（`#[cfg(test)] mod tests`）
**Why:** 行为契约：几何、保护带、门槛、错误跳过、dedup 语义。
**Change Necessity:** 测试；post-change 回归。
**Impact/Compatibility:** 无。
**Verification:**
```bash
cargo nextest run -p neo-agent-core --lib projection::tests -- 
```

**Steps:**

1. 在 projection.rs 测试模块顶部补导入与辅助：
```rust
    use crate::tools::{SnipHint, Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult};

    const TEST_HINT: SnipHint = SnipHint {
        head_lines: 3,
        tail_lines: 2,
        head_chars: 100,
        tail_chars: 100,
    };

    /// Registers a hinted tool named "Read" through the real registration path
    /// (idempotent) so `snip_hint_for("Read")` resolves in this process.
    fn register_hinted_read() {
        struct HintedRead {
            hint: SnipHint,
        }
        impl Tool for HintedRead {
            fn name(&self) -> &str { "Read" }
            fn description(&self) -> &str { "hinted read" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
                Box::pin(async { Ok(ToolResult::ok("ok")) })
            }
            fn snip_hint(&self) -> Option<SnipHint> { Some(self.hint) }
        }
        let mut registry = ToolRegistry::default();
        registry.register(HintedRead { hint: TEST_HINT });
    }

    fn read_result(content: &str) -> AgentMessage {
        AgentMessage::tool_result("call_1", "Read", vec![Content::text(content.to_owned())], false)
    }

    fn numbered_content(lines: usize) -> String {
        (1..=lines)
            .map(|i| format!("{i}\tline {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn snip_plan(len: usize, keep_recent: usize, min_tokens: usize) -> ProjectionPlan {
        ProjectionPlan {
            enabled: true,
            cutoff_index: len,
            min_tool_result_tokens: usize::MAX,
            keep_recent_messages: 0,
            snip_enabled: true,
            snip_min_tokens: min_tokens,
            snip_keep_recent: keep_recent,
            mode: ProjectionMode::Request,
        }
    }
```
2. 新增测试：
```rust
    #[test]
    fn snip_keeps_head_and_tail_of_stale_read_result() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![read_result(&content)];
        let plan = snip_plan(messages.len(), 0, 100);
        let result = project_for_request(&messages, &plan);
        let text = result.messages[0].text();
        assert!(text.contains("[tool result snipped: tool=Read"));
        assert!(text.contains("1\tline 1"));
        assert!(text.contains("3\tline 3"));
        assert!(text.contains("9\tline 9"));
        assert!(text.contains("10\tline 10"));
        assert!(text.contains("[... 5 lines omitted ...]"));
        assert!(result.omitted_tokens > 0);
    }

    #[test]
    fn snip_protects_recent_messages() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![read_result(&content), AgentMessage::user_text("recent")];
        let plan = snip_plan(messages.len(), 2, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), content);
        assert_eq!(result.omitted_tokens, 0);
    }

    #[test]
    fn snip_skips_small_results() {
        register_hinted_read();
        let content = numbered_content(3);
        let messages = vec![read_result(&content)];
        let plan = snip_plan(messages.len(), 0, 10_000);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), content);
        assert_eq!(result.omitted_tokens, 0);
    }

    #[test]
    fn snip_skips_error_results() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![AgentMessage::tool_result(
            "call_1",
            "Read",
            vec![Content::text(content.clone())],
            true,
        )];
        let plan = snip_plan(messages.len(), 0, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), content);
    }

    #[test]
    fn snip_skips_non_hinted_tools() {
        let content = numbered_content(10);
        let messages = vec![AgentMessage::tool_result(
            "call_1",
            "NoHintTool",
            vec![Content::text(content.clone())],
            false,
        )];
        let plan = snip_plan(messages.len(), 0, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), content);
    }

    #[test]
    fn snip_windows_giant_single_line() {
        register_hinted_read();
        let content = "x".repeat(1_000);
        let messages = vec![read_result(&content)];
        let plan = snip_plan(messages.len(), 0, 100);
        let result = project_for_request(&messages, &plan);
        let text = result.messages[0].text();
        assert!(text.contains("[... chars omitted ...]"));
    }

    #[test]
    fn dedup_collapses_identical_later_read() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![read_result(&content), read_result(&content)];
        let plan = snip_plan(messages.len(), 2, 100);
        let result = project_for_request(&messages, &plan);
        assert!(result.messages[1].text().contains("[duplicate of an earlier Read result"));
        assert!(result.messages[1].text().contains("message index 0"));
        assert_eq!(result.messages[0].text(), content);
        assert!(result.omitted_tokens > 0);
    }

    #[test]
    fn dedup_keeps_different_content() {
        register_hinted_read();
        let a = numbered_content(10);
        let b = numbered_content(11);
        let messages = vec![read_result(&a), read_result(&b)];
        let plan = snip_plan(messages.len(), 2, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), a);
        assert_eq!(result.messages[1].text(), b);
    }

    #[test]
    fn dedup_never_collapses_errors() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![
            AgentMessage::tool_result("c1", "Read", vec![Content::text(content.clone())], true),
            read_result(&content),
        ];
        let plan = snip_plan(messages.len(), 2, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[1].text(), content);
    }

    #[test]
    fn dedup_respects_sniped_earlier_result() {
        register_hinted_read();
        let content = numbered_content(20);
        let messages = vec![read_result(&content), read_result(&content)];
        let plan = snip_plan(messages.len(), 1, 100);
        let result = project_for_request(&messages, &plan);
        assert!(result.messages[0].text().contains("[tool result snipped"));
        assert_eq!(result.messages[1].text(), content);
    }

    #[test]
    fn disabled_plan_snips_nothing() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![read_result(&content)];
        let result = project_for_request(&messages, &ProjectionPlan::disabled());
        assert_eq!(result.messages[0].text(), content);
    }

    #[test]
    fn summary_input_mode_snips_like_request() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![read_result(&content)];
        let mut plan = snip_plan(messages.len(), 0, 100);
        plan.mode = ProjectionMode::SummaryInput;
        let result = project_for_summary(&messages, &plan);
        assert!(result.messages[0].text().contains("[tool result snipped"));
    }
```
（如 `snip_skips_non_hinted_tools` 与并行的其它测试共享进程，`NoHintTool` 未被任何测试注册，恒不命中。）

---

## Task B4 — 触发接线（turn_loop + chat_request 接线测试）

**Files:** `crates/neo-agent-core/src/runtime/turn_loop.rs`、`crates/neo-agent-core/src/runtime/chat_request.rs`（修改）
**Why:** 让 snip 在 `snip_enabled` 时每请求生效，且 `NoAction` 不再把 plan 降级为 disabled。
**Change Necessity:** 代码变更；最小边界 = `request_projection_plan` + `prepare_model_request` 一个分支。
**Impact/Compatibility:** `micro_enabled` 语义不变（其 omission 仍按原门槛）；controller 零改动（`UseProjectionOnly` 已透传 `snapshot.projection`）。
**Verification:**
```bash
cargo nextest run -p neo-agent-core --lib runtime::turn_loop::tests::request_projection_plan_ --exact --include-ignored
cargo nextest run -p neo-agent-core --lib runtime::chat_request::tests::chat_request_applies_sniped_projection_plan --exact
```

**Steps:**

1. `turn_loop.rs` 的 `request_projection_plan`（line 1097）整体替换：
```rust
fn request_projection_plan(
    config: &AgentConfig,
    context: &super::context::AgentContext,
) -> ProjectionPlan {
    let Some(settings) = config.compaction else {
        return ProjectionPlan::disabled();
    };
    let micro_on = settings.micro_enabled;
    let snip_on = settings.snip_enabled;
    if !micro_on && !snip_on {
        return ProjectionPlan::disabled();
    }
    let message_count = context.messages().len();
    ProjectionPlan {
        enabled: true,
        cutoff_index: if micro_on {
            message_count.saturating_sub(settings.micro_keep_recent)
        } else {
            message_count
        },
        min_tool_result_tokens: if micro_on { 1_000 } else { usize::MAX },
        keep_recent_messages: if micro_on { settings.micro_keep_recent } else { 0 },
        snip_enabled: snip_on,
        snip_min_tokens: settings.snip_min_tokens,
        snip_keep_recent: settings.snip_keep_recent,
        mode: ProjectionMode::Request,
    }
}
```
2. `prepare_model_request`（line 1015-1018）的 `NoAction` 分支：
```rust
        CompactionDecision::NoAction { snapshot: decided } => {
            snapshot = decided;
            snapshot.projection
        }
```
（`decided.projection` 即传入 controller 的 request plan，已含 snip 字段。）
3. `turn_loop.rs` 的 `mod tests`（line 1173）新增（复用该模块现有 imports；补 `use crate::harness::fake_model;` 如缺）：
```rust
    #[test]
    fn request_projection_plan_enables_snip_without_micro() {
        let config = AgentConfig::for_model(fake_model())
            .with_compaction(super::super::config::CompactionSettings::new(usize::MAX, 4));
        let mut context = super::super::context::AgentContext::new();
        context.append_message(AgentMessage::user_text("x"));
        let plan = request_projection_plan(&config, &context);
        assert!(plan.enabled);
        assert!(plan.snip_enabled);
        assert_eq!(plan.min_tool_result_tokens, usize::MAX);
        assert_eq!(plan.snip_min_tokens, 1_000);
        assert_eq!(plan.snip_keep_recent, 16);
    }

    #[test]
    fn request_projection_plan_disabled_without_compaction() {
        let config = AgentConfig::for_model(fake_model());
        let context = super::super::context::AgentContext::new();
        let plan = request_projection_plan(&config, &context);
        assert!(!plan.enabled);
        assert!(!plan.snip_enabled);
    }
```
4. `chat_request.rs` 测试模块（line 180）新增接线测试——context 构造照抄同文件既有测试 `chat_request_applies_supplied_projection_plan` 的模式：
```rust
    #[tokio::test]
    async fn chat_request_applies_sniped_projection_plan() {
        // 1) Register a hinted "Read" tool through the real registration path.
        // 2) Build the same context shape as the existing test, with one old
        //    large "Read" tool result (> snip_min_tokens) and a user message.
        // 3) Pass a plan with snip_enabled=true, snip_min_tokens=100,
        //    snip_keep_recent=0, cutoff_index=messages.len(),
        //    min_tool_result_tokens=usize::MAX, mode=Request.
        // 4) Assert the built ChatRequest's tool-result message text contains
        //    "[tool result snipped: tool=Read".
    }
```
（该测试体按同文件既有测试的 context/assert 模式补全，不得留 TODO。）

---

## Task D1 — Read 默认窗口 400 行 + 定点读引导

**Files:** `crates/neo-agent-core/src/tools/read.rs`（修改）
**Why:** 单次全读注入上界从 ~11K tokens 降到 ~4.3K；引导模型只读所需范围（对应实测 req4-6 的全文件连续读）。
**Change Necessity:** 代码变更；最小边界 = 常量 + 缺省分支 + 描述文本 + 测试。
**Impact/Compatibility:** `n_lines` 显式传值时行为不变；`MAX_LINES=1000` 硬上限不变；负偏移 tail 默认窗口同步变为 400。
**Verification:**
```bash
cargo nextest run -p neo-agent-core --lib tools::read::tests:: -- 
cargo nextest run -p neo-agent-core --lib tools::read::tests::max_lines_cap_is_reported --exact
```

**Steps:**

1. 常量区（line 8 附近）新增：
```rust
const DEFAULT_LINES: usize = 400;
```
2. `run_read`（line 295）：
```rust
    let requested_lines = n_lines.unwrap_or(DEFAULT_LINES);
```
3. `description()`（line 38-61）替换为（保留既有参数/行为描述，增强定点读引导并同步默认值）：
```rust
        "Read a UTF-8 text file.\
        \
        If the user provides a concrete file path, call Read directly. Do not use Glob, ls, or \
        other pre-checks for known text file paths; missing or invalid paths return errors you can \
        handle. Use Glob for pattern searches and Bash `ls` for directories.\
        \
        Prefer targeted reads: for files over ~200 lines, use `line_offset` and `n_lines` to read \
        only the range you need instead of the whole file. Small windows keep the context and the \
        provider cache small; a full read of a large file costs tokens for every line, and old \
        read results are shortened in the model input when they go stale.\
        \
        Parameters:\
        - path: Path to the text file. Relative paths resolve against the working directory; \
          absolute paths are used as-is, including paths outside the working directory.\
        - line_offset: 1-based line number to start reading from. Omit to start at line 1. Negative \
          values read from the end (e.g. -100 reads the last 100 lines); the absolute value must \
          not exceed 1000.\
        - n_lines: Maximum number of lines to read (default 400; cap 1000).\
        \
        Behavior:\
        - Returns up to 400 lines by default (1000 max) or 100 KB per call, whichever comes first.\
        - Lines longer than 2000 characters are truncated mid-line and marked with `...`.\
        - Output format: each line is prefixed with `<line-number>\\t<content>`.\
        - A `<system>...</system>` status block is appended after the content; it summarizes how \
          much was read and is not part of the file itself.\
        - Page larger files with multiple Read calls using line_offset and n_lines.\
        - When you need several files, prefer reading them in parallel.\
        - Only UTF-8 text files can be read. Binary files, images, and videos are refused."
```
4. 既有测试 `max_lines_cap_is_reported`（line 629）改为显式上限窗口：
```rust
        let result = render_from_content(&content, None, Some(MAX_LINES)).unwrap();
```
5. 新增测试：
```rust
    #[test]
    fn default_window_is_four_hundred_lines() {
        let content = (1..=MAX_LINES + 10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let result = render_from_content(&content, None, None).unwrap();
        assert_eq!(result.rendered_lines.len(), DEFAULT_LINES);
        assert!(result
            .finish_output()
            .contains(&format!("Total lines in file: {}.", MAX_LINES + 10)));
    }

    #[test]
    fn description_guides_targeted_reads() {
        let description = ReadTool.description();
        assert!(description.contains("Prefer targeted reads"));
        assert!(description.contains("default 400"));
        assert!(description.contains("line_offset"));
    }
```

---

## Task E1 — 配置文档（en + zh）

**Files:** `docs/en/configuration/config-files.md`、`docs/zh/configuration/config-files.md`（修改）
**Why:** 用户配置面必须有据可查。
**Change Necessity:** docs-only。
**Impact/Compatibility:** 无。
**Verification:** 无（markdown 只读检查）。

**Steps:**

1. `docs/en/configuration/config-files.md` 的 `[runtime.compaction]` 表（line 229-230 之后）新增：
```markdown
| `snip_enabled` | bool | `true` | Whether stale oversized tool results (e.g. long Read outputs) are shortened to a head/tail snippet in the model input |
| `snip_min_tokens` | usize | `1000` | Minimum tool-result size before a stale result is snipped |
| `snip_keep_recent` | usize | `16` | Number of recent messages exempt from snip |
```
并把 line 234 的 micro 提示修订为：
```markdown
> Tips: micro compaction rewrites old tool results in the middle of the context, which breaks the provider prefix cache once per rewritten result; it is best used for non-hinted tools. The snip maintenance above is the cache-conscious path: it keeps head/tail context, rewrites deterministically (one break per result, then stable), and is enabled by default.
```
2. `docs/zh/configuration/config-files.md` 对应行（line 229-230 之后）：
```markdown
| `snip_enabled` | bool | `true` | 是否把过时的大 tool result（如长 Read 输出）在模型输入中剪为头+尾片段 |
| `snip_min_tokens` | usize | `1000` | 触发剪枝的最小 tool-result 大小 |
| `snip_keep_recent` | usize | `16` | 剪枝豁免的最近消息数 |
```
并把 line 234 提示修订为：
```markdown
> Tips: micro compaction 会改写上下文中间的历史 tool result，每个被改写结果会使前缀缓存失效一次，建议仅用于未声明剪枝几何的工具。上表 snip 维护是缓存友好路径：保留头尾上下文、确定性改写（每个结果仅失效一次，之后稳定），默认开启。
```

---

## Task E2 — Aegis 索引

**Files:** `docs/aegis/INDEX.md`（修改）
**Why:** 仓库 aegis 工作区约定。
**Change Necessity:** docs-only。
**Verification:** 无。

**Steps:**

1. 表头行（`| --- | --- | --- | --- |`）之后追加两行：
```markdown
| 2026-08-02 | spec | docs/aegis/specs/2026-08-02-read-token-cache-optimization-design.md | Read 工具 Token 缓存优化 Design |
| 2026-08-02 | plan | docs/aegis/plans/2026-08-02-read-token-cache-optimization.md | Read 工具 Token 缓存优化 Implementation Plan |
```

---

## 验证总纲（最终合并检查）

```bash
cargo fmt --all --check
cargo clippy -p neo-agent-core --lib -- -D clippy::all
cargo clippy -p neo-agent-core --test <affected-test-target> -- -D clippy::all   # 如存在受影响集成测试
cargo nextest run -p neo-agent-core --lib projection::tests -- 
cargo nextest run -p neo-agent-core --lib tools::read::tests -- 
cargo nextest run -p neo-agent-core --lib tools::tests::snip_hint_registration_and_lookup --exact
cargo nextest run -p neo-agent --bin neo modes::run::tests::agent_config_for_app_applies_runtime_config --exact
```

## Risks / Retirement

- **风险 1（前缀一次性改写）**：snip 首触发会使该结果之后的上下文一次 uncached；确定性、每结果仅一次、收益为正（spec §4.1）。逃生阀：`[runtime.compaction] snip_enabled=false`。
- **风险 2（模型信息缺失）**：被剪结果只保留头尾；marker 明示"已入会话历史，重跑 Read 恢复"，且仅作用于 >16 条消息前、≥1000 tokens 的旧结果。
- **风险 3（测试漂移）**：`CompactionSettings::new` 默认 `snip_enabled=true` 使部分测试的 request plan 变为 enabled；新会话无陈旧大结果时 snip/dedup 均为 no-op，断言消息内容的既有测试不受影响。若个别测试因 plan 语义变化失败，先复核该测试是否依赖 `ProjectionPlan::disabled()` 的隐式行为，必要时改为显式 `ProjectionPlan::disabled()`。
- **Retirement**：`micro_enabled` 全删 omission 保留（非 hint 工具路径），不删除；无旧兼容路径需要退役。dedup/snip 不新增任何 fallback 或双轨 owner（同一 pass 内三态互斥）。

## Execution Readiness View

- **Intent Lock**: 降低 Read 的 uncached 注入与方差（spec 验收 §7）。
- **Scope Fence**: 仅 spec §4 四个方案；不含 bash 等非 hint 工具 snip、不含 durable 历史改写、不含 reasonix/cache_probe 改动。
- **Baseline Lock**: run `20260801-162932-d0877b`（Read uncached 61,706 / variance 13.85M）为优化前基线；验收 6 的可手工作对照。
- **Compatibility Boundary**: 配置键向后兼容（serde default）；wire/JSONL/schema 不变。
- **Task Batches**: A（hint 能力）→ C（配置）→ B（投影）→ D（Read 引导）→ E（文档）。
- **Test Obligations**: 每任务精确 nextest 过滤；B3 为行为契约主测试。
- **Review Gates**: 每批完成后跑 `cargo check -p neo-agent-core -p neo-agent` + 对应 clippy；任务级 commit。
- **Drift / Rewind Rules**: 任一任务验证失败 → 只修该任务范围，禁止回滚他人 worktree 改动；行为偏差回 spec §4 决策。
