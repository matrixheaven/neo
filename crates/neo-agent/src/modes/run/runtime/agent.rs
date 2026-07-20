use std::sync::Arc;

use anyhow::Context;
use neo_agent_core::instructions::{InstructionRegistry, InstructionRegistryConfig};
use neo_agent_core::{
    AgentConfig, ApprovalCancelReason, ApprovalResponse, CompactionSettings, HttpConfig,
    HttpOAuthConfig, McpClient, McpConnectionManager, ProcessSupervisor, StdioConfig, ToolRegistry,
    build_http_client, build_stdio_client,
};
use tokio::sync::{mpsc, oneshot};

use crate::config::{AppConfig, McpServerConfig, McpTransport, neo_home};
use crate::mcp_ops::{mcp_oauth_identity_for_server, mcp_oauth_service_for_current_home};
use crate::modes::run::PendingApproval;
use crate::resources;

pub(crate) fn agent_config_for_app(
    model: neo_ai::ModelSpec,
    config: &AppConfig,
    approval_tx: Option<mpsc::UnboundedSender<PendingApproval>>,
    instruction_registry: Option<Arc<InstructionRegistry>>,
) -> anyhow::Result<AgentConfig> {
    let mut agent_config = AgentConfig::for_model(model)
        .with_permission_mode(config.permission_mode)
        .with_live_permission_mode(Arc::clone(&config.live_permission_mode))
        .with_workspace_policy(Arc::clone(&config.workspace_policy))
        .with_queue_modes(
            config.runtime.steering_queue_mode,
            config.runtime.follow_up_queue_mode,
        )
        .with_tool_execution_mode(config.runtime.tool_execution_mode)
        .with_background_tasks(config.background_tasks.clone())
        .with_shell_runtime(config.runtime.shell_runtime.clone())
        .with_multi_agent(config.multi_agent.clone())
        .with_workspace_root(&config.project_dir)?;
    agent_config.instruction_registry = Some(match instruction_registry {
        Some(registry) => registry,
        None => Arc::new(InstructionRegistry::new(InstructionRegistryConfig {
            primary_workspace: config.project_dir.clone(),
            neo_home: neo_home(),
            project_trusted: config.project_trusted,
        })?),
    });
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
    agent_config.reasoning = config.runtime.reasoning.clone();
    agent_config.replay_reasoning = config.runtime.replay_reasoning;
    agent_config.max_retries = config.runtime.retry.max_retries;
    agent_config.first_event_timeout_secs = config.runtime.retry.first_event_timeout_secs;
    agent_config.stream_idle_timeout_secs = config.runtime.retry.stream_idle_timeout_secs;
    if let Some(system_prompt) =
        resources::load_system_prompt(config.system_prompt_file.as_deref())?
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
            max_rounds: compaction.max_rounds,
            max_retry_attempts: compaction.max_retry_attempts,
        });
    }
    if let Some(approval_tx) = approval_tx {
        agent_config = attach_async_approval_handler(agent_config, approval_tx);
    }
    Ok(agent_config)
}

fn attach_async_approval_handler(
    agent_config: AgentConfig,
    approval_tx: mpsc::UnboundedSender<PendingApproval>,
) -> AgentConfig {
    agent_config.with_async_approval_handler(move |request| {
        let approval_tx = approval_tx.clone();
        async move {
            let request_id = request.id.clone();
            let (response_tx, response_rx) = oneshot::channel();
            if approval_tx
                .send(PendingApproval {
                    request,
                    response_tx,
                })
                .is_err()
            {
                return ApprovalResponse::Cancelled {
                    request_id,
                    reason: ApprovalCancelReason::SessionEnded,
                };
            }
            response_rx.await.unwrap_or(ApprovalResponse::Cancelled {
                request_id,
                reason: ApprovalCancelReason::SessionEnded,
            })
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
        let snapshots = crate::mcp_ops::wait_for_mcp_manager_probe(manager_ref, config).await;
        for snapshot in snapshots {
            tracing::info!("{}", crate::mcp_ops::format_mcp_startup_message(&snapshot));
        }
        let diagnostics = manager_ref
            .register_connected_tools_into(&mut registry)
            .await;
        if mcp_manager.is_none() {
            for diagnostic in diagnostics {
                tracing::warn!(
                    server_id = %diagnostic.server_id,
                    message = %diagnostic.message,
                    "MCP server unavailable"
                );
            }
        }
    }
    Ok(registry)
}

const ONE_OFF_MCP_ATTEMPT_ID: u64 = 0;

/// Build a one-off [`McpClient`] for a short-lived CLI command.
///
/// This is used only by CLI operations such as `mcp add --probe` and
/// post-add tool listing. For HTTP/SSE servers it uses the same per-MCP OAuth
/// credential store as the long-lived [`McpConnectionManager`].
///
/// For long-lived connections (e.g. inside `tool_registry_for_config`),
/// use [`McpConnectionManager`] directly so that OAuth credentials persist
/// in the shared store.
pub(crate) async fn build_mcp_client(
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
                ONE_OFF_MCP_ATTEMPT_ID,
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
            let service = mcp_oauth_service_for_current_home();
            let identity = mcp_oauth_identity_for_server(&server.id, server)?;
            let client = build_http_client(HttpConfig {
                url,
                headers: server.headers.clone(),
                startup_timeout_ms: server.startup_timeout_ms,
                request_timeout_ms: server.tool_timeout_ms,
                oauth: Some(HttpOAuthConfig { service, identity }),
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(client)
        }
    }
}
