# Permission Mode And Plan Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Neo's low-level permission system with first-class `manual` / `auto` / `yolo` permission modes, add `/ask`, `/auto`, `/yolo`, `/permissions`, improve `/plan`, and bind Shift+Enter to cycle `plan -> manual -> auto -> yolo`.

**Architecture:** Do a bottom-up replacement, not a display rename. Delete the old configurable `PermissionPolicy { file_read, file_write, shell, tool }` model and make `PermissionMode` the runtime/config/TUI source of truth. Keep hard safety policies such as plan-mode write guards as explicit runtime rules that run before mode-based auto approvals.

**Tech Stack:** Rust 2024, Cargo workspace, `serde`/`schemars` config types, `tokio`, Neo TUI overlay/picker primitives, existing `AgentRuntime`, `ToolContext`, and transcript tests.

---

## Non-Negotiable Constraints

- Do not keep two permission systems. Remove old user-facing `[permissions]` / `PermissionPolicy` config semantics instead of mapping them forever into modes.
- Do not add compatibility shims that allow both old per-operation policy config and new modes.
- Do not run git mutations unless the user explicitly authorizes that exact mutation in the current conversation. This plan intentionally contains no `git add`, `git commit`, `git reset`, `git checkout`, `git stash`, `git clean`, `git rebase`, `git push`, or worktree steps.
- Subagents must never execute git mutations.
- Use `rtk` for shell commands in this repo.
- Use `cx` for symbol lookup when possible; fall back to `rtk rg` / `rtk sed`.
- Preserve user preference: remove obsolete compatibility code when replacing a feature.

## Target Product Semantics

### Permission Modes

Neo must expose exactly these user-facing permission modes:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Manual,
    Auto,
    Yolo,
}
```

- `manual`: ask before commands, edits, and other risky actions. Read/search tools run directly. Session approval rules are respected.
- `auto`: run fully non-interactively. Tool actions are approved automatically after hard-deny policies run. Agent questions are skipped/denied so the model must decide on its own.
- `yolo`: skip normal confirmations. Tool actions are approved automatically after hard-deny policies run. Explicit user questions are still allowed.

Kimi reference paths:

- `docs/kimi-code/packages/agent-core/src/agent/permission/types.ts`
- `docs/kimi-code/packages/agent-core/src/agent/permission/policies/index.ts`
- `docs/kimi-code/packages/agent-core/src/agent/permission/policies/auto-mode-ask-user-question-deny.ts`
- `docs/kimi-code/packages/agent-core/src/agent/permission/policies/exit-plan-mode-review-ask.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/components/dialogs/permission-selector.ts`
- `docs/kimi-code/packages/agent-core/src/loop/tool-call.ts`
- `docs/kimi-code/packages/agent-core/src/loop/tool-scheduler.ts`
- `docs/kimi-code/packages/agent-core/src/loop/tool-access.ts`
- `docs/kimi-code/packages/agent-core/src/tools/builtin/collaboration/ask-user.ts`

### Policy Precedence

Runtime policy must be evaluated in this order:

1. Pre-tool hard deny hooks, if any.
2. Auto mode `AskUserQuestion` deny.
3. Plan-mode hard guard deny.
4. Auto mode approve.
5. Session approvals from "Approve for this session".
6. Exit plan mode review ask for non-auto mode when there is a non-empty plan.
7. Plan-mode helper approvals: `EnterPlanMode`, writing the active plan file, `ExitPlanMode` with no reviewable plan.
8. Sensitive/default safety asks if Neo has existing local equivalents.
9. Yolo mode approve.
10. Default read/search/status tool approve.
11. Manual fallback ask.

Neo does not need to copy Kimi's entire permission-rule DSL in this task. The scope is mode replacement and the existing approval UX.

### Runtime Tool Scheduling

Kimi does **not** execute every tool call in a response with a blind
`Promise.all`. Its `runToolCallBatch` prepares and authorizes tool calls in
provider/source order, then submits executable tasks to `ToolScheduler`.
`ToolScheduler` starts tasks only when their declared `ToolAccesses` do not
conflict with currently active tasks or queued earlier tasks. Terminal
`tool.result` records are still finalized and emitted in provider order.

Neo should adopt the conservative part of this design:

- Permission and plan policies run before a tool is scheduled.
- The policy result must tell the scheduler whether the call can open a
  blocking user dialog.
- If a batch contains any blocking-dialog call, later calls in that model batch
  must not start until the dialog-producing call resolves.
- Non-blocking tools may still run in parallel when their runtime access class
  permits it.
- Results must remain appended to model context in source order, even if
  non-blocking executions finish out of order.

Blocking-dialog calls are mode-dependent:

- `manual`: `Write`, `Edit`, foreground `Bash`, foreground `Terminal`, unknown
  external tools, non-session-approved asks, and non-background
  `AskUserQuestion` can block.
- `auto`: tool approvals are auto-approved, but `AskUserQuestion` is denied
  before scheduling so it must not block the runtime.
- `yolo`: normal tool approvals are auto-approved, but non-background
  `AskUserQuestion` remains allowed and blocking.
- `ExitPlanMode` with non-empty reviewable plan is blocking in `manual` and
  `yolo`, and non-blocking in `auto`.

Kimi's `AskUserQuestion` **does** have `background?: boolean`. In Kimi, setting
`background=true` registers a `QuestionBackgroundTask`, returns a `task_id`
immediately, and says the answer will arrive automatically in a later turn. That
is only safe when the model can continue without the answer. Neo may keep this
semantics only if the TUI/runtime actually keeps the pending question visible,
lets the user answer it later, and injects the answer back into a later turn. If
that full background-question path is not implemented or not visible, then
`background=true` must be rejected with a clear tool error instead of becoming a
hidden indefinite wait.

### Plan Mode

- `/plan` toggles plan mode: first call turns it on, second call turns it off.
- `/plan on` forces plan mode on; `/plan off` forces plan mode off. The primary empty `/plan` behavior is toggle.
- `/plan clear` remains supported and clears the current plan file, matching existing Neo behavior and Kimi's command surface.
- Transcript status output must be exactly `Plan Mode On` and `Plan Mode Off`.
- Footer must show plan state without bloated prose.
- Plan mode hard guard must remain stronger than `auto` or `yolo`.
- In plan mode, `Write` / `Edit` may only write the current plan file. Writes elsewhere are hard denied.
- In plan mode, `TaskStop` is hard denied. If `CronCreate` or `CronDelete` tools exist in the implementation branch, they are hard denied too.
- `Bash` follows the active permission mode; plan mode must not add an extra Bash approval layer.
- `EnterPlanMode` is auto-approved in all permission modes.
- `ExitPlanMode` behavior:
  - `auto`: exits without approval.
  - `manual`: if plan content is non-empty, show plan review approval.
  - `yolo`: follow Kimi core policy, not confusing UI copy: do not bypass non-empty `ExitPlanMode` plan review.
  - rejected/revise keeps plan mode active and returns feedback to the model.
  - approved exits plan mode.

### Slash Commands

- `/ask`: switch current session permission mode to `manual`.
- `/auto`: switch current session permission mode to `auto`.
- `/yolo`: switch current session permission mode to `yolo`.
- `/permissions`: open a permission mode selector overlay.
- `/plan`: toggle plan mode and emit transcript status.

`/permissions` selector must look like Neo's existing picker style and use the Kimi copy:

```text
Select permission mode
↑↓ navigate · Enter select · Esc cancel

