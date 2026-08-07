//! display behavior (moved from `mcp_ops.rs`).

use super::super::*;
use super::*;

#[test]
fn parse_mcp_kind_maps_aliases() {
    assert_eq!(parse_mcp_kind("studio").unwrap(), McpTransport::Stdio);
    assert_eq!(parse_mcp_kind("stdio").unwrap(), McpTransport::Stdio);
    assert_eq!(parse_mcp_kind("remote-http").unwrap(), McpTransport::Http);
    assert_eq!(parse_mcp_kind("http").unwrap(), McpTransport::Http);
    assert_eq!(parse_mcp_kind("remote-sse").unwrap(), McpTransport::Sse);
    assert_eq!(parse_mcp_kind("sse").unwrap(), McpTransport::Sse);
    assert!(parse_mcp_kind("unknown").is_err());
}

#[test]
fn display_mcp_kind_round_trips() {
    assert_eq!(display_mcp_kind(McpTransport::Stdio), "studio");
    assert_eq!(display_mcp_kind(McpTransport::Http), "remote-http");
    assert_eq!(display_mcp_kind(McpTransport::Sse), "remote-sse");
}

#[test]
fn snapshot_summary_uses_real_tool_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    let config = crate::config::AppConfig {
        default_model: "gpt-4.1".to_owned(),
        default_provider: "openai".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: project_dir.join(".neo/sessions"),
        permission_mode: neo_agent_core::PermissionMode::Ask,
        live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
            neo_agent_core::PermissionMode::Ask,
        )),
        workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
        defaults: crate::config::Defaults {
            mode: "interactive".to_owned(),
        },
        runtime: crate::config::RuntimeConfig::default(),
        background_tasks: neo_agent_core::BackgroundTaskManager::new(),
        workflow_runtime: neo_agent_core::workflow::WorkflowRuntime::new(
            neo_agent_core::workflow::WorkflowLimits::default(),
        ),
        workflow_definitions: neo_agent_core::workflow::WorkflowDefinitionRegistry::empty(),
        workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver::default(),
        multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
        tui: crate::config::TuiConfig::default(),
        theme: crate::themes::ResolvedTheme::default(),
        theme_resolution: crate::themes::ThemeResolution::Default,
        mcp: crate::config::McpConfig {
            servers: vec![McpServerConfig {
                id: "docs".to_owned(),
                enabled: true,
                transport: McpTransport::Stdio,
                command: Some("docs-mcp".to_owned()),
                url: None,
                args: Vec::new(),
                env: BTreeMap::new(),
                headers: BTreeMap::new(),
                cwd: None,
                enabled_tools: Vec::new(),
                disabled_tools: Vec::new(),
                startup_timeout_ms: None,
                tool_timeout_ms: None,
            }],
        },
        prompt_templates: Vec::new(),
        system_prompt_file: None,
        extra_skill_dirs: Vec::new(),
        skill_path: Vec::new(),
        project_trusted: true,
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir,
        config_path: temp.path().join("config.toml"),
        config_file_exists: true,
    };
    let summaries = summarize_mcp_servers_from_snapshots(
        &config,
        &[McpServerSnapshot {
            id: "docs".to_owned(),
            transport: "stdio".to_owned(),
            status: McpServerStatus::Connected,
            tool_count: 2,
            tool_names: vec!["read_doc".to_owned(), "search_doc".to_owned()],
            resource_count: Some(0),
            error: None,
            reconnect_attempt: 0,
            next_retry_ms: None,
        }],
    );

    assert_eq!(
        summaries[0].tools,
        McpToolDiscovery::Success(vec!["read_doc".to_owned(), "search_doc".to_owned()])
    );
}

