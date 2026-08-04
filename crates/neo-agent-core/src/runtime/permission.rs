use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use super::config::AgentConfig;
use super::events::EventPublisher;
use super::plan_orchestration::exit_plan_mode_has_reviewable_plan;
use super::tool_arguments::{ApprovalExecutionContext, PreparedExecution, PreparedToolCall};
use super::tool_dispatch::{ask_user_runs_in_background, cancelled_tool_result};
use crate::approval::{EditApprovalPresentation, WriteApprovalPresentation};
use crate::permissions::{
    ApprovalRuleStore, PrefixApprovalRule, SessionApprovalKey, SessionApprovalScope,
    ThemeDraftAction, command_might_be_dangerous, is_known_safe_command,
};
use crate::tools::plan_mode::{
    ExitPlanModeInput, ExitPlanModeOption, ExitPlanModeSuggestion, prevalidate_exit_plan_mode,
};
use crate::tools::{ExitGoalModeArgs, prevalidate_exit_goal_mode};
use crate::tools::{PreparedEdit, PreparedWrite};
use crate::workspace_policy::normalize_path;
use crate::{
    AgentEvent, AgentToolCall, ApprovalAction, ApprovalCancelReason, ApprovalOption,
    ApprovalPresentation, ApprovalRequest, ApprovalResolution, PermissionMode, PermissionOperation,
    PlanModeGuard, PlanSelection, ToolAccess, ToolResult, check_plan_mode_guard,
};

#[derive(Debug)]
pub(super) enum PermissionPreparation {
    Run(ToolAccess),
    Ask {
        operation: PermissionOperation,
        subject: String,
        session_scope: Option<SessionApprovalScope>,
        prefix_rule: Option<PrefixApprovalRule>,
    },
    Deny(String),
    Terminal(ToolResult),
}

/// The outcome of resolving one [`PermissionPreparation`]: run with an
/// access grant (and optional Plan/Goal execution context), or finish with a
/// terminal result (denial/cancellation/revision feedback).
pub(super) enum PermissionResolution {
    Run {
        access: ToolAccess,
        approval: Option<ApprovalExecutionContext>,
    },
    Terminal {
        result: ToolResult,
        permission_decision: Option<PermissionTerminalDecision>,
    },
}

/// Resolve one permission preparation into an execution decision. Approval
/// dialogs emit `ApprovalRequested` and await the configured handler; this
/// runs during the batch authorization phase, after instruction preflight
/// and before the frozen fingerprint recheck.
pub(super) async fn resolve_permission_preparation(
    config: &AgentConfig,
    preparation: PermissionPreparation,
    tool_call: &AgentToolCall,
    prepared_call: &PreparedToolCall,
    turn: u32,
    emitter: &mut impl EventPublisher,
    cancel_token: &CancellationToken,
) -> PermissionResolution {
    match preparation {
        PermissionPreparation::Run(access) => PermissionResolution::Run {
            access,
            approval: None,
        },
        PermissionPreparation::Deny(message) => PermissionResolution::Terminal {
            result: ToolResult::error(message),
            permission_decision: Some(PermissionTerminalDecision::Denied),
        },
        PermissionPreparation::Terminal(result) => PermissionResolution::Terminal {
            result,
            permission_decision: None,
        },
        PermissionPreparation::Ask {
            operation,
            subject,
            session_scope,
            prefix_rule,
        } => {
            match resolve_approval(
                config,
                ApprovalInput {
                    turn,
                    tool_call,
                    prepared_call,
                    operation,
                    subject,
                    session_scope,
                    prefix_rule,
                },
                emitter,
                cancel_token,
            )
            .await
            {
                AppliedApproval::Terminal {
                    result,
                    permission_decision,
                } => PermissionResolution::Terminal {
                    result,
                    permission_decision,
                },
                AppliedApproval::Allow { approval } => PermissionResolution::Run {
                    access: access_for_tool(tool_call, true),
                    approval,
                },
            }
        }
    }
}

pub(super) fn permission_preparation_for_mode(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    prepared_call: &PreparedToolCall,
) -> PermissionPreparation {
    let arguments = &prepared_call.arguments;
    // Read live permission state once. The TUI may switch this mid-turn via
    // `/ask`, `/auto`, `/yolo`, or `/permissions`; every branch below must use
    // this `mode` instead of the static `config.permission_mode` snapshot.
    let mode = current_permission_mode(config);

    // 1. Plan-mode hard guard.
    if let Some(prep) = check_plan_guard(config, tool_call, prepared_call) {
        return prep;
    }

    // Workflow actions route through normal permission semantics. The
    // canonical parser owns the action matrix: read/validate actions run
    // directly, save and run actions open typed reviews in Ask mode, and plan
    // mode denies mutations. No capability, nonce, or hidden authorization.
    if tool_call.name.as_ref() == "Workflow" {
        return workflow_permission_preparation(config, tool_call, prepared_call, mode);
    }

    // ThemeDraft routes through its own action-aware preparation: preview is a
    // non-mutating tool action that never needs a write approval, while save is
    // a special host-owned mutation with a one-time Ask approval and no
    // session-wide grant. This branch returns before any cached ordinary
    // tool/session approval is consulted, so a cached "approve this tool for
    // the session" grant can never authorize a later ThemeDraft save.
    if tool_call.name.as_ref() == "ThemeDraft" {
        return theme_draft_permission_preparation(config, tool_call, prepared_call, mode);
    }

    // 2-5. Mode/tool-specific early returns (auto mode, EnterPlanMode, background AskUser).
    if let Some(prep) = check_mode_early_returns(tool_call, arguments, mode) {
        return prep;
    }

    // 6. Derive the reusable scope + prefix rule for the ask fallback.
    let (session_scope, prefix_rule) =
        approval_scope_for_prepared_call(config, tool_call, prepared_call);

    // 7-8. Cached approvals (persistent prefix rules + session approvals).
    if let Some(prep) = check_cached_approvals(config, tool_call, arguments, session_scope.as_ref())
    {
        return prep;
    }

    // 9-10. Transition tools (ExitPlanMode, ExitGoalMode).
    if let Some(prep) = check_transition_tools(config, tool_call, arguments, mode) {
        return prep;
    }

    // 10. Plan-mode helper approvals (e.g. writing the active plan file).
    if let Some(prep) = check_plan_file_write(config, tool_call, prepared_call) {
        return prep;
    }

    // 11-13. Yolo mode, safe commands, and default-approved tools.
    if let Some(prep) = check_safe_or_prompt(config, tool_call, arguments, mode) {
        return prep;
    }

    // 14. Ask fallback prompt.
    let (operation, subject) = permission_operation_for_tool(tool_call, arguments)
        .unwrap_or((PermissionOperation::Tool, tool_call.name.to_string()));
    PermissionPreparation::Ask {
        operation,
        subject,
        session_scope,
        prefix_rule,
    }
}

/// Action-aware Workflow preparation. Parses once through the canonical
/// action parser, then applies the permission matrix from the approved
/// contract: read/validate run directly everywhere, save/run open typed
/// reviews in Ask mode, and plan mode denies save/run only.
fn workflow_permission_preparation(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    prepared_call: &PreparedToolCall,
    mode: PermissionMode,
) -> PermissionPreparation {
    // Extract the already-parsed action from the prepared execution. This is the
    // single parse that flows from validation through approval to execution.
    let PreparedExecution::Workflow(prepared) = &prepared_call.execution else {
        unreachable!("Workflow calls are prepared before permission evaluation");
    };
    dispatch_workflow_permission(config, tool_call, prepared, mode)
}

