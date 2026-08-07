mod mcp_cli;
mod models_cli;
mod output;
mod runtime;
mod session_mgmt;

// Re-export runtime functions for callers that access them via
// `crate::modes::run::*` (interactive.rs, btw.rs, rpc/server.rs).
#[allow(unused_imports)]
pub(crate) use runtime::{
    agent_config_for_app, model_registry_for_config, resolve_model, resolve_model_client,
    select_config_model, tool_registry_for_config,
};

// Re-export CLI functions called from `main.rs` via `modes::run::*`.
pub(crate) use mcp_cli::{AddMcpServerInput, add_mcp_server, auth_mcp_server, list_mcp};
pub(crate) use models_cli::list_configured_models;

// Re-export session helpers used within this module.
use session_mgmt::{
    latest_session_id, record_initial_session_title, record_session_activity,
    session_root_from_wire_path,
};

use std::{
    collections::HashMap,
    io::IsTerminal as _,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::goal::GoalManager;
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter, SessionEventPersistence};
use neo_agent_core::{
    AgentContext, AgentEvent, AgentMessage, AgentRuntime, ApprovalRequest, ApprovalResponse,
    AskUserTool, Content, CreateSkillTool, ListSkillsTool, MessageOrigin, MoveSkillTool,
    SteerInputHandle, SummarizeSessionsTool, WorkflowNotification, skills::SkillStoreHandle,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::RunOutput,
    config::{AppConfig, neo_home, workspace_sessions_dir},
    modes::{
        interactive::{TurnChannels, TurnRequest},
        sessions,
    },
    resources,
};

pub async fn execute(
    prompt: &[String],
    config: &AppConfig,
    output: RunOutput,
    continue_latest: bool,
    no_session: bool,
) -> anyhow::Result<String> {
    let show_retry_notices = output == RunOutput::Text && !std::io::stdout().is_terminal();
    let turn = if no_session {
        run_prompt_ephemeral(prompt, config, show_retry_notices).await?
    } else if continue_latest {
        let session_id = latest_session_id(config)?;
        run_prompt_in_session(&session_id, prompt, config, show_retry_notices).await?
    } else {
        run_prompt_with_retry_notices(prompt, config, show_retry_notices).await?
    };
    match output {
        RunOutput::Json => output::stable_json_output(&turn, config),
        RunOutput::Text => Ok(format!("{}\n", turn.assistant_text)),
        RunOutput::Events => events_output(&turn, config),
    }
}

#[derive(Default)]
struct PersistedWorkflowProjection {
    max_sequence: Option<u64>,
    turn: u32,
    recovery_failure: Option<PersistedWorkflowRecoveryFailure>,
}

struct PersistedWorkflowRecoveryFailure {
    state: neo_agent_core::workflow::WorkflowState,
    reason: Option<String>,
    sequence_watermark: Option<u64>,
}

async fn prepare_recovered_workflow_dispatch(
    config: &AppConfig,
    session_dir: &std::path::Path,
    replayed_events: &[AgentEvent],
) -> anyhow::Result<()> {
    let dispatch_runtime =
        runtime_for_config(config, Some(session_dir.to_path_buf()), None, None).await?;
    let context = AgentContext::from_replay(replayed_events.iter());
    dispatch_runtime.refresh_workflow_dispatch(&context)?;
    Ok(())
}

