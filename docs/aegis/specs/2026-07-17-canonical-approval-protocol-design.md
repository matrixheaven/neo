# Canonical Approval Protocol Design

**Status:** Approved for implementation planning

**Date:** 2026-07-17

## Summary

Neo will replace its current approval pipeline with one runtime-owned, typed
approval protocol. The runtime constructs the complete ordered option list
once. The interactive controller, chrome, transcript, persistence layer, and
runtime resolver all carry that same request and selected action without
reconstructing semantics from a display index, label text, boolean flag, raw
tool arguments, or side-channel map.

This is a canonical-only replacement. The implementation must delete the old
approval model rather than retain adapters, aliases, fallback option builders,
or dual event shapes.

## Root Cause

The reported background Bash failure was not user error.

1. `bash_approval_scope` returns no session scope and no prefix rule for a
   background Bash call.
2. Chrome therefore constructed three real options: Approve once, Reject, and
   Reject with feedback.
3. The transcript independently added a synthetic Approve for this session
   option so that it could preserve a four-option layout.
4. Arrow navigation changed Chrome's selected index and synchronized only that
   bare index to the transcript.
5. One Down displayed the transcript's synthetic session option while Chrome's
   option at the same index was Reject. Enter submitted Reject.

The same structural defect already exists elsewhere:

- session and prefix grants share `ApprovalChoice::AlwaysApprove` and are
  distinguished by parsing an English label;
- plan alternatives share `ApprovalChoice::Approve` and recover the selected
  approach from a parallel label vector and index;
- plan suggestions recover feedback from another index calculation;
- decision, feedback, and selected plan label travel through three independent
  one-shot channels and two global maps;
- PlanMode and GoalMode renderers independently reconstruct presentation and
  options from raw JSON;
- requested approvals are persisted, but resolutions are not.

Fixing only the background Bash fallback would leave every other path exposed
to the same class of drift.

## Goals

- Make the runtime the only owner of approval option availability and order.
- Make every option carry its complete typed action.
- Make UI labels presentation-only and semantically inert.
- Make arrow, number, and Enter input select the same option object the user
  sees.
- Carry selection and feedback through one atomic response.
- Cover ordinary tool permissions, reusable session grants, persistent prefix
  grants, PlanMode review, and GoalMode review with one protocol.
- Preserve the distinct post-approval behavior of PlanMode and GoalMode.
- Persist both request and resolution for reliable transcript replay.
- Reject invalid or impossible responses instead of silently degrading them to
  Allow once.
- Remove every legacy approval DTO, inference path, and compensating race map.

## Non-Goals

- No new permission mode.
- No new hosted or remote approval service.
- No generic form/dialog framework.
- No reusable approvals for background Bash or transition reviews.
- No provider-facing protocol change.
- No compatibility decoder, adapter, or alias for the old approval event or
  response shapes.
- No change to the existing session and prefix matching algorithms.
- No new GoalMode alternative or suggestion feature.

## Canonical Types

The public protocol lives in `neo-agent-core`, in a dedicated
`crates/neo-agent-core/src/approval.rs` module. Permission matching remains in
`permissions.rs`; UI-only state remains in `neo-tui`.

```rust
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{PermissionOperation, PrefixApprovalRule, SessionApprovalScope};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanSelection {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalPresentation {
    Command {
        title: String,
        command: String,
        cwd: Option<PathBuf>,
    },
    Tool {
        title: String,
        details: Vec<String>,
    },
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
    PermitForSession {
        scope: SessionApprovalScope,
    },
    PermitForPrefix {
        rule: PrefixApprovalRule,
    },
    Reject,
    ApprovePlan {
        selection: Option<PlanSelection>,
    },
    RevisePlan {
        preset_feedback: Option<String>,
    },
    RejectPlan,
    StartGoal,
    ReviseGoal {
        preset_feedback: Option<String>,
    },
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
pub enum ApprovalCancelReason {
    Escape,
    Interrupt,
    SessionEnded,
}

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
    Cancelled {
        reason: ApprovalCancelReason,
    },
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
```

`ApprovalResponse` returns the selected action object rather than a list index
or label. The runtime must still validate exact membership against the
originating request before acting on it. This prevents a UI or future transport
from manufacturing a session scope or prefix rule that was not offered.

`validate_response` is the only response-validation entry point. It matches the
request id, finds the exact offered action, enforces revision feedback, rejects
feedback on non-revision actions, and recovers the canonical display label for
`ApprovalResolution`.

## Request Construction

The core request builder owns all option labels, descriptions, action payloads,
and ordering. No consumer may append, remove, reorder, rename, or reconstruct
options.

### Ordinary Tool And Shell Requests

Options are ordered as follows:

1. `PermitOnce` labeled `Approve once`.
2. `PermitForSession` when and only when a non-empty session scope exists.
3. `PermitForPrefix` when and only when a valid prefix rule exists.
4. `Reject` labeled `Reject`.

