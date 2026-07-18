# Canonical Approval Protocol Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use a no-commit adaptation of `aegis:subagent-driven-development`. Dispatch fresh implementers only in the dependency waves below, then run a spec-compliance review and a code-quality review for every task before unlocking dependents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every index-, label-, boolean-, and side-channel-based approval path with one runtime-owned typed request/response protocol covering ordinary tools, Shell, PlanMode, GoalMode, persistence, replay, and TUI interaction.

**Architecture:** `neo-agent-core` owns immutable `ApprovalRequest` values whose ordered options contain complete typed actions and presentation. `neo-agent` atomically transports one request with one response channel, while `neo-tui` renders and selects those exact option objects without reconstructing them. Runtime resolution validates membership, carries Plan/Goal execution metadata directly, and emits durable requested/resolved events.

**Tech Stack:** Rust 2024 workspace, serde/schemars, tokio oneshot and mpsc channels, JSONL sessions, Neo runtime permission pipeline, Neo transcript and chrome components, cargo-nextest.

## Global Constraints

- Follow `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md` exactly.
- Runtime construction is the only source of approval option availability, order, labels, action payloads, and presentation.
- Labels are presentation-only; no semantic branch may inspect label text.
- A selected index is UI state only; no runtime semantic branch may inspect or reconstruct meaning from it.
- `ApprovalResponse` is validated against the exact originating `ApprovalRequest` before any action runs.
- PlanMode and GoalMode use the shared transport but retain distinct typed actions and post-approval state transitions.
- Ordinary Tool/Shell approvals never offer revision feedback.
- Ask and Yolo review PlanMode and GoalMode transitions; Auto skips those dialogs.
- No compatibility adapter, alias, fallback builder, legacy event decoder, or dual approval path may remain.
- Do not add dependencies.
- Preserve Windows, Linux, and macOS behavior; use `Path`/`PathBuf` and no shell-specific implementation shortcuts.
- Preserve unrelated dirty-worktree changes. Never revert files to make tests pass.
- Implementation and review agents must not run `git add`, `git commit`, `git checkout`, `git restore`, `git reset`, `git stash`, `git clean`, `git rebase`, `git rm`, or any other Git mutation.
- Do not commit after individual tasks. Leave all reviewed implementation changes uncommitted for the user's final review with Codex.
- Every task requires both a spec-compliance verdict and a code-quality verdict. Critical or Important findings must be fixed and re-reviewed before dependent tasks start.
- Verification must use one package, one target selector, and a narrow test-name filter per test command. Do not use broad workspace tests as evidence.
- Tests must protect behavior, not derived traits, field round-trips, or library behavior.
- Update English and Chinese user documentation together.

## Frozen Interfaces

The implementation must use these names and field types verbatim. A necessary
change is a plan conflict: stop dependent work and escalate it to the user.

```rust
// crates/neo-agent-core/src/approval.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanSelection {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalPresentation {
    Command { title: String, command: String, cwd: Option<PathBuf> },
    Tool { title: String, details: Vec<String> },
    Plan {
        title: String,
        path: Option<PathBuf>,
        markdown: String,
        summary: Option<String>,
    },
    Goal {
        title: String,
        objective: String,
        completion_criterion: Option<String>,
        phases: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalAction {
    PermitOnce,
    PermitForSession { scope: SessionApprovalScope },
    PermitForPrefix { rule: PrefixApprovalRule },
    Reject,
    ApprovePlan { selection: Option<PlanSelection> },
    RevisePlan { preset_feedback: Option<String> },
    RejectPlan,
    StartGoal,
    ReviseGoal { preset_feedback: Option<String> },
    RejectGoal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalOption {
    pub label: String,
    pub description: Option<String>,
    pub action: ApprovalAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequest {
    pub turn: u32,
    pub id: String,
    pub operation: PermissionOperation,
    pub presentation: ApprovalPresentation,
    pub options: Vec<ApprovalOption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalCancelReason { Escape, Interrupt, SessionEnded }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalResponse {
    Selected {
        request_id: String,
        action: ApprovalAction,
        feedback: Option<String>,
    },
    Cancelled {
        request_id: String,
        reason: ApprovalCancelReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalResolution {
    Selected {
        action: ApprovalAction,
        label: String,
        feedback: Option<String>,
    },
    Cancelled { reason: ApprovalCancelReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalProtocolError {
    RequestIdMismatch,
    ActionNotOffered,
    FeedbackRequired,
    UnexpectedFeedback,
}

impl ApprovalRequest {
    pub fn validate_response(
        &self,
        response: &ApprovalResponse,
    ) -> Result<ApprovalResolution, ApprovalProtocolError>;
}

// crates/neo-agent-core/src/events.rs
AgentEvent::ApprovalRequested { request: ApprovalRequest }
AgentEvent::ApprovalResolved {
    turn: u32,
    request_id: String,
    resolution: ApprovalResolution,
}

// crates/neo-agent/src/modes/run/mod.rs
pub struct PendingApproval {
    pub request: ApprovalRequest,
    pub response_tx: oneshot::Sender<ApprovalResponse>,
}
```

The runtime-only execution context is also frozen:

```rust
// crates/neo-agent-core/src/runtime/tool_dispatch.rs
pub(super) enum ApprovalExecutionContext {
    Plan { selection: Option<PlanSelection> },
    Goal,
}

pub(super) struct PreparedToolCall {
    pub(super) result: PreparedToolCallResult,
    pub(super) access: ToolAccess,
    pub(super) approval: Option<ApprovalExecutionContext>,
}
```

## File And Ownership Map

| Area | Owned files | Responsibility |
| --- | --- | --- |
| Protocol | `neo-agent-core/src/approval.rs`, `lib.rs` | Frozen request, presentation, option, action, response, resolution, validation |
| Core permission runtime | `neo-agent-core/src/{permissions.rs,events.rs}`, `runtime/{config.rs,permission.rs,tool_dispatch.rs}` | Build requests once, validate response, apply grants, emit events |
| Plan/Goal runtime | `neo-agent-core/src/tools/{plan_mode.rs,goal.rs}`, `runtime/plan_orchestration.rs`, `runtime/tool_dispatch.rs` | Prevalidation, typed Plan/Goal outcomes, direct execution metadata |
| TUI approval model | `neo-tui/src/shell/{approval.rs,input_dispatch.rs,event_router.rs,mod.rs}` | Queue immutable requests, selection, feedback editor, response construction |
| TUI transcript | `neo-tui/src/transcript/{approval_data.rs,event_handler.rs,pane.rs,store.rs}`, `transcript/entry/mod.rs` | Render canonical presentation/options and requested/resolved states |
| App transport | `neo-agent/src/modes/run/{mod.rs,runtime/agent.rs}`, `modes/interactive/{approval.rs,input.rs,mod.rs,controller_factory.rs,turn.rs,sessions.rs}` | Atomic PendingApproval transport, cancellation, controller routing, remove maps |
| Persistence/replay | `neo-agent-core/src/session/event_persistence.rs`, `neo-agent-core/tests/session_jsonl.rs`, interactive replay and TUI event handlers | Requested/resolved JSONL and non-interactive replay |
| Documentation | paired `docs/{en,zh}/configuration/permissions.md`, `guides/{interaction,plan-mode,goals}.md` | Dynamic options and correct Plan/Goal review behavior |