pub(crate) async fn rehydrate_session_workflows(
    config: &AppConfig,
    session_id: &str,
    session_dir: &std::path::Path,
    replayed_events: &[AgentEvent],
) -> anyhow::Result<Vec<AgentEvent>> {
    let runtime = config.workflow_runtime.clone();
    let background_tasks = config.background_tasks.clone();
    config
        .workflow_dispatch_resolver
        .bind_workflow_runtime(&runtime)
        .context("failed to bind workflow runtime")?;
    runtime.notification_queue().restore_projected(
        neo_agent_core::session::workflow_notification_projection_ids(replayed_events),
    );
    let mut persisted_projections = HashMap::<String, PersistedWorkflowProjection>::new();
    for event in replayed_events {
        let (turn, workflow) = match event {
            AgentEvent::WorkflowStarted { turn, workflow }
            | AgentEvent::WorkflowUpdated { turn, workflow }
            | AgentEvent::WorkflowFinished { turn, workflow } => (*turn, workflow),
            _ => continue,
        };
        let entry = persisted_projections
            .entry(workflow.id.0.clone())
            .or_default();
        entry.turn = turn;
        if workflow.recovery_failure {
            entry.recovery_failure = Some(PersistedWorkflowRecoveryFailure {
                state: workflow.state,
                reason: workflow.terminal_reason.clone(),
                sequence_watermark: entry.max_sequence,
            });
        } else if let Some(sequence) = workflow.projection_sequence {
            let supersedes_recovery_failure =
                entry.recovery_failure.as_ref().is_some_and(|failure| {
                    failure
                        .sequence_watermark
                        .is_none_or(|watermark| sequence > watermark)
                });
            if supersedes_recovery_failure {
                entry.recovery_failure = None;
            }
            if entry.max_sequence.is_none_or(|current| sequence > current) {
                entry.max_sequence = Some(sequence);
            }
        }
    }
    let handles = runtime
        .rehydrate(session_dir)
        .await
        .with_context(|| format!("failed to recover workflows for session {session_id}"))?;
    let mut has_resumable_workflow = false;
    for handle in &handles {
        if handle.snapshot().await.state == neo_agent_core::workflow::WorkflowState::Paused {
            has_resumable_workflow = true;
            break;
        }
    }
    if has_resumable_workflow
        && let Err(error) =
            prepare_recovered_workflow_dispatch(config, session_dir, replayed_events).await
    {
        tracing::warn!(
            session_id,
            %error,
            "recovered workflow dispatch is not ready; resume will remain paused"
        );
    }
    let mut recovered_events = Vec::new();
    for handle in handles {
        let task_id = handle.run_id.0.clone();
        let snapshot = handle.snapshot().await;
        if background_tasks.workflow_handle(&task_id).await.is_none() {
            background_tasks
                .start_workflow(task_id.clone(), snapshot.title.clone(), handle)
                .await
                .with_context(|| {
                    format!("failed to register recovered workflow for session {session_id}")
                })?;
        }
        let persisted = persisted_projections.get(&task_id);
        let recovery_failure_already_projected = persisted
            .and_then(|projection| projection.recovery_failure.as_ref())
            .is_some_and(|failure| {
                failure.state == snapshot.state && failure.reason == snapshot.terminal_reason
            });
        let should_project = if snapshot.recovery_failure {
            snapshot.state.is_terminal() && !recovery_failure_already_projected
        } else {
            snapshot.projection_sequence.is_some_and(|sequence| {
                persisted
                    .and_then(|projection| projection.max_sequence)
                    .is_none_or(|persisted_sequence| sequence > persisted_sequence)
            })
        };
        if !should_project {
            continue;
        }
        let turn = persisted.map_or(0, |projection| projection.turn);
        let event = if snapshot.state.is_terminal() {
            AgentEvent::WorkflowFinished {
                turn,
                workflow: snapshot,
            }
        } else {
            AgentEvent::WorkflowUpdated {
                turn,
                workflow: snapshot,
            }
        };
        recovered_events.push(event);
    }
    if !recovered_events.is_empty() {
        let path = sessions::session_path(session_id, config)?;
        let mut writer = JsonlSessionWriter::open_append(path).await?;
        let mut persistence = SessionEventPersistence::default();
        for event in &recovered_events {
            for persisted in persistence.persisted_events(event) {
                writer.append_event(&persisted).await?;
            }
        }
        writer.flush().await?;
    }
    Ok(recovered_events)
}

fn events_output(turn: &PromptTurn, config: &AppConfig) -> anyhow::Result<String> {
    let mut rendered = String::new();
    for event in &turn.events {
        let value = match event {
            AgentEvent::InstructionEpoch { epoch } => {
                output::stable_instruction_epoch_event(epoch, config)
            }
            _ => serde_json::to_value(event)?,
        };
        rendered.push_str(&serde_json::to_string(&value)?);
        rendered.push('\n');
    }
    Ok(rendered)
}

pub struct PromptTurn {
    pub session_id: String,
    pub events: Vec<AgentEvent>,
    pub assistant_text: String,
}

pub struct StreamingPromptTurn {
    pub session_id: String,
    pub assistant_text: String,
    pub event_count: usize,
}

/// One live approval: the canonical request plus its single response channel.
///
/// The UI registers this atomically (store responder, open chrome modal, upsert
/// transcript). The runtime handler awaits exactly one `ApprovalResponse`.
pub struct PendingApproval {
    pub request: ApprovalRequest,
    pub response_tx: oneshot::Sender<ApprovalResponse>,
}

pub async fn run_prompt_with_event_stream(
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
) -> anyhow::Result<StreamingPromptTurn> {
    run_prompt_streaming_with_retry_notices(prompt, config, event_tx).await
}

