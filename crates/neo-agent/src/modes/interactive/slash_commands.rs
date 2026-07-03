//! Extracted: slash command parsing and dispatch (`/model`, `/plan`, `/skill:*`, etc.).

use std::time::Instant;

use anyhow::{Context, Result};
use neo_tui::dialogs::HelpPanelCommand;

use super::InteractiveController;
use super::task_browser;
use super::{
    InlineSkillDirectives, InlineSkillInvocation, expand_slash_skill,
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
            "/help" => self.open_help_panel(),
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

    fn handle_model_or_skill_slash_command(&mut self, prompt: &str) -> bool {
        if let Some(alias) = slash_arg(prompt, "/model") {
            self.handle_model_slash_command(alias);
            return true;
        }
        if let Some(directives) = parse_inline_skill_directives(prompt) {
            self.handle_skill_slash_command(directives);
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
        if self.handle_goal_command(arg).await {
            self.clear_submitted_prompt();
            return true;
        }
        self.replace_prompt_text(&goal_submission_text(arg));
        false
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

    fn handle_skill_slash_command(&mut self, directives: InlineSkillDirectives) {
        if directives
            .invocations
            .iter()
            .any(|invocation| invocation.name.is_empty())
        {
            self.push_status("Usage: /skill:<name> [args]");
        } else {
            match self.activate_skill_directives(directives) {
                Ok(_) => self.clear_submitted_prompt(),
                Err(err) => self.push_status(format!("Skill error: {err}")),
            }
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

    pub(super) fn activate_skill_directives(
        &mut self,
        directives: InlineSkillDirectives,
    ) -> Result<String> {
        let skill_store = self
            .skill_store
            .as_ref()
            .context("skill store not loaded")?;
        let mut names = Vec::new();
        let mut loaded_blocks = Vec::new();
        for invocation in &directives.invocations {
            let skill = skill_store
                .get(&invocation.name)
                .with_context(|| format!("skill `{}` not found", invocation.name))?;
            let (expanded_skill, _) =
                expand_slash_skill(&invocation.name, &invocation.args, skill)?;
            names.push(invocation.name.clone());
            loaded_blocks.push(render_loaded_skill_block(
                skill,
                invocation.args.as_str(),
                expanded_skill.as_str(),
            ));
        }

        self.push_skill_invocation_entry(names, directives.body.as_str());
        self.pending_skill_context = Some(render_user_slash_skill_context(
            &directives.invocations,
            &loaded_blocks,
            directives.body.as_str(),
        ));
        Ok(directives.body)
    }

    fn push_skill_invocation_entry(&mut self, names: Vec<String>, body: &str) {
        self.transcript_mut().push_transcript(
            neo_tui::transcript::TranscriptEntry::skill_activated(names, body),
        );
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

fn render_loaded_skill_block(
    skill: &neo_agent_core::skills::LoadedSkill,
    args: &str,
    body: &str,
) -> String {
    format!(
        "<neo-skill-loaded name=\"{}\" trigger=\"user-slash\" source=\"{}\" dir=\"{}\" args=\"{}\">\n{}\n</neo-skill-loaded>",
        escape_xml_attr(&skill.name),
        escape_xml_attr(skill_source_label(skill.source)),
        escape_xml_attr(&skill.root.to_string_lossy()),
        escape_xml_attr(args),
        body
    )
}

const fn skill_source_label(source: neo_agent_core::skills::SkillSource) -> &'static str {
    match source {
        neo_agent_core::skills::SkillSource::Builtin => "builtin",
        neo_agent_core::skills::SkillSource::Extra => "extra",
        neo_agent_core::skills::SkillSource::User => "user",
    }
}

fn escape_xml_attr(text: &str) -> String {
    escape_xml_text(text).replace('"', "&quot;")
}

fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
