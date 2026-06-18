use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter, SessionMetadataStore};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AskUserTool,
    CompactionSettings, Content, McpHttpConfig, McpHttpToolAdapter, McpStdioConfig,
    McpStdioToolAdapter, McpToolAdapter, McpToolProvider, PendingQuestion, PermissionDecision,
    ToolRegistry,
};
use neo_ai::{
    ChatMessage, ContentPart, CredentialResolver, ModelClient, ModelRegistry, ModelSpec,
    ProviderRegistry, ProviderSpec, RequestOptions, ResolvedCredential,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::RunOutput,
    config::{self, AppConfig, McpServerConfig, workspace_sessions_dir},
    extension_tools,
    modes::sessions,
    resources,
};

pub async fn execute(
    prompt: &[String],
    config: &AppConfig,
    output: RunOutput,
    continue_latest: bool,
    no_session: bool,
) -> anyhow::Result<String> {
    let turn = if no_session {
        run_prompt_ephemeral(prompt, config).await?
    } else if continue_latest {
        let session_id = latest_session_id(config)?;
        run_prompt_in_session(&session_id, prompt, config).await?
    } else {
        run_prompt(prompt, config).await?
    };
    match output {
        RunOutput::Json => stable_json_output(&turn, config),
        RunOutput::Text => Ok(format!("{}\n", turn.assistant_text)),
        RunOutput::Events => {
            let mut output = String::new();
            for event in turn.events {
                output.push_str(&serde_json::to_string(&event)?);
                output.push('\n');
            }
            Ok(output)
        }
    }
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
        neo_agent_core::StopReason::Cancelled => "cancelled",
        neo_agent_core::StopReason::Error => "error",
    }
}

fn stable_compaction_reason(reason: neo_agent_core::CompactionReason) -> &'static str {
    match reason {
        neo_agent_core::CompactionReason::Threshold => "threshold",
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

fn current_unix_timestamp() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", duration.as_secs(), duration.subsec_nanos())
}

