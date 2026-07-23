//! Extracted: slash command parsing and dispatch (`/model`, `/plan`, `/skill:*`, etc.).

use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result};
use neo_tui::dialogs::HelpPanelCommand;

use super::InteractiveController;
use super::task_browser;
use super::{
    InlineSkillDirectives, InlineSkillInvocation, content_to_display_text, expand_slash_skill,
    parse_inline_skill_directives, slash_arg, slash_permission_mode,
};

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
        if let Some(instruction) = super::init_command::init_instruction(prompt) {
            let instruction = instruction.to_owned();
            self.clear_submitted_prompt();
            if self.permission_mode == super::PermissionMode::Auto {
                self.open_interactive_preflight(
                    super::interactive_preflight::init_preflight(),
                    super::PendingInteractiveWorkflow::Init { instruction },
                );
                return true;
            }
            if let Err(error) = self.run_init_workflow(&instruction, false).await {
                self.push_status(format!("Failed to start /init: {error}"));
            }
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
            "/resume" | "/sessions" => self.open_session_picker(),
            "/fork" => {
                if let Err(error) = self.fork_current_session().await {
                    self.push_status(format!("Failed to fork session: {error}"));
                }
            }
            "/provider" => self.open_provider_picker(),
            "/help" => self.open_help_panel(),
            "/mcp" => self.open_mcp_manager().await,
            "/add-workspace" => self.open_workspace_manager(),
            "/tasks" => self.show_background_tasks().await,
            "/workflow" => {
                self.workflow_capability.grant();
                self.push_status(
                    "Workflow launch capability granted. Call RunWorkflow to use it.".to_owned(),
                );
            }
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

    fn open_help_panel(&mut self) {
        let commands = super::session_completion_items(self.skill_store.as_ref())
            .into_iter()
            .map(|item| HelpPanelCommand::new(item.value, item.description))
            .collect();
        self.tui.chrome_mut().open_help_panel(commands);
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

    pub(super) fn start_init_workflow(
        &mut self,
        instruction: &str,
        auto_mode_best_effort: bool,
    ) -> Result<()> {
        let current_date = chrono::Local::now().date_naive().to_string();
        let source_commit = current_git_commit();
        let workflow_prompt = super::init_command::build_init_workflow_prompt(
            super::init_command::InitPromptRequest {
                workspace_root: &self.completion_root,
                current_date: &current_date,
                source_commit: source_commit.as_deref(),
                instruction: (!instruction.is_empty()).then_some(instruction),
                auto_mode_best_effort,
            },
        );
        let prompt = super::init_command::wrap_init_system_reminder(&workflow_prompt);
        self.start_generated_injection_turn_from_text(prompt, "init", "/init AGENTS.md workflow")
    }

    pub(super) async fn run_init_workflow(
        &mut self,
        instruction: &str,
        auto_mode_best_effort: bool,
    ) -> Result<()> {
        self.start_init_workflow(instruction, auto_mode_best_effort)?;
        self.wait_for_active_turn().await?;
        self.repair_agents_guide_once_if_needed().await?;
        self.start_pending_background_question_followups().await
    }

    async fn repair_agents_guide_once_if_needed(&mut self) -> Result<()> {
        let path = self.workspace_root.join("AGENTS.md");
        let Ok(markdown) = tokio::fs::read_to_string(&path).await else {
            return Ok(());
        };
        let issues = super::init_command::validate_agents_guide(&markdown);
        if issues.is_empty() {
            self.push_status("AGENTS.md structure validation passed");
            return Ok(());
        }

        let repair_prompt = super::init_command::build_agents_guide_repair_prompt(&issues);
        let reminder = super::init_command::wrap_init_system_reminder(&repair_prompt);
        self.start_generated_injection_turn_from_text(reminder, "init", "/init AGENTS.md repair")?;
        self.wait_for_active_turn().await?;

        let Ok(repaired_markdown) = tokio::fs::read_to_string(&path).await else {
            self.push_status("AGENTS.md repair finished, but file could not be re-read");
            return Ok(());
        };
        let remaining = super::init_command::validate_agents_guide(&repaired_markdown);
        if remaining.is_empty() {
            self.push_status("AGENTS.md structure validation passed after repair");
        } else {
            self.push_status(format!(
                "AGENTS.md still has {} structure validation issue(s)",
                remaining.len()
            ));
        }
        Ok(())
    }

    pub(super) fn open_interactive_preflight(
        &mut self,
        spec: super::InteractivePreflightSpec,
        pending: super::PendingInteractiveWorkflow,
    ) {
        let items = spec.choice_items();
        let page_size = items.len();
        let initial_id = spec.initial_id();
        let title = spec.title.clone();
        self.pending_interactive_workflow = Some(pending);
        self.pending_preflight = Some(spec);
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
                title,
                items,
                initial_id: Some(initial_id),
                theme,
                page_size,
                current_id: None,
            });
    }

    fn handle_model_or_skill_slash_command(&mut self, prompt: &str) -> bool {
        if let Some(alias) = slash_arg(prompt, "/model") {
            self.handle_model_slash_command(alias);
            return true;
        }
        if let Some(directives) = parse_inline_skill_directives(prompt) {
            return self.handle_skill_slash_command(directives);
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
        if self.handle_goal_command(arg).await {
            self.clear_submitted_prompt();
            return true;
        }
        self.replace_prompt_text(&goal_submission_text(arg));
        false
    }

    pub(super) fn clear_submitted_prompt(&mut self) {
        self.slash_completion_catalog = None;
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

    fn handle_skill_slash_command(&mut self, directives: InlineSkillDirectives) -> bool {
        match super::interactive_preflight::skill_preflight_decision(
            &directives,
            self.permission_mode,
        ) {
            super::interactive_preflight::SkillPreflightDecision::Ready => {}
            super::interactive_preflight::SkillPreflightDecision::InvalidUsage => {
                self.push_status("Usage: /skill:<name> [args]");
                return true;
            }
            super::interactive_preflight::SkillPreflightDecision::Open {
                spec,
                generated_prompt,
            } => {
                self.clear_submitted_prompt();
                self.open_interactive_preflight(
                    *spec,
                    super::PendingInteractiveWorkflow::Skill {
                        directives,
                        generated_prompt,
                    },
                );
                return true;
            }
            super::interactive_preflight::SkillPreflightDecision::Blocked(message) => {
                self.clear_submitted_prompt();
                self.push_status(message);
                return true;
            }
        }
        match self.activate_skill_directives(directives) {
            Ok(_) => self.clear_submitted_prompt(),
            Err(err) => self.push_status(format!("Skill error: {err}")),
        }
        true
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

    /// Activate inline skill directives.
    ///
    /// Returns `(raw_stripped_body, expanded_display_body)`:
    /// - `raw_stripped_body`: the prompt with `/skill:` syntax removed, still containing
    ///   `[paste ...]` / `[image ...]` markers. Used for turn submission and skill context.
    /// - `expanded_display_body`: the same text with markers expanded, used for the
    ///   `SkillActivation` transcript card and for suppressing the runtime user-message echo.
    pub(super) fn activate_skill_directives(
        &mut self,
        directives: InlineSkillDirectives,
    ) -> Result<(String, String)> {
        self.refresh_skill_store_for_completion();
        let skill_store = self
            .skill_store
            .as_ref()
            .context("skill store not loaded")?;
        let mut names = Vec::with_capacity(directives.invocations.len());
        let mut loaded_blocks = Vec::with_capacity(directives.invocations.len());
        for invocation in &directives.invocations {
            let skill = skill_store
                .get(&invocation.name)
                .with_context(|| format!("skill `{}` not found", invocation.name))?;
            let (expanded_skill, _) =
                expand_slash_skill(&invocation.name, &invocation.args, skill)?;
            names.push(invocation.name.clone());
            loaded_blocks.push(neo_agent_core::skills::render_skill_context(
                skill,
                &expanded_skill,
            ));
        }

        let expanded_content = crate::prompt::parts::expand_prompt_markers(
            &directives.body,
            &self.paste_store,
            &self.image_attachment_store,
            &self.file_reference_store,
            &self.completion_root,
        );
        let display_body = content_to_display_text(&expanded_content);

        self.push_skill_invocation_entry(names, &display_body);
        self.pending_skill_context = Some(render_user_slash_skill_context(
            &directives.invocations,
            &loaded_blocks,
            directives.body.as_str(),
        ));
        Ok((directives.body, display_body))
    }

    fn push_skill_invocation_entry(&mut self, names: Vec<String>, body: &str) {
        self.transcript_mut()
            .apply_agent_event(neo_agent_core::AgentEvent::SkillInvocation {
                names,
                source: neo_agent_core::SkillInvocationSource::Manual,
                outcome: neo_agent_core::SkillInvocationOutcome::Activated,
                body: body.to_owned(),
            });
    }

    fn replace_prompt_text(&mut self, text: &str) {
        let prompt = self.tui.chrome_mut().prompt_mut();
        text.clone_into(&mut prompt.text);
        prompt.cursor = prompt.text.chars().count();
    }
}

fn goal_submission_text(arg: &str) -> String {
    let command = arg.trim();
    if let Some(objective) = command.strip_prefix("replace ") {
        return strip_goal_separator(objective).to_owned();
    }
    if let Some(objective) = command.strip_prefix("next ") {
        return strip_goal_separator(objective).to_owned();
    }
    strip_goal_separator(command).to_owned()
}

fn strip_goal_separator(text: &str) -> &str {
    text.trim()
        .strip_prefix("--")
        .map_or(text.trim(), str::trim)
}

fn current_git_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8(output.stdout).ok()?;
    let commit = commit.trim();
    if commit.is_empty() {
        None
    } else {
        Some(commit.to_owned())
    }
}

fn render_user_slash_skill_context(
    invocations: &[InlineSkillInvocation],
    loaded_blocks: &[String],
    body: &str,
) -> String {
    let names = invocations
        .iter()
        .map(|invocation| format!("\"{}\"", escape_xml_text(&invocation.name)))
        .collect::<Vec<_>>()
        .join(", ");
    let label = if invocations.len() == 1 {
        format!("the skill {names}")
    } else {
        format!("the skills {names}")
    };
    let mut context = format!(
        "User activated {label}. Follow the loaded skill instructions for this request.\n\n{}",
        loaded_blocks.join("\n\n")
    );
    if !body.trim().is_empty() {
        context.push_str("\n\nUser request after removing /skill control syntax:\n");
        context.push_str("<neo-user-request>\n");
        context.push_str(body);
        context.push_str("\n</neo-user-request>");
    }
    context
}

fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
