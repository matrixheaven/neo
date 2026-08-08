use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::path::PathBuf;

use crate::multi_agent::{
    AgentLifecycleState, AgentProgressSnapshot, AgentSnapshot, SwarmAggregate, SwarmChildProgress,
    SwarmSnapshot,
};
use crate::session::ToolOutputRef;
use crate::{
    AgentMessage, AgentToolCall, ApprovalRequest, ApprovalResolution, ShellCommandOutcome,
    ToolResult,
};

/// A preset revision suggestion offered during plan review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanSuggestion {
    /// Short label shown as the suggestion title.
    pub label: String,
    /// Longer explanation shown under the label.
    pub description: String,
    /// Feedback text to populate when the user selects this suggestion.
    #[serde(default)]
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ShellCommandOrigin {
    ModelBashTool,
    UserShellMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentTokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub input_cache_read_tokens: u32,
    #[serde(default)]
    pub input_cache_write_tokens: u32,
}

impl AgentTokenUsage {
    pub(crate) fn saturating_add(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            input_cache_read_tokens: self
                .input_cache_read_tokens
                .saturating_add(other.input_cache_read_tokens),
            input_cache_write_tokens: self
                .input_cache_write_tokens
                .saturating_add(other.input_cache_write_tokens),
        }
    }
}

impl From<neo_ai::TokenUsage> for AgentTokenUsage {
    fn from(value: neo_ai::TokenUsage) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            input_cache_read_tokens: value.input_cache_read_tokens,
            input_cache_write_tokens: value.input_cache_write_tokens,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Cancelled,
    Error,
}