/// List only the models explicitly configured in `config.toml`.
///
/// Unlike `list_models_with_options`, built-in seeded models are excluded so
/// the output reflects exactly what the user has configured.
pub fn list_configured_models(config: &AppConfig, json_output: bool) -> anyhow::Result<String> {
    #[derive(Debug)]
    struct Entry<'a> {
        alias: &'a str,
        provider: &'a str,
        model: &'a str,
        provider_type: &'a str,
        capabilities: &'a [String],
        max_context_tokens: Option<u32>,
        display_name: Option<&'a str>,
        is_default: bool,
    }

    if config.models.is_empty() {
        if json_output {
            return Ok(serde_json::to_string_pretty(&json!({
                "models": [],
                "default_model": config.default_model,
            }))? + "\n");
        }
        return Ok("no models configured\n".to_owned());
    }

    let mut entries = Vec::with_capacity(config.models.len());
    for (alias, model_cfg) in &config.models {
        let provider_cfg = config.providers.get(&model_cfg.provider);
        let provider_type = provider_cfg
            .and_then(|cfg| cfg.provider_type)
            .map_or("unknown", |t| t.as_config_str());
        entries.push(Entry {
            alias,
            provider: &model_cfg.provider,
            model: &model_cfg.model,
            provider_type,
            capabilities: &model_cfg.capabilities,
            max_context_tokens: model_cfg.max_context_tokens,
            display_name: model_cfg.display_name.as_deref(),
            is_default: *alias == config.default_model
                || format!("{}/{}", model_cfg.provider, model_cfg.model) == config.default_model
                || (model_cfg.provider == config.default_provider
                    && model_cfg.model == config.default_model),
        });
    }

    if json_output {
        let models_json: Vec<_> = entries
            .iter()
            .map(|e| {
                json!({
                    "alias": e.alias,
                    "provider": e.provider,
                    "model": e.model,
                    "type": e.provider_type,
                    "capabilities": e.capabilities,
                    "max_context_tokens": e.max_context_tokens,
                    "display_name": e.display_name,
                    "default": e.is_default,
                })
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&json!({
            "models": models_json,
            "default_model": config.default_model,
        }))? + "\n");
    }

    let mut out = "models:\n".to_owned();
    for e in &entries {
        let default_marker = if e.is_default { " default" } else { "" };
        let display = e
            .display_name
            .map(|d| format!(" - {d}"))
            .unwrap_or_default();
        let caps = e.capabilities.join(",");
        let ctx = e
            .max_context_tokens
            .map_or("?".to_owned(), |n| n.to_string());
        let alias_label = if e.alias.contains('/') {
            e.alias.to_owned()
        } else {
            format!("{} -> {}/{}", e.alias, e.provider, e.model)
        };
        let _ = writeln!(
            out,
            "- {alias_label} ({ptype}{default_marker}) ctx={ctx} [{caps}]{display}",
            alias_label = alias_label,
            ptype = e.provider_type,
        );
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

fn provider_with_invocation_overrides(
    config: &AppConfig,
    provider_id: &str,
) -> Option<ProviderSpec> {
    let registry = provider_registry_for_config(config);
    let mut provider = registry.get(provider_id).cloned()?;
    if let Some(env_name) = &config.api_key_env {
        provider.api_key_env_vars = vec![env_name.clone()];
    }
    Some(provider)
}

fn resolve_provider_credential(provider: &ProviderSpec) -> Option<ResolvedCredential> {
    resolve_provider_credential_from_env(provider, &env::vars().collect())
}

fn resolve_provider_credential_from_env(
    provider: &ProviderSpec,
    env: &BTreeMap<String, String>,
) -> Option<ResolvedCredential> {
    CredentialResolver::new(&provider.id)
        .with_env(provider.api_key_env_vars.iter().map(String::as_str), env)
        .with_auth_file_credentials(BTreeMap::new())
        .resolve()
}

fn apply_configured_provider_overrides(registry: &mut ProviderRegistry, config: &AppConfig) {
    for (provider_id, provider_config) in &config.providers {
        let existing = registry.get(provider_id).cloned();
        let provider = if let Some(mut p) = existing {
            // Override existing built-in provider fields
            if let Some(t) = &provider_config.provider_type {
                p.provider_type = Some(*t);
            }
            if let Some(base_url) = &provider_config.base_url {
                p.base_url = Some(base_url.clone());
            }
            if let Some(key) = &provider_config.api_key {
                p.api_key = Some(key.clone());
            }
            if let Some(env_name) = &provider_config.api_key_env {
                p.api_key_env_vars = vec![env_name.clone()];
            }
            p
        } else {
            let provider_type = provider_config.provider_type;
            let Some(provider_type) = provider_type else {
                tracing::warn!("ignoring provider {provider_id}: missing required `type`");
                continue;
            };
            let default_api = provider_type.to_api_kind();
            ProviderSpec {
                id: provider_id.clone(),
                display_name: provider_id.clone(),
                api: default_api,
                supported_apis: vec![default_api],
                base_url: provider_config.base_url.clone(),
                api_key: provider_config.api_key.clone(),
                api_key_env_vars: provider_config.api_key_env.iter().cloned().collect(),
                ambient_auth_env_vars: vec![],
                provider_type: Some(provider_type),
            }
        };
        registry.register(provider);
    }
}

fn parse_mcp_kind(type_arg: &str) -> anyhow::Result<&'static str> {
    match type_arg {
        "studio" => Ok("stdio"),
        "remote-http" => Ok("http"),
        "remote-sse" => Ok("sse"),
        _ => anyhow::bail!(
            "unknown MCP type '{type_arg}'; expected studio, remote-http, or remote-sse"
        ),
    }
}

fn display_mcp_kind(transport: &str) -> &str {
    match transport {
        "stdio" => "studio",
        "http" => "remote-http",
        "sse" => "remote-sse",
        _ => transport,
    }
}

