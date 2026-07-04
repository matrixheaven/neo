# 快速开始

本页带你从零完成安装、配置 API Key 并跑通第一个对话。

## 前置条件

| 依赖 | 版本 | 说明 |
| --- | --- | --- |
| Rust | 1.88+（stable） | 仓库通过 `rust-toolchain.toml` 锁定工具链，`rustup` 会自动安装 |
| `cargo` / `rustfmt` / `clippy` | 随 Rust 附带 | 标准安装即可 |
| API Key | 至少一个供应商 | 例如 `OPENAI_API_KEY` |

尚未安装 Rust？通过 [rustup](https://rustup.rs) 一键安装：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## 安装方式

### 方式一：从源码构建（推荐）

```bash
git clone https://github.com/matrixheaven/neo.git
cd neo
cargo install --path crates/neo-agent --locked --force
```

`cargo install` 会编译 release 二进制并自动放到 `~/.cargo/bin/`。请确认该目录已在 `PATH` 中（使用 rustup 时默认即生效）。

验证安装：

```bash
neo --version
neo models list        # 查看已解析的模型目录
```

### 方式二：直接 cargo install

> 当 Neo 发布到 crates.io 后可用。当前建议从源码构建以获取最新特性。

```bash
cargo install neo-agent --locked
```

## 首次启动

在任意目录执行 `neo` 即可进入交互式 TUI：

```bash
neo
```

首次运行会在 `~/.neo/config.toml` 生成默认配置文件。若尚未配置供应商，TUI 会提示你先设置。

## 配置 API Key

Neo 读取单一配置文件 `~/.neo/config.toml`（或 `$NEO_HOME/config.toml`，当你设置了 `NEO_HOME` 环境变量时）。Key 可通过两种方式提供。

### 方式一：环境变量（推荐）

把敏感 Key 放在 shell 环境里，配置文件只引用变量名：

```toml
default_model = "openai/gpt-4.1"

[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"   # 仅写变量名，不写真实 Key
```

```bash
export OPENAI_API_KEY=sk-...
neo
```

### 方式二：直接写入 config.toml

```toml
[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key = "sk-..."                # 直接写入 Key
```

> 安全提示：方式二会让 Key 落盘到磁盘，仅在你明确接受风险时使用。

### 常见供应商配置

```toml
# Anthropic
[providers.anthropic]
type = "anthropic_messages"
api_key_env = "ANTHROPIC_API_KEY"

# Google
[providers.google]
type = "google_generative_ai"
api_key_env = "GEMINI_API_KEY"

# OpenAI 兼容端点（如 Ollama / vLLM）
[providers.local]
type = "openai_response"
base_url = "http://localhost:11434/v1"
```

也可以用 CLI 子命令快速添加供应商：

```bash
neo provider add openai \
  --type openai_response \
  --base-url https://api.openai.com/v1 \
  --api-key-env OPENAI_API_KEY
```

从 [models.dev](https://models.dev) 目录导入（自动填充模型元数据）：

```bash
neo provider catalog list openai
neo provider catalog add openai --api-key sk-... --default-model gpt-4.1
```

## 第一个对话

### 交互式 TUI

```bash
neo                        # 进入交互界面
> 解释一下当前目录的代码结构
```

在提示符里输入问题回车即可发送。`Enter` 提交，`Alt+Enter` 或 `Ctrl+J` 换行。

### 一次性任务（headless）

```bash
neo run "用 Rust 写一个反转链表的函数"
```

`neo run` 接受 prompt 文本参数，并把结果以事件流打印到 stdout，适合脚本化使用。通过 `--output` 可切换输出格式：

```bash
neo run --output text "总结这个项目的架构"   # 纯文本
neo run --output json "列出所有 TODO"        # JSON 事件
```

也可用 `@文件名` 把文件内容拼进 prompt：

```bash
neo run "审查这段代码 @src/parser.rs"
```

## 速查表：常用操作

| 目标 | 命令 |
| --- | --- |
| 启动交互式 TUI | `neo` |
| 一次性 prompt | `neo run "<prompt>"` |
| 恢复上一次会话 | `neo -c` |
| 打开会话选择器 | `neo -r` |
| 列出会话 | `neo sessions list` |
| 恢复指定会话 | `neo resume <session-id>` |
| 列出已配置模型 | `neo models list` |
| 添加模型别名 | `neo models add <alias> --provider <p> --model <m>` |
| 设为默认模型 | `neo models set <alias>` |
| 列出供应商 | `neo provider list` |
| 列出 MCP 服务器 | `neo mcp list` |
| 信任当前工作区 | `neo trust approve` |

### 常用启动 flags

```bash
neo --auto             # Auto 权限模式：自动批准工具调用
neo --yolo             # YOLO 模式：自动批准工具与计划转换，但仍可向用户提问
neo --verbose          # 打印详细启动诊断
neo --config <path>    # 指定配置文件（覆盖 ~/.neo/config.toml）
```

## 下一步

- [交互模式指南](guides/interaction.md) — 多行输入、斜杠命令、权限模式与审批
- [会话管理](guides/sessions.md) — 恢复、分叉、压缩、导出
- [目标模式](guides/goals.md) — 让 Neo 自主推进一个可验证的目标
