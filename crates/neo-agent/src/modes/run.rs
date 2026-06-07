use std::{collections::BTreeMap, fmt::Write as _, sync::Arc};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::session::JsonlSessionWriter;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, Content, McpHttpConfig,
    McpHttpToolAdapter, McpStdioConfig, McpStdioToolAdapter, McpToolAdapter, McpToolProvider,
    PermissionDecision, ToolRegistry,
};
use neo_ai::{ModelClient, ModelRegistry, ModelSpec, ProviderRegistry};

use crate::{
    config::{AppConfig, McpServerConfig},
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

pub async fn resume(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let transcript = session_commands::transcript(session_id, config).await?;
    Ok(format!("session {session_id}\n{transcript}"))
}

pub fn list_models(config: &AppConfig) -> String {
    let providers = provider_registry_for_config(config);
    let mut out = format!(
        "models:\n- {}/{} (configured default)\n",
        config.default_provider, config.default_model
    );
    for model in ModelRegistry::seeded().list() {
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
    out
}

fn provider_registry_for_config(config: &AppConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::production();
    if let Some(env_name) = &config.api_key_env
        && let Some(mut provider) = registry.get(&config.default_provider).cloned()
    {
        provider.api_key_env_vars = vec![env_name.clone()];
        registry.register(provider);
    }
    registry
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

pub struct PromptTurn {
    pub events: Vec<AgentEvent>,
    pub assistant_text: String,
}

pub async fn run_prompt(prompt: &[String], config: &AppConfig) -> anyhow::Result<PromptTurn> {
    let prompt = prompt.join(" ");
    let session_path = create_session_path(config).await?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;

    let user_message = AgentMessage::user_text(prompt.clone());
    let user_event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    writer.append_event(&user_event).await?;
    writer.flush().await?;

    let model = resolve_model(config)?;
    let client = resolve_model_client(config, &model)?;
    let tools = tool_registry_for_config(config).await?;
    let runtime = AgentRuntime::with_tools(agent_config_for_app(model, config)?, client, tools);
    let mut context = AgentContext::new();
    let mut events = vec![user_event];
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

fn agent_config_for_app(model: ModelSpec, config: &AppConfig) -> anyhow::Result<AgentConfig> {
    let mut agent_config = AgentConfig::for_model(model)
        .with_tool_permission_policy(config.permissions.clone())
        .with_workspace_root(&config.project_dir)?;
    if config.approve {
        agent_config = agent_config.with_approval_handler(|_| PermissionDecision::Allow);
    } else if config.no_approve {
        agent_config = agent_config.with_approval_handler(|_| PermissionDecision::Deny);
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
    ModelRegistry::seeded()
        .get(&config.default_provider, &config.default_model)
        .cloned()
        .with_context(|| {
            format!(
                "unknown model {}/{}; run `neo models list` for supported catalog entries",
                config.default_provider, config.default_model
            )
        })
}

fn resolve_model_client(
    config: &AppConfig,
    model: &ModelSpec,
) -> anyhow::Result<Arc<dyn ModelClient>> {
    let mut registry = ProviderRegistry::production();
    if config.api_base.is_some() || config.api_key_env.is_some() {
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