## Dependency DAG And Parallel Waves

```text
Wave 1
Task 1: canonical protocol types and validation
   |
   +-------------------------------+
   |                               |
Wave 2                          Wave 2
Task 2: core permission runtime    Task 3: TUI canonical request model
   |                               |
   +---------------+---------------+
                   |
          +--------+--------+
          |                 |
Wave 3    Task 4            Task 5
          Plan/Goal core     atomic app transport
          semantics          and original regression
          |                 |
          +--------+--------+
                   |
Wave 4            Task 6: durable resolution and replay
                   |
Wave 5            Task 7: docs, residue deletion, final verification
```

Tasks in the same wave may run concurrently only when their owned file sets do
not overlap. A worker needing a file owned by another running task must stop and
ask the coordinator to serialize the work. The coordinator must not resolve a
conflict by allowing both workers to edit the same file.

Wave 2 and Wave 3 are lock-step interface waves: workers implement concurrently
against **Frozen Interfaces**, but package tests and reviews begin only after
every implementer in that wave has stopped writing. This prevents a TUI or app
test from compiling against a half-written `neo-agent-core` interface while
still preserving parallel implementation.

## No-Commit Review Protocol

The standard subagent-driven workflow is modified as follows:

1. Record `git status --short` before each wave.
2. Give each implementer only its task brief, global constraints, and exact
   owned file list.
3. The implementer writes a report under
   `docs/aegis/work/2026-07-17-canonical-approval-protocol/task-N-report.md` containing changed paths, exact test
   commands, outputs, self-review, and concerns.
4. Do not commit. Generate the review package with
   `git diff -U10 -- <task-owned-paths>` and save it under
   `docs/aegis/work/2026-07-17-canonical-approval-protocol/task-N.diff`.
5. Dispatch a spec-compliance reviewer with the spec, task brief, report, and
   diff paths. Require an explicit PASS/FAIL verdict.
6. After spec PASS, dispatch a different code-quality reviewer with the same
   artifacts. Require APPROVED or findings classified Critical/Important/Minor.
7. Fix every Critical or Important finding, rerun the covering exact tests, and
   repeat both reviews if behavior changed.
8. Mark the task complete in `docs/aegis/work/2026-07-17-canonical-approval-protocol/20-checkpoint.md` only after both
   reviews pass. Do not stage or commit.
9. Minor findings remain in the progress ledger for the final whole-diff review.

---

### Task 1: Add The Canonical Protocol And Trust-Boundary Validation

**Files:**
- Create: `crates/neo-agent-core/src/approval.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`

**Interfaces:**
- Produces every type and signature in **Frozen Interfaces** from
  `approval.rs`.
- Consumes existing `PermissionOperation`, `PrefixApprovalRule`, and
  `SessionApprovalScope` without changing their matching behavior.
- Does not modify runtime, event, agent, or TUI consumers.

- [ ] **Step 1: Add a failing validation test for membership and feedback.**

