use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::goal::GoalManager;
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter, SessionMetadataStore};
use neo_agent_core::skills::SkillStore;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AskUserTool,
    CompactionSettings, Content, CreateSkillTool, ListSkillsTool, McpClient, McpConnectionManager,
    McpServerStatus, MoveSkillTool, PendingQuestion, PermissionApprovalDecision,
    PermissionOperation, ProcessSupervisor, StdioConfig, SteerInputHandle, SummarizeSessionsTool,
    ToolRegistry, build_http_client_with_oauth, build_stdio_client, mode::PlanMode,
};
use neo_ai::{
    ChatMessage, ContentPart, CredentialResolver, ModelClient, ModelRegistry, ModelSpec,
    ProviderRegistry, ProviderSpec, RequestOptions, ResolvedCredential,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::mcp_ops::{
    authenticate_mcp_server_oauth, display_mcp_kind, parse_command_string, parse_mcp_kind,
};
use crate::{
    cli::RunOutput,
    config::{
        self, AppConfig, McpServerConfig, McpTransport, ModelConfig, neo_home,
        workspace_sessions_dir,
    },
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
    if config.models.is_empty() {
        return list_empty_configured_models(config, json_output);
    }

    let entries = configured_model_entries(config);
    if json_output {
        return configured_models_json(config, &entries);
    }
    Ok(configured_models_text(&entries))
}

#[derive(Debug)]
struct ConfiguredModelEntry<'a> {
    alias: &'a str,
    provider: &'a str,
    model: &'a str,
    provider_type: &'a str,
    capabilities: &'a [String],
    max_context_tokens: Option<u32>,
    display_name: Option<&'a str>,
    is_default: bool,
}

fn list_empty_configured_models(config: &AppConfig, json_output: bool) -> anyhow::Result<String> {
    if json_output {
        return Ok(serde_json::to_string_pretty(&json!({
            "models": [],
            "default_model": config.default_model,
        }))? + "\n");
    }
    Ok("no models configured\n".to_owned())
}

fn configured_model_entries(config: &AppConfig) -> Vec<ConfiguredModelEntry<'_>> {
    config
        .models
        .iter()
        .map(|(alias, model_cfg)| configured_model_entry(alias, model_cfg, config))
        .collect()
}

fn configured_model_entry<'a>(
    alias: &'a str,
    model_cfg: &'a ModelConfig,
    config: &'a AppConfig,
) -> ConfiguredModelEntry<'a> {
    let provider_type = config
        .providers
        .get(&model_cfg.provider)
        .and_then(|cfg| cfg.provider_type)
        .map_or("unknown", |t| t.as_config_str());
    ConfiguredModelEntry {
        alias,
        provider: &model_cfg.provider,
        model: &model_cfg.model,
        provider_type,
        capabilities: &model_cfg.capabilities,
        max_context_tokens: model_cfg.max_context_tokens,
        display_name: model_cfg.display_name.as_deref(),
        is_default: configured_model_is_default(alias, model_cfg, config),
    }
}

fn configured_model_is_default(alias: &str, model_cfg: &ModelConfig, config: &AppConfig) -> bool {
    model_config_matches_default(alias, model_cfg, config)
}

