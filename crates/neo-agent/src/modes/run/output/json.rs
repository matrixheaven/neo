use std::path::{Path, PathBuf};

use neo_agent_core::{AgentEvent, AgentMessage, Content, instructions::InstructionEpochData};
use serde_json::{Value, json};

use crate::config::{AppConfig, neo_home};
use crate::modes::run::PromptTurn;

pub(crate) fn stable_json_output(turn: &PromptTurn, config: &AppConfig) -> anyhow::Result<String> {
    let mut output = String::new();
    write_json_line(
        &mut output,
        &json!({
            "type": "session",
            "version": 1,
            "id": turn.session_id,
            "timestamp": current_unix_timestamp(),
            "cwd": config.project_dir,
        }),
    )?;

    let mut state = StableJsonState::with_instruction_roots(config.project_dir.clone(), neo_home());
    for event in &turn.events {
        for value in state.map_event(event) {
            write_json_line(&mut output, &value)?;
        }
    }
    Ok(output)
}

pub(crate) fn stable_instruction_epoch_event(
    epoch: &InstructionEpochData,
    config: &AppConfig,
) -> Value {
    let state = StableJsonState::with_instruction_roots(config.project_dir.clone(), neo_home());
    instruction_epoch_json(epoch, &state.primary_workspace, state.neo_home.as_deref())
}

fn write_json_line(output: &mut String, value: &Value) -> anyhow::Result<()> {
    output.push_str(&serde_json::to_string(value)?);
    output.push('\n');
    Ok(())
}

#[derive(Debug, Default)]
struct StableJsonState {
    assistant_content: Vec<AssistantContentState>,
    active_text_index: Option<usize>,
    active_thinking_index: Option<usize>,
    assistant_message_id: Option<String>,
    assistant_stop_reason: Option<neo_agent_core::StopReason>,
    messages: Vec<Value>,
    tool_results: Vec<Value>,
    primary_workspace: PathBuf,
    neo_home: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AssistantContentState {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
        redacted: bool,
    },
}

impl StableJsonState {
    fn with_instruction_roots(primary_workspace: PathBuf, neo_home: Option<PathBuf>) -> Self {
        let primary_workspace = primary_workspace
            .canonicalize()
            .unwrap_or(primary_workspace);
        let neo_home = neo_home.map(|path| path.canonicalize().unwrap_or(path));
        Self {
            primary_workspace,
            neo_home,
            ..Self::default()
        }
    }

    fn map_event(&mut self, event: &AgentEvent) -> Vec<Value> {
        if let Some(value) = self.map_lifecycle_event(event) {
            return vec![value];
        }
        if let Some(value) = self.map_tool_execution_event(event) {
            return vec![value];
        }
        self.map_other_event(event)
    }

