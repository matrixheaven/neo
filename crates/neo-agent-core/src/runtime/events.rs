//! Event emission infrastructure — `EventEmitter`, `EventPublisher`,
//! `EventSink`, and the `emit_*` helpers that translate tool results into
//! `AgentEvent` values.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    AgentEvent, AgentRuntimeError, AgentToolCall, QueueKind, ShellCommandOrigin,
    ShellCommandOutcome, TodoEventData, ToolContext, ToolResult, ToolUpdateCallback,
};
use crate::tools::{ShellAdmissionCallback, ShellAdmissionEvent};

use super::config::AgentConfig;
use super::context::AgentContext;

pub(super) struct EventEmitter {
    pub(super) sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
    pub(super) context: AgentContext,
    pub(super) last_context_window_tokens: Option<u32>,
}

impl EventEmitter {
    pub(super) fn new(
        sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
        context: AgentContext,
    ) -> Self {
        Self {
            sender,
            context,
            last_context_window_tokens: None,
        }
    }

    pub(super) fn emit(&mut self, event: AgentEvent) {
        Self::apply_to_context(&mut self.context, &event);
        let _ = self.sender.send(Ok(event));
    }

    pub(super) fn sink(&self) -> EventSink {
        EventSink {
            sender: self.sender.clone(),
        }
    }

    pub(super) fn send_error(&mut self, err: AgentRuntimeError) -> Result<(), AgentRuntimeError> {
        self.sender
            .send(Err(err))
            .map_err(|_| AgentRuntimeError::Cancelled)
    }

    pub(super) fn apply_to_context(context: &mut AgentContext, event: &AgentEvent) {
        match event {
            AgentEvent::MessageAppended { message } => context.append_message(message.clone()),
            AgentEvent::TurnFinished { turn, .. } => {
                // Same invariant as replay: even live cancelled turns must not
                // poison the context used by subsequent user prompts.
                context.turns = context.turns.max(*turn);
            }
            AgentEvent::SteeringQueued { message } => {
                context.queue_steering_message(message.clone());
            }
            AgentEvent::FollowUpQueued { message } => {
                context.queue_follow_up_message(message.clone());
            }
            AgentEvent::QueueDrained { kind, count } => match kind {
                QueueKind::Steering => {
                    let drain_count = (*count).min(context.steering_queue.len());
                    context.steering_queue.drain(0..drain_count);
                }
                QueueKind::FollowUp => {
                    let drain_count = (*count).min(context.follow_up_queue.len());
                    context.follow_up_queue.drain(0..drain_count);
                }
            },
            AgentEvent::CompactionApplied { summary } => {
                context.apply_compaction(summary.clone());
            }
            // Live conversion mirrors replay: the epoch pins one instruction
            // message and updates agent-local visibility.
            AgentEvent::InstructionEpoch { epoch } => {
                context.apply_instruction_epoch(epoch);
            }
            AgentEvent::PlanModeEntered { id, .. } => {
                context.plan_mode_active = true;
                context.plan_mode_id = Some(id.clone());
            }
            AgentEvent::PlanModeExited { .. } => {
                context.plan_mode_active = false;
            }
            AgentEvent::PlanUpdated { enabled, .. } => {
                context.plan_mode_active = *enabled;
            }
            AgentEvent::TodoUpdated { todos, .. } => {
                context.todos.clone_from(todos);
            }
            AgentEvent::RetryScheduled { .. }
            | AgentEvent::RetryStarted { .. }
            | AgentEvent::RetryResumed { .. }
            | AgentEvent::RetrySucceeded { .. }
            | AgentEvent::RetryExhausted { .. } => {}
            _ => {}
        }
    }
}

pub(super) trait EventPublisher {
    fn emit(&mut self, event: AgentEvent);
}

impl EventPublisher for EventEmitter {
    fn emit(&mut self, event: AgentEvent) {
        Self::emit(self, event);
    }
}

#[derive(Clone)]
pub(super) struct EventSink {
    pub(super) sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
}