async fn run_prompt_streaming_with_retry_notices(
    prompt: &[String],
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
) -> anyhow::Result<StreamingPromptTurn> {
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(prompt_text.as_str())];
    let created = crate::modes::sessions::create_new_session(config).await?;
    let session_path = created.wire_path;
    let session_id = created.session_id;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let user_message = user_message(content, MessageOrigin::User, None);
    record_session_activity(config, &session_id, &prompt_text);
    let runtime = match runtime_for_config(
        config,
        Some(session_root_from_wire_path(&session_path)?),
        None,
        None,
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            writer
                .append_event(&AgentEvent::MessageAppended {
                    message: user_message,
                })
                .await?;
            writer.flush().await?;
            return Err(error);
        }
    };
    let streaming = StreamingTurnIo {
        event_tx,
        session_id: session_id.clone(),
        cancel_token: CancellationToken::new(),
    };
    let turn = finish_prompt_turn_streaming(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        streaming,
    )
    .await?;
    record_initial_session_title(config, &turn.session_id, &turn.assistant_text, &prompt_text)
        .await;
    Ok(turn)
}

async fn run_prompt_with_retry_notices(
    prompt: &[String],
    config: &AppConfig,
    show_retry_notices: bool,
) -> anyhow::Result<PromptTurn> {
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(prompt_text.as_str())];
    let created = crate::modes::sessions::create_new_session(config).await?;
    let session_path = created.wire_path;
    let session_id = created.session_id;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let user_message = user_message(content, MessageOrigin::User, None);
    record_session_activity(config, &session_id, &prompt_text);
    let runtime = match runtime_for_config(
        config,
        Some(session_root_from_wire_path(&session_path)?),
        None,
        None,
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            // Persist user message so the session transcript is not empty
            // when credential checks or other early failures prevent the
            // runtime from emitting it.
            writer
                .append_event(&AgentEvent::MessageAppended {
                    message: user_message,
                })
                .await?;
            writer.flush().await?;
            return Err(error);
        }
    };
    let turn = finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        Vec::new(),
        session_id,
        show_retry_notices,
    )
    .await?;
    record_initial_session_title(config, &turn.session_id, &turn.assistant_text, &prompt_text)
        .await;
    Ok(turn)
}

async fn run_prompt_ephemeral(
    prompt: &[String],
    config: &AppConfig,
    show_retry_notices: bool,
) -> anyhow::Result<PromptTurn> {
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(prompt_text.as_str())];
    let mut writer = SessionEventWriter::memory();
    let user_message = user_message(content, MessageOrigin::User, None);
    let runtime = runtime_for_config(config, None, None, None).await?;
    finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        Vec::new(),
        "ephemeral".to_owned(),
        show_retry_notices,
    )
    .await
}

async fn run_prompt_in_session(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
    show_retry_notices: bool,
) -> anyhow::Result<PromptTurn> {
    let prompt_text = prompt.join(" ");
    let user_content = vec![Content::text(prompt_text.as_str())];
    let session_path = sessions::session_path(session_id, config)?;
    let replayed_events = JsonlSessionReader::read_all(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let context = AgentContext::from_replay(replayed_events.iter());
    let session_dir = session_root_from_wire_path(&session_path)?;
    let _ = rehydrate_session_workflows(config, session_id, &session_dir, &replayed_events).await?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let user_message = user_message(user_content, MessageOrigin::User, None);
    record_session_activity(config, session_id, &prompt_text);
    let runtime = runtime_for_config(config, Some(session_dir), None, None).await?;
    runtime.restore_plan_mode(&context);
    let turn = finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        Vec::new(),
        session_id.to_owned(),
        show_retry_notices,
    )
    .await?;
    let notification_queue = config.workflow_runtime.notification_queue();
    let notification_ids =
        neo_agent_core::session::workflow_notification_projection_ids(&turn.events);
    let has_terminal = !notification_ids.is_empty();
    for id in notification_ids {
        let _ = notification_queue.mark_projected(&id);
    }
    // After a terminal workflow whose transcript summary is persisted,
    // run automatic retention to reclaim eligible old runs.
    if has_terminal {
        let sessions_root = workspace_sessions_dir(config);
        let outcome = config.workflow_runtime.try_auto_retention(&sessions_root);
        if outcome.reclaimed_count > 0 {
            tracing::info!(
                "auto-retention after workflow completion: reclaimed {} runs ({} bytes)",
                outcome.reclaimed_count,
                outcome.reclaimed_bytes
            );
        }
    }
    Ok(turn)
}

pub async fn run_prompt_streaming(
    request: TurnRequest,
    channels: TurnChannels,
    config: &AppConfig,
) -> anyhow::Result<StreamingPromptTurn> {
    ensure_new_workflow_context_capacity(&request, &channels, config).await?;
    let prepared = prepare_new_streaming_turn(
        &request.prompt,
        request.prompt_origin.clone(),
        request.prompt_display_text.clone(),
        config,
        Some(channels.session_ids.clone()),
        request.skill_context.clone(),
    )
    .await?;
    let prompt = prepared.prompt.clone();
    let runtime = runtime_for_config(
        config,
        Some(prepared.session_directory.clone()),
        Some(&request),
        Some(&channels),
    )
    .await?;
    let turn = run_prepared_streaming_turn(
        prepared,
        runtime,
        channels.events,
        channels.cancel_token,
        request.compaction_only,
    )
    .await?;
    record_initial_session_title(config, &turn.session_id, &turn.assistant_text, &prompt).await;
    Ok(turn)
}

