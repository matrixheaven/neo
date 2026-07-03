use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::config::AgentConfig;
use super::events::EventPublisher;
use super::plan_orchestration::exit_plan_mode_has_reviewable_plan;
use super::tool_dispatch::{
    PreparedToolCall, PreparedToolCallResult, ask_user_runs_in_background, cancelled_tool_result,
};
use crate::permissions::{
    ApprovalRuleStore, FileWriteApprovalOperation, PrefixApprovalRule, SessionApprovalKey,
    SessionApprovalScope, command_might_be_dangerous, is_known_safe_command,
};
use crate::tools::normalize_path;
use crate::tools::plan_mode::prevalidate_exit_plan_mode;
use crate::{
    AgentEvent, AgentToolCall, PermissionApprovalDecision, PermissionMode, PermissionOperation,
    PlanModeGuard, ToolAccess, ToolResult, check_plan_mode_guard, is_active_plan_file_path,
};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequest {
    pub turn: u32,
    pub id: String,
    pub operation: PermissionOperation,
    pub subject: String,
    pub arguments: serde_json::Value,
    /// Reusable session scope for this request, when safely derivable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_scope: Option<SessionApprovalScope>,
    /// Proposed persistent prefix rule for this request (Layer 2), when the
    /// command reduces to a stable argv prefix. `None` when no prefix option
    /// should be offered (compound/opaque commands, non-shell tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_rule: Option<PrefixApprovalRule>,
    /// Preset revision suggestions for plan review (`PlanTransition` only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<crate::PlanSuggestion>,
}

pub(super) enum PermissionPreparation {
    Run(ToolAccess),
    Ask {
        operation: PermissionOperation,
        subject: String,
        session_scope: Option<SessionApprovalScope>,
        prefix_rule: Option<PrefixApprovalRule>,
    },
    Deny(String),
}

pub(super) async fn prepare_tool_call(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
    turn: u32,
    emitter: &mut impl EventPublisher,
    cancel_token: &CancellationToken,
) -> PreparedToolCall {
    let preparation = permission_preparation_for_mode(config, tool_call, arguments);

    match preparation {
        PermissionPreparation::Run(access) => PreparedToolCall {
            result: PreparedToolCallResult::Run,
            access,
        },
        PermissionPreparation::Deny(message) => PreparedToolCall {
            result: PreparedToolCallResult::Skip(ToolResult::error(message)),
            access: ToolAccess::none(),
        },
        PermissionPreparation::Ask {
            operation,
            subject,
            session_scope,
            prefix_rule,
        } => {
            match resolve_approval(
                config,
                turn,
                tool_call,
                arguments,
                operation,
                subject,
                session_scope,
                prefix_rule,
                emitter,
                cancel_token,
            )
            .await
            {
                Some(result) => PreparedToolCall {
                    result: PreparedToolCallResult::Skip(result),
                    access: ToolAccess::none(),
                },
                None => PreparedToolCall {
                    result: PreparedToolCallResult::Run,
                    access: access_for_tool(tool_call, true),
                },
            }
        }
    }
}

