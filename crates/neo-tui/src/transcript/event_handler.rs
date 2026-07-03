use std::borrow::Borrow;

use neo_agent_core::{
    AgentEvent, AgentToolCall, PermissionOperation, PlanSuggestion, ShellCommandOrigin,
    ShellCommandOutcome, ToolResult,
};

use crate::shell::ToolStatusKind;
use crate::transcript::ShellRunComponent;
use crate::transcript::TranscriptEntry;
use crate::transcript::entry::GoalCardKind;

use super::pane::{AbsorbedToolKind, TranscriptPane};

impl TranscriptPane {
    pub fn apply_agent_event<E>(&mut self, event: E)
    where
        E: Borrow<AgentEvent>,
    {
        let event = event.borrow();
        if self.apply_message_event(event) {
            return;
        }
        if self.apply_thinking_event(event) {
            return;
        }
        if self.apply_delegate_event(event) {
            return;
        }
        if self.apply_tool_event(event) {
            return;
        }
        if self.apply_queue_event(event) {
            return;
        }
        if self.apply_compaction_event(event) {
            return;
        }
        self.apply_skill_goal_event(event);
    }

    fn apply_message_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::MessageStarted { .. } => {
                self.mark_dirty();
                true
            }
            AgentEvent::TextDelta { text, .. } => {
                self.append_assistant_delta(text);
                true
            }
            AgentEvent::MessageFinished {
                turn,
                stop_reason: neo_agent_core::StopReason::Error,
                ..
            } => {
                self.mark_unfinished_tools_for_turn(
                    *turn,
                    ToolStatusKind::Failed,
                    "Turn ended before this tool executed".to_owned(),
                );
                self.finish_active_text_blocks();
                true
            }
            AgentEvent::TurnFinished {
                turn,
                stop_reason: neo_agent_core::StopReason::Error,
            } => {
                self.mark_unfinished_tools_for_turn(
                    *turn,
                    ToolStatusKind::Failed,
                    "Turn ended before this tool executed".to_owned(),
                );
                self.finish_active_text_blocks();
                true
            }
            AgentEvent::TurnFinished {
                turn,
                stop_reason: neo_agent_core::StopReason::Cancelled,
            } => {
                self.mark_unfinished_tools_for_turn(
                    *turn,
                    ToolStatusKind::Cancelled,
                    "Turn cancelled before this tool executed".to_owned(),
                );
                self.finish_active_text_blocks();
                true
            }
            AgentEvent::MessageFinished { .. } | AgentEvent::TurnFinished { .. } => {
                self.finish_active_text_blocks();
                true
            }
            _ => false,
        }
    }

    fn apply_thinking_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::ThinkingStarted { .. } => {
                self.start_thinking_block();
                true
            }
            AgentEvent::ThinkingDelta { text, .. } => {
                self.append_thinking_block(text);
                true
            }
            AgentEvent::ThinkingFinished { .. } => {
                self.finish_thinking_block();
                true
            }
            _ => false,
        }
    }

    fn apply_delegate_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::DelegateStarted { turn, agent }
            | AgentEvent::DelegateUpdated { turn, agent }
            | AgentEvent::DelegateFinished { turn, agent } => {
                self.finish_active_text_blocks();
                self.transcript.upsert_delegate(*turn, agent.clone());
                self.record_delegate_absorption_target(
                    *turn,
                    AbsorbedToolKind::Delegate,
                    agent.id.as_str(),
                );
                self.mark_dirty();
                true
            }
            AgentEvent::DelegateSwarmStarted { turn, swarm }
            | AgentEvent::DelegateSwarmUpdated { turn, swarm }
            | AgentEvent::DelegateSwarmFinished { turn, swarm } => {
                self.finish_active_text_blocks();
                self.transcript.upsert_delegate_swarm(swarm.clone());
                self.record_delegate_absorption_target(
                    *turn,
                    AbsorbedToolKind::DelegateSwarm,
                    &swarm.swarm_id,
                );
                self.mark_dirty();
                true
            }
            AgentEvent::WorkflowStarted { workflow, .. }
            | AgentEvent::WorkflowUpdated { workflow, .. }
            | AgentEvent::WorkflowFinished { workflow, .. } => {
                self.finish_active_text_blocks();
                self.transcript.upsert_workflow(workflow.clone());
                self.mark_dirty();
                true
            }
            _ => false,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn apply_tool_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::ToolCallStarted { turn, id, name } => {
                self.start_tool_call(*turn, id, name.clone());
                true
            }
            AgentEvent::ToolCallArgumentsDelta {
                id, json_fragment, ..
            } => {
                self.append_tool_call_arguments(id, json_fragment);
                true
            }
            AgentEvent::ToolCallFinished { turn, tool_call } => {
                self.finish_tool_call(*turn, tool_call.clone());
                true
            }
            AgentEvent::ToolExecutionStarted {
                turn,
                id,
                name,
                arguments,
            } => {
                self.start_tool_execution(*turn, id, name.clone(), arguments);
                true
            }
            AgentEvent::ApprovalRequested {
                id,
                operation,
                subject,
                arguments,
                session_scope,
                prefix_rule,
                suggestions,
                ..
            } => {
                let mut session_label = session_scope
                    .as_ref()
                    .filter(|scope| !scope.is_empty())
                    .map(|scope| scope.label.clone());
                // Tool and shell approvals always offer a session-approval option,
                // even when no explicit session scope was derived. Use the default
                // label so the modal keeps its four-option layout.
                if session_label.is_none()
                    && matches!(
                        operation,
                        PermissionOperation::Tool | PermissionOperation::Shell
                    )
                {
                    session_label = Some("Approve for this session".to_owned());
                }
                let prefix_label = prefix_rule
                    .as_ref()
                    .map(|rule| format!("Approve commands starting with {}", rule.label));
                self.request_approval(
                    id.clone(),
                    *operation,
                    subject,
                    arguments,
                    session_label,
                    prefix_label,
                    suggestions.clone(),
                );
                true
            }
            AgentEvent::ToolExecutionUpdate {
                turn,
                id,
                name,
                partial_result,
            } => {
                if self.transcript.has_shell_run(id) {
                    self.update_shell_run(id, partial_result.clone());
                } else {
                    self.remember_tool_call(*turn, id, name);
                    self.update_tool_execution(id, name.clone(), partial_result.clone());
                }
                true
            }
            AgentEvent::ToolExecutionFinished {
                turn,
                id,
                name,
                result,
            } => {
                self.finish_tool_execution(*turn, id.clone(), name.clone(), result.clone());
                true
            }
            AgentEvent::ShellCommandStarted {
                id,
                command,
                cwd,
                origin,
                ..
            } => {
                match origin {
                    ShellCommandOrigin::ModelBashTool => self.start_shell_command(id, command, cwd),
                    ShellCommandOrigin::UserShellMode => self.start_user_shell_command(id, command),
                }
                true
            }
            AgentEvent::ShellCommandFinished {
                id,
                exit_code,
                signal,
                stdout,
                stderr,
                truncated,
                origin,
                outcome,
                ..
            } => {
                match origin {
                    ShellCommandOrigin::ModelBashTool => {
                        self.finish_shell_command(
                            id.clone(),
                            *exit_code,
                            *signal,
                            stdout,
                            stderr,
                            *truncated,
                            outcome,
                        );
                    }
                    ShellCommandOrigin::UserShellMode => {
                        self.finish_user_shell_command(
                            id,
                            *exit_code,
                            *signal,
                            stdout,
                            stderr,
                            *truncated,
                            outcome.clone(),
                        );
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn apply_queue_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            // Queue events are now rendered in the dedicated Pending Input
            // Preview panel above the composer, not as transcript status lines.
            AgentEvent::SteeringQueued { .. }
            | AgentEvent::FollowUpQueued { .. }
            | AgentEvent::QueueDrained { .. } => true,
            AgentEvent::Error {
                turn,
                message,
                code,
                retry_after,
            } => {
                use crate::transcript::entry::StatusSeverity;
                self.mark_unfinished_tools_for_turn(
                    *turn,
                    ToolStatusKind::Failed,
                    format!("Turn ended before this tool executed: {message}"),
                );

                let severity = match code.as_deref() {
                    Some(
                        "provider.rate_limit" | "provider.server_error" | "provider.network_error",
                    ) => StatusSeverity::Warning,
                    _ => StatusSeverity::Error,
                };

                let text = match (code.as_deref(), retry_after) {
                    (Some("provider.rate_limit"), Some(secs)) => {
                        format!("⚠ Rate Limited — retry in {secs}s")
                    }
                    (Some(c), _) => {
                        let info = neo_agent_core::error_info(c);
                        if let Some(action) = info.action {
                            format!("✗ {} — {}", info.title, action)
                        } else {
                            format!("Error: {message}")
                        }
                    }
                    _ => format!("Error: {message}"),
                };

                self.push_status_with_severity(text, severity);
                true
            }
            AgentEvent::RunFinished { turn, stop_reason } => {
                if let Some(notice) = run_finished_notice(*turn, *stop_reason) {
                    self.push_status(notice);
                }
                true
            }
            _ => false,
        }
    }

    fn apply_compaction_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::CompactionStarted {
                tokens_before,
                message_count,
                ..
            } => {
                self.upsert_compaction(
                    Some(neo_agent_core::CompactionPhase::Estimating),
                    0,
                    *message_count,
                    *tokens_before,
                    0,
                );
                true
            }
            AgentEvent::CompactionProgress { phase, percent } => {
                self.update_compaction_progress(*phase, (*percent).min(99));
                true
            }
            AgentEvent::CompactionApplied { summary } => {
                self.upsert_compaction(
                    Some(neo_agent_core::CompactionPhase::Applying),
                    100,
                    summary.first_kept_message_index,
                    summary.tokens_before,
                    summary.tokens_after,
                );
                true
            }
            _ => false,
        }
    }

    fn apply_skill_goal_event(&mut self, event: &AgentEvent) {
        if let AgentEvent::SkillActivated { name, body, .. } = event {
            self.push_skill_activation(name.clone(), body.clone());
            return;
        }
        self.apply_goal_event(event);
    }

    fn apply_goal_event(&mut self, event: &AgentEvent) {
        if self.apply_goal_state_event(event) {
            return;
        }
        self.apply_goal_terminal_event(event);
    }

    fn apply_goal_state_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::GoalStarted { objective, .. } => {
                self.push_goal_state_card(GoalCardKind::Started, objective);
            }
            AgentEvent::GoalPaused { objective, .. } => {
                self.push_goal_state_card(GoalCardKind::Paused, objective);
            }
            AgentEvent::GoalResumed { objective, .. } => {
                self.push_goal_state_card(GoalCardKind::Resumed, objective);
            }
            _ => return false,
        }
        true
    }

    fn apply_goal_terminal_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::GoalBlocked { .. } => self.push_goal_blocked_card(event),
            AgentEvent::GoalFinished { .. } => self.push_goal_finished_card(event),
            _ => {}
        }
    }

    fn push_goal_blocked_card(&mut self, event: &AgentEvent) {
        let AgentEvent::GoalBlocked {
            objective, reason, ..
        } = event
        else {
            return;
        };
        self.push_goal_card(
            GoalCardKind::Blocked,
            objective.clone(),
            Some(reason.clone()),
            None,
        );
    }

    fn push_goal_finished_card(&mut self, event: &AgentEvent) {
        let AgentEvent::GoalFinished {
            objective,
            outcome,
            turn,
            ..
        } = event
        else {
            return;
        };
        self.push_goal_card(
            GoalCardKind::Finished,
            objective.clone(),
            Some(outcome.clone()),
            Some(*turn),
        );
    }

    fn push_goal_state_card(&mut self, kind: GoalCardKind, objective: &str) {
        self.push_goal_card(kind, objective.to_owned(), None, None);
    }

    fn start_thinking_block(&mut self) {
        self.finish_assistant_message();
        self.transcript.start_thinking();
        self.apply_expand_state_to_active_thinking();
        self.mark_dirty();
    }

    fn append_thinking_block(&mut self, text: &str) {
        self.transcript.append_thinking_delta(text);
        self.mark_dirty();
    }

    fn finish_thinking_block(&mut self) {
        self.transcript.finish_thinking();
        self.mark_dirty();
    }

    fn append_tool_call_arguments(&mut self, id: &str, json_fragment: &str) {
        let arguments = self.streaming_tool_args.entry(id.to_owned()).or_default();
        arguments.push_str(json_fragment);
        if let Some(tool) = self.transcript.tool_mut(id) {
            tool.update_call(Some(arguments.clone()));
            self.mark_dirty();
        }
    }

    fn finish_tool_call(&mut self, turn: u32, tool_call: AgentToolCall) {
        let arguments = tool_call.raw_arguments.to_string();
        self.streaming_tool_args
            .insert(tool_call.id.to_string(), arguments.clone());
        self.remember_tool_call(turn, &tool_call.id, &tool_call.name);
        if is_skill_tool(&tool_call.name) {
            return;
        }
        self.upsert_tool(
            &tool_call.id,
            tool_call.name.to_string(),
            Some(arguments),
            ToolStatusKind::Pending,
        );
        self.mark_dirty();
    }

    fn start_tool_call(&mut self, turn: u32, id: &str, name: String) {
        self.remember_tool_call(turn, id, &name);
        if is_skill_tool(&name) {
            return;
        }
        self.upsert_tool(id, name, None, ToolStatusKind::Pending);
        self.mark_dirty();
    }

    fn start_tool_execution(
        &mut self,
        turn: u32,
        id: &str,
        name: String,
        arguments: &serde_json::Value,
    ) {
        let arguments = self
            .streaming_tool_args
            .get(id)
            .cloned()
            .unwrap_or_else(|| arguments.to_string());
        self.remember_tool_call(turn, id, &name);
        if is_skill_tool(&name) {
            return;
        }
        self.upsert_tool(id, name, Some(arguments), ToolStatusKind::Running);
        self.mark_dirty();
    }

    #[allow(clippy::too_many_arguments)]
    fn request_approval(
        &mut self,
        id: String,
        operation: PermissionOperation,
        subject: &str,
        arguments: &serde_json::Value,
        session_option_label: Option<String>,
        prefix_option_label: Option<String>,
        suggestions: Vec<PlanSuggestion>,
    ) {
        self.upsert_approval(
            id,
            operation,
            subject,
            arguments,
            session_option_label,
            prefix_option_label,
            suggestions,
        );
        self.mark_dirty();
    }

    fn update_tool_execution(&mut self, id: &str, name: String, partial_result: ToolResult) {
        self.upsert_tool(id, name, None, ToolStatusKind::Running);
        if let Some(tool) = self.transcript.tool_mut(id) {
            tool.append_live_output(partial_result.content);
        }
        self.mark_dirty();
    }

    fn update_shell_run(&mut self, id: &str, partial_result: ToolResult) {
        if let Some(shell_run) = self.transcript.shell_run_mut(id) {
            shell_run.append_live_output(partial_result.content);
        }
        self.mark_dirty();
    }

    fn finish_tool_execution(&mut self, turn: u32, id: String, name: String, result: ToolResult) {
        self.remember_tool_call(turn, &id, &name);
        if is_skill_tool(&name) {
            self.streaming_tool_args.remove(&id);
            self.completed_tool_result_ids.push(id);
            return;
        }
        let tool_name = name.clone();
        self.upsert_tool(&id, name, None, ToolStatusKind::Running);
        self.streaming_tool_args.remove(&id);
        let is_error = result.is_error;
        let details_for_check = result.details.clone();
        if let Some(tool) = self.transcript.tool_mut(&id) {
            let details = result.details;
            let exit_code = details
                .as_ref()
                .and_then(|details| details.get("exit_code"))
                .and_then(serde_json::Value::as_i64)
                .and_then(|code| i32::try_from(code).ok());
            let content = if tool_name == "Bash" && result.content.is_empty() {
                details
                    .as_ref()
                    .and_then(shell_detail_from_tool_result_details)
                    .unwrap_or(result.content)
            } else {
                result.content
            };
            tool.set_result(Some(content), details, is_error, exit_code);
        }
        self.reconcile_delegate_tool_result(
            turn,
            &id,
            &tool_name,
            is_error,
            details_for_check.as_ref(),
        );
        self.completed_tool_result_ids.push(id);
        self.mark_dirty();
    }

    fn start_shell_command(&mut self, id: &str, command: &str, cwd: &std::path::Path) {
        self.upsert_tool(
            id,
            "Bash".to_owned(),
            Some(format!("{command} ({})", cwd.display())),
            ToolStatusKind::Running,
        );
        self.mark_dirty();
    }

    fn start_user_shell_command(&mut self, id: &str, command: &str) {
        if !self.transcript.has_shell_run(id) {
            self.transcript
                .push_shell_run(ShellRunComponent::running(id, command));
        }
        self.mark_dirty();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_shell_command(
        &mut self,
        id: String,
        exit_code: Option<i32>,
        signal: Option<i32>,
        stdout: &str,
        stderr: &str,
        truncated: bool,
        outcome: &ShellCommandOutcome,
    ) {
        let detail = shell_finished_detail(exit_code, signal, stdout, stderr, truncated, outcome);
        self.upsert_tool(&id, "Bash".to_owned(), None, ToolStatusKind::Running);
        if let Some(tool) = self.transcript.tool_mut(&id) {
            let is_error = exit_code != Some(0)
                || !matches!(
                    outcome,
                    ShellCommandOutcome::Completed | ShellCommandOutcome::Backgrounded { .. }
                );
            tool.set_result(Some(detail), None, is_error, exit_code);
        }
        self.completed_tool_result_ids.push(id);
        self.mark_dirty();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_user_shell_command(
        &mut self,
        id: &str,
        exit_code: Option<i32>,
        signal: Option<i32>,
        stdout: &str,
        stderr: &str,
        truncated: bool,
        outcome: ShellCommandOutcome,
    ) {
        if let Some(shell_run) = self.transcript.shell_run_mut(id) {
            shell_run.finish(stdout, stderr, exit_code, signal, outcome, truncated);
        } else {
            self.transcript.push_shell_run(ShellRunComponent::finished(
                id, "", stdout, stderr, exit_code, signal, outcome, truncated,
            ));
        }
        self.mark_dirty();
    }

    fn push_skill_activation(&mut self, name: String, body: String) {
        self.push_transcript(TranscriptEntry::skill_activated(vec![name], body));
    }

    fn push_goal_card(
        &mut self,
        kind: GoalCardKind,
        objective: String,
        detail: Option<String>,
        turns: Option<u32>,
    ) {
        self.push_transcript(TranscriptEntry::goal_card(kind, objective, detail, turns));
    }
}

fn shell_finished_detail(
    exit_code: Option<i32>,
    signal: Option<i32>,
    stdout: &str,
    stderr: &str,
    truncated: bool,
    outcome: &ShellCommandOutcome,
) -> String {
    let mut detail = String::new();
    for line in super::shell_run::finished_plain_lines(
        stdout, stderr, exit_code, signal, outcome, truncated,
    ) {
        if !detail.ends_with('\n') && !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(&line);
    }
    detail
}

fn shell_detail_from_tool_result_details(details: &serde_json::Value) -> Option<String> {
    let stdout = details
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let stderr = details
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let exit_code = details
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
        .and_then(|code| i32::try_from(code).ok());
    let signal = details
        .get("signal")
        .and_then(serde_json::Value::as_i64)
        .and_then(|sig| i32::try_from(sig).ok());
    let truncated = details
        .get("truncated")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let outcome = shell_outcome_from_tool_result_details(details);
    let detail = shell_finished_detail(exit_code, signal, stdout, stderr, truncated, &outcome);
    (!detail.is_empty()).then_some(detail)
}

fn shell_outcome_from_tool_result_details(details: &serde_json::Value) -> ShellCommandOutcome {
    match details.get("outcome").and_then(serde_json::Value::as_str) {
        Some("cancelled") => ShellCommandOutcome::Cancelled,
        Some("timed_out") => ShellCommandOutcome::TimedOut,
        Some("backgrounded") => {
            let task_id = details
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            ShellCommandOutcome::Backgrounded {
                task_id: task_id.into(),
            }
        }
        _ => ShellCommandOutcome::Completed,
    }
}

fn run_finished_notice(turn: u32, stop_reason: neo_agent_core::StopReason) -> Option<String> {
    match stop_reason {
        neo_agent_core::StopReason::MaxTokens => Some(format!(
            "Run stopped after turn {turn}: response hit the output length cap (max_tokens). \
             Raise [models.<alias>].max_output_tokens or [runtime].max_tokens to continue."
        )),
        neo_agent_core::StopReason::Error => {
            Some(format!("Run stopped after turn {turn}: runtime error."))
        }
        neo_agent_core::StopReason::Cancelled => {
            Some(format!("Run stopped after turn {turn}: cancelled."))
        }
        neo_agent_core::StopReason::EndTurn | neo_agent_core::StopReason::ToolUse => None,
    }
}

/// Returns `true` when the tool name belongs to the model-invoked `Skill`
/// tool. These tool calls are rendered as `SkillActivation` cards instead of
/// the standard tool-call card.
fn is_skill_tool(name: &str) -> bool {
    name == "Skill"
}