pub async fn run_prompt_in_session_streaming(
    session_id: &str,
    request: TurnRequest,
    channels: TurnChannels,
    config: &AppConfig,
) -> anyhow::Result<StreamingPromptTurn> {
    let prepared = prepare_existing_streaming_turn(
        session_id,
        &request.prompt,
        request.prompt_origin.clone(),
        request.prompt_display_text.clone(),
        config,
        Some(channels.session_ids.clone()),
        request.skill_context.clone(),
    )
    .await?;
    let runtime = runtime_for_config(
        config,
        Some(prepared.session_directory.clone()),
        Some(&request),
        Some(&channels),
    )
    .await?;
    ensure_workflow_context_capacity(
        &runtime,
        &request,
        &prepared.context,
        &prepared.user_message,
    )?;
    runtime.restore_plan_mode(&prepared.context);
    run_prepared_streaming_turn(
        prepared,
        runtime,
        channels.events,
        channels.cancel_token,
        request.compaction_only,
    )
    .await
}

async fn prepare_new_streaming_turn(
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    prompt_display_text: Option<String>,
    config: &AppConfig,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    skill_context: Option<String>,
) -> anyhow::Result<PreparedStreamingTurn> {
    let prompt_text = prompt
        .iter()
        .filter_map(|c| c.as_text())
        .collect::<Vec<_>>()
        .join(" ");
    let created = crate::modes::sessions::create_new_session(config).await?;
    let session_path = created.wire_path;
    let session_id = created.session_id;
    let writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let user_message = user_message(prompt.to_vec(), prompt_origin, prompt_display_text);
    record_session_activity(config, &session_id, &prompt_text);
    send_streaming_session_id(session_id_tx, &session_id);
    let session_directory = session_root_from_wire_path(&session_path)?;
    Ok(PreparedStreamingTurn {
        prompt: prompt_text,
        session_id,
        session_directory,
        context: streaming_context(skill_context),
        writer,
        user_message,
    })
}

async fn prepare_existing_streaming_turn(
    session_id: &str,
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    prompt_display_text: Option<String>,
    config: &AppConfig,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    skill_context: Option<String>,
) -> anyhow::Result<PreparedStreamingTurn> {
    let prompt_text = prompt
        .iter()
        .filter_map(|c| c.as_text())
        .collect::<Vec<_>>()
        .join(" ");
    let session_path = sessions::session_path(session_id, config)?;
    let session_directory = session_root_from_wire_path(&session_path)?;
    let mut context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    apply_skill_context(&mut context, skill_context);
    let writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    send_streaming_session_id(session_id_tx, session_id);
    let user_message = user_message(prompt.to_vec(), prompt_origin, prompt_display_text);
    record_session_activity(config, session_id, &prompt_text);
    Ok(PreparedStreamingTurn {
        prompt: prompt_text,
        session_id: session_id.to_owned(),
        session_directory,
        context,
        writer,
        user_message,
    })
}

fn send_streaming_session_id(
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    session_id: &str,
) {
    if let Some(session_id_tx) = session_id_tx {
        let _ = session_id_tx.send(session_id.to_owned());
    }
}

fn streaming_context(skill_context: Option<String>) -> AgentContext {
    let mut context = AgentContext::new();
    apply_skill_context(&mut context, skill_context);
    context
}

fn apply_skill_context(context: &mut AgentContext, skill_context: Option<String>) {
    if let Some(skill_context) = skill_context {
        context.set_skill_context(AgentMessage::system_text(skill_context));
    }
}

async fn run_prepared_streaming_turn(
    prepared: PreparedStreamingTurn,
    runtime: AgentRuntime,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    cancel_token: CancellationToken,
    compaction_only: bool,
) -> anyhow::Result<StreamingPromptTurn> {
    let PreparedStreamingTurn {
        session_id,
        session_directory: _,
        context,
        mut writer,
        user_message,
        prompt: _,
    } = prepared;
    let streaming = StreamingTurnIo {
        event_tx,
        session_id,
        cancel_token,
    };
    if compaction_only {
        finish_compaction_turn_streaming(context, &mut writer, runtime, streaming).await
    } else {
        finish_prompt_turn_streaming(user_message, context, &mut writer, runtime, streaming).await
    }
}

