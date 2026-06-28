use std::sync::Arc;

use anyhow::Context;
use neo_agent_core::{
    AgentConfig, CompactionSettings, McpClient, McpConnectionManager, McpServerStatus,
    ProcessSupervisor, StdioConfig, ToolRegistry, build_http_client_with_oauth, build_stdio_client,
};
use neo_agent_core::skills::SkillStore;
use tokio::sync::{mpsc, oneshot};
use neo_agent_core::{
    PermissionApprovalDecision, PermissionOperation,
};

use crate::config::{AppConfig, McpServerConfig, McpTransport, neo_home};
use crate::modes::run::PromptApprovalRequest;
use crate::resources;

pub(crate) fn agent_config_for_app(
    model: neo_ai::ModelSpec,
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
