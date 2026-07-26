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
        // Strict slash_arg boundary: `/workflow` and `/workflow <args>` only.
        // `/workflowish` does not match and must not grant capability.
        if let Some(arg) = slash_arg(prompt, "/workflow") {
            self.clear_submitted_prompt();
            self.handle_workflow_slash(arg).await;
            return true;
        }
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

    /// Bare `/workflow` grants one-shot dynamic capability. Named
    /// `/workflow <name> [JSON_OBJECT]` resolves the registry and launches via
    /// the shared coordinator with zero model turns.
    async fn handle_workflow_slash(&mut self, arg: &str) {
        if arg.is_empty() {
            self.workflow_capability.grant();
            self.push_status(
                "Workflow launch capability granted. Call RunWorkflow to use it.".to_owned(),
            );
            return;
        }
        if let Err(error) = self.launch_named_workflow_slash(arg).await {
            self.push_status(format!("Workflow launch failed: {error}"));
        }
    }

    async fn launch_named_workflow_slash(&mut self, arg: &str) -> Result<(), String> {
        let (name, args) = parse_named_workflow_slash_args(arg)?;
        let Some(config) = self.local_config.clone() else {
            return Err("No config available".to_owned());
        };

        let definition = config
            .workflow_definitions
            .resolve(&name)
            .map_err(|error| error.to_string())?;

        if let Some(schema) = &definition.compiled_input_schema {
            schema
                .validate_instance(&args)
                .map_err(|error| format!("args failed input_schema validation: {error}"))?;
        }

        self.ensure_shell_session_path(&config)
            .await
            .map_err(|error| error.to_string())?;
        let session_dir = self
            .active_session_directory()
            .ok_or_else(|| "session directory missing after materialization".to_owned())?;

        let permission_mode = self.permission_mode;
        let prepared = PreparedNamedWorkflowLaunch {
            definition,
            args,
            session_dir,
            workspace: config.project_dir.clone(),
            permission_mode,
        };

        if permission_mode == super::PermissionMode::Ask {
            self.open_named_workflow_launch_review(prepared);
            return Ok(());
        }

        self.execute_named_workflow_launch(prepared).await
    }

    fn open_named_workflow_launch_review(&mut self, prepared: PreparedNamedWorkflowLaunch) {
        // Grant Available before review so Ask Revise preserves generation/nonce
        // and Cancel can revoke the same capability instance.
        self.workflow_capability.grant();
        let request_id = uuid::Uuid::new_v4().to_string();
        let workflow = named_workflow_approval_presentation(&prepared);
        let request = neo_agent_core::ApprovalRequest {
            turn: 0,
            id: request_id.clone(),
            operation: neo_agent_core::PermissionOperation::WorkflowLaunch,
            presentation: neo_agent_core::ApprovalPresentation::Workflow {
                title: "Launch workflow?".to_owned(),
                workflow,
            },
            options: named_workflow_approval_options(),
            workflow_origin: None,
        };
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        self.register_pending_approval(crate::modes::run::PendingApproval {
            request,
            response_tx,
        });
        self.pending_named_workflow_launch = Some(PendingNamedWorkflowLaunch {
            request_id,
            prepared,
        });
    }

    pub(super) async fn resolve_approval_response(
        &mut self,
        response: neo_agent_core::ApprovalResponse,
    ) {
        let request_id = match &response {
            neo_agent_core::ApprovalResponse::Selected { request_id, .. }
            | neo_agent_core::ApprovalResponse::Cancelled { request_id, .. } => request_id.clone(),
        };
        let pending = self
            .pending_named_workflow_launch
            .take_if(|pending| pending.request_id == request_id);
        // Always complete the chrome/transcript responder first.
        self.resolve_approval(response.clone());
        let Some(pending) = pending else {
            return;
        };
        match response {
            neo_agent_core::ApprovalResponse::Selected {
                action: neo_agent_core::ApprovalAction::LaunchWorkflow,
                ..
            } => {
                if let Err(error) = self.execute_named_workflow_launch(pending.prepared).await {
                    // Launch failure after Approve must not leave a stale Available
                    // capability that could authorize a different proposal.
                    self.workflow_capability.revoke_now();
                    self.push_status(format!("Workflow launch failed: {error}"));
                }
            }
            neo_agent_core::ApprovalResponse::Selected {
                action: neo_agent_core::ApprovalAction::ReviseWorkflow { .. },
                feedback,
                ..
            } => {
                // Design §13: Revise returns the same generation to Available.
                self.workflow_capability.unbind();
                let feedback = feedback.unwrap_or_default();
                if feedback.trim().is_empty() {
                    self.push_status(
                        "Workflow launch revised. Capability remains available for another launch."
                            .to_owned(),
                    );
                } else {
                    self.push_status(format!(
                        "Workflow launch revised. Capability remains available. Feedback: {feedback}"
                    ));
                }
            }
            neo_agent_core::ApprovalResponse::Selected {
                action: neo_agent_core::ApprovalAction::CancelWorkflow,
                ..
            }
            | neo_agent_core::ApprovalResponse::Cancelled { .. } => {
                self.workflow_capability.revoke_now();
                self.push_status("Workflow launch cancelled.".to_owned());
            }
            neo_agent_core::ApprovalResponse::Selected { .. } => {
                self.workflow_capability.revoke_now();
                self.push_status("Workflow launch cancelled.".to_owned());
            }
        }
    }

    pub(super) fn queue_approval_response(&mut self, response: neo_agent_core::ApprovalResponse) {
        self.deferred_approval_response = Some(response);
    }

    pub(super) async fn drain_deferred_approval_response(&mut self) {
        if let Some(response) = self.deferred_approval_response.take() {
            self.resolve_approval_response(response).await;
        }
    }

    async fn execute_named_workflow_launch(
        &mut self,
        prepared: PreparedNamedWorkflowLaunch,
    ) -> Result<(), String> {
        let Some(config) = self.local_config.as_ref() else {
            return Err("No config available".to_owned());
        };

        // Production workers resolve through the shared dispatch owner.
        config
            .workflow_dispatch_resolver
            .bind_workflow_runtime(&config.workflow_runtime)
            .map_err(|error| error.to_string())?;

        // Named slash authorizes via session capability with a Human actor.
        // Grant only if Ask review did not already grant (Auto/Yolo path).
        if !self.workflow_capability.inspect() {
            self.workflow_capability.grant();
        }
        let launch_nonce = self
            .workflow_capability
            .launch_nonce()
            .ok_or_else(|| "workflow launch capability missing".to_owned())?;

        let schema_sha256 = prepared
            .definition
            .input_schema
            .as_ref()
            .map(neo_agent_core::workflow::canonical_input_hash)
            .unwrap_or_default();

        let request = neo_agent_core::workflow::WorkflowLaunchRequest {
            name: prepared.definition.name.as_str().to_owned(),
            description: prepared.definition.description.clone(),
            phases: prepared.definition.phases.clone(),
            script: prepared.definition.lua_source.clone(),
            args: prepared.args.clone(),
            launch_source: format!("named:{}", prepared.definition.name.as_str()),
            parent_run_id: None,
        };

        let mut intent = neo_agent_core::workflow::WorkflowLaunchIntent::from_parts(
            request,
            prepared.session_dir.display().to_string(),
            prepared.workspace.display().to_string(),
            launch_nonce,
            neo_agent_core::workflow::WorkflowActor::Human,
            prepared.permission_mode,
            None,
            prepared.definition.compiled_input_schema.clone(),
            schema_sha256,
        );
        // Prefer the registry content revision over the script-byte fallback.
        intent.definition_revision = prepared.definition.revision.clone();

        let outcome = match neo_agent_core::workflow::WorkflowLaunchCoordinator
            .launch(
                &intent,
                neo_agent_core::workflow::WorkflowLaunchHosts {
                    runtime: &config.workflow_runtime,
                    capability: &self.workflow_capability,
                    background_tasks: &config.background_tasks,
                    session_dir: &prepared.session_dir,
                },
                neo_agent_core::workflow::LaunchAuthorizationMode::SessionCapability,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                // Preflight failures leave Available; do not leave a stale grant
                // from the named slash host path.
                if self.workflow_capability.is_unbound() {
                    self.workflow_capability.revoke_now();
                }
                return Err(error.to_string());
            }
        };

        self.push_status(format!(
            "Workflow `{}` launched as task `{}`.",
            prepared.definition.name.as_str(),
            outcome.task_id
        ));
        Ok(())
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
        let mut state = self
            .tui
            .chrome()
            .task_browser_state()
            .cloned()
            .unwrap_or_default();
        let intent = state.list_intent();
        let query = neo_agent_core::tools::BackgroundTaskListQuery {
            active_only: intent.active_only,
            kind: intent
                .workflow_only
                .then_some(neo_agent_core::tools::BackgroundTaskKind::Workflow),
            limit: if intent.limit == 0 { 50 } else { intent.limit },
            cursor: intent.cursor,
            ..neo_agent_core::tools::BackgroundTaskListQuery::default()
        };
        match config.background_tasks.list_page(query).await {
            Ok(page) => {
                let snapshot = task_browser::list_page_to_browser_snapshot(&page);
                if let Err(message) = state.apply_snapshot_checked(&snapshot) {
                    state.set_footer_message(message);
                }
            }
            Err(error) => {
                state.set_footer_message(error.to_string());
            }
        }
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