async fn runtime_for_config(
    config: &AppConfig,
    session_directory: Option<PathBuf>,
    request: Option<&TurnRequest>,
    channels: Option<&TurnChannels>,
) -> anyhow::Result<AgentRuntime> {
    let model = runtime::resolve_model(config)?;
    let client = runtime::resolve_model_client(config, &model)?;
    let skill_store = resources::load_skill_store(
        neo_home().as_deref(),
        &config.extra_skill_dirs,
        &config.skill_path,
    )?;
    let skill_store_handle = SkillStoreHandle::new(skill_store.clone());
    let mut agent_config = runtime::agent_config_for_app(
        model,
        config,
        channels.map(|channels| channels.approvals.clone()),
        request.and_then(|request| request.instruction_registry.clone()),
    )?;
    if let Some(session_directory) = &session_directory {
        agent_config = agent_config.with_session_directory(session_directory.clone());
    }
    agent_config.manual_compact_request = request.map_or_else(
        || Arc::new(Mutex::new(None)),
        |request| Arc::clone(&request.manual_compact_request),
    );
    if let Some(request) = request {
        agent_config = agent_config.with_plan_mode(Arc::clone(&request.plan_mode));
        if let Some(workflow_context) = &request.workflow_context {
            agent_config = agent_config.with_turn_injection(workflow_context.clone());
        }
    }
    if request.is_some_and(|request| request.goal_mode_authoring) {
        agent_config = agent_config.with_goal_mode_authoring(true);
    }
    let mut tools = runtime::tool_registry_for_config(
        config,
        std::sync::Arc::clone(&agent_config.todos),
        request.and_then(|request| request.mcp_manager.as_ref()),
    )
    .await?;
    if let Some(channels) = channels {
        tools.register(AskUserTool::new(channels.questions.clone()));
    }
    tools.register(ListSkillsTool::new(skill_store_handle.clone()));
    if let Some(home) = neo_home() {
        let skill_store_reload = skill_store_reloader(config);
        let move_reload = Arc::clone(&skill_store_reload);
        tools.register(
            MoveSkillTool::new(home.clone())
                .with_skill_store_reload(skill_store_handle.clone(), move || move_reload()),
        );
        let create_reload = Arc::clone(&skill_store_reload);
        tools.register(
            CreateSkillTool::new(home.clone())
                .with_skill_store_reload(skill_store_handle.clone(), move || create_reload()),
        );
        tools.register(SummarizeSessionsTool::new(home));
    }
    // ThemeDraft is a root-runtime-only host tool: it mutates $NEO_HOME/themes/
    // and is deliberately absent from `tool_registry_for_config`, so the Btw
    // sidecar and child/delegate registries never acquire it. The bounded draft
    // store is session-scoped: the interactive controller shares one store
    // across turns so a preview in turn N can be saved in turn N+1; headless
    // call sites without a request get a fresh store.
    let theme_draft = if let Some(request) = request {
        crate::theme_draft::ThemeDraftTool::new(
            crate::themes::ThemeRepository::default(),
            Arc::clone(&request.theme_draft_store),
        )
    } else {
        crate::theme_draft::ThemeDraftTool::default_with_store()
    };
    tools.register(theme_draft);
    let mut runtime =
        AgentRuntime::with_tools_and_skill_handle(agent_config, client, tools, skill_store_handle);
    runtime = runtime.with_steer_input(channels.map_or_else(SteerInputHandle::new, |channels| {
        channels.steer_input.clone()
    }));
    if let Some(session_dir) = session_directory {
        let goal_manager = Arc::new(GoalManager::load(session_dir).await?);
        if let Some(tools) = runtime.tools_mut() {
            Arc::get_mut(tools)
                .expect("tools arc not yet shared")
                .register_goal_tools(Arc::clone(&goal_manager));
        }
        runtime = runtime.with_goal_manager(&goal_manager);
    }
    Ok(runtime)
}

fn ensure_workflow_context_capacity(
    runtime: &AgentRuntime,
    request: &TurnRequest,
    context: &AgentContext,
    user_message: &AgentMessage,
) -> anyhow::Result<()> {
    if request.workflow_context.is_some()
        && !runtime.turn_messages_fit_after_compaction(context, std::slice::from_ref(user_message))
    {
        anyhow::bail!(crate::modes::interactive::workflow_slash::WORKFLOW_CONTEXT_TOO_LARGE);
    }
    Ok(())
}

async fn ensure_new_workflow_context_capacity(
    request: &TurnRequest,
    channels: &TurnChannels,
    config: &AppConfig,
) -> anyhow::Result<()> {
    if request.workflow_context.is_none() {
        return Ok(());
    }
    let temp_session = tempfile::tempdir()?;
    let runtime = runtime_for_config(
        config,
        Some(temp_session.path().to_path_buf()),
        Some(request),
        Some(channels),
    )
    .await?;
    let context = streaming_context(request.skill_context.clone());
    let user_message = user_message(
        request.prompt.clone(),
        request.prompt_origin.clone(),
        request.prompt_display_text.clone(),
    );
    ensure_workflow_context_capacity(&runtime, request, &context, &user_message)
}