Place this test module in `approval.rs` while adding the type declarations but
before implementing `validate_response`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::PermissionOperation;

    fn plan_request() -> ApprovalRequest {
        ApprovalRequest {
            turn: 1,
            id: "approval-1".to_owned(),
            operation: PermissionOperation::PlanTransition,
            presentation: ApprovalPresentation::Plan {
                title: "Plan Review".to_owned(),
                path: None,
                markdown: "# Plan".to_owned(),
                summary: None,
            },
            options: vec![
                ApprovalOption {
                    label: "Approve".to_owned(),
                    description: None,
                    action: ApprovalAction::ApprovePlan { selection: None },
                },
                ApprovalOption {
                    label: "Reject with feedback".to_owned(),
                    description: None,
                    action: ApprovalAction::RevisePlan {
                        preset_feedback: None,
                    },
                },
            ],
        }
    }

    #[test]
    fn validate_response_enforces_request_action_and_feedback_contract() {
        let request = plan_request();
        let wrong_request = ApprovalResponse::Selected {
            request_id: "other".to_owned(),
            action: ApprovalAction::ApprovePlan { selection: None },
            feedback: None,
        };
        assert_eq!(
            request.validate_response(&wrong_request),
            Err(ApprovalProtocolError::RequestIdMismatch)
        );

        let unoffered = ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        };
        assert_eq!(
            request.validate_response(&unoffered),
            Err(ApprovalProtocolError::ActionNotOffered)
        );

        let blank = ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::RevisePlan {
                preset_feedback: None,
            },
            feedback: Some("  ".to_owned()),
        };
        assert_eq!(
            request.validate_response(&blank),
            Err(ApprovalProtocolError::FeedbackRequired)
        );

        let unexpected = ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::ApprovePlan { selection: None },
            feedback: Some("not allowed".to_owned()),
        };
        assert_eq!(
            request.validate_response(&unexpected),
            Err(ApprovalProtocolError::UnexpectedFeedback)
        );

        let approved = ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::ApprovePlan { selection: None },
            feedback: None,
        };
        assert_eq!(
            request.validate_response(&approved),
            Ok(ApprovalResolution::Selected {
                action: ApprovalAction::ApprovePlan { selection: None },
                label: "Approve".to_owned(),
                feedback: None,
            })
        );
    }
}
```

- [ ] **Step 2: Run the exact test and confirm RED.**

```bash
cargo test --package neo-agent-core --lib approval::tests::validate_response_enforces_request_action_and_feedback_contract -- --exact --nocapture
```

Expected: compilation fails because `validate_response` has no implementation.

- [ ] **Step 3: Implement the frozen protocol and validation.**

Use the exact derives and serde attributes from **Frozen Interfaces**. Implement
validation with this control flow:

```rust
impl ApprovalRequest {
    pub fn validate_response(
        &self,
        response: &ApprovalResponse,
    ) -> Result<ApprovalResolution, ApprovalProtocolError> {
        match response {
            ApprovalResponse::Cancelled { request_id, reason } => {
                if request_id != &self.id {
                    return Err(ApprovalProtocolError::RequestIdMismatch);
                }
                Ok(ApprovalResolution::Cancelled { reason: *reason })
            }
            ApprovalResponse::Selected {
                request_id,
                action,
                feedback,
            } => {
                if request_id != &self.id {
                    return Err(ApprovalProtocolError::RequestIdMismatch);
                }
                let option = self
                    .options
                    .iter()
                    .find(|option| &option.action == action)
                    .ok_or(ApprovalProtocolError::ActionNotOffered)?;
                let revises = matches!(
                    action,
                    ApprovalAction::RevisePlan { .. } | ApprovalAction::ReviseGoal { .. }
                );
                if !revises && feedback.is_some() {
                    return Err(ApprovalProtocolError::UnexpectedFeedback);
                }
                let feedback = feedback
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                if revises && feedback.is_none() {
                    return Err(ApprovalProtocolError::FeedbackRequired);
                }
                Ok(ApprovalResolution::Selected {
                    action: action.clone(),
                    label: option.label.clone(),
                    feedback,
                })
            }
        }
    }
}
```

- [ ] **Step 4: Export the module and run the exact test GREEN.**

Add `mod approval;` and public re-exports in `lib.rs`. Run the exact command from
Step 2.

Expected: one test passes.

- [ ] **Step 5: Self-review and pass both task review gates without committing.**

The spec reviewer must confirm exact type names, derives, serde tags, membership
validation, revision validation, and absence of runtime/TUI behavior. The
quality reviewer must confirm no redundant trait tests or speculative helper
abstractions.

---

### Task 2: Make Core Permission Resolution Produce And Consume Typed Requests

**Files:**
- Modify: `crates/neo-agent-core/src/permissions.rs`
- Modify: `crates/neo-agent-core/src/events.rs`
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/runtime/permission.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

**Interfaces:**
- Consumes the complete Task 1 protocol.
- Produces `ApprovalHandler` and `AsyncApprovalHandler` returning
  `ApprovalResponse`.
- Produces ordinary Tool/Shell `ApprovalRequest` construction and typed grant
  resolution.
- Produces `AgentEvent::ApprovalRequested` and `ApprovalResolved` frozen shapes.
- Produces `PreparedToolCall.approval`, initialized to `None` for ordinary
  grants.
- Leaves Plan/Goal request construction and action resolution for Task 4.

- [ ] **Step 1: Add the failing ordinary-request contract test.**

Add one table-driven integration test named
`approval_requests_only_offer_runtime_supported_actions` in `runtime_turn.rs`.
Drive foreground Bash, background Bash, and scoped Write through the existing
`FakeHarness`, stop at `ApprovalRequested`, and assert these action vectors:

```rust
assert_eq!(
    background_request
        .options
        .iter()
        .map(|option| &option.action)
        .collect::<Vec<_>>(),
    vec![&ApprovalAction::PermitOnce, &ApprovalAction::Reject],
);

assert!(matches!(
    foreground_request.options.as_slice(),
    [
        ApprovalOption { action: ApprovalAction::PermitOnce, .. },
        ApprovalOption { action: ApprovalAction::PermitForSession { .. }, .. },
        ApprovalOption { action: ApprovalAction::PermitForPrefix { .. }, .. },
        ApprovalOption { action: ApprovalAction::Reject, .. },
    ]
));

assert!(matches!(
    write_request.options.as_slice(),
    [
        ApprovalOption { action: ApprovalAction::PermitOnce, .. },
        ApprovalOption { action: ApprovalAction::PermitForSession { .. }, .. },
        ApprovalOption { action: ApprovalAction::Reject, .. },
    ]
));
```

- [ ] **Step 2: Run the exact test and confirm RED.**

```bash
cargo test --package neo-agent-core --test runtime_turn approval_requests_only_offer_runtime_supported_actions -- --exact --nocapture
```

Expected: compilation fails because `ApprovalRequested` still exposes flat
legacy fields and handlers still return `PermissionApprovalDecision`.

- [ ] **Step 3: Replace the handler and event contracts.**

In `runtime/config.rs`, replace both handler aliases:

```rust
pub type ApprovalHandler =
    Arc<dyn Fn(&ApprovalRequest) -> ApprovalResponse + Send + Sync>;
pub type AsyncApprovalHandler = Arc<
    dyn Fn(ApprovalRequest) -> BoxFuture<'static, ApprovalResponse> + Send + Sync,
