# Coding Agent Hooks 系统横向调研报告 & Neo Hook 功能设计草案

> 调研范围：`.references/` 下 claude-code、codex、kimi-code、opencode、pi 五个 coding agent 的 hooks/plugins 系统
> 调研方式：并行子代理源码深挖 + 关键文件定点验证（事件 schema、配置 schema、执行协议、安全模型）
> 目的：为 Neo 开发 hook 功能提供事实依据与推荐设计
> 日期：2026-06（epoch 156+ 版本基线）

---

## 目录

- [第一部分 横向调研报告](#第一部分-横向调研报告)
  - [0. 结论先行](#0-结论先行)
  - [1. 五家系统总览对比](#1-五家系统总览对比)
  - [2. Claude Code（claude-code）](#2-claude-codeclaude-code)
  - [3. OpenAI Codex（codex）](#3-openai-codexcodex)
  - [4. Kimi Code（kimi-code）](#4-kimi-codekimi-code)
  - [5. opencode](#5-opencode)
  - [6. Pi](#6-pi)
  - [7. 关键维度横向对比](#7-关键维度横向对比)
  - [8. 对 Neo 的关键启示](#8-对-neo-的关键启示)
- [第二部分 Neo Hook 功能设计草案](#第二部分-neo-hook-功能设计草案)
  - [1. 设计目标与原则](#1-设计目标与原则)
  - [2. 事件模型](#2-事件模型)
  - [3. 配置格式](#3-配置格式)
  - [4. 执行协议（stdin/stdout JSON）](#4-执行协议stdinstdout-json)
  - [5. 运行时架构](#5-运行时架构)
  - [6. 生命周期](#6-生命周期)
  - [7. 与现有 Neo 架构的集成点](#7-与现有-neo-架构的集成点)
  - [8. 安全模型](#8-安全模型)
  - [9. 分期实施建议](#9-分期实施建议)
  - [10. 待决问题与风险](#10-待决问题与风险)

---

# 第一部分 横向调研报告

## 0. 结论先行

1. **hooks 已是行业标配**：5 个 reference 项目中，claude-code / codex / kimi-code / opencode 都有成熟的 hooks 机制；pi 采用进程内 TS 扩展（jiti）而非子进程 hook。
2. **两种主流实现范式**：
   - **子进程 + JSON 协议**（claude-code、codex、kimi-code、opencode 的 v1 风格）：hook 配置为「事件 → matcher → 命令」，运行时以子进程执行，stdin 传入事件 JSON，stdout 返回决策 JSON，`exit code 2` 表示阻断。
   - **进程内插件 API**（opencode v2、pi）：TS/JS 插件函数注册 hook，进程内 await 调用，可拿 client/SDK 引用做任意扩展。
3. **事件集高度趋同**：核心事件为 `PreToolUse` / `PostToolUse` / `UserPromptSubmit` / `SessionStart` / `SessionEnd` / `Stop` / `SubagentStart` / `SubagentStop` / `PreCompact` / `PostCompact` / `Notification`；claude-code 与 codex 还扩展了 `PermissionRequest` / `PermissionDenied`，claude-code 是唯一实现 `Elicitation` / `FileChanged` / `WorktreeCreate` / `TaskCreated` 等长尾事件的系统。
4. **安全模型三层**：信任门禁（未信任的 hook 不运行）→ 配置来源分级（System/User/Project/MDM/Plugin）→ 输出治理（hook 只能返回受控决策字段，不能任意注入系统提示）。
5. **对 Neo 的推荐**：采用「配置驱动 + 子进程 JSON 协议 + 信任/预算治理」的薄引擎，事件集取五家交集，优先落地 `PreToolUse` / `PostToolUse` / `UserPromptSubmit` / `SessionStart` / `Stop` / `SubagentStop`，后续再考虑进程内插件 API（可复用 Neo 已有的 MCP/技能基础设施）。

---

## 1. 五家系统总览对比

| 维度 | claude-code | codex | kimi-code | opencode | pi |
|---|---|---|---|---|---|
| 实现语言 | TypeScript | Rust (codex-rs) | TypeScript | TS + Go（新 v2） | TypeScript |
| 范式 | 子进程 + JSON | 子进程 + JSON（另有 Rust 插件分发层） | 子进程 + JSON | 进程内插件 API（v2）+ 事件总线 | 进程内 TS 扩展（jiti） |
| 配置位置 | `settings.json` 系列 | `config.toml`（managed-hooks-only） | 配置 schema `HookDefSchema` | `opencode.json` + `plugins/` 目录 | `settings.json` 的 `extensions`/`packages` |
| 配置结构 | `hooks` → 事件 → `[{matcher, hooks[]}]` | `[hooks]` 表 + 插件 manifest | `{event, matcher?, command, timeout?}` | JS 模块导出 hooks 对象 | 事件 → 处理器映射 |
| 事件数 | 28 | 11 | 16 | ~30（v2 事件总线） | 事件驱动（数量随扩展） |
| 阻断方式 | exit 2 / JSON `decision` | JSON entries（Stop 类） | exit 2 / JSON `action: block` | throw / decision approve-reject | 结果类型 block/transform/patch |
| 超时 | 默认 10min（可 async 放行） | 配置化 | 默认 30s（fail-open） | 进程内无独立超时 | 进程内无独立超时 |
| 信任门禁 | ✅ workspace trust + `allowManagedHooksOnly` | ✅ `HookTrustStatus`（Managed/Trusted/Modified） | ⚠️ 部分（plugin manifest 注入） | ⚠️ 文档级 | ✅ `trust-manager.ts` 门控加载 |
| 上下文注入 | `additionalContext` / `initialUserMessage` / `watchPaths` | `additional_context` map | 未发现 | `additionalContext` | patch/transform 结果 |
| 工具输入改写 | ✅ `updatedInput`（PreToolUse） | ✅（PreToolUse 拦截/改写） | ❌ 未发现 | ✅ `output.args` 改写 | ✅ transform |

---

## 2. Claude Code（claude-code）

> 证据基线：`src/entrypoints/sdk/coreSchemas.ts`、`src/types/hooks.ts`、`src/utils/hooks/{hooksConfigManager,execAgentHook,execPromptHook}.ts`、`src/utils/settings/*`

### 2.1 配置格式与位置

- hooks 配置在 **settings.json 系列** 的 `"hooks"` 键下：项目级 `.claude/settings.json`、用户级 `~/.claude/settings.json`、本地级 `.claude/settings.local.json`（另有企业托管配置）。
- 结构：事件名 → **matcher 数组**；每个 matcher 含可选的 `matcher` 字符串与 `hooks` 命令数组。

```jsonc
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash|Write",   // 精确工具名 / "|" 分隔 / 正则 /^\/.*$/i
        "hooks": [
          "node ~/.claude/hooks/guard.mjs",
          "python3 ~/.claude/hooks/audit.py"
        ]
      }
    ],
    "UserPromptSubmit": [{ "hooks": ["echo prompt-log"] }]
  }
}
```

- `matcher` 语法（claude-code 独有细节）：精确匹配、`|` 分隔的备选集合、或正则（`/.../i`）。不写 matcher 表示匹配该事件全部。

### 2.2 事件全集（28 个，`coreSchemas.ts:355-383`）

| 类别 | 事件 |
|---|---|
| 工具 | `PreToolUse`、`PostToolUse`、`PostToolUseFailure` |
| 用户输入 | `UserPromptSubmit` |
| 会话 | `SessionStart`（source: startup/resume/clear/compact）、`SessionEnd`、`Setup`（init/maintenance）、`ConfigChange`、`CwdChanged`、`FileChanged`、`InstructionsLoaded` |
| 停止 | `Stop`（`stop_hook_active`、`last_assistant_message`）、`StopFailure` |
| 子代理 | `SubagentStart`、`SubagentStop`（`agent_id`、`agent_transcript_path`）、`TeammateIdle`、`TaskCreated`、`TaskCompleted` |
| 压缩 | `PreCompact`（manual/auto）、`PostCompact`（`compact_summary`） |
| 权限 | `PermissionRequest`（可返回 allow+updatedPermissions / deny+interrupt）、`PermissionDenied` |
| 通知/其它 | `Notification`、`Elicitation`、`ElicitationResult`、`WorktreeCreate`、`WorktreeRemove` |

### 2.3 执行方式与协议

- **子进程执行**，命令经 shell 展开（命令即 `hooks` 数组元素）。
- **stdin 输入**（`coreSchemas.ts:387-411`）：所有事件共有基座字段：

```json
{
  "session_id": "…", "transcript_path": "…", "cwd": "…",
  "permission_mode": "default|acceptEdits|plan|bypassPermissions|…",
  "agent_id": "…", "agent_type": "…",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash", "tool_input": { "command": "rm -rf x" }, "tool_use_id": "toolu_…"
}
```

- **stdout 输出**（`hooks.ts:50-166`）同步响应 schema：

```json
{
  "continue": true,
  "suppressOutput": true,
  "stopReason": "blocked by guard",
  "decision": "approve",
  "reason": "…",
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "permissionDecisionReason": "…",
    "updatedInput": { "command": "echo 安全命令" },
    "additionalContext": "追加到上下文的文本"
  }
}
```

- **异步协议**：hook 可在首行输出 `{"async": true, "asyncTimeout": N}` 后立即放行，实际执行在后台继续（`isAsyncHookJSONOutput`）。
- **阻断通道**：`exit code 2` = 阻断（停止继续）；`exit 0` = 继续；其它非零码按失败处理。
- **超时**：默认约 10 分钟；超时 kill 子进程，按失败处理。
- **执行并发**：同一事件的多个 hook **并行**执行；所有 hook 均需通过 workspace trust 门禁才会运行。

### 2.4 生命周期与执行顺序

1. hook 在 **权限检查之前** 运行（PreToolUse 可先于 PermissionMode 决策）。
2. hook 的 `permissionDecision: allow` **不会绕过** 既有 deny/ask 规则；反之 hook 的 `deny`/`block` 直接阻断工具。
3. `UserPromptSubmit` hook 可阻断（`continue: false`）或截断用户输入并注入 `additionalContext`。
4. `Stop` hook 可阻止会话停止（`stop_hook_active` 语义）。
5. `SessionStart` hook 可注入 `initialUserMessage` 与 `watchPaths`（FileChanged 观察路径）。
6. `PostToolUse` 可改写 MCP 工具输出（`updatedMCPToolOutput`）。

### 2.5 权限与安全模型

- 所有 hook 必须通过 **workspace trust**（`~/.claude/trustedProjects` 体系），未信任项目不执行 hook。
- 企业管控：`allowManagedHooksOnly` 只允许托管 hook。
- hook 输出受 schema 约束（Zod），非法输出被拒绝；`permissionMode` 以只读方式传给 hook，hook 不能提升自身权限。

### 2.6 限制与最佳实践

- 文档明确：hook **不应调用 LLM**（避免递归/成本）；输出大小受限；超时后按失败处理。
- 常见用途：安全守卫（禁止危险命令/路径）、审计日志、自动格式化（PostToolUse）、通知（Notification）、上下文增强（additionalContext）。

---

## 3. OpenAI Codex（codex）

> 证据基线：`codex-rs/protocol/src/protocol.rs:1488-1614`、`codex-rs/core-plugins/src/{loader,manager,store,marketplace,manifest}.rs`、`codex-rs/plugin/src/*`、`codex-rs/docs/config.md`

### 3.1 机制概述

- Codex 的 hook 是 **「配置驱动 + 子进程 JSON 协议 + 信任/预算治理」的薄引擎**；Rust plugin（`core-plugins` crate：loader/manager/store/marketplace/manifest）只是**文件分发层**（把 hook 命令/资源分发到本地），不是 hook 执行层。
- 仓库内唯一 hook 文档为 `codex-rs/docs/config.md`，明确 **managed-hooks-only**：未受信任/被修改的 hook 不执行。

### 3.2 协议级类型（`protocol.rs`）

| 类型 | 值域 | 说明 |
|---|---|---|
| `HookEventName` | `PreToolUse`、`PermissionRequest`、`PostToolUse`、`PreCompact`、`PostCompact`、`SessionStart`、`SessionEnd`、`UserPromptSubmit`、`SubagentStart`、`SubagentStop`、`Stop` | 11 个事件，与 claude-code 交集高度重合 |
| `HookHandlerType` | `Command`、`Prompt`、`Agent` | 命令 / 提示词 / 子代理三种 handler |
| `HookExecutionMode` | `Sync`、`Async` | 同步阻塞 / 异步放行 |
| `HookScope` | `Thread`、`Turn` | 线程级 / 单轮级作用域 |
| `HookSource` | `System`、`User`、`Project`、`Mdm`、`SessionFlags`、`Plugin`、`CloudRequirements`、`CloudManagedConfig`、`LegacyManagedConfigFile`、`LegacyManagedConfigMdm` | 配置来源分级，安全模型的核心 |
| `HookTrustStatus` | `Managed`、`Untrusted`、`Trusted`、`Modified` | 信任状态机：托管 / 未信任 / 已信任 / 被修改 |
| `HookRunStatus` | `Running`、`Completed`、`Failed`、`Blocked`、`Stopped` | 运行状态 |
| `HookOutputEntryKind` | `Warning`、`Stop`、`Feedback`、`Context`、`Error` | 输出条目类型，`Stop` = 阻断 |
| `HookRunSummary` | id、event_name、handler_type、execution_mode、scope、source_path、source、display_order、status、status_message、started_at、completed_at、duration_ms、entries | 一次 hook 运行的完整可观测摘要 |

- 运行时以 `HookStartedEvent` / `HookCompletedEvent` 事件对外发布（带 `turn_id` 与 `HookRunSummary`），TUI/审计可直接渲染。
- 上下文注入：`additional_context: BTreeMap<String, AdditionalContextEntry>`（`protocol.rs:558`）。

### 3.3 对 Neo 的启示

- **信任状态机**（Managed/Untrusted/Trusted/Modified + 配置来源分级）是 codex 最值得借鉴的部分——按来源决定 hook 的信任基线，hash 校验决定是否 `Modified`。
- 事件集刻意克制（11 个），全部落在 Neo 现有 `AgentEvent` 可映射的范围内。
- `HookRunSummary` 的 `display_order` / `duration_ms` / `status` 是「hook 运行可观测化」的范本，可直接映射为 Neo 的 `AgentEvent` 变体。

---

## 4. Kimi Code（kimi-code）

> 证据基线：`packages/agent-core/src/config/schema.ts:229-236`、`packages/agent-core/src/session/hooks/types.ts`（全文件 72 行）、`packages/agent-core/src/session/hooks/{runner,user-prompt}.ts`

### 4.1 配置格式（最小可用设计）

```ts
// schema.ts:229-236
export const HookDefSchema = z.object({
  event: z.enum(HOOK_EVENT_TYPES),
  matcher: z.string().optional(),       // 正则
  command: z.string().min(1),           // 单一 shell 命令
  timeout: z.number().int().min(1).max(600).optional(),
}).strict();

// types.ts:24-31 运行时接口补充
export interface HookDef {
  readonly event: HookEventType;
  readonly matcher?: string;
  readonly command: string;
  readonly timeout?: number;
  readonly cwd?: string;
  readonly env?: Readonly<Record<string, string>>;
}
```

- **单事件单命令**（一个 HookDef 一个 command），比 claude-code 的 matcher 数组更简单；matcher 为**正则**。

### 4.2 事件全集（16 个，`types.ts:3-20`）

`PreToolUse`、`PostToolUse`、`PostToolUseFailure`、`PermissionRequest`、`PermissionResult`、`UserPromptSubmit`、`Stop`、`StopFailure`、`Interrupt`、`SessionStart`、`SessionEnd`、`SubagentStart`、`SubagentStop`、`PreCompact`、`PostCompact`、`Notification`

- 独有事件：`PermissionResult`（权限结果回执）、`Interrupt`（用户打断）。缺少 `SessionEnd` 之外的 opencode 式长尾事件。

### 4.3 执行方式与协议

- **单一 shell 命令** + **JSON stdin/stdout 协议**；**双通道阻断**：`exit code 2` 或 stdout JSON `{"action": "block"}` 均可阻断。
- 结果模型（`types.ts:33-42`）：

```ts
interface HookResult {
  readonly action: 'allow' | 'block';
  readonly message?: string;   // 展示给用户的信息
  readonly reason?: string;
  readonly stdout?: string; readonly stderr?: string;
  readonly exitCode?: number; readonly timedOut?: boolean;
  readonly structuredOutput?: boolean;
}
```

- **超时**：默认 30s，**fail-open**（超时按 allow 处理，不阻断主流程）。
- **并发**：同一事件的多个 hook **并行**执行，**第一个 block 生效**。
- **触发分类**：fire-and-forget（通知类，如 Notification）与阻塞触发（PreToolUse）分开处理。
- **插件注入**：plugin 通过 manifest 的 `hooks` 字段向 hook 注入 `cwd` / `env`。
- **无钩子间顺序保证**：外部 hooks 之间无顺序契约（顺序只存在于业务域内部的有序 slot）；**无 hook 权限审批机制**。

### 4.4 对 Neo 的启示

- 「单命令 + 双通道阻断 + fail-open 超时」是最简可用基线，适合 Neo 的 M1。
- 30s 默认超时 + fail-open 对用户感知更友好（claude-code 的 10min 默认对守卫类 hook 过慢）。

---

## 5. opencode

> 证据基线：`packages/web/src/content/docs/plugins.mdx`（事件表 146-208 行）、`packages/opencode/src/session/llm/request.ts:69,114,134`、`packages/opencode/src/session/status.ts:39-48`、`packages/opencode/src/tool/code-mode.ts:141-185`、`packages/core/src/plugin/{host.ts,plugin.ts}`、`packages/plugin/src/v2/effect/README.md`

### 5.1 机制概述（进程内插件 API）

- 插件是 **JS/TS 模块**：`.opencode/plugins/`（项目级）与 `~/.config/opencode/plugins/`（全局）自动加载；npm 包经 `opencode.json` 的 `"plugin": [...]` 字段安装（Bun 安装到 `~/.cache/opencode/node_modules/`）。
- **加载顺序**：全局 config → 项目 config → 全局插件目录 → 项目插件目录；同名去重。
- 插件函数签名：`export const MyPlugin = async ({ project, client, $, directory, worktree }) => ({ hooks })`——`client` 是 opencode SDK 客户端，`$` 是 Bun shell。

### 5.2 事件全集（按类别，docs 146-208）

| 类别 | 事件 |
|---|---|
| 命令 | `command.executed` |
| 文件 | `file.edited`、`file.watcher.updated` |
| 安装 | `installation.updated` |
| LSP | `lsp.client.diagnostics`、`lsp.updated` |
| 消息 | `message.part.removed/updated`、`message.removed/updated` |
| 权限 | `permission.asked`、`permission.replied` |
| 服务器 | `server.connected` |
| 会话 | `session.created`、`session.compacted`、`session.deleted`、`session.diff`、`session.error`、`session.idle`、`session.status`、`session.updated` |
| 其它 | `todo.updated`、`shell.env`、`tui.prompt.append`、`tui.command.execute`、`tui.toast.show` |
| 工具 | **`tool.execute.before`**、**`tool.execute.after`**（另有 `tool.execute.error`、`chat.params`、`chat.message`、`chat.idle`、`session.start/end`、`event`、`subagent.start` 等 v2 事件） |

### 5.3 Hook 行为

- `tool.execute.before` 可**改写**工具参数（`output.args.command` 转义示例）或 `throw` 阻断：

```ts
export const EnvProtection = async ({}) => ({
  "tool.execute.before": async (input, output) => {
    if (input.tool === "read" && output.args.filePath.includes(".env")) {
      throw new Error("Do not read .env files")
    }
  },
})
```

- 决策语义：`approve` / `reject` / `allow` / `miss`；hook 可与 permission 系统联动（`code-mode.ts:141-185`：MCP 工具钩子 + permission 顺序）。
- `chat.params` / `headers` / `system.transform` 系列 hook 可改写发给模型的请求参数与系统提示（`request.ts`）。
- `session.idle` 用于通知类场景（`session/status.ts:39-48`）。
- **顺序**：所有 hook **按加载顺序依次执行**（docs「all hooks run in sequence」），与 claude-code/kimi 的并行不同。
- v2 提供 Effect 插件 API（`packages/plugin/src/v2/effect/README.md`），类型安全、可组合。

### 5.4 对 Neo 的启示

- opencode 证明**进程内插件 API 可以做得比子进程协议更强大**（可直接改写系统提示、访问 SDK），但代价是安全面更大（任意代码在进程内运行）。Neo 若做插件 API，需要更重的信任模型。
- 事件命名（`tool.execute.before/after`、`session.idle`）清晰，可作为 Neo 事件命名的参考系。

---

## 6. Pi

> 证据基线：`packages/coding-agent/src/core/trust-manager.ts:29-37`、`packages/coding-agent/src/core/tools/bash.ts:150-197`、`packages/coding-agent/src/cli/args.ts:149-152,262-263`、settings.json `extensions`/`packages` 字段

### 6.1 机制概述（进程内 TS 扩展）

- Pi 的 hook = **进程内 TypeScript 扩展**，经 **jiti** 加载；配置在 `settings.json` 的 `extensions` / `packages` 字段，CLI 侧 `-e/--extension`、`-ne` 控制。
- **无子进程式 hook、无 JSON 命令配置、无 stdin/stdout 协议**——与其余四家形成最明显反差。
- **扩展信任门控**：`trust-manager.ts` 决定哪些扩展可加载（29-37 行），未信任扩展不执行。

### 6.2 Hook 行为

- **顺序 await**：事件处理器按注册顺序逐个 await，天然有序。
- **事件自带结果类型**：可 `block`（阻断）、`transform`（改写）、`patch`（修补）。
- **工具钩子挂执行管线**：参数校验后（before）/ 执行后（after）；示例 `BashSpawnHook`（`bash.ts:150-197`）可拦截/改写 bash 命令。
- **错误策略分事件类型**：普通事件出错 continue，tool_call 事件出错 block（防呆设计）。
- **权限门禁即 hook**：permission 决策本身实现为 hook，与工具钩子同一机制。
- **`/reload` 热重载**：扩展变更无需重启。

### 6.3 对 Neo 的启示

- 「事件自带结果类型（block/transform/patch）」与「权限门禁即 hook」值得借鉴：Neo 的 `PermissionMode` 决策链可以预留 hook 参与位。
- 进程内扩展的风险：与 opencode 相同，安全面大；Pi 用 trust-manager 门控兜底。

---

## 7. 关键维度横向对比

### 7.1 事件覆盖矩阵

| 事件 | claude-code | codex | kimi-code | opencode | pi |
|---|:-:|:-:|:-:|:-:|:-:|
| PreToolUse | ✅ | ✅ | ✅ | ✅(tool.execute.before) | ✅ |
| PostToolUse | ✅ | ✅ | ✅ | ✅(tool.execute.after) | ✅ |
| PostToolUseFailure | ✅ | ❌ | ✅ | ✅(tool.execute.error) | ~ |
| UserPromptSubmit | ✅ | ✅ | ✅ | ✅(chat.message) | ~ |
| SessionStart | ✅ | ✅ | ✅ | ✅(session.start) | ~ |
| SessionEnd | ✅ | ✅ | ✅ | ✅(session.end) | ~ |
| Stop | ✅ | ✅ | ✅ | ~ | ~ |
| SubagentStart/Stop | ✅ | ✅ | ✅ | ✅(subagent.start) | ~ |
| PreCompact/PostCompact | ✅ | ✅ | ✅ | ✅(session.compacted) | ❌ |
| Notification | ✅ | ~ | ✅ | ~ | ~ |
| PermissionRequest/Denied | ✅ | ✅ | ✅(PermissionResult) | ✅(permission.asked/replied) | ✅(权限即 hook) |
| 长尾（Elicitation/FileChanged/Worktree/Task） | ✅ | ❌ | ❌ | ~ | ❌ |

（✅=有证据；~=机制存在但无直接对应事件；❌=未发现）

### 7.2 执行协议对比

| 维度 | claude-code | codex | kimi-code | opencode | pi |
|---|---|---|---|---|---|
| 执行体 | shell 子进程 | 子进程（Command/Prompt/Agent） | shell 子进程 | 进程内函数 | 进程内函数 |
| 输入 | stdin JSON（含 session/transcript/cwd/permission_mode） | stdin JSON（协议类型化） | stdin JSON | 函数入参 input | 事件对象 |
| 输出 | stdout JSON（decision/permissionDecision/additionalContext/updatedInput） | stdout entries（Stop/Context/Feedback…） | stdout JSON（action allow/block） | 返回值 / throw | block/transform/patch |
| 阻断通道 | exit 2 + JSON | JSON entries | exit 2 + JSON | throw / decision | 结果类型 |
| 并发 | 并行 | 并行 | 并行（首 block 生效） | 串行（加载顺序） | 串行 |
| 超时 | 10min 默认 | 配置化 | 30s fail-open | 无独立超时 | 无独立超时 |
| 异步放行 | ✅ async 首行协议 | ✅ Sync/Async 模式 | ❌ | n/a | n/a |

### 7.3 安全模型对比

| 维度 | claude-code | codex | kimi-code | opencode | pi |
|---|---|---|---|---|---|
| 信任门禁 | workspace trust + allowManagedHooksOnly | HookTrustStatus 状态机 + 来源分级 | 弱（plugin 注入 cwd/env） | 弱（目录即信任） | trust-manager 门控 |
| 来源分级 | settings 层级 | System/User/Project/Mdm/Plugin/… | 配置层级 | config/plugins 目录层级 | settings 层级 |
| 输出治理 | Zod schema 强校验 | 类型化 entries | 结构化输出标记 | 类型化 | 类型化 |
| 绕过风险 | 低 | 低 | 中（fail-open） | 中（进程内任意代码） | 中（进程内任意代码） |

---

## 8. 对 Neo 的关键启示

1. **薄引擎 + 子进程协议是性价比最高的起点**：claude-code/codex/kimi 三家的共识是「配置驱动 + JSON 子进程协议」，Neo 可直接复用同一协议形状，且完全符合 Neo「跨平台、无 unsafe、类型化接口」的约束（子进程协议天然跨平台，无需嵌入 JS 运行时）。
2. **事件集取交集，命名对齐 claude-code**：`PreToolUse` / `PostToolUse` / `UserPromptSubmit` / `SessionStart` / `SessionEnd` / `Stop` / `SubagentStart` / `SubagentStop` / `PreCompact` / `PostCompact` / `Notification` / `PermissionRequest` 这 12 个事件覆盖五家 90% 的能力，且全部能映射到 Neo 现有 `AgentEvent`。
3. **信任模型照搬 codex 状态机，配置来源照搬分级**：`Managed/Trusted/Untrusted/Modified` + 来源分级，配合 Neo 已有的 `~/.neo/trust.json` 门禁（AGENTS.md 信任链）是天然契合点。
4. **阻断通道用「exit 2 + stdout JSON 双通道」**（kimi/claude-code 共识），超时默认 30s fail-open（kimi）或按事件类配置。
5. **hook 决策与 PermissionMode 的关系**（claude-code 语义）：hook 先于权限检查，hook allow 不绕过 deny/ask，hook deny 直接阻断——这可以直接嵌入 Neo `tool_dispatch.rs` 的 `authorize_tool_batch` 之前。
6. **可观测性**：codex 的 `HookRunSummary`（display_order/duration_ms/status）+ `HookStarted/Completed` 事件 = Neo `AgentEvent` 新变体，TUI 与 JSONL 天然受益。
7. **进程内插件 API（opencode v2 / pi）后置**：属于 M2/M3，需要独立的信任与沙箱设计，不应阻塞 M1。

---

# 第二部分 Neo Hook 功能设计草案

## 1. 设计目标与原则

**目标**：为 Neo 增加「配置驱动 + 子进程 JSON 协议」的 hooks 系统，让用户可以在会话/轮次/工具执行的关键节点注入外部脚本（守卫、审计、通知、上下文增强），且不影响 Neo 的上下文完整性与权限边界。

**原则**（对齐 Neo AGENTS.md 的硬约束）：

1. **不碰上下文前缀**：hook 的 `additionalContext` 只能作为**新事件**追加注入（走 `MessageAppended`/`InstructionEpoch` 等价通道），绝不改写历史。
2. **跨平台**：hook 命令统一用 `std::process::Command` 直接执行（不经 shell），Windows/Linux/macOS 一致；不引入 Unix 信号依赖（超时用 `wait_timeout` 等价机制或 `tokio::time::timeout` + kill 进程树）。
3. **无 unsafe、类型化**：hook 协议用 serde 结构体（`HookInput`/`HookOutput`），复用 `schemars::JsonSchema` 生成文档。
4. **安全默认**：未信任的 hook 不执行；hook 拿不到模型上下文原文，只能拿事件输入；hook 不能提升权限。
5. **不依赖 LLM**：hook 执行不回调模型（文档级约束 + 代码上无此路径）。
6. **最小可落地**：M1 只做子进程协议 + 6 个核心事件，进程内插件 API 后置。

## 2. 事件模型

### 2.1 NeoHookEvent 枚举（M1 = 6 个，M2 扩到 12 个）

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub enum HookEvent {
    // M1 核心
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
    Stop,
    // M2 扩展
    PostToolUseFailure,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    Notification,
    PermissionRequest,   // 与 PermissionMode 联动
}
```

### 2.2 与现有 `AgentEvent` 的映射（事件来源）

| NeoHookEvent | 触发锚点（Neo 现有 AgentEvent / 代码位置） | 输入负载要点 |
|---|---|---|
| `SessionStart` | `RunStarted`（`events.rs:104`）+ 会话初始化 | source(startup/resume)、cwd、session_id |
| `SessionEnd` | `RunFinished`（`events.rs:360`） | stop_reason |
| `UserPromptSubmit` | 用户消息入队（`MessageAppended` 前置点） | prompt 原文、队列位置 |
| `PreToolUse` | `execute_tool_calls` 内、`authorize_tool_batch` **之前**（`tool_dispatch.rs:1039` 前） | tool_name、tool_input、tool_use_id |
| `PostToolUse` | `execute_prepared_tool` 返回后、`finalize_authorized_batch`（`tool_dispatch.rs:1072`） | tool_name、tool_input、tool_result |
| `PostToolUseFailure` | `ToolExecutionFinished` 且 result 为 Err | 同上 + error |
| `Stop` | `RunFinished`/`TurnFinished` 前 | stop_hook_active、last_assistant_message |
| `SubagentStart/Stop` | `DelegateStarted` / `DelegateFinished`（`events.rs:402/420`） | agent_id、agent_type |
| `PreCompact` | `CompactionStarted`（`events.rs:334`） | trigger(manual/auto) |
| `PostCompact` | `CompactionApplied`（`events.rs:343`） | compact_summary |
| `Notification` | 预留（TUI 通知点） | message、title、type |
| `PermissionRequest` | `authorize_tool_batch` 内部（权限弹窗前） | tool_name、tool_input、permission_suggestions |

### 2.3 Hook 运行的可观测事件（新 AgentEvent 变体）

```rust
HookStarted {
    turn: u32,
    run: HookRunSummary,        // id, event, command, display_order, source_path
}
HookCompleted {
    turn: u32,
    run: HookRunSummary,        // + status: Running/Completed/Failed/Blocked/Stopped
                                // + duration_ms, entries
}
```

（对齐 codex `HookStartedEvent`/`HookCompletedEvent`；TUI 可渲染为可折叠卡片，JSONL 天然记录。）

## 3. 配置格式

### 3.1 位置（对齐 Neo「无项目本地配置」的架构）

- 全局：`~/.neo/config.toml` 的 `[hooks]` 段（或 `~/.neo/hooks.toml`）。
- 项目级：**可选**，经 `~/.neo/trust.json` 信任门禁后才生效（与 AGENTS.md 信任链同源）。

### 3.2 Schema（TOML，对齐 kimi 的单命令 + claude-code 的 matcher）

```toml
# ~/.neo/config.toml
[hooks]

# 每个事件可挂多个 hook；同一事件按声明顺序执行（M1 串行，保持确定性）
[[hooks.PreToolUse]]
matcher = "Bash|Write"        # 可选：精确工具名 / "|" 分隔 / 正则 /^\/.*$/i
command = "node ~/.neo/hooks/guard.mjs"
timeout_secs = 30             # 可选，默认 30；超时策略见 [hooks.runtime]
fail_open = false             # 可选，默认 false（守卫类默认 fail-closed）
env = { EXTRA = "value" }     # 可选，附加环境变量

[[hooks.PreToolUse]]
matcher = "/^Read$/"
command = "python3 -m neo_hooks.audit"

[[hooks.UserPromptSubmit]]
command = "~/bin/prompt-filter.sh"

[[hooks.Stop]]
command = "notify-send 'neo stopped'"

[hooks.runtime]
default_timeout_secs = 30     # 默认超时（kimi 基线）
fail_open_default = false     # 全局默认：失败时放行（false=阻断）
max_hooks_per_event = 8       # 单事件 hook 数上限（防配置失控）
output_max_bytes = 65536      # stdout 采集上限（防内存放大）
```

### 3.3 事件 → matcher → 命令的求值语义

- 事件触发 → 收集该事件全部 hook → 按声明顺序逐个求值 matcher（不写 matcher = 全匹配）→ 命中者进入执行队列。
- M1 **串行执行**（确定性、顺序可预期；与 opencode/pi 一致），M2 可加 `parallel = true` 选项（对齐 claude-code/kimi 并行 + 首 block 生效）。

## 4. 执行协议（stdin/stdout JSON）

### 4.1 输入（stdin，单行 JSON，UTF-8）

```json
{
  "event": "PreToolUse",
  "session_id": "wd_neo_ab12cd34ef56",
  "cwd": "/Users/me/workspace",
  "permission_mode": "auto",
  "turn": 7,
  "hook_command": "node ~/.neo/hooks/guard.mjs",
  "tool_name": "Bash",
  "tool_input": { "command": "rm -rf /" },
  "tool_use_id": "toolu_01",
  "tool_result": null
}
```

字段说明：

| 字段 | 出现事件 | 说明 |
|---|---|---|
| `event` | 全部 | 事件名（PascalCase） |
| `session_id` | 全部 | 会话 ID |
| `cwd` | 全部 | 工作目录（**hook 的默认工作目录**，与工具一致） |
| `permission_mode` | 全部 | `ask`/`auto`/`yolo`（只读，防 hook 提升权限） |
| `turn` | 轮次内事件 | 当前 turn |
| `tool_name` / `tool_input` / `tool_use_id` | 工具类事件 | 与 `AgentEvent::ToolExecutionStarted` 同源数据 |
| `tool_result` | `PostToolUse` | `ToolResult` 序列化 |
| `prompt` | `UserPromptSubmit` | 用户输入原文 |
| `stop_hook_active` / `last_assistant_message` | `Stop` | 停止上下文 |

### 4.2 输出（stdout，单行 JSON；可先输出 `{"async":true}` 放行）

```json
{
  "decision": "continue",
  "reason": "…",
  "permissionDecision": "allow",
  "suppressOutput": true,
  "hookSpecificOutput": {
    "updatedInput": { "command": "echo safe" },
    "additionalContext": "…"
  }
}
```

字段语义：

| 字段 | 取值 | 语义 |
|---|---|---|
| `decision` | `continue` / `block` | `block` 阻断当前操作（与 exit 2 等效） |
| `permissionDecision` | `allow` / `deny` / `ask` | 仅 `PreToolUse`/`PermissionRequest` 生效；**`allow` 不绕过既有 deny/ask 规则** |
| `updatedInput` | 对象 | 仅 `PreToolUse`：改写工具参数（claude-code 语义） |
| `additionalContext` | 字符串 | 追加注入上下文（新事件通道，不改历史） |
| `suppressOutput` | bool | 隐藏 hook 自身输出（审计类 hook） |
| `reason` / `message` | 字符串 | 展示给用户的阻断原因 |

### 4.3 退出码与失败语义

| 情况 | 行为 |
|---|---|
| exit 0 + 无 stdout JSON | 视为 `continue` |
| exit 2 | 视为 `block`（阻断） |
| stdout 含合法 JSON | JSON 优先；JSON 与 exit code 冲突时以 JSON `decision` 为准（kimi 双通道语义） |
| 超时（默认 30s） | kill 进程树；按 `fail_open` 配置：true=continue（记 warning），false=block |
| 命令不存在 / 非零退出 | 同超时策略 + 记录 `HookCompleted(status=Failed)` |
| stdout 非法 JSON | 视为无 JSON；按 exit code 处理 |
| 输出超限 | 截断采集，记 warning（`output_max_bytes`） |

## 5. 运行时架构

### 5.1 模块划分（新增 `crates/neo-agent-core/src/hooks/`）

```
crates/neo-agent-core/src/hooks/
├── mod.rs              # 对外 API：HookManager::new / handle_event / preflight
├── event.rs            # HookEvent 枚举 + matcher 求值
├── config.rs           # [hooks] TOML 解析（serde + JsonSchema）
├── runner.rs           # HookRunner：Command 构建、stdin 写、stdout 读、超时/kill、exit 映射
├── protocol.rs         # HookInput / HookOutput / HookRunSummary（serde 类型）
├── decisions.rs        # 决策聚合：多 hook 结果 → 最终 decision / permissionDecision / updatedInput / additionalContext
├── trust.rs            # 信任门禁：HookSource 分级 + trust.json 校验 + (M2) hash 校验 Modified 检测
└── observer.rs         # HookStarted/HookCompleted → EventEmitter（AgentEvent 新变体）
```

### 5.2 数据流（字符画）

```
                    ┌─────────────────────────────────────────────────────────────┐
                    │                    AgentRuntime (neo-agent-core)            │
                    │                                                             │
 模型流 ──────────▶ │  turn loop                                                  │
                    │     │                                                       │
                    │     ▼                                                       │
                    │  execute_tool_calls (tool_dispatch.rs)                      │
                    │     │                                                       │
                    │     ├─ prepare_tool_calls ──▶  instruction preflight        │
                    │     │                              │                        │
                    │     ▼                              ▼                        │
                    │  ┌──────────────────────────────────────────────────┐       │
                    │  │  HookManager.dispatch(PreToolUse, ctx)           │       │
                    │  │  ┌──────────────────────────────────────────┐    │       │
                    │  │  │ trust.rs 门禁 → config 命中 matcher      │    │       │
                    │  │  │ runner.rs: spawn → stdin JSON → stdout   │    │       │
                    │  │  │ decisions.rs: block? updatedInput?       │    │       │
                    │  │  │ observer.rs: emit HookStarted/Completed  │    │       │
                    │  │  └──────────────────────────────────────────┘    │       │
                    │  └──────────────────────────────────────────────────┘       │
                    │     │ block → 短路（ToolExecutionFinished(Err)）            │
                    │     ▼ continue / updatedInput                              │
                    │  authorize_tool_batch (PermissionMode × PermissionOperation)│
                    │     │                                                       │
                    │     ▼                                                       │
                    │  execute_authorized_batch → execute_prepared_tool            │
                    │     │                                                       │
                    │     ▼                                                       │
                    │  ┌──────────────────────────────────────────────────┐       │
                    │  │  HookManager.dispatch(PostToolUse, ctx+result)   │       │
                    │  │  (additionalContext → 新 AgentEvent 注入通道)    │       │
                    │  └──────────────────────────────────────────────────┘       │
                    │     │                                                       │
                    │     ▼                                                       │
                    │  finalize_authorized_batch ──▶ 结果回填模型                 │
                    └─────────────────────────────────────────────────────────────┘

  外部 hook 子进程（node/python/sh/…）
        ▲  stdin: 单行 JSON（事件负载）
        │  stdout: 单行 JSON（决策）
        │  exit 2 = block；超时 = kill
```

### 5.3 决策聚合规则（多 hook）

```
事件触发
  │
  ├─ 按声明顺序执行 hook₁ … hookₙ（M1 串行）
  │    每个 hook 产出 (decision, permissionDecision?, updatedInput?, additionalContext?)
  │
  ├─ 短路：任一 hook decision=block → 整体 block（reason 取首个 block 的 reason）
  │
  ├─ permissionDecision：多个 hook 中取「最严」——deny > ask > allow > 无（对齐 claude-code：allow 不覆盖 deny）
  │
  ├─ updatedInput：**仅第一个**非空 updatedInput 生效（防互相覆盖，顺序即优先级）
  │
  └─ additionalContext：全部拼接（顺序稳定），注入为新的追加事件
```

## 6. 生命周期

### 6.1 会话级时序（字符画）

```
 用户启动 neo                  Hook 子进程
    │                              │
    ▼                              │
 ┌─────────────┐    stdin JSON     │
 │ SessionStart│──────────────────▶│  ┌─ 例：加载项目环境、写审计头
 │ hook 触发   │◀──────────────────│  └─ additionalContext / initialUserMessage
 └─────────────┘  stdout JSON      │
    │                              │
    ▼                              │
 模型对话循环（多轮）               │
    │                              │
    ▼                              │
 ┌─────────────┐    stdin JSON     │
 │ PreToolUse  │──────────────────▶│  ┌─ 例：危险命令守卫
 │ (每工具调用)│◀── block/updated──│  └─ decision=block → 工具不执行
 └─────────────┘                  │
    │                              │
    ▼                              │
 ┌─────────────┐    stdin JSON     │
 │ PostToolUse │──────────────────▶│  ┌─ 例：自动格式化、审计日志
 │ (工具返回后)│◀──────────────────│  └─ additionalContext 注入
 └─────────────┘                  │
    │                              │
    ▼                              │
 ┌─────────────┐    stdin JSON     │
 │ Stop        │──────────────────▶│  ┌─ 例：通知、收尾清理
 │ (会话停止前)│◀──────────────────│  └─ decision=block → 阻止停止
 └─────────────┘                  │
    │                              │
    ▼                              │
 ┌─────────────┐    stdin JSON     │
 │ SessionEnd  │──────────────────▶│  ┌─ 例：汇总统计、上报
 └─────────────┘                  │
    │                              │
    ▼                              │
 会话结束                          │
```

### 6.2 单次工具调用精化时序（PreToolUse 三通道决策）

```
 模型发出 ToolCall
      │
      ▼
 ToolCallFinished (AgentEvent)
      │
      ▼
 ┌───────────────────────────────────────────────────────────────┐
 │ PreToolUse hooks                                              │
 │                                                               │
 │   hook A: decision=continue, updatedInput=改写参数             │
 │   hook B: decision=block, reason="guard: 禁止 rm -rf /"        │
 │                                                               │
 │   聚合结果 → block（B 短路）                                   │
 └───────────────────────────────────────────────────────────────┘
      │ block（工具不执行，错误回填模型）
      ▼
 ToolExecutionStarted ──(跳过)──▶ ToolExecutionFinished(Err BlockedByHook)
      │
      │ continue（且无 block）
      ▼
 authorize_tool_batch（PermissionMode 决策，hook 的
 permissionDecision=allow 不绕过 ask/deny）
      │
      ▼
 execute_prepared_tool ──▶ PostToolUse hooks ──▶ finalize
```

## 7. 与现有 Neo 架构的集成点

### 7.1 代码级接线点

| 集成点 | 位置 | 改动 |
|---|---|---|
| 事件来源 | `crates/neo-agent-core/src/runtime/tool_dispatch.rs` | `execute_tool_calls` 内 `authorize_tool_batch`（1039 行）前插 PreToolUse；`finalize_authorized_batch`（1072 行）前插 PostToolUse |
| 会话事件 | `crates/neo-agent-core/src/runtime/`（run 循环） | `RunStarted` 发射点接 SessionStart；`RunFinished` 接 SessionEnd/Stop |
| 用户输入 | `crates/neo-agent-core/src/messages.rs` / run 循环入队点 | `AgentMessage::user_text` 入队前接 UserPromptSubmit |
| 子代理 | `crates/neo-agent-core/src/multi_agent/` | `DelegateStarted`/`DelegateFinished` 发射点接 SubagentStart/Stop |
| 压缩 | `crates/neo-agent-core/src/runtime/compaction` | `CompactionStarted`/`CompactionApplied` 接 PreCompact/PostCompact |
| 可观测 | `crates/neo-agent-core/src/events.rs` | 新增 `HookStarted`/`HookCompleted` 变体（append-only，不影响既有前缀） |
| 配置 | `crates/neo-agent/src/config.rs` | `[hooks]` 段解析（serde + schemars） |
| 权限 | `crates/neo-agent-core/src/permissions.rs` | 决策优先级：hook block > PermissionMode deny > hook allow < PermissionMode ask |

### 7.2 与 PermissionMode 的优先级（最终裁决序）

```
 1. hook decision = block                      → 阻断（最高优先级）
 2. PermissionMode / PermissionOperation deny  → 阻断（hook allow 无法覆盖）
 3. PermissionMode = ask 且无 allow 记录       → 弹窗（hook allow 可视为预授权，但用户仍可拒绝）
 4. hook permissionDecision = deny             → 阻断
 5. 其余                                        → 放行
```

### 7.3 上下文注入的合规路径（对齐「上下文完整性」硬约束）

- `additionalContext` 不改写历史：通过新增 `AgentEvent` 变体（如 `HookContextAppended { turn, source, text }`）在**当前消息之后追加**，模型下一轮可见；JSONL append-only，可重放。
- `updatedInput` 只作用于**当前工具调用**的执行参数，不落历史。

## 8. 安全模型

1. **信任门禁（M1 必备）**：hook 仅在 `~/.neo/trust.json` 信任的 workspace 内执行（复用 AGENTS.md 信任链）；全局 hook 需用户显式 `neo hooks trust <path>`。
2. **来源分级（M1）**：`HookSource { System, User, Project }`；项目级 hook 默认 `Untrusted`，需显式信任。
3. **校验（M2）**：`HookTrustStatus { Managed, Trusted, Untrusted, Modified }`——记录 hook 命令文件 hash，变更即 `Modified`，默认停用（codex 语义）。
4. **权限只读**：`permission_mode` 只读传入；hook 无法提升权限、无法绕过 deny；`PermissionRequest` 类 hook 的 allow 仅作建议。
5. **输出治理**：stdout 严格按 `HookOutput` schema 解析（serde deny_unknown_fields）；`additionalContext` 设大小上限（对齐 `max(65536, max_tokens/8)` 预算精神，hook 单独设上限如 16KiB）。
6. **资源治理**：默认超时 30s、输出上限 64KiB、单事件 hook 数上限；超时 kill 整个进程树。
7. **无 LLM 回路**：hook 执行路径不持有 model client，代码层面杜绝递归。
8. **日志脱敏**：hook 输入中的 `tool_input` 可能含敏感参数——TUI/日志展示 hook 输入时做截断，`suppressOutput` 默认对非守卫事件开启。

## 9. 分期实施建议

| 里程碑 | 范围 | 验证方式 |
|---|---|---|
| **M1 核心协议** | `HookEvent`（6 事件：PreToolUse/PostToolUse/UserPromptSubmit/SessionStart/SessionEnd/Stop）、TOML 配置、子进程 runner、stdin/stdout JSON、exit 2、超时、决策聚合、HookStarted/Completed 事件、信任门禁 | 单测：fake hook 脚本（node/python/sh）驱动 `HookRunner`；`neo-agent-core --lib` 定点测试；FakeModelClient 端到端验证 PreToolUse block 短路 |
| **M2 事件扩展** | PostToolUseFailure/SubagentStart/Stop/PreCompact/PostCompact/Notification/PermissionRequest、matcher 正则、parallel 模式、additionalContext 注入通道、`HookSource` 分级 | 各事件一个定点测试；注入通道做「前缀不变」断言 |
| **M3 治理** | hash 校验 Modified、managed hooks 目录、`neo hooks` CLI（list/trust/reload）、hook 运行 TUI 卡片、审计 | `neo hooks list` 集成测试；修改 hook 文件后状态翻转测试 |

## 10. 待决问题与风险

| # | 问题 | 倾向 | 理由 |
|---|---|---|---|
| 1 | hook 命令是否经 shell 执行？ | 直接用 `Command`（不经 shell），命令字符串按 argv 解析 | 跨平台、免注入；代价是失去 shell 管道，文档示例可引导 `sh -c`/`cmd /c` |
| 2 | 默认 fail-open 还是 fail-closed？ | 按事件类：守卫类（PreToolUse）默认 fail-closed，通知类（Notification/SessionEnd）fail-open | 对齐 kimi fail-open + claude-code 守卫语义 |
| 3 | `additionalContext` 是否进 M1？ | 进 M1 的 SessionStart/PreToolUse（最小注入通道） | 这是 hook 最有价值的能力之一，且实现可控 |
| 4 | 是否支持项目级 hook？ | M1 仅全局；项目级随信任链成熟后开放 | Neo 无项目本地配置的架构约束 |
| 5 | 进程内插件 API（opencode v2/pi 范式） | 明确后置为独立里程碑 | 安全面大，需要沙箱与信任设计，不应阻塞子进程协议 |
| 6 | hook 与 MCP 工具的关系 | hook 只观察/干预内建工具链，MCP 工具经适配层同样暴露 PreToolUse/PostToolUse | 复用工具执行管线即可，无额外工作 |
| 7 | 风险：hook 拖慢交互 | 30s 超时 + 并行（M2）+ 异步放行（M2，claude-code async 首行协议） | 平衡守卫能力与响应延迟 |

---

*调研证据均来自 `.references/` 源码与文档，关键文件已随文标注；未标注「未发现」的能力项均为源码中无对应实现。*
