# 并行开发任务 Handoff

> **使用说明**:每个任务是一个独立的开发单元,可分配给不同的 AI Session 并发开发。
> 文件触碰矩阵已标注冲突热点,请按照「优先级 + 依赖关系」启动任务。

---

## 当前状态

- **正在进行的重构**: `feat/tool-call-transcript` (tool-call-transcript-implementation.md)
- **触碰的文件**: `crates/neo-tui/src/app.rs`、`crates/neo-tui/src/components.rs`、`crates/neo-agent/src/modes/interactive.rs`、`crates/neo-tui/tests/primitives.rs`
- **建议**: 等该重构 merge 后再启动 Task A/B/C,Task D 可立即启动

---

## 文件触碰矩阵

图例: ✏️ 改 / ➕ 新建 / ⚠️ 冲突热点 / 🔴 被别的 AI 占用 / 👀 只读参考

| 文件 | Task A (Fork 命令) | Task B (Todo Tool) | Task C (Plan Mode) | Task D (Glob/Grep Tool) |
|---|:---:|:---:|:---:|:---:|
| `agent-core/src/session/jsonl.rs` | 👀 (fork 已存在) | | | |
| `agent-core/src/tools/mod.rs` (with_builtin_tools) | | ⚠️ ✏️ | | ⚠️ ✏️ (若新建 GlobTool) |
| `agent-core/src/tools/mod.rs` (mod 声明) | | ➕ | | ➕ (glob.rs) |
| `agent-core/src/tools/todo.rs` | | ➕ 新建 | | |
| `agent-core/src/tools/glob.rs` | | | | ➕ 新建 (若需要真 glob) |
| `agent-core/src/permissions.rs` | | | ⚠️ ✏️ | |
| `agent-core/src/runtime.rs` | | | 👀 (execute_tool_calls) | |
| `neo-agent/src/modes/interactive.rs` 🔴 | ⚠️ ✏️ (command_specs + run_selected_command + fork picker) | | ⚠️ ✏️ (command_specs + run_selected_command + mode 状态) | |
| `neo-agent/src/modes/run.rs` (tool_registry_for_config) | | 👀 | ⚠️ ✏️ (按 mode 过滤) | |
| `neo-agent/src/session_commands.rs` | 👀 (fork 已存在) | | | |
| `tui/src/app.rs` 🔴 | ✏️ (fork 成功 banner) | ⚠️ ✏️ (可能加 TodoTranscript 变体 + StreamUpdate) | ⚠️ ✏️ (NeoTuiApp 加 mode 字段 + footer 指示) | ✏️ (tool 结果展示) |
| `tui/src/components.rs` 🔴 | | ⚠️ ✏️ (todo 渲染) | ✏️ (mode 指示渲染) | ✏️ (glob 结果渲染) |
| `tui/tests/primitives.rs` 🔴 | ⚠️ (snapshot) | ⚠️ (snapshot) | ⚠️ (snapshot) | ⚠️ (snapshot) |
| `neo-agent/src/prompt_templates.rs` | 👀 | | | |

---

## Task A: Fork Slash 命令

**优先级**: 高(等 tool-call-transcript merge 后)
**工作量**: 小-中(数据层已就绪,只需加 UI 入口)

### 现状
- ✅ 数据层完整: `SessionRecord.parent_id`、`children`、`SessionMetadataStore::fork` (jsonl.rs:446-476)
- ✅ CLI 命令存在: `session_commands::fork` (session_commands.rs:17-76)
- ✅ Interactive fork 已有: `fork_session_transcript` (interactive.rs:2584-2595)
- ❌ 缺少 `/fork` slash 命令入口

### 需要做的

#### 1. 加 slash 命令注册
**文件**: `crates/neo-agent/src/modes/interactive.rs`

在 `command_specs` (约 1921-1967 行) 添加:
```rust
CommandSpec::new("fork", "Fork the current session")
    .with_key_hint("f")
```

