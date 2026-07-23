//! Extracted: command palette specs, open/close, and command dispatch.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use neo_agent_core::PermissionMode;
use neo_tui::shell::{CommandSpec, PromptEdit};

use crate::prompt::templates::load_project_prompt_templates;

use super::InteractiveController;

fn command_specs(project_dir: &Path, project_trusted: bool) -> (Vec<CommandSpec>, Option<String>) {
    let mut commands = vec![
        CommandSpec::new("sessions", "Open sessions", Some("Browse local sessions")),
        CommandSpec::new("models", "Open models", Some("Switch active model")),
        CommandSpec::new(
            "providers",
            "Open providers",
            Some("View configured providers"),
        ),
        CommandSpec::new("mcp", "Open MCP servers", Some("Manage MCP servers")),
        CommandSpec::new(
            "add-workspace",
            "Open workspace access",
            Some("Manage additional workspace directories"),
        ),
        CommandSpec::new(
            "session.new",
            "New session",
            Some("Start a fresh local session"),
        ),
        CommandSpec::new(
            "session.exportHtml",
            "Export session to HTML",
            Some("Write the active local session as sanitized HTML"),
        ),
        CommandSpec::new(
            "fork",
            "Fork session",
            Some("Create a child fork of the current session"),
        ),
        CommandSpec::new(
            "copy-prompt",
            "Copy prompt",
            Some("Copy current prompt text"),
        ),
        CommandSpec::new(
            "select-transcript",
            "Select transcript item",
            Some("Start transcript item selection"),
        ),
        CommandSpec::new(
            "copy-transcript-selection",
            "Copy transcript selection",
            Some("Copy selected transcript items"),
        ),
        CommandSpec::new(
            "clear-transcript-selection",
            "Clear transcript selection",
            Some("Remove transcript selection"),
        ),
        CommandSpec::new("submit", "Submit prompt", Some("Submit the current prompt")),
        CommandSpec::new(
            "permissions",
            "Open permissions",
            Some("Select permission mode"),
        ),
        CommandSpec::new(
            "permission.ask",
            "Ask permission mode",
            Some("Ask before risky actions"),
        ),
        CommandSpec::new(
            "permission.auto",
            "Auto permission mode",
            Some("Run non-interactively"),
        ),
        CommandSpec::new(
            "permission.yolo",
            "YOLO permission mode",
            Some("Skip confirmations"),
        ),
        CommandSpec::new(
            "plan",
            "Toggle plan mode",
            Some("Read-only mode for investigation and planning"),
        ),
        CommandSpec::new(
            "btw",
            "/btw sidecar",
            Some("Open a temporary side-question panel"),
        ),
    ];
    let mut templates = match load_project_prompt_templates(project_dir, project_trusted) {
        Ok(templates) => templates,
        Err(error) => return (commands, Some(error.to_string())),
    };
    templates.sort_by(|left, right| left.name.cmp(&right.name));
    commands.extend(templates.into_iter().map(|template| {
        let label = format!("/{}", template.name);
        let description = (!template.description.is_empty()).then_some(template.description);
        CommandSpec::new(
            format!("prompt-template.{}", template.name),
            label,
            description,
        )
    }));
    (commands, None)
}

impl InteractiveController {
    pub(super) fn open_command_palette(&mut self) {
        let (commands, error) = command_specs(&self.completion_root, self.project_trusted());
        if let Some(error) = error {
            self.push_status(format!("Error loading prompt templates: {error}"));
        }
        self.tui.chrome_mut().open_command_palette(commands);
    }

    pub(super) async fn run_selected_command(&mut self) -> Result<()> {
        let Some(command) = self.tui.chrome_mut().confirm_command_palette() else {
            return Ok(());
        };
        if self.run_prompt_template_command(&command.id) {
            return Ok(());
        }
        if self.run_sync_command(&command.id).await {
            return Ok(());
        }
        self.run_async_command(&command.id).await
    }

    fn run_prompt_template_command(&mut self, command_id: &str) -> bool {
        let Some(name) = command_id.strip_prefix("prompt-template.") else {
            return false;
        };
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit(PromptEdit::Insert(&format!("/{name} ")));
        true
    }

    async fn run_sync_command(&mut self, command_id: &str) -> bool {
        if self.run_picker_command(command_id).await {
            return true;
        }
        self.run_transcript_command(command_id)
            || self.run_permission_command(command_id)
            || self.run_plan_command(command_id)
    }

    async fn run_picker_command(&mut self, command_id: &str) -> bool {
        if self.run_open_picker_command(command_id).await {
            return true;
        }
        self.run_copy_prompt_command(command_id)
    }

    async fn run_open_picker_command(&mut self, command_id: &str) -> bool {
        match command_id {
            "sessions" => self.open_session_picker(),
            "models" => self.open_model_picker(),
            "providers" => self.open_provider_picker(),
            "mcp" => self.open_mcp_manager().await,
            "add-workspace" => self.open_workspace_manager(),
            _ => return false,
        }
        true
    }

    fn run_copy_prompt_command(&mut self, command_id: &str) -> bool {
        if command_id != "copy-prompt" {
            return false;
        }
        self.copy_prompt_to_clipboard();
        true
    }

    fn run_transcript_command(&mut self, command_id: &str) -> bool {
        match command_id {
            "select-transcript" => self.transcript_mut().select_visible_transcript_entry(),
            "clear-transcript-selection" => self.transcript_mut().clear_transcript_selection(),
            "copy-transcript-selection" => self.copy_transcript_selection_to_clipboard(),
            _ => return false,
        }
        true
    }

    fn run_permission_command(&mut self, command_id: &str) -> bool {
        match command_id {
            "permissions" => self.open_permission_picker(),
            "permission.ask" => self.set_permission_mode(PermissionMode::Ask),
            "permission.auto" => self.set_permission_mode(PermissionMode::Auto),
            "permission.yolo" => self.set_permission_mode(PermissionMode::Yolo),
            _ => return false,
        }
        true
    }

    fn run_plan_command(&mut self, command_id: &str) -> bool {
        if command_id != "plan" {
            return false;
        }
        let currently_active = self.tui.chrome_mut().is_plan_mode();
        self.set_plan_mode_from_user(!currently_active);
        true
    }

    async fn run_async_command(&mut self, command_id: &str) -> Result<()> {
        if self.run_session_async_command(command_id).await? {
            return Ok(());
        }
        if command_id == "submit" {
            self.submit_current_prompt().await?;
            return Ok(());
        }
        if command_id == "btw" {
            self.open_btw_panel(None).await;
            return Ok(());
        }
        self.push_status(format!("Unknown command: {command_id}"));
        Ok(())
    }

    async fn run_session_async_command(&mut self, command_id: &str) -> Result<bool> {
        match command_id {
            "session.exportHtml" => self.export_active_session_to_html().await?,
            "session.new" => self.start_new_session_from_slash(),
            "fork" => self.fork_current_session().await?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    async fn export_active_session_to_html(&mut self) -> Result<()> {
        let Some(session_id) = self.active_session_id.clone() else {
            self.push_status("No active session to export");
            return Ok(());
        };
        let config = self
            .local_config
            .clone()
            .context("session HTML export is unavailable")?;
        let html = crate::modes::sessions::export_html(&session_id, &config).await?;
        let output_path =
            crate::modes::sessions::session_path(&session_id, &config)?.with_extension("html");
        fs::write(&output_path, html)
            .with_context(|| format!("failed to write {}", output_path.display()))?;
        self.push_status(format!(
            "Exported session {session_id} to {}",
            output_path.display()
        ));
        Ok(())
    }
}