>;
```

In `events.rs`, replace the flat request event and add the resolved event exactly
as frozen. Delete `PermissionApprovalDecision` from `permissions.rs` and from
`lib.rs` exports. Do not retain an adapter from decisions to actions.

Mechanically migrate every approval closure and event fixture in
`neo-agent-core` so the crate still compiles. Use this test helper in
`runtime_turn.rs` instead of repeating response construction:

```rust
fn select_action(request: &ApprovalRequest, action: ApprovalAction) -> ApprovalResponse {
    assert!(request.options.iter().any(|option| option.action == action));
    ApprovalResponse::Selected {
        request_id: request.id.clone(),
        action,
        feedback: None,
    }
}
```

Ordinary allow/deny fixtures select PermitOnce or Reject. Transition fixtures
may temporarily select the first offered action until Task 4 gives them their
final typed Plan/Goal assertions; do not introduce a production adapter.

- [ ] **Step 4: Build ordinary typed requests once in the permission layer.**

Add these private helpers in `runtime/permission.rs`:

```rust
fn ordinary_approval_options(
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
) -> Vec<ApprovalOption> {
    let mut options = vec![ApprovalOption {
        label: "Approve once".to_owned(),
        description: None,
        action: ApprovalAction::PermitOnce,
    }];
    if let Some(scope) = session_scope {
        options.push(ApprovalOption {
            label: scope.label.clone(),
            description: Some(scope.detail.clone()),
            action: ApprovalAction::PermitForSession { scope },
        });
    }
    if let Some(rule) = prefix_rule {
        options.push(ApprovalOption {
            label: format!("Approve commands starting with {}", rule.label),
            description: None,
            action: ApprovalAction::PermitForPrefix { rule },
        });
    }
    options.push(ApprovalOption {
        label: "Reject".to_owned(),
        description: None,
        action: ApprovalAction::Reject,
    });
    options
}
```

Construct `ApprovalPresentation::Command` for Shell and terminal operations and
`ApprovalPresentation::Tool` for all other ordinary operations. Preserve the
existing prompt title/details copy by moving it into the core builder; do not
leave a second copy in TUI.

- [ ] **Step 5: Validate and resolve ordinary responses.**

Change `resolve_approval` to construct one request, emit
`ApprovalRequested { request: request.clone() }`, await one response, call
`request.validate_response`, emit `ApprovalResolved`, then match the validated
action. Use this exhaustive shape:

```rust
match resolution {
    ApprovalResolution::Cancelled { .. } => Some(permission_error(
        operation,
        &subject,
        "approval cancelled",
    )),
    ApprovalResolution::Selected { action: ApprovalAction::PermitOnce, .. } => None,
    ApprovalResolution::Selected {
        action: ApprovalAction::PermitForSession { scope },
        ..
    } => {
        if let Ok(mut approved) = config.session_approvals.lock() {
            scope.record(&mut approved);
        }
        None
    }
    ApprovalResolution::Selected {
        action: ApprovalAction::PermitForPrefix { rule },
        ..
    } => persist_prefix_rule_or_error(config, rule),
    ApprovalResolution::Selected { action: ApprovalAction::Reject, .. } => {
        Some(permission_error(operation, &subject, "approval denied"))
    }
    ApprovalResolution::Selected { action, .. } => Some(ToolResult::error(format!(
        "approval action is invalid for {operation:?}: {action:?}"
    ))),
}
```

Extract only the existing prefix persistence/rollback block into
`persist_prefix_rule_or_error`; do not alter its storage semantics.

- [ ] **Step 6: Add the frozen execution-context field.**

Add `ApprovalExecutionContext` and `approval` to `PreparedToolCall`. Initialize
it to `None` in every constructor in `permission.rs`. Do not add Plan/Goal
branches yet.

- [ ] **Step 7: Run focused tests, self-review, and both task review gates.**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn approval_requests_only_offer_runtime_supported_actions -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn runtime_executes_ask_permission_tool_after_async_approval_wait_allows_it -- --exact --nocapture
```

Expected: both tests pass with typed responses. Reviewers must reject any
ordinary `Reject with feedback`, no-scope session option, semantic label parse,
or unavailable-action fallback.

---

### Task 3: Make TUI Render And Select The Canonical Request Object

**Files:**
- Modify: `crates/neo-tui/src/shell/approval.rs`
- Modify: `crates/neo-tui/src/shell/input_dispatch.rs`
- Modify: `crates/neo-tui/src/shell/event_router.rs`
- Modify: `crates/neo-tui/src/shell/mod.rs`
- Modify: `crates/neo-tui/src/transcript/approval_data.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/tests/app_shell.rs`
- Modify: `crates/neo-tui/tests/transcript_pane.rs`

**Interfaces:**
- Consumes Task 1 protocol only; this task does not depend on Task 2 runtime
  integration.
- Produces `NeoChromeState::push_approval(request: ApprovalRequest)`.
- Produces input handlers returning `Option<ApprovalResponse>`.
- Produces transcript requested/resolved rendering directly from canonical
  request and resolution values.
- Consumes the frozen AgentEvent shapes implemented by Task 2 in the same
  lock-step wave; do not wait for Task 2 source details or invent an adapter.
- Does not modify `neo-agent` controller code.

- [ ] **Step 1: Add a failing visible-option identity test.**

Replace index/choice-specific approval tests with one representative test named
`approval_selection_returns_the_visible_option_action`. Build a background Bash
request with only PermitOnce and Reject, push it to chrome and transcript, then:

```rust
fn background_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
        },
        options: vec![
            ApprovalOption {
                label: "Approve once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::Reject,
            },
        ],
    }
}

app.push_approval(background_request());
app.handle_pending_approval_input(InputEvent::Key(KeyId::new("down").unwrap()));

let rendered = render_app(80, &app).join("\n");
assert!(rendered.contains("2. Reject"), "frame: {rendered}");
assert_eq!(app.approval_selection().map(|(_, selected, ..)| selected), Some(1));

let response = app
    .handle_pending_approval_input(InputEvent::Key(KeyId::new("enter").unwrap()))
    .expect("Enter resolves visible Reject");
assert!(matches!(
    response,
    ApprovalResponse::Selected {
        action: ApprovalAction::Reject,
        feedback: None,
        ..
    }
));
```

- [ ] **Step 2: Run the exact test and confirm RED.**

```bash
cargo test --package neo-tui --test app_shell approval_selection_returns_the_visible_option_action -- --exact --nocapture
```

Expected: compilation fails because TUI still constructs `ApprovalChoice` and
`ApprovalResult` values.

- [ ] **Step 3: Replace the modal data model.**

Use one request inside `ApprovalRequestModal`:

```rust
pub struct ApprovalRequestModal {
    pub request: ApprovalRequest,
    pub selected: usize,
    pub feedback_input: String,
    collecting_feedback: bool,
}

impl ApprovalRequestModal {
    pub fn selected_option(&self) -> Option<&ApprovalOption> {
        self.request.options.get(self.selected)
    }

    pub fn selected_action(&self) -> Option<&ApprovalAction> {
        self.selected_option().map(|option| &option.action)
    }
}
```

Delete `ApprovalChoice`, `ApprovalOption` from neo-tui, `ApprovalModal`,
`ApprovalResult`, `plan_option_labels`, suggestions index arithmetic, and all
modal constructors that build semantic options.

Migrate every approval fixture in `neo-tui/tests/app_shell.rs` and
`neo-tui/tests/transcript_pane.rs` to construct a canonical `ApprovalRequest`.
Delete tests whose only purpose was asserting the removed coarse enum or a
fixed numeric meaning; retain behavioral coverage by asserting the selected
option's exact core `ApprovalAction`.

- [ ] **Step 4: Make response construction clone the selected action.**

Implement one confirmation path:

```rust
fn response_for_selected(modal: &ApprovalRequestModal) -> Option<ApprovalResponse> {
    let action = modal.selected_action()?.clone();
    let revises = matches!(
        action,
        ApprovalAction::RevisePlan { .. } | ApprovalAction::ReviseGoal { .. }
    );
    if revises {
        let feedback = modal.feedback_input.trim();
        if !modal.collecting_feedback || feedback.is_empty() {
            return None;
        }
        return Some(ApprovalResponse::Selected {
            request_id: modal.request.id.clone(),
            action,
            feedback: Some(feedback.to_owned()),
        });
    }
    Some(ApprovalResponse::Selected {
        request_id: modal.request.id.clone(),
        action,
        feedback: None,
    })
}
```

