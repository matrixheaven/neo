//! Extracted: MCP manager overlay — open, sync from config, add servers, probe,
//! toggle, delete, and OAuth flow.

use crate::config::{self, neo_home};
use crate::mcp_ops::{self, authenticate_mcp_server_oauth};

use neo_tui::dialogs::{
    ChoiceItem, ChoicePickerOptions, McpAddFormData, McpAddFormOptions, McpAddFormResult,
    McpManagerAction, McpManagerOptions, McpServerRow, McpToolStatus,
};

use super::InteractiveController;

/// Background probe of a single MCP server (connect / refresh-tools).
pub(super) struct PendingMcpProbe {
    pub(super) server_id: String,
    pub(super) handle: tokio::task::JoinHandle<anyhow::Result<neo_agent_core::McpServerSnapshot>>,
}

impl InteractiveController {
    pub(super) async fn open_mcp_manager(&mut self) {
        let Some(config) = self.local_config.clone() else {
            self.push_status("No config available");
            return;
        };
        self.sync_mcp_manager_from_config().await;
        let summaries = if let Some(manager) = &self.mcp_manager {
            let snapshots = manager.snapshots().await;
            mcp_ops::summarize_mcp_servers_from_snapshots(&config, &snapshots)
        } else {
            mcp_ops::summarize_mcp_servers_without_discovery(&config)
        };
        let rows = Self::mcp_rows_from_summaries(summaries);
        let theme = self.tui.chrome().theme();
        self.tui.chrome_mut().open_mcp_manager(&McpManagerOptions {
            servers: rows,
            theme,
        });
    }

    pub(super) async fn sync_mcp_manager_from_config(&mut self) {
        let Some(config) = self.local_config.clone() else {
            return;
        };
        let Some(manager) = self.mcp_manager.clone() else {
            return;
        };
        if let Err(error) = mcp_ops::reload_mcp_manager_from_config(&config, &manager).await {
            self.push_status(format!("MCP manager sync failed: {error}"));
        }
    }

    fn mcp_rows_from_summaries(summaries: Vec<mcp_ops::McpServerSummary>) -> Vec<McpServerRow> {
        summaries
            .into_iter()
            .map(|summary| McpServerRow {
                id: summary.id,
                transport_label: summary.transport_label,
                enabled: summary.enabled,
                endpoint_summary: summary.endpoint_summary,
                cwd_summary: summary.cwd.map(|p| p.to_string_lossy().into_owned()),
                env_keys: summary.env_keys,
                header_keys: summary.header_keys,
                tool_status: match summary.tools {
                    mcp_ops::McpToolDiscovery::SkippedDisabled => McpToolStatus::SkippedDisabled,
                    mcp_ops::McpToolDiscovery::NotRequested => McpToolStatus::NotDiscovered,
                    mcp_ops::McpToolDiscovery::Success(names) => McpToolStatus::Discovered(names),
                    mcp_ops::McpToolDiscovery::Failed(reason) => McpToolStatus::Failed(reason),
                },
            })
            .collect()
    }

    pub(super) async fn handle_mcp_manager_action(&mut self) {
        let action = self.tui.chrome_mut().take_mcp_manager_action();
        let Some(action) = action else {
            return;
        };
        match action {
            McpManagerAction::Close => {
                self.tui.chrome_mut().close_focused_overlay();
            }
            McpManagerAction::Add => {
                self.tui.chrome_mut().close_focused_overlay();
                self.open_add_mcp_transport_picker();
            }
            McpManagerAction::Test(id) => {
                self.start_mcp_probe(&id, true);
            }
            McpManagerAction::Refresh(id) => {
                self.start_mcp_probe(&id, false);
            }
            McpManagerAction::ToggleEnabled(id) => {
                self.toggle_mcp_server_enabled(&id).await;
            }
            McpManagerAction::Delete(id) => {
                self.delete_mcp_server(&id).await;
            }
            McpManagerAction::Auth(id) => {
                self.start_mcp_oauth_flow(id).await;
            }
        }
    }

