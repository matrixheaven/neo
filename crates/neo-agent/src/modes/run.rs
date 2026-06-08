use std::{collections::BTreeMap, fmt::Write as _, path::Path, sync::Arc};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, CompactionSettings, Content,
    McpHttpConfig, McpHttpToolAdapter, McpStdioConfig, McpStdioToolAdapter, McpToolAdapter,
    McpToolProvider, PermissionDecision, ToolRegistry,
};
use neo_ai::{ModelClient, ModelRegistry, ModelSpec, ProviderRegistry};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::RunOutput,
    config::{AppConfig, McpServerConfig, provider_api_base},
    session_commands,
};

pub async fn execute(
    prompt: &[String],
    config: &AppConfig,
    output: RunOutput,
) -> anyhow::Result<String> {
    let turn = run_prompt(prompt, config).await?;
    if matches!(output, RunOutput::Json) {
        return stable_json_output(&turn, config);
    }

    let mut output = String::new();
    for event in turn.events {
        output.push_str(&serde_json::to_string(&event)?);
        output.push('\n');
    }
    Ok(output)
}

pub async fn resume(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = session_commands::resolve_session_id(session_ref, config)?;
    let transcript = session_commands::transcript(&session_id, config).await?;
    Ok(format!("session {session_id}\n{transcript}"))
}

fn stable_json_output(turn: &PromptTurn, config: &AppConfig) -> anyhow::Result<String> {
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

    let mut state = StableJsonState::default();
    for event in &turn.events {
        for value in state.map_event(event) {
            write_json_line(&mut output, &value)?;
        }
    }
    Ok(output)
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
            AgentEvent::Error { turn, message } => vec![json!({
                "type": "error",
                "turn": turn,
                "message": message,
            })],
            AgentEvent::QueueDrained { kind, count } => vec![json!({
                "type": "queue_update",
                "kind": format!("{kind:?}").to_lowercase(),
                "count": count,
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
        self.assistant_content.clear();
        self.active_text_index = None;
        self.active_thinking_index = None;
        self.assistant_message_id = Some(id.to_owned());
        self.assistant_stop_reason = None;
        json!({
            "type": "message_start",
            "turn": turn,
            "message": self.assistant_message(),
        })
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

fn stable_message(message: &AgentMessage) -> Value {
    match message {
        AgentMessage::System { content } => json!({
            "role": "system",
            "content": stable_content(content),
        }),
        AgentMessage::User { content } => json!({
            "role": "user",
            "content": stable_content(content),
        }),
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
        neo_agent_core::StopReason::MaxTurns => "max_turns",
        neo_agent_core::StopReason::Cancelled => "cancelled",
        neo_agent_core::StopReason::Error => "error",
    }
}

fn current_unix_timestamp() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", duration.as_secs(), duration.subsec_nanos())
}

pub fn list_models(config: &AppConfig) -> anyhow::Result<String> {
    let providers = provider_registry_for_config(config);
    let models = model_registry_for_config(config)?;
    let mut out = format!(
        "models:\n- {}/{} (configured default)\n",
        config.default_provider, config.default_model
    );
    for model in models.list() {
        let marker =
            if model.provider.0 == config.default_provider && model.model == config.default_model {
                " default"
            } else {
                ""
            };
        let _ = writeln!(
            out,
            "- {}/{} ({:?}{marker})",
            model.provider.0, model.model, model.api
        );
    }
    out.push_str("providers:\n");
    for provider in providers.list() {
        let status = providers
            .credential_status(&provider.id)
            .map_or("unknown", |status| {
                if status.configured {
                    "configured"
                } else {
                    "missing credentials"
                }
            });
        let _ = writeln!(out, "- {} ({:?}, {status})", provider.id, provider.api);
    }
    Ok(out)
}

fn provider_registry_for_config(config: &AppConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::production();
    apply_configured_provider_overrides(&mut registry, config);
    if let Some(env_name) = &config.api_key_env
        && let Some(mut provider) = registry.get(&config.default_provider).cloned()
    {
        provider.api_key_env_vars = vec![env_name.clone()];
        registry.register(provider);
    }
    registry
}

fn apply_configured_provider_overrides(registry: &mut ProviderRegistry, config: &AppConfig) {
    for (provider_id, provider_config) in &config.providers {
        let Some(mut provider) = registry.get(provider_id).cloned() else {
            continue;
        };
        if let Some(base_url) = &provider_config.api_base {
            provider.base_url = Some(base_url.clone());
        }
        if let Some(env_name) = &provider_config.api_key_env {
            provider.api_key_env_vars = vec![env_name.clone()];
        }
        registry.register(provider);
    }
}

pub fn list_mcp_servers(config: &AppConfig) -> String {
    if config.mcp.servers.is_empty() {
        return "no MCP servers configured\n".to_owned();
    }

    let mut out = String::new();
    for server in &config.mcp.servers {
        let state = if server.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let args = if server.args.is_empty() {
            String::new()
        } else {
            format!(" {}", server.args.join(" "))
        };
        let endpoint = if matches!(server.transport.as_str(), "http" | "sse") {
            server.url.as_deref().unwrap_or("")
        } else {
            server.command.as_deref().unwrap_or("")
        };
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}{}",
            server.id, state, server.transport, endpoint, args
        );
    }
    out
}

pub async fn list_mcp_resources(config: &AppConfig, server_id: &str) -> anyhow::Result<String> {
    let server = enabled_mcp_server(config, server_id)?;
    let adapter = mcp_adapter_for_server(server)?;
    let resources = adapter
        .list_resources()
        .await
        .with_context(|| format!("failed to list MCP resources from {server_id}"))?;
    if resources.is_empty() {
        return Ok("no MCP resources\n".to_owned());
    }
    let mut out = String::new();
    for resource in resources {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}",
            resource.uri,
            resource.name,
            resource.mime_type.unwrap_or_default(),
            resource.description.unwrap_or_default()
        );
    }
    Ok(out)
}