Manual
Ask before commands, edits, and other risky actions. Read/search tools run directly; session approval rules are respected.
Auto
Run fully non-interactively. Tool actions are approved automatically, and agent questions are skipped so it can decide on its own.
YOLO
Automatically approve tool actions and plan transitions. The agent can still ask you explicit questions when your input is needed.
```

Show `← current` on the current mode row.

### Hotkey

- Bind Shift+Enter to cycle modes in this exact order:
  - if not in plan mode: enter plan mode.
  - if in plan mode: turn plan mode off and set permission mode to `manual`.
  - if `manual`: set `auto`.
  - if `auto`: set `yolo`.
  - if `yolo`: enter plan mode.
- Because Shift+Enter currently means newline in `neo-tui/src/input.rs`, move newline behavior to Alt+Enter and Ctrl+J.
- Do not break bracketed paste newline handling.

## File Responsibility Map

### Core Runtime

- `crates/neo-agent-core/src/permissions.rs`
  - Replace `PermissionDecision` / `PermissionPolicy` with `PermissionMode`, `PermissionOperation`, `PermissionApprovalDecision`, and tool access flags.
- `crates/neo-agent-core/src/tools/mod.rs`
  - Replace `ToolContext.permissions: PermissionPolicy` with runtime-granted access flags.
- `crates/neo-agent-core/src/runtime.rs`
  - Replace `AgentConfig.tool_permission_policy` with `permission_mode`.
  - Rewrite tool preparation policy order.
  - Add runtime scheduling metadata so mode-dependent blocking dialogs serialize
    their model tool-call batch before later tools start.
  - Preserve approval event emission and session approval behavior.
- `crates/neo-agent-core/src/mode/plan_mode_guard.rs`
  - Stop returning old `PermissionDecision`; return a dedicated hard-guard result.
- `crates/neo-agent-core/src/tools/ask_user.rs`
  - Ensure auto mode denies `AskUserQuestion` before tool execution.
  - Keep `background=true` only if the background-question task path is fully
    visible and answerable; otherwise reject it with a clear tool error.
- `crates/neo-agent-core/src/tools/plan_mode.rs`
  - Ensure tool result semantics support plan review approve/reject/revise.

### CLI And Config

- `crates/neo-agent/src/cli.rs`
  - Keep `--yolo`, add required `--auto`, and make both select `PermissionMode`.
- `crates/neo-agent/src/config.rs`
  - Replace `permissions: PermissionPolicy` with `permission_mode: PermissionMode`.
  - Replace `FileConfig.permissions` with canonical `permission_mode`.
  - Do not deserialize old `[permissions]` into the new model.
- `crates/neo-agent/src/modes/run.rs`
  - Pass `permission_mode` into `AgentConfig`.
  - In non-interactive run, `--yolo` must actually skip confirmations, not only trust project context.
- `docs/config.md`, `docs/tools.md`, `docs/quickstart.md`, `examples/config/*.toml`
  - Remove old per-operation permission config examples.

### TUI

- `crates/neo-tui/src/chrome.rs`
  - Store `PermissionMode`, not `PermissionDecision`.
  - Render mode badge `[manual]`, `[auto]`, `[yolo]`.
  - Add or reuse permission selector overlay state.
- `crates/neo-tui/src/transcript/pane.rs`
  - Render footer badge from `PermissionMode`.
- `crates/neo-tui/src/dialogs/choice_picker.rs`
  - Reuse existing `ChoicePickerOptions` / `ChoiceItem`; add current-row marker if missing.
- `crates/neo-tui/src/input.rs`
  - Add `KeybindingAction::CyclePermissionMode`.
  - Bind Shift+Enter to cycle permission/plan modes.
  - Keep Alt+Enter and Ctrl+J as newline.
- `crates/neo-agent/src/modes/interactive.rs`
  - Add session-level `permission_mode`.
  - Implement `/ask`, `/auto`, `/yolo`, `/permissions`, improved `/plan`.
  - Ensure current mode is copied into every next `TurnRequest`.
  - Add transcript statuses for mode changes and plan mode changes.
  - Handle permission selector result.

## Task 1: Replace Core Permission Types

**Files:**
- Modify: `crates/neo-agent-core/src/permissions.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`
- Search/Update references in: `crates/neo-agent-core/src/**/*.rs`, `crates/neo-agent-core/tests/**/*.rs`, `crates/neo-agent/src/**/*.rs`, `crates/neo-tui/src/**/*.rs`

- [ ] **Step 1: Write failing type migration checks**

Run:

```bash
rtk rg -n "PermissionPolicy|PermissionDecision::Allow|PermissionDecision::Ask|PermissionDecision::Deny|file_read|file_write|shell: PermissionDecision|tool: PermissionDecision" crates/neo-agent-core crates/neo-agent crates/neo-tui
```

Expected before implementation: many matches.

- [ ] **Step 2: Replace `permissions.rs` contents**

Use this target shape:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Manual,
    Auto,
    Yolo,
}

impl PermissionMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
            Self::Yolo => "yolo",
        }
    }

    #[must_use]
    pub const fn next_after_plan(self) -> Self {
        match self {
            Self::Manual => Self::Auto,
            Self::Auto => Self::Yolo,
            Self::Yolo => Self::Manual,
        }
    }
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Manual
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionApprovalDecision {
    AllowOnce,
    AllowForSession,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PermissionOperation {
    FileRead,
    FileWrite,
    Shell,
    Tool,
    UserQuestion,
    PlanTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolAccess {
    pub file_read: bool,
    pub file_write: bool,
    pub shell: bool,
    pub tool: bool,
    pub user_question: bool,
}

impl ToolAccess {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            file_read: false,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        }
    }

    #[must_use]
    pub const fn all() -> Self {
        Self {
            file_read: true,
            file_write: true,
            shell: true,
            tool: true,
            user_question: true,
        }
    }
}
```

