# 常见用例

本页整理一组可直接复用的提示词模板与典型预期。每个配方按「场景 → 模板 → 预期结果」组织，可直接复制使用。

## 通用提示

- 描述越具体越好：指明文件路径、函数名与约束条件。
- 用 `@file` 引用把代码带进上下文，避免 Neo 靠猜。
- 复杂改动先 `/plan`，让 Neo 出方案、你再批准。
- 跨多个轮次的自主任务用 `/goal`。

---

## 代码审查

> 目标：让 Neo 评估一段或一组代码的质量、风险与改进点。

```
审查 @src/auth/session.rs 的安全性与错误处理。
重点关注：
1. 认证流程是否有越权或时序漏洞
2. 错误分支是否泄露敏感信息
3. 是否有可被 fuzz 的输入解析
列出按严重度排序的问题，每条给出文件:行 与修复建议。
```

**预期**：Neo 会用 Read/Grep 通读相关文件，输出一份按严重度排序的问题清单（含行号与修复建议），必要时直接给出补丁草稿。你可以继续让它「按严重度从高到低逐条修复」。

---

## 实现 feature

> 目标：实现一个完整的新功能。建议先用计划模式。

```
/plan
为 CLI 增加 `neo foo <name>` 子命令：
- 读取 ~/.neo/config.toml 里的某个 section
- 输出 JSON 或纯文本（--output text|json）
- 复用现有 clap 子命令风格
先做方案，给出涉及文件与改动顺序。
```

**预期**：Neo 进入计划模式，调研 `cli.rs` 与现有子命令的实现，把分步方案写入计划文件，并调用 `ExitPlanMode` 请求审批。批准后按方案实现并自测。

变体（跨多个轮次的自主实现）：

```
/goal 实现 neo foo 子命令并补齐单元测试，完成判据：cargo nextest run -p neo-agent 全绿
```

---

## 修复 bug

> 目标：基于现象定位并修复一个 bug。

```
现象：运行 `neo sessions list` 时，如果某条会话的 summary 字段为 null 会 panic。
栈底在 crates/neo-agent/src/modes/sessions.rs 的 list()。
@crates/neo-agent/src/modes/sessions.rs
请定位根因，给出最小修复，并补一个回归测试。
```

**预期**：Neo 用 Grep/Read 定位问题，给出最小修复方案，按权限模式直接写入或请求审批；随后补充回归测试，验证「summary 为 null 时正常列出」。

带复现命令更佳：

```
复现：NEO_HOME=/tmp/neo-empty neo sessions list
```

---

## 重构

> 目标：在不改变外部行为的前提下改进结构。务必先有测试护栏。

```
/plan
把 crates/neo-agent-core/src/runtime/turn_loop.rs 里 goal_continuation_messages()
拆成独立模块 runtime/goal_continuation.rs，保持现有调用点行为不变。
前置条件：先确认 cargo nextest run -p neo-agent-core --lib 全绿作为护栏。
```

**预期**：Neo 先运行现有测试确认绿色基线，再给出重构方案，注明会移动哪些符号、调用点如何调整；批准后执行并复测。

---

## 调研代码库

> 目标：理解一片陌生代码的结构、依赖与关键路径。纯调研，不改代码。

```
我要接手 crates/neo-tui 的输入子系统。
请给我一份导览：
1. 入口模块与对外 API
2. 事件从按键到 KeybindingAction 的流转
3. 与 InteractiveController 的边界
4. 推荐的阅读顺序
不要修改任何文件。
```

**预期**：Neo 只用只读工具，产出一份带文件:行引用的结构化导览；你可以接着问「在哪些地方加自定义键位最合适」。

快速摸底变体：

```
一句话总结 @crates/neo-agent-core/src/tools/ 下每个工具的职责，用表格输出。
```

---

## 编写测试

> 目标：为一个模块补齐单元/集成测试。

```
为 crates/neo-agent-core/src/tools/plan_mode.rs 的 prevalidate_exit_plan_mode
补一组表驱动单元测试，覆盖：
- 合法 input
- 保留字 label（approve/reject/revise）
- 重复 label
- options 超过 3 个
- suggestions 超过 5 个
保持与现有 #[cfg(test)] 风格一致。
```

**预期**：Neo 阅读现有测试风格，新增 `#[test]` 用例，运行 `cargo nextest run -p neo-agent-core --lib plan_mode` 确认全绿。

---

## 编写与更新文档

> 目标：新增或修订项目文档，与现有文档保持风格一致。

```
/plan
@docs/user_guide/zh/guides/use-cases.md
参照这份文档的结构、术语与格式，为 docs/user_guide/zh 补一页「XX 指南」。
先给出提纲，说明每节写什么，再逐节落笔。
```

**预期**：Neo 先读被引用的文档，提炼结构与风格，用 `/plan` 产出提纲；批准后按提纲写作，命令、配置键与术语保持与现有文档一致。

---

## 提示词速查

| 目标 | 推荐入口 |
| --- | --- |
| 小范围改动 | 直接 `neo run "..."` 或交互输入 |
| 中大型实现 | `/plan` 出方案 → 批准 → 执行 |
| 自主长任务 | `/goal <objective>`，配完成判据 |
| 调研 | 明确写「不要修改任何文件」 |
| 加测试 | 给出函数名、要覆盖的分支与现有测试风格 |

## 下一步

- [交互模式](interaction.md) — `/plan`、`/goal`、审批与权限模式详解
- [计划模式](plan-mode.md) — 方案审批流程
- [目标模式](goals.md) — 自主推进可验证目标
- [常见问题](faq.md) — 常见症状与解决方法
- [快速开始](../quickstart.md) — 命令与参数速查