    fn map_lifecycle_event(&mut self, event: &AgentEvent) -> Option<Value> {
        match event {
            AgentEvent::RunStarted { .. } => Some(json!({ "type": "agent_start" })),
            AgentEvent::TurnStarted { turn } => Some(json!({
                "type": "turn_start",
                "turn": turn,
            })),
            AgentEvent::RetryScheduled {
                turn,
                retry,
                max_retries,
                delay_ms,
                error_code,
                ..
            } => {
                self.reset_assistant_attempt();
                Some(json!({
                    "type": "retry_scheduled",
                    "turn": turn,
                    "retry": retry,
                    "maxRetries": max_retries,
                    "delayMs": delay_ms,
                    "errorCode": error_code,
                }))
            }
            AgentEvent::RetryStarted {
                turn,
                retry,
                max_retries,
            } => Some(json!({
                "type": "retry_started",
                "turn": turn,
                "retry": retry,
                "maxRetries": max_retries,
            })),
            AgentEvent::RetryResumed { turn, retry } => Some(json!({
                "type": "retry_resumed",
                "turn": turn,
                "retry": retry,
            })),
            AgentEvent::RetrySucceeded { turn, retries_used } => Some(json!({
                "type": "retry_succeeded",
                "turn": turn,
                "retriesUsed": retries_used,
            })),
            AgentEvent::RetryExhausted {
                turn,
                retries_used,
                error_code,
                message,
            } => {
                self.reset_assistant_attempt();
                Some(json!({
                    "type": "retry_exhausted",
                    "turn": turn,
                    "retriesUsed": retries_used,
                    "errorCode": error_code,
                    "message": message,
                }))
            }
            AgentEvent::MessageStarted { turn, id } => Some(self.map_message_started(*turn, id)),
            AgentEvent::ThinkingStarted { turn, id: _ } => Some(self.map_thinking_started(*turn)),
            AgentEvent::ThinkingDelta { turn, text } => Some(self.map_thinking_delta(*turn, text)),
            AgentEvent::ThinkingFinished {
                turn,
                signature,
                redacted,
            } => Some(self.map_thinking_finished(*turn, signature.as_ref(), *redacted)),
            AgentEvent::TextDelta { turn, text } => Some(self.map_text_delta(*turn, text)),
            AgentEvent::MessageFinished {
                turn,
                id: _,
                stop_reason,
            } => {
                self.assistant_stop_reason = Some(*stop_reason);
                Some(json!({
                    "type": "message_end",
                    "turn": turn,
                    "message": self.assistant_message(),
                }))
            }
            AgentEvent::TurnFinished { turn, stop_reason } => Some(json!({
                "type": "turn_end",
                "turn": turn,
                "stopReason": stable_stop_reason(*stop_reason),
                "message": self.assistant_message(),
                "toolResults": self.tool_results,
            })),
            AgentEvent::RunFinished { turn, stop_reason } => Some(json!({
                "type": "agent_end",
                "turn": turn,
                "stopReason": stable_stop_reason(*stop_reason),
                "messages": self.messages,
            })),
            _ => None,
        }
    }

    fn map_tool_execution_event(&mut self, event: &AgentEvent) -> Option<Value> {
        match event {
            AgentEvent::ToolExecutionStarted {
                turn,
                id,
                name,
                arguments,
            } => Some(json!({
                "type": "tool_execution_start",
                "turn": turn,
                "toolCallId": id,
                "toolName": name,
                "args": arguments,
            })),
            AgentEvent::ToolExecutionUpdate {
                turn,
                id,
                name,
                partial_result,
            } => Some(json!({
                "type": "tool_execution_update",
                "turn": turn,
                "toolCallId": id,
                "toolName": name,
                "partialResult": partial_result,
            })),
            AgentEvent::ToolExecutionFinished {
                turn,
                id,
                name,
                result,
            } => {
                let result_message = json!({
                    "role": "tool",
                    "toolCallId": id,
                    "toolName": name,
                    "content": result.content,
                    "isError": result.is_error,
                });
                push_unique(&mut self.tool_results, result_message);
                Some(json!({
                    "type": "tool_execution_end",
                    "turn": turn,
                    "toolCallId": id,
                    "toolName": name,
                    "result": result,
                    "isError": result.is_error,
                }))
            }
            _ => None,
        }
    }

    fn map_other_event(&mut self, event: &AgentEvent) -> Vec<Value> {
        match event {
            AgentEvent::MessageAppended { message } => {
                push_unique(&mut self.messages, stable_message(message));
                Vec::new()
            }
            AgentEvent::InstructionEpoch { epoch } => vec![instruction_epoch_json(
                epoch,
                &self.primary_workspace,
                self.neo_home.as_deref(),
            )],
            AgentEvent::Error { turn, message, .. } => {
                self.reset_assistant_attempt();
                vec![json!({
                    "type": "error",
                    "turn": turn,
                    "message": message,
                })]
            }
            AgentEvent::QueueDrained { kind, count } => vec![json!({
                "type": "queue_update",
                "kind": format!("{kind:?}").to_lowercase(),
                "count": count,
            })],
            AgentEvent::CompactionStarted {
                reason,
                tokens_before,
                message_count,
            } => vec![json!({
                "type": "compaction_start",
                "reason": stable_compaction_reason(*reason),
                "tokensBefore": tokens_before,
                "messageCount": message_count,
            })],
            AgentEvent::CompactionProgress { phase, percent } => vec![json!({
                "type": "compaction_update",
                "phase": stable_compaction_phase(*phase),
                "percent": percent,
            })],
            AgentEvent::CompactionApplied { summary } => vec![json!({
                "type": "compaction_end",
                "reason": "threshold",
                "result": summary,
                "aborted": false,
                "willRetry": false,
            })],
            _ => Vec::new(),
        }
    }