    async fn start_mcp_oauth_flow(&mut self, server_id: String) {
        let Some(config) = self.local_config.clone() else {
            self.push_status("No config available");
            return;
        };
        let Some(server) = config.mcp.servers.iter().find(|s| s.id == server_id) else {
            self.push_status("MCP server not found");
            return;
        };
        if server.transport != crate::config::McpTransport::Http
            && server.transport != crate::config::McpTransport::Sse
        {
            self.push_status("OAuth is limited to HTTP/SSE servers");
            return;
        }

        let Some(neo_home) = neo_home() else {
            self.push_status("Failed to resolve neo home directory");
            return;
        };

        self.push_status("Waiting for browser authorization...");
        match authenticate_mcp_server_oauth(&server_id, server, &neo_home).await {
            Ok(_) => {
                self.push_status("OAuth token saved");
                // Automatically sync the manager with the new credentials and
                // probe the server so tool discovery happens without the user
                // having to manually press Enter (Test).
                self.sync_mcp_manager_from_config().await;
                self.start_mcp_probe(&server_id, true);
            }
            Err(err) => {
                self.push_status(format!("OAuth flow failed: {err}"));
            }
        }
    }

    fn open_add_mcp_transport_picker(&mut self) {
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(ChoicePickerOptions {
                title: "Add MCP Server".to_owned(),
                items: vec![
                    ChoiceItem::new("mcp:add:stdio", "Local stdio (studio)")
                        .with_description("Run a command on this machine"),
                    ChoiceItem::new("mcp:add:http", "Remote HTTP")
                        .with_description("JSON-RPC HTTP endpoint"),
                    ChoiceItem::new("mcp:add:sse", "Remote SSE")
                        .with_description("JSON-RPC endpoint over SSE"),
                ],
                initial_id: None,
                theme,
                page_size: 0,
                current_id: None,
            });
    }

    pub(super) fn handle_mcp_choice_item(&mut self, id: &str) -> bool {
        let transport = match id {
            "mcp:add:stdio" => "stdio",
            "mcp:add:http" => "http",
            "mcp:add:sse" => "sse",
            _ => return false,
        };
        self.pending_mcp_add_transport = Some(transport);
        let title = match transport {
            "stdio" => "Add Local stdio MCP Server",
            "http" => "Add Remote HTTP MCP Server",
            "sse" => "Add Remote SSE MCP Server",
            _ => "Add MCP Server",
        };
        self.tui
            .chrome_mut()
            .open_mcp_add_form(McpAddFormOptions {
                title: title.to_owned(),
                transport: transport.to_owned(),
            });
        true
    }