When a revision action is first confirmed, enter editing and initialize
`feedback_input` from its `preset_feedback`. Navigation alone must not enter
editing or change feedback.

- [ ] **Step 5: Replace fixed number mapping with option selection.**

Remove both fixed 1-through-4 match functions. In TUI input dispatch use:

```rust
let number = character.to_digit(10).map(|value| value as usize);
if let Some(number @ 1..=9) = number
    && number <= approval.request.options.len()
{
    approval.selected = number - 1;
    return self.confirm_or_edit_selected_approval();
}
```

Do not assign a semantic meaning to the number. Option 10 remains arrow-only.

- [ ] **Step 6: Render canonical presentation and options.**

Change approval transcript data to retain the complete `ApprovalRequest` and
mutable view state only:

```rust
pub enum ApprovalDisplayState {
    Pending,
    Resolved(ApprovalResolution),
    Abandoned,
}

pub struct ApprovalPromptData {
    pub request: ApprovalRequest,
    pub selected: usize,
    pub feedback_input: String,
    pub feedback_active: bool,
    pub state: ApprovalDisplayState,
}
```

Render titles/bodies from `ApprovalPresentation` and render options with:

```rust
for (index, option) in request.options.iter().enumerate() {
    render_option(index + 1, option.label.as_str(), index == selected);
}
```

`shell/event_router.rs` must not open a live modal from
`AgentEvent::ApprovalRequested`; it may update passive chrome status only.
`transcript/event_handler.rs` must upsert the request exactly as carried by the
event and must not append session or prefix options.

- [ ] **Step 7: Add revision-path identity coverage.**

Add one test that selects a preset Plan revision by arrow and by number in two
fresh app instances. Assert both enter editing with the same preset, allow an
edit, and return the same `RevisePlan` action plus trimmed feedback on Enter.

Run:

```bash
cargo test --package neo-tui --test app_shell approval_selection_returns_the_visible_option_action -- --exact --nocapture
cargo test --package neo-tui --test app_shell plan_revision_arrow_and_number_share_one_editor_path -- --exact --nocapture
```

Expected: both pass.

- [ ] **Step 8: Self-review and pass both task review gates without committing.**

Reviewers must compare request option count, rendered option count, selected
label, and returned action. Any raw-JSON option reconstruction, semantic label
parse, or second option vector is a blocking failure.

---

### Task 4: Preserve PlanMode And GoalMode Semantics Without Side Maps

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/runtime/permission.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify: `crates/neo-agent-core/src/runtime/plan_orchestration.rs`
- Modify: `crates/neo-agent-core/src/tools/plan_mode.rs`
- Modify: `crates/neo-agent-core/src/tools/goal.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

**Interfaces:**
- Consumes Tasks 1 and 2.
- Produces Plan and Goal typed request construction and resolution.
- Produces `prevalidate_exit_goal_mode(input: &Value) -> Result<(), ToolError>`.
- Produces direct Plan selection decoration through
  `PreparedToolCall.approval`.
- Deletes both `plan_review_*` maps from `AgentConfig`.

- [ ] **Step 1: Add failing Plan and Goal outcome tests.**

Migrate existing plan and goal runtime tests to return `ApprovalResponse` and
add these assertions:

```rust
assert!(matches!(
    plan_request.options.first().map(|option| &option.action),
    Some(ApprovalAction::ApprovePlan { selection: None })
));

assert!(matches!(
    selected_plan_result.details.as_ref(),
    Some(details) if details["plan_selected_label"] == "Safe path"
));

