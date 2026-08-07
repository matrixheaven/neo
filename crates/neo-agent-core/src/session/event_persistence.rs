use std::collections::HashMap;

use crate::multi_agent::{AgentProgressSnapshot, SwarmChildProgress};
use crate::workflow::WorkflowExecutionOrigin;
use crate::{AgentEvent, AgentMessage};

#[derive(Default)]
pub struct SessionEventPersistence {
    attempt: Vec<AgentEvent>,
    agents: HashMap<String, PersistedAgentProgress>,
    swarm_agents: HashMap<String, HashMap<String, PersistedAgentProgress>>,
}

impl SessionEventPersistence {
    #[must_use]
    pub fn persisted_events(&mut self, event: &AgentEvent) -> Vec<AgentEvent> {
        match event {
            AgentEvent::MessageStarted { .. }
            | AgentEvent::MessageFinished { .. }
            | AgentEvent::TextDelta { .. }
            | AgentEvent::ThinkingStarted { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ThinkingFinished { .. }
            | AgentEvent::ToolCallStarted { .. }
            | AgentEvent::ToolCallArgumentsDelta { .. }
            | AgentEvent::ToolCallFinished { .. }
            | AgentEvent::TokenUsage { .. } => {
                self.push_attempt_event(event);
                Vec::new()
            }
            AgentEvent::RetryScheduled { .. } | AgentEvent::CompactionStarted { .. } => {
                self.attempt.clear();
                vec![event.clone()]
            }
            AgentEvent::MessageAppended {
                message: AgentMessage::Assistant { .. },
            } => {
                let mut persisted = std::mem::take(&mut self.attempt);
                persisted.push(event.clone());
                persisted
            }
            AgentEvent::DelegateStarted { .. } | AgentEvent::DelegateFinished { .. } => {
                self.process_delegate_boundary(event)
            }
            AgentEvent::DelegateUpdated {
                turn,
                agent,
                workflow_origin,
            } => self.persist_delegate_progress(
                *turn,
                agent.progress_snapshot(),
                workflow_origin.clone(),
            ),
            AgentEvent::DelegateProgressUpdated {
                turn,
                progress,
                workflow_origin,
            } => self.persist_delegate_progress(*turn, progress.clone(), workflow_origin.clone()),
            AgentEvent::DelegateSwarmStarted { .. } | AgentEvent::DelegateSwarmFinished { .. } => {
                self.process_swarm_boundary(event)
            }
            AgentEvent::DelegateSwarmUpdated {
                turn,
                swarm,
                workflow_origin,
            } => {
                for child in &swarm.children {
                    let persisted = self.persist_swarm_progress(
                        *turn,
                        swarm.swarm_id.clone(),
                        swarm.state,
                        swarm.aggregate,
                        SwarmChildProgress {
                            item_index: child.item_index,
                            progress: child.agent.progress_snapshot(),
                        },
                        workflow_origin.clone(),
                    );
                    if !persisted.is_empty() {
                        return persisted;
                    }
                }
                Vec::new()
            }
            AgentEvent::DelegateSwarmProgressUpdated {
                turn,
                swarm_id,
                state,
                aggregate,
                child_progress,
                workflow_origin,
            } => self.persist_swarm_progress(
                *turn,
                swarm_id.clone(),
                *state,
                *aggregate,
                child_progress.clone(),
                workflow_origin.clone(),
            ),
            // Live queue rank/wait ticks must not land in session JSONL.
            AgentEvent::ToolExecutionQueueUpdated { .. }
            | AgentEvent::ShellCommandQueueUpdated { .. } => Vec::new(),
            // Durable transcript-only approval lifecycle. Never model messages;
            // never rehydrated into PendingApproval or grants on resume.
            AgentEvent::ApprovalRequested { .. } | AgentEvent::ApprovalResolved { .. } => {
                vec![event.clone()]
            }
            // The catch-all also covers `AgentEvent::InstructionEpoch`: the
            // epoch is the single persisted source for instruction model
            // content and transcript metadata, persisted exactly once and
            // never duplicated as a `MessageAppended` copy. Queued transition
            // events (`ToolExecutionQueued` / `ShellCommandQueued`) persist
            // through this default branch.
            _ => vec![event.clone()],
        }
    }

    fn push_attempt_event(&mut self, event: &AgentEvent) {
        match (self.attempt.last_mut(), event) {
            (
                Some(AgentEvent::TextDelta {
                    turn: previous_turn,
                    text: previous,
                }),
                AgentEvent::TextDelta { turn, text },
            ) if previous_turn == turn => previous.push_str(text),
            (
                Some(AgentEvent::ThinkingDelta {
                    turn: previous_turn,
                    text: previous,
                }),
                AgentEvent::ThinkingDelta { turn, text },
            ) if previous_turn == turn => previous.push_str(text),
            (
                Some(AgentEvent::ToolCallArgumentsDelta {
                    turn: previous_turn,
                    id: previous_id,
                    json_fragment: previous,
                }),
                AgentEvent::ToolCallArgumentsDelta {
                    turn,
                    id,
                    json_fragment,
                },
            ) if previous_turn == turn && previous_id == id => previous.push_str(json_fragment),
            _ => self.attempt.push(event.clone()),
        }
    }

    fn process_delegate_boundary(&mut self, event: &AgentEvent) -> Vec<AgentEvent> {
        let mut event = event.clone();
        if let AgentEvent::DelegateStarted { agent, .. }
        | AgentEvent::DelegateFinished { agent, .. } = &mut event
        {
            agent.clear_live_queue_metadata();
            let mut progress = agent.progress_snapshot();
            normalize_persisted_progress(&mut progress);
            self.agents.insert(
                agent.id.as_str().to_owned(),
                PersistedAgentProgress::from_progress(progress),
            );
        }
        vec![event]
    }

    fn process_swarm_boundary(&mut self, event: &AgentEvent) -> Vec<AgentEvent> {
        let mut event = event.clone();
        if let AgentEvent::DelegateSwarmStarted { swarm, .. }
        | AgentEvent::DelegateSwarmFinished { swarm, .. } = &mut event
        {
            swarm.clear_live_queue_metadata();
            let swarm_gates = self.swarm_agents.entry(swarm.swarm_id.clone()).or_default();
            for child in &swarm.children {
                let mut progress = child.agent.progress_snapshot();
                normalize_persisted_progress(&mut progress);
                swarm_gates.insert(
                    child.agent.id.as_str().to_owned(),
                    PersistedAgentProgress::from_progress(progress),
                );
            }
        }
        vec![event]
    }

    fn persist_delegate_progress(
        &mut self,
        turn: u32,
        mut progress: AgentProgressSnapshot,
        workflow_origin: Option<WorkflowExecutionOrigin>,
    ) -> Vec<AgentEvent> {
        normalize_persisted_progress(&mut progress);
        let gate = self
            .agents
            .entry(progress.agent_id.as_str().to_owned())
            .or_default();
        if !gate.should_persist(progress) {
            return Vec::new();
        }
        vec![AgentEvent::DelegateProgressUpdated {
            turn,
            progress: gate.last_progress.clone().expect("progress recorded"),
            workflow_origin,
        }]
    }

    fn persist_swarm_progress(
        &mut self,
        turn: u32,
        swarm_id: String,
        state: crate::multi_agent::AgentLifecycleState,
        aggregate: crate::multi_agent::SwarmAggregate,
        mut child_progress: SwarmChildProgress,
        workflow_origin: Option<WorkflowExecutionOrigin>,
    ) -> Vec<AgentEvent> {
        normalize_persisted_progress(&mut child_progress.progress);
        let gate = self
            .swarm_agents
            .entry(swarm_id.clone())
            .or_default()
            .entry(child_progress.progress.agent_id.as_str().to_owned())
            .or_default();
        if !gate.should_persist(child_progress.progress) {
            return Vec::new();
        }
        child_progress.progress = gate.last_progress.clone().expect("progress recorded");
        vec![AgentEvent::DelegateSwarmProgressUpdated {
            turn,
            swarm_id,
            state,
            aggregate,
            child_progress,
            workflow_origin,
        }]
    }
}

fn normalize_persisted_progress(progress: &mut AgentProgressSnapshot) {
    progress.clear_live_queue_metadata();
    if let Some(tool) = &mut progress.last_tool {
        // Live output reaches the TUI; terminal snapshots persist the final preview.
        tool.output = None;
    }
}

#[derive(Default)]
struct PersistedAgentProgress {
    last_progress: Option<crate::multi_agent::AgentProgressSnapshot>,
    last_text_persisted_at_ms: u64,
}

impl PersistedAgentProgress {
    fn from_progress(progress: crate::multi_agent::AgentProgressSnapshot) -> Self {
        let last_text_persisted_at_ms = progress.updated_at_ms;
        Self {
            last_progress: Some(progress),
            last_text_persisted_at_ms,
        }
    }