fn parse_command_string(cmd: &str) -> anyhow::Result<(String, Vec<String>)> {
    let parts =
        shell_words::split(cmd).with_context(|| format!("invalid command string: {cmd}"))?;
    let (command, args) = parts.split_first().context("command string is empty")?;
    Ok((command.clone(), args.to_vec()))
}

pub async fn list_mcp(config: &AppConfig) -> String {
    if config.mcp.servers.is_empty() {
        return "no MCP servers configured\n".to_owned();
    }

    let mut out = String::new();
    for (idx, server) in config.mcp.servers.iter().enumerate() {
        let kind = display_mcp_kind(&server.transport);
        let _ = writeln!(out, "[{}]<{}>({})", idx + 1, server.id, kind);

        if !server.enabled {
            let _ = writeln!(out, "{{}}");
            continue;
        }

        match list_mcp_tools_for_server(server).await {
            Ok(tools) => {
                let map: serde_json::Map<String, serde_json::Value> = tools
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| ((i + 1).to_string(), serde_json::Value::String(name)))
                    .collect();
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_owned())
                );
            }
            Err(_) => {
                let _ = writeln!(out, "{{}}");
            }
        }
    }
    out
}

async fn list_mcp_tools_for_server(server: &McpServerConfig) -> anyhow::Result<Vec<String>> {
    let adapter = mcp_adapter_for_server(server)?;
    let provider = McpToolProvider::discover_dyn(&server.id, adapter)
        .await
        .with_context(|| format!("failed to discover MCP tools from {}", server.id))?;
    let mut tools = provider.tool_names();
    apply_tool_filter(&mut tools, &server.enabled_tools, &server.disabled_tools);
    Ok(tools)
}

#[allow(clippy::too_many_arguments)]
pub async fn add_mcp_server(
    mcp_name: String,
    r#type: String,
    command: Option<String>,
    url: Option<String>,
    env: Vec<String>,
    headers: Vec<String>,
    cwd: Option<PathBuf>,
    enabled_tools: Vec<String>,
    disabled_tools: Vec<String>,
    startup_timeout_ms: Option<u64>,
    tool_timeout_ms: Option<u64>,
    enabled: bool,
    _config: &AppConfig,
) -> anyhow::Result<String> {
    let transport = parse_mcp_kind(&r#type)?;

    let (command, args) = if transport == "stdio" {
        let Some(cmd) = command else {
            anyhow::bail!("studio MCP requires --command");
        };
        let (cmd, args) = parse_command_string(&cmd)?;
        (Some(cmd), args)
    } else {
        if command.is_some() {
            anyhow::bail!("remote MCP uses --url, not --command");
        }
        (None, Vec::new())
    };

    let url = if transport == "http" || transport == "sse" {
        let Some(url) = url else {
            anyhow::bail!("remote MCP requires --url");
        };
        Some(url)
    } else {
        if url.is_some() {
            anyhow::bail!("studio MCP uses --command, not --url");
        }
        None
    };

    if transport != "http" && transport != "sse" && !headers.is_empty() {
        anyhow::bail!("--header is only valid for remote-http / remote-sse");
    }
    if transport != "stdio" && cwd.is_some() {
        anyhow::bail!("--cwd is only valid for studio");
    }

    let server = McpServerConfig {
        id: mcp_name.clone(),
        enabled,
        transport: transport.to_owned(),
        command,
        url,
        args,
        env: key_value_pairs(env, "--env")?,
        headers: key_value_pairs(headers, "--header")?,
        cwd,
        enabled_tools,
        disabled_tools,
        startup_timeout_ms,
        tool_timeout_ms,
    };

    let saved = config::upsert_mcp_server(&server)?;

    if !enabled {
        return Ok(format!("{saved}{mcp_name} added (disabled)\n"));
    }

    let probe_result = probe_mcp_server(&server, startup_timeout_ms).await;
    let probe_msg = match probe_result {
        Ok(()) => format!("{mcp_name} successfully connected!\n"),
        Err(_) => format!("{mcp_name} connect failed\n"),
    };
    Ok(format!("{saved}{probe_msg}"))
}

async fn probe_mcp_server(server: &McpServerConfig, timeout_ms: Option<u64>) -> anyhow::Result<()> {
    let adapter = mcp_adapter_for_server(server)?;
    let fut = adapter.list_tools();
    let tools = if let Some(ms) = timeout_ms {
        tokio::time::timeout(std::time::Duration::from_millis(ms), fut)
            .await
            .with_context(|| format!("timeout connecting to MCP server {}", server.id))??
    } else {
        fut.await
            .with_context(|| format!("failed to list tools from {}", server.id))?
    };
    let mut names: Vec<String> = tools.into_iter().map(|t| t.name).collect();
    apply_tool_filter(&mut names, &server.enabled_tools, &server.disabled_tools);
    Ok(())
}

fn apply_tool_filter(tools: &mut Vec<String>, enabled_tools: &[String], disabled_tools: &[String]) {
    if !enabled_tools.is_empty() {
        let allow: std::collections::HashSet<_> = enabled_tools.iter().cloned().collect();
        tools.retain(|name| allow.contains(name));
    }
    if !disabled_tools.is_empty() {
        let deny: std::collections::HashSet<_> = disabled_tools.iter().cloned().collect();
        tools.retain(|name| !deny.contains(name));
    }
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
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(prompt.clone(), &mut writer).await?;
    record_session_activity(config, &session_id, &prompt);
    let runtime = runtime_for_config(config, None, None).await?;
    let turn = finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        session_id,
    )
    .await?;
    record_initial_session_title(config, &turn, &prompt).await;
    Ok(turn)
}

