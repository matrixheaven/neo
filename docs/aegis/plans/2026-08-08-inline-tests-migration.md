# 内联测试迁移计划（2026-08-08）

## 目标

深度扫描四个 crate 的 `src/` 内联测试块，将超过治理阈值（>300 行目标、>600 行或 >12 测试硬上限）的 16 个 `#[cfg(test)]` 块从生产文件迁出，落到源码侧 `test_cases/` 目录（用户确认范围：超限 + 超目标；目标：源码侧 test_cases/，保持私有访问）。

治理依据：`docs/aegis/specs/2026-08-07-test-suite-governance-design.md` §5.1/§5.2（内联测试硬上限 600 行或 12 测试；拆出后使用明确文件名，由生产模块用显式 `#[path]` 声明；禁止测试专用 `mod.rs`/`tests.rs` 聚合）。

## 扫描结果

- 全部内联块：172 个（26,079 行，812 个测试函数）。
- 范围内 16 块（约 8,256 行，145 个测试函数）。
- 范围外：156 块（≤300 行且 ≤12 测试），按治理 §5.1 保留内联。

## 通用迁移模式（15 个普通文件）

对 `src/<dir>/X.rs` 中的内联块：

1. 将 `#[cfg(test)] mod tests { ... }` 的块体（不含 `mod tests {` 包装）原样复制到 `src/<dir>/test_cases/<behavior>.rs`（目录不存在则创建；`mod.rs` 用父目录名命名）。
2. 生产文件中用声明替换整块：

```rust
#[cfg(test)]
#[path = "test_cases/<behavior>.rs"]
mod tests;
```

   （沿用 `crates/neo-agent-core/src/tools/workflow.rs:1394` 的既有形式；模块名保持 `tests`，测试全路径不变。）
3. 新文件头部加 `//!` 文档注释说明来源（如 `//! Tool dispatch behavior (moved from tool_dispatch.rs).`）。
4. 内容逐字节保留，不改测试名、不改断言、不改 helper。

### 目标清单

| # | 生产文件 | 目标 test_cases 文件 |
|---|---|---|
| 1 | `neo-agent-core/src/runtime/tool_dispatch.rs`（863L） | `runtime/test_cases/tool_dispatch.rs` |
| 2 | `neo-agent-core/src/runtime/chat_request.rs`（401L） | `runtime/test_cases/chat_request.rs` |
| 3 | `neo-agent-core/src/tools/delegate_controls.rs`（407L） | `tools/test_cases/delegate_controls.rs` |
| 4 | `neo-agent-core/src/workspace_policy.rs`（309L） | `src/test_cases/workspace_policy.rs`（新建目录） |
| 5 | `neo-agent-core/src/events.rs`（304L） | `src/test_cases/events.rs` |
| 6 | `neo-agent-core/src/skills/builtin/mod.rs`（302L） | `skills/builtin/test_cases/builtin.rs`（新建目录） |
| 7 | `neo-ai/src/providers/openai/responses.rs`（406L） | `providers/openai/test_cases/responses.rs`（新建目录） |
| 8 | `neo-ai/src/providers/google.rs`（339L） | `providers/test_cases/google.rs`（新建目录） |
| 9 | `neo-ai/src/tool_assembly.rs`（359L） | `src/test_cases/tool_assembly.rs`（新建目录） |
| 10 | `neo-ai/src/catalog.rs`（345L） | `src/test_cases/catalog.rs` |
| 11 | `neo-tui/src/transcript/store.rs`（460L） | `transcript/test_cases/store.rs` |
| 12 | `neo-agent/src/modes/btw.rs`（405L/13T） | `modes/test_cases/btw.rs`（新建目录） |
| 13 | `neo-agent/src/modes/lifecycle.rs`（578L） | `modes/test_cases/lifecycle.rs` |
| 14 | `neo-agent/src/modes/task_browser.rs`（429L） | `modes/test_cases/task_browser.rs` |
| 15 | `neo-agent/src/modes/interactive/custom_endpoint_provider.rs`（409L） | `interactive/test_cases/custom_endpoint_provider.rs` |