/// Host-parsed named slash launch payload awaiting Ask review or execution.
#[derive(Clone)]
pub(super) struct PreparedNamedWorkflowLaunch {
    definition: neo_agent_core::workflow::ResolvedWorkflowDefinition,
    args: serde_json::Value,
    session_dir: std::path::PathBuf,
    workspace: std::path::PathBuf,
    permission_mode: super::PermissionMode,
}

pub(super) struct PendingNamedWorkflowLaunch {
    pub(super) request_id: String,
    prepared: PreparedNamedWorkflowLaunch,
}

fn parse_named_workflow_slash_args(arg: &str) -> Result<(String, serde_json::Value), String> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Err("workflow name is required".to_owned());
    }
    let mut parts = arg.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim();
    if name.is_empty() {
        return Err("workflow name is required".to_owned());
    }
    let rest = parts.next().map(str::trim).unwrap_or("");
    let args = if rest.is_empty() {
        serde_json::json!({})
    } else {
        let value: serde_json::Value = serde_json::from_str(rest).map_err(|error| {
            format!("workflow arguments must be one complete JSON object: {error}")
        })?;
        if !value.is_object() {
            return Err("workflow arguments must be one complete JSON object".to_owned());
        }
        value
    };
    Ok((name.to_owned(), args))
}