pub async fn run_prompt_ephemeral(
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let mut writer = SessionEventWriter::memory();
    let (user_message, events) = append_user_event(prompt.clone(), &mut writer).await?;
    let runtime = runtime_for_config(config, None, None).await?;
    finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        "ephemeral".to_owned(),
    )
    .await
}

pub async fn run_prompt_in_session(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = sessions::session_path(session_id, config)?;
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(prompt.clone(), &mut writer).await?;
    record_session_activity(config, session_id, &prompt);
    let runtime = runtime_for_config(config, None, None).await?;
    runtime.restore_plan_mode(&context);
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
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    if let Some(session_id_tx) = session_id_tx {
        let _ = session_id_tx.send(session_id.clone());
    }
    let (user_message, events) = append_user_event_jsonl(prompt.clone(), &mut writer).await?;
    record_session_activity(config, &session_id, &prompt);
    let runtime = runtime_for_config(config, Some(approval_tx), question_tx).await?;
    let streaming = StreamingTurnIo {
        event_tx,
        session_id,
        cancel_token,
    };
    let turn = finish_prompt_turn_streaming(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        streaming,
    )
    .await?;
    record_initial_session_title(config, &turn, &prompt).await;
    Ok(turn)
}

pub async fn run_prompt_in_session_streaming(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = sessions::session_path(session_id, config)?;
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    if let Some(session_id_tx) = session_id_tx {
        let _ = session_id_tx.send(session_id.to_owned());
    }
    let (user_message, events) = append_user_event_jsonl(prompt.clone(), &mut writer).await?;
    record_session_activity(config, session_id, &prompt);
    let runtime = runtime_for_config(config, Some(approval_tx), question_tx).await?;
    runtime.restore_plan_mode(&context);
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
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
) -> anyhow::Result<AgentRuntime> {
    let model = resolve_model(config)?;
    let client = resolve_model_client(config, &model)?;
    let mut tools = tool_registry_for_config(config).await?;
    if let Some(question_tx) = question_tx {
        tools.register(AskUserTool::new(question_tx));
    }
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
    let mut writer = SessionEventWriter::jsonl(writer);
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        "test-session".to_owned(),
    )
    .await
}

async fn append_user_event(
    prompt: String,
    writer: &mut SessionEventWriter<'_>,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let user_message = AgentMessage::user_text(prompt);
    let user_event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    writer.append_event(&user_event).await?;
    writer.flush().await?;
    Ok((user_message, vec![user_event]))
}

