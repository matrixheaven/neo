use super::*;

#[cfg(test)]
use std::future::{Ready, ready};

#[allow(clippy::too_many_lines)]
pub fn controller_for_config(config: &AppConfig) -> InteractiveController {
    let catalogs = picker_catalogs_for_config(config);
    let registry = crate::modes::run::model_registry_for_config(config).ok();
    let selected_model = registry
        .as_ref()
        .and_then(|r| crate::modes::run::select_config_model(r, config).ok());
    let model_capabilities: std::collections::HashMap<String, neo_ai::ModelCapabilities> = registry
        .map(|r| {
            r.list()
                .into_iter()
                .map(|spec| {
                    let alias = format!("{}/{}", spec.provider.0, spec.model);
                    (alias, spec.capabilities)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut config = config.clone();
    if let Some(model) = &selected_model {
        config.default_provider.clone_from(&model.provider.0);
        config.default_model.clone_from(&model.model);
    }
    let run_config = config.clone();
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        // Prefer the live config snapshot from the dispatching controller so
        // providers/models added at runtime (e.g. via `/provider`) resolve;
        // fall back to the startup snapshot for safety.
        let mut effective_config = request.base_config.unwrap_or_else(|| run_config.clone());
        Box::pin(async move {
            if let Some(model) = request.model {
                effective_config.default_provider = model.provider;
                effective_config.default_model = model.alias;
            }
            effective_config.runtime.reasoning_effort = request.reasoning_effort;
            effective_config.permission_mode = request.permission_mode;
            effective_config.live_permission_mode = Arc::clone(&request.live_permission_mode);
            if let Some(session_id) = request.session_id {
                let turn = crate::modes::run::run_prompt_in_session_streaming(
                    &session_id,
                    &request.prompt,
                    &effective_config,
                    channels.events,
                    channels.approvals,
                    Some(channels.session_ids),
                    channels.cancel_token,
                    Some(channels.questions),
                    request.skill_context.clone(),
                    Some(request.plan_review_feedback.clone()),
                    Some(Arc::clone(&request.plan_mode)),
                    request.goal_mode_authoring,
                    channels.steer_input,
                    request.mcp_manager.clone(),
                    Arc::clone(&request.manual_compact_request),
                    request.compaction_only,
                )
                .await?;
                Ok(TurnOutcome::session(turn.session_id))
            } else {
                let turn = crate::modes::run::run_prompt_streaming(
                    &request.prompt,
                    &effective_config,
                    channels.events,
                    channels.approvals,
                    Some(channels.session_ids),
                    channels.cancel_token,
                    Some(channels.questions),
                    request.skill_context.clone(),
                    Some(request.plan_review_feedback.clone()),
                    Some(Arc::clone(&request.plan_mode)),
                    request.goal_mode_authoring,
                    channels.steer_input,
                    request.mcp_manager.clone(),
                    Arc::clone(&request.manual_compact_request),
                    request.compaction_only,
                )
                .await?;
                Ok(TurnOutcome::session(turn.session_id))
            }
        })
    });
    let load_config = config.clone();
    let load_session: SessionLoader = Arc::new(move |session_id| {
        let config = load_config.clone();
        Box::pin(async move { load_session_transcript(session_id, &config).await })
    });
    let fork_config = config.clone();
    let fork_session: SessionForker = Arc::new(move |session_id| {
        let config = fork_config.clone();
        Box::pin(async move { fork_session_transcript(session_id, &config).await })
    });

    let mut controller = InteractiveController::new(
        "neo",
        "new",
        config.default_model_label(),
        config.project_dir.clone(),
        run_turn,
        catalogs,
        load_session,
        fork_session,
    );
    let mut keybindings = KeybindingsManager::default();
    keybindings.set_user_bindings(
        config
            .tui
            .keybinding_overrides()
            .expect("AppConfig TUI keybindings should be validated before controller creation"),
    );
    controller.keybindings = keybindings;
    controller.completion_root.clone_from(&config.project_dir);
    let default_model_value = config.default_model.clone();
    let default_context_window = selected_model
        .as_ref()
        .and_then(|model| model.capabilities.max_context_tokens)
        .map(ContextWindow::new)
        .or_else(|| {
            controller
                .model_items
                .iter()
                .find(|item| item.value == default_model_value)
                .and_then(context_window_from_picker_item)
                .map(ContextWindow::new)
        });
    controller
        .tui
        .chrome_mut()
        .set_context_window(default_context_window);
    controller.current_thinking = config.runtime.reasoning_effort.is_some();
    controller
        .tui
        .chrome_mut()
        .set_thinking_enabled(controller.current_thinking);
    controller.local_config = Some(config.clone());
    let skill_store = resources::load_skill_store(
        neo_home().as_deref(),
        &config.extra_skill_dirs,
        &config.skill_path,
    )
    .ok();
    if let Some(ref store) = skill_store {
        controller
            .tui
            .transcript_mut()
            .set_skill_store(store.clone());
    }
    controller.skill_store = skill_store;
    controller.model_capabilities = model_capabilities;
    // Initialise the active model from the default so that features like image
    // paste work before the first turn (which would otherwise set it lazily).
    let model_label = config.default_model_label();
    if controller.active_model.is_none()
        && let Ok(model) =
            SelectedModel::from_alias(&model_label, Some(&config), &controller.model_items)
    {
        controller.active_model = Some(model);
    }
    // Seed the composer's in-memory history from the workspace bucket so Up/Down
    // can recall prompts submitted in earlier TUI sessions for this workspace.
    controller.prompt_history = Some(crate::prompt::history::PromptHistoryStore::for_config(
        &config,
    ));
    controller.load_prompt_history();
    controller.trust_store = crate::trust::ProjectTrustStore::from_home().ok();
    controller
}

#[cfg(test)]
pub(super) fn empty_session_loader(session_id: String) -> Ready<Result<LoadedSessionTranscript>> {
    ready(Ok(LoadedSessionTranscript::new(
        session_id,
        Vec::new(),
        Vec::new(),
    )))
}

#[cfg(test)]
pub(super) fn empty_session_forker(session_id: String) -> Ready<Result<ForkedSessionTranscript>> {
    ready(Ok(ForkedSessionTranscript::new(
        session_id.clone(),
        LoadedSessionTranscript::new(session_id, Vec::new(), Vec::new()),
    )))
}

pub(super) const fn dialog_result_may_close(result: InputResult) -> bool {
    matches!(
        result,
        InputResult::Submitted | InputResult::Cancelled | InputResult::Handled
    )
}

pub(super) fn startup_notices(config: &AppConfig) -> Vec<String> {
    let model_scope = if config.model_scope.is_empty() {
        "all".to_owned()
    } else {
        config.model_scope.join(",")
    };
    let mut notices = vec![
        "Startup".to_owned(),
        format!("project: {}", config.project_dir.display()),
        format!("sessions: {}", workspace_sessions_dir(config).display()),
        format!(
            "model: {}/{}",
            config.default_provider, config.default_model
        ),
        format!("model scope: {model_scope}"),
        format!("theme: {}", config.theme.name),
        "resources: auto-discovered".to_owned(),
        format!("trust: project={}", enabled_label(config.project_trusted)),
    ];
    if !config.tui.keybindings.is_empty() {
        notices.push(format!(
            "keybindings: {} {}",
            config.tui.keybindings.len(),
            pluralize(config.tui.keybindings.len(), "override", "overrides")
        ));
        notices.push("local config: tui.keybindings available".to_owned());
    }
    notices
}

pub(super) fn enabled_label(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

pub(super) const fn pluralize(
    count: usize,
    singular: &'static str,
    plural: &'static str,
) -> &'static str {
    if count == 1 { singular } else { plural }
}

pub(super) fn same_work_dir(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub(super) async fn create_interactive_session_path(config: &AppConfig) -> Result<PathBuf> {
    let bucket_dir = workspace_sessions_dir(config);
    tokio::fs::create_dir_all(&bucket_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create sessions directory {}",
                bucket_dir.display()
            )
        })?;

    loop {
        let session_id = format!("session_{}", uuid::Uuid::new_v4());
        let session_dir = bucket_dir.join(&session_id);
        if tokio::fs::metadata(&session_dir).await.is_err() {
            tokio::fs::create_dir_all(&session_dir)
                .await
                .with_context(|| {
                    format!(
                        "failed to create session directory {}",
                        session_dir.display()
                    )
                })?;
            return Ok(session_dir.join("transcript.jsonl"));
        }
    }
}

pub(super) fn session_id_from_transcript_path(path: &Path) -> Result<String> {
    let session_dir = path
        .parent()
        .with_context(|| format!("invalid session path {}", path.display()))?;
    let id = session_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session directory name {}", session_dir.display()))?;
    Ok(id.to_owned())
}

pub(super) fn current_unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

/// Parse a timestamp string (epoch millis, epoch secs, or RFC3339) into `SystemTime`.
pub(super) fn parse_timestamp(ts: &str) -> std::time::SystemTime {
    // Try epoch millis first
    if let Ok(millis) = ts.parse::<u64>() {
        let secs = millis / 1000;
        let nanos = u32::try_from((millis % 1000) * 1_000_000)
            .expect("millisecond remainder fits in nanoseconds");
        if let Some(t) = std::time::UNIX_EPOCH.checked_add(std::time::Duration::new(secs, nanos)) {
            return t;
        }
    }
    // Try epoch seconds
    let seconds_str = ts.split_once('.').map_or(ts, |(s, _)| s);
    if let Ok(secs) = seconds_str.parse::<u64>()
        && let Some(t) = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
    {
        return t;
    }
    std::time::UNIX_EPOCH
}