impl EventSink {
    /// Emit an event by value without needing `&mut self`.
    pub(super) fn emit_event(&self, event: AgentEvent) {
        let _ = self.sender.send(Ok(event));
    }
}

impl EventPublisher for EventSink {
    fn emit(&mut self, event: AgentEvent) {
        self.emit_event(event);
    }
}

/// Build the shell-admission callback for Bash / Terminal Start after permission.
/// Shares one `Arc` of prepared arguments so waiters never deep-copy command JSON.
pub(super) fn make_shell_admission_callback(
    sink: EventSink,
    turn: u32,
    id: String,
    name: String,
    arguments: Arc<serde_json::Value>,
    bash_display_cwd: PathBuf,
) -> ShellAdmissionCallback {
    Arc::new(move |event| match event {
        ShellAdmissionEvent::Queued => {
            sink.emit_event(AgentEvent::ToolExecutionQueued {
                turn,
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.as_ref().clone(),
            });
        }
        ShellAdmissionEvent::Position { position, waiting } => {
            sink.emit_event(AgentEvent::ToolExecutionQueueUpdated {
                turn,
                id: id.clone(),
                position,
                waiting_ms: u64::try_from(waiting.as_millis()).unwrap_or(u64::MAX),
            });
        }
        ShellAdmissionEvent::Started => {
            sink.emit_event(AgentEvent::ToolExecutionStarted {
                turn,
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.as_ref().clone(),
            });
            if name == "Bash"
                && let Some(command) = arguments
                    .get("command")
                    .and_then(serde_json::Value::as_str)
            {
                sink.emit_event(AgentEvent::ShellCommandStarted {
                    turn,
                    id: id.clone(),
                    command: command.to_owned(),
                    cwd: bash_display_cwd.clone(),
                    origin: ShellCommandOrigin::ModelBashTool,
                });
            }
        }
    })
}

/// Build a `ToolEventCallback` that forwards structured `AgentEvent` values
/// through the event sink. Used by `tool_dispatch` so delegate/swarm tools can
/// emit lifecycle events without holding a mutable emitter reference.
pub(super) fn make_tool_event_callback(sink: EventSink) -> crate::tools::ToolEventCallback {
    std::sync::Arc::new(move |event: AgentEvent| {
        sink.emit_event(event.without_delegate_prior_messages());
    })
}

pub(super) fn emit_todo_event(
    turn: u32,
    config: &AgentConfig,
    tool_name: &str,
    result: &ToolResult,
    emitter: &mut EventEmitter,
) {
    if tool_name != "TodoList" || result.is_error {
        return;
    }
    let Some(details) = &result.details else {
        return;
    };
    let Some(todos_val) = details.get("todos") else {
        return;
    };
    let Ok(todos) = serde_json::from_value::<Vec<TodoEventData>>(todos_val.clone()) else {
        return;
    };
    if let Ok(mut shared) = config.todos.lock() {
        shared.clone_from(&todos);
    }
    emitter.emit(AgentEvent::TodoUpdated { turn, todos });
}

pub(super) fn emit_goal_event_from_result(
    turn: u32,
    tool_name: &str,
    result: &ToolResult,
    emitter: &mut EventEmitter,
) {
    if result.is_error {
        return;
    }
    let Some(details) = &result.details else {
        return;
    };
    if details.get("kind").and_then(serde_json::Value::as_str) != Some("goal") {
        return;
    }
    let Some(objective) = details
        .get("objective")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return;
    };
    match (
        tool_name,
        details.get("event").and_then(serde_json::Value::as_str),
        details.get("status").and_then(serde_json::Value::as_str),
    ) {
        ("StartGoal" | "ExitGoalMode", Some("started"), _) => {
            emitter.emit(AgentEvent::GoalStarted { turn, objective });
        }
        ("UpdateGoalStatus", Some("updated"), Some("paused")) => {
            emitter.emit(AgentEvent::GoalPaused { turn, objective });
        }
        ("UpdateGoalStatus", Some("updated"), Some("active")) => {
            emitter.emit(AgentEvent::GoalResumed { turn, objective });
        }
        ("UpdateGoalStatus", Some("updated"), Some("blocked")) => {
            let reason = details
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("blocked")
                .to_owned();
            emitter.emit(AgentEvent::GoalBlocked {
                turn,
                objective,
                reason,
            });
        }
        ("UpdateGoalStatus", Some("updated"), Some("complete")) => {
            emitter.emit(AgentEvent::GoalFinished {
                turn,
                objective,
                outcome: "complete".to_owned(),
            });
            if let Some(next_objective) = details
                .get("next_objective")
                .and_then(serde_json::Value::as_str)
            {
                emitter.emit(AgentEvent::GoalStarted {
                    turn,
                    objective: next_objective.to_owned(),
                });
            }
        }
        _ => {}
    }
}