fn skill_store_reloader(
    config: &AppConfig,
) -> Arc<dyn Fn() -> Result<neo_agent_core::skills::SkillStore, String> + Send + Sync> {
    let neo_home = neo_home();
    let extra_skill_dirs = config.extra_skill_dirs.clone();
    let skill_path = config.skill_path.clone();
    Arc::new(move || {
        resources::load_skill_store(neo_home.as_deref(), &extra_skill_dirs, &skill_path)
            .map_err(|err| err.to_string())
    })
}

/// Set up the workflow dispatch resolver for a headless CLI workflow run.
///
/// Creates a minimal AgentRuntime so the workflow dispatch resolver has a bound
/// snapshot, then binds the workflow runtime to that resolver. This enables
/// real Lua execution during `neo workflow run`.
pub async fn setup_workflow_dispatch(
    config: &AppConfig,
    session_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let runtime = runtime_for_config(config, Some(session_dir.to_path_buf()), None, None).await?;
    runtime.refresh_workflow_dispatch(&AgentContext::new())?;
    config
        .workflow_dispatch_resolver
        .bind_workflow_runtime(&config.workflow_runtime)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[cfg(test)]
pub(crate) async fn run_prompt_with_runtime_message(
    content: Vec<Content>,
    origin: MessageOrigin,
    display_text: Option<String>,
    context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
) -> anyhow::Result<PromptTurn> {
    let mut writer = SessionEventWriter::jsonl(writer);
    let user_message = user_message(content, origin, display_text);
    finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        Vec::new(),
        "test-session".to_owned(),
        false,
    )
    .await
}

#[cfg(test)]
async fn run_prompt_with_runtime(
    prompt: String,
    context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
) -> anyhow::Result<PromptTurn> {
    run_prompt_with_runtime_message(
        vec![Content::text(prompt)],
        MessageOrigin::User,
        None,
        context,
        writer,
        runtime,
    )
    .await
}

fn user_message(
    content: Vec<Content>,
    origin: MessageOrigin,
    display_text: Option<String>,
) -> AgentMessage {
    AgentMessage::User {
        content,
        display_text: display_text.map(Into::into),
        origin,
    }
}

async fn finish_prompt_turn(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut SessionEventWriter<'_>,
    runtime: AgentRuntime,
    mut events: Vec<AgentEvent>,
    session_id: String,
    show_retry_notices: bool,
) -> anyhow::Result<PromptTurn> {
    let mut assistant_text = String::new();
    let mut persistence = SessionEventPersistence::default();
    let mut turn_stream =
        runtime.run_turn_with_cancel(&mut context, user_message.clone(), CancellationToken::new());
    while let Some(event) = turn_stream.next().await {
        let event = event?;
        if show_retry_notices {
            let mut stderr = std::io::stderr();
            let _ = write_retry_notice(&event, &mut stderr);
        }
        if let AgentEvent::MessageAppended { message } = &event
            && matches!(message, AgentMessage::Assistant { .. })
        {
            assistant_text.push_str(&message.text());
        }
        for persisted in persistence.persisted_events(&event) {
            writer.append_event(&persisted).await?;
        }
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id,
        events,
        assistant_text,
    })
}

fn write_retry_notice<W: std::io::Write>(
    event: &AgentEvent,
    output: &mut W,
) -> std::io::Result<()> {
    let AgentEvent::RetryScheduled {
        retry,
        max_retries,
        delay_ms,
        error_code,
        message,
        ..
    } = event
    else {
        return Ok(());
    };
    let message = neo_tui::primitive::strip_ansi(message)
        .chars()
        .map(|character| {
            matches!(character, '\r' | '\n')
                .then_some(' ')
                .unwrap_or(character)
        })
        .collect::<String>();
    let message = if error_code == "provider.transport_error" {
        let detail = message
            .strip_prefix("transport error: ")
            .unwrap_or(message.as_str());
        format!("Network error: {detail}")
    } else {
        message
    };
    writeln!(
        output,
        "Reconnecting {retry}/{max_retries} in {delay_ms}ms: {message}"
    )
}

enum SessionEventWriter<'a> {
    Jsonl(&'a mut JsonlSessionWriter),
    Memory,
}

impl<'a> SessionEventWriter<'a> {
    fn jsonl(writer: &'a mut JsonlSessionWriter) -> Self {
        Self::Jsonl(writer)
    }

    fn memory() -> Self {
        Self::Memory
    }

