use std::collections::HashMap;

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
                self.attempt.push(event.clone());
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
            AgentEvent::DelegateStarted { agent, .. }
            | AgentEvent::DelegateFinished { agent, .. } => {
                self.agents.insert(
                    agent.id.as_str().to_owned(),
                    PersistedAgentProgress::from_progress(agent.progress_snapshot()),
                );
                vec![event.clone()]
            }
            AgentEvent::DelegateUpdated { turn, agent } => {
                let progress = agent.progress_snapshot();
                let agent_id = progress.agent_id.as_str().to_owned();
                let gate = self.agents.entry(agent_id).or_default();
                if gate.should_persist(progress) {
                    let progress = gate.last_progress.clone().expect("progress recorded");
                    vec![AgentEvent::DelegateProgressUpdated {
                        turn: *turn,
                        progress,
                    }]
                } else {
                    Vec::new()
                }
            }
            AgentEvent::DelegateSwarmStarted { swarm, .. }
            | AgentEvent::DelegateSwarmFinished { swarm, .. } => {
                let swarm_gates = self.swarm_agents.entry(swarm.swarm_id.clone()).or_default();
                for child in &swarm.children {
                    swarm_gates.insert(
                        child.agent.id.as_str().to_owned(),
                        PersistedAgentProgress::from_progress(child.agent.progress_snapshot()),
                    );
                }
                vec![event.clone()]
            }
            AgentEvent::DelegateSwarmUpdated { turn, swarm } => {
                let swarm_gates = self.swarm_agents.entry(swarm.swarm_id.clone()).or_default();
                for child in &swarm.children {
                    let progress = child.agent.progress_snapshot();
                    let agent_id = progress.agent_id.as_str().to_owned();
                    let gate = swarm_gates.entry(agent_id).or_default();
                    if gate.should_persist(progress) {
                        let progress = gate.last_progress.clone().expect("progress recorded");
                        return vec![AgentEvent::DelegateSwarmProgressUpdated {
                            turn: *turn,
                            swarm_id: swarm.swarm_id.clone(),
                            state: swarm.state,
                            aggregate: swarm.aggregate,
                            child_progress: crate::multi_agent::SwarmChildProgress {
                                item_index: child.item_index,
                                progress,
                            },
                        }];
                    }
                }
                Vec::new()
            }
            _ => vec![event.clone()],
        }
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
        let text_changed = progress.latest_text != last.latest_text
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
            },
            AgentEvent::ThinkingStarted {
                turn: 1,
                id: "thinking".to_owned(),
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
        assert!(winning.len() >= 1);
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
}