pub async fn read_mcp_resource(
    config: &AppConfig,
    server_id: &str,
    uri: &str,
) -> anyhow::Result<String> {
    let server = enabled_mcp_server(config, server_id)?;
    let adapter = mcp_adapter_for_server(server)?;
    let resource = adapter
        .read_resource(uri)
        .await
        .with_context(|| format!("failed to read MCP resource {uri} from {server_id}"))?;
    if resource.contents.is_empty() {
        return Ok("no MCP resource content\n".to_owned());
    }
    let mut out = String::new();
    for content in resource.contents {
        let _ = writeln!(
            out,
            "{}\t{}",
            content.uri,
            content.mime_type.unwrap_or_default()
        );
        if let Some(text) = content.text {
            out.push_str(&text);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        } else if let Some(blob) = content.blob {
            out.push_str(&blob);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    Ok(out)
}

pub async fn watch_mcp_resource(
    config: &AppConfig,
    server_id: &str,
    uri: &str,
    count: usize,
) -> anyhow::Result<String> {
    anyhow::ensure!(count > 0, "MCP resource watch count must be greater than 0");
    let server = enabled_mcp_server(config, server_id)?;
    let adapter = mcp_adapter_for_server(server)?;
    adapter
        .subscribe_resource(uri)
        .await
        .with_context(|| format!("failed to subscribe to MCP resource {uri} from {server_id}"))?;

    let mut out = String::new();
    let mut watch_result = Ok(());
    for _ in 0..count {
        match adapter.next_resource_update().await {
            Ok(update) => {
                let _ = writeln!(out, "{}", update.uri);
            }
            Err(err) => {
                watch_result = Err(err);
                break;
            }
        }
    }

    let unsubscribe_result = adapter.unsubscribe_resource(uri).await;
    if let Err(err) = watch_result {
        return Err(anyhow::Error::from(err).context(format!(
            "failed while watching MCP resource {uri} from {server_id}"
        )));
    }
    unsubscribe_result
        .with_context(|| format!("failed to unsubscribe from MCP resource {uri} on {server_id}"))?;
    Ok(out)
}

pub struct PromptTurn {
    pub session_id: String,
    pub events: Vec<AgentEvent>,
    pub assistant_text: String,
}

pub struct PromptApprovalRequest {
    pub id: String,
    pub decision_tx: oneshot::Sender<PermissionDecision>,
}

pub async fn run_prompt(prompt: &[String], config: &AppConfig) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, None).await?;
    finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        session_id,
    )
    .await
}