impl From<neo_ai::StopReason> for StopReason {
    fn from(value: neo_ai::StopReason) -> Self {
        match value {
            neo_ai::StopReason::EndTurn => Self::EndTurn,
            neo_ai::StopReason::ToolUse => Self::ToolUse,
            neo_ai::StopReason::MaxTokens => Self::MaxTokens,
            neo_ai::StopReason::Cancelled => Self::Cancelled,
            neo_ai::StopReason::Error => Self::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SkillInvocationSource {
    Auto,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SkillInvocationOutcome {
    Activated,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AgentEvent {
    RunStarted {
        turn: u32,
    },
    TurnStarted {
        turn: u32,
    },
    RetryScheduled {
        turn: u32,
        retry: u32,
        max_retries: u32,
        delay_ms: u64,
        error_code: String,
        message: String,
    },
    RetryStarted {
        turn: u32,
        retry: u32,
        max_retries: u32,
    },
    RetryResumed {
        turn: u32,
        retry: u32,
    },
    RetrySucceeded {
        turn: u32,
        retries_used: u32,
    },
    RetryExhausted {
        turn: u32,
        retries_used: u32,
        error_code: String,
        message: String,
    },
    MessageStarted {
        turn: u32,
        id: String,
        #[serde(default)]
        phase: neo_ai::MessagePhase,
    },
    MessageFinished {
        turn: u32,
        id: String,
        stop_reason: StopReason,
        #[serde(default)]
        phase: neo_ai::MessagePhase,
    },
    TextDelta {
        turn: u32,
        text: String,
    },
    ThinkingStarted {
        turn: u32,
        id: String,
        #[serde(default)]
        kind: neo_ai::ThinkingKind,
    },
    ThinkingDelta {
        turn: u32,
        text: String,
    },
    ThinkingFinished {
        turn: u32,
        signature: Option<String>,
        redacted: bool,
    },
    ToolCallStarted {
        turn: u32,
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        turn: u32,
        id: String,
        json_fragment: String,
    },
    ToolCallFinished {
        turn: u32,
        tool_call: AgentToolCall,
    },
    ToolExecutionStarted {
        turn: u32,
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
        /// Typed complete-display-output artifact for this execution, when the
        /// runtime captured one. Presentation metadata only: never enters
        /// `ToolResult`, canonical messages, or provider requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_ref: Option<ToolOutputRef>,
    },
    ToolExecutionQueued {
        turn: u32,
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    ToolExecutionQueueUpdated {
        turn: u32,
        id: String,
        position: usize,
        waiting_ms: u64,
    },
    ToolExecutionFinished {
        turn: u32,
        id: String,
        name: String,
        result: ToolResult,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
        /// Typed complete-display-output artifact for this execution, when the
        /// runtime captured one. Presentation metadata only: never enters
        /// `ToolResult`, canonical messages, or provider requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_ref: Option<ToolOutputRef>,
    },
    ToolExecutionUpdate {
        turn: u32,
        id: String,
        name: String,
        partial_result: ToolResult,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
        /// Typed complete-display-output artifact for this execution, when the
        /// runtime captured one. Presentation metadata only: never enters
        /// `ToolResult`, canonical messages, or provider requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_ref: Option<ToolOutputRef>,
    },
    SkillInvocation {
        names: Vec<String>,
        source: SkillInvocationSource,
        outcome: SkillInvocationOutcome,
        body: String,
    },
    GoalStarted {
        turn: u32,
        objective: String,
    },
    GoalPaused {
        turn: u32,
        objective: String,
    },
    GoalResumed {
        turn: u32,
        objective: String,
    },
    GoalBlocked {
        turn: u32,
        objective: String,
        reason: String,
    },
    GoalFinished {
        turn: u32,
        objective: String,
        outcome: String,
    },
    ApprovalRequested {
        request: ApprovalRequest,
    },
    ApprovalResolved {
        turn: u32,
        request_id: String,
        resolution: ApprovalResolution,
    },
    ShellCommandStarted {
        turn: u32,
        id: String,
        command: String,
        cwd: PathBuf,
        origin: ShellCommandOrigin,
    },
    ShellCommandQueued {
        turn: u32,
        id: String,
        command: String,
        cwd: PathBuf,
        origin: ShellCommandOrigin,
    },
    ShellCommandQueueUpdated {
        turn: u32,
        id: String,
        position: usize,
        waiting_ms: u64,
    },
    ShellCommandFinished {
        turn: u32,
        id: String,
        exit_code: Option<i32>,
        /// Unix signal number when the process was killed by a signal.
        signal: Option<i32>,
        stdout: String,
        stderr: String,
        truncated: bool,
        origin: ShellCommandOrigin,
        outcome: ShellCommandOutcome,
        /// Typed complete-display-output artifact for this command, when the
        /// runtime captured one. Presentation metadata only: never enters
        /// `ToolResult`, canonical messages, or provider requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_ref: Option<ToolOutputRef>,
    },
    TerminalSessionStarted {
        turn: u32,
        id: String,
        handle: String,
        command: String,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
        /// Typed complete-display-output artifact for this session, when the
        /// runtime captured one. One terminal process keeps one reference
        /// across start/read/write/resize/stop tool calls.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_ref: Option<ToolOutputRef>,
    },
    TerminalSessionOutput {
        turn: u32,
        id: String,
        handle: String,
        output: String,
        truncated: bool,
        /// Typed complete-display-output artifact for this session, when the
        /// runtime captured one. One terminal process keeps one reference
        /// across start/read/write/resize/stop tool calls.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_ref: Option<ToolOutputRef>,
    },
    TerminalSessionFinished {
        turn: u32,
        id: String,
        handle: String,
        status: String,
        exit_code: Option<i32>,
        /// Typed complete-display-output artifact for this session, when the
        /// runtime captured one. One terminal process keeps one reference
        /// across start/read/write/resize/stop tool calls.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_ref: Option<ToolOutputRef>,
    },
    TokenUsage {
        turn: u32,
        usage: AgentTokenUsage,
    },
    ContextWindowUpdated {
        turn: u32,
        used_tokens: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projected_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remaining_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<ContextWindowSource>,
    },
    SteeringQueued {
        message: AgentMessage,
    },
    FollowUpQueued {
        message: AgentMessage,
    },
    QueueDrained {
        kind: QueueKind,
        count: usize,
    },
    CompactionStarted {
        reason: CompactionReason,
        tokens_before: usize,
        message_count: usize,
    },
    CompactionProgress {
        phase: CompactionPhase,
        percent: u8,
    },
    CompactionApplied {
        summary: CompactionSummary,
    },
    MessageAppended {
        message: AgentMessage,
    },
    /// One append-only instruction epoch: the single persisted source for
    /// path-scoped AGENTS.md model content and transcript metadata. Never
    /// duplicated as a `MessageAppended` event; replay rebuilds the sourced
    /// instruction injection and agent-local visibility from this.
    InstructionEpoch {
        epoch: crate::instructions::InstructionEpochData,
    },
    TurnFinished {
        turn: u32,
        stop_reason: StopReason,
    },
    RunFinished {
        turn: u32,
        stop_reason: StopReason,
    },
    Error {
        turn: u32,
        message: String,
        /// Stable error code (e.g. `"provider.rate_limit"`). `None` for old sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Retry-After hint in seconds, if the provider included one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    },
    /// Plan mode was entered — read-only exploration plus plan file writes.
    PlanModeEntered {
        turn: u32,
        id: String,
    },
    /// Plan mode was exited — normal tool access restored.
    PlanModeExited {
        turn: u32,
        id: String,
    },
    /// Plan-mode active state changed (for TUI replay / status updates).
    PlanUpdated {
        turn: u32,
        enabled: bool,
    },
    /// Structured todo list was updated (for persistence + TUI panel).
    TodoUpdated {
        turn: u32,
        todos: Vec<TodoEventData>,
    },
    /// `AskUser` question request (reverse-RPC from tool to host).
    QuestionRequested {
        turn: u32,
        id: String,
        questions: Vec<QuestionEventData>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    DelegateStarted {
        turn: u32,
        agent: AgentSnapshot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    DelegateUpdated {
        turn: u32,
        agent: AgentSnapshot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    DelegateProgressUpdated {
        turn: u32,
        progress: AgentProgressSnapshot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    DelegateFinished {
        turn: u32,
        agent: AgentSnapshot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    DelegateSwarmStarted {
        turn: u32,
        swarm: SwarmSnapshot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    DelegateSwarmUpdated {
        turn: u32,
        swarm: SwarmSnapshot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    DelegateSwarmProgressUpdated {
        turn: u32,
        swarm_id: String,
        state: AgentLifecycleState,
        aggregate: SwarmAggregate,
        child_progress: SwarmChildProgress,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    DelegateSwarmFinished {
        turn: u32,
        swarm: SwarmSnapshot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_origin: Option<crate::workflow::WorkflowExecutionOrigin>,
    },
    WorkflowStarted {
        turn: u32,
        workflow: crate::workflow::WorkflowSnapshot,
    },
    WorkflowUpdated {
        turn: u32,
        workflow: crate::workflow::WorkflowSnapshot,
    },
    WorkflowFinished {
        turn: u32,
        workflow: crate::workflow::WorkflowSnapshot,
    },
}

impl AgentEvent {
    #[must_use]
    pub fn without_delegate_prior_messages(mut self) -> Self {
        self.strip_delegate_prior_messages();
        self
    }

    pub fn strip_delegate_prior_messages(&mut self) {
        match self {
            Self::DelegateStarted { agent, .. }
            | Self::DelegateUpdated { agent, .. }
            | Self::DelegateFinished { agent, .. } => {
                agent.prior_messages.clear();
            }
            Self::DelegateSwarmStarted { swarm, .. }
            | Self::DelegateSwarmUpdated { swarm, .. }
            | Self::DelegateSwarmFinished { swarm, .. } => {
                for child in &mut swarm.children {
                    child.agent.prior_messages.clear();
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Value types for new events
// ---------------------------------------------------------------------------

/// Serializable representation of a single todo item, used in
/// [`AgentEvent::TodoUpdated`]. Kept in `events.rs` so that persistence
/// does not depend on the `tools` module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TodoEventData {
    /// Short, human-readable description of the task.
    pub title: String,
    /// Current status: `"pending"`, `"in_progress"`, or `"done"`.
    pub status: String,
}

/// Serializable representation of a single question in an `AskUser` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionEventData {
    /// The question text (should end with `?`).
    pub question: String,
    /// Optional short header displayed above the question (max ~12 chars).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// Optional longer body / context for the question.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Available options the user can choose from.
    pub options: Vec<QuestionOptionData>,
    /// Whether the user may select multiple options.
    pub multi_select: bool,
}

/// Serializable representation of a single option in a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOptionData {
    /// Short label shown as the choice.
    pub label: String,
    /// Optional description explaining the option.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum QueueKind {
    Steering,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ContextWindowSource {
    Configured,
    ObservedOverflow,
    MissingModelWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CompactionReason {
    Threshold,
    Manual,
}

/// Whether compaction was triggered by the user (`/compact`) or automatically
/// by the threshold strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CompactionSource {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CompactionPhase {
    Estimating,
    SelectingBoundary,
    Summarizing,
    Applying,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSummary {
    pub summary: String,
    pub tokens_before: usize,
    /// Estimated token count *after* compaction (summary + retained messages).
    pub tokens_after: usize,
    pub first_kept_message_index: usize,
}

#[cfg(test)]
#[path = "test_cases/events.rs"]
mod tests;