pub(super) fn permission_preparation_for_mode(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> PermissionPreparation {
    // Read live permission state once. The TUI may switch this mid-turn via
    // `/ask`, `/auto`, `/yolo`, or `/permissions`; every branch below must use
    // this `mode` instead of the static `config.permission_mode` snapshot.
    let mode = current_permission_mode(config);

    // 1. Plan-mode hard guard.
    if let Some(prep) = check_plan_guard(config, tool_call, arguments) {
        return prep;
    }

    // 2-5. Mode/tool-specific early returns (auto mode, EnterPlanMode, background AskUser).
    if let Some(prep) = check_mode_early_returns(tool_call, arguments, mode) {
        return prep;
    }

    // 6. Derive the reusable scope + prefix rule for the ask fallback.
    let (session_scope, prefix_rule) = approval_scope_for_tool_call(config, tool_call, arguments);

    // 7-8. Cached approvals (persistent prefix rules + session approvals).
    if let Some(prep) = check_cached_approvals(config, tool_call, arguments, session_scope.as_ref())
    {
        return prep;
    }

    // 9-10. Transition tools (ExitPlanMode, ExitGoalMode).
    if let Some(prep) = check_transition_tools(config, tool_call, mode) {
        return prep;
    }

    // 10. Plan-mode helper approvals (e.g. writing the active plan file).
    if let Some(prep) = check_plan_file_write(config, tool_call, arguments) {
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

/// Section 1 — plan-mode hard guard. Returns `Some(Deny)` if the plan-mode guard
/// rejects the call, or `None` to continue the pipeline.
fn check_plan_guard(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> Option<PermissionPreparation> {
    let plan_mode = config.plan_mode.read().ok()?;
    if plan_mode.is_active() {
        match check_plan_mode_guard(
            &plan_mode,
            config.workspace_root.as_deref(),
            &tool_call.name,
            arguments,
        ) {
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
    mode: PermissionMode,
) -> Option<PermissionPreparation> {
    if tool_call.name.as_ref() == "ExitPlanMode" {
        if exit_plan_mode_has_reviewable_plan(config)
            && serde_json::from_str::<serde_json::Value>(&tool_call.raw_arguments)
                .ok()
                .and_then(|v| prevalidate_exit_plan_mode(&v).ok())
                .is_some()
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
        return Some(PermissionPreparation::Ask {
            operation: PermissionOperation::GoalTransition,
            subject: "Start reviewed goal".to_owned(),
            session_scope: None,
            prefix_rule: None,
        });
    }

    None
}

/// Section 10 — plan-mode helper approvals (Write/Edit to the active plan file).
fn check_plan_file_write(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> Option<PermissionPreparation> {
    if !matches!(tool_call.name.as_ref(), "Write" | "Edit") {
        return None;
    }
    let plan_mode = config.plan_mode.read().ok()?;
    if let Some(path) = arguments.get("path").and_then(|v| v.as_str())
        && plan_mode.is_active()
        && is_active_plan_file_path(&plan_mode, config.workspace_root.as_deref(), path)
    {
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

#[allow(clippy::too_many_arguments)]
async fn resolve_approval(
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
    operation: PermissionOperation,
    subject: String,
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
    emitter: &mut impl EventPublisher,
    cancel_token: &CancellationToken,
) -> Option<ToolResult> {
    let mut approval_arguments = arguments.clone();
    // For plan transitions, inject the plan file content so the TUI can
    // render it inside the approval dialog.
    if operation == PermissionOperation::PlanTransition
        && let Ok(plan_mode) = config.plan_mode.read()
        && let Ok(Some(plan_data)) = plan_mode.data()
        && let Some(obj) = approval_arguments.as_object_mut()
    {
        obj.insert(
            "plan_content".to_string(),
            serde_json::Value::String(plan_data.content.clone()),
        );
        obj.insert(
            "plan_path".to_string(),
            serde_json::Value::String(plan_data.path.display().to_string()),
        );
    }
    let suggestions = parse_plan_suggestions(arguments);
    let request = ApprovalRequest {
        turn,
        id: tool_call.id.to_string(),
        operation,
        subject: subject.clone(),
        arguments: approval_arguments,
        session_scope: session_scope.clone(),
        prefix_rule: prefix_rule.clone(),
        suggestions: suggestions.clone(),
    };
    emitter.emit(AgentEvent::ApprovalRequested {
        turn: request.turn,
        id: request.id.clone(),
        operation: request.operation,
        subject: request.subject.clone(),
        arguments: request.arguments.clone(),
        session_scope: request.session_scope.clone(),
        prefix_rule: request.prefix_rule.clone(),
        suggestions,
    });
    let decision = if let Some(handler) = &config.approval_handler {
        handler(&request)
    } else if let Some(handler) = &config.async_approval_handler {
        tokio::select! {
            biased;
            () = cancel_token.cancelled() => return Some(cancelled_tool_result()),
            decision = handler(request.clone()) => decision,
        }
    } else {
        return Some(permission_error(operation, &subject, "approval required"));
    };
    match decision {
        PermissionApprovalDecision::AllowOnce => None,
        PermissionApprovalDecision::AllowForSession => {
            // Layer 1: record each narrow key (exact canonical command/cwd,
            // exact file path/op). With no derived scope this degrades to a
            // no-op AllowOnce — it never creates a tool-name wildcard.
            if let Some(scope) = &session_scope
                && let Ok(mut set) = config.session_approvals.lock()
            {
                scope.record(&mut set);
            }
            None
        }
        PermissionApprovalDecision::AllowForPrefix => {
            if let Some(rule) = &prefix_rule
                && !ApprovalRuleStore::is_would_approve_all(&rule.prefix)
            {
                let should_save = if let Ok(mut store) = config.prefix_approval_rules.lock() {
                    let was_new = !store.prefix_rules.iter().any(|r| r.prefix == rule.prefix);
                    store.insert(rule.clone());
                    was_new
                } else {
                    false
                };
                if should_save {
                    let _ = config.save_prefix_approval_rules();
                }
            }
            None
        }
        PermissionApprovalDecision::Reject => {
            // Review feedback is delivered via the review-feedback side-channel.
            if matches!(tool_call.name.as_ref(), "ExitPlanMode" | "ExitGoalMode")
                && let Some(feedback) = config
                    .plan_review_feedback
                    .lock()
                    .ok()
                    .and_then(|mut m| m.remove(tool_call.id.as_ref()))
            {
                let target = if tool_call.name.as_ref() == "ExitGoalMode" {
                    "Goal mode"
                } else {
                    "Plan mode"
                };
                return Some(ToolResult::ok(format!(
                    "User requested revisions. {target} remains active.\n\nFeedback: {feedback}"
                )));
            }
            Some(permission_error(operation, &subject, "approval denied"))
        }
    }
}

/// Extract preset revision suggestions from `ExitPlanMode`/`ExitGoalMode` arguments.
fn parse_plan_suggestions(arguments: &serde_json::Value) -> Vec<crate::PlanSuggestion> {
    arguments
        .get("suggestions")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let label = item.get("label")?.as_str()?.to_owned();
                    let description = item
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&label)
                        .to_owned();
                    let feedback = item
                        .get("feedback")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| Some(description.clone()));
                    Some(crate::PlanSuggestion {
                        label,
                        description,
                        feedback,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn permission_error(
    operation: PermissionOperation,
    subject: &str,
    prefix: &'static str,
) -> ToolResult {
    let noun = match operation {
        PermissionOperation::FileRead => "file read",
        PermissionOperation::FileWrite => "file write",
        PermissionOperation::Shell => "shell",
        PermissionOperation::Tool => "tool",
        PermissionOperation::UserQuestion => "user question",
        PermissionOperation::PlanTransition => "plan transition",
        PermissionOperation::GoalTransition => "goal transition",
    };
    ToolResult::error(format!("{prefix} for {noun}: {subject}"))
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
fn approval_scope_for_tool_call(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    // Review transitions and dangerous commands never offer scope/prefix.
    if matches!(tool_call.name.as_ref(), "ExitPlanMode" | "ExitGoalMode") {
        return (None, None);
    }
    match tool_call.name.as_ref() {
        "Bash" => bash_approval_scope(config, arguments),
        "Write" => {
            let (scope, _) =
                file_write_approval_scope(config, arguments, FileWriteApprovalOperation::Write);
            (scope, None)
        }
        "Edit" => {
            let (scope, _) =
                file_write_approval_scope(config, arguments, FileWriteApprovalOperation::Edit);
            (scope, None)
        }
        _ => tool_approval_scope(config, &tool_call.name),
    }
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

/// Build the session scope for a Write/Edit call. Returns no prefix rule.
fn file_write_approval_scope(
    config: &AgentConfig,
    arguments: &serde_json::Value,
    operation: FileWriteApprovalOperation,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str) else {
        return (None, None);
    };
    if raw_path.trim().is_empty() {
        return (None, None);
    }
    let workspace = workspace_key_root(config);
    let Some(workspace_root) = config.workspace_root.as_deref() else {
        return (None, None);
    };
    let resolved = if std::path::Path::new(raw_path).is_absolute() {
        std::path::PathBuf::from(raw_path)
    } else {
        workspace_root.join(raw_path)
    };
    let normalized = normalize_path(&resolved);
    if !normalized.starts_with(workspace_root) {
        return (None, None);
    }
    let path = normalized.display().to_string();
    let key = SessionApprovalKey::FileWrite {
        workspace: workspace.clone(),
        path: path.clone(),
        operation,
    };
    let (verb, label) = match operation {
        FileWriteApprovalOperation::Write => {
            ("writes to", "Approve writes to this file for this session")
        }
        FileWriteApprovalOperation::Edit => {
            ("edits to", "Approve edits to this file for this session")
        }
    };
    let scope = SessionApprovalScope {
        keys: vec![key],
        label: label.to_owned(),
        detail: format!("File ({verb}): {path}"),
    };
    (Some(scope), None)
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
