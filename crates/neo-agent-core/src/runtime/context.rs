use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{estimate_message_tokens, estimate_messages_tokens};
use crate::instructions::{AgentInstructionState, InstructionEpochData, InstructionRegistry};
use crate::{
    AgentEvent, AgentMessage, CompactionSummary, Content, QueueKind, TodoEventData,
    sanitize_tool_exchange_messages, trim_trailing_incomplete_tool_turn,
};

/// Host-only handle to the session instruction registry. Never serialized:
/// replay rebuilds durable state from epoch events, and the host re-attaches
/// a registry after replay. Equality compares pointer identity so two
/// contexts are equal only when they share one registry (or both have none).
#[derive(Debug, Clone, Default)]
pub(crate) struct SharedInstructionRegistry(Option<Arc<InstructionRegistry>>);

impl SharedInstructionRegistry {
    fn get(&self) -> Option<&Arc<InstructionRegistry>> {
        self.0.as_ref()
    }
}

impl PartialEq for SharedInstructionRegistry {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            (None, Some(_)) | (Some(_), None) => false,
        }
    }
}

impl Eq for SharedInstructionRegistry {}

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
    /// Agent-local instruction visibility, rebuilt from replayed
    /// `InstructionEpoch` events. Old sessions without epochs keep the
    /// default (`visible_generation == 0`) so resume can establish a fresh
    /// baseline.
    #[serde(default)]
    pub(super) instruction_state: AgentInstructionState,
    /// Host-only shared instruction registry; skipped for serialization and
    /// schema generation.
    #[serde(skip)]
    #[schemars(skip)]
    pub(super) instruction_registry: SharedInstructionRegistry,
}