assert!(matches!(
    goal_request.options.as_slice(),
    [
        ApprovalOption { action: ApprovalAction::StartGoal, .. },
        ApprovalOption { action: ApprovalAction::RejectGoal, .. },
        ApprovalOption { action: ApprovalAction::ReviseGoal { .. }, .. },
    ]
));
```

Add exact tests named:

- `exit_plan_mode_generic_approval_has_no_selected_approach`
- `exit_plan_mode_typed_selection_reaches_tool_result`
- `exit_goal_mode_reject_and_revise_create_no_goal`

- [ ] **Step 2: Run the three exact tests and confirm RED.**

```bash
cargo test --package neo-agent-core --test runtime_turn exit_plan_mode_generic_approval_has_no_selected_approach -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn exit_plan_mode_typed_selection_reaches_tool_result -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn exit_goal_mode_reject_and_revise_create_no_goal -- --exact --nocapture
```

Expected: failures because transition requests and outcomes still use generic
permission decisions and side maps.

- [ ] **Step 3: Build Plan requests from validated typed input.**

After `prevalidate_exit_plan_mode`, parse `ExitPlanModeInput` once and construct
options in the exact spec order. Alternatives become:

```rust
ApprovalOption {
    label: format!("Approach: {}", option.label),
    description: option.description.clone(),
    action: ApprovalAction::ApprovePlan {
        selection: Some(PlanSelection {
            label: option.label.clone(),
            description: option.description.clone(),
        }),
    },
}
```

With no alternatives, add only `ApprovePlan { selection: None }`. Suggestions
become `RevisePlan { preset_feedback }`, followed by `RejectPlan` and manual
`RevisePlan`. Build `ApprovalPresentation::Plan` from the active plan file and
summary.

- [ ] **Step 4: Build Goal requests from prevalidated typed input.**

Expose:

```rust
pub fn prevalidate_exit_goal_mode(input: &Value) -> Result<(), ToolError> {
    let args = serde_json::from_value::<ExitGoalModeArgs>(input.clone()).map_err(|error| {
        ToolError::InvalidInput {
            tool: "ExitGoalMode".to_owned(),
            message: error.to_string(),
        }
    })?;
    let invalid = args.objective.trim().is_empty()
        || args
            .completion_criterion
            .as_deref()
            .is_some_and(|criterion| criterion.trim().is_empty())
        || args.phases.iter().any(|phase| phase.trim().is_empty());
    if invalid {
        return Err(ToolError::InvalidInput {
            tool: "ExitGoalMode".to_owned(),
            message: "objective, present completion criterion, and every phase must be non-empty"
                .to_owned(),
        });
    }
    Ok(())
}
```

Call it before emitting approval. Parse the same immutable arguments into
`ApprovalPresentation::Goal`, and add exactly StartGoal, RejectGoal, and
ReviseGoal actions.

- [ ] **Step 5: Carry accepted Plan selection directly through tool dispatch.**

When resolving `ApprovePlan`, return a runnable `PreparedToolCall` whose
`approval` is:

```rust
Some(ApprovalExecutionContext::Plan { selection })
```

Decorate the matching ExitPlanMode result directly before mode-exit events:

```rust
if let Some(PlanSelection { label, .. }) = selection {
    result.content = format!(
        "Selected approach: {label}\n\
         Execute ONLY the selected approach. Do not execute any unselected alternatives.\n\n{}",
        result.content
    );
    result
        .details
        .get_or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .expect("tool result details object")
        .insert("plan_selected_label".to_owned(), label.into());
}
```

Preserve existing plan content/path details. Remove lookup by tool id.

- [ ] **Step 6: Resolve reject and revise transitions without executing tools.**

Return non-executing results directly:

```rust
ApprovalAction::RejectPlan => permission_error(
    PermissionOperation::PlanTransition,
    "Exit plan mode",
    "approval denied",
),
ApprovalAction::RevisePlan { .. } => ToolResult::ok(format!(
    "User requested revisions. Plan mode remains active.\n\nFeedback: {feedback}"
)),
ApprovalAction::RejectGoal => permission_error(
    PermissionOperation::GoalTransition,
    "Start reviewed goal",
    "approval denied",
),
ApprovalAction::ReviseGoal { .. } => ToolResult::ok(format!(
    "User requested revisions. Goal mode remains active.\n\nFeedback: {feedback}"
)),
```

Only `StartGoal` executes `ExitGoalMode`. Preserve GoalStarted emission and the
existing no-inline-spin behavior.

- [ ] **Step 7: Delete global side maps and run focused tests.**

Delete `plan_review_feedback` and `plan_review_selected_label` from
`AgentConfig`, initialization, controller factory inputs, and
`plan_orchestration.rs`. Delete the map-consumption logic rather than leaving it
unused.

Run the three commands from Step 2 plus:

```bash
cargo test --package neo-agent-core --test runtime_turn exit_goal_mode_starts_goal_and_ends_run_without_spinning -- --exact --nocapture
```

Expected: all four pass.

- [ ] **Step 8: Self-review and pass both task review gates without committing.**

Reviewers must verify Plan generic approval has no fabricated selection, Plan
continues inline, Goal ends the run, reject/revise keep the respective authoring
mode, and no global map or tool-id side channel remains.

---

### Task 5: Replace App Transport With One Atomic PendingApproval

**Files:**
- Modify: `crates/neo-agent/src/modes/run/mod.rs`
- Modify: `crates/neo-agent/src/modes/run/runtime/agent.rs`
- Modify: `crates/neo-agent/src/modes/interactive/approval.rs`
- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/controller_factory.rs`
- Modify: `crates/neo-agent/src/modes/interactive/turn.rs`
- Modify: `crates/neo-agent/src/modes/interactive/sessions.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

**Interfaces:**
- Consumes Tasks 1, 2, and 3.
- Produces frozen `PendingApproval` and one channel carrying one
  `ApprovalResponse`.
- Opens live modal state only from `PendingApproval`; approval events remain
  observer/persistence input.
- Deletes every pre-resolution and side-channel map in `neo-agent`.

- [ ] **Step 1: Add the exact original regression through the controller.**

Add
`background_bash_one_down_submits_the_visible_reject_action` to interactive
tests. Send a `PendingApproval` whose request has PermitOnce and Reject, then:

```rust
controller.register_pending_approval(pending);
controller
    .handle_input_event(InputEvent::Key(KeyId::new("down").unwrap()))
    .await?;
assert!(controller.render_snapshot().contains("> 2. Reject"));
controller
    .handle_input_event(InputEvent::Key(KeyId::new("enter").unwrap()))
    .await?;
assert!(matches!(
    response_rx.await.unwrap(),
    ApprovalResponse::Selected {
        action: ApprovalAction::Reject,
        ..
    }
));
```

- [ ] **Step 2: Run the exact test and confirm RED.**

```bash
cargo test --package neo-agent --bin neo modes::interactive::tests::background_bash_one_down_submits_the_visible_reject_action -- --exact --nocapture
```

Expected: compilation fails because `PendingApproval` and typed response routing
are not integrated.

- [ ] **Step 3: Replace PromptApprovalRequest and the triple channel.**

Define only the frozen `PendingApproval` in `modes/run/mod.rs`. In
`attach_async_approval_handler`, use one channel:

```rust
agent_config.with_async_approval_handler(move |request| {
    let approval_tx = approval_tx.clone();
    async move {
        let request_id = request.id.clone();
        let (response_tx, response_rx) = oneshot::channel();
        if approval_tx
            .send(PendingApproval { request, response_tx })
            .is_err()
        {
            return ApprovalResponse::Cancelled {
                request_id,
                reason: ApprovalCancelReason::SessionEnded,
            };
        }
        response_rx.await.unwrap_or(ApprovalResponse::Cancelled {
            request_id,
            reason: ApprovalCancelReason::SessionEnded,
        })
    }
})
```

Delete decision, feedback, selected-label channels and all map writes.

Mechanically migrate all `PromptApprovalRequest`, `PendingApprovalResponse`,
`PermissionApprovalDecision`, and flat `ApprovalRequested` fixtures in
`modes/interactive/tests.rs` in this task. Use request-builder helpers grouped
by ordinary, Plan, and Goal request; do not repeat option-building logic inside
individual tests.

- [ ] **Step 4: Make controller registration atomic.**

`register_pending_approval` must first place the request and responder together
in the controller queue, then call `chrome.push_approval(request.clone())` and
upsert the transcript request. `AgentEvent::ApprovalRequested` must not open or
resolve a live request.

Delete `resolved_approvals`; a response cannot occur before registration because
the UI is unavailable until `PendingApproval` arrives.

- [ ] **Step 5: Route selected and cancelled responses without semantic mapping.**

Replace `resolve_approval(&ApprovalResult)` with:

```rust
fn resolve_approval(&mut self, response: ApprovalResponse) {
    let request_id = match &response {
        ApprovalResponse::Selected { request_id, .. }
        | ApprovalResponse::Cancelled { request_id, .. } => request_id.clone(),
    };
    if let Some(pending) = self.pending_approvals.remove(&request_id) {
        let _ = pending.response_tx.send(response);
    }
}
```

Escape, interrupt, and session teardown construct explicit Cancelled responses
with the correct reason.

- [ ] **Step 6: Remove legacy controller state and plan feedback forwarding.**

Delete `PendingApprovalResponse`, `approval_decision`, `approval_feedback`,
`resolved_approvals`, `pending_plan_review_feedback`, and controller-factory
plumbing for `plan_review_feedback`. Turn requests no longer carry approval
feedback maps; feedback is already in the runtime's atomic response.

- [ ] **Step 7: Run focused controller tests.**

Run:

```bash
cargo test --package neo-agent --bin neo modes::interactive::tests::background_bash_one_down_submits_the_visible_reject_action -- --exact --nocapture
cargo test --package neo-agent --bin neo modes::interactive::tests::approval_interrupt_cancels_all_pending_approvals -- --exact --nocapture
cargo test --package neo-agent --bin neo modes::interactive::tests::revise_exit_plan_mode_feedback_is_forwarded_with_current_approval -- --exact --nocapture
```

Rename or replace the existing interrupt test so its exact name and assertion
use `Cancelled { reason: Interrupt }`. Expected: all three pass.

- [ ] **Step 8: Self-review and pass both task review gates without committing.**

Reviewers must prove one request owns one responder, no event-before-channel
compensation remains, cancellation reasons are accurate, and controller code
does not map labels/indices/actions to another semantic type.

---

### Task 6: Persist Resolution And Replay Approval Cards Without Reopening Them

**Files:**
- Modify: `crates/neo-agent-core/src/session/event_persistence.rs`
- Modify: `crates/neo-agent-core/tests/session_jsonl.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Modify: `crates/neo-tui/src/transcript/store.rs`
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/tests/transcript_pane.rs`

**Interfaces:**
- Consumes all preceding tasks.
- Persists both frozen event variants.
- Produces resolved and abandoned historical approval cards.
- Replay never creates `PendingApproval` or installs grants.

- [ ] **Step 1: Add a failing JSONL request/resolution round-trip test.**

Replace the old flat approval fixtures in `session_jsonl.rs` and add:

```rust
fn background_bash_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
        },
        options: vec![
            ApprovalOption {
                label: "Approve once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::Reject,
            },
        ],
    }
}

