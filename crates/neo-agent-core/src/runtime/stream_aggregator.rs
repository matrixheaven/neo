use std::sync::Arc;

use futures::StreamExt;
use neo_ai::{AiStreamEvent, ChatRequest, ModelClient};
use tokio_util::sync::CancellationToken;

use super::config::AgentConfig;
use super::error::AgentRuntimeError;
use super::events::EventEmitter;
use super::turn_loop::emit_effective_context_window;
use crate::{AgentEvent, AgentMessage, AgentTokenUsage, AgentToolCall, Content, StopReason};

pub(super) async fn run_model_turn(
    model: Arc<dyn ModelClient>,
    config: &AgentConfig,
    request: ChatRequest,
    turn: u32,
    emitter: &mut EventEmitter,
    cancel_token: CancellationToken,
) -> Result<Option<AgentMessage>, AgentRuntimeError> {
    emitter.emit(AgentEvent::TurnStarted { turn });
    let mut state = ModelTurnState::new();
    let mut stream = model.stream_chat(request);

    while let Some(event) = next_model_event(&mut stream, &cancel_token).await {
        if cancel_token.is_cancelled() {
            state.finish_current_message(turn, StopReason::Cancelled, emitter);
            break;
        }
        state.apply_model_event(config, turn, event?, emitter);
    }

    let stop_reason = state.stop_reason;
    let message = state.into_assistant_message(stop_reason);
    emitter.emit(AgentEvent::MessageAppended {
        message: message.clone(),
    });
    emit_effective_context_window(config, emitter, turn).await;
    emitter.emit(AgentEvent::TurnFinished { turn, stop_reason });
    Ok(Some(message))
}

struct ModelTurnState {
    content: Vec<Content>,
    active_text_index: Option<usize>,
    active_thinking_index: Option<usize>,
    tool_calls: Vec<AgentToolCall>,
    tool_names: std::collections::HashMap<String, String>,
    current_message_id: Option<String>,
    stop_reason: StopReason,
}

impl ModelTurnState {
    fn new() -> Self {
        Self {
            content: Vec::new(),
            active_text_index: None,
            active_thinking_index: None,
            tool_calls: Vec::new(),
            tool_names: std::collections::HashMap::new(),
            current_message_id: None,
            stop_reason: StopReason::EndTurn,
        }
    }

    fn apply_model_event(
        &mut self,
        config: &AgentConfig,
        turn: u32,
        event: AiStreamEvent,
        emitter: &mut EventEmitter,
    ) {
        match event {
            AiStreamEvent::MessageStart { id } => self.start_message(turn, id, emitter),
            AiStreamEvent::TextDelta { text } => self.apply_text_delta(turn, text, emitter),
            AiStreamEvent::ThinkingStart { id } => self.start_thinking(turn, id, emitter),
            AiStreamEvent::ThinkingDelta { text } => {
                self.apply_thinking_delta(turn, text, emitter);
            }
            AiStreamEvent::ThinkingEnd {
                signature,
                redacted,
            } => self.finish_thinking(turn, signature, redacted, emitter),
            AiStreamEvent::ToolCallStart { id, name } => {
                self.start_tool_call(turn, id, name, emitter);
            }
            AiStreamEvent::ToolCallArgsDelta { id, json_fragment } => {
                emitter.emit(AgentEvent::ToolCallArgumentsDelta {
                    turn,
                    id,
                    json_fragment,
                });
            }
            AiStreamEvent::ToolCallEnd { id, raw_arguments } => {
                self.finish_tool_call(turn, id, raw_arguments, emitter);
            }
            AiStreamEvent::MessageEnd { stop_reason, usage } => {
                if let Some(usage) = usage {
                    let usage = AgentTokenUsage::from(usage);
                    emitter.emit(AgentEvent::TokenUsage { turn, usage });
                    maybe_advance_micro_compaction_cutoff(config, usage, emitter);
                }
                self.finish_current_message(turn, stop_reason.into(), emitter);
            }
            AiStreamEvent::Error { message } => {
                emitter.emit(AgentEvent::Error {
                    turn,
                    message: message.clone(),
                    code: None,
                    retry_after: None,
                });
                self.finish_current_message(turn, StopReason::Error, emitter);
            }
        }
    }

