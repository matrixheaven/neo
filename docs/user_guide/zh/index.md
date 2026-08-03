---
layout: home

hero:
  name: Neo
  text: 交互式本地编程 Agent
  tagline: 在你的终端里运行，理解你的代码库，直接帮你干活。
  actions:
    - theme: brand
      text: 快速开始
      link: /user_guide/zh/quickstart
    - theme: alt
      text: 查看指南
      link: /user_guide/zh/guides/interaction

features:
  - title: 原生工具调用
    details: 通过服务商原生 tool-call 协议直接操作文件、运行命令、搜索代码。
  - title: 多 Agent 协作
    details: 内置 Delegate 与 DelegateSwarm，可并行派发子 Agent 处理独立任务。
  - title: 权限分层
    details: Ask / Auto / YOLO / Plan 四种模式，细粒度的审批控制。
  - title: 可定制
    details: MCP 集成、Agent 技能、自定义主题、本地工作流，按需扩展。
---

Neo 是一个 Rust 原生的本地 AI 编程 Agent，以 CLI 和 TUI 两种形态运行在你的机器上：没有托管后端、没有账号、没有遥测。你自带 API key，连接 OpenAI、Anthropic、Google 或任何 OpenAI 兼容端点即可使用。

## 从这里开始

- [快速开始](quickstart.md) — 安装、配置 API key、跑通第一个对话
- [交互模式](guides/interaction.md) — 输入、权限模式、审批、排队与引导
- [会话管理](guides/sessions.md) — 恢复、分叉、压缩、导出
- [常见问题](guides/faq.md) — 遇到问题时先来这里
- [命令行参考](reference/cli.md) — 全部 CLI 命令一览