async fn append_user_event_jsonl(
    prompt: String,
    writer: &mut JsonlSessionWriter,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let mut writer = SessionEventWriter::jsonl(writer);
    append_user_event(prompt, &mut writer).await
}

async fn finish_prompt_turn(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut SessionEventWriter<'_>,
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

enum SessionEventWriter<'a> {
    Jsonl(&'a mut JsonlSessionWriter),
    Memory,
}

impl<'a> SessionEventWriter<'a> {
    fn jsonl(writer: &'a mut JsonlSessionWriter) -> Self {
        Self::Jsonl(writer)
    }

    fn memory() -> Self {
        Self::Memory
    }

    async fn append_event(&mut self, event: &AgentEvent) -> anyhow::Result<()> {
        match self {
            Self::Jsonl(writer) => writer
                .append_event(event)
                .await
                .map_err(anyhow::Error::from),
            Self::Memory => Ok(()),
        }
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        match self {
            Self::Jsonl(writer) => writer.flush().await.map_err(anyhow::Error::from),
            Self::Memory => Ok(()),
        }
    }
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
    agent_config.replay_reasoning = config.runtime.replay_reasoning;
    if let Some(system_prompt) =
        resources::load_system_prompt(&config.project_dir, config.project_trusted)?
    {
        agent_config = agent_config.with_system_prompt(system_prompt);
    }
    if let Some(compaction) = &config.runtime.compaction {
        let model_max_context_tokens = agent_config.model.capabilities.max_context_tokens;
        agent_config = agent_config.with_compaction(CompactionSettings {
            enabled: compaction.enabled,
            max_estimated_tokens: effective_compaction_max_estimated_tokens(
                compaction.max_estimated_tokens,
                model_max_context_tokens,
            ),
            keep_recent_messages: compaction.keep_recent_messages,
        });
    }
    if let Some(approval_tx) = approval_tx {
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

const LEGACY_DEFAULT_COMPACTION_MAX_ESTIMATED_TOKENS: usize = 32_000;
const MODEL_CONTEXT_COMPACTION_NUMERATOR: usize = 4;
const MODEL_CONTEXT_COMPACTION_DENOMINATOR: usize = 5;

fn effective_compaction_max_estimated_tokens(
    configured_max_estimated_tokens: usize,
    model_max_context_tokens: Option<u32>,
) -> usize {
    if configured_max_estimated_tokens != LEGACY_DEFAULT_COMPACTION_MAX_ESTIMATED_TOKENS {
        return configured_max_estimated_tokens;
    }
    let Some(model_max_context_tokens) = model_max_context_tokens else {
        return configured_max_estimated_tokens;
    };
    let model_threshold = model_max_context_tokens as usize * MODEL_CONTEXT_COMPACTION_NUMERATOR
        / MODEL_CONTEXT_COMPACTION_DENOMINATOR;
    configured_max_estimated_tokens.max(model_threshold)
}

async fn tool_registry_for_config(config: &AppConfig) -> anyhow::Result<ToolRegistry> {
    let mut registry = ToolRegistry::with_builtin_tools();
    extension_tools::register_enabled_extension_tools(
        &mut registry,
        &extension_tools::default_extension_root(&config.project_dir),
        &extension_tools::default_extension_state_path(&config.project_dir),
    )
    .await?;
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
        .with_context(|| format!("failed to discover MCP tools from {}", server.id))?
        .with_tool_filter(&server.enabled_tools, &server.disabled_tools);
    provider.register_into(registry);
    Ok(())
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
                env: server.env.clone(),
                cwd: server.cwd.clone(),
                tool_timeout_ms: server.tool_timeout_ms,
            })))
        }
        "http" | "sse" => {
            let url = server
                .url
                .clone()
                .with_context(|| format!("missing MCP url for {}", server.id))?;
            Ok(Arc::new(McpHttpToolAdapter::new(McpHttpConfig {
                url,
                headers: server.headers.clone(),
                tool_timeout_ms: server.tool_timeout_ms,
            })))
        }
        other => anyhow::bail!("unsupported MCP transport for {}: {other}", server.id),
    }
}

