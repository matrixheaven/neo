# NEO-30 Session Approval Key Scope Handoff Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix `Approve for this session` so one approval does not approve every future call to the same tool, especially `Bash`. Replace tool-name session approvals with Codex-style approval keys: a later request may skip prompting only when every required approval key for that request was already approved in this session.

**Architecture:** Treat session approval as a reusable permission grant over explicit operation scopes, not over tools. The runtime should derive stable `SessionApprovalKey` values from the tool call, workspace/cwd, operation, and normalized target. The TUI must show the scope it is about to cache, so the user can tell whether they are approving one exact command, one file path, or another narrow reusable target.

**Tech Stack:** Rust 2024, `neo-agent-core` permission preparation/runtime, `neo-agent` interactive/run approval plumbing, `neo-tui` approval modal/transcript rendering, Codex approval-key references, `xtask`/`nextest` focused verification.

---

## Linear Context

- Linear: [NEO-26](https://linear.app/ezc2/issue/NEO-26/scope-approve-for-this-session-by-approval-key-instead-of-tool-name)
- Title: Scope "Approve for this session" by approval key instead of tool name
- Priority: Urgent
- Project: Mode System
- Label: Bug
- User-observed symptom: approving one Bash command with `Approve for this session` appears to allow later `git status`, `git log --oneline -20`, `git diff --stat`, and `git branch --show-current && git remote -v` without another prompt.

## Mandatory References

Read before implementation:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `crates/neo-agent-core/src/runtime.rs`
- `crates/neo-agent-core/src/permissions.rs`
- `crates/neo-agent-core/src/events.rs`
- `crates/neo-agent-core/src/tools/bash.rs`
- `crates/neo-agent-core/src/tools/terminal.rs`
- `crates/neo-agent/src/modes/run.rs`
- `crates/neo-agent/src/modes/interactive.rs`
- `crates/neo-tui/src/chrome.rs`
- `crates/neo-tui/src/transcript/entry.rs`
- `docs/codex/codex-rs/core/src/tools/sandboxing.rs`
- `docs/codex/codex-rs/core/src/tools/runtimes/shell.rs`
- `docs/codex/codex-rs/core/src/tools/network_approval.rs`
- `docs/codex/codex-rs/tui/src/app.rs`
- `docs/codex/codex-rs/tui/src/history_cell/approvals.rs`

Reference conclusions:

- Current Neo stores session approvals by tool name. That is the bug.
- Codex stores serialized approval keys, not tool names.
- Codex's `with_cached_approval` takes a vector of keys. A request is skipped only if **all** keys are already approved for the session.
- Codex stores each approved key individually. This lets future requests touching a subset of targets skip prompting without broadening the grant.
- Codex shell approval keys include canonicalized command, cwd, sandbox permissions, and additional permissions.
- Codex approval transcript text says what command/network target was approved for the session. Neo needs similar scope visibility.

## Non-Negotiable Project Rules

- Before coding, run:

```bash
rtk icm recall-context "NEO-26 session approval key scope Approve for this session Bash tool name" --limit 5
```

- Use `rtk` for shell commands.
- Use `cx` before broad file reads when navigating symbols.
- Do not run bare `cargo test`; use `cargo run -p xtask -- test ...` through `rtk`.
- Do not perform git mutations unless the user gives explicit per-command authorization. This includes `git add`, `git commit`, `git push`, `git switch`, `git checkout`, `git reset`, `git stash`, `git clean`, `git rm`, `git merge`, and `git rebase`.
- Do not preserve obsolete compatibility branches or duplicate approval stores. Replace the old tool-name model.
- Stay inside NEO-26 scope. Do not redesign all permission modes.
- If a meaningful error is resolved, store it with ICM before final response.
- When the task is complete, store a significant-task memory before final response:

```bash
rtk icm store -t context-neo -c "Completed NEO-26: replaced tool-name session approvals with narrow approval-key scoped session approvals, added dynamic approval scope labels, and verified Bash/file approval regressions." -i high -k "NEO-26,approval,session,Bash,permissions"
```

## Current Neo State

### Runtime Bug

- `crates/neo-agent-core/src/runtime.rs`
  - `AgentConfig::session_approvals` is currently:

```rust
pub session_approvals: Arc<Mutex<std::collections::HashSet<String>>>,
```

- Its comment says:

```text
Tool names approved for the current session via "Approve for this session".
```

- `prepare_tool_call` checks this before the manual fallback:

```rust
if config
    .session_approvals
    .lock()
    .ok()
    .is_some_and(|set| set.contains(&tool_call.name))
{
    return PermissionPreparation::Run(access_for_tool(tool_call, true));
}
```

- `resolve_approval` stores the tool name:

```rust
set.insert(tool_call.name.clone());
```

Impact: approving `Bash` once for the session approves every future `Bash`, including commands with unrelated side effects.

### Existing Permission Request Shape

- `crates/neo-agent-core/src/runtime.rs`
  - `ApprovalRequest` currently has:

```rust
pub struct ApprovalRequest {
    pub turn: u32,
    pub id: String,
    pub operation: PermissionOperation,
    pub subject: String,
    pub arguments: serde_json::Value,
}
```

- `AgentEvent::ApprovalRequested` in `crates/neo-agent-core/src/events.rs` carries the same data.
- There is no field for reusable session scope, label, key count, or whether session approval is available.

### Existing UI Shape

- `crates/neo-tui/src/chrome.rs`
  - `ApprovalRequestModal::new` hard-codes four options:
    - `Approve once`
    - `Approve for this session`
    - `Reject`
    - `Reject with feedback`
- `crates/neo-tui/src/transcript/entry.rs`
  - inline approval rendering hard-codes the same labels.
- `crates/neo-agent/src/modes/interactive.rs`
  - `approval_result_label` maps `AlwaysApprove` to `Approved for this session`.
  - `approval_decision` maps `AlwaysApprove` to `PermissionApprovalDecision::AllowForSession`.
- Existing TUI tests correctly assert that the TUI must not turn one approval into a global bypass. They do not protect the runtime from coarse session keys.

### Bash Tool Details

- `crates/neo-agent-core/src/tools/bash.rs`
  - input fields include `command`, optional workspace-relative `cwd`, timeout/background settings, and output caps.
  - `spawn_bash_process` resolves `cwd` through `ToolContext::resolve_workspace_path` when provided.

Session approval keys for Bash must include the effective cwd. The same command in a different directory must not inherit approval.

## Codex Model To Copy

Codex's relevant shape is in `docs/codex/codex-rs/core/src/tools/sandboxing.rs`:

```rust
pub(crate) async fn with_cached_approval<K, F, Fut>(
    services: &SessionServices,
    tool_name: &str,
    keys: Vec<K>,
    fetch: F,
) -> ReviewDecision
where
    K: Serialize,
```

Behavior:

- If `keys` is empty, prompt normally.
- If every key is already `ApprovedForSession`, skip prompting.
- If the user approves for session, store each key individually.

Codex also has an `Approvable` trait:

```rust
fn approval_keys(&self, req: &Req) -> Vec<Self::ApprovalKey>;
```

For shell, Codex uses:

```rust
ApprovalKey {
    command: canonicalize_command_for_approval(&req.command),
    cwd: req.cwd.clone(),
    sandbox_permissions: req.sandbox_permissions,
    additional_permissions: req.additional_permissions.clone(),
}
```

For Neo, the direct equivalent is not sandbox permissions yet, but the principle still applies: the approval cache key must be derived from the operation target, not the tool name.

## Product Decisions

### Replace Tool-Name Approvals

Delete the old `HashSet<String>` model and replace it with:

```rust
pub session_approvals: Arc<Mutex<HashSet<SessionApprovalKey>>>,
```

Do not keep a compatibility branch that still checks tool names.

### Session Approval Is Optional

Some approval prompts should not offer a session-scoped approval:

- `ExitPlanMode`
- `ExitGoalMode`
- `AskUserQuestion`
- `TaskStop`
- interactive `Terminal` write/read operations where the future target cannot be predicted
- any tool call whose scope cannot be represented safely

For those cases, the UI should not show a vague reusable option. Prefer only:

1. Approve once
2. Reject
3. Reject with feedback

For review-style approvals such as `ExitPlanMode`, keep the existing review labels:

1. Approve
2. Reject
3. Reject with feedback

### Session Approval Key Shape

Define these in `crates/neo-agent-core/src/permissions.rs` or a small sibling module imported from there:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum SessionApprovalKey {
    BashExact {
        workspace: String,
        cwd: String,
        command: String,
    },
    FileWrite {
        workspace: String,
        path: String,
        operation: FileWriteApprovalOperation,
    },
    ToolExact {
        tool: String,
        arguments_hash: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum FileWriteApprovalOperation {
    Write,
    Edit,
}
```

Recommended first implementation:

- Use `BashExact` for every Bash command.
- Do not attempt a broad `GitReadOnly` key in the first patch. Exact-command session approval is much safer and directly fixes the user's reported issue.
- Include effective cwd, not merely workspace root.
- Include workspace root to avoid cross-workspace inheritance if config/session objects are reused.
- Include exact normalized command text, not a broad command family.
- Use stable whitespace normalization only if it cannot change command semantics. Trimming leading/trailing whitespace is okay; collapsing arbitrary internal whitespace is not safe for quoted strings or shell scripts.

Future extension, not part of this patch:

- A shell parser can later introduce narrower safe families such as `GitStatusExactFlags` or `GitDiffReadOnlyExactPathspec`, but only when the parser is reliable and the UI says the exact family.

### Approval Scope Descriptor

Runtime and UI need a user-facing descriptor in addition to hashable keys:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionApprovalScope {
    pub keys: Vec<SessionApprovalKey>,
    pub label: String,
    pub detail: String,
}
```

Examples:

- Bash:
  - `label`: `Approve this exact command for this session`
  - `detail`: `Exact Bash command in /Users/chenyuanhao/Workspace/neo`
- Write:
  - `label`: `Approve writes to this file for this session`
  - `detail`: `File: docs/superpowers/plans/example.md`
- Edit:
  - `label`: `Approve edits to this file for this session`
  - `detail`: `File: crates/neo-agent-core/src/runtime.rs`

The label is what the modal option uses. The detail is rendered inside the approval card and can be recorded in transcript.

### Cache Semantics

The equivalent of Codex `with_cached_approval` should be:

```rust
fn session_scope_is_approved(config: &AgentConfig, scope: &SessionApprovalScope) -> bool {
    if scope.keys.is_empty() {
        return false;
    }
    config
        .session_approvals
        .lock()
        .ok()
        .is_some_and(|set| scope.keys.iter().all(|key| set.contains(key)))
}

fn approve_scope_for_session(config: &AgentConfig, scope: SessionApprovalScope) {
    if let Ok(mut set) = config.session_approvals.lock() {
        set.extend(scope.keys);
    }
}
```

Rules:

- Empty key list never skips prompting.
- A request with multiple keys skips only when all keys are approved.
- On session approval, each key is inserted individually.
- `AllowForSession` with `None` scope is treated as `AllowOnce`; optionally log/debug this because UI should normally prevent it.

### Bash Scope

Default:

```rust
SessionApprovalKey::BashExact {
    workspace,
    cwd,
    command,
}
```

Normalize:

- `command`: `trim()` only.
- `cwd`: resolve `arguments.cwd` through the same workspace containment semantics as the tool execution path, or use workspace/session cwd when omitted.
- `workspace`: resolved workspace root if available, otherwise a stable placeholder such as `""`.

Do not broaden:

- `git status` must not approve `git log`.
- `git status` must not approve `git status && git push`.
- `git branch --show-current && git remote -v` should be one exact compound command unless Neo later introduces a real shell parser. It must not be covered by an approval for only `git branch --show-current`.
- `git diff --stat` must not approve `git diff --cached -- . ':!safe'` unless it is exactly the same normalized command and cwd.
- Background commands should use a distinct exact key from foreground commands or be session-approval-ineligible. Recommended first patch: make background Bash ineligible for session approval unless there is a clear product need.

### File Write/Edit Scope

Recommended first implementation:

- `Write` and `Edit` may offer session approval only when there is a valid path.
- Key includes resolved workspace-relative or canonical workspace-contained path plus operation.
- `Write` approval for one file does not approve `Edit` for the same file unless the team intentionally wants combined write access. Keep them separate for the first patch.
- Writes to active plan-mode file that are auto-approved by plan-mode helper path should remain auto-approved before manual prompts, as today.

### Unknown Dynamic Tools

For MCP, extensions, and generic custom tools:

- Prefer no session approval until a safe key is designed.
- If product insists on session approval, use `ToolExact { tool, arguments_hash }` and label it as exact arguments.
- Do not use `tool name` alone.

Recommended first patch:

- Only implement reusable session scopes for `Bash`, `Write`, and `Edit`.
- Other manual prompts keep `Approve once`, `Reject`, `Reject with feedback`.

## TUI / UX Design

### Exact Bash Command Approval

Inline transcript card:

```text
────────────────────────────────────────────────────────
▶ Shell approval

    git status

    scope: exact Bash command
    cwd:   /Users/chenyuanhao/Workspace/neo

  ▶ 1. Approve once
    2. Approve this exact command for this session
    3. Reject
    4. Reject with feedback
────────────────────────────────────────────────────────
```

Bottom blocking modal:

```text
╭─ Shell approval ──────────────────────────────────────╮
│ git status                                            │
│                                                       │
│ Scope if saved: exact Bash command                    │
│ Directory: /Users/chenyuanhao/Workspace/neo           │
│                                                       │
│  1  Approve once                                      │
│  2  Approve this exact command for this session       │
│  3  Reject                                            │
│  4  Reject with feedback                              │
╰───────────────────────────────────────────────────────╯
```

Resolved transcript:

```text
approval: Approved exact Bash command for this session
```

### Unsupported Reusable Scope

Use this when session approval is unsafe or undefined:

```text
╭─ Tool approval ───────────────────────────────────────╮
│ TaskStop: background-task-17                          │
│                                                       │
│  1  Approve once                                      │
│  2  Reject                                            │
│  3  Reject with feedback                              │
╰───────────────────────────────────────────────────────╯
```

Do not show `Approve for this session` in this case.

### File Write Approval

```text
╭─ File write approval ─────────────────────────────────╮
│ docs/superpowers/plans/example.md                     │
│                                                       │
│ Scope if saved: writes to this file                   │
│ Workspace: /Users/chenyuanhao/Workspace/neo           │
│                                                       │
│  1  Approve once                                      │
│  2  Approve writes to this file for this session      │
│  3  Reject                                            │
│  4  Reject with feedback                              │
╰───────────────────────────────────────────────────────╯
```

### Review Approval Remains Different

Do not show session approval for plan/goal review:

```text
╭─ Plan review ─────────────────────────────────────────╮
│ Exit plan mode                                        │
│                                                       │
│  1  Approve                                           │
│  2  Reject                                            │
│  3  Reject with feedback                              │
╰───────────────────────────────────────────────────────╯
```

## Implementation Plan

### Step 1: Add Failing Runtime Tests

- [ ] Add tests in `crates/neo-agent-core/tests/runtime_turn.rs` or a focused permission test module.
- [ ] Build a fake model sequence that requests one Bash command, receives an `AllowForSession`, then requests a different Bash command.
- [ ] Assert the second command emits another `ApprovalRequested` instead of running automatically.
- [ ] Add a positive test: approving `git status` for session allows a later identical `git status` in the same cwd.
- [ ] Add a cwd test: approving `git status` in cwd `.` does not approve the same command in cwd `crates/neo-agent`.
- [ ] Add a compound command test: approving `git branch --show-current` does not approve `git branch --show-current && git remote -v`.
- [ ] Add file tests: approving `Write` for `a.txt` does not approve `Write` for `b.txt`; approving `Write` for `a.txt` does not approve `Edit` for `a.txt`.
- [ ] Add one test proving `ExitPlanMode`/`ExitGoalMode` session approval is treated as one-shot or no session option.

Expected failure before implementation:

- Any test where a second different Bash command is expected to prompt will fail because `session_approvals` contains `Bash`.

### Step 2: Introduce Session Approval Types

- [ ] Add `SessionApprovalKey`, `FileWriteApprovalOperation`, and `SessionApprovalScope`.
- [ ] Derive `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash` where needed, plus `Serialize`, `Deserialize`, and `JsonSchema` if event/config surfaces require it.
- [ ] Keep the types in `neo-agent-core`, ideally near `PermissionApprovalDecision` and `PermissionOperation` so the approval model stays coherent.
- [ ] Add small unit tests for key equality where useful.

### Step 3: Replace Config Store

- [ ] Change `AgentConfig::session_approvals` to `Arc<Mutex<HashSet<SessionApprovalKey>>>`.
- [ ] Update `AgentConfig::for_model`.
- [ ] Remove comments and code that refer to tool-name approvals.
- [ ] Do not add a second old/new store.

### Step 4: Compute Scope During Permission Preparation

Change `PermissionPreparation::Ask` from:

```rust
Ask {
    operation: PermissionOperation,
    subject: String,
}
```

to:

```rust
Ask {
    operation: PermissionOperation,
    subject: String,
    session_scope: Option<SessionApprovalScope>,
}
```

Then:

- [ ] Derive `operation` and `subject` as today.
- [ ] Compute `session_scope` at the same point, before returning `Ask`.
- [ ] Check `session_scope_is_approved` before returning `Ask`.
- [ ] Do not check approval cache by `tool_call.name`.
- [ ] Keep existing auto-approved branches in their current order where appropriate:
  - Auto mode hard deny for non-background `AskUserQuestion`.
  - Background `AskUserQuestion`.
  - Auto mode.
  - `EnterPlanMode`.
  - Plan-mode helper writes.
  - Yolo mode.
  - Default safe tools.

Important ordering:

- Session approval check should happen only after one-shot special cases that should never be cached are handled, or the scope helper must return `None` for those special cases.
- Do not allow a session approval to skip `ExitPlanMode`/`ExitGoalMode` review.

### Step 5: Implement Scope Derivation

Add helpers in `runtime.rs` or a dedicated `approval_scope.rs`:

```rust
fn session_approval_scope_for_tool_call(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    operation: PermissionOperation,
    subject: &str,
) -> Option<SessionApprovalScope>
```

Bash:

- [ ] Read `arguments.command` as string.
- [ ] Trim leading/trailing whitespace. Do not collapse internal whitespace.
- [ ] Resolve effective cwd:
  - if `arguments.cwd` exists, resolve it against `config.workspace_root`;
  - otherwise use `config.workspace_root` or a stable empty cwd if unavailable.
- [ ] If cwd cannot be resolved inside the workspace, return `None` and let normal tool validation deny or handle it later.
- [ ] Return one `BashExact` key.
- [ ] Label: `Approve this exact command for this session`.
- [ ] Detail: `Exact Bash command in <cwd>`.

Write/Edit:

- [ ] Read `arguments.path`.
- [ ] Resolve to workspace-contained path.
- [ ] Return one `FileWrite` key with operation `Write` or `Edit`.
- [ ] Labels:
  - Write: `Approve writes to this file for this session`
  - Edit: `Approve edits to this file for this session`
- [ ] Detail: `File: <path>`.

Other tools:

- [ ] Return `None` for first patch.

### Step 6: Store Scope On AllowForSession

Change `resolve_approval` to accept scope:

```rust
async fn resolve_approval(
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    operation: PermissionOperation,
    subject: String,
    session_scope: Option<SessionApprovalScope>,
    emitter: &mut impl EventPublisher,
    cancel_token: &CancellationToken,
) -> Option<ToolResult>
```

Then:

- [ ] Include `session_scope` in `ApprovalRequest`.
- [ ] Include `session_scope` in `AgentEvent::ApprovalRequested`.
- [ ] On `AllowForSession`, if `session_scope` is `Some`, insert every key.
- [ ] On `AllowForSession` with `None`, treat as `AllowOnce`.
- [ ] Keep plan/goal feedback behavior unchanged.

### Step 7: Plumb Scope Through Run And Interactive Modes

- [ ] Extend `PromptApprovalRequest` in `crates/neo-agent/src/modes/run.rs` to include the session scope descriptor or at least its label/detail.
- [ ] When the async approval handler receives an `ApprovalRequest`, forward scope label/detail to TUI.
- [ ] Update `InteractiveController::register_pending_approval` to pass scope-aware options into chrome.
- [ ] Update tests that construct `PromptApprovalRequest` manually.

Design preference:

- The runtime owns the scope and keys.
- The TUI only receives display metadata and returns `AllowForSession` or not.
- The TUI must not compute approval keys.

### Step 8: Make Chrome Approval Modal Options Dynamic

Current:

```rust
ApprovalRequestModal::new(
    request_id,
    title,
    body,
)
```

Add either:

```rust
ApprovalRequestModal::new_tool(
    request_id,
    title,
    body,
    session_option_label: Option<String>,
)
```

or extend `new` to accept an options builder.

Rules:

- If session label exists:
  - `Approve once`
  - `<session label>`
  - `Reject`
  - `Reject with feedback`
- If session label is absent:
  - `Approve once`
  - `Reject`
  - `Reject with feedback`
- Review dialogs continue using `new_review`.
- Numeric shortcuts must follow the visible list. If no session option exists, `2` should reject, not choose a hidden always-approve action.

### Step 9: Update Inline Transcript Rendering

- [ ] Extend `ApprovalPromptData` to carry `options: Vec<ApprovalOption>` or enough labels to render dynamic options.
- [ ] Render the same option labels as chrome.
- [ ] Show `scope:` detail when provided.
- [ ] Update selection sync logic because option indices can change when session approval is absent.
- [ ] Update resolved label:
  - for scope-aware session approval, use `Approved exact Bash command for this session`, `Approved writes to this file for this session`, etc.
  - for one-shot, keep `Approved`.
  - for rejected, keep existing labels.

### Step 10: Update Events And JSONL Compatibility

`AgentEvent::ApprovalRequested` is persisted in sessions. Adding optional fields is acceptable if serde defaults are used.

- [ ] Add optional field:

```rust
session_scope: Option<SessionApprovalScope>,
```

- [ ] Ensure old JSONL without this field replays cleanly.
- [ ] Ensure new JSONL includes it only when useful.
- [ ] Add or update session replay tests if needed.

Do not create a parallel event variant.

### Step 11: Tests

Focused runtime tests:

- [ ] `bash_session_approval_is_exact_command_scoped`
- [ ] `bash_session_approval_reuses_identical_command_in_same_cwd`
- [ ] `bash_session_approval_is_cwd_scoped`
- [ ] `bash_session_approval_does_not_cover_compound_command`
- [ ] `file_write_session_approval_is_path_scoped`
- [ ] `file_write_session_approval_is_operation_scoped`

Focused TUI/interactive tests:

- [ ] approval modal shows dynamic label for Bash exact scope.
- [ ] modal omits session option when scope is `None`.
- [ ] numeric shortcuts map to visible options when session option is omitted.
- [ ] inline approval prompt uses dynamic labels.
- [ ] existing approval blocking-dialog tests still pass.

Session/JSONL tests:

- [ ] replay older `ApprovalRequested` events without `session_scope`.
- [ ] serialize/deserialize new `ApprovalRequested` with a scope.

### Step 12: Docs

- [ ] Update `docs/permissions.md` or the relevant TUI/permissions doc if it exists.
- [ ] Mention that `Approve for this session` is scoped to the visible target, not all commands.
- [ ] Include an example:
  - approving `git status` for session repeats only that exact command in that cwd;
  - different Bash commands still prompt.

## Suggested Verification

Use focused commands only:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core runtime_turn session_approval
rtk cargo run -p xtask -- test -p neo-agent interactive approval
rtk cargo fmt --all --check
```

If test names differ, use:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core approval
rtk cargo run -p xtask -- test -p neo-agent approval
```

This is a medium-sized permission/runtime/UI task. Full workspace CI is not required unless the patch grows into a broader approval refactor.

## Easy Pitfalls

- Do not store `tool_call.name` anywhere as a session approval key.
- Do not let `AllowForSession` with `None` scope create a wildcard.
- Do not compute approval keys in the TUI. The runtime must own security semantics.
- Do not reuse an approval across cwd changes.
- Do not normalize shell whitespace aggressively; shell quoting and here-docs can change meaning.
- Do not parse `git` with ad hoc string splitting and call it safe. Exact-command scope is acceptable and safer for the first patch.
- Do not let a read-only looking git command approve later destructive git commands.
- Do not let `git status` approve `git status && git push`.
- Do not forget background Bash. Either exact-scope it with background state included, or omit the session option.
- Do not break plan-mode helper writes to the active plan file.
- Do not break `ExitPlanMode`/`ExitGoalMode` review feedback side-channel.
- Do not leave transcript labels saying vague `Approve for this session` if the actual scope is exact command/file.
- Do not rely only on UI tests. The bug lives in runtime caching.

## Self Review Checklist

- [ ] Can I point to the code that removed `HashSet<String>` tool-name approvals?
- [ ] Can I show the test where `git status` approval does not approve `git log --oneline -20`?
- [ ] Can I show the test where identical `git status` in the same cwd is reused?
- [ ] Can I show the test where the same command in a different cwd prompts again?
- [ ] Can I show that `ExitPlanMode` and `ExitGoalMode` cannot become session-approved wildcards?
- [ ] Does the UI label tell the user exactly what is cached?
- [ ] Does inline transcript rendering match the modal options?
- [ ] Does numeric shortcut behavior remain predictable after dynamic options?
- [ ] Does old session JSONL still replay?
- [ ] Did I avoid broad git/read-only command families in the first patch?
- [ ] Did I run focused `xtask` tests and report exact commands/results?
- [ ] Did I store an ICM completion memory before the final response?

## Expected Outcome

After implementation:

- If the user approves `git status` with the session option, Neo may skip a later identical `git status` in the same cwd.
- Neo must still prompt for `git log --oneline -20`, `git diff --stat`, `git branch --show-current && git remote -v`, or any other different Bash command.
- File write/edit session approvals are limited to the visible file path and operation.
- Prompts without a safe reusable scope do not show a session approval option.
- The TUI clearly communicates the saved scope instead of implying a global permission.