    async fn append_event(&mut self, event: &AgentEvent) -> anyhow::Result<()> {
        match self {
            Self::Jsonl(writer) => writer
                .append_event(event)
                .await
                .map_err(anyhow::Error::from),
            Self::Memory => Ok(()),
        }
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        match self {
            Self::Jsonl(writer) => writer.flush().await.map_err(anyhow::Error::from),
            Self::Memory => Ok(()),
        }
    }
}

struct StreamingTurnIo {
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    session_id: String,
    cancel_token: CancellationToken,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionWorkflowEvent {
    pub(crate) session_id: String,
    pub(crate) generation: u64,
    pub(crate) event: AgentEvent,
}

#[derive(Debug)]
pub(crate) enum PersistedSessionWorkflowEvent {
    Event(Box<SessionWorkflowEvent>),
    Error {
        session_id: String,
        generation: u64,
        message: String,
    },
}

pub(crate) async fn persist_session_workflow_events(
    config: AppConfig,
    mut events: mpsc::UnboundedReceiver<SessionWorkflowEvent>,
    persisted: mpsc::UnboundedSender<PersistedSessionWorkflowEvent>,
) {
    let mut persistence = HashMap::<String, SessionEventPersistence>::new();
    while let Some(envelope) = events.recv().await {
        let result = persist_session_workflow_event(&config, &mut persistence, &envelope).await;
        let delivery = match result {
            Ok(()) => PersistedSessionWorkflowEvent::Event(Box::new(envelope)),
            Err(error) => PersistedSessionWorkflowEvent::Error {
                session_id: envelope.session_id,
                generation: envelope.generation,
                message: error.to_string(),
            },
        };
        if persisted.send(delivery).is_err() {
            break;
        }
    }
}

async fn persist_session_workflow_event(
    config: &AppConfig,
    persistence: &mut HashMap<String, SessionEventPersistence>,
    envelope: &SessionWorkflowEvent,
) -> anyhow::Result<()> {
    let path = sessions::session_path(&envelope.session_id, config)?;
    let mut writer = JsonlSessionWriter::open_append(path).await?;
    let session = persistence.entry(envelope.session_id.clone()).or_default();
    for event in session.persisted_events(&envelope.event) {
        writer.append_event(&event).await?;
    }
    writer.flush().await?;
    Ok(())
}

struct PreparedStreamingTurn {
    prompt: String,
    session_id: String,
    session_directory: PathBuf,
    context: AgentContext,
    writer: JsonlSessionWriter,
    user_message: AgentMessage,
}

#[derive(Debug, PartialEq, Eq)]
struct StreamingEventEffect {
    persist: bool,
    forward: bool,
    assistant_text: Option<String>,
}

async fn finish_prompt_turn_streaming(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    streaming: StreamingTurnIo,
) -> anyhow::Result<StreamingPromptTurn> {
    let mut assistant_text = String::new();
    let mut event_count = 0;
    let mut persistence = SessionEventPersistence::default();
    let mut stream =
        runtime.run_turn_with_cancel(&mut context, user_message.clone(), streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = streaming_event_or_bail(event, &streaming.event_tx)?;
        append_streaming_event(
            event,
            writer,
            &mut assistant_text,
            &streaming.event_tx,
            &mut event_count,
            &mut persistence,
        )
        .await?;
    }
    writer.flush().await?;

    Ok(StreamingPromptTurn {
        session_id: streaming.session_id,
        assistant_text,
        event_count,
    })
}

async fn finish_compaction_turn_streaming(
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    streaming: StreamingTurnIo,
) -> anyhow::Result<StreamingPromptTurn> {
    let mut event_count = 0;
    let mut stream =
        runtime.run_manual_compaction_turn_with_cancel(&mut context, streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                writer.flush().await?;
                return Err(streaming_error(error, &streaming.event_tx));
            }
        };
        writer.append_event(&event).await?;
        event_count += 1;
        let _ = streaming.event_tx.send(Ok(event));
    }
    writer.flush().await?;

    Ok(StreamingPromptTurn {
        session_id: streaming.session_id,
        assistant_text: String::new(),
        event_count,
    })
}

fn streaming_event_or_bail<E: std::fmt::Display>(
    event: Result<AgentEvent, E>,
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
) -> anyhow::Result<AgentEvent> {
    event.map_err(|error| streaming_error(error, event_tx))
}

fn streaming_error(
    error: impl std::fmt::Display,
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
) -> anyhow::Error {
    let message = error.to_string();
    let _ = event_tx.send(Err(anyhow::anyhow!(message.clone())));
    anyhow::anyhow!(message)
}