/// Action-aware `ThemeDraft` preparation. Branches on the typed wire `action`
/// (`preview` vs `save`), never on presentation labels:
///
/// - `preview` runs directly in every mode, including plan mode: it is
///   non-mutating and only fills the bounded in-memory draft store.
/// - `save` is denied in plan mode, opens a one-time `ThemeSave` approval in
///   Ask mode with no session scope (only "Approve once" is offered), and runs
///   directly in Auto/Yolo, matching the existing permission-mode contract.
/// - An unparseable action runs the tool so its own strict input validation
///   surfaces the typed error; no side effect can occur because `execute`
///   validates before acting.
fn theme_draft_permission_preparation(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    prepared_call: &PreparedToolCall,
    mode: PermissionMode,
) -> PermissionPreparation {
    let action = prepared_call
        .arguments
        .get("action")
        .and_then(|value| serde_json::from_value::<ThemeDraftAction>(value.clone()).ok());
    match action {
        Some(ThemeDraftAction::Preview) => {
            PermissionPreparation::Run(access_for_tool(tool_call, true))
        }
        Some(ThemeDraftAction::Save) => {
            let plan_active = config
                .plan_mode
                .read()
                .ok()
                .is_some_and(|plan_mode| plan_mode.is_active());
            if plan_active {
                return PermissionPreparation::Deny(
                    "blocked by plan mode: ThemeDraft save is not allowed while planning"
                        .to_owned(),
                );
            }
            let subject = prepared_call
                .arguments
                .get("draft_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(tool_call.name.as_ref())
                .to_owned();
            if mode == PermissionMode::Ask {
                PermissionPreparation::Ask {
                    operation: PermissionOperation::ThemeSave,
                    subject,
                    session_scope: None,
                    prefix_rule: None,
                }
            } else {
                PermissionPreparation::Run(access_for_tool(tool_call, true))
            }
        }
        None => PermissionPreparation::Run(access_for_tool(tool_call, true)),
    }
}

fn dispatch_workflow_permission(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    prepared: &crate::tools::workflow::PreparedWorkflowAction,
    mode: PermissionMode,
) -> PermissionPreparation {
    use crate::tools::workflow::PreparedWorkflowAction;
    let plan_active = config
        .plan_mode
        .read()
        .ok()
        .is_some_and(|plan_mode| plan_mode.is_active());
    match &prepared {
        PreparedWorkflowAction::List { .. }
        | PreparedWorkflowAction::Show { .. }
        | PreparedWorkflowAction::ValidateInline { .. }
        | PreparedWorkflowAction::ValidateSaved { .. } => {
            PermissionPreparation::Run(access_for_tool(tool_call, true))
        }
        PreparedWorkflowAction::Save { request, .. } => {
            if plan_active {
                return PermissionPreparation::Deny(
                    "blocked by plan mode: Workflow save is not allowed while planning".to_owned(),
                );
            }
            if let Err(result) = workflow_save_preflight(config, request) {
                return PermissionPreparation::Terminal(result);
            }
            if mode == PermissionMode::Ask {
                PermissionPreparation::Ask {
                    operation: PermissionOperation::WorkflowSave,
                    subject: format!("Save workflow `{}`", request.name),
                    session_scope: None,
                    prefix_rule: None,
                }
            } else {
                PermissionPreparation::Run(access_for_tool(tool_call, true))
            }
        }
        PreparedWorkflowAction::RunInline { definition, args } => {
            if plan_active {
                return PermissionPreparation::Deny(
                    "blocked by plan mode: Workflow run is not allowed while planning".to_owned(),
                );
            }
            match crate::workflow::resolve_dynamic_definition(
                definition.clone(),
                &config.workflow_runtime.limits(),
            ) {
                Ok(resolved) => workflow_run_preparation(
                    config,
                    tool_call,
                    &resolved,
                    args.clone(),
                    crate::tools::workflow::WorkflowAction::RunInline,
                    mode,
                ),
                Err(error) => {
                    PermissionPreparation::Terminal(crate::tools::workflow::preflight_error_result(
                        crate::tools::workflow::WorkflowAction::RunInline,
                        error,
                    ))
                }
            }
        }
        PreparedWorkflowAction::RunSaved { name, args } => {
            if plan_active {
                return PermissionPreparation::Deny(
                    "blocked by plan mode: Workflow run is not allowed while planning".to_owned(),
                );
            }
            match config.workflow_definitions.resolve(name) {
                Ok(resolved) => workflow_run_preparation(
                    config,
                    tool_call,
                    &resolved,
                    args.clone(),
                    crate::tools::workflow::WorkflowAction::RunSaved,
                    mode,
                ),
                Err(error) => {
                    PermissionPreparation::Terminal(crate::tools::workflow::preflight_error_result(
                        crate::tools::workflow::WorkflowAction::RunSaved,
                        error,
                    ))
                }
            }
        }
    }
}

fn workflow_workspace_identity(config: &AgentConfig) -> String {
    config
        .workspace_root
        .as_deref()
        .map_or_else(|| ".".to_owned(), |root| root.display().to_string())
}

/// Zero-effect definition validation for `save` before an Ask review opens.
fn workflow_save_preflight(
    config: &AgentConfig,
    request: &crate::workflow::WorkflowSaveRequest,
) -> Result<(), ToolResult> {
    let definition = crate::workflow::resolve_dynamic_definition(
        crate::workflow::DynamicWorkflowDefinitionInput {
            name: request.name.clone(),
            display_name: Some(request.display_name.clone()),
            description: request.description.clone(),
            phases: request.phases.clone(),
            script: request.lua_source.clone(),
            input_schema: request.input_schema.clone(),
            output_schema: request.output_schema.clone(),
        },
        &config.workflow_runtime.limits(),
    )
    .map_err(|error| {
        crate::tools::workflow::preflight_error_result(
            crate::tools::workflow::WorkflowAction::Save,
            error,
        )
    })?;
    crate::tools::workflow::preflight_run(
        &definition,
        serde_json::json!({}),
        crate::tools::workflow::WorkflowAction::Save,
        workflow_workspace_identity(config),
        current_permission_mode(config),
        &config.workflow_runtime,
        false,
    )
    .map_err(|error| {
        crate::tools::workflow::preflight_error_result(
            crate::tools::workflow::WorkflowAction::Save,
            error,
        )
    })
}

/// Zero-effect launch preflight plus the Ask/Auto/Yolo decision for run
/// actions. Invalid definitions or arguments finish terminally before any
/// approval dialog opens.
fn workflow_run_preparation(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    definition: &crate::workflow::ResolvedWorkflowDefinition,
    args: serde_json::Value,
    action: crate::tools::workflow::WorkflowAction,
    mode: PermissionMode,
) -> PermissionPreparation {
    if let Err(error) = crate::tools::workflow::preflight_run(
        definition,
        args,
        action,
        workflow_workspace_identity(config),
        mode,
        &config.workflow_runtime,
        true,
    ) {
        return PermissionPreparation::Terminal(crate::tools::workflow::preflight_error_result(
            action, error,
        ));
    }
    if mode == PermissionMode::Ask {
        PermissionPreparation::Ask {
            operation: PermissionOperation::WorkflowLaunch,
            subject: format!("Launch workflow `{}`", definition.name.as_str()),
            session_scope: None,
            prefix_rule: None,
        }
    } else {
        PermissionPreparation::Run(access_for_tool(tool_call, true))
    }
}

/// Section 1 — plan-mode hard guard. Returns `Some(Deny)` if the plan-mode guard
/// rejects the call, or `None` to continue the pipeline.
fn check_plan_guard(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    prepared_call: &PreparedToolCall,
) -> Option<PermissionPreparation> {
    let plan_mode = config.plan_mode.read().ok()?;
    if plan_mode.is_active() {
        let targets_match_plan_file = match &prepared_call.execution {
            PreparedExecution::Edit(edit) => plan_mode
                .plan_file_path()
                .is_some_and(|path| edit.all_resolved_targets_match(path)),
            PreparedExecution::Write(write) => plan_mode
                .plan_file_path()
                .is_some_and(|path| write.all_resolved_targets_match(path)),
            PreparedExecution::Direct | PreparedExecution::Workflow(_) => false,
        };
        if targets_match_plan_file {
            return None;
        }
        match check_plan_mode_guard(&plan_mode, &tool_call.name) {
            PlanModeGuard::Allow => {}
            PlanModeGuard::Deny { message } => {
                return Some(PermissionPreparation::Deny(message));
            }
        }
    }
    None
}

/// Sections 2-5 — mode/tool-specific early returns: auto-mode `AskUserQuestion`
/// deny, background `AskUserQuestion` run, auto-mode approves-all, `EnterPlanMode`
/// auto-approve.
fn check_mode_early_returns(
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
    mode: PermissionMode,
) -> Option<PermissionPreparation> {
    // 2. Auto mode hard deny for AskUserQuestion.
    if tool_call.name.as_ref() == "AskUserQuestion" && mode == PermissionMode::Auto {
        return Some(PermissionPreparation::Deny(
            "AskUserQuestion is disabled while auto permission mode is active".to_owned(),
        ));
    }

    // 3. Background AskUserQuestion does not need an approval dialog in any mode.
    if tool_call.name.as_ref() == "AskUserQuestion" && ask_user_runs_in_background(arguments) {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    // 4. Auto mode approves everything else.
    if mode == PermissionMode::Auto {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    // 5. EnterPlanMode is auto-approved in all modes.
    if tool_call.name.as_ref() == "EnterPlanMode" {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    None
}

/// Sections 7-8 — cached approvals: persistent prefix rules (layer 2) and
/// session approvals (layer 1). Returns `Some(Run)` when the call matches a
/// cached approval.
fn check_cached_approvals(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
    session_scope: Option<&SessionApprovalScope>,
) -> Option<PermissionPreparation> {
    // Layer 2 — persistent prefix rules (loaded from disk).
    if let Some(argv) = shell_argv_for_prefix_check(config, tool_call, arguments)
        && config
            .prefix_approval_rules
            .lock()
            .ok()
            .is_some_and(|store| store.matches(&argv))
    {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    // Layer 1 — session approvals scoped by exact canonical command + cwd
    // (or exact file path + operation).
    if let Some(scope) = session_scope
        && config
            .session_approvals
            .lock()
            .ok()
            .is_some_and(|set| scope.is_approved(&set))
    {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    None
}

/// Sections 9-10 — transition tools: `ExitPlanMode` and `ExitGoalMode`. These
/// transitions must never become session-scoped wildcards.
fn check_transition_tools(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
    mode: PermissionMode,
) -> Option<PermissionPreparation> {
    if tool_call.name.as_ref() == "ExitPlanMode" {
        // Pre-validate before showing the approval dialog. Without this,
        // invalid input (e.g. a reserved label like "Approve") would still
        // pop the dialog, and if the user approved it the tool would then
        // fail with InvalidInput — showing both "approval: Approved" and
        // the error simultaneously. By validating here we skip the dialog
        // and let execute() return the error directly to the model.
        if exit_plan_mode_has_reviewable_plan(config)
            && prevalidate_exit_plan_mode(arguments).is_ok()
        {
            return Some(PermissionPreparation::Ask {
                operation: PermissionOperation::PlanTransition,
                subject: "Exit plan mode".to_owned(),
                session_scope: None,
                prefix_rule: None,
            });
        }
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    if tool_call.name.as_ref() == "ExitGoalMode" {
        if mode == PermissionMode::Auto {
            return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
        }
        // Skip the review dialog when the payload can never succeed.
        if prevalidate_exit_goal_mode(arguments).is_ok() {
            return Some(PermissionPreparation::Ask {
                operation: PermissionOperation::GoalTransition,
                subject: "Start reviewed goal".to_owned(),
                session_scope: None,
                prefix_rule: None,
            });
        }
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    None
}

/// Section 10 — plan-mode helper approvals (Write/Edit to the active plan file).
fn check_plan_file_write(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    prepared_call: &PreparedToolCall,
) -> Option<PermissionPreparation> {
    if !matches!(tool_call.name.as_ref(), "Write" | "Edit") {
        return None;
    }
    let plan_mode = config.plan_mode.read().ok()?;
    if !plan_mode.is_active() {
        return None;
    }
    let allowed = match &prepared_call.execution {
        PreparedExecution::Edit(edit) => plan_mode
            .plan_file_path()
            .is_some_and(|path| edit.all_resolved_targets_match(path)),
        PreparedExecution::Write(write) => plan_mode
            .plan_file_path()
            .is_some_and(|path| write.all_resolved_targets_match(path)),
        PreparedExecution::Direct | PreparedExecution::Workflow(_) => false,
    };
    if allowed {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }
    None
}

/// Sections 11-13 — yolo mode approves-all, dangerous-command force-prompt,
/// known-safe commands, and default-approved tools.
fn check_safe_or_prompt(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
    mode: PermissionMode,
) -> Option<PermissionPreparation> {
    // 11. Yolo mode approves all remaining tools.
    if mode == PermissionMode::Yolo {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    // 12. Read-only safe commands skip the prompt in ask mode. Dangerous
    //     commands bypass this and force a prompt.
    if let Some(argv) = shell_argv_for_prefix_check(config, tool_call, arguments) {
        if command_might_be_dangerous(&argv) {
            let (operation, subject) = permission_operation_for_tool(tool_call, arguments)
                .unwrap_or((PermissionOperation::Tool, tool_call.name.to_string()));
            return Some(PermissionPreparation::Ask {
                operation,
                subject,
                session_scope: None,
                prefix_rule: None,
            });
        }
        if is_known_safe_command(&argv) {
            return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
        }
    }

    // 13. Default safe tools in ask mode.
    if is_default_approved_tool(tool_call) {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    None
}

/// Read the live permission mode. Falls back to the static snapshot only when
/// the live lock is poisoned (which would already abort the turn elsewhere).
#[inline]
pub(super) fn current_permission_mode(config: &AgentConfig) -> PermissionMode {
    config
        .live_permission_mode
        .read()
        .map_or(config.permission_mode, |guard| *guard)
}

fn is_default_approved_tool(tool_call: &AgentToolCall) -> bool {
    matches!(
        tool_call.name.as_ref(),
        "Read"
            | "List"
            | "Grep"
            | "Find"
            | "Glob"
            | "TodoList"
            | "TaskList"
            | "TaskOutput"
            | "Skill"
            | "AskUserQuestion"
            | "Sleep"
    )
}

fn access_for_tool(tool_call: &AgentToolCall, grant: bool) -> ToolAccess {
    match tool_call.name.as_ref() {
        "Read" | "List" | "Grep" | "Find" | "Glob" => ToolAccess {
            file_read: grant,
            ..ToolAccess::none()
        },
        "Write" | "Edit" => ToolAccess {
            file_write: grant,
            ..ToolAccess::none()
        },
        "Bash" | "Terminal" | "TaskStop" => ToolAccess {
            shell: grant,
            ..ToolAccess::none()
        },
        "AskUserQuestion" => ToolAccess {
            user_question: grant,
            ..ToolAccess::none()
        },
        _ => ToolAccess {
            tool: grant,
            ..ToolAccess::none()
        },
    }
}

/// Build ordinary Tool/Shell approval options. Labels are presentation-only;
/// callers must branch on the typed `ApprovalAction`, never on label text.
fn ordinary_approval_options(
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
) -> Vec<ApprovalOption> {
    let mut options = vec![ApprovalOption {
        label: "Approve once".to_owned(),
        description: None,
        action: ApprovalAction::PermitOnce,
    }];
    if let Some(scope) = session_scope.filter(|scope| !scope.is_empty()) {
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

/// Presentation copy for ordinary Tool/Shell approvals.
fn ordinary_approval_presentation(
    operation: PermissionOperation,
    subject: &str,
    arguments: &serde_json::Value,
    edit_presentation: Option<EditApprovalPresentation>,
    write_presentation: Option<WriteApprovalPresentation>,
) -> ApprovalPresentation {
    let is_task_stop =
        operation == PermissionOperation::Shell && arguments.get("task_id").is_some();
    let is_terminal = operation == PermissionOperation::Shell && arguments.get("mode").is_some();

    if is_task_stop {
        return ApprovalPresentation::Tool {
            title: "Stop background task?".to_owned(),
            details: compact_details([
                labeled_argument(arguments, "task_id"),
                labeled_argument(arguments, "reason"),
            ]),
        };
    }

    if is_terminal || operation == PermissionOperation::Shell {
        let title = if is_terminal {
            terminal_approval_title(arguments)
        } else {
            "Run this command?".to_owned()
        };
        let command = arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(subject)
            .to_owned();
        let cwd = arguments
            .get("cwd")
            .or_else(|| arguments.get("workdir"))
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from);
        return ApprovalPresentation::Command {
            title,
            command,
            cwd,
        };
    }

    if let Some(edit) = edit_presentation {
        let n = edit.files;
        return ApprovalPresentation::Edit {
            title: format!("Edit {n} files?"),
            edit,
        };
    }

    if let Some(write) = write_presentation {
        let n = write.files;
        return ApprovalPresentation::Write {
            title: format!("Write {n} files?"),
            write,
        };
    }

    match operation {
        PermissionOperation::FileWrite => ApprovalPresentation::Tool {
            title: "Write file?".to_owned(),
            details: compact_details([labeled_argument(arguments, "path")]),
        },
        PermissionOperation::FileRead => ApprovalPresentation::Tool {
            title: "Read workspace data?".to_owned(),
            details: non_empty_details(
                compact_details([
                    labeled_argument(arguments, "path"),
                    labeled_argument(arguments, "pattern"),
                ]),
                || vec![format!("target: {subject}")],
            ),
        },
        PermissionOperation::Tool => ApprovalPresentation::Tool {
            title: "Run tool?".to_owned(),
            details: compact_details([Some(format!("tool: {subject}"))]),
        },
        PermissionOperation::UserQuestion => ApprovalPresentation::Tool {
            title: "User question".to_owned(),
            details: compact_details([Some(subject.to_owned())]),
        },
        PermissionOperation::WorkflowSave | PermissionOperation::WorkflowLaunch => {
            unreachable!("Workflow save/launch use dedicated presentation builders")
        }
        PermissionOperation::ThemeSave => ApprovalPresentation::Tool {
            title: "Save theme draft?".to_owned(),
            details: compact_details([
                labeled_argument(arguments, "draft_id"),
                labeled_argument(arguments, "overwrite"),
            ]),
        },
        PermissionOperation::PlanTransition | PermissionOperation::GoalTransition => {
            unreachable!("Plan/Goal use dedicated presentation builders")
        }
        PermissionOperation::Shell => unreachable!("Shell handled above"),
    }
}

fn plan_approval_options(input: &ExitPlanModeInput) -> Vec<ApprovalOption> {
    let mut options = Vec::new();
    let alternatives = input.options.as_deref().unwrap_or(&[]);
    if alternatives.is_empty() {
        options.push(ApprovalOption {
            label: "Approve".to_owned(),
            description: None,
            action: ApprovalAction::ApprovePlan { selection: None },
        });
    } else {
        for option in alternatives {
            options.push(approve_plan_option(option));
        }
    }
    for suggestion in input.suggestions.as_deref().unwrap_or(&[]) {
        options.push(revise_plan_suggestion_option(suggestion));
    }
    options.push(ApprovalOption {
        label: "Reject".to_owned(),
        description: None,
        action: ApprovalAction::RejectPlan,
    });
    options.push(ApprovalOption {
        label: "Reject with feedback".to_owned(),
        description: None,
        action: ApprovalAction::RevisePlan {
            preset_feedback: None,
        },
    });
    options
}

fn approve_plan_option(option: &ExitPlanModeOption) -> ApprovalOption {
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
}

fn revise_plan_suggestion_option(suggestion: &ExitPlanModeSuggestion) -> ApprovalOption {
    let preset = suggestion
        .feedback
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(|| suggestion.description.clone(), str::to_owned);
    ApprovalOption {
        label: suggestion.label.clone(),
        description: Some(suggestion.description.clone()),
        action: ApprovalAction::RevisePlan {
            preset_feedback: Some(preset),
        },
    }
}

fn goal_approval_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Approve".to_owned(),
            description: None,
            action: ApprovalAction::StartGoal,
        },
        ApprovalOption {
            label: "Reject".to_owned(),
            description: None,
            action: ApprovalAction::RejectGoal,
        },
        ApprovalOption {
            label: "Reject with feedback".to_owned(),
            description: None,
            action: ApprovalAction::ReviseGoal {
                preset_feedback: None,
            },
        },
    ]
}

fn workflow_launch_approval_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Launch".to_owned(),
            description: None,
            action: ApprovalAction::LaunchWorkflow,
        },
        ApprovalOption {
            label: "Revise".to_owned(),
            description: Some("Return feedback without creating a run.".to_owned()),
            action: ApprovalAction::ReviseWorkflow {
                preset_feedback: None,
            },
        },
        ApprovalOption {
            label: "Cancel".to_owned(),
            description: Some("Cancel without creating a run.".to_owned()),
            action: ApprovalAction::CancelWorkflow,
        },
    ]
}

fn workflow_save_approval_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Save".to_owned(),
            description: None,
            action: ApprovalAction::SaveWorkflow,
        },
        ApprovalOption {
            label: "Revise".to_owned(),
            description: Some("Return feedback without saving.".to_owned()),
            action: ApprovalAction::ReviseWorkflow {
                preset_feedback: None,
            },
        },
        ApprovalOption {
            label: "Cancel".to_owned(),
            description: Some("Cancel without saving.".to_owned()),
            action: ApprovalAction::CancelWorkflow,
        },
    ]
}

fn with_workflow_origin(mut request: ApprovalRequest, config: &AgentConfig) -> ApprovalRequest {
    if request.workflow_origin.is_none() {
        request.workflow_origin = config.workflow_execution_origin.clone();
    }
    request
}

fn build_workflow_approval_request(
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    prepared: &crate::tools::workflow::PreparedWorkflowAction,
) -> Result<ApprovalRequest, ToolResult> {
    let workflow = crate::tools::workflow::launch_approval_presentation_from_prepared(
        prepared,
        &config.workflow_definitions,
    )?;
    Ok(ApprovalRequest {
        turn,
        id: tool_call.id.to_string(),
        operation: PermissionOperation::WorkflowLaunch,
        presentation: ApprovalPresentation::Workflow {
            title: "Launch workflow?".to_owned(),
            workflow,
        },
        options: workflow_launch_approval_options(),
        workflow_origin: None,
    })
}

fn build_workflow_save_approval_request(
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    prepared: &crate::tools::workflow::PreparedWorkflowAction,
) -> Result<ApprovalRequest, ToolResult> {
    let save = crate::tools::workflow::save_approval_presentation_from_prepared(
        prepared,
        &config.workflow_definitions,
    )?;
    Ok(ApprovalRequest {
        turn,
        id: tool_call.id.to_string(),
        operation: PermissionOperation::WorkflowSave,
        presentation: ApprovalPresentation::WorkflowSave {
            title: "Save workflow?".to_owned(),
            save: Box::new(save),
        },
        options: workflow_save_approval_options(),
        workflow_origin: None,
    })
}

fn build_plan_approval_request(
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> ApprovalRequest {
    // Prevalidation already ran; fall back to empty options if reparse fails.
    let input = serde_json::from_value::<ExitPlanModeInput>(arguments.clone()).unwrap_or(
        ExitPlanModeInput {
            plan_summary: None,
            options: None,
            suggestions: None,
        },
    );
    let summary = input
        .plan_summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let (path, markdown) = config
        .plan_mode
        .read()
        .ok()
        .and_then(|pm| pm.data().ok().flatten())
        .map_or((None, String::new()), |data| {
            (Some(data.path), data.content)
        });
    ApprovalRequest {
        turn,
        id: tool_call.id.to_string(),
        operation: PermissionOperation::PlanTransition,
        presentation: ApprovalPresentation::Plan {
            title: "Plan Review".to_owned(),
            path,
            markdown,
            summary,
        },
        options: plan_approval_options(&input),
        workflow_origin: None,
    }
}

fn build_goal_approval_request(
    turn: u32,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> ApprovalRequest {
    let args =
        serde_json::from_value::<ExitGoalModeArgs>(arguments.clone()).unwrap_or(ExitGoalModeArgs {
            objective: String::new(),
            completion_criterion: None,
            phases: Vec::new(),
        });
    ApprovalRequest {
        turn,
        id: tool_call.id.to_string(),
        operation: PermissionOperation::GoalTransition,
        presentation: ApprovalPresentation::Goal {
            title: "Start reviewed goal".to_owned(),
            objective: args.objective,
            completion_criterion: args.completion_criterion,
            phases: args.phases,
        },
        options: goal_approval_options(),
        workflow_origin: None,
    }
}

fn terminal_approval_title(arguments: &serde_json::Value) -> String {
    match arguments
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
    {
        "start" => "Start terminal?".to_owned(),
        "write" => "Write to terminal?".to_owned(),
        "resize" => "Resize terminal?".to_owned(),
        "stop" => "Stop terminal?".to_owned(),
        _ => "Use terminal?".to_owned(),
    }
}

fn labeled_argument(arguments: &serde_json::Value, key: &str) -> Option<String> {
    let value = arguments.get(key)?;
    match value {
        serde_json::Value::String(value) if !value.is_empty() => Some(format!("{key}: {value}")),
        serde_json::Value::Bool(value) => Some(format!("{key}: {value}")),
        serde_json::Value::Number(value) => Some(format!("{key}: {value}")),
        _ => None,
    }
}

fn compact_details(lines: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    lines.into_iter().flatten().collect()
}

fn non_empty_details(details: Vec<String>, fallback: impl FnOnce() -> Vec<String>) -> Vec<String> {
    if details.is_empty() {
        fallback()
    } else {
        details
    }
}

fn build_ordinary_approval_request(
    turn: u32,
    tool_call: &AgentToolCall,
    prepared_call: &PreparedToolCall,
    operation: PermissionOperation,
    subject: &str,
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
) -> ApprovalRequest {
    let (edit_presentation, write_presentation) = match &prepared_call.execution {
        PreparedExecution::Edit(edit) => (Some(edit.approval_presentation()), None),
        PreparedExecution::Write(write) => (None, Some(write.approval_presentation())),
        PreparedExecution::Direct | PreparedExecution::Workflow(_) => (None, None),
    };
    ApprovalRequest {
        turn,
        id: tool_call.id.to_string(),
        operation,
        presentation: ordinary_approval_presentation(
            operation,
            subject,
            &prepared_call.arguments,
            edit_presentation,
            write_presentation,
        ),
        options: ordinary_approval_options(session_scope, prefix_rule),
        workflow_origin: None,
    }
}

/// Outcome of applying a validated approval resolution.
enum AppliedApproval {
    Allow {
        approval: Option<ApprovalExecutionContext>,
    },
    Terminal {
        result: ToolResult,
        permission_decision: Option<PermissionTerminalDecision>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PermissionTerminalDecision {
    Denied,
    Cancelled,
    Required,
}

impl PermissionTerminalDecision {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Required => "required",
        }
    }
}

/// Persist a Layer-2 prefix rule, rolling back the in-memory insert when disk
/// write fails. Returns `None` on success (continue execution) or a tool error.
fn persist_prefix_rule_or_error(
    config: &AgentConfig,
    rule: &PrefixApprovalRule,
) -> Option<ToolResult> {
    if ApprovalRuleStore::is_would_approve_all(&rule.prefix) {
        return None;
    }
    let should_save = if let Ok(mut store) = config.prefix_approval_rules.lock() {
        let was_new = !store.prefix_rules.iter().any(|r| r.prefix == rule.prefix);
        store.insert(rule.clone());
        was_new
    } else {
        false
    };
    if should_save && let Err(error) = config.save_prefix_approval_rules() {
        if let Ok(mut store) = config.prefix_approval_rules.lock() {
            store
                .prefix_rules
                .retain(|saved| saved.prefix != rule.prefix);
        }
        tracing::warn!(%error, "failed to persist prefix approval rule");
        return Some(ToolResult::error(format!(
            "failed to persist prefix approval rule: {error}"
        )));
    }
    None
}

fn reject_approval(operation: PermissionOperation, subject: &str) -> AppliedApproval {
    AppliedApproval::Terminal {
        result: permission_error(
            operation,
            subject,
            PermissionTerminalDecision::Denied,
            "approval denied",
        ),
        permission_decision: Some(PermissionTerminalDecision::Denied),
    }
}

fn revise_approval(mode: &str, feedback: Option<String>) -> AppliedApproval {
    let feedback = feedback.unwrap_or_default();
    AppliedApproval::Terminal {
        result: ToolResult::ok(format!(
            "User requested revisions. {mode} remains active.\n\nFeedback: {feedback}"
        )),
        permission_decision: None,
    }
}

fn apply_approval_resolution(
    config: &AgentConfig,
    operation: PermissionOperation,
    subject: &str,
    resolution: ApprovalResolution,
) -> AppliedApproval {
    match resolution {
        ApprovalResolution::Cancelled { .. } => AppliedApproval::Terminal {
            result: permission_error(
                operation,
                subject,
                PermissionTerminalDecision::Cancelled,
                "approval cancelled",
            ),
            permission_decision: Some(PermissionTerminalDecision::Cancelled),
        },
        ApprovalResolution::Selected {
            action:
                ApprovalAction::PermitOnce
                | ApprovalAction::SaveWorkflow
                | ApprovalAction::LaunchWorkflow,
            ..
        } => AppliedApproval::Allow { approval: None },
        ApprovalResolution::Selected {
            action: ApprovalAction::PermitForSession { scope },
            ..
        } => {
            // Layer 1: record each narrow key (exact canonical command/cwd,
            // exact file path/op). With no derived scope this degrades to a
            // no-op PermitOnce — it never creates a tool-name wildcard.
            if let Ok(mut approved) = config.session_approvals.lock() {
                scope.record(&mut approved);
            }
            AppliedApproval::Allow { approval: None }
        }
        ApprovalResolution::Selected {
            action: ApprovalAction::PermitForPrefix { rule },
            ..
        } => match persist_prefix_rule_or_error(config, &rule) {
            Some(error) => AppliedApproval::Terminal {
                result: error,
                permission_decision: None,
            },
            None => AppliedApproval::Allow { approval: None },
        },
        ApprovalResolution::Selected {
            action: ApprovalAction::Reject,
            ..
        } => reject_approval(operation, subject),
        ApprovalResolution::Selected {
            action: ApprovalAction::ApprovePlan { selection },
            ..
        } => AppliedApproval::Allow {
            approval: Some(ApprovalExecutionContext::Plan { selection }),
        },
        ApprovalResolution::Selected {
            action: ApprovalAction::StartGoal,
            ..
        } => AppliedApproval::Allow {
            approval: Some(ApprovalExecutionContext::Goal),
        },
        ApprovalResolution::Selected {
            action: ApprovalAction::CancelWorkflow,
            ..
        } => AppliedApproval::Terminal {
            result: permission_error(
                operation,
                subject,
                PermissionTerminalDecision::Cancelled,
                "approval cancelled",
            ),
            permission_decision: Some(PermissionTerminalDecision::Cancelled),
        },
        ApprovalResolution::Selected {
            action: ApprovalAction::RejectPlan,
            ..
        } => reject_approval(PermissionOperation::PlanTransition, "Exit plan mode"),
        ApprovalResolution::Selected {
            action: ApprovalAction::RejectGoal,
            ..
        } => reject_approval(PermissionOperation::GoalTransition, "Start reviewed goal"),
        ApprovalResolution::Selected {
            action: ApprovalAction::RevisePlan { .. },
            feedback,
            ..
        } => revise_approval("Plan mode", feedback),
        ApprovalResolution::Selected {
            action: ApprovalAction::ReviseGoal { .. },
            feedback,
            ..
        } => revise_approval("Goal mode", feedback),
        ApprovalResolution::Selected {
            action: ApprovalAction::ReviseWorkflow { .. },
            feedback,
            ..
        } => AppliedApproval::Terminal {
            result: ToolResult::ok(format!(
                "User requested workflow revisions. No workflow save or run was created.\n\nFeedback: {}",
                feedback.unwrap_or_default()
            )),
            permission_decision: None,
        },
    }
}

fn emit_approval_resolved(
    emitter: &mut impl EventPublisher,
    turn: u32,
    request_id: &str,
    resolution: ApprovalResolution,
) {
    emitter.emit(AgentEvent::ApprovalResolved {
        turn,
        request_id: request_id.to_owned(),
        resolution,
    });
}

struct ApprovalInput<'a> {
    turn: u32,
    tool_call: &'a AgentToolCall,
    prepared_call: &'a PreparedToolCall,
    operation: PermissionOperation,
    subject: String,
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
}

fn approval_request(
    config: &AgentConfig,
    input: &ApprovalInput<'_>,
) -> Result<ApprovalRequest, ToolResult> {
    let arguments = &input.prepared_call.arguments;
    let request = match input.operation {
        PermissionOperation::PlanTransition => Ok(build_plan_approval_request(
            config,
            input.turn,
            input.tool_call,
            arguments,
        )),
        PermissionOperation::GoalTransition => Ok(build_goal_approval_request(
            input.turn,
            input.tool_call,
            arguments,
        )),
        PermissionOperation::WorkflowLaunch => {
            let prepared = match &input.prepared_call.execution {
                PreparedExecution::Workflow(action) => action,
                _ => {
                    return Err(crate::tools::workflow::input_error_result(
                        crate::tools::workflow::WorkflowInputError::input(
                            crate::tools::workflow::WorkflowAction::RunInline,
                            Some("action"),
                            "launch review requires prepared workflow action",
                        ),
                    ));
                }
            };
            build_workflow_approval_request(config, input.turn, input.tool_call, prepared)
        }
        PermissionOperation::WorkflowSave => {
            let prepared = match &input.prepared_call.execution {
                PreparedExecution::Workflow(action) => action,
                _ => {
                    return Err(crate::tools::workflow::input_error_result(
                        crate::tools::workflow::WorkflowInputError::input(
                            crate::tools::workflow::WorkflowAction::Save,
                            Some("action"),
                            "save review requires prepared workflow action",
                        ),
                    ));
                }
            };
            build_workflow_save_approval_request(config, input.turn, input.tool_call, prepared)
        }
        _ => Ok(build_ordinary_approval_request(
            input.turn,
            input.tool_call,
            input.prepared_call,
            input.operation,
            &input.subject,
            input.session_scope.clone(),
            input.prefix_rule.clone(),
        )),
    }?;
    Ok(with_workflow_origin(request, config))
}

async fn resolve_approval(
    config: &AgentConfig,
    input: ApprovalInput<'_>,
    emitter: &mut impl EventPublisher,
    cancel_token: &CancellationToken,
) -> AppliedApproval {
    let prepared_call = input.prepared_call;
    let operation = input.operation;
    let subject = &input.subject;
    let request = match approval_request(config, &input) {
        Ok(request) => request,
        Err(result) => {
            return AppliedApproval::Terminal {
                result,
                permission_decision: None,
            };
        }
    };
    emitter.emit(AgentEvent::ApprovalRequested {
        request: request.clone(),
    });
    let response = if let Some(handler) = &config.approval_handler {
        handler(&request)
    } else if let Some(handler) = &config.async_approval_handler {
        tokio::select! {
            biased;
            () = cancel_token.cancelled() => {
                emit_approval_resolved(
                    emitter,
                    request.turn,
                    &request.id,
                    ApprovalResolution::Cancelled {
                        reason: ApprovalCancelReason::Interrupt,
                    },
                );
                let result = match &prepared_call.execution {
                    PreparedExecution::Edit(edit) => edit.cancelled_before_commit_result(),
                    PreparedExecution::Write(write) => write.cancelled_before_commit_result(),
                    PreparedExecution::Direct | PreparedExecution::Workflow(_) => cancelled_tool_result(),
                };
                return AppliedApproval::Terminal {
                    result,
                    permission_decision: None,
                };
            }
            response = handler(request.clone()) => response,
        }
    } else {
        // No handler: close the request with Cancelled so UI/session never
        // leave an unpaired ApprovalRequested open.
        emit_approval_resolved(
            emitter,
            request.turn,
            &request.id,
            ApprovalResolution::Cancelled {
                reason: ApprovalCancelReason::SessionEnded,
            },
        );
        return AppliedApproval::Terminal {
            result: permission_error(
                operation,
                subject,
                PermissionTerminalDecision::Required,
                "approval required",
            ),
            permission_decision: Some(PermissionTerminalDecision::Required),
        };
    };
    let resolution = match request.validate_response(&response) {
        Ok(resolution) => resolution,
        Err(error) => {
            // Invalid responses are rejected at the trust boundary; still emit a
            // terminal resolution so every ApprovalRequested has a pair.
            emit_approval_resolved(
                emitter,
                request.turn,
                &request.id,
                ApprovalResolution::Cancelled {
                    reason: ApprovalCancelReason::SessionEnded,
                },
            );
            return AppliedApproval::Terminal {
                result: ToolResult::error(format!(
                    "invalid approval response for {}: {error:?}",
                    request.id
                )),
                permission_decision: None,
            };
        }
    };
    emit_approval_resolved(emitter, request.turn, &request.id, resolution.clone());
    apply_approval_resolution(config, operation, subject, resolution)
}

fn permission_error(
    operation: PermissionOperation,
    subject: &str,
    decision: PermissionTerminalDecision,
    prefix: &'static str,
) -> ToolResult {
    let noun = match operation {
        PermissionOperation::FileRead => "file read",
        PermissionOperation::FileWrite => "file write",
        PermissionOperation::Shell => "shell",
        PermissionOperation::Tool => "tool",
        PermissionOperation::UserQuestion => "user question",
        PermissionOperation::WorkflowSave => "workflow save",
        PermissionOperation::WorkflowLaunch => "workflow launch",
        PermissionOperation::ThemeSave => "theme save",
        PermissionOperation::PlanTransition => "plan transition",
        PermissionOperation::GoalTransition => "goal transition",
    };
    ToolResult::error(format!("{prefix} for {noun}: {subject}")).with_details(serde_json::json!({
        "kind": "permission",
        "decision": decision.as_str(),
        "operation": noun,
        "subject": subject,
        "side_effect_occurred": false,
    }))
}

fn permission_operation_for_tool(
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> Option<(PermissionOperation, String)> {
    match tool_call.name.as_ref() {
        "Read" | "List" | "Grep" | "Find" | "Glob" => Some((
            PermissionOperation::FileRead,
            path_subject(arguments).unwrap_or_else(|| tool_call.name.to_string()),
        )),
        "Write" | "Edit" => Some((
            PermissionOperation::FileWrite,
            path_subject(arguments).unwrap_or_else(|| tool_call.name.to_string()),
        )),
        "Bash" | "Terminal" | "TaskStop" => Some((
            PermissionOperation::Shell,
            arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .or_else(|| arguments.get("task_id").and_then(serde_json::Value::as_str))
                .or_else(|| arguments.get("handle").and_then(serde_json::Value::as_str))
                .unwrap_or(tool_call.name.as_ref())
                .to_owned(),
        )),
        "AskUserQuestion" => Some((
            PermissionOperation::UserQuestion,
            arguments
                .get("questions")
                .and_then(|q| q.as_array())
                .and_then(|arr| arr.first())
                .and_then(|q| q.get("question"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("question")
                .to_owned(),
        )),
        _ => None,
    }
}

fn path_subject(arguments: &serde_json::Value) -> Option<String> {
    arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

// ---------------------------------------------------------------------------
// Layer 1/2/3 — approval scope, prefix rule, and safety derivation helpers
// ---------------------------------------------------------------------------

/// Workspace root string used as part of every approval key. Stored on the key
/// so a session store reused across workspaces never leaks an approval. Empty
/// when the workspace root is unknown.
fn workspace_key_root(config: &AgentConfig) -> String {
    config
        .workspace_root
        .as_deref()
        .map_or_else(String::new, |root| root.display().to_string())
}

/// Resolve the effective Bash cwd: if the caller passed `cwd`, resolve it
/// through workspace containment, else use the workspace root. Returns `None`
/// when the path escapes the workspace or the workspace root is unknown.
fn resolve_bash_cwd(config: &AgentConfig, arguments: &serde_json::Value) -> Option<String> {
    let workspace_root = config.workspace_root.as_deref()?;
    let candidate = arguments
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(std::path::Path::new);
    let resolved = match candidate {
        Some(rel) if !rel.is_absolute() => workspace_root.join(rel),
        Some(abs) => abs.to_path_buf(),
        None => workspace_root.to_path_buf(),
    };
    let normalized = normalize_path(&resolved);
    if !normalized.starts_with(workspace_root) {
        return None;
    }
    Some(normalized.display().to_string())
}

/// Split a shell command string into argv tokens using POSIX-ish word rules.
/// Handles single/double quotes and backslash escapes. Returns `None` when the
/// string is empty or unparseable (e.g. unmatched quote).
fn tokenize_shell_command(command: &str) -> Option<Vec<String>> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;
    let mut chars = trimmed.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            } else if ch == '\\' {
                if let Some(&next) = chars.peek()
                    && matches!(next, '"' | '\\' | '$' | '`')
                {
                    current.push(next);
                    chars.next();
                    continue;
                }
                current.push(ch);
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' => {
                in_single = true;
                has_token = true;
            }
            '"' => {
                in_double = true;
                has_token = true;
            }
            '\\' => {
                if let Some(&next) = chars.peek() {
                    current.push(next);
                    chars.next();
                    has_token = true;
                }
            }
            c if c.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if in_single || in_double {
        return None; // unmatched quote
    }
    if has_token {
        tokens.push(current);
    }
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

/// True when a command string contains shell control operators that make it a
/// compound/opaque script (`&&`, `||`, `;`, `|`, `>`, `<`, backticks, `$(...)`,
/// `{...}`). Used to decide whether a stable argv prefix can be proposed.
fn is_compound_or_opaque_command(command: &str) -> bool {
    // Quick scan outside of quotes. Conservative: any of these operators marks
    // the line as compound/opaque for prefix purposes.
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '&' if !in_single && !in_double => {
                if chars.peek() == Some(&'&') {
                    return true;
                }
            }
            '|' | ';' | '>' | '<' | '`' | '{' if !in_single && !in_double => return true,
            '$' if !in_single && !in_double && chars.peek() == Some(&'(') => return true,
            _ => {}
        }
    }
    false
}

/// Tokenize a Bash command for prefix-check / safety classification. Returns
/// `None` when there is no `command` arg or it cannot be tokenized.
fn shell_argv(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> Option<Vec<String>> {
    if tool_call.name.as_ref() != "Bash" {
        return None;
    }
    let background = arguments
        .get("run_in_background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if background {
        return None;
    }
    let raw = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)?;
    let _ = config.workspace_root.as_deref()?;
    tokenize_shell_command(raw)
}

/// Alias used in `permission_preparation_for_mode` for clarity.
fn shell_argv_for_prefix_check(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> Option<Vec<String>> {
    shell_argv(config, tool_call, arguments)
}

/// Derive `(session_scope, prefix_rule)` for a tool call. Returns `(None, None)`
/// for review transitions, dangerous commands, interactive tools, and anything
/// where a reusable grant is unsafe.
fn approval_scope_for_prepared_call(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    prepared_call: &PreparedToolCall,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    // Review transitions and dangerous commands never offer scope/prefix.
    if matches!(tool_call.name.as_ref(), "ExitPlanMode" | "ExitGoalMode") {
        return (None, None);
    }
    match tool_call.name.as_ref() {
        "Bash" => bash_approval_scope(config, &prepared_call.arguments),
        "Write" => match &prepared_call.execution {
            PreparedExecution::Write(write) => (write_session_approval_scope(config, write), None),
            _ => (None, None),
        },
        "Edit" => match &prepared_call.execution {
            PreparedExecution::Edit(edit) => (edit_session_approval_scope(config, edit), None),
            _ => (None, None),
        },
        _ => tool_approval_scope(config, &tool_call.name),
    }
}

/// Multi-key session scope for a prepared Edit batch. Omitted when any target
/// cannot participate in a narrow workspace-contained `FileWrite` key.
fn edit_session_approval_scope(
    config: &AgentConfig,
    edit: &PreparedEdit,
) -> Option<SessionApprovalScope> {
    let workspace = workspace_key_root(config);
    let workspace_root = config.workspace_root.as_deref()?;
    edit.session_approval_scope(&workspace, workspace_root)
}

/// Multi-key session scope for a prepared batch Write. Omitted when the
/// workspace root is unknown or any target cannot participate in a narrow
/// workspace-contained `FileWrite` key.
fn write_session_approval_scope(
    config: &AgentConfig,
    write: &PreparedWrite,
) -> Option<SessionApprovalScope> {
    let workspace = workspace_key_root(config);
    let workspace_root = config.workspace_root.as_deref()?;
    write.session_approval_scope(&workspace, workspace_root)
}

/// Build the session scope + optional prefix rule for a Bash call.
fn bash_approval_scope(
    config: &AgentConfig,
    arguments: &serde_json::Value,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    let background = arguments
        .get("run_in_background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if background {
        return (None, None); // background bash has no safe reusable scope
    }
    let Some(raw_command) = arguments.get("command").and_then(serde_json::Value::as_str) else {
        return (None, None);
    };
    let command = raw_command.trim();
    if command.is_empty() {
        return (None, None);
    }
    let workspace = workspace_key_root(config);
    let cwd = resolve_bash_cwd(config, arguments).unwrap_or_else(|| workspace.clone());
    // Dangerous commands get no scope (re-prompt every time).
    if let Some(argv) = tokenize_shell_command(command) {
        if command_might_be_dangerous(&argv) {
            return (None, None);
        }
        // Layer 1: exact canonical argv key (only when not compound/opaque, so
        // `git status && git push` does not get cached as if it were `git status`).
        let key = if is_compound_or_opaque_command(command) {
            SessionApprovalKey::Shell {
                workspace: workspace.clone(),
                cwd: cwd.clone(),
                command: vec!["__shell_script__".to_owned(), command.to_owned()],
            }
        } else {
            SessionApprovalKey::Shell {
                workspace: workspace.clone(),
                cwd: cwd.clone(),
                command: argv.clone(),
            }
        };
        let scope = SessionApprovalScope {
            keys: vec![key],
            label: "Approve this exact command for this session".to_owned(),
            detail: format!("Exact command in {cwd}: {command}"),
        };
        // Layer 2: propose a prefix rule only for non-compound commands (so the
        // prefix is a real argv prefix, not half of a `&&`). Use the first
        // program token; refuse empty (would approve everything).
        let prefix_rule = if !is_compound_or_opaque_command(command) && !argv.is_empty() {
            let prefix = vec![argv[0].clone()];
            if ApprovalRuleStore::is_would_approve_all(&prefix) {
                None
            } else {
                Some(PrefixApprovalRule {
                    label: argv[0].clone(),
                    prefix,
                })
            }
        } else {
            None
        };
        (Some(scope), prefix_rule)
    } else {
        // Could not tokenize (unmatched quote etc.) — opaque exact-text key.
        let key = SessionApprovalKey::Shell {
            workspace: workspace.clone(),
            cwd: cwd.clone(),
            command: vec!["__shell_script__".to_owned(), command.to_owned()],
        };
        let scope = SessionApprovalScope {
            keys: vec![key],
            label: "Approve this exact command for this session".to_owned(),
            detail: format!("Exact command in {cwd}: {command}"),
        };
        (Some(scope), None)
    }
}

/// Build the session scope for a generic tool call (MCP tools and any
/// non-builtin tool). The scope is keyed by the fully-qualified tool name so the
/// same tool is auto-approved for the rest of the session. Returns no prefix rule
/// (prefix rules are shell-specific). Built-in write/shell tools are handled by
/// their own dedicated scope derivations before this is reached.
fn tool_approval_scope(
    config: &AgentConfig,
    tool_name: &str,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    if tool_name.is_empty() {
        return (None, None);
    }
    let workspace = workspace_key_root(config);
    let key = SessionApprovalKey::Tool {
        workspace,
        name: tool_name.to_owned(),
    };
    let scope = SessionApprovalScope {
        keys: vec![key],
        label: "Approve this tool for this session".to_owned(),
        detail: format!("Tool: {tool_name}"),
    };
    (Some(scope), None)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use serde_json::json;

    use super::*;
    use crate::harness::fake_model;
    use crate::workspace_policy::{
        WorkspaceAccessPolicy, WorkspaceAccessRoot, WorkspaceAccessRootKind,
    };

    #[test]
    fn plan_mode_denies_write_to_added_write_root() {
        let primary = tempfile::tempdir().expect("primary tempdir");
        let added = tempfile::tempdir().expect("added tempdir");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [WorkspaceAccessRoot {
                path: added.path().to_path_buf(),
                kind: WorkspaceAccessRootKind::Added,
                read: true,
                write: true,
            }],
        )
        .expect("workspace policy");
        let config = AgentConfig::for_model(fake_model())
            .with_workspace_root(primary.path())
            .expect("workspace root")
            .with_workspace_policy(Arc::new(RwLock::new(Some(policy))))
            .with_permission_mode(PermissionMode::Ask);
        config
            .plan_mode
            .write()
            .expect("plan mode lock")
            .enter_in_memory();
        let blocked_path = added.path().join("blocked.txt");
        let arguments = json!({
            "path": blocked_path.display().to_string(),
            "content": "blocked",
        });
        let call = AgentToolCall {
            id: "call-write-added-root".into(),
            name: "Write".into(),
            raw_arguments: arguments.to_string().into(),
        };
        let prepared = PreparedToolCall {
            id: call.id.to_string(),
            name: call.name.to_string(),
            raw_arguments: call.raw_arguments.to_string(),
            arguments,
            warning: None,
            approval: None,
            execution: PreparedExecution::Direct,
        };

        let preparation = permission_preparation_for_mode(&config, &call, &prepared);

        assert!(matches!(preparation, PermissionPreparation::Deny(_)));
    }

    #[test]
    fn sleep_is_default_approved() {
        let call = AgentToolCall {
            id: "call-sleep".into(),
            name: "Sleep".into(),
            raw_arguments: r#"{"duration_seconds":1,"reason":"wait"}"#.into(),
        };
        assert!(is_default_approved_tool(&call));
    }

    fn workflow_prepared(arguments: serde_json::Value) -> (AgentToolCall, PreparedToolCall) {
        let call = AgentToolCall {
            id: "call-workflow".into(),
            name: "Workflow".into(),
            raw_arguments: arguments.to_string().into(),
        };
        let action = crate::tools::workflow::prepare_action(&arguments)
            .expect("valid workflow permission test input");
        let prepared = PreparedToolCall {
            id: call.id.to_string(),
            name: call.name.to_string(),
            raw_arguments: call.raw_arguments.to_string(),
            arguments,
            warning: None,
            approval: None,
            execution: PreparedExecution::Workflow(Arc::new(action)),
        };
        (call, prepared)
    }

    fn workflow_config(mode: PermissionMode) -> AgentConfig {
        AgentConfig::for_model(fake_model()).with_permission_mode(mode)
    }

    fn inline_run_input() -> serde_json::Value {
        json!({
            "action": "run_inline",
            "name": "perm-test",
            "description": "Permission routing test",
            "phases": [{"id": "work", "description": "Do the work"}],
            "script": "neo.phase('work')\nreturn {}",
            "input_schema": {"type": "object"},
            "output_schema": {"type": "object"}
        })
    }

    fn save_input() -> serde_json::Value {
        let mut input = inline_run_input();
        input["action"] = json!("save");
        input["scope"] = json!("user");
        input
    }

    #[test]
    fn workflow_read_and_validate_actions_run_directly_in_ask_and_plan_mode() {
        for arguments in [
            json!({"action": "list"}),
            json!({"action": "show", "name": "perm-test"}),
            json!({"action": "validate_saved", "name": "perm-test"}),
            {
                let mut input = inline_run_input();
                input["action"] = json!("validate_inline");
                input
            },
        ] {
            for mode in [
                PermissionMode::Ask,
                PermissionMode::Auto,
                PermissionMode::Yolo,
            ] {
                let config = workflow_config(mode);
                let (call, prepared) = workflow_prepared(arguments.clone());
                let preparation = permission_preparation_for_mode(&config, &call, &prepared);
                assert!(
                    matches!(preparation, PermissionPreparation::Run(_)),
                    "read/validate must run directly in {mode:?}: {arguments}"
                );
            }
            let config = workflow_config(PermissionMode::Ask);
            config
                .plan_mode
                .write()
                .expect("plan mode lock")
                .enter_in_memory();
            let (call, prepared) = workflow_prepared(arguments.clone());
            let preparation = permission_preparation_for_mode(&config, &call, &prepared);
            assert!(
                matches!(preparation, PermissionPreparation::Run(_)),
                "read/validate must run directly in plan mode: {arguments}"
            );
        }
    }

    #[test]
    fn workflow_save_asks_with_typed_save_review_in_ask_mode() {
        let config = workflow_config(PermissionMode::Ask);
        let (call, prepared) = workflow_prepared(save_input());
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(
                preparation,
                PermissionPreparation::Ask {
                    operation: PermissionOperation::WorkflowSave,
                    ..
                }
            ),
            "save must open the typed save review in Ask mode"
        );
    }

    #[test]
    fn workflow_run_asks_with_typed_launch_review_in_ask_mode() {
        let config = workflow_config(PermissionMode::Ask);
        let (call, prepared) = workflow_prepared(inline_run_input());
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(
                preparation,
                PermissionPreparation::Ask {
                    operation: PermissionOperation::WorkflowLaunch,
                    ..
                }
            ),
            "run must open the typed launch review in Ask mode"
        );
    }

    #[test]
    fn workflow_save_and_run_execute_directly_in_auto_and_yolo() {
        for mode in [PermissionMode::Auto, PermissionMode::Yolo] {
            for arguments in [save_input(), inline_run_input()] {
                let config = workflow_config(mode);
                let (call, prepared) = workflow_prepared(arguments.clone());
                let preparation = permission_preparation_for_mode(&config, &call, &prepared);
                assert!(
                    matches!(preparation, PermissionPreparation::Run(_)),
                    "save/run must execute directly in {mode:?}: {arguments}"
                );
            }
        }
    }

    #[test]
    fn workflow_save_and_run_are_denied_in_plan_mode() {
        for arguments in [save_input(), inline_run_input()] {
            let config = workflow_config(PermissionMode::Ask);
            config
                .plan_mode
                .write()
                .expect("plan mode lock")
                .enter_in_memory();
            let (call, prepared) = workflow_prepared(arguments.clone());
            let preparation = permission_preparation_for_mode(&config, &call, &prepared);
            assert!(
                matches!(preparation, PermissionPreparation::Deny(_)),
                "save/run must be denied in plan mode: {arguments}"
            );
        }
    }

    fn theme_draft_prepared(arguments: serde_json::Value) -> (AgentToolCall, PreparedToolCall) {
        let call = AgentToolCall {
            id: "call-theme-draft".into(),
            name: "ThemeDraft".into(),
            raw_arguments: arguments.to_string().into(),
        };
        let prepared = PreparedToolCall {
            id: call.id.to_string(),
            name: call.name.to_string(),
            raw_arguments: call.raw_arguments.to_string(),
            arguments,
            warning: None,
            approval: None,
            execution: PreparedExecution::Direct,
        };
        (call, prepared)
    }

    #[test]
    fn theme_draft_preview_runs_directly_in_every_mode_including_plan() {
        let preview = json!({
            "action": "preview",
            "name": "Aurora Night",
            "colors": {"brand": "#58a6ff"},
        });
        for mode in [
            PermissionMode::Ask,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            let config = workflow_config(mode);
            let (call, prepared) = theme_draft_prepared(preview.clone());
            let preparation = permission_preparation_for_mode(&config, &call, &prepared);
            assert!(
                matches!(preparation, PermissionPreparation::Run(_)),
                "preview must run directly in {mode:?}"
            );
        }
        let config = workflow_config(PermissionMode::Ask);
        config
            .plan_mode
            .write()
            .expect("plan mode lock")
            .enter_in_memory();
        let (call, prepared) = theme_draft_prepared(preview.clone());
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(preparation, PermissionPreparation::Run(_)),
            "preview must run directly in plan mode"
        );
    }

    #[test]
    fn theme_draft_preview_never_grants_file_write_access() {
        let preview = json!({
            "action": "preview",
            "name": "Aurora Night",
        });
        let config = workflow_config(PermissionMode::Ask);
        let (call, prepared) = theme_draft_prepared(preview);
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        let PermissionPreparation::Run(access) = preparation else {
            panic!("preview must run: {preparation:?}");
        };
        assert!(access.tool, "preview carries the tool access grant");
        assert!(
            !access.file_write,
            "ThemeDraft must never grant generic file_write"
        );
    }

    #[test]
    fn theme_draft_save_asks_with_typed_theme_save_operation_and_no_session_scope() {
        let config = workflow_config(PermissionMode::Ask);
        let (call, prepared) = theme_draft_prepared(json!({
            "action": "save",
            "draft_id": "draft-0001",
            "overwrite": false,
        }));
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(
                preparation,
                PermissionPreparation::Ask {
                    operation: PermissionOperation::ThemeSave,
                    session_scope: None,
                    ..
                }
            ),
            "save must open the typed ThemeSave review with no session scope in Ask mode"
        );
    }

    #[test]
    fn theme_draft_save_runs_directly_in_auto_and_yolo() {
        for mode in [PermissionMode::Auto, PermissionMode::Yolo] {
            let config = workflow_config(mode);
            let (call, prepared) = theme_draft_prepared(json!({
                "action": "save",
                "draft_id": "draft-0001",
            }));
            let preparation = permission_preparation_for_mode(&config, &call, &prepared);
            assert!(
                matches!(preparation, PermissionPreparation::Run(_)),
                "save must execute directly in {mode:?}"
            );
        }
    }

    #[test]
    fn theme_draft_save_is_denied_in_plan_mode() {
        let config = workflow_config(PermissionMode::Ask);
        config
            .plan_mode
            .write()
            .expect("plan mode lock")
            .enter_in_memory();
        let (call, prepared) = theme_draft_prepared(json!({
            "action": "save",
            "draft_id": "draft-0001",
        }));
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(preparation, PermissionPreparation::Deny(_)),
            "save must be denied in plan mode"
        );
    }

    #[test]
    fn cached_tool_session_approval_never_authorizes_theme_draft_save() {
        // A generic "approve this tool for the session" grant for a different
        // tool must not leak into ThemeDraft: the ThemeDraft branch returns
        // before cached approvals are consulted, and save never offers a scope.
        let config = workflow_config(PermissionMode::Ask);
        let mut approved = std::collections::HashSet::new();
        approved.insert(SessionApprovalKey::Tool {
            workspace: String::new(),
            name: "ThemeDraft".to_owned(),
        });
        *config.session_approvals.lock().expect("session approvals") = approved;
        let (call, prepared) = theme_draft_prepared(json!({
            "action": "save",
            "draft_id": "draft-0001",
        }));
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(
                preparation,
                PermissionPreparation::Ask {
                    operation: PermissionOperation::ThemeSave,
                    ..
                }
            ),
            "a cached tool session approval must not authorize a ThemeDraft save"
        );
    }
}