在 `run_selected_command` (约 959-984 行) 的 `match` 添加:
```rust
"fork" => {
    // 调用 fork_session_transcript 或显示 fork picker
}
```

#### 2. (可选) 加交互式 fork picker
显示可用 session 列表,让用户选择 fork 源。

#### 3. Fork 成功后显示 banner
**文件**: `crates/neo-tui/src/app.rs`

在 fork 成功后 push `TranscriptItem::Banner` 提示用户新 session ID。

### 参考
- kimi-code 实现: `apps/kimi-code/src/tui/commands/session.ts` (handleForkCommand)
- neo 现有 fork 逻辑: `crates/neo-agent-core/src/session/jsonl.rs:446-476`

---

## Task B: Todo Tool

**优先级**: 中(等 tool-call-transcript merge 后,最好等 Task A 完成)
**工作量**: 中(新建 tool + TUI 渲染 + 事件溯源)

### 现状
- ❌ 完全没有 todo tool
- ✅ Tool 框架就绪: `trait Tool`、`ToolRegistry`
- ✅ 事件记录系统就绪(可通过 agent record 持久化)

### 需要做的

#### 1. 新建 todo tool
**文件**: `crates/neo-agent-core/src/tools/todo.rs` (新建)

参考 kimi-code 的 `TodoListTool`:
```rust
pub struct TodoTool {
    store: ToolStore, // 或内部状态
}

impl Tool for TodoTool {
    fn name(&self) -> &str { "todo" }

    fn input_schema(&self) -> serde_json::Value {
        // { todos?: [{title, status: "pending"|"in_progress"|"done"}] }
        // 省略 todos = 查询, 空数组 = 清空
    }

    fn execute(&self, input: Value) -> ToolResult {
        // 更新内部状态,产出 tools.update_store record
    }
}
```

**注册**: 在 `tools/mod.rs` 的 `with_builtin_tools` 添加:
```rust
register(TodoTool::new());
```

#### 2. TUI 渲染
**文件**: `crates/neo-tui/src/components.rs` (新建 `todo_panel` 组件)

参考 kimi-code 的 `TodoPanelComponent`:
- 状态符号: `in_progress`=`●`, `done`=`✓`, `pending`=`○`
- 超过 5 个时智能裁剪(优先显示 in_progress + 最近 done)
- 全 done 时自动清空

**接入**: 在 `TranscriptItem` (app.rs:2194-2233) 添加 `Todo` 变体?或用持久面板(Overlay 体系)?

#### 3. 事件溯源(持久化)
参考 kimi-code:每次 `store.set` 产出 `tools.update_store` record,追加到 `wire.jsonl`。resume 时重放记录重建 todo 状态。

### 参考
- kimi-code 实现: `packages/agent-core/src/tools/builtin/state/todo-list.ts`
- kimi-code TUI: `apps/kimi-code/src/tui/components/chrome/todo-panel.ts`

---

## Task C: Plan Mode

**优先级**: 低(最复杂,建议最后做)
**工作量**: 大(新建 mode 状态机 + 权限策略 + AskUser 工具 + TUI 提问卡片)

### 现状
- ❌ 完全没有 mode 系统
- ✅ 权限系统就绪: `PermissionPolicy` (permissions.rs)

### 需要做的

#### 1. 新建 PlanMode 状态机
**文件**: `crates/neo-agent-core/src/mode/plan.rs` (新建模块)

参考 kimi-code 的 `PlanMode` 类:
```rust
pub struct PlanMode {
    is_active: bool,
    plan_file_path: Option<PathBuf>,
}

impl PlanMode {
    pub fn enter(&mut self, homedir: &Path) -> Result<PathBuf> {
        // 生成 plan 文件路径 <homedir>/plans/<id>.md
        // 记录 plan_mode.enter agent record
    }

    pub fn exit(&mut self) {
        // 清理状态,记录 plan_mode.exit record
    }

    pub fn data(&self) -> Option<String> {
        // 读取 plan 文件内容
    }
}
```

#### 2. 加权限策略
**文件**: `crates/neo-agent-core/src/permissions/policies/plan_mode_guard.rs` (新建)