pub(super) fn emit_context_window_snapshot(
    emitter: &mut EventEmitter,
    snapshot: &super::context_budget::ContextBudgetSnapshot,
) {
    let used_tokens = u32::try_from(snapshot.projected_tokens).unwrap_or(u32::MAX);
    if emitter.last_context_window_tokens == Some(used_tokens) {
        return;
    }
    emitter.last_context_window_tokens = Some(used_tokens);
    emitter.emit(AgentEvent::ContextWindowUpdated {
        turn: snapshot.turn,
        used_tokens,
        projected_tokens: Some(used_tokens),
        max_tokens: snapshot
            .effective_max_context_tokens
            .map(|tokens| u32::try_from(tokens).unwrap_or(u32::MAX)),
        trigger_tokens: snapshot
            .trigger_tokens
            .map(|tokens| u32::try_from(tokens).unwrap_or(u32::MAX)),
        remaining_tokens: snapshot
            .remaining_to_max
            .map(|tokens| u32::try_from(tokens).unwrap_or(u32::MAX)),
        source: Some(snapshot.source),
    });
}

pub(super) fn emit_shell_finished(
    turn: u32,
    tool_call: &AgentToolCall,
    result: &ToolResult,
    emitter: &mut impl EventPublisher,
) {
    if tool_call.name.as_ref() != "Bash" {
        return;
    }
    let Some(details) = &result.details else {
        return;
    };
    let stdout = details
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let stderr = details
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
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
    let outcome = shell_command_outcome_from_details(details);
    emitter.emit(AgentEvent::ShellCommandFinished {
        turn,
        id: tool_call.id.to_string(),
        exit_code,
        signal,
        stdout,
        stderr,
        truncated,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome,
    });
}