fn configured_models_json(
    config: &AppConfig,
    entries: &[ConfiguredModelEntry<'_>],
) -> anyhow::Result<String> {
    let models_json: Vec<_> = entries.iter().map(configured_model_json).collect();
    Ok(serde_json::to_string_pretty(&json!({
        "models": models_json,
        "default_model": config.default_model,
    }))? + "\n")
}

fn configured_model_json(entry: &ConfiguredModelEntry<'_>) -> Value {
    json!({
        "alias": entry.alias,
        "provider": entry.provider,
        "model": entry.model,
        "type": entry.provider_type,
        "capabilities": entry.capabilities,
        "max_context_tokens": entry.max_context_tokens,
        "display_name": entry.display_name,
        "default": entry.is_default,
    })
}

fn configured_models_text(entries: &[ConfiguredModelEntry<'_>]) -> String {
    let mut out = "models:\n".to_owned();
    for entry in entries {
        out.push_str(&configured_model_text(entry));
    }
    out
}

fn configured_model_text(entry: &ConfiguredModelEntry<'_>) -> String {
    let default_marker = if entry.is_default { " default" } else { "" };
    let display = entry
        .display_name
        .map(|display_name| format!(" - {display_name}"))
        .unwrap_or_default();
    let caps = entry.capabilities.join(",");
    let ctx = entry
        .max_context_tokens
        .map_or("?".to_owned(), |tokens| tokens.to_string());
    let alias_label = configured_model_alias_label(entry);
    format!(
        "- {alias_label} ({ptype}{default_marker}) ctx={ctx} [{caps}]{display}\n",
        ptype = entry.provider_type,
    )
}

fn configured_model_alias_label(entry: &ConfiguredModelEntry<'_>) -> String {
    if entry.alias.contains('/') {
        entry.alias.to_owned()
    } else {
        format!("{} -> {}/{}", entry.alias, entry.provider, entry.model)
    }
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

pub async fn list_mcp(config: &AppConfig) -> String {
    if config.mcp.servers.is_empty() {
        return "no MCP servers configured\n".to_owned();
    }

    let mut out = String::new();
    for (idx, server) in config.mcp.servers.iter().enumerate() {
        let kind = display_mcp_kind(server.transport);
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
    let supervisor = ProcessSupervisor::default();
    let client = build_mcp_client(server, &supervisor).await?;
    let tools = client
        .list_tools()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut tools: Vec<String> = tools.into_iter().map(|t| t.name).collect();
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
    config: &AppConfig,
) -> anyhow::Result<String> {
    let transport = parse_mcp_kind(&r#type)?;

    let (command, args) = if transport == McpTransport::Stdio {
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

    let url = if transport == McpTransport::Http || transport == McpTransport::Sse {
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

    if transport != McpTransport::Http && transport != McpTransport::Sse && !headers.is_empty() {
        anyhow::bail!("--header is only valid for remote-http / remote-sse");
    }
    if transport != McpTransport::Stdio && cwd.is_some() {
        anyhow::bail!("--cwd is only valid for studio");
    }

    let server = McpServerConfig {
        id: mcp_name.clone(),
        enabled,
        transport,
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

    let saved = config::mutations::upsert_mcp_server(&server, &config.config_path)?;

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

/// Run the OAuth authorization-code flow for a configured MCP server and save
/// the resulting token to `~/.neo/oauth.json`.
pub async fn auth_mcp_server(server_id: String, config: &AppConfig) -> anyhow::Result<String> {
    let server = config
        .mcp
        .servers
        .iter()
        .find(|server| server.id == server_id)
        .context("MCP server not found")?;

    if server.transport != McpTransport::Http && server.transport != McpTransport::Sse {
        anyhow::bail!("OAuth is limited to HTTP/SSE servers");
    }

    let neo_home = neo_home().context("failed to resolve neo home directory")?;
    authenticate_mcp_server_oauth(&server_id, server, &neo_home).await?;

    Ok(format!("OAuth token saved for MCP server {server_id}\n"))
}

async fn probe_mcp_server(server: &McpServerConfig, timeout_ms: Option<u64>) -> anyhow::Result<()> {
    let supervisor = ProcessSupervisor::default();
    let client = build_mcp_client(server, &supervisor).await?;
    let fut = client.list_tools();
    let tools = if let Some(ms) = timeout_ms {
        tokio::time::timeout(std::time::Duration::from_millis(ms), fut)
            .await
            .with_context(|| format!("timeout connecting to MCP server {}", server.id))?
            .map_err(|e| anyhow::anyhow!("{e}"))?
    } else {
        fut.await
            .map_err(|e| anyhow::anyhow!("{e}"))
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
    pub decision_tx: oneshot::Sender<PermissionApprovalDecision>,
    pub feedback_tx: Option<oneshot::Sender<Option<String>>>,
    /// Returns the model-supplied plan-review option label the user picked,
    /// when the approval was an `ExitPlanMode` plan-review approve choice.
    pub selected_label_tx: Option<oneshot::Sender<Option<String>>>,
    /// Display label for the session-approval option (Layer 1). `None` hides it.
    pub session_option_label: Option<String>,
    /// Display label for the prefix-approval option (Layer 2). `None` hides it.
    /// When the user picks the prefix option, the controller sets
    /// `prefix_rule` so the runtime persists the rule.
    pub prefix_option_label: Option<String>,
}

pub async fn run_prompt(prompt: &[String], config: &AppConfig) -> anyhow::Result<PromptTurn> {
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(&prompt_text)];
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(content, &mut writer).await?;
    record_session_activity(config, &session_id, &prompt_text);
    let runtime = runtime_for_config(
        config,
        session_path.parent().map(Path::to_path_buf),
        None,
        None,
        None,
        None,
        false,
        SteerInputHandle::new(),
        None,
        Arc::new(Mutex::new(None)),
    )
    .await?;
    let turn = finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        session_id,
    )
    .await?;
    record_initial_session_title(config, &turn, &prompt_text).await;
    Ok(turn)
}

pub async fn run_prompt_ephemeral(
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(&prompt_text)];
    let mut writer = SessionEventWriter::memory();
    let (user_message, events) = append_user_event(content, &mut writer).await?;
    let runtime = runtime_for_config(
        config,
        None,
        None,
        None,
        None,
        None,
        false,
        SteerInputHandle::new(),
        None,
        Arc::new(Mutex::new(None)),
    )
    .await?;
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
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(&prompt_text)];
    let session_path = sessions::session_path(session_id, config)?;
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(content, &mut writer).await?;
    record_session_activity(config, session_id, &prompt_text);
    let runtime = runtime_for_config(
        config,
        session_path.parent().map(Path::to_path_buf),
        None,
        None,
        None,
        None,
        false,
        SteerInputHandle::new(),
        None,
        Arc::new(Mutex::new(None)),
    )
    .await?;
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

#[allow(clippy::too_many_arguments)]
pub async fn run_prompt_streaming(
    prompt: &[Content],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
    skill_context: Option<String>,
    plan_review_feedback: Option<std::collections::BTreeMap<String, String>>,
    plan_mode: Option<Arc<RwLock<PlanMode>>>,
    goal_mode_authoring: bool,
    steer_input: SteerInputHandle,
    mcp_manager: Option<McpConnectionManager>,
    manual_compact_request: Arc<Mutex<Option<String>>>,
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let prepared = prepare_new_streaming_turn(prompt, config, session_id_tx, skill_context).await?;
    let prompt = prepared.prompt.clone();
    let runtime = runtime_for_config(
        config,
        Some(prepared.session_directory.clone()),
        Some(approval_tx),
        question_tx,
        plan_review_feedback.clone(),
        plan_mode.clone(),
        goal_mode_authoring,
        steer_input,
        mcp_manager,
        manual_compact_request,
    )
    .await?;
    let turn =
        run_prepared_streaming_turn(prepared, runtime, event_tx, cancel_token, compaction_only)
            .await?;
    record_initial_session_title(config, &turn, &prompt).await;
    Ok(turn)
}

#[allow(clippy::too_many_arguments)]
pub async fn run_prompt_in_session_streaming(
    session_id: &str,
    prompt: &[Content],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
    skill_context: Option<String>,
    plan_review_feedback: Option<std::collections::BTreeMap<String, String>>,
    plan_mode: Option<Arc<RwLock<PlanMode>>>,
    goal_mode_authoring: bool,
    steer_input: SteerInputHandle,
    mcp_manager: Option<McpConnectionManager>,
    manual_compact_request: Arc<Mutex<Option<String>>>,
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let prepared =
        prepare_existing_streaming_turn(session_id, prompt, config, session_id_tx, skill_context)
            .await?;
    let runtime = runtime_for_config(
        config,
        Some(prepared.session_directory.clone()),
        Some(approval_tx),
        question_tx,
        plan_review_feedback.clone(),
        plan_mode.clone(),
        goal_mode_authoring,
        steer_input,
        mcp_manager,
        manual_compact_request,
    )
    .await?;
    runtime.restore_plan_mode(&prepared.context);
    run_prepared_streaming_turn(prepared, runtime, event_tx, cancel_token, compaction_only).await
}

async fn prepare_new_streaming_turn(
    prompt: &[Content],
    config: &AppConfig,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    skill_context: Option<String>,
) -> anyhow::Result<PreparedStreamingTurn> {
    let prompt_text = prompt
        .iter()
        .filter_map(|c| c.as_text())
        .collect::<Vec<_>>()
        .join(" ");
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    send_streaming_session_id(session_id_tx, &session_id);
    let (user_message, initial_events) =
        append_user_event_jsonl(prompt.to_vec(), &mut writer).await?;
    record_session_activity(config, &session_id, &prompt_text);
    let session_directory = session_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workspace_sessions_dir(config).join(&session_id));
    Ok(PreparedStreamingTurn {
        prompt: prompt_text,
        session_id,
        session_directory,
        context: streaming_context(skill_context),
        writer,
        user_message,
        initial_events,
    })
}

async fn prepare_existing_streaming_turn(
    session_id: &str,
    prompt: &[Content],
    config: &AppConfig,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    skill_context: Option<String>,
) -> anyhow::Result<PreparedStreamingTurn> {
    let prompt_text = prompt
        .iter()
        .filter_map(|c| c.as_text())
        .collect::<Vec<_>>()
        .join(" ");
    let session_path = sessions::session_path(session_id, config)?;
    let session_directory = session_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| workspace_sessions_dir(config).join(session_id));
    let mut context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    apply_skill_context(&mut context, skill_context);
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    send_streaming_session_id(session_id_tx, session_id);
    let (user_message, initial_events) =
        append_user_event_jsonl(prompt.to_vec(), &mut writer).await?;
    record_session_activity(config, session_id, &prompt_text);
    Ok(PreparedStreamingTurn {
        prompt: prompt_text,
        session_id: session_id.to_owned(),
        session_directory,
        context,
        writer,
        user_message,
        initial_events,
    })
}

fn send_streaming_session_id(
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    session_id: &str,
) {
    if let Some(session_id_tx) = session_id_tx {
        let _ = session_id_tx.send(session_id.to_owned());
    }
}

fn streaming_context(skill_context: Option<String>) -> AgentContext {
    let mut context = AgentContext::new();
    apply_skill_context(&mut context, skill_context);
    context
}

fn apply_skill_context(context: &mut AgentContext, skill_context: Option<String>) {
    if let Some(skill_context) = skill_context {
        context.set_skill_context(AgentMessage::system_text(skill_context));
    }
}

async fn run_prepared_streaming_turn(
    prepared: PreparedStreamingTurn,
    runtime: AgentRuntime,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    cancel_token: CancellationToken,
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let PreparedStreamingTurn {
        session_id,
        session_directory: _,
        context,
        mut writer,
        user_message,
        initial_events,
        prompt: _,
    } = prepared;
    let streaming = StreamingTurnIo {
        event_tx,
        session_id,
        cancel_token,
    };
    if compaction_only {
        finish_compaction_turn_streaming(context, &mut writer, runtime, initial_events, streaming)
            .await
    } else {
        finish_prompt_turn_streaming(
            user_message,
            context,
            &mut writer,
            runtime,
            initial_events,
            streaming,
        )
        .await
    }
}

async fn runtime_for_config(
    config: &AppConfig,
    session_directory: Option<PathBuf>,
    approval_tx: Option<mpsc::UnboundedSender<PromptApprovalRequest>>,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
    plan_review_feedback: Option<std::collections::BTreeMap<String, String>>,
    plan_mode: Option<Arc<RwLock<PlanMode>>>,
    goal_mode_authoring: bool,
    steer_input: SteerInputHandle,
    mcp_manager: Option<McpConnectionManager>,
    manual_compact_request: Arc<Mutex<Option<String>>>,
) -> anyhow::Result<AgentRuntime> {
    let model = resolve_model(config)?;
    let client = resolve_model_client(config, &model)?;
    let skill_store = resources::load_skill_store(
        neo_home().as_deref(),
        &config.extra_skill_dirs,
        &config.skill_path,
    )?;
    let mut agent_config = agent_config_for_app(model, config, approval_tx, &skill_store)?;
    if let Some(session_directory) = &session_directory {
        agent_config = agent_config.with_session_directory(session_directory.clone());
    }
    agent_config.manual_compact_request = manual_compact_request;
    if let Some(plan_mode) = plan_mode {
        agent_config = agent_config.with_plan_mode(plan_mode);
    }
    if goal_mode_authoring {
        agent_config = agent_config.with_goal_mode_authoring(true);
    }
    if let Some(feedback) = plan_review_feedback {
        agent_config.plan_review_feedback =
            Arc::new(std::sync::Mutex::new(feedback.into_iter().collect()));
    }
    let mut tools = tool_registry_for_config(
        config,
        std::sync::Arc::clone(&agent_config.todos),
        mcp_manager.as_ref(),
    )
    .await?;
    if let Some(question_tx) = question_tx {
        tools.register(AskUserTool::new(question_tx));
    }
    let extra_skill_paths: Vec<PathBuf> =
        config.extra_skill_dirs.iter().map(PathBuf::from).collect();
    tools.register(ListSkillsTool::new(neo_home(), extra_skill_paths));
    if let Some(home) = neo_home() {
        tools.register(MoveSkillTool::new(home.clone()));
        tools.register(CreateSkillTool::new(home.clone()));
        tools.register(SummarizeSessionsTool::new(home));
    }
    let mut runtime = AgentRuntime::with_tools_and_skills(agent_config, client, tools, skill_store);
    runtime = runtime.with_steer_input(steer_input);
    if let Some(session_dir) = session_directory {
        let goal_manager = Arc::new(GoalManager::load(session_dir).await?);
        if let Some(tools) = runtime.tools_mut() {
            Arc::get_mut(tools)
                .expect("tools arc not yet shared")
                .register_goal_tools(Arc::clone(&goal_manager));
        }
        runtime = runtime.with_goal_manager(&goal_manager);
    }
    Ok(runtime)
}

#[cfg(test)]
async fn run_prompt_with_runtime(
    prompt: String,
    context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
) -> anyhow::Result<PromptTurn> {
    let mut writer = SessionEventWriter::jsonl(writer);
    let (user_message, events) =
        append_user_event(vec![Content::text(prompt)], &mut writer).await?;
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
    content: Vec<Content>,
    writer: &mut SessionEventWriter<'_>,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let user_message = AgentMessage::User { content };
    let user_event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    writer.append_event(&user_event).await?;
    writer.flush().await?;
    Ok((user_message, vec![user_event]))
}

async fn append_user_event_jsonl(
    content: Vec<Content>,
    writer: &mut JsonlSessionWriter,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let mut writer = SessionEventWriter::jsonl(writer);
    append_user_event(content, &mut writer).await
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
            assistant_text.push_str(&message.text());
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

struct PreparedStreamingTurn {
    prompt: String,
    session_id: String,
    session_directory: PathBuf,
    context: AgentContext,
    writer: JsonlSessionWriter,
    user_message: AgentMessage,
    initial_events: Vec<AgentEvent>,
}

#[derive(Debug, PartialEq, Eq)]
struct StreamingEventEffect {
    persist: bool,
    assistant_text: Option<String>,
}

async fn finish_prompt_turn_streaming(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    initial_events: Vec<AgentEvent>,
    streaming: StreamingTurnIo,
) -> anyhow::Result<PromptTurn> {
    let mut events = forward_initial_streaming_events(&streaming.event_tx, initial_events);
    let mut assistant_text = String::new();
    let mut stream =
        runtime.run_turn_with_cancel(&mut context, user_message.clone(), streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = streaming_event_or_bail(event, &streaming.event_tx)?;
        append_streaming_event(
            &event,
            &user_message,
            writer,
            &mut assistant_text,
            &streaming.event_tx,
            &mut events,
        )
        .await?;
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id: streaming.session_id,
        events,
        assistant_text,
    })
}

async fn finish_compaction_turn_streaming(
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    initial_events: Vec<AgentEvent>,
    streaming: StreamingTurnIo,
) -> anyhow::Result<PromptTurn> {
    let mut events = forward_initial_streaming_events(&streaming.event_tx, initial_events);
    let mut stream = runtime.run_compaction_turn_with_cancel(&mut context, streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = streaming_event_or_bail(event, &streaming.event_tx)?;
        writer.append_event(&event).await?;
        let _ = streaming.event_tx.send(Ok(event.clone()));
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id: streaming.session_id,
        events,
        assistant_text: String::new(),
    })
}

fn forward_initial_streaming_events(
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    initial_events: Vec<AgentEvent>,
) -> Vec<AgentEvent> {
    for event in &initial_events {
        let _ = event_tx.send(Ok(event.clone()));
    }
    initial_events
}

fn streaming_event_or_bail<E: std::fmt::Display>(
    event: Result<AgentEvent, E>,
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
) -> anyhow::Result<AgentEvent> {
    event.map_err(|error| {
        let message = error.to_string();
        let _ = event_tx.send(Err(anyhow::anyhow!(message.clone())));
        anyhow::anyhow!(message)
    })
}

async fn append_streaming_event(
    event: &AgentEvent,
    user_message: &AgentMessage,
    writer: &mut JsonlSessionWriter,
    assistant_text: &mut String,
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    events: &mut Vec<AgentEvent>,
) -> anyhow::Result<()> {
    let effect = streaming_event_effect(event, user_message);
    if let Some(text) = effect.assistant_text {
        assistant_text.push_str(&text);
    }
    if effect.persist {
        writer.append_event(event).await?;
    }
    let _ = event_tx.send(Ok(event.clone()));
    events.push(event.clone());
    Ok(())
}

fn streaming_event_effect(event: &AgentEvent, user_message: &AgentMessage) -> StreamingEventEffect {
    if is_duplicate_user_message_event(event, user_message) {
        return StreamingEventEffect {
            persist: false,
            assistant_text: None,
        };
    }
    StreamingEventEffect {
        persist: true,
        assistant_text: assistant_text_from_event(event),
    }
}

fn is_duplicate_user_message_event(event: &AgentEvent, user_message: &AgentMessage) -> bool {
    matches!(
        event,
        AgentEvent::MessageAppended { message } if message == user_message
    )
}

fn assistant_text_from_event(event: &AgentEvent) -> Option<String> {
    let AgentEvent::MessageAppended { message } = event else {
        return None;
    };
    if matches!(message, AgentMessage::Assistant { .. }) {
        Some(message.text())
    } else {
        None
    }
}

pub(crate) fn agent_config_for_app(
    model: ModelSpec,
    config: &AppConfig,
    approval_tx: Option<mpsc::UnboundedSender<PromptApprovalRequest>>,
    skill_store: &SkillStore,
) -> anyhow::Result<AgentConfig> {
    let mut agent_config = AgentConfig::for_model(model)
        .with_permission_mode(config.permission_mode)
        .with_live_permission_mode(Arc::clone(&config.live_permission_mode))
        .with_queue_modes(
            config.runtime.steering_queue_mode,
            config.runtime.follow_up_queue_mode,
        )
        .with_tool_execution_mode(config.runtime.tool_execution_mode)
        .with_background_tasks(config.background_tasks.clone())
        .with_workspace_root(&config.project_dir)?;
    if let Some(home) = neo_home() {
        agent_config = agent_config.with_home_dir(home);
        // Layer 2: load persistent prefix-approval rules from disk so they
        // survive restarts.
        agent_config.load_prefix_approval_rules();
    }
    agent_config.temperature = config.runtime.temperature;
    // Output token cap precedence: explicit `[runtime].max_tokens` wins; otherwise
    // fall back to the model's declared `max_output_tokens`; otherwise leave unset
    // and let the provider decide (Anthropic applies its own fallback).
    agent_config.max_tokens = config
        .runtime
        .max_tokens
        .or(agent_config.model.capabilities.max_output_tokens);
    agent_config.reasoning_effort = config.runtime.reasoning_effort;
    agent_config.replay_reasoning = config.runtime.replay_reasoning;
    if let Some(system_prompt) =
        resources::load_system_prompt(&config.project_dir, config.project_trusted, skill_store)?
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
            trigger_ratio: compaction.trigger_ratio,
            reserved_context_tokens: compaction.reserved_context_tokens,
            max_recent_messages: compaction.max_recent_messages,
            micro_enabled: compaction.micro_enabled,
            micro_keep_recent: compaction.micro_keep_recent,
        });
    }
    if let Some(approval_tx) = approval_tx {
        agent_config = attach_async_approval_handler(agent_config, approval_tx);
    }
    Ok(agent_config)
}

fn attach_async_approval_handler(
    agent_config: AgentConfig,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
) -> AgentConfig {
    let plan_review_feedback = Arc::clone(&agent_config.plan_review_feedback);
    let plan_review_selected_label = Arc::clone(&agent_config.plan_review_selected_label);
    agent_config.with_async_approval_handler(move |request| {
        let approval_tx = approval_tx.clone();
        let plan_review_feedback = Arc::clone(&plan_review_feedback);
        let plan_review_selected_label = Arc::clone(&plan_review_selected_label);
        async move {
            let (decision_tx, decision_rx) = oneshot::channel();
            let (feedback_tx, feedback_rx) = oneshot::channel();
            let (selected_label_tx, selected_label_rx) = oneshot::channel();
            let id = request.id.clone();
            let operation = request.operation;
            let session_scope = request.session_scope.clone();
            let prefix_rule = request.prefix_rule.clone();
            let session_option_label = session_scope
                .as_ref()
                .filter(|scope| !scope.is_empty())
                .map(|scope| scope.label.clone());
            let prefix_option_label = prefix_rule
                .as_ref()
                .map(|rule| format!("Approve commands starting with {}", rule.label));
            if approval_tx
                .send(PromptApprovalRequest {
                    id,
                    decision_tx,
                    feedback_tx: Some(feedback_tx),
                    selected_label_tx: Some(selected_label_tx),
                    session_option_label,
                    prefix_option_label,
                })
                .is_err()
            {
                return PermissionApprovalDecision::Reject;
            }
            let decision = decision_rx
                .await
                .unwrap_or(PermissionApprovalDecision::Reject);
            if decision == PermissionApprovalDecision::Reject
                && matches!(
                    operation,
                    PermissionOperation::PlanTransition | PermissionOperation::GoalTransition
                )
                && let Ok(Some(feedback)) = feedback_rx.await
                && !feedback.trim().is_empty()
                && let Ok(mut map) = plan_review_feedback.lock()
            {
                map.insert(request.id.clone(), feedback);
            }
            // The user approved a specific model-supplied plan-review
            // option. Record its label so `attach_exit_plan_details` can
            // prefix the tool result with "Selected approach: <label>".
            if decision == PermissionApprovalDecision::AllowOnce
                && operation == PermissionOperation::PlanTransition
                && let Ok(Some(label)) = selected_label_rx.await
                && !label.trim().is_empty()
                && let Ok(mut map) = plan_review_selected_label.lock()
            {
                map.insert(request.id.clone(), label);
            }
            decision
        }
    })
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

pub(crate) async fn tool_registry_for_config(
    config: &AppConfig,
    todos: std::sync::Arc<std::sync::Mutex<Vec<neo_agent_core::TodoEventData>>>,
    mcp_manager: Option<&McpConnectionManager>,
) -> anyhow::Result<ToolRegistry> {
    let mut registry = ToolRegistry::with_builtin_tools_and_todos(todos);
    let extension_home =
        crate::config::neo_home().unwrap_or_else(|| config.project_dir.join(".neo"));
    neo_agent_core::tools::extensions::register_enabled_extension_tools(
        &mut registry,
        &neo_agent_core::tools::extensions::default_extension_root(&extension_home),
        &neo_agent_core::tools::extensions::default_extension_state_path(&extension_home),
    )
    .await?;
    let manager;
    let manager_ref = if let Some(manager) = mcp_manager {
        manager
    } else {
        manager = McpConnectionManager::new(ProcessSupervisor::default());
        &manager
    };
    if let Err(error) = crate::mcp_ops::reload_mcp_manager_from_config(config, manager_ref).await {
        tracing::warn!(?error, "failed to load MCP manager config");
    } else {
        wait_for_mcp_manager_probe(manager_ref, config).await;
        for diagnostic in manager_ref
            .register_connected_tools_into(&mut registry)
            .await
        {
            tracing::warn!(
                server_id = %diagnostic.server_id,
                message = %diagnostic.message,
                "MCP server unavailable"
            );
        }
    }
    Ok(registry)
}

async fn wait_for_mcp_manager_probe(manager: &McpConnectionManager, config: &AppConfig) {
    let enabled_count = config
        .mcp
        .servers
        .iter()
        .filter(|server| server.enabled)
        .count();
    if enabled_count == 0 {
        return;
    }
    let max_configured_timeout = config
        .mcp
        .servers
        .iter()
        .filter(|server| server.enabled)
        .filter_map(|server| server.startup_timeout_ms)
        .max()
        .unwrap_or(500);
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_millis(max_configured_timeout.min(1_000));
    loop {
        let snapshots = manager.snapshots().await;
        let settled = snapshots.iter().all(|snapshot| {
            !matches!(
                snapshot.status,
                McpServerStatus::Pending | McpServerStatus::Reconnecting
            )
        });
        if settled || tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Build a one-off [`McpClient`] for a short-lived CLI command.
///
/// This is used only by CLI operations such as `mcp add --probe` and
/// post-add tool listing. For HTTP/SSE servers it goes through
/// [`build_http_client_with_oauth`], which creates a *standalone*
/// `AuthorizationManager` that is not shared with the long-lived
/// [`McpConnectionManager`] credential store.
///
/// For long-lived connections (e.g. inside `tool_registry_for_config`),
/// use [`McpConnectionManager`] directly so that OAuth credentials persist
/// in the shared store.
async fn build_mcp_client(
    server: &McpServerConfig,
    supervisor: &ProcessSupervisor,
) -> anyhow::Result<Arc<dyn McpClient>> {
    match server.transport {
        McpTransport::Stdio => {
            let command = server
                .command
                .clone()
                .with_context(|| format!("missing MCP command for {}", server.id))?;
            let client = build_stdio_client(
                &server.id,
                StdioConfig {
                    command,
                    args: server.args.clone(),
                    env: server.env.clone(),
                    cwd: server.cwd.clone(),
                    startup_timeout_ms: server.startup_timeout_ms,
                    tool_timeout_ms: server.tool_timeout_ms,
                },
                supervisor,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(client)
        }
        McpTransport::Http | McpTransport::Sse => {
            let url = server
                .url
                .clone()
                .with_context(|| format!("missing MCP url for {}", server.id))?;
            let oauth_store_path = neo_home().map(|home| home.join("oauth.json"));
            let client = build_http_client_with_oauth(
                url,
                server.headers.clone(),
                server.startup_timeout_ms,
                server.tool_timeout_ms,
                oauth_store_path,
                &server.id,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(client)
        }
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

    loop {
        let session_id = format!("session_{}", Uuid::new_v4());
        let session_dir = bucket_dir.join(&session_id);
        if tokio::fs::metadata(&session_dir).await.is_err() {
            tokio::fs::create_dir_all(&session_dir)
                .await
                .with_context(|| {
                    format!(
                        "failed to create session directory {}",
                        session_dir.display()
                    )
                })?;
            return Ok(session_dir.join("transcript.jsonl"));
        }
    }
}

fn session_id_from_path(path: &Path) -> anyhow::Result<String> {
    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session path {}", path.display()))?;

    if file_name != "transcript.jsonl" {
        anyhow::bail!("invalid session path {}", path.display());
    }

    let session_dir = path.parent().with_context(|| {
        format!(
            "session transcript has no parent directory {}",
            path.display()
        )
    })?;
    let dir_name = session_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session directory name {}", session_dir.display()))?;
    if dir_name.starts_with("session_") {
        return Ok(dir_name.to_owned());
    }

    anyhow::bail!("invalid session path {}", path.display())
}

pub(crate) fn latest_session_id(config: &AppConfig) -> anyhow::Result<String> {
    let bucket_dir = workspace_sessions_dir(config);
    let mut latest: Option<(std::time::SystemTime, String)> = None;
    let entries = std::fs::read_dir(&bucket_dir)
        .with_context(|| format!("failed to read sessions directory {}", bucket_dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("session_") {
            continue;
        }
        let transcript = path.join("transcript.jsonl");
        if !transcript.exists() {
            continue;
        }
        let Ok(session_id) = session_id_from_path(&transcript) else {
            continue;
        };
        if neo_agent_core::session::validate_session_id(&session_id).is_err() {
            continue;
        }
        let modified = std::fs::metadata(&transcript)
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

pub(crate) fn resolve_model(config: &AppConfig) -> anyhow::Result<ModelSpec> {
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
    let default = find_default_model(&models, config);
    if config.model_scope.is_empty() {
        return default.cloned().with_context(|| {
            format!(
                "unknown model {}; run `neo models list` for supported catalog entries",
                config.default_model_label()
            )
        });
    }

    candidates
        .iter()
        .find(|model| model_spec_matches_default(model, config))
        .or_else(|| candidates.first())
        .cloned()
        .with_context(|| {
            format!(
                "unknown model {}; run `neo models list` for supported catalog entries",
                config.default_model_label()
            )
        })
}

fn find_default_model<'a>(models: &'a [ModelSpec], config: &AppConfig) -> Option<&'a ModelSpec> {
    if let Some(model_cfg) = config.models.get(&config.default_model) {
        return models.iter().find(|model| {
            model.provider.0 == model_cfg.provider && model.model == model_cfg.model
        });
    }
    models
        .iter()
        .find(|model| model_spec_matches_default(model, config))
}

fn model_spec_matches_default(model: &ModelSpec, config: &AppConfig) -> bool {
    let qualified = format!("{}/{}", model.provider.0, model.model);
    qualified == config.default_model
        || (model.provider.0 == config.default_provider && model.model == config.default_model)
}

fn model_config_matches_default(alias: &str, model_cfg: &ModelConfig, config: &AppConfig) -> bool {
    alias == config.default_model
        || (model_cfg.provider == config.default_provider
            && model_cfg.model == config.default_model)
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
    let capabilities = parse_model_capabilities(
        &cfg.capabilities,
        cfg.max_context_tokens,
        cfg.max_output_tokens,
    );

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
    max_output_tokens: Option<u32>,
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
    mc.max_output_tokens = max_output_tokens;
    mc
}

pub(crate) fn resolve_model_client(
    config: &AppConfig,
    model: &ModelSpec,
) -> anyhow::Result<Arc<dyn ModelClient>> {
    const RESOLVED_API_KEY_ENV: &str = "__NEO_RESOLVED_API_KEY";
    let mut registry = provider_registry_for_config(config);
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use neo_agent_core::{
        AgentConfig, AgentEvent, AgentMessage, ApprovalRequest, CompactionSettings, Content,
        PermissionApprovalDecision, PermissionMode, PermissionOperation, QueueMode,
        StopReason as AgentStopReason, ToolExecutionMode,
        session::{JsonlSessionReader, JsonlSessionWriter},
        skills::SkillStore,
    };
    use neo_ai::{
        AiStreamEvent, ApiKind, ApiType, ChatMessage, ContentPart, ModelCapabilities, ModelSpec,
        ProviderId, StopReason, providers::fake::FakeModelClient,
    };

    use super::{
        PromptApprovalRequest, StableJsonState, agent_config_for_app, auth_mcp_server,
        create_session_path, list_configured_models, model_registry_for_config,
        run_prompt_with_runtime, select_config_model, tool_registry_for_config,
    };
    use crate::config::{
        AppConfig, Defaults, McpConfig, McpTransport, ModelConfig, ProviderConfig,
        RuntimeCompactionConfig, RuntimeConfig, TuiConfig,
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
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
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
                    trigger_ratio: 0.85,
                    reserved_context_tokens: 50_000,
                    max_recent_messages: 4,
                    micro_enabled: false,
                    micro_keep_recent: 20,
                }),
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat(),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store).expect("agent config");

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
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
            })
        );
        assert!(agent_config.workspace_root.is_some());
    }

    #[test]
    fn agent_config_for_app_falls_back_to_model_max_output_tokens() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::OneAtATime,
                follow_up_queue_mode: QueueMode::OneAtATime,
                tool_execution_mode: ToolExecutionMode::Sequential,
                compaction: None,
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        // Model declares max_output_tokens; runtime does not override.
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat().with_max_output_tokens(64_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store).expect("agent config");

        assert_eq!(agent_config.max_tokens, Some(64_000));
    }

    #[tokio::test]
    async fn create_session_path_uses_named_uuid_session_ids() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };

        let path = create_session_path(&config)
            .await
            .expect("session path is created");
        let session_dir = path.parent().expect("session directory");
        let session_id = session_dir
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("session id");

        assert!(session_id.starts_with("session_"));
        assert_eq!(session_id.len(), "session_".len() + 36);
        assert!(neo_agent_core::session::validate_session_id(session_id).is_ok());
        assert!(path.ends_with("transcript.jsonl"));
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
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
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
                    trigger_ratio: 0.85,
                    reserved_context_tokens: 50_000,
                    max_recent_messages: 4,
                    micro_enabled: false,
                    micro_keep_recent: 20,
                }),
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store).expect("agent config");

        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 800_000,
                keep_recent_messages: 20,
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
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
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
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
                    trigger_ratio: 0.85,
                    reserved_context_tokens: 50_000,
                    max_recent_messages: 4,
                    micro_enabled: false,
                    micro_keep_recent: 20,
                }),
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store).expect("agent config");

        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 12_000,
                keep_recent_messages: 16,
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
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
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
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
        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config = agent_config_for_app(model, &config, Some(approval_tx), &skill_store)
            .expect("agent config");
        let handler = agent_config
            .async_approval_handler
            .expect("async approval handler");

        let decision = tokio::spawn(handler(ApprovalRequest {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: PermissionOperation::Tool,
            subject: "Write".to_owned(),
            arguments: serde_json::json!({"path": "approved.txt"}),
            session_scope: None,
            prefix_rule: None,
        }));
        let PromptApprovalRequest {
            id,
            decision_tx,
            feedback_tx: _,
            selected_label_tx: _,
            session_option_label: _,
            prefix_option_label: _,
        } = approval_rx.recv().await.expect("approval waiter");

        assert_eq!(id, "tool-1");
        decision_tx
            .send(PermissionApprovalDecision::AllowOnce)
            .expect("send decision");
        assert_eq!(
            decision.await.expect("approval task joins"),
            PermissionApprovalDecision::AllowOnce
        );
    }

    #[tokio::test]
    async fn run_prompt_with_runtime_appends_continuation_to_existing_session_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = temp
            .path()
            .join("session_00000000-0000-4000-8000-000000000501");
        let session_path = session_dir.join("transcript.jsonl");
        tokio::fs::create_dir_all(&session_dir)
            .await
            .expect("create session dir");
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

    #[test]
    fn streaming_event_effects_skip_duplicate_user_message() {
        let user_message = AgentMessage::user_text("hello");
        let event = AgentEvent::MessageAppended {
            message: user_message.clone(),
        };

        let effect = super::streaming_event_effect(&event, &user_message);

        assert!(!effect.persist);
        assert_eq!(effect.assistant_text.as_deref(), None);
    }

    #[test]
    fn streaming_event_effects_persist_assistant_text() {
        let user_message = AgentMessage::user_text("hello");
        let event = AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("answer")],
                Vec::new(),
                AgentStopReason::EndTurn,
            ),
        };

        let effect = super::streaming_event_effect(&event, &user_message);

        assert!(effect.persist);
        assert_eq!(effect.assistant_text.as_deref(), Some("answer"));
    }

    #[test]
    fn streaming_event_effects_persist_non_message_events_without_text() {
        let user_message = AgentMessage::user_text("hello");
        let event = AgentEvent::TurnStarted { turn: 1 };

        let effect = super::streaming_event_effect(&event, &user_message);

        assert!(effect.persist);
        assert_eq!(effect.assistant_text.as_deref(), None);
    }

    #[test]
    fn list_configured_models_formats_text_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "gpt-4.1".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                provider_type: Some(ApiType::OpenAiResponses),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "fast".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                max_context_tokens: Some(1_000_000),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                display_name: Some("GPT 4.1".to_owned()),
                ..ModelConfig::default()
            },
        );
        config.models.insert(
            "local/echo".to_owned(),
            ModelConfig {
                provider: "missing".to_owned(),
                model: "echo".to_owned(),
                capabilities: vec!["streaming".to_owned()],
                ..ModelConfig::default()
            },
        );

        let output = list_configured_models(&config, false).expect("models list");

        assert_eq!(
            output,
            concat!(
                "models:\n",
                "- fast -> openai/gpt-4.1 (openai-responses default) ctx=1000000 [streaming,tools] - GPT 4.1\n",
                "- local/echo (unknown) ctx=? [streaming]\n",
            )
        );
    }

    #[test]
    fn list_configured_models_formats_json_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "fast".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                provider_type: Some(ApiType::OpenAiResponses),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "fast".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                max_context_tokens: Some(1_000_000),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                display_name: Some("GPT 4.1".to_owned()),
                ..ModelConfig::default()
            },
        );

        let output = list_configured_models(&config, true).expect("models json");
        let value: serde_json::Value = serde_json::from_str(&output).expect("json output");

        assert_eq!(
            value,
            serde_json::json!({
                "models": [{
                    "alias": "fast",
                    "provider": "openai",
                    "model": "gpt-4.1",
                    "type": "openai-responses",
                    "capabilities": ["streaming", "tools"],
                    "max_context_tokens": 1_000_000,
                    "display_name": "GPT 4.1",
                    "default": true,
                }],
                "default_model": "fast",
            })
        );
    }

    #[test]
    fn select_config_model_accepts_default_model_alias() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "openai/gpt-large".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                provider_type: Some(ApiType::OpenAiResponses),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "openai/gpt-large".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-large".to_owned(),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                ..ModelConfig::default()
            },
        );

        let registry = model_registry_for_config(&config).expect("registry");
        let model = select_config_model(&registry, &config).expect("model resolves");

        assert_eq!(model.provider.0, "openai");
        assert_eq!(model.model, "gpt-large");
    }

    #[test]
    fn select_config_model_accepts_unqualified_config_alias() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "fast".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                provider_type: Some(ApiType::OpenAiResponses),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "fast".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                ..ModelConfig::default()
            },
        );

        let registry = model_registry_for_config(&config).expect("registry");
        let model = select_config_model(&registry, &config).expect("alias resolves");

        assert_eq!(model.provider.0, "openai");
        assert_eq!(model.model, "gpt-4.1");
    }

    #[test]
    fn select_config_model_accepts_bare_default_model_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "gpt-4.1".to_owned();

        let registry = model_registry_for_config(&config).expect("registry");
        let model = select_config_model(&registry, &config).expect("builtin model resolves");

        assert_eq!(model.provider.0, "openai");
        assert_eq!(model.model, "gpt-4.1");
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

    fn test_config(project_dir: &std::path::Path) -> AppConfig {
        AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: project_dir.join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: project_dir.to_path_buf(),
            config_path: project_dir.join(".neo/config.toml"),
        }
    }

    #[tokio::test]
    async fn tool_registry_ignores_failed_mcp_server_startup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.mcp.servers.push(crate::config::McpServerConfig {
            id: "bad".to_owned(),
            enabled: true,
            transport: McpTransport::Stdio,
            command: Some("neo-missing-mcp-binary-for-test".to_owned()),
            url: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            startup_timeout_ms: Some(50),
            tool_timeout_ms: None,
        });

        let registry =
            tool_registry_for_config(&config, Arc::new(std::sync::Mutex::new(Vec::new())), None)
                .await
                .expect("bad MCP server should not abort registry construction");

        assert!(
            registry
                .specs()
                .iter()
                .all(|spec| !spec.name.starts_with("mcp__bad__")),
            "failed MCP tools must not be exposed"
        );
    }

    fn test_mcp_server(
        id: &str,
        transport: McpTransport,
        url: Option<&str>,
    ) -> crate::config::McpServerConfig {
        crate::config::McpServerConfig {
            id: id.to_owned(),
            enabled: true,
            transport,
            command: None,
            url: url.map(str::to_owned),
            args: Vec::new(),
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            startup_timeout_ms: None,
            tool_timeout_ms: None,
        }
    }

    #[tokio::test]
    async fn auth_mcp_server_errors_for_missing_server() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = test_config(temp.path());
        let result = auth_mcp_server("missing".to_owned(), &config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn auth_mcp_server_errors_for_non_remote_transport() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config
            .mcp
            .servers
            .push(test_mcp_server("fs", McpTransport::Stdio, None));
        let result = auth_mcp_server("fs".to_owned(), &config).await;
        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(message.contains("HTTP/SSE"), "unexpected error: {message}");
    }
}