参考 kimi-code 的 `PlanModeGuardDenyPermissionPolicy`:
```rust
pub struct PlanModeGuardPolicy;

impl PermissionPolicy for PlanModeGuardPolicy {
    fn check_tool_execution(&self, tool: &str, args: &Value, mode: &PlanMode) -> PermissionDecision {
        if mode.is_active {
            match tool {
                "write" | "edit" => {
                    // 除非写入路径 == plan_file_path,否则 deny
                    if is_plan_file_write(args, mode.plan_file_path) {
                        PermissionDecision::Approved
                    } else {
                        PermissionDecision::Denied("read-only mode")
                    }
                }
                "task_stop" | "cron_create" | "cron_delete" => PermissionDecision::Denied,
                _ => PermissionDecision::Approved,
            }
        } else {
            PermissionDecision::Approved
        }
    }
}
```

#### 3. 新建 AskUser 工具
**文件**: `crates/neo-agent-core/src/tools/ask_user.rs` (新建)

参考 kimi-code 的 `AskUserQuestionTool`:
```rust
pub struct AskUserQuestionTool {
    rpc: RpcClient, // reverse-RPC 客户端
}

impl Tool for AskUserQuestionTool {
    fn execute(&self, input: Value) -> ToolResult {
        // 构造问题请求
        // 调 rpc.request_question (反向调用 host)
        // 等待用户答案 (async)
        // 返回答案
    }
}
```

**注意**:需要实现 reverse-RPC 双向通道(LLM → Host → TUI → 用户 → Host → LLM)。

#### 4. TUI 提问卡片
**文件**: `crates/neo-tui/src/components/question_dialog.rs` (新建)

参考 kimi-code 的 `QuestionDialogComponent`:
- 渲染选项卡片(数字键 1-9 选择)
- 支持 `other` 自定义输入
- multi_select 模式

#### 5. Plan Mode 入口
**文件**: `crates/neo-agent/src/modes/interactive.rs`

在 `command_specs` 添加:
```rust
CommandSpec::new("plan", "Toggle plan mode")
```

在 `NeoTuiApp` (app.rs:307-335) 添加 `agent_mode` 字段:
```rust
pub struct NeoTuiApp {
    // ...
    pub agent_mode: AgentMode, // new
}

pub enum AgentMode {
    Default,
    Plan,
}
```

在 footer (components.rs) 显示 mode 指示器。

#### 6. EnterPlanMode / ExitPlanMode 工具
**文件**: `crates/neo-agent-core/src/tools/plan_mode.rs` (新建)

- `EnterPlanMode`: 无参数,进入 plan mode
- `ExitPlanMode`: 可带 `options: [{label, description}]` 多选项,触发审批

### 参考
- kimi-code PlanMode: `packages/agent-core/src/agent/plan/index.ts`
- kimi-code AskUser: `packages/agent-core/src/tools/builtin/collaboration/ask-user.ts`
- kimi-code TUI: `apps/kimi-code/src/tui/components/dialogs/question-dialog.ts`

---

## Task D: Glob/Grep Tool

**优先级**: 高(可立即启动,冲突最小)
**工作量**: 小(grep 已存在,只需确认/微调;glob 需新建)

### 现状
- ✅ `GrepTool` 已存在 (`tools/grep.rs`),基于 `ignore` crate(ripgrep 同源)
- ✅ `FindTool` 已存在 (`tools/find.rs`),文件名子串匹配
- ❌ 没有真正的 glob tool

### 需要做的

#### 1. 确认 GrepTool 行为(可选)
检查 `grep.rs` 是否符合预期:
- 是否支持 `--hidden` / `--no-ignore`
- 敏感文件过滤是否完善
- 输出格式是否合理

如需调整,直接修改 `grep.rs`。

#### 2. (可选) 新建 GlobTool
**文件**: `crates/neo-agent-core/src/tools/glob.rs` (新建)

