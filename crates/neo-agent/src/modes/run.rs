use std::{collections::BTreeMap, fmt::Write as _, sync::Arc};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, CompactionSettings, Content,
    McpHttpConfig, McpHttpToolAdapter, McpStdioConfig, McpStdioToolAdapter, McpToolAdapter,
    McpToolProvider, PermissionDecision, ToolRegistry,
};
use neo_ai::{ModelClient, ModelRegistry, ModelSpec, ProviderRegistry};
use tokio::sync::{mpsc, oneshot};

use crate::{
    config::{AppConfig, McpServerConfig, provider_api_base},
    session_commands,
};

pub async fn execute(prompt: &[String], config: &AppConfig) -> anyhow::Result<String> {
    let turn = run_prompt(prompt, config).await?;
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
    finish_prompt_turn(user_message, context, &mut writer, runtime, events).await
}

pub async fn run_prompt_streaming(
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = create_session_path(config).await?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let (user_message, events) = append_user_event(prompt, &mut writer).await?;
    let runtime = runtime_for_config(config, Some(approval_tx)).await?;
    finish_prompt_turn_streaming(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        event_tx,
    )
    .await
}

pub async fn run_prompt_in_session_streaming(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
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
    finish_prompt_turn_streaming(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        event_tx,
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
    finish_prompt_turn(user_message, context, writer, runtime, events).await
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
        events,
        assistant_text,
    })
}

async fn finish_prompt_turn_streaming(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    initial_events: Vec<AgentEvent>,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
) -> anyhow::Result<PromptTurn> {
    let mut events = Vec::new();
    for event in initial_events {
        let _ = event_tx.send(Ok(event.clone()));
        events.push(event);
    }

    let mut assistant_text = String::new();
    let mut stream = runtime.run_turn(&mut context, user_message.clone());
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                let message = error.to_string();
                let _ = event_tx.send(Err(anyhow::anyhow!(message.clone())));
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
        let _ = event_tx.send(Ok(event.clone()));
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
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