    fn map_message_started(&mut self, turn: u32, id: &str) -> Value {
        self.reset_assistant_attempt();
        self.assistant_message_id = Some(id.to_owned());
        json!({
            "type": "message_start",
            "turn": turn,
            "message": self.assistant_message(),
        })
    }

    fn reset_assistant_attempt(&mut self) {
        self.assistant_content.clear();
        self.active_text_index = None;
        self.active_thinking_index = None;
        self.assistant_message_id = None;
        self.assistant_stop_reason = None;
    }

    fn map_thinking_started(&mut self, turn: u32) -> Value {
        let content_index = self.push_thinking_content();
        json!({
            "type": "message_update",
            "turn": turn,
            "message": self.assistant_message(),
            "assistantMessageEvent": {
                "type": "thinking_start",
                "contentIndex": content_index,
                "partial": self.content_part(content_index),
            },
        })
    }

    fn map_thinking_delta(&mut self, turn: u32, text: &str) -> Value {
        let content_index = self.ensure_active_thinking_content();
        if let Some(AssistantContentState::Thinking { thinking, .. }) =
            self.assistant_content.get_mut(content_index)
        {
            thinking.push_str(text);
        }
        json!({
            "type": "message_update",
            "turn": turn,
            "message": self.assistant_message(),
            "assistantMessageEvent": {
                "type": "thinking_delta",
                "contentIndex": content_index,
                "delta": text,
                "partial": self.content_part(content_index),
            },
        })
    }

    fn map_thinking_finished(
        &mut self,
        turn: u32,
        signature: Option<&String>,
        redacted: bool,
    ) -> Value {
        let content_index = self.ensure_active_thinking_content();
        if let Some(AssistantContentState::Thinking {
            signature: state_signature,
            redacted: state_redacted,
            ..
        }) = self.assistant_content.get_mut(content_index)
        {
            *state_signature = signature.cloned();
            *state_redacted = redacted;
        }
        let content = self
            .assistant_content
            .get(content_index)
            .and_then(AssistantContentState::thinking_text)
            .unwrap_or_default();
        let partial = self.content_part(content_index);
        self.active_thinking_index = None;
        json!({
            "type": "message_update",
            "turn": turn,
            "message": self.assistant_message(),
            "assistantMessageEvent": {
                "type": "thinking_end",
                "contentIndex": content_index,
                "content": content,
                "partial": partial,
            },
        })
    }

    fn map_text_delta(&mut self, turn: u32, text: &str) -> Value {
        let content_index = self.ensure_active_text_content();
        if let Some(AssistantContentState::Text { text: state_text }) =
            self.assistant_content.get_mut(content_index)
        {
            state_text.push_str(text);
        }
        json!({
            "type": "message_update",
            "turn": turn,
            "message": self.assistant_message(),
            "assistantMessageEvent": {
                "type": "text_delta",
                "contentIndex": content_index,
                "delta": text,
                "partial": self.content_part(content_index),
            },
        })
    }

    fn assistant_message(&self) -> Value {
        json!({
            "role": "assistant",
            "id": self.assistant_message_id,
            "content": self.assistant_content(),
            "toolCalls": [],
            "stopReason": self.assistant_stop_reason.map(stable_stop_reason),
        })
    }

    fn assistant_content(&self) -> Vec<Value> {
        self.assistant_content
            .iter()
            .map(AssistantContentState::to_json)
            .collect()
    }

    fn content_part(&self, index: usize) -> Value {
        self.assistant_content
            .get(index)
            .map_or(Value::Null, AssistantContentState::to_json)
    }