#[tokio::test]
async fn jsonl_session_round_trips_requested_and_resolved_approval() {
    let request = background_bash_request();
    let requested = AgentEvent::ApprovalRequested {
        request: request.clone(),
    };
    let resolved = AgentEvent::ApprovalResolved {
        turn: 1,
        request_id: request.id.clone(),
        resolution: ApprovalResolution::Selected {
            action: ApprovalAction::Reject,
            label: "Reject".to_owned(),
            feedback: None,
        },
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path).await.expect("create");
    writer.append(&requested).await.expect("append request");
    writer.append(&resolved).await.expect("append resolution");
    writer.flush().await.expect("flush");
    assert_eq!(
        JsonlSessionReader::read_all(&path).await.expect("read"),
        vec![requested, resolved]
    );
}
```

- [ ] **Step 2: Run the exact test and confirm RED.**

```bash
cargo test --package neo-agent-core --test session_jsonl jsonl_session_round_trips_requested_and_resolved_approval -- --exact --nocapture
```

Expected: failure until all event fixtures and persistence matches use the new
variants.

- [ ] **Step 3: Persist both events through the normal path.**

Keep requested and resolved events in the default durable branch of
`SessionEventPersistence`. Update all exhaustive matches and test fixtures. Do
not add an old flat-event decoder or serde alias.

- [ ] **Step 4: Resolve transcript entries by request id.**

On `ApprovalRequested`, insert one entry containing the canonical request. On
`ApprovalResolved`, mutate that same entry to store the resolution and remove
interactive feedback state. Rendering must use the event's canonical label and
action, not re-derive them.

If replay reaches the end with an unresolved request, render it as `Abandoned`.
Do not enqueue it in chrome.

- [ ] **Step 5: Add replay behavior tests.**

Replace `replay_session_into_transcript_discards_historical_pending_approvals`
with two exact tests:

```rust
fn replay_background_bash_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
        },
        options: vec![
            ApprovalOption {
                label: "Approve once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::Reject,
            },
        ],
    }
}

#[test]
fn replay_renders_resolved_approval_without_reopening_it() {
    let request = replay_background_bash_request();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new())
        .with_events([
            AgentEvent::ApprovalRequested {
                request: request.clone(),
            },
            AgentEvent::ApprovalResolved {
                turn: 1,
                request_id: request.id.clone(),
                resolution: ApprovalResolution::Selected {
                    action: ApprovalAction::Reject,
                    label: "Reject".to_owned(),
                    feedback: None,
                },
            },
        ]);
    let mut transcript = TranscriptPane::new(80, 12);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript.render_frame(80, 12).unwrap().join("\n");
    assert!(rendered.contains("Rejected"), "frame: {rendered}");
    assert!(transcript.transcript().entries().iter().all(|entry| {
        !matches!(
            entry,
            TranscriptEntry::ApprovalPrompt(data)
                if matches!(data.state, ApprovalDisplayState::Pending)
        )
    }));
}