    fn should_persist(&mut self, progress: crate::multi_agent::AgentProgressSnapshot) -> bool {
        const TEXT_PROGRESS_GATE_MS: u64 = 750;
        let Some(last) = &self.last_progress else {
            self.last_text_persisted_at_ms = progress.updated_at_ms;
            self.last_progress = Some(progress);
            return true;
        };
        let structural_changed = progress.state != last.state
            || progress.mode != last.mode
            || progress.detached_from_foreground != last.detached_from_foreground
            || progress.terminal_reason != last.terminal_reason
            || progress.run_count != last.run_count
            || progress.live_messages_received != last.live_messages_received
            || progress.tool_count != last.tool_count
            || progress.token_count != last.token_count
            || progress.cache_read_token_count != last.cache_read_token_count
            || progress.cache_write_token_count != last.cache_write_token_count
            || progress.last_tool != last.last_tool
            || progress.outcome != last.outcome;
        let text_changed = (progress.latest_text != last.latest_text
            || progress.latest_thinking != last.latest_thinking)
            && progress
                .updated_at_ms
                .saturating_sub(self.last_text_persisted_at_ms)
                >= TEXT_PROGRESS_GATE_MS;
        if structural_changed || text_changed {
            if text_changed {
                self.last_text_persisted_at_ms = progress.updated_at_ms;
            }
            self.last_progress = Some(progress);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AgentEvent, AgentMessage, AgentTokenUsage, AgentToolCall, CompactionReason, Content,
        StopReason,
    };

    use super::SessionEventPersistence;

    fn text_delta(text: &str) -> AgentEvent {
        AgentEvent::TextDelta {
            turn: 1,
            text: text.to_owned(),
        }
    }

    fn retry_scheduled() -> AgentEvent {
        AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 2,
            delay_ms: 10,
            error_code: "provider.transport_error".to_owned(),
            message: "retry".to_owned(),
        }
    }