fn shell_command_outcome_from_details(details: &serde_json::Value) -> ShellCommandOutcome {
    match details.get("outcome").and_then(serde_json::Value::as_str) {
        Some("cancelled") => ShellCommandOutcome::Cancelled,
        Some("timed_out") => ShellCommandOutcome::TimedOut,
        Some("resource_limited") => ShellCommandOutcome::ResourceLimited,
        Some("backgrounded") => {
            let task_id = details
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            ShellCommandOutcome::Backgrounded {
                task_id: task_id.into(),
            }
        }
        _ if details.get("kind").and_then(serde_json::Value::as_str) == Some("bash")
            && details.get("status").and_then(serde_json::Value::as_str) == Some("running") =>
        {
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

pub(super) fn emit_terminal_events(
    turn: u32,
    arguments: &serde_json::Value,
    tool_call: &AgentToolCall,
    result: &ToolResult,
    tool_context: &ToolContext,
    emitter: &mut impl EventPublisher,
) {
    if tool_call.name.as_ref() != "Terminal" {
        return;
    }
    let Some(details) = &result.details else {
        return;
    };
    let Some(handle) = details
        .get("handle")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return;
    };
    match arguments.get("mode").and_then(serde_json::Value::as_str) {
        Some("start") => {
            let command = details
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let cols = details
                .get("cols")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(80);
            let rows = details
                .get("rows")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(24);
            emitter.emit(AgentEvent::TerminalSessionStarted {
                turn,
                id: tool_call.id.to_string(),
                handle,
                command,
                cwd: tool_context.workspace_root().to_path_buf(),
                cols,
                rows,
            });
        }
        Some("read") => {
            let output = details
                .get("output")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let truncated = details
                .get("truncated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !output.is_empty() {
                emitter.emit(AgentEvent::TerminalSessionOutput {
                    turn,
                    id: tool_call.id.to_string(),
                    handle: handle.clone(),
                    output,
                    truncated,
                });
            }
            let status = details
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("running");
            if status != "running" {
                let exit_code = details
                    .get("exit_code")
                    .and_then(serde_json::Value::as_i64)
                    .and_then(|code| i32::try_from(code).ok());
                emitter.emit(AgentEvent::TerminalSessionFinished {
                    turn,
                    id: tool_call.id.to_string(),
                    handle,
                    status: status.to_owned(),
                    exit_code,
                });
            }
        }
        Some("stop") => {
            let status = details
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("stopped")
                .to_owned();
            let exit_code = details
                .get("exit_code")
                .and_then(serde_json::Value::as_i64)
                .and_then(|code| i32::try_from(code).ok());
            emitter.emit(AgentEvent::TerminalSessionFinished {
                turn,
                id: tool_call.id.to_string(),
                handle,
                status,
                exit_code,
            });
        }
        _ => {}
    }
}

/// Creates a `ToolUpdateCallback` that emits `ToolExecutionUpdate` events
/// through an `EventSink`. This lets tools (e.g. bash) stream intermediate
/// output that the TUI renders live.
pub(super) fn make_tool_update_callback(
    sink: EventSink,
    turn: u32,
    id: String,
    name: String,
) -> ToolUpdateCallback {
    Arc::new(move |partial: &str| {
        sink.emit_event(AgentEvent::ToolExecutionUpdate {
            turn,
            id: id.clone(),
            name: name.clone(),
            partial_result: ToolResult {
                content: partial.to_owned(),
                is_error: false,
                details: None,
                terminate: false,
            },
        });
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;

    #[test]
    fn complete_goal_result_with_next_goal_emits_finished_then_started() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut emitter = EventEmitter::new(tx, AgentContext::new());
        let result = ToolResult::ok("complete").with_details(json!({
            "kind": "goal",
            "event": "updated",
            "objective": "First goal",
            "status": "complete",
            "next_objective": "Second goal"
        }));

        emit_goal_event_from_result(7, "UpdateGoalStatus", &result, &mut emitter);

        assert_eq!(
            rx.try_recv().expect("finished").expect("event"),
            AgentEvent::GoalFinished {
                turn: 7,
                objective: "First goal".to_owned(),
                outcome: "complete".to_owned(),
            }
        );
        assert_eq!(
            rx.try_recv().expect("started").expect("event"),
            AgentEvent::GoalStarted {
                turn: 7,
                objective: "Second goal".to_owned(),
            }
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn terminal_read_emits_finished_for_empty_natural_exit() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut emitter = EventEmitter::new(tx, AgentContext::new());
        let workspace = tempfile::tempdir().expect("workspace");
        let context = ToolContext::new(workspace.path()).expect("tool context");
        let call = AgentToolCall {
            id: "tool-read".into(),
            name: "Terminal".into(),
            raw_arguments: json!({ "mode": "read" }).to_string().into(),
        };
        let result = ToolResult::ok("").with_details(json!({
            "handle": "terminal-1",
            "status": "completed",
            "exit_code": 0,
            "output": "",
            "truncated": false,
        }));

        emit_terminal_events(
            3,
            &json!({ "mode": "read" }),
            &call,
            &result,
            &context,
            &mut emitter,
        );

        assert_eq!(
            rx.try_recv().expect("finished").expect("event"),
            AgentEvent::TerminalSessionFinished {
                turn: 3,
                id: "tool-read".to_owned(),
                handle: "terminal-1".to_owned(),
                status: "completed".to_owned(),
                exit_code: Some(0),
            }
        );
        assert!(rx.try_recv().is_err());
    }
}