#[test]
fn snapshot_summary_maps_needs_auth() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    let config = crate::config::AppConfig {
        default_model: "gpt-4.1".to_owned(),
        default_provider: "openai".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: project_dir.join(".neo/sessions"),
        permission_mode: neo_agent_core::PermissionMode::Ask,
        live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
            neo_agent_core::PermissionMode::Ask,
        )),
        workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
        defaults: crate::config::Defaults {
            mode: "interactive".to_owned(),
        },
        runtime: crate::config::RuntimeConfig::default(),
        background_tasks: neo_agent_core::BackgroundTaskManager::new(),
        workflow_runtime: neo_agent_core::workflow::WorkflowRuntime::new(
            neo_agent_core::workflow::WorkflowLimits::default(),
        ),
        workflow_definitions: neo_agent_core::workflow::WorkflowDefinitionRegistry::empty(),
        workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver::default(),
        multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
        tui: crate::config::TuiConfig::default(),
        theme: crate::themes::ResolvedTheme::default(),
        theme_resolution: crate::themes::ThemeResolution::Default,
        mcp: crate::config::McpConfig {
            servers: vec![McpServerConfig {
                id: "linear".to_owned(),
                enabled: true,
                transport: McpTransport::Http,
                command: None,
                url: Some("https://mcp.example.com/mcp".to_owned()),
                args: Vec::new(),
                env: BTreeMap::new(),
                headers: BTreeMap::new(),
                cwd: None,
                enabled_tools: Vec::new(),
                disabled_tools: Vec::new(),
                startup_timeout_ms: None,
                tool_timeout_ms: None,
            }],
        },
        prompt_templates: Vec::new(),
        system_prompt_file: None,
        extra_skill_dirs: Vec::new(),
        skill_path: Vec::new(),
        project_trusted: true,
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir,
        config_path: temp.path().join("config.toml"),
        config_file_exists: true,
    };
    let summaries = summarize_mcp_servers_from_snapshots(
        &config,
        &[McpServerSnapshot {
            id: "linear".to_owned(),
            transport: "http".to_owned(),
            status: McpServerStatus::NeedsAuth,
            tool_count: 0,
            tool_names: Vec::new(),
            resource_count: None,
            error: Some(neo_agent_core::McpDiagnostic {
                server_id: "linear".to_owned(),
                transport: "http".to_owned(),
                message: "OAuth authentication required".to_owned(),
                hint: Some("Run /mcp and authenticate this server.".to_owned()),
                stderr_tail: Some("\x1b]0;owned\x07authorization failed".to_owned()),
            }),
            reconnect_attempt: 0,
            next_retry_ms: None,
        }],
    );

    assert_eq!(
        summaries[0].tools,
        McpToolDiscovery::NeedsAuth(
            "OAuth authentication required · stderr: authorization failed".to_owned()
        )
    );
}

#[test]
fn startup_message_formats_connected_server_like_kimi() {
    let snapshot = McpServerSnapshot {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        status: McpServerStatus::Connected,
        tool_count: 38,
        tool_names: Vec::new(),
        resource_count: None,
        error: None,
        reconnect_attempt: 0,
        next_retry_ms: None,
    };

    assert_eq!(
        format_mcp_startup_message(&snapshot),
        "MCP server \"linear\" connected · 38 tools (http)"
    );
}

#[test]
fn startup_status_data_maps_connected_snapshot() {
    let snapshot = McpServerSnapshot {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        status: McpServerStatus::Connected,
        tool_count: 38,
        tool_names: Vec::new(),
        resource_count: None,
        error: None,
        reconnect_attempt: 0,
        next_retry_ms: None,
    };

    assert_eq!(
        mcp_startup_status_from_snapshot(&snapshot),
        neo_tui::transcript::McpStartupStatusData {
            id: "linear".to_owned(),
            transport: "http".to_owned(),
            phase: neo_tui::transcript::McpStartupPhase::Connected { tool_count: 38 },
        }
    );
}

#[test]
fn startup_message_formats_needs_auth_with_hint() {
    let snapshot = McpServerSnapshot {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        status: McpServerStatus::NeedsAuth,
        tool_count: 0,
        tool_names: Vec::new(),
        resource_count: None,
        error: Some(neo_agent_core::McpDiagnostic {
            server_id: "linear".to_owned(),
            transport: "http".to_owned(),
            message: "OAuth authentication required".to_owned(),
            hint: Some("Run /mcp to authenticate.".to_owned()),
            stderr_tail: None,
        }),
        reconnect_attempt: 0,
        next_retry_ms: None,
    };

    assert_eq!(
        format_mcp_startup_message(&snapshot),
        "MCP server \"linear\" needs OAuth · Run /mcp to authenticate."
    );
}

#[test]
fn failed_mcp_diagnostics_render_sanitized_stderr_tail() {
    let snapshot = McpServerSnapshot {
        id: "broken".to_owned(),
        transport: "stdio".to_owned(),
        status: McpServerStatus::Failed,
        tool_count: 0,
        tool_names: Vec::new(),
        resource_count: None,
        error: Some(neo_agent_core::McpDiagnostic {
            server_id: "broken".to_owned(),
            transport: "stdio".to_owned(),
            message: "connection closed".to_owned(),
            hint: None,
            stderr_tail: Some("\x1b]0;owned\x07visible failure\nsecond line".to_owned()),
        }),
        reconnect_attempt: 0,
        next_retry_ms: None,
    };

    let status = format_mcp_status(std::slice::from_ref(&snapshot));
    let startup = mcp_startup_status_from_snapshot(&snapshot);

    assert!(status.contains("stderr: visible failure | second line"));
    assert!(!status.contains('\x1b'));
    assert_eq!(
        startup.phase,
        McpStartupPhase::Failed {
            message: "connection closed · stderr: visible failure | second line".to_owned(),
        }
    );
}