    fn compaction_started() -> AgentEvent {
        AgentEvent::CompactionStarted {
            reason: CompactionReason::Threshold,
            tokens_before: 100,
            message_count: 4,
        }
    }

    fn message_appended(text: &str) -> AgentEvent {
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text(text)],
                Vec::new(),
                StopReason::EndTurn,
            ),
        }
    }

    fn winning_stream_details() -> Vec<AgentEvent> {
        vec![
            AgentEvent::MessageStarted {
                turn: 1,
                id: "message".to_owned(),
                phase: neo_ai::MessagePhase::Unknown,
            },
            AgentEvent::ThinkingStarted {
                turn: 1,
                id: "thinking".to_owned(),
                kind: neo_ai::ThinkingKind::Unknown,
            },
            AgentEvent::ThinkingDelta {
                turn: 1,
                text: "reasoning".to_owned(),
            },
            AgentEvent::ThinkingFinished {
                turn: 1,
                signature: None,
                redacted: false,
            },
            text_delta("winning"),
            AgentEvent::ToolCallStarted {
                turn: 1,
                id: "call".to_owned(),
                name: "read".to_owned(),
            },
            AgentEvent::ToolCallArgumentsDelta {
                turn: 1,
                id: "call".to_owned(),
                json_fragment: "{}".to_owned(),
            },
            AgentEvent::ToolCallFinished {
                turn: 1,
                tool_call: AgentToolCall {
                    id: "call".into(),
                    name: "read".into(),
                    raw_arguments: "{}".into(),
                },
            },
            AgentEvent::TokenUsage {
                turn: 1,
                usage: AgentTokenUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    input_cache_read_tokens: 3,
                    input_cache_write_tokens: 4,
                },
            },
            AgentEvent::MessageFinished {
                turn: 1,
                id: "message".to_owned(),
                stop_reason: StopReason::EndTurn,
                phase: neo_ai::MessagePhase::Unknown,
            },
        ]
    }

    #[test]
    fn session_event_persistence_discards_failed_attempt() {
        let mut persistence = SessionEventPersistence::default();

        assert!(
            persistence
                .persisted_events(&text_delta("failed"))
                .is_empty()
        );
        let mut projected = persistence.persisted_events(&retry_scheduled());
        assert_eq!(projected.len(), 1);
        let winning_details = winning_stream_details();
        for detail in &winning_details {
            assert!(persistence.persisted_events(detail).is_empty());
        }
        let winning = persistence.persisted_events(&message_appended("winning"));
        assert!(!winning.is_empty());
        projected.extend(winning);

        let mut expected = vec![retry_scheduled()];
        expected.extend(winning_details);
        expected.push(message_appended("winning"));
        assert_eq!(projected, expected);

        let mut persistence = SessionEventPersistence::default();
        assert!(
            persistence
                .persisted_events(&text_delta("overflow partial"))
                .is_empty()
        );
        let mut projected = persistence.persisted_events(&compaction_started());
        assert_eq!(projected, vec![compaction_started()]);
        assert!(
            persistence
                .persisted_events(&text_delta("overflow winning"))
                .is_empty()
        );
        projected.extend(persistence.persisted_events(&message_appended("overflow winning")));
        assert_eq!(
            projected,
            vec![
                compaction_started(),
                text_delta("overflow winning"),
                message_appended("overflow winning")
            ]
        );
    }

    #[test]
    fn session_event_persistence_coalesces_adjacent_stream_deltas() {
        let mut persistence = SessionEventPersistence::default();

        for _ in 0..10_000 {
            assert!(persistence.persisted_events(&text_delta("x")).is_empty());
        }
        assert_eq!(persistence.attempt.len(), 1);
        let AgentEvent::TextDelta { text, .. } = &persistence.attempt[0] else {
            panic!("coalesced text delta must remain text");
        };
        assert_eq!(text.len(), 10_000);

        let thinking = AgentEvent::ThinkingDelta {
            turn: 1,
            text: "y".to_owned(),
        };
        for _ in 0..10_000 {
            assert!(persistence.persisted_events(&thinking).is_empty());
        }
        assert_eq!(persistence.attempt.len(), 2);
        let AgentEvent::ThinkingDelta { text, .. } = &persistence.attempt[1] else {
            panic!("coalesced thinking delta must remain thinking");
        };
        assert_eq!(text.len(), 10_000);

        let arguments = AgentEvent::ToolCallArgumentsDelta {
            turn: 1,
            id: "call".to_owned(),
            json_fragment: "z".to_owned(),
        };
        for _ in 0..10_000 {
            assert!(persistence.persisted_events(&arguments).is_empty());
        }
        assert_eq!(persistence.attempt.len(), 3);
        let AgentEvent::ToolCallArgumentsDelta { json_fragment, .. } = &persistence.attempt[2]
        else {
            panic!("coalesced arguments delta must remain tool arguments");
        };
        assert_eq!(json_fragment.len(), 10_000);
    }
}
