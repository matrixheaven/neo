# Neo 包结构重构设计：解散 neo-sdk、收编 neo-extensions

日期：2026-06-19
状态：已批准并实施

## 背景与问题

原工作区包含六个 crate，其中 `neo-sdk` 和 `neo-extensions` 的边界与职责不清晰：

- **`neo-sdk`** 成了“杂物间”：
  - `export`：HTML 导出，属于 session 表现层；
  - `rpc`：JSONL RPC 编解码与基础类型，属于运行时协议原语；
  - `skills`：skill 清单加载，属于系统提示资源加载。
  三块完全不同的职责被硬塞进一个所谓 “SDK”。

- **`neo-extensions`** 游离在核心外：
  - 本地扩展与 MCP 本质上是同一类“外部 tool adapter”，但 MCP 完整位于 `neo-agent-core`，扩展却单独成 crate。
  - 扩展当前只支持 stdio 传输、只支持本地路径安装、工具结果格式与 MCP 不统一。

- **`neo-agent::rpc_mode`**：
  - JSONL 编解码在 `neo-sdk`，server loop/dispatch 在 `neo-agent`，而 MCP 协议与适配全在 `neo-agent-core`，导致边界不一致。

## 设计目标

1. 删除 `neo-sdk`，把其三块职责分别放到正确位置。
2. 把 `neo-extensions` 整体迁入 `neo-agent-core`，作为与 MCP 并列的本地扩展 tool adapter。
3. 让 `neo-agent-core` 统一承载运行时所需的协议适配层（MCP、extension、RPC 基础、skill 资源、session 导出）。
4. `neo-agent` 回归为 CLI/TUI 薄壳。
5. 先搬迁、再逐步重构，避免一次性巨大 diff。

## 最终架构

```text
neo-agent CLI/TUI
  -> neo-agent-core runtime, sessions, permissions, tools, MCP,
     local extensions, skill loading, JSONL RPC, HTML export
      -> neo-ai provider-neutral model and stream contracts
  -> neo-tui terminal UI primitives
xtask maintenance commands
```

## 模块归宿

| 原位置 | 新位置 | 理由 |
|--------|--------|------|
| `neo-sdk::export` | `neo-agent-core::session::export` | session 导出/表现层能力 |
| `neo-sdk::skills` | `neo-agent-core::skills` | 运行时系统提示资源加载 |
| `neo-sdk::rpc`（基础类型/codec） | `neo-agent-core::rpc` | 运行时协议原语 |
| `neo-sdk::rpc`（session 结果类型） | `neo-agent::rpc_types` | RPC API 的 wire 契约，属于 agent 层 |
| `neo-extensions::*` | `neo-agent-core::tools::extensions` | 与 MCP 并列的 tool adapter |
| `neo-agent::extension_tools` | `neo-agent-core::tools::extensions::bridge` | 扩展接入 `ToolRegistry` 的桥接 |
| `neo-agent::rpc_mode`（server loop） | 保留在 `neo-agent` | 依赖 `AppConfig`、prompt templates、run mode 等 agent 层概念 |

## 实施阶段

### 阶段一：解散 neo-sdk

1. 新建 `neo-agent-core/src/session/export.rs`，迁移 HTML 导出。
2. 新建 `neo-agent-core/src/skills.rs`，迁移 skill 加载。
3. 新建 `neo-agent-core/src/rpc/mod.rs` 与 `neo-agent-core/src/rpc/codec.rs`，迁移 JSONL RPC 基础类型与 codec。
4. 把 session 专用的 RPC 结果类型迁到 `neo-agent/src/rpc_types.rs`。
5. 删除 `crates/neo-sdk`，从 workspace members 移除，更新所有导入路径。
6. 迁移测试到 `neo-agent-core/tests/` 与 `neo-agent/src/rpc_types.rs`。

### 阶段二：RPC server 边界

- `neo-agent-core::rpc` 只负责协议层：基础类型、JSONL 编解码。
- `neo-agent::rpc_mode` 继续负责业务 dispatch：`get_commands`、`sessions.*`、`prompt` 等方法依赖 agent 层配置，保留在 `neo-agent`。
- 未来如需进一步统一，可在 core 提供 `RpcHandler` trait，由 agent 实现；当前阶段不强制推行。

### 阶段三：收编 neo-extensions

1. 在 `neo-agent-core/src/tools/extensions/` 下新建：
   - `discovery.rs`：扩展发现
   - `installation.rs`：安装/卸载/更新
   - `lifecycle.rs`：enable/disable 生命周期
   - `runner.rs`：stdio JSONL RPC client
   - `bridge.rs`：原 `neo-agent::extension_tools` 的 ToolRegistry 接入
   - `mod.rs`：公共类型与 re-export
2. `neo-agent-core::tools::extensions` 设为 public，供 `neo-agent` 调用。
3. 删除 `crates/neo-extensions`，从 workspace members 移除。
4. `neo-agent::extension_commands.rs` 保留在 agent 层，仅负责 CLI 输出与用户交互。

### 阶段四：清理与验证

1. 更新 README.md、AGENTS.md、docs/architecture.md、docs/index.md、docs/sessions.md，删除 `neo-sdk` / `neo-extensions` 描述。
2. 运行 `cargo fmt --all --check`。
3. 运行 `cargo test -p neo-agent-core -p neo-agent`。
4. 记录 `neo-ai` 与 `neo-agent-core` 中存在的预存 clippy 警告，不归入本次范围。

## 关键决策

- **为什么 RPC server dispatch 不迁进 core？**
  `rpc_mode` 依赖 `AppConfig`、prompt templates、`modes::run::run_prompt` 等 agent 层概念。强行迁入会把这些概念也拉进 core，破坏 core 的“运行时”边界。协议原语进 core 已足够消除与 MCP 的不一致。

- **为什么扩展不先 redesign 再搬迁？**
  当前最大痛点是“东西放错了地方”。先把代码搬到正确位置、消除错误依赖后，再在 core 内部逐步改进传输协议、安装源、生命周期等细节。

- **为什么 session RPC 结果类型留在 agent 层？**
  这些类型是 `neo-agent` RPC API 的输入/输出契约，不是通用协议原语。放在 agent 层可以避免 core 被 agent 特定 wire 格式污染。

## 验收标准

1. 工作区中不再存在 `crates/neo-sdk` 和 `crates/neo-extensions`。
2. `cargo build -p neo-agent` 成功。
3. `cargo test -p neo-agent-core -p neo-agent` 全部通过。
4. `cargo fmt --all --check` 通过。
5. `neo-agent-core` 包含 `rpc`、`skills`、`session::export`、`tools::extensions` 模块，且 `tools::mcp` 与 `tools::extensions` 并列。
6. 文档不再提及 `neo-sdk` / `neo-extensions`。

## 后续工作

本次重构只解决“位置”问题。`neo-extensions` 的进一步设计可在 core 内部独立进行：

- 统一 extension 与 MCP 的 tool adapter 抽象；
- 支持 HTTP/SSE 等更多传输方式；
- 支持 git/URL/版本化等安装源；
- 完善生命周期模型（per-project/per-user 隔离、依赖、自动更新等）。