    fn push_thinking_content(&mut self) -> usize {
        self.assistant_content
            .push(AssistantContentState::Thinking {
                thinking: String::new(),
                signature: None,
                redacted: false,
            });
        let index = self.assistant_content.len() - 1;
        self.active_thinking_index = Some(index);
        self.active_text_index = None;
        index
    }

    fn ensure_active_thinking_content(&mut self) -> usize {
        if let Some(index) = self.active_thinking_index
            && matches!(
                self.assistant_content.get(index),
                Some(AssistantContentState::Thinking { .. })
            )
        {
            return index;
        }
        self.push_thinking_content()
    }

    fn ensure_active_text_content(&mut self) -> usize {
        if let Some(index) = self.active_text_index
            && matches!(
                self.assistant_content.get(index),
                Some(AssistantContentState::Text { .. })
            )
        {
            return index;
        }
        self.assistant_content.push(AssistantContentState::Text {
            text: String::new(),
        });
        let index = self.assistant_content.len() - 1;
        self.active_text_index = Some(index);
        index
    }
}

impl AssistantContentState {
    fn to_json(&self) -> Value {
        match self {
            Self::Text { text } => json!({
                "type": "text",
                "text": text,
            }),
            Self::Thinking {
                thinking,
                signature,
                redacted,
            } => json!({
                "type": "thinking",
                "thinking": thinking,
                "thinkingSignature": signature,
                "redacted": redacted,
            }),
        }
    }

    fn thinking_text(&self) -> Option<String> {
        match self {
            Self::Thinking { thinking, .. } => Some(thinking.clone()),
            Self::Text { .. } => None,
        }
    }
}

fn push_unique(values: &mut Vec<Value>, value: Value) {
    if values.last() != Some(&value) {
        values.push(value);
    }
}