参考 kimi-code 的 `GlobTool`:
```rust
pub struct GlobTool {
    fs: FileSystem,
}

impl Tool for GlobTool {
    fn name(&self) -> &str { "glob" }

    fn input_schema(&self) -> serde_json::Value {
        // { pattern: string, max_matches?: number }
    }

    fn execute(&self, input: Value) -> ToolResult {
        // 1. brace 展开 (*.{ts,tsx} → 子模式列表)
        // 2. 对每个模式调 glob walker
        // 3. 按 mtime 降序排序,上限 100
    }
}
```

**底层库**:推荐 `globset` + `walkdir` 或 `ignore` crate(ripgrep 的 walker)。

**注册**: 在 `tools/mod.rs` 的 `with_builtin_tools` 添加:
```rust
register(GlobTool::new());
```

### 参考
- kimi-code Glob: `packages/agent-core/src/tools/builtin/file/glob.ts`
- kimi-code Grep: `packages/agent-core/src/tools/builtin/file/grep.ts`
- neo 现有 Grep: `crates/neo-agent-core/src/tools/grep.rs`

---

## 启动顺序建议

### 立即启动
- **Task D (Glob/Grep Tool)**: 零冲突,隔离性最好

### 等待 tool-call-transcript merge 后
1. **Task A (Fork 命令)**: 工作量小,快速见效
2. **Task B (Todo Tool)**: 工作量中,需新建 tool + TUI
3. **Task C (Plan Mode)**: 工作量大,最后做

### 如果不等,风险控制
- A 和 C **必须串行**(都改 `interactive.rs` 的 command palette)
- B 和 D 可并行(各自独立 tool 文件)
- 所有任务都需注意 `app.rs`/`components.rs` 的变体冲突

---

## kimi-code 参考点总结

| 特性 | 核心文件 | 关键机制 |
|---|---|---|
| Fork | `apps/kimi-code/src/tui/commands/session.ts` | 物理复制整个 session 目录(cp 递归) |
| Todo | `packages/agent-core/src/tools/builtin/state/todo-list.ts` | 单工具读写合一,事件溯源持久化 |
| Plan Mode | `packages/agent-core/src/agent/plan/index.ts` | 状态机 + 权限策略 + reverse-RPC 提问 |
| Grep | `packages/agent-core/src/tools/builtin/file/grep.ts` | spawn ripgrep 15.0.0 二进制 |
| Glob | `packages/agent-core/src/tools/builtin/file/glob.ts` | 自研 async generator walker + brace 展开 |

---

## 附录:关键文件快速索引

### neo 侧
- Session: `crates/neo-agent-core/src/session/jsonl.rs`
- Tools: `crates/neo-agent-core/src/tools/mod.rs`
- Slash 命令: `crates/neo-agent/src/prompt_templates.rs`
- Command palette: `crates/neo-agent/src/modes/interactive.rs:1921-1967` (command_specs), `:959-984` (run_selected_command)
- TUI transcript: `crates/neo-tui/src/app.rs:2194-2233` (TranscriptItem), `:2959+` (ChatTranscript)
- TUI 渲染: `crates/neo-tui/src/components.rs:264-273` (TranscriptWidget), `:598-650` (tool_render_rows)

### kimi-code 侧
- Fork: `apps/kimi-code/src/tui/commands/session.ts`, `packages/agent-core/src/session/store/session-store.ts`
- Todo: `packages/agent-core/src/tools/builtin/state/todo-list.ts`, `apps/kimi-code/src/tui/components/chrome/todo-panel.ts`
- Plan Mode: `packages/agent-core/src/agent/plan/index.ts`, `packages/agent-core/src/tools/builtin/planning/enter-plan-mode.ts`
- AskUser: `packages/agent-core/src/tools/builtin/collaboration/ask-user.ts`, `apps/kimi-code/src/tui/components/dialogs/question-dialog.ts`
- Grep: `packages/agent-core/src/tools/builtin/file/grep.ts`
- Glob: `packages/agent-core/src/tools/builtin/file/glob.ts`