#[allow(dead_code)]
pub async fn run_prompt_in_session(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = session_commands::session_path(session_id, config)?;
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, None).await?;
    finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        session_id.to_owned(),
    )
    .await
}

pub async fn run_prompt_streaming(
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    cancel_token: CancellationToken,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, Some(approval_tx)).await?;
    let streaming = StreamingTurnIo {
        event_tx,
        session_id,
        cancel_token,
    };
    finish_prompt_turn_streaming(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        streaming,
    )
    .await
}

pub async fn run_prompt_in_session_streaming(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    cancel_token: CancellationToken,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = session_commands::session_path(session_id, config)?;
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, Some(approval_tx)).await?;
    let streaming = StreamingTurnIo {
        event_tx,
        session_id: session_id.to_owned(),
        cancel_token,
    };
    finish_prompt_turn_streaming(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        streaming,
    )
    .await
}

async fn runtime_for_config(
    config: &AppConfig,
    approval_tx: Option<mpsc::UnboundedSender<PromptApprovalRequest>>,
) -> anyhow::Result<AgentRuntime> {
    let model = resolve_model(config)?;
    let client = resolve_model_client(config, &model)?;
    let tools = tool_registry_for_config(config).await?;
    Ok(AgentRuntime::with_tools(
        agent_config_for_app(model, config, approval_tx)?,
        client,
        tools,
    ))
}

#[cfg(test)]
async fn run_prompt_with_runtime(
    prompt: String,
    context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
) -> anyhow::Result<PromptTurn> {
    let (user_message, events) = append_user_event(prompt, writer).await?;
    finish_prompt_turn(
        user_message,
        context,
        writer,
        runtime,
        events,
        "test-session".to_owned(),
    )
    .await
}

async fn append_user_event(
    prompt: String,
    writer: &mut JsonlSessionWriter,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let user_message = AgentMessage::user_text(prompt);
    let user_event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    writer.append_event(&user_event).await?;
    writer.flush().await?;
    Ok((user_message, vec![user_event]))
}

async fn finish_prompt_turn(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    mut events: Vec<AgentEvent>,
    session_id: String,
) -> anyhow::Result<PromptTurn> {
    let mut assistant_text = String::new();
    let turn_events = runtime
        .run_turn(&mut context, user_message.clone())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    for event in turn_events {
        let is_duplicate_user_message = matches!(
            &event,
            AgentEvent::MessageAppended { message } if message == &user_message
        );
        if is_duplicate_user_message {
            events.push(event);
            continue;
        }
        if let AgentEvent::MessageAppended { message } = &event
            && matches!(message, AgentMessage::Assistant { .. })
        {
            assistant_text.push_str(&message_text(message));
        }
        writer.append_event(&event).await?;
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id,
        events,
        assistant_text,
    })
}

struct StreamingTurnIo {
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    session_id: String,
    cancel_token: CancellationToken,
}

async fn finish_prompt_turn_streaming(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    initial_events: Vec<AgentEvent>,
    streaming: StreamingTurnIo,
) -> anyhow::Result<PromptTurn> {
    let mut events = Vec::new();
    for event in initial_events {
        let _ = streaming.event_tx.send(Ok(event.clone()));
        events.push(event);
    }

    let mut assistant_text = String::new();
    let mut stream =
        runtime.run_turn_with_cancel(&mut context, user_message.clone(), streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                let message = error.to_string();
                let _ = streaming
                    .event_tx
                    .send(Err(anyhow::anyhow!(message.clone())));
                anyhow::bail!(message);
            }
        };
        let is_duplicate_user_message = matches!(
            &event,
            AgentEvent::MessageAppended { message } if message == &user_message
        );
        if !is_duplicate_user_message {
            if let AgentEvent::MessageAppended { message } = &event
                && matches!(message, AgentMessage::Assistant { .. })
            {
                assistant_text.push_str(&message_text(message));
            }
            writer.append_event(&event).await?;
        }
        let _ = streaming.event_tx.send(Ok(event.clone()));
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id: streaming.session_id,
        events,
        assistant_text,
    })
}