fn instruction_epoch_json(
    epoch: &InstructionEpochData,
    primary_workspace: &Path,
    neo_home: Option<&Path>,
) -> Value {
    let display_path = |path: &Path| display_instruction_path(path, primary_workspace, neo_home);
    let scopes = epoch
        .scopes
        .iter()
        .map(|scope| {
            json!({
                "display_path": display_path(&scope.display_path),
                "kind": scope.kind,
                "revision": scope.revision,
                "token_estimate": scope.token_estimate,
            })
        })
        .collect::<Vec<_>>();
    let selected_bundles = epoch
        .selected_bundles
        .iter()
        .map(|bundle| {
            json!({
                "display_path": display_path(&bundle.display_path),
                "revision": bundle.revision,
                "token_estimate": bundle.token_estimate,
                "byte_size": bundle.byte_size,
                "source_count": bundle.source_count,
                "import_count": bundle.import_count,
                "import_paths": bundle.import_paths.iter().map(|path| display_path(path)).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let ignored_bundles = epoch
        .ignored_bundles
        .iter()
        .map(|bundle| {
            json!({
                "display_path": display_path(&bundle.display_path),
                "revision": bundle.revision,
                "token_estimate": bundle.token_estimate,
                "reason": bundle.reason,
            })
        })
        .collect::<Vec<_>>();
    let replacements = epoch
        .replacements
        .iter()
        .map(|replacement| {
            json!({
                "display_path": display_path(&replacement.display_path),
                "previous_revision": replacement.previous_revision,
                "new_revision": replacement.new_revision,
            })
        })
        .collect::<Vec<_>>();
    let failure = epoch.failure.as_ref().map(|failure| {
        json!({
            "fingerprint": failure.fingerprint,
            "display_path": display_path(&failure.display_path),
            "kind": failure.kind,
        })
    });

    json!({
        "type": "instruction_epoch",
        "agentId": epoch.agent_id,
        "generation": epoch.generation,
        "outcome": epoch.outcome,
        "scopes": scopes,
        "selectedBundles": selected_bundles,
        "ignoredBundles": ignored_bundles,
        "replacements": replacements,
        "failure": failure,
        "deferredToolIds": epoch.deferred_tool_ids,
        "budget": epoch.budget,
    })
}

fn display_instruction_path(
    path: &Path,
    primary_workspace: &Path,
    neo_home: Option<&Path>,
) -> String {
    if !primary_workspace.as_os_str().is_empty() {
        if path == primary_workspace {
            return ".".to_owned();
        }
        if let Ok(relative) = path.strip_prefix(primary_workspace) {
            return relative.display().to_string();
        }
    }
    if let Some(neo_home) = neo_home
        && !neo_home.as_os_str().is_empty()
    {
        if path == neo_home {
            return "$NEO_HOME".to_owned();
        }
        if let Ok(relative) = path.strip_prefix(neo_home) {
            return Path::new("$NEO_HOME").join(relative).display().to_string();
        }
    }
    if path.is_absolute() {
        return "<outside-workspace>".to_owned();
    }
    path.display().to_string()
}

fn stable_message(message: &AgentMessage) -> Value {
    match message {
        AgentMessage::System { content } => json!({
            "role": "system",
            "content": stable_content(content),
        }),
        AgentMessage::Instruction { generation, .. } => json!({
            "role": "instruction",
            "generation": generation,
        }),
        AgentMessage::User {
            content, origin, ..
        } => {
            let mut message = json!({
                "role": "user",
                "content": stable_content(content),
            });
            if !origin.is_user() {
                message["origin"] = json!(origin);
            }
            message
        }
        AgentMessage::Assistant {
            content,
            tool_calls,
            stop_reason,
        } => json!({
            "role": "assistant",
            "content": stable_content(content),
            "toolCalls": tool_calls,
            "stopReason": stable_stop_reason(*stop_reason),
        }),
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } => json!({
            "role": "tool",
            "toolCallId": tool_call_id,
            "toolName": tool_name,
            "content": stable_content(content),
            "isError": is_error,
        }),
        AgentMessage::ShellCommand {
            command,
            stdout,
            stderr,
            exit_code,
            outcome,
            truncated,
        } => json!({
            "role": "shell",
            "command": command,
            "stdout": stdout,
            "stderr": stderr,
            "exitCode": exit_code,
            "outcome": outcome,
            "truncated": truncated,
        }),
    }
}

fn stable_content(content: &[Content]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            Content::Text { text } => json!({
                "type": "text",
                "text": text,
            }),
            Content::Thinking {
                text,
                signature,
                redacted,
            } => json!({
                "type": "thinking",
                "thinking": text,
                "thinkingSignature": signature,
                "redacted": redacted,
            }),
            Content::Image { mime_type, data } => json!({
                "type": "image",
                "mimeType": mime_type,
                "data": data,
            }),
        })
        .collect()
}

fn stable_stop_reason(stop_reason: neo_agent_core::StopReason) -> &'static str {
    match stop_reason {
        neo_agent_core::StopReason::EndTurn => "end_turn",
        neo_agent_core::StopReason::ToolUse => "tool_use",
        neo_agent_core::StopReason::MaxTokens => "max_tokens",
        neo_agent_core::StopReason::Cancelled => "cancelled",
        neo_agent_core::StopReason::Error => "error",
    }
}

fn stable_compaction_reason(reason: neo_agent_core::CompactionReason) -> &'static str {
    match reason {
        neo_agent_core::CompactionReason::Threshold => "threshold",
        neo_agent_core::CompactionReason::Manual => "manual",
    }
}

fn stable_compaction_phase(phase: neo_agent_core::CompactionPhase) -> &'static str {
    match phase {
        neo_agent_core::CompactionPhase::Estimating => "estimating",
        neo_agent_core::CompactionPhase::SelectingBoundary => "selecting_boundary",
        neo_agent_core::CompactionPhase::Summarizing => "summarizing",
        neo_agent_core::CompactionPhase::Applying => "applying",
    }
}

