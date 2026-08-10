//! Shared fixtures for the web session-state unit tests: an in-memory
//! `AppConfig`, a running-turn `WebSessionState`, and a user-message event.

use std::collections::BTreeMap;

use super::*;
use crate::config::{Defaults, RuntimeConfig, TuiConfig};

pub(super) fn test_config() -> AppConfig {
    AppConfig {
        default_model: "test-model".to_owned(),
        default_provider: "openai".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: std::env::temp_dir().join("neo-webui-test-sessions"),
        permission_mode: PermissionMode::Ask,
        live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(PermissionMode::Ask)),
        workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
        defaults: Defaults {
            mode: "webui".to_owned(),
        },
        runtime: RuntimeConfig::default(),
        background_tasks: neo_agent_core::BackgroundTaskManager::new(),
        workflow_runtime: neo_agent_core::workflow::WorkflowRuntime::new(
            neo_agent_core::workflow::WorkflowLimits::default(),
        ),
        workflow_definitions: neo_agent_core::workflow::WorkflowDefinitionRegistry::empty(),
        workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver::default(),
        multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
        tui: TuiConfig::default(),
        theme: crate::themes::ResolvedTheme::default(),
        theme_resolution: crate::themes::ThemeResolution::Default,
        mcp: crate::config::McpConfig::default(),
        prompt_templates: Vec::new(),
        system_prompt_file: None,
        extra_skill_dirs: Vec::new(),
        skill_path: Vec::new(),
        project_trusted: true,
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: std::env::temp_dir().join("neo-webui-test-project"),
        config_path: std::env::temp_dir().join("neo-webui-test-config.toml"),
        config_file_exists: true,
    }
}

pub(super) fn test_state(
    relay: &Relay,
    session_id: &str,
    turn_id: Option<&str>,
) -> Arc<Mutex<WebSessionState>> {
    test_state_in(
        relay,
        session_id,
        turn_id,
        std::env::temp_dir().join("neo-webui-test-session-dir"),
    )
}

pub(super) fn test_state_in(
    relay: &Relay,
    session_id: &str,
    turn_id: Option<&str>,
    session_dir: std::path::PathBuf,
) -> Arc<Mutex<WebSessionState>> {
    let state = WebSessionState::new(
        session_id.to_owned(),
        session_dir,
        relay.publisher(session_id),
        PerSessionContainers::fresh(&test_config()),
        std::env::temp_dir().join("neo-webui-test-project"),
        "neo-webui-test-project".to_owned(),
        None,
    );
    let state = Arc::new(Mutex::new(state));
    {
        let mut guard = state.lock().expect("state lock");
        guard.turn_id = turn_id.map(str::to_owned);
        guard.phase = if turn_id.is_some() {
            WebUiPhase::Running
        } else {
            WebUiPhase::Idle
        };
        guard.active = Some(ActiveTurnControl {
            cancel_token: CancellationToken::new(),
            steer_input: neo_agent_core::SteerInputHandle::new(),
        });
    }
    state
}

pub(super) fn user_message(text: &str) -> AgentEvent {
    AgentEvent::MessageAppended {
        message: AgentMessage::user_text(text),
    }
}