fn named_workflow_approval_presentation(
    prepared: &PreparedNamedWorkflowLaunch,
) -> neo_agent_core::WorkflowApprovalPresentation {
    let args = serde_json::to_string_pretty(&prepared.args).unwrap_or_else(|_| "{}".to_owned());
    neo_agent_core::WorkflowApprovalPresentation {
        name: prepared.definition.name.as_str().to_owned(),
        description: prepared.definition.description.clone(),
        phases: prepared
            .definition
            .phases
            .iter()
            .map(|phase| format!("{}: {}", phase.id, phase.description))
            .collect(),
        args,
        line_count: prepared.definition.lua_source.split('\n').count().max(1),
        byte_count: prepared.definition.lua_source.len(),
        source: prepared.definition.lua_source.clone(),
        warning: "Launch approval authorizes orchestration only; child tool effects remain independently authorized."
            .to_owned(),
    }
}

fn named_workflow_approval_options() -> Vec<neo_agent_core::ApprovalOption> {
    vec![
        neo_agent_core::ApprovalOption {
            label: "Launch".to_owned(),
            description: None,
            action: neo_agent_core::ApprovalAction::LaunchWorkflow,
        },
        neo_agent_core::ApprovalOption {
            label: "Revise".to_owned(),
            description: Some("Return feedback without consuming the capability.".to_owned()),
            action: neo_agent_core::ApprovalAction::ReviseWorkflow {
                preset_feedback: None,
            },
        },
        neo_agent_core::ApprovalOption {
            label: "Cancel".to_owned(),
            description: Some("Revoke the capability without creating a run.".to_owned()),
            action: neo_agent_core::ApprovalAction::CancelWorkflow,
        },
    ]
}
