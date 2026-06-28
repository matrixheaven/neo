//! Extracted: slash command parsing and dispatch (`/model`, `/plan`, `/skill:*`, etc.).

use std::time::Instant;

use anyhow::{Context, Result};

use super::task_browser;
use super::InteractiveController;
use super::{slash_arg, slash_permission_mode, split_skill_invocation, expand_slash_skill, skill_invocation_args};

impl InteractiveController {
    /// Handle slash commands. Returns `true` if the prompt was consumed and should
    /// not be submitted as a chat turn.
    pub(super) async fn handle_slash_command(&mut self, prompt: &str) -> bool {
        let prompt = prompt.trim();
        if let Some(arg) = slash_arg(prompt, "/btw") {
            self.clear_submitted_prompt();
            self.open_btw_panel(if arg.is_empty() {
                None
            } else {
                Some(arg.to_owned())
            })
            .await;
            return true;
        }
        if self.handle_simple_slash_command(prompt).await {
            return true;
        }
        if self.handle_model_or_skill_slash_command(prompt) {
            return true;
        }
        if self.handle_permission_slash_command(prompt) {
            return true;
        }
        if self.handle_plan_slash_prefix(prompt) {
            return true;
        }
        self.handle_goal_slash_prefix(prompt).await
    }

    pub(super) async fn handle_simple_slash_command(&mut self, prompt: &str) -> bool {
        match prompt {
            "/new" | "/clear" => {
                let blocked = self.active_turn.is_some();
                self.start_new_session_from_slash();
                if blocked {
                    // Preserve the command text so the user can retry after
                    // interrupting the running turn.
                    return true;
                }
            }
            "/resume" => self.open_session_picker(),
            "/provider" => self.open_provider_picker(),
            "/mcp" => self.open_mcp_manager().await,
            "/tasks" => self.show_background_tasks().await,
            "/compact" => {
                let instruction = slash_arg(prompt, "/compact").map(|arg| {
                    if arg.is_empty() {
                        None
                    } else {
                        Some(arg.to_owned())
                    }
                });
                self.request_manual_compaction(instruction.flatten());
            }
            _ => return false,
        }
        self.clear_submitted_prompt();
        true
    }

    pub(super) async fn show_background_tasks(&mut self) {
        let Some(config) = self.local_config.as_ref() else {
            self.push_status("No config available");
            return;
        };
        let tasks = config.background_tasks.list(false, 50).await;
        let snapshot = task_browser::snapshots_to_browser_snapshot(&tasks);
        let mut state = self
            .tui
            .chrome()
            .task_browser_state()
            .cloned()
            .unwrap_or_default();
        state.apply_snapshot(&snapshot);
        self.last_task_browser_refresh = Some(Instant::now());
        self.tui.chrome_mut().push_task_browser_overlay(state);
    }

    fn handle_model_or_skill_slash_command(&mut self, prompt: &str) -> bool {
        if let Some(alias) = slash_arg(prompt, "/model") {
            self.handle_model_slash_command(alias);
            return true;
        }
        if let Some(arg) = prompt.strip_prefix("/skill:").map(str::trim) {
            self.handle_skill_slash_command(arg);
            return true;
        }
        false
    }

    pub(super) fn handle_permission_slash_command(&mut self, prompt: &str) -> bool {
        if let Some(mode) = slash_permission_mode(prompt) {
            self.clear_submitted_prompt();
            self.set_permission_mode(mode);
            return true;
        }
        if matches!(prompt, "/permissions" | "/permission") {
            self.clear_submitted_prompt();
            self.open_permission_picker();
            return true;
        }
        false
    }

    fn handle_plan_slash_prefix(&mut self, prompt: &str) -> bool {
        let Some(arg) = slash_arg(prompt, "/plan") else {
            return false;
        };
        self.handle_plan_slash_command(arg);
        true
    }

    async fn handle_goal_slash_prefix(&mut self, prompt: &str) -> bool {
        let Some(arg) = slash_arg(prompt, "/goal") else {
            return false;
        };
        self.clear_submitted_prompt();
        self.handle_goal_command(arg).await
    }

    pub(super) fn clear_submitted_prompt(&mut self) {
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
    }

    fn handle_model_slash_command(&mut self, alias: &str) {
        self.clear_submitted_prompt();
        if alias.is_empty() {
            self.open_model_picker();
        } else if self.model_items.iter().any(|item| item.value == alias) {
            self.open_model_picker_with_alias(alias);
        } else {
            self.push_status(format!("Error: Unknown model alias: {alias}"));
        }
    }

    fn handle_skill_slash_command(&mut self, arg: &str) {
        self.clear_submitted_prompt();
        if arg.is_empty() {
            self.push_status("Usage: /skill:<name> [args]");
        } else if let Err(err) = self.handle_skill_invocation(arg) {
            self.push_status(format!("Skill error: {err}"));
        }
    }

    fn handle_plan_slash_command(&mut self, arg: &str) {
        self.clear_submitted_prompt();
        if self.handle_plan_toggle_argument(arg) {
            return;
        }
        self.handle_plan_file_argument(arg);
    }

    fn handle_plan_toggle_argument(&mut self, arg: &str) -> bool {
        match arg {
            "" => self.toggle_plan_mode_from_user(),
            "on" => self.set_plan_mode_from_user(true),
            "off" => self.set_plan_mode_from_user(false),
            _ => return false,
        }
        true
    }

    fn handle_plan_file_argument(&mut self, arg: &str) {
        if arg == "clear" {
            self.clear_plan_file();
        } else {
            self.push_unknown_plan_argument(arg);
        }
    }

    fn handle_skill_invocation(&mut self, arg: &str) -> Result<()> {
        let skill_store = self
            .skill_store
            .as_ref()
            .context("skill store not loaded")?;
        let (name, args_str) = split_skill_invocation(arg);
        let skill = skill_store
            .get(name)
            .with_context(|| format!("skill `{name}` not found"))?;
        let description = skill.manifest.description.clone();
        let (expanded, raw_arguments) = expand_slash_skill(name, args_str, skill)?;
        self.push_skill_invocation_entry(name, description, &raw_arguments);
        self.pending_skill_context = Some(expanded);
        self.replace_prompt_text(args_str);
        Ok(())
    }

    fn push_skill_invocation_entry(
        &mut self,
        name: &str,
        description: String,
        raw_arguments: &str,
    ) {
        let args = skill_invocation_args(raw_arguments);
        self.transcript_mut().push_transcript(
            neo_tui::transcript::TranscriptEntry::skill_activated(name, Some(description), args),
        );
    }

    fn replace_prompt_text(&mut self, text: &str) {
        let prompt = self.tui.chrome_mut().prompt_mut();
        text.clone_into(&mut prompt.text);
        prompt.cursor = prompt.text.chars().count();
    }
}