#[test]
fn replay_marks_unresolved_approval_abandoned_without_reopening_it() {
    let request = replay_background_bash_request();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new())
        .with_events([AgentEvent::ApprovalRequested { request }]);
    let mut transcript = TranscriptPane::new(80, 12);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript.render_frame(80, 12).unwrap().join("\n");
    assert!(rendered.contains("Abandoned"), "frame: {rendered}");
    assert!(transcript.transcript().entries().iter().all(|entry| {
        !matches!(
            entry,
            TranscriptEntry::ApprovalPrompt(data)
                if matches!(data.state, ApprovalDisplayState::Pending)
        )
    }));
}
```

Assert transcript text contains the resolved/abandoned state and
`chrome.approval_is_pending()` is false.

- [ ] **Step 6: Run focused persistence and replay tests.**

```bash
cargo test --package neo-agent-core --test session_jsonl jsonl_session_round_trips_requested_and_resolved_approval -- --exact --nocapture
cargo test --package neo-agent --bin neo modes::interactive::tests::replay_renders_resolved_approval_without_reopening_it -- --exact --nocapture
cargo test --package neo-agent --bin neo modes::interactive::tests::replay_marks_unresolved_approval_abandoned_without_reopening_it -- --exact --nocapture
cargo test --package neo-tui --test transcript_pane approval_resolution_updates_the_matching_inline_card -- --exact --nocapture
```

Expected: all four pass.

- [ ] **Step 7: Self-review and pass both task review gates without committing.**

Reviewers must verify events remain transcript-only, replay never creates a live
responder, requested/resolved ordering is stable, and no legacy event decoder
exists.

---

### Task 7: Delete Legacy Residue, Align Documentation, And Run Final Review

**Files:**
- Modify: `docs/en/configuration/permissions.md`
- Modify: `docs/zh/configuration/permissions.md`
- Modify: `docs/en/guides/interaction.md`
- Modify: `docs/zh/guides/interaction.md`
- Modify: `docs/en/guides/plan-mode.md`
- Modify: `docs/zh/guides/plan-mode.md`
- Modify: `docs/en/guides/goals.md`
- Modify: `docs/zh/guides/goals.md`
- Modify: any production/test file still containing a canonical-deletion symbol
  found by the exact residue scan below

**Interfaces:**
- Consumes all preceding tasks.
- Produces no new production abstraction.
- Produces paired user documentation and a residue-free final worktree diff.
- Leaves all implementation changes uncommitted for the user's final review.

- [ ] **Step 1: Update English and Chinese behavior together.**

Document these exact facts in both languages:

- approval options are dynamic and include only actions the runtime can honor;
- ordinary Tool/Shell approval offers Approve once, optional session/prefix
  grants, and Reject, but no revision feedback;
- background Bash offers no reusable grant;
- Plan Reject and Revise keep PlanMode active; Approve exits and continues;
- Goal review displays objective, completion criterion, and phases; Reject and
  Revise create no goal;
- Ask/Yolo show Plan/Goal review and Auto skips it;
- remove references to the deleted Rust type `PermissionApprovalDecision`.

- [ ] **Step 2: Run the canonical residue scan and delete every hit.**

```bash
rg -n 'PermissionApprovalDecision|ApprovalChoice|ApprovalResult|picked_prefix|selected_option_label|plan_option_labels|PromptApprovalRequest|PendingApprovalResponse|decision_tx|feedback_tx|selected_label_tx|resolved_approvals|plan_review_feedback|plan_review_selected_label|Approve commands starting with.*starts_with' crates
```

Expected: exit code 1 with no matches. Do not suppress a legitimate hit with an
allow attribute or comment; delete the obsolete path.

- [ ] **Step 3: Run the focused final behavioral matrix sequentially.**

```bash
cargo test --package neo-agent-core --lib approval::tests::validate_response_enforces_request_action_and_feedback_contract -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn approval_requests_only_offer_runtime_supported_actions -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn exit_plan_mode_typed_selection_reaches_tool_result -- --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn exit_goal_mode_reject_and_revise_create_no_goal -- --exact --nocapture
cargo test --package neo-agent-core --test session_jsonl jsonl_session_round_trips_requested_and_resolved_approval -- --exact --nocapture
cargo test --package neo-tui --test app_shell approval_selection_returns_the_visible_option_action -- --exact --nocapture
cargo test --package neo-tui --test app_shell plan_revision_arrow_and_number_share_one_editor_path -- --exact --nocapture
cargo test --package neo-agent --bin neo modes::interactive::tests::background_bash_one_down_submits_the_visible_reject_action -- --exact --nocapture
cargo test --package neo-agent --bin neo modes::interactive::tests::replay_renders_resolved_approval_without_reopening_it -- --exact --nocapture
```

Expected: every exact test passes.

- [ ] **Step 4: Run formatting and diff checks.**

```bash
cargo fmt --all --check
git diff --check
```

Expected: both exit 0. If `cargo fmt --all --check` reports unrelated dirty
files, run `rustfmt --check --edition 2024` only on touched Rust files and report
the workspace-wide blocker instead of formatting unrelated work.

- [ ] **Step 5: Run Task 7's two review gates.**

The spec reviewer compares every acceptance criterion in the design spec with a
task report or exact test result. The quality reviewer checks deletion quality,
cross-platform behavior, test value, documentation parity, and absence of
duplicate sources of truth.

- [ ] **Step 6: Run one final whole-diff review and stop without committing.**

Give the final reviewer:

- the design spec;
- this plan;
- `docs/aegis/work/2026-07-17-canonical-approval-protocol/20-checkpoint.md`;
- every task report and unresolved Minor finding;
- `git status --short`;
- `git diff --stat`;
- `git diff -U10` for the complete implementation diff.

Fix Critical and Important findings in one coordinated fix wave, rerun covering
tests, and re-review. When the final review is clean, stop. Do not stage or
commit implementation code. Report the exact changed paths, tests, review
verdicts, residue result, and remaining Minor findings so the user can return
the worktree to Codex for final review.

## Executor Prompt

Use this prompt verbatim in the implementation session:

```text
Implement Neo's canonical approval protocol in /Users/chenyuanhao/Workspace/neo.

Read these files completely before changing code:
1. /Users/chenyuanhao/Workspace/neo/AGENTS.md
2. /Users/chenyuanhao/Workspace/neo/docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md
3. /Users/chenyuanhao/Workspace/neo/docs/aegis/plans/2026-07-17-canonical-approval-protocol.md

Use ICM recall before work. Use CodeGraph/cx before grep for code discovery. Follow the plan's frozen interfaces, global constraints, dependency DAG, exact tests, file ownership, and no-commit review protocol.

Coordinate implementation with fresh task agents. Parallelize only tasks shown in the same dependency wave and only when their owned file sets do not overlap. Every task must receive: implementation self-review, an independent spec-compliance review, and an independent code-quality review. Fix Critical and Important findings and re-review before unlocking dependents.

Do not run any Git mutation. In particular, do not add, commit, checkout, restore, reset, stash, clean, rebase, rm, or push. Do not revert unrelated dirty-worktree changes. Do not commit after tasks or after final verification. Leave the full reviewed implementation uncommitted so the user can return it to Codex for final review.

Do not preserve the old approval path. Delete every legacy type, side channel, label/index inference, fallback option builder, and compatibility shape listed in the spec. If a frozen interface or task boundary is impossible, stop dependent work and ask the user instead of inventing a compatibility layer.

Continue through all tasks without routine check-ins. Stop only for a genuine blocker, a plan conflict requiring user authority, or completion of all task reviews and the final whole-diff review. At the end, report changed files, exact test outputs, per-task review verdicts, final review verdict, residue scan result, and remaining Minor findings. Do not commit.
```