fn agent_config_for_app(
    model: ModelSpec,
    config: &AppConfig,
    approval_tx: Option<mpsc::UnboundedSender<PromptApprovalRequest>>,
) -> anyhow::Result<AgentConfig> {
    let mut agent_config = AgentConfig::for_model(model)
        .with_tool_permission_policy(config.permissions.clone())
        .with_queue_modes(
            config.runtime.steering_queue_mode,
            config.runtime.follow_up_queue_mode,
        )
        .with_tool_execution_mode(config.runtime.tool_execution_mode)
        .with_workspace_root(&config.project_dir)?;
    agent_config.temperature = config.runtime.temperature;
    agent_config.max_tokens = config.runtime.max_tokens;
    agent_config.reasoning_effort = config.runtime.reasoning_effort;
    if let Some(compaction) = &config.runtime.compaction {
        agent_config = agent_config.with_compaction(CompactionSettings {
            enabled: compaction.enabled,
            max_estimated_tokens: compaction.max_estimated_tokens,
            keep_recent_messages: compaction.keep_recent_messages,
        });
    }
    if config.approve {
        agent_config = agent_config.with_approval_handler(|_| PermissionDecision::Allow);
    } else if config.no_approve {
        agent_config = agent_config.with_approval_handler(|_| PermissionDecision::Deny);
    } else if let Some(approval_tx) = approval_tx {
        agent_config = agent_config.with_async_approval_handler(move |request| {
            let approval_tx = approval_tx.clone();
            async move {
                let (decision_tx, decision_rx) = oneshot::channel();
                let id = request.id.clone();
                if approval_tx
                    .send(PromptApprovalRequest { id, decision_tx })
                    .is_err()
                {
                    return PermissionDecision::Deny;
                }
                decision_rx.await.unwrap_or(PermissionDecision::Deny)
            }
        });
    }
    Ok(agent_config)
}

async fn tool_registry_for_config(config: &AppConfig) -> anyhow::Result<ToolRegistry> {
    let mut registry = ToolRegistry::with_builtin_tools();
    for server in config.mcp.servers.iter().filter(|server| server.enabled) {
        register_mcp_server(&mut registry, server).await?;
    }
    Ok(registry)
}

async fn register_mcp_server(
    registry: &mut ToolRegistry,
    server: &McpServerConfig,
) -> anyhow::Result<()> {
    let adapter = mcp_adapter_for_server(server)?;
    let provider = McpToolProvider::discover_dyn(&server.id, adapter)
        .await
        .with_context(|| format!("failed to discover MCP tools from {}", server.id))?;
    provider.register_into(registry);
    Ok(())
}

fn enabled_mcp_server<'a>(
    config: &'a AppConfig,
    server_id: &str,
) -> anyhow::Result<&'a McpServerConfig> {
    let server = config
        .mcp
        .servers
        .iter()
        .find(|server| server.id == server_id)
        .with_context(|| format!("MCP server {server_id} is not configured"))?;
    anyhow::ensure!(server.enabled, "MCP server {server_id} is disabled");
    Ok(server)
}

fn mcp_adapter_for_server(server: &McpServerConfig) -> anyhow::Result<Arc<dyn McpToolAdapter>> {
    match server.transport.as_str() {
        "stdio" => {
            let command = server
                .command
                .clone()
                .with_context(|| format!("missing MCP command for {}", server.id))?;
            Ok(Arc::new(McpStdioToolAdapter::new(McpStdioConfig {
                command,
                args: server.args.clone(),
                env: server
                    .env
                    .iter()
                    .fold(BTreeMap::new(), |mut env, (key, value)| {
                        env.insert(key.clone(), value.clone());
                        env
                    }),
            })))
        }
        "http" | "sse" => {
            let url = server
                .url
                .clone()
                .with_context(|| format!("missing MCP url for {}", server.id))?;
            Ok(Arc::new(McpHttpToolAdapter::new(McpHttpConfig {
                url,
                headers: server.headers.iter().fold(
                    BTreeMap::new(),
                    |mut headers, (key, value)| {
                        headers.insert(key.clone(), value.clone());
                        headers
                    },
                ),
            })))
        }
        other => anyhow::bail!("unsupported MCP transport for {}: {other}", server.id),
    }
}