Ordinary Tool, Shell, Write, and Edit requests do not offer revision feedback.
Background Bash, dangerous commands, and any other scope-ineligible request
therefore show only actions the runtime can actually honor.

### PlanMode Review

Plan review options are ordered as follows:

1. One `ApprovePlan { selection: Some(...) }` action per validated model
   alternative, preserving input order.
2. If no alternatives exist, one generic
   `ApprovePlan { selection: None }` action labeled `Approve`.
3. One `RevisePlan { preset_feedback: Some(...) }` action per validated
   suggestion, preserving input order.
4. `RejectPlan` labeled `Reject`.
5. `RevisePlan { preset_feedback: None }` labeled `Reject with feedback`.

The generic approve action must not manufacture a plan selection named
`Approve`. A selected alternative carries both its label and description in
`PlanSelection`.

### GoalMode Review

Goal review options are exactly:

1. `StartGoal` labeled `Approve`.
2. `RejectGoal` labeled `Reject`.
3. `ReviseGoal { preset_feedback: None }` labeled `Reject with feedback`.

Goal review never exposes session or prefix grants. The presentation contains
the immutable objective, completion criterion, and phases that will be used if
the goal starts.

## Presentation

The runtime builds typed presentation data together with the options:

- Command presentation carries the exact command and resolved cwd.
- Tool presentation carries the final title and display-safe details.
- Plan presentation carries the path, markdown, and optional summary.
- Goal presentation carries the objective, completion criterion, and phases.

Chrome and transcript render this typed presentation. They must not infer a
title, details, options, suggestions, plan selection, or goal payload from raw
tool arguments.

## Live Transport And State Ownership

The app-level transport becomes one atomic value:

```rust
pub struct PendingApproval {
    pub request: ApprovalRequest,
    pub response_tx: tokio::sync::oneshot::Sender<ApprovalResponse>,
}
```

The async approval handler sends `PendingApproval` before the user can interact
with the request, then awaits exactly one `ApprovalResponse`.

`AgentEvent::ApprovalRequested` remains an observable and durable event, but it
does not independently create a live modal. The interactive controller opens
the live approval from `PendingApproval`, using the same immutable request for
the chrome queue and transcript entry. This removes the event-before-channel
pre-resolution race.

The chrome owns only mutable interaction state:

- active request position in the queue;
- selected option index into that request's immutable option vector;
- whether feedback editing is active;
- current feedback text.

The transcript receives the same request and current view state. Synchronizing
a selected index is safe only because both surfaces hold the same request
object and neither can rebuild its option list.

The transcript entry uses one explicit state instead of optional fields whose
combinations need interpretation:

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

## Input Semantics

- Up and Down move within the canonical option vector.
- Enter submits the selected non-revision action immediately.
- Number keys 1 through 9 select the correspondingly displayed option object;
  they do not encode an approval meaning. Options beyond 9 remain reachable by
  arrows and Enter.
- Selecting a manual revision action enters feedback editing without resolving
  the request.
- Selecting a preset revision action enters the same editor with its preset
  text. Arrow and number paths behave identically.
- Navigation alone never enters feedback editing and never mutates feedback.
- Revision submission requires `feedback.trim()` to be non-empty. The runtime
  repeats this validation at the trust boundary.
- Escape produces `Cancelled { reason: Escape }`.
- Interrupt rejects all active and queued requests with
  `Cancelled { reason: Interrupt }` and keeps existing turn-cancellation
  behavior.
- Session teardown resolves remaining responders with
  `Cancelled { reason: SessionEnded }`.

## Runtime Resolution

The runtime validates the response request id and exact action membership
before applying it.

- `PermitOnce` runs the tool once.
- `PermitForSession` records its embedded scope and runs the tool.
- `PermitForPrefix` validates and persists its embedded rule, then runs the
  tool. Persistence failure fails the tool and rolls back the rule.
- A reusable-grant action without its typed payload cannot exist. There is no
  fallback to Permit once.
- `Reject` returns the existing permission-denied tool result.
- `ApprovePlan` runs `ExitPlanMode` with a typed execution context carrying the
  optional plan selection. The selected approach is attached directly to that
  tool result; no global map is used. Successful PlanMode exit continues the
  model loop.
- `RejectPlan` does not execute `ExitPlanMode` and leaves PlanMode active.
- `RevisePlan` returns the existing non-terminating revision result with exact
  feedback and leaves PlanMode active.
- `StartGoal` executes the immutable reviewed payload, creates the durable goal,
  emits `GoalStarted`, switches Pending to Active, and ends the current run.
- `RejectGoal` and `ReviseGoal` do not create a goal and leave GoalMode authoring
  pending. Revision returns exact feedback to the model.
- Cancelled responses never execute the tool or install a grant.