    fn start_message(&mut self, turn: u32, id: String, emitter: &mut EventEmitter) {
        self.current_message_id = Some(id.clone());
        emitter.emit(AgentEvent::MessageStarted { turn, id });
    }

    fn apply_text_delta(&mut self, turn: u32, text: String, emitter: &mut EventEmitter) {
        self.append_text(&text);
        emitter.emit(AgentEvent::TextDelta { turn, text });
    }

    fn start_thinking(&mut self, turn: u32, id: String, emitter: &mut EventEmitter) {
        self.content.push(Content::thinking("", None, false));
        self.active_thinking_index = Some(self.content.len() - 1);
        self.active_text_index = None;
        emitter.emit(AgentEvent::ThinkingStarted { turn, id });
    }

    fn apply_thinking_delta(&mut self, turn: u32, text: String, emitter: &mut EventEmitter) {
        let index = self.ensure_active_thinking();
        if let Some(Content::Thinking { text: thinking, .. }) = self.content.get_mut(index) {
            let mut s = String::from(&**thinking);
            s.push_str(&text);
            *thinking = Arc::from(s);
        }
        emitter.emit(AgentEvent::ThinkingDelta { turn, text });
    }

    fn finish_thinking(
        &mut self,
        turn: u32,
        signature: Option<String>,
        redacted: bool,
        emitter: &mut EventEmitter,
    ) {
        let index = self.ensure_active_thinking();
        if let Some(Content::Thinking {
            signature: thinking_signature,
            redacted: thinking_redacted,
            ..
        }) = self.content.get_mut(index)
        {
            *thinking_signature = signature.map(Arc::from);
            *thinking_redacted = redacted;
        }
        emitter.emit(AgentEvent::ThinkingFinished {
            turn,
            signature: match self.content.get(index) {
                Some(Content::Thinking { signature, .. }) => {
                    signature.as_ref().map(|s| s.to_string())
                }
                _ => None,
            },
            redacted,
        });
        self.active_thinking_index = None;
    }

    fn start_tool_call(&mut self, turn: u32, id: String, name: String, emitter: &mut EventEmitter) {
        self.tool_names.insert(id.clone(), name.clone());
        emitter.emit(AgentEvent::ToolCallStarted { turn, id, name });
    }

    fn finish_tool_call(
        &mut self,
        turn: u32,
        id: String,
        raw_arguments: String,
        emitter: &mut EventEmitter,
    ) {
        let tool_call = AgentToolCall {
            name: self.tool_names.remove(&id).unwrap_or_default().into(),
            id: id.into(),
            raw_arguments: raw_arguments.into(),
        };
        emitter.emit(AgentEvent::ToolCallFinished {
            turn,
            tool_call: tool_call.clone(),
        });
        self.tool_calls.push(tool_call);
    }

    fn finish_current_message(
        &mut self,
        turn: u32,
        stop_reason: StopReason,
        emitter: &mut EventEmitter,
    ) {
        self.stop_reason = stop_reason;
        if let Some(id) = self.current_message_id.take() {
            emitter.emit(AgentEvent::MessageFinished {
                turn,
                id,
                stop_reason,
            });
        }
    }

    fn into_assistant_message(self, stop_reason: StopReason) -> AgentMessage {
        AgentMessage::assistant(self.content, self.tool_calls, stop_reason)
    }

    fn append_text(&mut self, delta: &str) {
        if let Some(index) = self.active_text_index
            && let Some(Content::Text { text }) = self.content.get_mut(index)
        {
            let mut s = String::from(&**text);
            s.push_str(delta);
            *text = Arc::from(s);
            return;
        }

        self.content.push(Content::text(delta));
        self.active_text_index = Some(self.content.len() - 1);
    }