async fn create_session_path(config: &AppConfig) -> anyhow::Result<std::path::PathBuf> {
    tokio::fs::create_dir_all(&config.sessions_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create sessions directory {}",
                config.sessions_dir.display()
            )
        })?;

    let mut counter = 0_u32;
    loop {
        let suffix = if counter == 0 {
            String::new()
        } else {
            format!("-{counter}")
        };
        let path = config.sessions_dir.join(format!(
            "{}{suffix}.jsonl",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis()
        ));
        if tokio::fs::metadata(&path).await.is_err() {
            return Ok(path);
        }
        counter = counter.saturating_add(1);
    }
}

fn session_id_from_path(path: &Path) -> anyhow::Result<String> {
    path.file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .with_context(|| format!("invalid session path {}", path.display()))
}

fn resolve_model(config: &AppConfig) -> anyhow::Result<ModelSpec> {
    model_registry_for_config(config)?
        .get(&config.default_provider, &config.default_model)
        .cloned()
        .with_context(|| {
            format!(
                "unknown model {}/{}; run `neo models list` for supported catalog entries",
                config.default_provider, config.default_model
            )
        })
}

pub(crate) fn model_registry_for_config(config: &AppConfig) -> anyhow::Result<ModelRegistry> {
    let mut registry = ModelRegistry::seeded();
    for path in &config.model_catalogs {
        registry
            .load_catalog_path(path)
            .map_err(anyhow::Error::from)?;
    }
    Ok(registry)
}

fn resolve_model_client(
    config: &AppConfig,
    model: &ModelSpec,
) -> anyhow::Result<Arc<dyn ModelClient>> {
    let mut registry = ProviderRegistry::production();
    apply_configured_provider_overrides(&mut registry, config);
    if config.api_base.is_some()
        || config.api_key_env.is_some()
        || provider_api_base(&config.providers, &model.provider.0).is_some()
    {
        let Some(mut provider) = registry.get(&model.provider.0).cloned() else {
            return registry
                .resolver()
                .resolve(model)
                .map_err(anyhow::Error::from);
        };
        if let Some(base_url) = &config.api_base {
            provider.base_url = Some(base_url.clone());
        }
        if let Some(env_name) = &config.api_key_env {
            provider.api_key_env_vars = vec![env_name.clone()];
        }
        registry.register(provider);
    }
    registry
        .resolver()
        .resolve(model)
        .map_err(anyhow::Error::from)
}