fn is_zero<T: Default + PartialEq>(v: &T) -> bool {
    v == &T::default()
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

    /// Agent-local instruction visibility state (which instruction revision
    /// the model has actually seen).
    #[must_use]
    pub fn instruction_state(&self) -> &AgentInstructionState {
        &self.instruction_state
    }

    /// Mutable access to the agent-local instruction state. Live preflight
    /// callers use this to record the decision fingerprint
    /// (`last_epoch_fingerprint`) after applying an epoch; replay cannot
    /// reconstruct fingerprints and leaves them unset.
    pub fn instruction_state_mut(&mut self) -> &mut AgentInstructionState {
        &mut self.instruction_state
    }

    /// Attach the session-shared instruction registry (host-only; never
    /// serialized).
    pub fn attach_instruction_registry(&mut self, registry: Arc<InstructionRegistry>) {
        self.instruction_registry = SharedInstructionRegistry(Some(registry));
    }

    /// The attached instruction registry, if the host attached one.
    #[must_use]
    pub fn instruction_registry(&self) -> Option<Arc<InstructionRegistry>> {
        self.instruction_registry.get().cloned()
    }

    /// Apply one instruction epoch: pin its model content as an
    /// [`AgentMessage::Instruction`] (only when `model_content` is `Some`)
    /// and update agent-local visibility. Never synthesizes a
    /// `MessageAppended` event — the epoch itself is the persisted record.
    ///
    /// Replay cannot reconstruct the preflight fingerprint, so this updates
    /// visibility only; live callers record `fingerprint.hash` afterwards via
    /// [`Self::instruction_state_mut`].
    pub fn apply_instruction_epoch(&mut self, epoch: &InstructionEpochData) {
        self.instruction_state.apply_epoch_visibility(epoch);
        if let Some(model_content) = &epoch.model_content {
            self.append_message(AgentMessage::Instruction {
                generation: epoch.generation,
                content: vec![Content::text(model_content.clone())],
            });
        }
    }

    #[must_use]
    pub fn from_replay<'a>(events: impl IntoIterator<Item = &'a AgentEvent>) -> Self {
        let mut context = Self::new();
        for event in events {
            context.apply_replay_event(event);
        }
        context.messages =
            sanitize_tool_exchange_messages(&trim_trailing_incomplete_tool_turn(&context.messages))
                .into_owned();
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
            // Rebuild pinned instruction messages and agent-local visibility
            // from the epoch events themselves, in wire order. Replacements
            // append a new pinned message without rewriting earlier bytes.
            AgentEvent::InstructionEpoch { epoch } => self.apply_instruction_epoch(epoch),
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
    use crate::instructions::{
        InstructionBundleMetadata, InstructionEpochData, InstructionEpochOutcome,
        InstructionReplacement, InstructionScopeData, InstructionScopeKind,
    };
    use crate::{AgentMessage, CompactionSummary};
    use std::path::PathBuf;

    fn instruction_epoch(
        generation: u64,
        revision: &str,
        outcome: InstructionEpochOutcome,
        model_content: Option<&str>,
        replacements: Vec<InstructionReplacement>,
    ) -> InstructionEpochData {
        let scope = PathBuf::from("/workspace");
        InstructionEpochData {
            agent_id: "main".to_owned(),
            generation,
            outcome,
            scopes: vec![InstructionScopeData {
                display_path: scope.clone(),
                kind: InstructionScopeKind::WorkspaceRoot,
                revision: Some(revision.to_owned()),
                token_estimate: 12,
            }],
            selected_bundles: vec![InstructionBundleMetadata {
                display_path: scope,
                revision: revision.to_owned(),
                token_estimate: 12,
                byte_size: 64,
                source_count: 1,
                import_count: 0,
            }],
            ignored_bundles: Vec::new(),
            replacements,
            failure: None,
            deferred_tool_ids: Vec::new(),
            model_content: model_content.map(str::to_owned),
        }
    }

    #[test]
    fn replay_instruction_replacement_preserves_historical_messages_and_updates_authority() {
        let workspace = PathBuf::from("/workspace");
        let first = instruction_epoch(
            1,
            "rev-1",
            InstructionEpochOutcome::Ready,
            Some("first rules"),
            Vec::new(),
        );
        let second = instruction_epoch(
            2,
            "rev-2",
            InstructionEpochOutcome::Updated,
            Some("second rules"),
            vec![InstructionReplacement {
                display_path: workspace.clone(),
                previous_revision: "rev-1".to_owned(),
                new_revision: "rev-2".to_owned(),
            }],
        );
        let events = [
            AgentEvent::InstructionEpoch { epoch: first },
            AgentEvent::InstructionEpoch { epoch: second },
        ];

        let context = AgentContext::from_replay(events.iter());

        // The replacement appends a second pinned message; the earlier
        // revision's bytes are preserved, not rewritten.
        assert_eq!(context.messages().len(), 2);
        let Some(AgentMessage::Instruction {
            generation: first_generation,
            content: first_content,
        }) = context.messages().first()
        else {
            panic!("expected first pinned instruction message");
        };
        assert_eq!(*first_generation, 1);
        assert_eq!(
            first_content
                .iter()
                .filter_map(crate::Content::as_text)
                .collect::<String>(),
            "first rules"
        );
        let Some(AgentMessage::Instruction {
            generation: second_generation,
            content: second_content,
        }) = context.messages().get(1)
        else {
            panic!("expected second pinned instruction message");
        };
        assert_eq!(*second_generation, 2);
        assert_eq!(
            second_content
                .iter()
                .filter_map(crate::Content::as_text)
                .collect::<String>(),
            "second rules"
        );

        // Authority moves to the replacement revision.
        let state = context.instruction_state();
        assert_eq!(state.visible_generation, 2);
        assert_eq!(
            state.visible_revisions.get(&workspace).map(String::as_str),
            Some("rev-2")
        );
        assert_eq!(state.active_scopes, vec![workspace.clone()]);
        assert_eq!(
            state.most_recent_scope.as_deref(),
            Some(workspace.as_path())
        );

        // A removal epoch without model content updates authority without
        // pinning a new message.
        let removal = InstructionEpochData {
            outcome: InstructionEpochOutcome::Removed,
            scopes: Vec::new(),
            selected_bundles: Vec::new(),
            model_content: None,
            ..instruction_epoch(
                3,
                "rev-2",
                InstructionEpochOutcome::Removed,
                None,
                Vec::new(),
            )
        };
        let mut removal_events = events.to_vec();
        removal_events.push(AgentEvent::InstructionEpoch { epoch: removal });
        let context = AgentContext::from_replay(removal_events.iter());
        assert_eq!(context.messages().len(), 2, "removal pins no new message");
        let state = context.instruction_state();
        assert_eq!(state.visible_generation, 3);
        assert!(state.visible_revisions.is_empty());
        assert!(state.active_scopes.is_empty());
        assert_eq!(state.most_recent_scope, None);

        // Old sessions without any epoch keep generation 0 so resume can
        // establish a fresh baseline.
        let legacy = AgentContext::from_replay(
            [AgentEvent::MessageAppended {
                message: AgentMessage::user_text("hi"),
            }]
            .iter(),
        );
        assert_eq!(legacy.instruction_state().visible_generation, 0);
        assert!(
            legacy
                .messages()
                .iter()
                .all(|message| !matches!(message, AgentMessage::Instruction { .. }))
        );
    }

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