- [ ] **Step 3: Remove old names from core compile surface**

Update imports and references until this command has no production matches except docs/plans explaining deletion:

```bash
rtk rg -n "PermissionPolicy|PermissionDecision" crates/neo-agent-core/src crates/neo-agent/src crates/neo-tui/src
```

Expected after implementation: no matches.

- [ ] **Step 4: Run focused compile**

Run:

```bash
rtk cargo check -p neo-agent-core
```

Expected: compile errors identify the remaining migration sites. Do not proceed past Task 3 until `neo-agent-core` compiles.

## Task 2: Migrate `ToolContext` To Granted Access Flags

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Modify: `crates/neo-agent-core/src/tools/*.rs`
- Modify tests that construct `ToolContext`

- [ ] **Step 1: Change `ToolContext`**

Replace:

```rust
pub permissions: PermissionPolicy,
```

with:

```rust
pub access: ToolAccess,
```

`ToolContext::new` must initialize with `ToolAccess::none()`. Add:

```rust
#[must_use]
pub const fn with_access(mut self, access: ToolAccess) -> Self {
    self.access = access;
    self
}
```

Delete `with_permission_policy`.

- [ ] **Step 2: Update ensure helpers**

Every helper that currently checks `context.permissions.can_*()` must check booleans:

```rust
if !context.access.file_read {
    return Err(ToolError::PermissionDenied { operation: "file read" });
}
```

Use exact operation strings currently asserted by tests.

- [ ] **Step 3: Update direct tool tests**

Tests that previously used `PermissionPolicy::allow_all()` must use:

```rust
ToolContext::new(temp.path())
    .expect("context")
    .with_access(ToolAccess::all())
```

Tests that expected read-only must construct:

```rust
ToolAccess {
    file_read: true,
    file_write: false,
    shell: false,
    tool: true,
    user_question: false,
}
```

- [ ] **Step 4: Verify tool crates**

Run:

```bash
rtk cargo test -p neo-agent-core tool_files -- --nocapture
rtk cargo test -p neo-agent-core tool_bash -- --nocapture
rtk cargo test -p neo-agent-core tool_terminal -- --nocapture
rtk cargo test -p neo-agent-core tool_permissions -- --nocapture
```

Expected: pass after all ToolContext callers are migrated.

## Task 3: Replace Runtime Permission Gate With Mode Policies

**Files:**
- Modify: `crates/neo-agent-core/src/runtime.rs`
- Modify: `crates/neo-agent-core/src/mode/plan_mode_guard.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Change `AgentConfig`**

Replace:

```rust
pub tool_permission_policy: PermissionPolicy,
```

with:

```rust
pub permission_mode: PermissionMode,
```

Add builder:

```rust
#[must_use]
pub const fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
    self.permission_mode = mode;
    self
}
```

Delete `with_tool_permission_policy`.

- [ ] **Step 2: Rewrite plan guard return type**

In `plan_mode_guard.rs`, replace `PermissionDecision` return with:

```rust
pub enum PlanModeGuard {
    Allow,
    Deny { message: String },
}
```

`check_plan_mode_guard` must return `Deny` for writes outside the active plan file and for hard-blocked tools. It must not use permission mode.

- [ ] **Step 3: Implement `permission_preparation_for_mode`**

In `runtime.rs`, split the old `prepare_tool_context` into small helpers:

```rust
enum PermissionPreparation {
    Run(ToolAccess),
    Ask {
        operation: PermissionOperation,
        subject: String,
    },
    Deny(String),
}
```

Decision rules:

```rust
match config.permission_mode {
    PermissionMode::Auto => {
        if tool_call.name == "AskUserQuestion" {
            Deny("AskUserQuestion is disabled while auto permission mode is active".to_owned())
        } else {
            Run(access_for_tool(tool_call, true))
        }
    }
    PermissionMode::Yolo => Run(access_for_tool(tool_call, true)),
    PermissionMode::Manual => {
        if is_default_approved_tool(tool_call) {
            Run(access_for_tool(tool_call, true))
        } else {
            Ask { operation, subject }
        }
    }
}
```

`is_default_approved_tool` must include read/search/list tools and status/helper tools that are already safe today. It must not include `Write`, `Edit`, `Bash`, `Terminal`, or unknown external tools.

- [ ] **Step 4: Preserve approval event behavior**

Keep `ApprovalRequested` emission for manual asks and yolo plan review asks. Approval response should return a dedicated decision, not old `PermissionDecision`.

Minimum target:

```rust
pub type ApprovalHandler =
    Arc<dyn Fn(&ApprovalRequest) -> PermissionApprovalDecision + Send + Sync>;
