use neo_agent_core::AgentEvent;

use crate::dialogs::{QuestionDisplayData, QuestionDisplayOption};
use crate::primitive::theme::{ChromeMode, DevelopmentMode, GoalModeStatus};
use crate::widgets::{TodoDisplayItem, TodoDisplayStatus};

use super::context::ContextWindow;
use super::state::NeoChromeState;
use super::stream::StreamUpdate;

impl NeoChromeState {
    pub fn apply_stream_update(&mut self, update: StreamUpdate) {
        match update {
            StreamUpdate::AssistantStarted { .. }
            | StreamUpdate::TextDelta { .. }
            | StreamUpdate::ToolStarted { .. }
            | StreamUpdate::ToolUpdated { .. }
            | StreamUpdate::ToolFinished { .. }
            | StreamUpdate::ThinkingStarted
            | StreamUpdate::ThinkingDelta { .. } => {
                self.mode = ChromeMode::Streaming;
            }
            StreamUpdate::Error { text } => {
                let _ = text;
                self.mode = self.overlay_mode();
            }
            StreamUpdate::TurnFinished | StreamUpdate::RunFinished { .. } => {
                self.mode = self.overlay_mode();
            }
            StreamUpdate::PlanModeChanged { active } => self.set_plan_mode(active),
            StreamUpdate::TodoUpdated { todos } => {
                self.apply_todo_items(todos);
            }
            StreamUpdate::QuestionRequested { id, questions } => {
                self.push_question_overlay(id, questions);
            }
            StreamUpdate::ThinkingFinished => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn apply_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::MessageStarted { .. }
            | AgentEvent::TextDelta { .. }
            | AgentEvent::ThinkingStarted { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ThinkingFinished { .. }
            | AgentEvent::ToolCallStarted { .. }
            | AgentEvent::ToolCallArgumentsDelta { .. }
            | AgentEvent::ToolCallFinished { .. }
            | AgentEvent::ToolExecutionStarted { .. }
            | AgentEvent::ToolExecutionQueued { .. }
            | AgentEvent::ToolExecutionQueueUpdated { .. }
            | AgentEvent::ToolExecutionUpdate { .. }
            | AgentEvent::ToolExecutionFinished { .. }
            | AgentEvent::ShellCommandStarted { .. }
            | AgentEvent::ShellCommandQueued { .. }
            | AgentEvent::ShellCommandQueueUpdated { .. }
            | AgentEvent::ShellCommandFinished { .. }
            | AgentEvent::DelegateStarted { .. }
            | AgentEvent::DelegateUpdated { .. }
            | AgentEvent::DelegateProgressUpdated { .. }
            | AgentEvent::DelegateFinished { .. }
            | AgentEvent::DelegateSwarmStarted { .. }
            | AgentEvent::DelegateSwarmUpdated { .. }
            | AgentEvent::DelegateSwarmProgressUpdated { .. }
            | AgentEvent::DelegateSwarmFinished { .. }
            | AgentEvent::WorkflowStarted { .. }
            | AgentEvent::WorkflowUpdated { .. }
            | AgentEvent::WorkflowFinished { .. }
            | AgentEvent::InstructionEpoch { .. }
            | AgentEvent::RetryScheduled { .. }
            | AgentEvent::RetryStarted { .. }
            | AgentEvent::RetryResumed { .. }
            | AgentEvent::RetrySucceeded { .. } => {
                self.mode = ChromeMode::Streaming;
            }
            AgentEvent::RetryExhausted { turn, .. } => {
                self.retry_exhausted_error_turn = Some(turn);
                self.mode = ChromeMode::Streaming;
            }
            // Live modal is opened by the interactive controller via
            // `push_approval` from the PendingApproval channel — not from this
            // observable event. Passive status only.
            AgentEvent::ApprovalRequested { .. }
            | AgentEvent::ApprovalResolved { .. }
            | AgentEvent::CompactionStarted { .. }
            | AgentEvent::CompactionProgress { .. }
            | AgentEvent::CompactionApplied { .. }
            | AgentEvent::MessageAppended { .. }
            | AgentEvent::RunStarted { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::MessageFinished { .. }
            | AgentEvent::TerminalSessionStarted { .. }
            | AgentEvent::TerminalSessionOutput { .. }
            | AgentEvent::TerminalSessionFinished { .. }
            | AgentEvent::SkillInvocation { .. } => {}
            AgentEvent::ContextWindowUpdated {
                used_tokens,
                projected_tokens,
                max_tokens,
                trigger_tokens,
                source,
                ..
            } => {
                let window = self.context_window.unwrap_or(ContextWindow::new(0));
                self.context_window = Some(
                    window
                        .with_used_tokens(used_tokens)
                        .with_projected_tokens(projected_tokens)
                        .with_max_tokens(max_tokens.or(window.max_tokens))
                        .with_trigger_tokens(trigger_tokens)
                        .with_source(source),
                );
            }
            AgentEvent::TurnFinished { .. } => {
                self.retry_exhausted_error_turn = None;
                self.apply_stream_update(StreamUpdate::TurnFinished);
            }
            AgentEvent::Error { turn, message, .. } => {
                if self.retry_exhausted_error_turn.take() != Some(turn) {
                    self.apply_stream_update(StreamUpdate::Error { text: message });
                }
            }
            AgentEvent::RunFinished { turn, stop_reason } => {
                self.retry_exhausted_error_turn = None;
                self.apply_stream_update(StreamUpdate::RunFinished { turn, stop_reason });
            }
            AgentEvent::SteeringQueued { message } => {
                self.pending_input.queue_steer(message.presentation_text());
            }
            AgentEvent::FollowUpQueued { message } => {
                self.pending_input
                    .queue_follow_up(message.presentation_text());
            }
            AgentEvent::QueueDrained { kind, count } => {
                self.pending_input.drain(kind, count);
            }
            AgentEvent::TokenUsage { usage, .. } => {
                self.add_main_agent_token_usage(usage);
            }
            AgentEvent::GoalStarted { .. } | AgentEvent::GoalResumed { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Active));
            }
            AgentEvent::GoalPaused { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Paused));
            }
            AgentEvent::GoalBlocked { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Blocked));
            }
            AgentEvent::GoalFinished { .. } => {
                self.set_development_mode(DevelopmentMode::Normal);
            }
            AgentEvent::PlanModeEntered { .. } => self.set_plan_mode(true),
            AgentEvent::PlanModeExited { .. } => {
                self.set_plan_mode(false);
            }
            AgentEvent::PlanUpdated { enabled, .. } => self.set_plan_mode(enabled),
            AgentEvent::TodoUpdated { todos, .. } => {
                let display: Vec<TodoDisplayItem> = todos
                    .iter()
                    .map(|t| TodoDisplayItem {
                        title: t.title.clone(),
                        status: match t.status.as_str() {
                            "in_progress" => TodoDisplayStatus::InProgress,
                            "done" => TodoDisplayStatus::Done,
                            _ => TodoDisplayStatus::Pending,
                        },
                    })
                    .collect();
                self.apply_todo_items(display);
            }
            AgentEvent::QuestionRequested { id, questions, .. } => {
                let display: Vec<QuestionDisplayData> = questions
                    .iter()
                    .map(|q| QuestionDisplayData {
                        question: q.question.clone(),
                        header: q.header.clone(),
                        body: q.body.clone(),
                        options: q
                            .options
                            .iter()
                            .map(|o| QuestionDisplayOption {
                                label: o.label.clone(),
                                description: o.description.clone(),
                            })
                            .collect(),
                        multi_select: q.multi_select,
                    })
                    .collect();
                self.push_question_overlay(id, display);
            }
        }
    }

    fn apply_todo_items(&mut self, todos: Vec<TodoDisplayItem>) {
        if todos.is_empty() {
            self.clear_todos();
        } else {
            self.set_todo_items(todos);
        }
    }
}
