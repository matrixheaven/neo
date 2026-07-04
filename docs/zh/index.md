# Neo

一个 Rust 原生、纯本地运行的 AI 编程助手。

Neo 以 CLI/TUI 的形式运行在你的本机——没有托管后端、没有账号、没有遥测。自带 API Key，即可对话 OpenAI、Anthropic、Google 或任何 OpenAI 兼容端点；内置读写、编辑、grep、glob、bash、计划模式与目标跟踪等工具，全部由分层权限系统把关。

| | |
| --- | --- |
| **本地优先** | 会话、配置、技能、信任决策全部存于 `~/.neo/`，除你显式配置的 API 调用外不外发任何数据 |
| **多模型供应商** | OpenAI Responses、Anthropic Messages、Google Generative AI、Ollama、vLLM 等 |
| **可恢复会话** | 每次对话都是一份可恢复、可分叉的本地 JSONL 记录 |
| **跨平台** | macOS、Linux、Windows |

## 下一步

- [快速开始](quickstart.md) — 五分钟装好并跑通第一个对话
- [查看指南](guides/interaction.md) — 交互模式、权限、斜杠命令一网打尽