fn key_value_pairs(values: Vec<String>, flag: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let mut pairs = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            anyhow::bail!("{flag} values must use KEY=VALUE");
        };
        let key = key.trim();
        anyhow::ensure!(!key.is_empty(), "{flag} key must not be empty");
        pairs.insert(key.to_owned(), value.trim().to_owned());
    }
    Ok(pairs)
}

async fn create_session_path(config: &AppConfig) -> anyhow::Result<std::path::PathBuf> {
    let bucket_dir = workspace_sessions_dir(config);
    tokio::fs::create_dir_all(&bucket_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create sessions directory {}",
                bucket_dir.display()
            )
        })?;

    let mut counter = 0_u32;
    loop {
        let suffix = if counter == 0 {
            String::new()
        } else {
            format!("-{counter}")
        };
        let path = bucket_dir.join(format!(
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

pub(crate) fn latest_session_id(config: &AppConfig) -> anyhow::Result<String> {
    let bucket_dir = workspace_sessions_dir(config);
    let mut latest: Option<(std::time::SystemTime, String)> = None;
    let entries = std::fs::read_dir(&bucket_dir)
        .with_context(|| format!("failed to read sessions directory {}", bucket_dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("jsonl") {
            continue;
        }
        let Ok(session_id) = session_id_from_path(&path) else {
            continue;
        };
        if neo_agent_core::session::validate_session_id(&session_id).is_err() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let should_replace = latest.as_ref().is_none_or(|(latest_modified, latest_id)| {
            modified > *latest_modified || (modified == *latest_modified && session_id > *latest_id)
        });
        if should_replace {
            latest = Some((modified, session_id));
        }
    }

    latest
        .map(|(_, session_id)| session_id)
        .with_context(|| format!("no sessions found in {}", bucket_dir.display()))
}

fn resolve_model(config: &AppConfig) -> anyhow::Result<ModelSpec> {
    let registry = model_registry_for_config(config)?;
    select_config_model(&registry, config)
}

fn record_session_activity(config: &AppConfig, session_id: &str, prompt: &str) {
    let bucket_dir = workspace_sessions_dir(config);
    let _ = SessionMetadataStore::new(&bucket_dir).record_activity(
        session_id,
        Some(config.project_dir.display().to_string()),
        Some(one_line(prompt, 240)),
        current_unix_timestamp(),
    );
}

async fn record_initial_session_title(config: &AppConfig, turn: &PromptTurn, prompt: &str) {
    let bucket_dir = workspace_sessions_dir(config);
    let store = SessionMetadataStore::new(&bucket_dir);
    let Ok(sessions) = store.list() else {
        return;
    };
    let Some(record) = sessions
        .into_iter()
        .find(|session| session.id == turn.session_id)
    else {
        return;
    };
    if record.name.is_some() || record.title_model.is_some() {
        return;
    }

    let fallback = one_line(prompt, 40);
    let (title, model_label) =
        match generate_session_title(config, prompt, &turn.assistant_text).await {
            Ok((title, model_label)) if !title.is_empty() => (title, Some(model_label)),
            _ => (fallback, None),
        };
    let _ = store.record_title(
        &turn.session_id,
        title,
        model_label,
        current_unix_timestamp(),
    );
}

async fn generate_session_title(
    config: &AppConfig,
    prompt: &str,
    assistant_text: &str,
) -> anyhow::Result<(String, String)> {
    let model = resolve_model(config)?;
    let client = resolve_model_client(config, &model)?;
    let model_label = format!("{}/{}", model.provider.0, model.model);
    let request = neo_ai::ChatRequest {
        model,
        messages: vec![
            ChatMessage::System {
                content: vec![ContentPart::Text {
                    text: "Generate a concise session title. Return only the title, no quotes."
                        .to_owned(),
                }],
            },
            ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: format!(
                        "User prompt:\n{}\n\nAssistant response:\n{}",
                        one_line(prompt, 500),
                        one_line(assistant_text, 500)
                    ),
                }],
            },
        ],
        tools: Vec::new(),
        options: RequestOptions {
            max_tokens: Some(32),
            temperature: Some(0.2),
            ..RequestOptions::default()
        },
    };
    let events = client.stream_chat(request).collect::<Vec<_>>().await;
    let mut title = String::new();
    for event in events {
        if let neo_ai::AiStreamEvent::TextDelta { text } = event? {
            title.push_str(&text);
        }
    }
    Ok((clean_session_title(&title), model_label))
}

fn clean_session_title(title: &str) -> String {
    one_line(title.trim().trim_matches(['"', '\'', '`']), 40)
        .trim_matches(['*', '#'])
        .trim()
        .to_owned()
}

fn one_line(text: &str, max_chars: usize) -> String {
    let mut line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > max_chars {
        line = line.chars().take(max_chars.saturating_sub(1)).collect();
        line.push('…');
    }
    line
}

pub(crate) fn model_registry_for_config(config: &AppConfig) -> anyhow::Result<ModelRegistry> {
    let mut registry = ModelRegistry::seeded();

    for (alias, model_cfg) in &config.models {
        let spec = model_config_to_spec(alias, model_cfg, &config.providers)?;
        registry.register(spec);
    }

    Ok(registry)
}

pub(crate) fn select_config_model(
    registry: &ModelRegistry,
    config: &AppConfig,
) -> anyhow::Result<ModelSpec> {
    let models = registry.list();
    let candidates = config::scoped_models(models.iter(), &config.model_scope);
    if !config.model_scope.is_empty() && candidates.is_empty() {
        anyhow::bail!(
            "no models match model_scope {}; run `neo models list` for supported catalog entries",
            config.model_scope.join(",")
        );
    }
    let default = models.iter().find(|model| {
        model.provider.0 == config.default_provider && model.model == config.default_model
    });
    if config.model_scope.is_empty() {
        return default.cloned().with_context(|| {
            format!(
                "unknown model {}/{}; run `neo models list` for supported catalog entries",
                config.default_provider, config.default_model
            )
        });
    }

    candidates
        .iter()
        .find(|model| {
            model.provider.0 == config.default_provider && model.model == config.default_model
        })
        .or_else(|| candidates.first())
        .cloned()
        .with_context(|| {
            format!(
                "unknown model {}/{}; run `neo models list` for supported catalog entries",
                config.default_provider, config.default_model
            )
        })
}

/// Convert a `[models.<alias>]` config entry into a `ModelSpec`.
fn model_config_to_spec(
    alias: &str,
    cfg: &crate::config::ModelConfig,
    providers: &BTreeMap<String, crate::config::ProviderConfig>,
) -> anyhow::Result<ModelSpec> {
    let provider_cfg = providers.get(&cfg.provider).ok_or_else(|| {
        anyhow::anyhow!(
            "model '{}' references unknown provider '{}'; define it in config.toml with [providers.{}]",
            alias,
            cfg.provider,
            cfg.provider
        )
    })?;

    let api = provider_cfg
        .provider_type
        .with_context(|| format!("provider '{}' must declare `type`", cfg.provider))?
        .to_api_kind();

    // Parse capabilities from string list
    let capabilities = parse_model_capabilities(&cfg.capabilities, cfg.max_context_tokens);

    Ok(ModelSpec {
        provider: neo_ai::ProviderId(cfg.provider.clone()),
        model: cfg.model.clone(),
        api,
        capabilities,
    })
}

/// Parse a capability string list into `ModelCapabilities`.
fn parse_model_capabilities(
    caps: &[String],
    max_context_tokens: Option<u32>,
) -> neo_ai::ModelCapabilities {
    let mut mc = neo_ai::ModelCapabilities::tool_chat();
    mc.streaming = false;
    mc.tools = false;
    mc.images = false;
    mc.reasoning = false;
    mc.embeddings = false;
    for cap in caps {
        match cap.trim().to_ascii_lowercase().as_str() {
            "streaming" => mc.streaming = true,
            "tools" | "tool_use" => mc.tools = true,
            "images" | "image_in" | "vision" => mc.images = true,
            "reasoning" | "thinking" => mc.reasoning = true,
            "embeddings" | "embedding" => mc.embeddings = true,
            _ => {}
        }
    }
    mc.max_context_tokens = max_context_tokens;
    mc
}

fn resolve_model_client(
    config: &AppConfig,
    model: &ModelSpec,
) -> anyhow::Result<Arc<dyn ModelClient>> {
    const RESOLVED_API_KEY_ENV: &str = "__NEO_RESOLVED_API_KEY";
    let mut registry = ProviderRegistry::production();
    apply_configured_provider_overrides(&mut registry, config);
    if let Some(mut provider) = provider_with_invocation_overrides(config, &model.provider.0) {
        let credential = resolve_provider_credential(&provider);
        let mut env = env::vars().collect::<BTreeMap<_, _>>();
        if let Some(credential) = credential {
            provider.api_key_env_vars = vec![RESOLVED_API_KEY_ENV.to_owned()];
            env.insert(
                RESOLVED_API_KEY_ENV.to_owned(),
                credential.secret().to_owned(),
            );
        }
        registry.register(provider);
        return registry
            .resolver_from(env)
            .resolve(model)
            .map_err(anyhow::Error::from);
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

    use super::{
        PromptApprovalRequest, StableJsonState, agent_config_for_app, run_prompt_with_runtime,
    };
    use crate::config::{
        AppConfig, Defaults, McpConfig, RuntimeCompactionConfig, RuntimeConfig, TuiConfig,
    };

    #[test]
    fn agent_config_for_app_applies_runtime_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: Some(0.35),
                max_tokens: Some(512),
                reasoning_effort: Some(neo_ai::ReasoningEffort::High),
                replay_reasoning: true,
                steering_queue_mode: QueueMode::OneAtATime,
                follow_up_queue_mode: QueueMode::OneAtATime,
                tool_execution_mode: ToolExecutionMode::Sequential,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 16_000,
                    keep_recent_messages: 24,
                }),
            },
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            project_trusted: true,
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
                    first_kept_message_index: 4,
                },
            }),
            vec![serde_json::json!({
                "type": "compaction_end",
                "reason": "threshold",
                "result": {
                    "summary": "Older context summarized.",
                    "tokens_before": 12_345,
                    "first_kept_message_index": 4,
                },
                "aborted": false,
                "willRetry": false,
            })]
        );
    }

    #[test]
    fn agent_config_for_app_scales_default_compaction_to_model_context_window() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "large-context-model".to_owned(),
            default_provider: "anthropic".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::All,
                follow_up_queue_mode: QueueMode::All,
                tool_execution_mode: ToolExecutionMode::Parallel,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 32_000,
                    keep_recent_messages: 20,
                }),
            },
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            project_trusted: true,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let agent_config = agent_config_for_app(model, &config, None).expect("agent config");

        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 800_000,
                keep_recent_messages: 20,
            })
        );
    }

    #[test]
    fn agent_config_for_app_keeps_explicit_custom_compaction_threshold() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "large-context-model".to_owned(),
            default_provider: "anthropic".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::All,
                follow_up_queue_mode: QueueMode::All,
                tool_execution_mode: ToolExecutionMode::Parallel,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 12_000,
                    keep_recent_messages: 16,
                }),
            },
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            project_trusted: true,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let agent_config = agent_config_for_app(model, &config, None).expect("agent config");

        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 12_000,
                keep_recent_messages: 16,
            })
        );
    }

    #[tokio::test]
    async fn agent_config_for_app_async_approval_channel_waits_for_ui_decision() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            project_trusted: true,
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
                ContentPart::Thinking { .. } | ContentPart::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