## 特殊任务：interactive/mod.rs test_cases 容器（1940L）

容器现状：`#[cfg(test)] mod test_cases { <~1870 行共享 fixture helper> + 33 个 #[path] 模块声明 }`。33 个测试文件通过 `use super::*;` 访问 helper。

处理：

1. 将 72 个 helper 定义（const/struct/impl/fn，行 3177–5043 区间）按用途拆入 `test_cases/fixtures_<用途>.rs`（如 `fixtures_sessions.rs`、`fixtures_approvals.rs`、`fixtures_skills.rs`、`fixtures_controllers.rs`、`fixtures_config.rs`、`fixtures_workflow.rs`、`fixtures_transcript.rs`；4–7 个文件，每个目标 200–600 行，硬上限 1200 行）。
2. helper 在 fixture 文件中改为 `pub`（仅测试构建可见，无泄漏风险）。
3. fixture 文件之间交叉引用用 `use super::fixtures_<x>::*;`。
4. 容器保留全部 `#[path]` 模块声明，新增 fixture 声明与重导出：

```rust
#[path = "test_cases/fixtures_sessions.rs"]
mod fixtures_sessions;
pub use fixtures_sessions::*;
// ... 其余 fixture 同理
```

   （`use super::*` 的 glob 导入会带出 `pub use` 重导出，33 个测试文件无需改动。）
5. 删除容器顶部不再使用的 `use` 导入（helper 迁走后成为死代码）。
6. 保证 fixture 间 helper 名称无重复（重导出进同一容器命名空间）。

## 非目标

- 不迁移 156 个 ≤300L 且 ≤12T 的内联块（治理 §5.1 允许内联）。
- 不迁入 crate 顶层 `tests/`（私有单元测试保持源码侧，不 pub 化生产代码）。
- 不改测试内容、不改测试名、不改断言。
- 不触碰工作树中其他 agent 的脏文件（当前脏文件均在 `neo-tui/src/frame_selection.rs`、`tasks_browser/render.rs`、`tests/` 下，与范围无交集）。
- 不新增/删除生产逻辑。

## 任务切分（subagent-driven，串行）

| 任务 | 内容 | 验证 |
|---|---|---|
| T1 | neo-agent-core：tool_dispatch.rs、chat_request.rs | `cargo check -p neo-agent-core --tests`；`cargo nextest run -p neo-agent-core --lib <模块>_tests 相关过滤` |
| T2 | neo-agent-core：delegate_controls.rs、workspace_policy.rs、events.rs、skills/builtin/mod.rs | 同上 |
| T3 | neo-ai：responses.rs、google.rs、tool_assembly.rs、catalog.rs | `cargo check -p neo-ai --tests`；nextest --lib 过滤 |
| T4 | neo-tui：transcript/store.rs | `cargo check -p neo-tui --tests`；nextest --lib 过滤 |
| T5 | neo-agent：btw.rs、lifecycle.rs、task_browser.rs、custom_endpoint_provider.rs | `cargo check -p neo-agent --tests`；nextest --lib 过滤 |
| T6 | neo-agent：interactive/mod.rs 容器 fixture 拆分 | `cargo check -p neo-agent --tests`；nextest --lib `interactive::` 过滤 |

每任务：implementer → spec 合规评审 → 代码质量评审 → 修复循环 → 协调者验证 → 单任务提交（`refactor(crate): extract <module> inline tests to test_cases/`）。

## 验证基线

- 迁移前后测试全路径不变（`<mod>::tests::<test>`），可用 `--lib` + 模块名过滤做精确回归。
- 每个任务提交前运行：`cargo check -p <crate> --tests` + 指向被迁移测试的最小 nextest 过滤 + `cargo clippy -p <crate> --lib -- -D clippy::all`。