fn message_text(message: &AgentMessage) -> String {
    let content = match message {
        AgentMessage::System { content }
        | AgentMessage::User { content }
        | AgentMessage::Assistant { content, .. }
        | AgentMessage::ToolResult { content, .. } => content,
    };

    content
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use neo_agent_core::{
        AgentConfig, AgentEvent, AgentMessage, ApprovalRequest, CompactionSettings, Content,
        PermissionDecision, PermissionOperation, PermissionPolicy, QueueMode,
        StopReason as AgentStopReason, ToolExecutionMode,
        session::{JsonlSessionReader, JsonlSessionWriter},
    };
    use neo_ai::{
        AiStreamEvent, ApiKind, ChatMessage, ContentPart, ModelCapabilities, ModelSpec, ProviderId,
        StopReason, providers::fake::FakeModelClient,
    };

    use super::{PromptApprovalRequest, agent_config_for_app, run_prompt_with_runtime};
    use crate::config::{AppConfig, Defaults, McpConfig, RuntimeCompactionConfig, RuntimeConfig};

    #[test]
    fn agent_config_for_app_applies_runtime_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_base: None,
            api_key_env: None,
            providers: BTreeMap::new(),
            model_catalogs: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "print".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: Some(0.35),
                max_tokens: Some(512),
                reasoning_effort: Some(neo_ai::ReasoningEffort::High),
                steering_queue_mode: QueueMode::OneAtATime,
                follow_up_queue_mode: QueueMode::OneAtATime,
                tool_execution_mode: ToolExecutionMode::Sequential,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 16_000,
                    keep_recent_messages: 24,
                }),
            },
            mcp: McpConfig::default(),
            approve: false,
            no_approve: false,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat(),
        };

        let agent_config = agent_config_for_app(model, &config, None).expect("agent config");

        assert_eq!(agent_config.temperature, Some(0.35));
        assert_eq!(agent_config.max_tokens, Some(512));
        assert_eq!(
            agent_config.reasoning_effort,
            Some(neo_ai::ReasoningEffort::High)
        );
        assert_eq!(agent_config.steering_queue_mode, QueueMode::OneAtATime);
        assert_eq!(agent_config.follow_up_queue_mode, QueueMode::OneAtATime);
        assert_eq!(
            agent_config.tool_execution_mode,
            ToolExecutionMode::Sequential
        );
        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 16_000,
                keep_recent_messages: 24,
            })
        );
        assert!(agent_config.workspace_root.is_some());
    }

    #[tokio::test]
    async fn agent_config_for_app_async_approval_channel_waits_for_ui_decision() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_base: None,
            api_key_env: None,
            providers: BTreeMap::new(),
            model_catalogs: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            mcp: McpConfig::default(),
            approve: false,
            no_approve: false,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat(),
        };
        let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
        let agent_config =
            agent_config_for_app(model, &config, Some(approval_tx)).expect("agent config");
        let handler = agent_config
            .async_approval_handler
            .expect("async approval handler");

        let decision = tokio::spawn(handler(ApprovalRequest {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: PermissionOperation::Tool,
            subject: "write".to_owned(),
            arguments: serde_json::json!({"path": "approved.txt"}),
        }));
        let PromptApprovalRequest { id, decision_tx } =
            approval_rx.recv().await.expect("approval waiter");

        assert_eq!(id, "tool-1");
        decision_tx
            .send(PermissionDecision::Allow)
            .expect("send decision");
        assert_eq!(
            decision.await.expect("approval task joins"),
            PermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn run_prompt_with_runtime_appends_continuation_to_existing_session_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_path = temp.path().join("alpha.jsonl");
        let mut seed = JsonlSessionWriter::create(&session_path)
            .await
            .expect("create session");
        seed.append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("hello"),
        })
        .await
        .expect("append user");
        seed.append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("hi back")],
                Vec::new(),
                AgentStopReason::EndTurn,
            ),
        })
        .await
        .expect("append assistant");
        seed.append_event(&AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: AgentStopReason::EndTurn,
        })
        .await
        .expect("append turn finish");
        seed.flush().await.expect("flush seed");

        let context = JsonlSessionReader::replay_context(&session_path)
            .await
            .expect("replay context");
        let fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "continued answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let runtime =
            super::AgentRuntime::new(AgentConfig::for_model(fake_model()), Arc::new(fake.clone()));
        let mut writer = JsonlSessionWriter::open_append(&session_path)
            .await
            .expect("append session");

        let turn = run_prompt_with_runtime("continue".to_owned(), context, &mut writer, runtime)
            .await
            .expect("run continuation");

        assert_eq!(turn.assistant_text, "continued answer");
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let contents = requests[0]
            .messages
            .iter()
            .map(chat_message_text)
            .collect::<Vec<_>>();
        assert_eq!(contents, vec!["hello", "hi back", "continue"]);

        let messages = JsonlSessionReader::replay_messages(&session_path)
            .await
            .expect("replay appended messages");
        assert_eq!(messages.len(), 4);
        assert!(matches!(
            &messages[2],
            AgentMessage::User { content } if content[0].as_text() == Some("continue")
        ));
        assert!(matches!(
            &messages[3],
            AgentMessage::Assistant { content, .. }
                if content[0].as_text() == Some("continued answer")
        ));
    }

    fn fake_model() -> ModelSpec {
        ModelSpec {
            provider: ProviderId("test-provider".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::Local,
            capabilities: ModelCapabilities::tool_chat(),
        }
    }

    fn chat_message_text(message: &ChatMessage) -> String {
        let content = match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content,
        };
        content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
