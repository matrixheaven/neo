use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{estimate_message_tokens, estimate_messages_tokens};
use crate::{
    AgentEvent, AgentMessage, CompactionSummary, QueueKind, TodoEventData,
    sanitize_tool_exchange_messages,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentContext {
    // IMPORTANT: do not delete this comment unless the cancellation model is
    // intentionally redesigned and the regression test below is replaced with
    // an equivalent guard:
    // `runtime_resumed_cancelled_turn_accepts_followup_prompt`.
    //
    // Bug background, 2026-06-22:
    // `AgentContext` used to contain a persistent `cancelled: bool`. Replay of
    // any historical `TurnFinished { stop_reason: Cancelled }` set that flag,
    // and `run_turn_with_cancel` checked it before starting the next turn. That
    // made a resumed session permanently poisoned: after a user interrupted one
    // turn, every later prompt in that JSONL session immediately produced
    // `RunFinished(Cancelled)` without calling the model. The observed failure
    // was `neo resume session_0774471a-c613-40d3-b758-3ebfb3dc40d1`, where
    // turns 321 and 322 were cancelled as soon as the recalled prompt was sent.
    //
    // Cancellation is a property of the currently executing turn, carried by
    // that turn's `CancellationToken`. It is not durable session state. A
    // replayed cancelled turn must remain visible in the transcript and must
    // still advance the turn counter, but it must not affect future turns.
    pub(super) messages: Vec<AgentMessage>,
    pub(super) turns: u32,
    pub(super) steering_queue: Vec<AgentMessage>,
    pub(super) follow_up_queue: Vec<AgentMessage>,
    /// Skill context injected before the next user message in the current turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) skill_context: Option<AgentMessage>,
    pub(super) compaction_summary: Option<CompactionSummary>,
    /// Whether plan mode was active at the end of the last replayed/exected turn.
    #[serde(default)]
    pub(super) plan_mode_active: bool,
    /// The plan id from the last `PlanModeEntered` event, if any.
    #[serde(default)]
    pub(super) plan_mode_id: Option<String>,
    /// Latest todo list state, restored on resume replay.
    #[serde(default)]
    pub(super) todos: Vec<TodoEventData>,
    /// Running estimate of token count for `messages`. Updated incrementally
    /// on `append_message`; recomputed after `apply_compaction`. Avoids
    /// repeated O(n) char-walks of the full message history.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) estimated_tokens: usize,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

impl AgentContext {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    #[must_use]
    pub fn estimated_context_tokens(&self) -> u32 {
        u32::try_from(self.estimated_tokens).unwrap_or(u32::MAX)
    }

    /// Raw `usize` token estimate for internal use (avoids `u32` truncation).
    #[must_use]
    pub fn estimated_tokens(&self) -> usize {
        self.estimated_tokens
    }

    #[must_use]
    pub fn turns(&self) -> u32 {
        self.turns
    }

    pub fn append_message(&mut self, message: AgentMessage) {
        self.estimated_tokens += estimate_message_tokens(&message);
        self.messages.push(message);
    }

    pub fn queue_steering_message(&mut self, message: AgentMessage) {
        self.steering_queue.push(message);
    }

    pub fn queue_follow_up_message(&mut self, message: AgentMessage) {
        self.follow_up_queue.push(message);
    }

    /// Set a skill context message to be inserted before the next user message.
    pub fn set_skill_context(&mut self, message: AgentMessage) {
        self.skill_context = Some(message);
    }

    /// Take the pending skill context message, if any.
    #[must_use]
    pub fn take_skill_context(&mut self) -> Option<AgentMessage> {
        self.skill_context.take()
    }

    pub fn apply_compaction(&mut self, summary: CompactionSummary) {
        let keep_from = summary.first_kept_message_index.min(self.messages.len());
        let mut kept = self.messages.split_off(keep_from);
        // Drop any trailing assistant-with-tool-calls whose results were
        // compacted away, so the retained tail is always provider-valid.
        kept = sanitize_tool_exchange_messages(&kept).into_owned();
        // Inject the LLM-generated summary as a system message so the model
        // has the compacted context when continuing the conversation.
        let summary_msg = AgentMessage::system_text(format!(
            "<compaction_summary>\n\
             This is a compaction summary of earlier conversation context. \
             Use it as background only. Before acting, continue from the newest user request and any unsummarized messages after this summary; \
             do not answer an older request just because it appears in the summary.\n\n\
             Summary:\n\n{}\n</compaction_summary>",
            summary.summary
        ));
        kept.insert(0, summary_msg);
        self.messages = kept;
        // Recompute token estimate after compaction rewrites the message list.
        self.estimated_tokens = estimate_messages_tokens(&self.messages);
        self.compaction_summary = Some(summary);
    }

    #[must_use]
    pub fn compaction_summary(&self) -> Option<&CompactionSummary> {
        self.compaction_summary.as_ref()
    }

    #[must_use]
    pub fn pending_steering_len(&self) -> usize {
        self.steering_queue.len()
    }

    #[must_use]
    pub fn pending_follow_up_len(&self) -> usize {
        self.follow_up_queue.len()
    }

    /// Whether plan mode is currently active (from replayed state).
    #[must_use]
    pub fn is_plan_mode_active(&self) -> bool {
        self.plan_mode_active
    }

    /// The plan id from the last replayed `PlanModeEntered` event, if any.
    #[must_use]
    pub fn plan_mode_id(&self) -> Option<&str> {
        self.plan_mode_id.as_deref()
    }

    /// Latest todo list from replayed state.
    #[must_use]
    pub fn todos(&self) -> &[TodoEventData] {
        &self.todos
    }

    #[must_use]
    pub fn from_replay<'a>(events: impl IntoIterator<Item = &'a AgentEvent>) -> Self {
        let mut context = Self::new();
        for event in events {
            context.apply_replay_event(event);
        }
        context.messages = sanitize_tool_exchange_messages(&context.messages).into_owned();
        // sanitize may have dropped orphaned tool exchanges; recompute.
        context.estimated_tokens = estimate_messages_tokens(&context.messages);
        context
    }

    fn apply_replay_event(&mut self, event: &AgentEvent) {
        if self.apply_replay_message_event(event) {
            return;
        }
        if self.apply_replay_queue_event(event) {
            return;
        }
        self.apply_replay_state_event(event);
    }

    fn apply_replay_message_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::MessageAppended { message } => self.append_message(message.clone()),
            AgentEvent::TurnFinished { turn, .. } => {
                // See the invariant on `AgentContext`: replayed cancellation is
                // historical transcript state only. Do not inspect
                // `stop_reason` here or reintroduce durable cancellation.
                self.turns = self.turns.max(*turn);
            }
            _ => return false,
        }
        true
    }

    fn apply_replay_queue_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::SteeringQueued { message } => {
                self.queue_steering_message(message.clone());
            }
            AgentEvent::FollowUpQueued { message } => {
                self.queue_follow_up_message(message.clone());
            }
            AgentEvent::QueueDrained { kind, count } => self.drain_replay_queue(*kind, *count),
            _ => return false,
        }
        true
    }

    fn drain_replay_queue(&mut self, kind: QueueKind, count: usize) {
        match kind {
            QueueKind::Steering => {
                let drain_count = count.min(self.steering_queue.len());
                self.steering_queue.drain(0..drain_count);
            }
            QueueKind::FollowUp => {
                let drain_count = count.min(self.follow_up_queue.len());
                self.follow_up_queue.drain(0..drain_count);
            }
        }
    }

    fn apply_replay_state_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::CompactionApplied { summary } => self.apply_compaction(summary.clone()),
            AgentEvent::PlanModeEntered { id, .. } => {
                self.plan_mode_active = true;
                self.plan_mode_id = Some(id.clone());
            }
            AgentEvent::PlanModeExited { .. } => {
                self.plan_mode_active = false;
            }
            AgentEvent::PlanUpdated { enabled, .. } => {
                self.plan_mode_active = *enabled;
            }
            AgentEvent::TodoUpdated { todos, .. } => self.todos.clone_from(todos),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentMessage, CompactionSummary};

    #[test]
    fn apply_compaction_injects_resume_guardrails() {
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::user_text("Earlier request"));

        context.apply_compaction(CompactionSummary {
            summary: "Old conversation summary.".to_owned(),
            tokens_before: 100,
            tokens_after: 50,
            first_kept_message_index: 1,
        });

        let Some(AgentMessage::System { content }) = context.messages().first() else {
            panic!("expected compaction summary system message");
        };
        let text = content
            .iter()
            .filter_map(crate::Content::as_text)
            .collect::<String>();

        assert!(text.contains("compaction summary"));
        assert!(
            text.contains("continue from the newest user request"),
            "{text}"
        );
        assert!(
            text.contains("do not answer an older request just because it appears in the summary"),
            "{text}"
        );
    }
}