Plan and Goal inputs are schema- and semantic-validated before the request is
shown. Goal validation rejects a blank objective, a present but blank
completion criterion, and any blank phase; an empty phase list remains valid.
Approval grants permission to execute the reviewed payload; it does not hide an
unrelated execution failure that occurs after approval.

## Tool Execution Metadata

`PreparedToolCall` gains typed approval execution context instead of consulting
global maps after execution:

```rust
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

The tool dispatch path decorates the corresponding result before mode-exit
events run. `plan_review_feedback` and `plan_review_selected_label` are deleted
from `AgentConfig`.

## Events, Persistence, And Replay

The event contract becomes:

```rust
AgentEvent::ApprovalRequested {
    request: ApprovalRequest,
}

AgentEvent::ApprovalResolved {
    turn: u32,
    request_id: String,
    resolution: ApprovalResolution,
}
```

Both events are persisted through normal session event persistence.

Replay rules:

- requested followed by resolved renders one historical resolved approval card;
- a requested event without a resolution is rendered as abandoned and never
  reopened;
- replay never recreates a response channel or reusable permission grant;
- approval events remain transcript state and do not become model messages.

The old flat `ApprovalRequested` event is removed. There is no legacy decoder,
alias, or runtime fallback for that shape.

## Validation And Failure Behavior

- A request with no options is an internal error and must not open a dialog.
- A response for the wrong request id is rejected.
- An action not present in the request is rejected.
- Empty or whitespace-only revision feedback is rejected without closing the
  editor.
- A closed response channel becomes SessionEnded cancellation, not a fabricated
  Reject selection.
- Prefix rules continue to reject empty or approve-all prefixes.
- Invalid PlanMode or GoalMode input is reported before approval UI appears.

## Canonical Deletions

The implementation must remove all of the following before final review:

- `PermissionApprovalDecision`;
- `ApprovalChoice`;
- `ApprovalResult`;
- `picked_prefix`;
- `selected_option_label`;
- `plan_option_labels` and index-based plan selection recovery;
- label-prefix parsing for persistent grants;
- duplicated approval-number parsers and fixed 1-through-4 semantics;
- `PromptApprovalRequest`;
- `PendingApprovalResponse`;
- `decision_tx`, `feedback_tx`, and `selected_label_tx`;
- `resolved_approvals`;
- `plan_review_feedback` and `plan_review_selected_label` maps;
- transcript fallback insertion of session options;
- Chrome and transcript raw-JSON option reconstruction;
- generic Tool/Shell `Reject with feedback` options.

No old and new path may coexist in the completed implementation.

## Test Contract

The implementation requires the following high-signal coverage:

1. A table-driven core request test covers foreground Bash, background Bash,
   dangerous Bash, scoped and prefix Bash, Write, Edit, generic Tool, Plan with
   zero and multiple alternatives, Plan suggestions, and Goal.
2. The original regression test performs one Down and Enter on background Bash
   and proves the visible second option and returned action are both Reject.
3. A TUI invariant test iterates every option in representative requests and
   proves rendered label, arrow selection, number selection when available, and
   returned action all come from the same option object.
4. Plan tests prove generic approval has no selection, selected alternatives
   preserve label and description, preset and manual revision share editor
   semantics, Reject and Revise keep PlanMode active, and approval continues the
   model loop.
5. Goal tests prove the complete reviewed payload is rendered, StartGoal creates
   the durable goal and emits GoalStarted, Reject and Revise create no goal, and
   revision feedback returns to the model.
6. Transport tests prove request and responder registration are atomic and no
   pre-resolution selection payload can be lost.
7. Runtime trust-boundary tests reject mismatched request ids, unavailable
   actions, and blank revision feedback.
8. Session tests round-trip requested and resolved events and prove replay never
   reopens historical requests.
9. A residue scan proves every canonical deletion is absent.

Tests must follow the repository's narrow exact-target policy. No task may use a
broad workspace test command as completion evidence.

## Documentation

Update English and Chinese documentation together:

- permission options are dynamic and reflect only actions the runtime can honor;
- ordinary approvals have no revision-feedback option;
- Plan Reject and Revise leave PlanMode active;
- Goal approval reviews objective, completion criterion, and phases;
- Auto skips PlanMode and GoalMode review while Ask and Yolo show it;
- remove references to the deleted `PermissionApprovalDecision` type.

## Acceptance Criteria

- The background Bash reproduction cannot display an action different from the
  action submitted by one Down and Enter.
- No approval semantics are recovered from index, label, boolean discriminator,
  raw arguments, or a parallel vector.
- PlanMode and GoalMode use the same request/response transport without sharing
  ambiguous action variants.
- All selection payload and feedback travel atomically.
- Requested and resolved states survive replay without reopening a prompt.
- All canonical deletions are complete.
- Focused tests, formatting checks for touched Rust files, documentation parity,
  and `git diff --check` pass.