```

If changing every approval handler in one edit is too large, introduce a temporary internal `ApprovalDecision` enum with the same non-old names and remove it before Task 10. Do not keep old `PermissionDecision` names.

- [ ] **Step 5: Implement session approval**

When TUI sends "Approve for this session", later matching tool calls in the same session should skip asking. If existing code only stores allow by tool id/name, retain current behavior but rename it away from `PermissionDecision`.

- [ ] **Step 6: Implement `ExitPlanMode` mode semantics**

In `prepare_tool_context`:

```rust
if tool_call.name == "ExitPlanMode" {
    match config.permission_mode {
        PermissionMode::Auto => return ToolPreparation::Run(context.with_access(ToolAccess::all())),
        PermissionMode::Manual | PermissionMode::Yolo => {
            // ask only when active plan content is non-empty
        }
    }
}
```

For reject/revise, return an ok tool result that tells the model plan mode remains active and includes feedback.

- [ ] **Step 7: Runtime tests**

Add or update tests in `crates/neo-agent-core/tests/runtime_turn.rs`:

```rust
#[tokio::test]
async fn manual_mode_asks_for_bash() { /* Bash produces ApprovalRequested */ }

#[tokio::test]
async fn manual_mode_allows_read_without_approval() { /* Read runs directly */ }

#[tokio::test]
async fn auto_mode_approves_bash_without_approval() { /* no ApprovalRequested */ }

#[tokio::test]
async fn auto_mode_denies_ask_user_question() { /* tool result contains disabled message */ }

#[tokio::test]
async fn yolo_mode_approves_write_without_approval() { /* no ApprovalRequested */ }

#[tokio::test]
async fn yolo_mode_still_allows_ask_user_question() { /* question request reaches host */ }

#[tokio::test]
async fn plan_guard_blocks_write_outside_plan_even_in_yolo() { /* hard deny */ }

#[tokio::test]
async fn auto_exit_plan_mode_does_not_request_review() { /* exits directly */ }