    fn ensure_active_thinking(&mut self) -> usize {
        if let Some(index) = self.active_thinking_index
            && matches!(self.content.get(index), Some(Content::Thinking { .. }))
        {
            return index;
        }

        self.content.push(Content::thinking("", None, false));
        let index = self.content.len() - 1;
        self.active_thinking_index = Some(index);
        self.active_text_index = None;
        index
    }
}

fn maybe_advance_micro_compaction_cutoff(
    config: &AgentConfig,
    usage: AgentTokenUsage,
    emitter: &mut EventEmitter,
) {
    let Some(settings) = config.compaction else {
        return;
    };
    if !settings.micro_enabled
        || usage.input_cache_read_tokens > 0
        || usage.input_cache_write_tokens > 0
    {
        return;
    }
    let max_context_tokens = super::config::effective_max_context_tokens(config);
    if max_context_tokens == 0 {
        return;
    }
    let used_tokens = emitter.context.estimated_tokens();
    if used_tokens.saturating_mul(2) < max_context_tokens {
        return;
    }
    let next_cutoff = emitter
        .context
        .messages()
        .len()
        .saturating_sub(settings.micro_keep_recent);
    if next_cutoff > emitter.context.micro_compaction_cutoff() {
        emitter.emit(AgentEvent::MicroCompactionApplied {
            cutoff: next_cutoff,
        });
    }
}

async fn next_model_event(
    stream: &mut futures::stream::BoxStream<'_, Result<AiStreamEvent, neo_ai::AiError>>,
    cancel_token: &CancellationToken,
) -> Option<Result<AiStreamEvent, neo_ai::AiError>> {
    tokio::select! {
        event = stream.next() => event,
        () = cancel_token.cancelled() => Some(Err(neo_ai::AiError::Cancelled)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_ai::{ApiKind, ModelCapabilities, ModelSpec, ProviderId};
    use tokio::sync::mpsc;

    fn config() -> AgentConfig {
        AgentConfig::for_model(ModelSpec {
            provider: ProviderId("test".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::Local,
            capabilities: ModelCapabilities {
                max_context_tokens: Some(10),
                ..ModelCapabilities::chat()
            },
        })
        .with_compaction(super::super::config::CompactionSettings {
            micro_keep_recent: 1,
            ..super::super::config::CompactionSettings::new(1_000, 4)
        })
    }

    fn emitter_with_large_context() -> EventEmitter {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut context = super::super::context::AgentContext::new();
        context.append_message(AgentMessage::user_text("one two three four five"));
        context.append_message(AgentMessage::user_text("six seven eight nine ten"));
        context.append_message(AgentMessage::user_text("eleven twelve thirteen"));
        EventEmitter::new(tx, context)
    }

    #[test]
    fn micro_compaction_cutoff_advances_after_uncached_turn_without_cache_write() {
        let config = config();
        let mut emitter = emitter_with_large_context();

        maybe_advance_micro_compaction_cutoff(
            &config,
            AgentTokenUsage {
                input_tokens: 100,
                output_tokens: 1,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            },
            &mut emitter,
        );

        assert_eq!(emitter.context.micro_compaction_cutoff(), 2);
    }

    #[test]
    fn micro_compaction_cutoff_does_not_advance_on_cache_hit() {
        let config = config();
        let mut emitter = emitter_with_large_context();

        maybe_advance_micro_compaction_cutoff(
            &config,
            AgentTokenUsage {
                input_tokens: 100,
                output_tokens: 1,
                input_cache_read_tokens: 20,
                input_cache_write_tokens: 0,
            },
            &mut emitter,
        );

        assert_eq!(emitter.context.micro_compaction_cutoff(), 0);
    }

    #[test]
    fn micro_compaction_cutoff_does_not_advance_on_cache_write() {
        let config = config();
        let mut emitter = emitter_with_large_context();

        maybe_advance_micro_compaction_cutoff(
            &config,
            AgentTokenUsage {
                input_tokens: 100,
                output_tokens: 1,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 20,
            },
            &mut emitter,
        );

        assert_eq!(emitter.context.micro_compaction_cutoff(), 0);
    }
}