pub(crate) fn current_unix_timestamp() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", duration.as_secs(), duration.subsec_nanos())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use neo_agent_core::instructions::{InstructionEpochData, InstructionEpochOutcome};
    use neo_agent_core::{AgentEvent, AgentMessage, Content};

    use super::{StableJsonState, display_instruction_path, stable_message};

    fn instruction_epoch_with_body(body: &str) -> InstructionEpochData {
        InstructionEpochData {
            agent_id: "main".to_owned(),
            generation: 7,
            outcome: InstructionEpochOutcome::Updated,
            scopes: Vec::new(),
            selected_bundles: Vec::new(),
            ignored_bundles: Vec::new(),
            replacements: Vec::new(),
            failure: None,
            deferred_tool_ids: vec!["call-1".to_owned()],
            budget: neo_agent_core::instructions::InstructionBudget {
                nominal: 65_536,
                actual: 65_536,
            },
            model_content: Some(body.to_owned()),
        }
    }

    #[test]
    fn instruction_epoch_is_stable_metadata_without_model_content() {
        let mut state = StableJsonState::default();
        let records = state.map_event(&AgentEvent::InstructionEpoch {
            epoch: instruction_epoch_with_body("SECRET INSTRUCTION BODY"),
        });

        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "instruction_epoch");
        assert_eq!(records[0]["agentId"], "main");
        assert_eq!(records[0]["generation"], 7);
        assert_eq!(records[0]["outcome"], "updated");
        assert_eq!(records[0]["deferredToolIds"], serde_json::json!(["call-1"]));
        let encoded = serde_json::to_string(&records[0]).expect("stable JSON");
        assert!(!encoded.contains("SECRET INSTRUCTION BODY"));
        assert!(!encoded.contains("model_content"));
        assert!(!encoded.contains("modelContent"));
    }

    #[test]
    fn instruction_message_is_stable_metadata_without_body() {
        let record = stable_message(&AgentMessage::Instruction {
            generation: 7,
            content: vec![Content::text("SECRET INSTRUCTION BODY")],
        });

        assert_eq!(record["role"], "instruction");
        assert_eq!(record["generation"], 7);
        assert!(record.get("content").is_none());
        assert!(!record.to_string().contains("SECRET INSTRUCTION BODY"));
    }

    #[test]
    fn instruction_path_projection_treats_empty_roots_as_untrusted() {
        let path = Path::new("/private/secret/AGENTS.md");

        assert_eq!(
            display_instruction_path(path, Path::new(""), Some(Path::new(""))),
            "<outside-workspace>"
        );
    }

    #[test]
    fn retry_events_are_stable_json_records() {
        let mut state = StableJsonState::default();

        let _ = state.map_event(&AgentEvent::MessageStarted {
            turn: 1,
            id: "message-1".to_owned(),
        });
        let _ = state.map_event(&AgentEvent::TextDelta {
            turn: 1,
            text: "failed partial".to_owned(),
        });
        assert_eq!(
            state.map_event(&AgentEvent::RetryScheduled {
                turn: 1,
                retry: 1,
                max_retries: 5,
                delay_ms: 500,
                error_code: "provider.transport_error".to_owned(),
                message: "transport error: body closed".to_owned(),
            }),
            vec![serde_json::json!({
                "type": "retry_scheduled",
                "turn": 1,
                "retry": 1,
                "maxRetries": 5,
                "delayMs": 500,
                "errorCode": "provider.transport_error",
            })]
        );
        assert_eq!(
            state.map_event(&AgentEvent::RetryStarted {
                turn: 1,
                retry: 1,
                max_retries: 5,
            }),
            vec![serde_json::json!({
                "type": "retry_started",
                "turn": 1,
                "retry": 1,
                "maxRetries": 5,
            })]
        );
        assert_eq!(
            state.map_event(&AgentEvent::RetryResumed { turn: 1, retry: 1 }),
            vec![serde_json::json!({
                "type": "retry_resumed",
                "turn": 1,
                "retry": 1,
            })]
        );

        let _ = state.map_event(&AgentEvent::TextDelta {
            turn: 1,
            text: "winning answer".to_owned(),
        });
        let message_end = state.map_event(&AgentEvent::MessageFinished {
            turn: 1,
            id: "message-1".to_owned(),
            stop_reason: neo_agent_core::StopReason::EndTurn,
        });
        assert_eq!(
            message_end[0]["message"]["content"],
            serde_json::json!([{"type": "text", "text": "winning answer"}])
        );
        assert_eq!(
            state.map_event(&AgentEvent::RetrySucceeded {
                turn: 1,
                retries_used: 1,
            }),
            vec![serde_json::json!({
                "type": "retry_succeeded",
                "turn": 1,
                "retriesUsed": 1,
            })]
        );
        assert_eq!(
            state.map_event(&AgentEvent::RetryExhausted {
                turn: 1,
                retries_used: 5,
                error_code: "provider.transport_error".to_owned(),
                message: "transport error: body closed".to_owned(),
            }),
            vec![serde_json::json!({
                "type": "retry_exhausted",
                "turn": 1,
                "retriesUsed": 5,
                "errorCode": "provider.transport_error",
                "message": "transport error: body closed",
            })]
        );
    }

    #[test]
    fn retry_exhausted_clears_failed_assistant_attempt() {
        let mut state = StableJsonState::default();
        let _ = state.map_event(&AgentEvent::MessageStarted {
            turn: 1,
            id: "failed-message".to_owned(),
        });
        let _ = state.map_event(&AgentEvent::TextDelta {
            turn: 1,
            text: "failed partial".to_owned(),
        });

        let exhausted = state.map_event(&AgentEvent::RetryExhausted {
            turn: 1,
            retries_used: 1,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: body closed".to_owned(),
        });
        let turn_end = state.map_event(&AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: neo_agent_core::StopReason::Error,
        });

        assert_eq!(exhausted[0]["type"], "retry_exhausted");
        assert_eq!(turn_end[0]["message"]["id"], serde_json::Value::Null);
        assert_eq!(turn_end[0]["message"]["content"], serde_json::json!([]));
        assert_eq!(
            turn_end[0]["message"]["stopReason"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn terminal_error_clears_failed_assistant_attempt() {
        let mut state = StableJsonState::default();
        let _ = state.map_event(&AgentEvent::MessageStarted {
            turn: 1,
            id: "failed-message".to_owned(),
        });
        let _ = state.map_event(&AgentEvent::TextDelta {
            turn: 1,
            text: "failed partial".to_owned(),
        });

        let error = state.map_event(&AgentEvent::Error {
            turn: 1,
            message: "transport error: body closed".to_owned(),
            code: Some("provider.transport_error".to_owned()),
            retry_after: None,
        });
        let turn_end = state.map_event(&AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: neo_agent_core::StopReason::Error,
        });

        assert_eq!(error[0]["type"], "error");
        assert_eq!(turn_end[0]["message"]["id"], serde_json::Value::Null);
        assert_eq!(turn_end[0]["message"]["content"], serde_json::json!([]));
        assert_eq!(
            turn_end[0]["message"]["stopReason"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn stable_json_maps_compaction_lifecycle_events() {
        let mut state = StableJsonState::default();

        assert_eq!(
            state.map_event(&AgentEvent::CompactionStarted {
                reason: neo_agent_core::CompactionReason::Threshold,
                tokens_before: 12_345,
                message_count: 8,
            }),
            vec![serde_json::json!({
                "type": "compaction_start",
                "reason": "threshold",
                "tokensBefore": 12_345,
                "messageCount": 8,
            })]
        );
        assert_eq!(
            state.map_event(&AgentEvent::CompactionProgress {
                phase: neo_agent_core::CompactionPhase::Summarizing,
                percent: 70,
            }),
            vec![serde_json::json!({
                "type": "compaction_update",
                "phase": "summarizing",
                "percent": 70,
            })]
        );
        assert_eq!(
            state.map_event(&AgentEvent::CompactionApplied {
                summary: neo_agent_core::CompactionSummary {
                    summary: "Older context summarized.".to_owned(),
                    tokens_before: 12_345,
                    tokens_after: 6_000,
                    first_kept_message_index: 4,
                },
            }),
            vec![serde_json::json!({
                "type": "compaction_end",
                "reason": "threshold",
                "result": {
                    "summary": "Older context summarized.",
                    "tokens_before": 12_345,
                    "tokens_after": 6_000,
                    "first_kept_message_index": 4,
                },
                "aborted": false,
                "willRetry": false,
            })]
        );
    }
}
