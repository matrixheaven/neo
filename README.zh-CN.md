# Neo

[English](README.md) | [简体中文](README.zh-CN.md)

Rust 原生、纯本地运行的 AI 编程助手。Neo 作为 CLI 和 TUI 完全运行在你的机器上——没有托管后端、无需账号、不上传遥测数据。带上你自己的 API 密钥，即可接入 OpenAI、Anthropic、Google 或任何兼容 OpenAI 的端点。

## 功能特性

- **本地优先。** 所有会话、配置、技能和信任决策都保存在 `~/.neo/` 下。除了你显式配置的 API 调用之外，没有任何数据离开你的机器。
- **多模型提供商。** 支持 OpenAI Responses、Anthropic Messages、Google Generative AI，以及任何兼容 OpenAI 的端点（Ollama、vLLM 等）。
- **内置工具。** 读取、列出、查找、grep 搜索、glob 匹配、写入、编辑、bash 命令、PTY 终端、待办清单、计划模式、目标追踪——全部通过分层权限系统管控。
- **MCP 支持。** 连接 stdio 或远程 MCP 服务器；工具会被自动发现，并以 `mcp__<server>__<tool>` 的命名空间提供。
- **会话。** 每次对话都是以工作区为作用域、可恢复、可派生（fork）的本地 JSONL 记录。
- **技能。** 分层提示注入系统（project → user → extra → built-in），可按上下文自动激活。
- **排队与引导。** 在 Agent 忙时排队追加提示，或在下一个断点处注入引导消息。
- **跨平台。** 支持 macOS、Linux 和 Windows。

## 前置要求

- **Rust** 1.88+（stable 通道）。仓库通过 `rust-toolchain.toml` 固定工具链，`rustup` 会自动处理。
- **`cargo`**、**`rustfmt`** 和 **`clippy`** —— 标准 Rust 安装已包含。
- 至少一个提供商的 API 密钥（例如 `OPENAI_API_KEY`）。

<details>
<summary>还没有安装 Rust？</summary>

通过 [rustup](https://rustup.rs) 安装：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

在 Windows 上，从同一站点下载并运行 `rustup-init.exe`。
</details>

## 安装

```bash
git clone https://github.com/matrixheaven/neo.git
cd neo
cargo install --path crates/neo-agent --locked --force
```

这会编译 release 二进制文件并自动安装到 `~/.cargo/bin/`。确保 `~/.cargo/bin` 在你的 `PATH` 中（使用 rustup 时默认如此）。

### 验证安装

```bash
neo --version          # 如果已加入 PATH
neo models list        # 查看解析后的模型目录
```

## 配置

Neo 读取单一配置文件：`~/.neo/config.toml`（若设置了 `NEO_HOME`，则为 `$NEO_HOME/config.toml`）。最小配置示例：

```toml
default_model = "openai/gpt-4.1"

[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
```

设置环境变量后即可使用：

```bash
export OPENAI_API_KEY=sk-...
neo run "解释这个代码库"
```

Anthropic、Google、自定义提供商、模型别名、MCP 服务器等所有选项，请参阅 **[配置指南](docs/zh/configuration/config-files.md)**。

## 快速开始

```bash
# 单次提示
neo run "用 Rust 写一个反转链表的函数"

# 交互式 TUI 会话
neo

# 恢复之前的会话
neo resume                 # 打开会话选择器
neo resume <session-id>    # 或恢复指定会话
neo sessions list          # 列出当前工作区的会话
```

### 常用标志

```bash
neo run --output text "纯文本输出"
neo run --output json "JSON 输出"
neo --no-session run "不创建会话直接回答"
```

## 文档

| 主题 | 链接 |
|------|------|
| 快速开始 | [docs/zh/quickstart.md](docs/zh/quickstart.md) |
| 配置 | [docs/zh/configuration/config-files.md](docs/zh/configuration/config-files.md) |
| 概览 | [docs/zh/index.md](docs/zh/index.md) |
| 提供商 | [docs/zh/configuration/providers.md](docs/zh/configuration/providers.md) |
| 内置工具 | [docs/zh/reference/tools.md](docs/zh/reference/tools.md) |
| 会话 | [docs/zh/guides/sessions.md](docs/zh/guides/sessions.md) |
| MCP | [docs/zh/customization/mcp.md](docs/zh/customization/mcp.md) |
| 技能 | [docs/zh/customization/skills.md](docs/zh/customization/skills.md) |
| 目标 | [docs/zh/guides/goals.md](docs/zh/guides/goals.md) |
| 交互 | [docs/zh/guides/interaction.md](docs/zh/guides/interaction.md) |

---

## 开发

### 仓库结构

```
crates/
  neo-ai/          提供商无关的请求/流/错误类型 + HTTP 客户端
  neo-agent-core/  Agent 运行时：工具、权限、会话、MCP、技能
  neo-tui/         终端 UI 基础组件（crossterm + ratatui）
  neo-agent/       `neo` 二进制文件：CLI 解析、配置、TUI 入口
```

### 构建与检查

```bash
cargo build -p neo-agent                         # 构建二进制文件
cargo fmt --all --check                          # 格式化检查
cargo clippy -p neo-agent --bin neo -- -D warnings   # 静态检查
```

### 测试

建议安装 [cargo-nextest](https://nexte.st)：

```bash
cargo nextest run -p neo-agent --bin neo cli_commands    # 二进制集成测试
cargo nextest run -p neo-agent-core --lib                # 库单元测试
```

对于单个已知测试函数，使用精确的 `cargo test` 也可以：

```bash
cargo test --package neo-agent --bin neo -- modes::task_browser::tests::test_name --exact --nocapture
```

### 代码规范

- `unsafe_code = "forbid"`；`clippy::pedantic` 会发出警告。
- 跨平台是强制要求——除非使用 `#[cfg]` 守卫，否则不要使用硬编码路径分隔符或仅 Unix 的假设。
- 提供商代码位于 `crates/neo-ai/src/providers/`。
- 会话事件被规范化为 `AgentEvent` 值——JSONL 不依赖任何提供商的原始数据格式。

## 许可证

MIT