#[tokio::test]
async fn yolo_exit_plan_mode_with_non_empty_plan_requests_review() { /* ApprovalRequested */ }
```

- [ ] **Step 8: Verify runtime**

Run:

```bash
rtk cargo test -p neo-agent-core runtime_turn -- --nocapture
```

Expected: pass.

## Task 3.5: Runtime Tool Scheduling Refactor

**Files:**
- Modify: `crates/neo-agent-core/src/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/ask_user.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`
- Modify: `docs/tools.md` if runtime scheduling docs become stale

**Why this task exists:** Permission modes change which tools can open
Approval dialogs. A tool batch that is safe to run in parallel in `auto` can be
blocking in `manual`. Do not keep the current scheduling as a separate ad-hoc
check bolted onto old `PermissionDecision`; make scheduling consume the new
mode-policy result from Task 3.

- [ ] **Step 1: Write the failing Ask User scheduling test**

Add this test to `crates/neo-agent-core/tests/runtime_turn.rs`. Adjust helper
names to match the migrated types from Task 3, but keep the behavior exactly:

```rust
#[tokio::test]
async fn parallel_mode_serializes_non_background_ask_user_question() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart { id: "msg_1".to_owned() },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "questions": [{
                        "question": "Continue?",
                        "header": "Flow",
                        "options": [
                            { "label": "Yes", "description": "Continue now" },
                            { "label": "No", "description": "Stop now" }
                        ],
                        "multi_select": false
                    }]
                }),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "must wait" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(neo_agent_core::AskUserTool::new(question_tx));
    tools.register(RecordingEchoTool { executed: Arc::clone(&executed) });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Parallel),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("ask then echo"));
    let pending = timeout(Duration::from_millis(250), question_rx.recv())
        .await
        .expect("question should be requested")
        .expect("question should be pending");

    assert!(
        executed.lock().expect("executed lock poisoned").is_empty(),
        "later tools must not start while a blocking user question waits"
    );

    pending
        .response_tx
        .send(neo_agent_core::QuestionResponse {
            answers: vec!["Yes".to_owned()],
        })
        .expect("send question response");
    while let Some(event) = stream.next().await {
        event.expect("event should be ok");
    }

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["must wait".to_owned()]
    );
}
```

Run:

```bash
rtk cargo test -p neo-agent-core --test runtime_turn parallel_mode_serializes_non_background_ask_user_question -- --nocapture
```

Expected before implementation: fail because the second tool can start while
the question is waiting.

- [ ] **Step 2: Write the failing manual Approval scheduling test**

Convert the existing approval-pending parallel test, or add this behavior if
the old test is gone:

```rust
#[tokio::test]
async fn parallel_mode_serializes_manual_approval_batches() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = parallel_write_and_echo_harness();
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::with_builtin_tools();
    tools.register(RecordingEchoTool { executed: Arc::clone(&executed) });
    let (decision_sender, decision_receiver) = oneshot::channel();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Manual)
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_async_approval_handler({
                let decision_receiver = Arc::clone(&decision_receiver);
                move |_request| {
                    let decision_receiver = take_decision_receiver(&decision_receiver);
                    async move {
                        decision_receiver
                            .await
                            .expect("approval decision should be sent")
                    }
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tools"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(
        timeout(Duration::from_millis(250), stream.next()).await.is_err(),
        "later tools in a manual approval batch must wait for the active approval"
    );
    assert!(executed.lock().expect("executed lock poisoned").is_empty());

    decision_sender
        .send(PermissionApprovalDecision::AllowOnce)
        .expect("send allow decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["already allowed".to_owned()]
    );
}
```

Run:

```bash
rtk cargo test -p neo-agent-core --test runtime_turn parallel_mode_serializes_manual_approval_batches -- --nocapture
```

Expected before implementation: fail if the allowed `echo` tool starts while
the write approval is pending.

- [ ] **Step 3: Add scheduling metadata to permission preparation**

In `runtime.rs`, extend the Task 3 preparation result so every runnable or
synthetic tool carries scheduling metadata:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolSchedulingClass {
    ParallelSafe,
    Exclusive,
    BlockingDialog,
}

struct PreparedToolCall {
    tool_call: AgentToolCall,
    result: PreparedToolCallResult,
    scheduling: ToolSchedulingClass,
    access: ToolAccess,
}
```

Use simple conservative classification:

```rust
fn scheduling_class_for_preparation(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    preparation: &PermissionPreparation,
) -> ToolSchedulingClass {
    if matches!(preparation, PermissionPreparation::Ask { .. }) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "AskUserQuestion" && !ask_user_runs_in_background(tool_call) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "ExitPlanMode"
        && config.permission_mode != PermissionMode::Auto
        && exit_plan_mode_has_reviewable_plan(config)
    {
        return ToolSchedulingClass::BlockingDialog;
    }
    if matches!(tool_call.name.as_str(), "Bash" | "Terminal" | "Write" | "Edit") {
        return ToolSchedulingClass::Exclusive;
    }
    ToolSchedulingClass::ParallelSafe
}

fn ask_user_runs_in_background(tool_call: &AgentToolCall) -> bool {
    tool_call
        .arguments
        .get("background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
```

Do not implement file-path conflict scheduling in this plan. Kimi has
`ToolAccesses.file(...)` and conflict detection; Neo can add that later. This
plan only needs `ParallelSafe`, `Exclusive`, and `BlockingDialog`.

- [ ] **Step 4: Replace ad-hoc parallel/sequential choice with a scheduler**

Replace the old branch:

```rust
match config.tool_execution_mode {
    ToolExecutionMode::Sequential => execute_tool_calls_sequential(...),
    ToolExecutionMode::Parallel => execute_tool_calls_parallel(...),
}
```

with a small scheduler function:

```rust
async fn execute_tool_calls_scheduled(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    skill_invocation_active: &AtomicBool,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    if matches!(config.tool_execution_mode, ToolExecutionMode::Sequential) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            skill_invocation_active,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    if tool_calls.iter().any(|call| {
        let prep = permission_preparation_for_mode(config, call);
        scheduling_class_for_preparation(config, call, &prep) == ToolSchedulingClass::BlockingDialog
    }) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            skill_invocation_active,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    if tool_calls.iter().any(|call| {
        let prep = permission_preparation_for_mode(config, call);
        scheduling_class_for_preparation(config, call, &prep) == ToolSchedulingClass::Exclusive
    }) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            skill_invocation_active,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    execute_tool_calls_parallel(
        config,
        registry,
        skills,
        skill_invocation_active,
        turn,
        tool_calls,
        emitter,
        cancel_token,
        process_supervisor,
    )
    .await
}
```

This intentionally serializes the whole batch when a blocking or exclusive call
is present. It is more conservative than Kimi's access-level scheduler but much
harder to get wrong during the permission-mode migration.

- [ ] **Step 5: Make `AskUserQuestion background=true` honest**

Kimi supports `background=true`, but only because it has a real
`QuestionBackgroundTask` path. Neo must choose one of these two explicit
implementations:

Option A, full support:

```rust
// AskUserTool::execute
if input.background {
    // Register a background question task.
    // Return task_id immediately.
    // Keep the pending question visible in TUI task state.
    // Inject the eventual answer into a later turn as a background task notification.
}
```

Option B, conservative rejection:

```rust
if input.background {
    return Ok(ToolResult::error(
        "AskUserQuestion background=true is not supported yet. Ask a foreground question or continue without user input.",
    ));
}
```

Do **not** implement a hidden wait. If the UI cannot surface and answer the
background question later, choose Option B.

- [ ] **Step 6: Add background Ask User tests**

If Option A is implemented, add:

```rust
#[tokio::test]
async fn ask_user_background_returns_task_id_without_blocking_later_tools() {
    /* assert output contains task_id and later echo can execute before answer */
}
```

If Option B is implemented, add:

```rust
#[tokio::test]
async fn ask_user_background_is_rejected_when_background_question_tasks_are_unavailable() {
    /* assert ToolResult::error contains "background=true is not supported yet" */
}
```

- [ ] **Step 7: Preserve non-blocking parallel behavior**

Keep or add this regression test:

```rust
#[tokio::test]
async fn parallel_mode_still_runs_parallel_safe_tools_by_completion_order() {
    /* two read-only/sleep-echo style tools may finish out of order, but context appends in source order */
}
```

Expected: ordinary non-blocking tools still overlap.

- [ ] **Step 8: Verify scheduling**

Run:

```bash
rtk cargo test -p neo-agent-core --test runtime_turn parallel_mode_serializes_non_background_ask_user_question -- --nocapture
rtk cargo test -p neo-agent-core --test runtime_turn parallel_mode_serializes_manual_approval_batches -- --nocapture
rtk cargo test -p neo-agent-core --test runtime_turn parallel_mode_still_runs_parallel_safe_tools_by_completion_order -- --nocapture
rtk cargo test -p neo-agent-core runtime_turn -- --nocapture
```

Expected: all pass.

## Task 4: Config And CLI Mode Replacement

**Files:**
- Modify: `crates/neo-agent/src/config.rs`
- Modify: `crates/neo-agent/src/cli.rs`
- Modify: `crates/neo-agent/src/modes/run.rs`
- Modify: `crates/neo-agent/tests/mock_provider_e2e.rs`
- Modify: `crates/neo-agent/tests/cli_commands.rs`

- [ ] **Step 1: Replace config field**

`AppConfig` target:

```rust
pub permission_mode: PermissionMode,
```

`FileConfig` target:

```rust
pub(crate) permission_mode: Option<PermissionMode>,
```

Resolution rule:

1. CLI `--yolo` -> `PermissionMode::Yolo`.
2. CLI `--auto` -> `PermissionMode::Auto`.
3. `permission_mode` config if present.
4. `PermissionMode::Manual`.

Reject simultaneous `--auto` and `--yolo`.

- [ ] **Step 2: Delete old config deserialization**

Remove:

```rust
pub(crate) permissions: Option<PermissionPolicy>
```

Do not add old `[permissions]` compatibility parsing.

- [ ] **Step 3: Fix `--yolo` behavior and add `--auto`**

Keep existing trust behavior exactly as it works today, but `--yolo` must also set runtime permission mode. Add `--auto` to CLI:

```rust
#[arg(long = "auto", conflicts_with = "yolo")]
pub auto: bool,
```

- [ ] **Step 4: Pass mode into runtime**

Replace:

```rust
.with_tool_permission_policy(config.permissions.clone())
```

with:

```rust
.with_permission_mode(config.permission_mode)
```

- [ ] **Step 5: Config tests**

Add tests:

```rust
#[test]
fn config_defaults_to_manual_permission_mode() { /* assert Manual */ }

#[test]
fn config_loads_permission_mode_auto() { /* permission_mode = "auto" */ }

#[test]
fn cli_yolo_overrides_config_permission_mode() { /* assert Yolo */ }

#[test]
fn cli_auto_overrides_config_permission_mode() { /* assert Auto */ }
```

- [ ] **Step 6: Verify agent config**

Run:

```bash
rtk cargo test -p neo-agent config -- --nocapture
rtk cargo test -p neo-agent cli_commands -- --nocapture
```

Expected: pass.

## Task 5: TUI State, Footer, And Slash Commands

**Files:**
- Modify: `crates/neo-tui/src/chrome.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-tui/tests/app_shell.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs` tests

- [ ] **Step 1: Change chrome state**

Replace `permission_decision: PermissionDecision` with:

```rust
permission_mode: PermissionMode,
```

Add:

```rust
pub const fn permission_mode(&self) -> PermissionMode { self.permission_mode }

pub fn set_permission_mode(&mut self, mode: PermissionMode) {
    self.permission_mode = mode;
}
```

Delete `permission_badge` using allow/ask/deny. New badge:

```rust
pub fn permission_badge(&self) -> (&'static str, Color) {
    match self.permission_mode {
        PermissionMode::Manual => ("manual", self.theme().footer_permission_ask),
        PermissionMode::Auto => ("auto", self.theme().footer_permission_allow),
        PermissionMode::Yolo => ("yolo", self.theme().footer_permission_deny),
    }
}
```

If color names are misleading, rename theme fields in a separate focused step.

- [ ] **Step 2: Initialize TUI from config**

In `InteractiveController::new` / config application, replace:

```rust
.set_permission_decision(config.permissions.shell);
```

with:

```rust
.set_permission_mode(config.permission_mode);
```

Store the same mode in the controller:

```rust
permission_mode: PermissionMode,
```

- [ ] **Step 3: Ensure mode affects next turn**

Add `permission_mode` to `TurnRequest`, or mutate the effective config before spawning a turn:

```rust
effective_config.permission_mode = request.permission_mode;
```

Test must prove `/yolo` changes actual runtime behavior, not just footer text.

- [ ] **Step 4: Implement slash command handlers**

Add helpers:

```rust
fn set_permission_mode(&mut self, mode: PermissionMode) {
    self.permission_mode = mode;
    self.tui.chrome_mut().set_permission_mode(mode);
    self.push_status(format!("Permission Mode: {}", mode.label()));
}

fn open_permission_picker(&mut self) {
    let items = permission_mode_items(self.permission_mode);
    self.tui.chrome_mut().open_choice_picker(ChoicePickerOptions {
        title: "Select permission mode".to_owned(),
        items,
        initial_id: Some(self.permission_mode.label().to_owned()),
        theme: self.tui.chrome().theme().clone(),
        page_size: 3,
    });
}
```

`handle_slash_command`:

```rust
"/ask" => set Manual
"/auto" => set Auto
"/yolo" => set Yolo
"/permissions" | "/permission" => open picker
```

Accept `/permission` as a harmless alias because Kimi uses singular in some places, but document `/permissions` as Neo's command.

- [ ] **Step 5: Implement picker result**

In `handle_choice_picker_result`, match ids:

```rust
"permission:manual" => set_permission_mode(PermissionMode::Manual)
"permission:auto" => set_permission_mode(PermissionMode::Auto)
"permission:yolo" => set_permission_mode(PermissionMode::Yolo)
```

Descriptions must match Kimi copy. Current row label should append ` ← current`.

- [ ] **Step 6: Update command palette and slash completion**

Add command palette specs:

```rust
CommandSpec::new("permissions", "Open permissions", Some("Select permission mode"))
CommandSpec::new("permission.manual", "Manual permission mode", Some("Ask before risky actions"))
CommandSpec::new("permission.auto", "Auto permission mode", Some("Run non-interactively"))
CommandSpec::new("permission.yolo", "YOLO permission mode", Some("Skip confirmations"))
```

Add slash completion items:

```rust
PickerItem::new("/permissions", "/permissions", Some("select permission mode"))
PickerItem::new("/ask", "/ask", Some("manual permission mode"))
PickerItem::new("/auto", "/auto", Some("auto permission mode"))
PickerItem::new("/yolo", "/yolo", Some("yolo permission mode"))
```

- [ ] **Step 7: TUI tests**

Add tests:

```rust
#[test]
fn footer_renders_permission_mode_badge() { /* [manual], [auto], [yolo] */ }

#[tokio::test]
async fn slash_permission_modes_update_footer_and_next_turn() { /* /yolo then prompt */ }

#[tokio::test]
async fn slash_permissions_opens_picker_and_selects_mode() { /* Down/Enter -> auto */ }

#[test]
fn slash_completion_contains_permission_commands() { /* all four commands */ }
```

- [ ] **Step 8: Verify TUI slice**

Run:

```bash
rtk cargo test -p neo-tui app_shell -- --nocapture
rtk cargo test -p neo-agent interactive -- --nocapture
```

Expected: pass.

## Task 6: Improve `/plan` Toggle And Transcript Output

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-tui/src/chrome.rs`
- Modify: `crates/neo-agent-core/src/tools/plan_mode.rs`
- Modify: `crates/neo-tui/src/transcript/tool_call.rs`
- Modify: `crates/neo-tui/src/transcript/plan_box.rs`

- [ ] **Step 1: Add controller helper**

```rust
fn set_plan_mode_from_user(&mut self, active: bool) {
    self.tui.chrome_mut().set_plan_mode(active);
    self.push_status(if active { "Plan Mode On" } else { "Plan Mode Off" });
}
```

If runtime shared `PlanMode` must also be toggled, wire the helper into that state, not only chrome.

- [ ] **Step 2: Make `/plan` toggle**

Replace current `/plan` empty behavior:

```rust
let next = !self.tui.chrome_mut().is_plan_mode();
self.set_plan_mode_from_user(next);
```

Keep:

```rust
"/plan on" => Plan Mode On
"/plan off" => Plan Mode Off
"/plan clear" => existing clear behavior
```

Unknown args:

```text
Unknown /plan argument: '<arg>'. Usage: /plan [on|off|clear]
```

- [ ] **Step 3: Transcript status tests**

Add tests:

```rust
#[tokio::test]
async fn slash_plan_toggles_on_then_off() {
    // type /plan, submit, assert status "Plan Mode On"
    // type /plan, submit, assert status "Plan Mode Off"
}

#[tokio::test]
async fn plan_hotkey_or_command_updates_footer() {
    // assert footer contains [PLAN MODE] only while active
}
```

- [ ] **Step 4: Plan review UX checks**

Ensure existing ExitPlanMode card behavior remains:

- EnterPlanMode success output does not leak long prompt scaffolding.
- ExitPlanMode result renders a plan box.
- Revise feedback is displayed and sent back to runtime.

Run:

```bash
rtk cargo test -p neo-tui tool_cards -- --nocapture
rtk cargo test -p neo-agent interactive -- --nocapture
```

Expected: pass.

## Task 7: Shift+Enter Mode Cycle

**Files:**
- Modify: `crates/neo-tui/src/input.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-tui/tests/primitives.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs` tests

- [ ] **Step 1: Add keybinding action**

Add enum variant:

```rust
CyclePermissionMode,
```

Action id:

```rust
Self::CyclePermissionMode => "tui.permission.cycle",
```

Parser:

```rust
"tui.permission.cycle" => Self::CyclePermissionMode,
```

Default binding:

```rust
definition(Action::CyclePermissionMode, &["shift+enter"], "Cycle plan/manual/auto/yolo mode")
```

- [ ] **Step 2: Stop hard-coding Shift+Enter as newline**

In `InputEvent::from_key_event_with_keybindings`, let keybindings inspect Shift+Enter before newline fallback. Keep:

```rust
alt+enter => NewLine
ctrl+j => NewLine
```

Keep bracketed paste behavior unchanged.

- [ ] **Step 3: Implement cycle handler**

In `handle_keybinding_action`:

```rust
KeybindingAction::CyclePermissionMode => self.cycle_permission_mode(),
```

Target helper:

```rust
fn cycle_permission_mode(&mut self) {
    if self.tui.chrome_mut().is_plan_mode() {
        self.set_plan_mode_from_user(false);
        self.set_permission_mode(PermissionMode::Manual);
        return;
    }
    match self.permission_mode {
        PermissionMode::Manual => self.set_permission_mode(PermissionMode::Auto),
        PermissionMode::Auto => self.set_permission_mode(PermissionMode::Yolo),
        PermissionMode::Yolo => self.set_plan_mode_from_user(true),
    }
}
```

This yields the sequence `plan -> manual -> auto -> yolo -> plan` when starting in plan mode, and `manual -> auto -> yolo -> plan -> manual` when starting in manual.

- [ ] **Step 4: Input tests**

Update old Shift+Enter newline tests:

```rust
#[test]
fn shift_enter_uses_permission_cycle_binding() { /* expects CyclePermissionMode */ }

#[test]
fn alt_enter_still_inserts_newline() { /* expects NewLine */ }

#[test]
fn ctrl_j_still_inserts_newline() { /* expects NewLine */ }

#[test]
fn bracketed_paste_preserves_newlines() { /* existing behavior */ }
```

- [ ] **Step 5: Controller cycle tests**

Add:

```rust
#[tokio::test]
async fn shift_enter_cycles_plan_manual_auto_yolo() {
    // trigger Shift+Enter repeatedly
    // assert plan on, then manual, then auto, then yolo, then plan on
}
```

- [ ] **Step 6: Verify input**

Run:

```bash
rtk cargo test -p neo-tui primitives -- --nocapture
rtk cargo test -p neo-agent interactive -- --nocapture
```

Expected: pass.

## Task 8: Approval UI And Plan Review Polish

**Files:**
- Modify: `crates/neo-tui/src/chrome.rs`
- Modify: `crates/neo-tui/src/transcript/tool_call.rs`
- Modify: `crates/neo-tui/src/transcript/plan_box.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: Keep inline approval transcript UX**

Approval prompts must remain visible and operable in the chat/tool transcript near the running tool card. Do not add a separate page or unrelated panel.

General approval options:

```text
Approve once
Approve for this session
Reject
Reject with feedback
```

Plan review options:

```text
Approve
Reject
Revise
```

- [ ] **Step 2: Feedback path**

When user chooses "Reject with feedback" or "Revise", the feedback must reach runtime and then the model. Validate that `plan_review_feedback` or its replacement is keyed by tool call id and consumed exactly once.

- [ ] **Step 3: Tests**

Add tests:

```rust
#[tokio::test]
async fn revise_exit_plan_mode_returns_feedback_to_model() { /* assert tool result includes feedback */ }

#[tokio::test]
async fn approve_for_session_skips_later_manual_prompt() { /* first asks, second does not */ }
```

Run:

```bash
rtk cargo test -p neo-agent interactive -- --nocapture
rtk cargo test -p neo-agent-core runtime_turn -- --nocapture
```

Expected: pass.

## Task 9: Documentation And Examples Cleanup

**Files:**
- Modify: `docs/config.md`
- Modify: `docs/tools.md`
- Modify: `docs/quickstart.md`
- Modify: `docs/architecture.md`
- Modify: `README.md` if it mentions old permissions
- Modify: `examples/config/*.toml`
- Modify: `AGENTS.md` only if project docs need current-state update

- [ ] **Step 1: Delete old permission docs**

Remove examples like:

```toml
[permissions]
file_read = "Allow"
file_write = "Ask"
shell = "Ask"
tool = "Allow"
```

- [ ] **Step 2: Add new config docs**

Document:

```toml
permission_mode = "manual"
```

Use only `permission_mode` as the canonical Neo config key. Do not implement `default_permission_mode`.

- [ ] **Step 3: Document slash commands**

Add:

```text
/ask          switch to manual permission mode
/auto         switch to auto permission mode
/yolo         switch to yolo permission mode
/permissions  open permission mode selector
/plan         toggle plan mode
```

- [ ] **Step 4: Verify no stale old docs**

Run:

```bash
rtk rg -n "file_read|file_write|shell = \"Ask\"|tool = \"Allow\"|PermissionPolicy|PermissionDecision|\\[permissions\\]" docs README.md examples crates
```

Expected: no user-facing old config references. Internal deletion-plan references in `docs/superpowers/plans/` are acceptable.

- [ ] **Step 5: Docs parity**

Run:

```bash
rtk cargo run -p xtask -- parity
```

Expected: pass.

## Task 10: Final Verification

**Files:**
- No new edits unless verification exposes a bug.

- [ ] **Step 1: Format**

Run:

```bash
rtk cargo fmt --all --check
```

Expected: pass.

- [ ] **Step 2: Focused tests**

Run:

```bash
rtk cargo test -p neo-agent-core runtime_turn -- --nocapture
rtk cargo test -p neo-agent-core tool_permissions -- --nocapture
rtk cargo test -p neo-tui primitives -- --nocapture
rtk cargo test -p neo-tui app_shell -- --nocapture
rtk cargo test -p neo-agent interactive -- --nocapture
rtk cargo test -p neo-agent cli_commands -- --nocapture
```

Expected: all pass.

- [ ] **Step 3: Workspace check**

Run:

```bash
rtk cargo run -p xtask -- check --workspace
```

Expected: pass. If pre-existing unrelated failures appear, record exact failing tests and prove they do not touch the files in this plan.

- [ ] **Step 4: Manual TUI smoke**

Run:

```bash
rtk cargo run -p neo-agent
```

Smoke steps:

1. Confirm footer starts with `[manual]` unless config says otherwise.
2. Type `/permissions`, choose Auto, confirm footer shows `[auto]`.
3. Type `/ask`, confirm footer shows `[manual]`.
4. Press Shift+Enter repeatedly and confirm cycle: plan on -> manual -> auto -> yolo -> plan on.
5. Type `/plan` twice and confirm transcript contains `Plan Mode On` then `Plan Mode Off`.
6. In manual mode, request a Bash command and confirm inline approval appears.
7. In auto mode, request a tool action and confirm no approval blocks execution.
8. In auto mode, trigger/ask for a model question and confirm `AskUserQuestion` is denied/skipped.

Expected: all smoke steps match.

## Completion Criteria

- `PermissionPolicy` and `PermissionDecision::{Allow,Ask,Deny}` are gone from production code.
- Runtime/config/TUI all use `PermissionMode`.
- `/ask`, `/auto`, `/yolo`, `/permissions`, `/plan` work in TUI.
- Shift+Enter cycles `plan -> manual -> auto -> yolo`.
- Auto mode denies/skips `AskUserQuestion`.
- Yolo mode skips normal confirmations but still allows explicit user questions.
- Plan mode guard cannot be bypassed by auto or yolo.
- Non-empty `ExitPlanMode` review is auto-approved only in auto mode; yolo still reviews.
- Tool batches containing blocking dialogs are serialized before later tools
  start, while parallel-safe tools can still overlap.
- `AskUserQuestion background=true` either has a real visible background-task
  answer path or is rejected; it must never become a hidden indefinite wait.
- Footer displays `[manual]`, `[auto]`, or `[yolo]`, never `[ask]`.
- Docs and examples no longer teach old `[permissions]` config.

## Suggested Subagent Split

Use at least 3 parallel workers only after the core type shape is agreed:

- Worker A: core runtime and tests (`crates/neo-agent-core`).
- Worker A2: runtime scheduling and AskUser background semantics
  (`crates/neo-agent-core/src/runtime.rs`, `tools/ask_user.rs`,
  `runtime_turn` tests). Run after Worker A defines the new permission types.
- Worker B: config/CLI/run wiring (`crates/neo-agent/src/config.rs`, `cli.rs`, `modes/run.rs`, CLI tests).
- Worker C: TUI state/slash/selector/hotkey (`crates/neo-tui`, `crates/neo-agent/src/modes/interactive.rs`).
- Worker D: docs/examples cleanup after A-C land.

All workers must include the git mutation ban in their prompt.