    pub(super) async fn handle_mcp_add_form_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().mcp_add_form_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        let transport = self.pending_mcp_add_transport.take().unwrap_or("stdio");
        match result {
            McpAddFormResult::Submitted(data) => {
                self.save_mcp_form_server(data, transport).await;
            }
            McpAddFormResult::Cancelled => {
                // The add-form overlay was just closed; reopen the MCP manager
                // so the user returns to the server list (updates in-place if
                // an overlay is already focused).
                self.open_mcp_manager().await;
            }
        }
    }

    async fn save_mcp_form_server(&mut self, data: McpAddFormData, transport: &'static str) {
        let cli_type = match transport {
            "stdio" => "studio",
            "http" => "remote-http",
            "sse" => "remote-sse",
            _ => transport,
        };
        let mut headers = data.headers;
        if let Some(token) = data.bearer_token {
            headers.push(format!("Authorization=Bearer {token}"));
        }
        let input = mcp_ops::AddMcpServerInput {
            id: data.name,
            cli_type: cli_type.to_owned(),
            command: data.command,
            url: data.url,
            env: data.env,
            headers,
            cwd: None,
            enabled_tools: vec![],
            disabled_tools: vec![],
            startup_timeout_ms: None,
            tool_timeout_ms: None,
            enabled: true,
        };
        let config = match mcp_ops::build_mcp_server_config(input) {
            Ok(config) => config,
            Err(err) => {
                self.push_status(format!("Invalid MCP server: {err}"));
                return;
            }
        };
        let Some(config_path) = self.config_path() else {
            return;
        };
        if let Err(err) = config::mutations::upsert_mcp_server(&config, &config_path) {
            self.push_status(format!("Failed to save MCP server: {err}"));
            return;
        }
        self.push_status(format!("MCP server {} saved", config.id));
        self.refresh_config();
        self.sync_mcp_manager_from_config().await;
        // Reopen the MCP manager overlay to show the newly saved server. With
        // the chrome fix this updates the existing overlay in-place rather
        // than pushing a duplicate layer.
        self.open_mcp_manager().await;
    }

    pub(super) fn start_mcp_probe(&mut self, id: &str, reconnect: bool) {
        let Some(manager) = self.mcp_manager.clone() else {
            self.push_status("MCP manager unavailable");
            return;
        };
        self.tui
            .chrome_mut()
            .set_custom_working_label(Some(format!("Testing MCP server {id}...")));
        let id = id.to_owned();
        let probe_id = id.clone();
        let handle = tokio::spawn(async move {
            if reconnect {
                manager.reconnect_now(&probe_id).await
            } else {
                manager.refresh_tools(&probe_id).await
            }
        });
        self.pending_mcp_probe = Some(PendingMcpProbe {
            server_id: id,
            handle,
        });
    }

    pub(super) async fn poll_pending_mcp_probe(&mut self) {
        let Some(pending) = self.pending_mcp_probe.take() else {
            return;
        };
        if !pending.handle.is_finished() {
            self.pending_mcp_probe = Some(pending);
            return;
        }
        self.tui.chrome_mut().set_custom_working_label(None);
        match pending.handle.await {
            Ok(Ok(snapshot)) => {
                self.push_status(format!(
                    "MCP {} connected ({} tools)",
                    pending.server_id, snapshot.tool_count
                ));
            }
            Ok(Err(err)) => {
                self.push_status(format!("MCP {} connect failed: {err}", pending.server_id));
            }
            Err(join_err) => {
                self.push_status(format!(
                    "MCP {} probe panicked: {join_err}",
                    pending.server_id
                ));
            }
        }
        // Refresh the MCP manager overlay to reflect the probe results.
        // Updates the existing overlay in-place rather than stacking a new one.
        self.open_mcp_manager().await;
    }

    async fn toggle_mcp_server_enabled(&mut self, id: &str) {
        let Some(config) = self.local_config.clone() else {
            return;
        };
        let Some(server) = config.mcp.servers.iter().find(|s| s.id == id) else {
            return;
        };
        let new_enabled = !server.enabled;
        let Some(config_path) = self.config_path() else {
            return;
        };
        if let Err(err) = config::mutations::set_mcp_server_enabled(id, new_enabled, &config_path) {
            self.push_status(format!("Failed to update MCP server: {err}"));
            return;
        }
        self.push_status(format!(
            "MCP server {id} {}",
            if new_enabled { "enabled" } else { "disabled" }
        ));
        self.refresh_config();
        self.sync_mcp_manager_from_config().await;
        // Refresh the MCP manager overlay to reflect the new enabled/disabled
        // state. Updates the existing overlay in-place rather than stacking.
        self.open_mcp_manager().await;
    }

    async fn delete_mcp_server(&mut self, id: &str) {
        let Some(config_path) = self.config_path() else {
            return;
        };
        if let Err(err) = config::mutations::remove_mcp_server(id, &config_path) {
            self.push_status(format!("Failed to remove MCP server: {err}"));
            return;
        }
        self.push_status(format!("MCP server {id} removed"));
        self.refresh_config();
        self.sync_mcp_manager_from_config().await;
        // Refresh the MCP manager overlay so the deleted server disappears.
        // Updates the existing overlay in-place rather than stacking.
        self.open_mcp_manager().await;
    }
}