async fn append_streaming_event(
    event: AgentEvent,
    writer: &mut JsonlSessionWriter,
    assistant_text: &mut String,
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    event_count: &mut usize,
    persistence: &mut SessionEventPersistence,
) -> anyhow::Result<()> {
    let effect = streaming_event_effect(&event);
    if let Some(text) = effect.assistant_text {
        assistant_text.push_str(&text);
    }
    if effect.persist {
        for persisted in persistence.persisted_events(&event) {
            writer.append_event(&persisted).await?;
        }
        if matches!(
            &event,
            AgentEvent::MessageAppended { message }
                if WorkflowNotification::projection_id(message).is_some()
        ) {
            writer.flush().await?;
        }
    }
    if effect.forward {
        *event_count += 1;
        let _ = event_tx.send(Ok(event));
    }
    Ok(())
}

fn streaming_event_effect(event: &AgentEvent) -> StreamingEventEffect {
    StreamingEventEffect {
        persist: true,
        forward: true,
        assistant_text: assistant_text_from_event(event),
    }
}

fn assistant_text_from_event(event: &AgentEvent) -> Option<String> {
    let AgentEvent::MessageAppended { message } = event else {
        return None;
    };
    if matches!(message, AgentMessage::Assistant { .. }) {
        Some(message.text())
    } else {
        None
    }
}
#[cfg(test)]
mod test_cases {

    use std::collections::BTreeMap;

    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, PermissionMode,
        PermissionOperation,
    };
    use neo_ai::{ApiKind, ChatMessage, ContentPart, ModelCapabilities, ModelSpec, ProviderId};

    use tracing_subscriber::prelude::*;

    use crate::config::{AppConfig, Defaults, McpConfig, McpTransport, RuntimeConfig, TuiConfig};

    fn sample_tool_approval_request(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            turn: 1,
            id: id.to_owned(),
            operation: PermissionOperation::Tool,
            presentation: ApprovalPresentation::Tool {
                title: "Run tool?".to_owned(),
                details: vec![format!("tool: {id}")],
            },
            options: vec![
                ApprovalOption {
                    label: "Approve once".to_owned(),
                    description: None,
                    action: ApprovalAction::PermitOnce,
                },
                ApprovalOption {
                    label: "Reject".to_owned(),
                    description: None,
                    action: ApprovalAction::Reject,
                },
            ],
            workflow_origin: None,
        }
    }

    fn sample_plan_approval_request(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            turn: 1,
            id: id.to_owned(),
            operation: PermissionOperation::PlanTransition,
            presentation: ApprovalPresentation::Plan {
                title: "Plan Review".to_owned(),
                path: None,
                markdown: "Ready".to_owned(),
                summary: Some("Ready".to_owned()),
            },
            options: vec![
                ApprovalOption {
                    label: "Approve".to_owned(),
                    description: None,
                    action: ApprovalAction::ApprovePlan { selection: None },
                },
                ApprovalOption {
                    label: "Reject with feedback".to_owned(),
                    description: None,
                    action: ApprovalAction::RevisePlan {
                        preset_feedback: None,
                    },
                },
            ],
            workflow_origin: None,
        }
    }

    fn fake_model() -> ModelSpec {
        ModelSpec {
            provider: ProviderId("test-provider".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::Local,
            capabilities: ModelCapabilities::tool_chat(),
        }
    }

    fn chat_message_text(message: &ChatMessage) -> String {
        let content = match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content,
        };
        content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::Thinking { .. } | ContentPart::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn test_config(project_dir: &std::path::Path) -> AppConfig {
        AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: project_dir.join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            workflow_runtime: neo_agent_core::workflow::WorkflowRuntime::new(
                neo_agent_core::workflow::WorkflowLimits::default(),
            ),
            workflow_definitions: neo_agent_core::workflow::WorkflowDefinitionRegistry::empty(),
            workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver::default(
            ),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            theme_resolution: crate::themes::ThemeResolution::Default,
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: project_dir.to_path_buf(),
            config_path: project_dir.join(".neo/config.toml"),
            config_file_exists: true,
        }
    }

    fn test_mcp_server(
        id: &str,
        transport: McpTransport,
        url: Option<&str>,
    ) -> crate::config::McpServerConfig {
        crate::config::McpServerConfig {
            id: id.to_owned(),
            enabled: true,
            transport,
            command: None,
            url: url.map(str::to_owned),
            args: Vec::new(),
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            startup_timeout_ms: None,
            tool_timeout_ms: None,
        }
    }

    #[path = "context.rs"]
    mod context;
    #[path = "context_mcp.rs"]
    mod context_mcp;
    #[path = "context_workflow.rs"]
    mod context_workflow;
    #[path = "output.rs"]
    mod output;
    #[path = "session.rs"]
    mod session;
    #[path = "stream.rs"]
    mod stream;
}
