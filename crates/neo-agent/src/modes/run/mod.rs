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
pub(crate) use mcp_cli::{add_mcp_server, auth_mcp_server, list_mcp};
pub(crate) use models_cli::list_configured_models;

// Re-export session helpers used within this module.
use session_mgmt::{
    create_session_path, latest_session_id, record_initial_session_title, record_session_activity,
    session_id_from_path, session_root_from_wire_path,
};

use std::{
    io::IsTerminal as _,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::goal::GoalManager;
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter, SessionEventPersistence};
use neo_agent_core::{
    AgentContext, AgentEvent, AgentMessage, AgentRuntime, AskUserTool, Content, CreateSkillTool,
    ListSkillsTool, McpConnectionManager, MessageOrigin, MoveSkillTool, PendingQuestion,
    PermissionApprovalDecision, SteerInputHandle, SummarizeSessionsTool,
    instructions::InstructionRegistry, mode::PlanMode, skills::SkillStoreHandle,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::RunOutput,
    config::{AppConfig, neo_home},
    modes::sessions,
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

pub struct PromptApprovalRequest {
    pub id: String,
    pub decision_tx: oneshot::Sender<PermissionApprovalDecision>,
    pub feedback_tx: Option<oneshot::Sender<Option<String>>>,
    /// Returns the model-supplied plan-review option label the user picked,
    /// when the approval was an `ExitPlanMode` plan-review approve choice.
    pub selected_label_tx: Option<oneshot::Sender<Option<String>>>,
    /// Display label for the session-approval option (Layer 1). `None` hides it.
    pub session_option_label: Option<String>,
    /// Display label for the prefix-approval option (Layer 2). `None` hides it.
    /// When the user picks the prefix option, the controller sets
    /// `prefix_rule` so the runtime persists the rule.
    pub prefix_option_label: Option<String>,
}

pub async fn run_prompt(prompt: &[String], config: &AppConfig) -> anyhow::Result<PromptTurn> {
    run_prompt_with_retry_notices(prompt, config, false).await
}

async fn run_prompt_with_retry_notices(
    prompt: &[String],
    config: &AppConfig,
    show_retry_notices: bool,
) -> anyhow::Result<PromptTurn> {
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(prompt_text.as_str())];
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let user_message = user_message(content, MessageOrigin::User);
    record_session_activity(config, &session_id, &prompt_text);
    let runtime = runtime_for_config(
        config,
        Some(session_root_from_wire_path(&session_path)?),
        None,
        None,
        None,
        None,
        false,
        SteerInputHandle::new(),
        None,
        Arc::new(Mutex::new(None)),
        None,
    )
    .await?;
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
    record_initial_session_title(config, &turn, &prompt_text).await;
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
    let user_message = user_message(content, MessageOrigin::User);
    let runtime = runtime_for_config(
        config,
        None,
        None,
        None,
        None,
        None,
        false,
        SteerInputHandle::new(),
        None,
        Arc::new(Mutex::new(None)),
        None,
    )
    .await?;
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
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let user_message = user_message(user_content, MessageOrigin::User);
    record_session_activity(config, session_id, &prompt_text);
    let runtime = runtime_for_config(
        config,
        Some(session_root_from_wire_path(&session_path)?),
        None,
        None,
        None,
        None,
        false,
        SteerInputHandle::new(),
        None,
        Arc::new(Mutex::new(None)),
        None,
    )
    .await?;
    runtime.restore_plan_mode(&context);
    finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        Vec::new(),
        session_id.to_owned(),
        show_retry_notices,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_prompt_streaming(
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
    skill_context: Option<String>,
    plan_review_feedback: Option<std::collections::BTreeMap<String, String>>,
    plan_mode: Option<Arc<RwLock<PlanMode>>>,
    goal_mode_authoring: bool,
    steer_input: SteerInputHandle,
    mcp_manager: Option<McpConnectionManager>,
    manual_compact_request: Arc<Mutex<Option<String>>>,
    instruction_registry: Option<Arc<InstructionRegistry>>,
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let prepared =
        prepare_new_streaming_turn(prompt, prompt_origin, config, session_id_tx, skill_context)
            .await?;
    let prompt = prepared.prompt.clone();
    let runtime = runtime_for_config(
        config,
        Some(prepared.session_directory.clone()),
        Some(approval_tx),
        question_tx,
        plan_review_feedback.clone(),
        plan_mode.clone(),
        goal_mode_authoring,
        steer_input,
        mcp_manager,
        manual_compact_request,
        instruction_registry,
    )
    .await?;
    let turn =
        run_prepared_streaming_turn(prepared, runtime, event_tx, cancel_token, compaction_only)
            .await?;
    record_initial_session_title(config, &turn, &prompt).await;
    Ok(turn)
}

#[allow(clippy::too_many_arguments)]
pub async fn run_prompt_in_session_streaming(
    session_id: &str,
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
    skill_context: Option<String>,
    plan_review_feedback: Option<std::collections::BTreeMap<String, String>>,
    plan_mode: Option<Arc<RwLock<PlanMode>>>,
    goal_mode_authoring: bool,
    steer_input: SteerInputHandle,
    mcp_manager: Option<McpConnectionManager>,
    manual_compact_request: Arc<Mutex<Option<String>>>,
    instruction_registry: Option<Arc<InstructionRegistry>>,
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let prepared = prepare_existing_streaming_turn(
        session_id,
        prompt,
        prompt_origin,
        config,
        session_id_tx,
        skill_context,
    )
    .await?;
    let runtime = runtime_for_config(
        config,
        Some(prepared.session_directory.clone()),
        Some(approval_tx),
        question_tx,
        plan_review_feedback.clone(),
        plan_mode.clone(),
        goal_mode_authoring,
        steer_input,
        mcp_manager,
        manual_compact_request,
        instruction_registry,
    )
    .await?;
    runtime.restore_plan_mode(&prepared.context);
    run_prepared_streaming_turn(prepared, runtime, event_tx, cancel_token, compaction_only).await
}

async fn prepare_new_streaming_turn(
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    config: &AppConfig,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    skill_context: Option<String>,
) -> anyhow::Result<PreparedStreamingTurn> {
    let prompt_text = prompt
        .iter()
        .filter_map(|c| c.as_text())
        .collect::<Vec<_>>()
        .join(" ");
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    send_streaming_session_id(session_id_tx, &session_id);
    let user_message = user_message(prompt.to_vec(), prompt_origin);
    record_session_activity(config, &session_id, &prompt_text);
    let session_directory = session_root_from_wire_path(&session_path)?;
    Ok(PreparedStreamingTurn {
        prompt: prompt_text,
        session_id,
        session_directory,
        context: streaming_context(skill_context),
        writer,
        user_message,
        initial_events: Vec::new(),
    })
}

async fn prepare_existing_streaming_turn(
    session_id: &str,
    prompt: &[Content],
    prompt_origin: MessageOrigin,
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
    let user_message = user_message(prompt.to_vec(), prompt_origin);
    record_session_activity(config, session_id, &prompt_text);
    Ok(PreparedStreamingTurn {
        prompt: prompt_text,
        session_id: session_id.to_owned(),
        session_directory,
        context,
        writer,
        user_message,
        initial_events: Vec::new(),
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
) -> anyhow::Result<PromptTurn> {
    let PreparedStreamingTurn {
        session_id,
        session_directory: _,
        context,
        mut writer,
        user_message,
        initial_events,
        prompt: _,
    } = prepared;
    let streaming = StreamingTurnIo {
        event_tx,
        session_id,
        cancel_token,
    };
    if compaction_only {
        finish_compaction_turn_streaming(context, &mut writer, runtime, initial_events, streaming)
            .await
    } else {
        finish_prompt_turn_streaming(
            user_message,
            context,
            &mut writer,
            runtime,
            initial_events,
            streaming,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
async fn runtime_for_config(
    config: &AppConfig,
    session_directory: Option<PathBuf>,
    approval_tx: Option<mpsc::UnboundedSender<PromptApprovalRequest>>,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
    plan_review_feedback: Option<std::collections::BTreeMap<String, String>>,
    plan_mode: Option<Arc<RwLock<PlanMode>>>,
    goal_mode_authoring: bool,
    steer_input: SteerInputHandle,
    mcp_manager: Option<McpConnectionManager>,
    manual_compact_request: Arc<Mutex<Option<String>>>,
    instruction_registry: Option<Arc<InstructionRegistry>>,
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
        approval_tx,
        &skill_store,
        instruction_registry,
    )?;
    if let Some(session_directory) = &session_directory {
        agent_config = agent_config.with_session_directory(session_directory.clone());
    }
    agent_config.manual_compact_request = manual_compact_request;
    if let Some(plan_mode) = plan_mode {
        agent_config = agent_config.with_plan_mode(plan_mode);
    }
    if goal_mode_authoring {
        agent_config = agent_config.with_goal_mode_authoring(true);
    }
    if let Some(feedback) = plan_review_feedback {
        // Merge into the existing map rather than replacing the entire Arc.
        // The async approval handler may have already inserted Revise feedback
        // for the current turn into this map via the side-channel. Replacing
        // the Arc here would discard that entry, causing permission.rs to
        // find an empty map and return "approval denied" instead of the
        // user's revision note.
        let mut map = agent_config
            .plan_review_feedback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.clear();
        map.extend(feedback);
    }
    let mut tools = runtime::tool_registry_for_config(
        config,
        std::sync::Arc::clone(&agent_config.todos),
        mcp_manager.as_ref(),
    )
    .await?;
    if let Some(question_tx) = question_tx {
        tools.register(AskUserTool::new(question_tx));
    }
    let extra_skill_paths: Vec<PathBuf> =
        config.extra_skill_dirs.iter().map(PathBuf::from).collect();
    tools.register(ListSkillsTool::new(neo_home(), extra_skill_paths));
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
    let mut runtime =
        AgentRuntime::with_tools_and_skill_handle(agent_config, client, tools, skill_store_handle);
    runtime = runtime.with_steer_input(steer_input);
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

#[cfg(test)]
async fn run_prompt_with_runtime(
    prompt: String,
    context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
) -> anyhow::Result<PromptTurn> {
    let mut writer = SessionEventWriter::jsonl(writer);
    let user_message = user_message(vec![Content::text(prompt)], MessageOrigin::User);
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

fn user_message(content: Vec<Content>, origin: MessageOrigin) -> AgentMessage {
    AgentMessage::User { content, origin }
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
    let mut turn_stream = runtime.run_turn(&mut context, user_message.clone());
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

struct PreparedStreamingTurn {
    prompt: String,
    session_id: String,
    session_directory: PathBuf,
    context: AgentContext,
    writer: JsonlSessionWriter,
    user_message: AgentMessage,
    initial_events: Vec<AgentEvent>,
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
    initial_events: Vec<AgentEvent>,
    streaming: StreamingTurnIo,
) -> anyhow::Result<PromptTurn> {
    let mut events = forward_initial_streaming_events(&streaming.event_tx, initial_events);
    let mut assistant_text = String::new();
    let mut persistence = SessionEventPersistence::default();
    let mut stream =
        runtime.run_turn_with_cancel(&mut context, user_message.clone(), streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = streaming_event_or_bail(event, &streaming.event_tx)?;
        append_streaming_event(
            &event,
            writer,
            &mut assistant_text,
            &streaming.event_tx,
            &mut events,
            &mut persistence,
        )
        .await?;
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id: streaming.session_id,
        events,
        assistant_text,
    })
}

async fn finish_compaction_turn_streaming(
    mut context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
    initial_events: Vec<AgentEvent>,
    streaming: StreamingTurnIo,
) -> anyhow::Result<PromptTurn> {
    let mut events = forward_initial_streaming_events(&streaming.event_tx, initial_events);
    let mut stream =
        runtime.run_manual_compaction_turn_with_cancel(&mut context, streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = streaming_event_or_bail(event, &streaming.event_tx)?;
        writer.append_event(&event).await?;
        let _ = streaming.event_tx.send(Ok(event.clone()));
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id: streaming.session_id,
        events,
        assistant_text: String::new(),
    })
}

fn forward_initial_streaming_events(
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    initial_events: Vec<AgentEvent>,
) -> Vec<AgentEvent> {
    for event in &initial_events {
        let _ = event_tx.send(Ok(event.clone()));
    }
    initial_events
}

fn streaming_event_or_bail<E: std::fmt::Display>(
    event: Result<AgentEvent, E>,
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
) -> anyhow::Result<AgentEvent> {
    event.map_err(|error| {
        let message = error.to_string();
        let _ = event_tx.send(Err(anyhow::anyhow!(message.clone())));
        anyhow::anyhow!(message)
    })
}

async fn append_streaming_event(
    event: &AgentEvent,
    writer: &mut JsonlSessionWriter,
    assistant_text: &mut String,
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    events: &mut Vec<AgentEvent>,
    persistence: &mut SessionEventPersistence,
) -> anyhow::Result<()> {
    let effect = streaming_event_effect(event);
    if let Some(text) = effect.assistant_text {
        assistant_text.push_str(&text);
    }
    if effect.persist {
        for persisted in persistence.persisted_events(event) {
            writer.append_event(&persisted).await?;
        }
    }
    if effect.forward {
        let _ = event_tx.send(Ok(event.clone()));
        events.push(event.clone());
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
mod tests {
    use std::{collections::BTreeMap, path::Path, sync::Arc};

    use neo_agent_core::instructions::{InstructionRegistry, InstructionRegistryConfig};
    use neo_agent_core::{
        AgentConfig, AgentContext, AgentEvent, AgentMessage, ApprovalRequest, CompactionSettings,
        Content, McpConnectionManager, MessageOrigin, PermissionApprovalDecision, PermissionMode,
        PermissionOperation, ProcessSupervisor, QueueMode, StopReason as AgentStopReason,
        ToolExecutionMode, ToolRegistry,
        harness::FakeHarness,
        session::{JsonlSessionReader, JsonlSessionWriter},
        skills::SkillStore,
    };
    use neo_ai::{
        AiStreamEvent, ApiKind, ApiType, ChatMessage, ContentPart, ModelCapabilities, ModelSpec,
        ProviderId, StopReason, providers::fake::FakeModelClient,
    };
    use tracing_subscriber::prelude::*;

    use super::mcp_cli::auth_mcp_server;
    use super::models_cli::list_configured_models;
    use super::runtime::{
        agent_config_for_app, model_registry_for_config, select_config_model,
        tool_registry_for_config,
    };
    use super::session_mgmt::{
        create_session_path, latest_session_id, session_id_from_path, session_root_from_wire_path,
    };
    use super::{PromptApprovalRequest, run_prompt_with_runtime, runtime_for_config, user_message};
    use crate::config::{
        AppConfig, Defaults, McpConfig, McpTransport, ModelConfig, ProviderConfig,
        RuntimeCompactionConfig, RuntimeConfig, RuntimeRetryConfig, TuiConfig,
    };

    #[test]
    fn agent_config_for_app_applies_runtime_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: Some(0.35),
                max_tokens: Some(512),
                reasoning: neo_ai::ReasoningSelection::Effort {
                    effort: neo_ai::ReasoningEffort::high(),
                },
                replay_reasoning: true,
                steering_queue_mode: QueueMode::OneAtATime,
                follow_up_queue_mode: QueueMode::OneAtATime,
                tool_execution_mode: ToolExecutionMode::Sequential,
                retry: RuntimeRetryConfig {
                    max_retries: 100,
                    first_event_timeout_secs: 7,
                    stream_idle_timeout_secs: 11,
                },
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 16_000,
                    keep_recent_messages: 24,
                    trigger_ratio: 0.85,
                    reserved_context_tokens: 50_000,
                    max_recent_messages: 4,
                    micro_enabled: false,
                    micro_keep_recent: 20,
                    max_rounds: 5,
                    max_retry_attempts: 5,
                }),
                ..RuntimeConfig::default()
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
            config_file_exists: true,
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponse,
            capabilities: ModelCapabilities::tool_chat(),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store, None).expect("agent config");

        assert_eq!(agent_config.temperature, Some(0.35));
        assert_eq!(agent_config.max_tokens, Some(512));
        assert_eq!(agent_config.max_retries, 100);
        assert_eq!(agent_config.first_event_timeout_secs, 7);
        assert_eq!(agent_config.stream_idle_timeout_secs, 11);
        assert_eq!(
            agent_config.reasoning,
            neo_ai::ReasoningSelection::Effort {
                effort: neo_ai::ReasoningEffort::high(),
            }
        );
        assert_eq!(agent_config.steering_queue_mode, QueueMode::OneAtATime);
        assert_eq!(agent_config.follow_up_queue_mode, QueueMode::OneAtATime);
        assert_eq!(
            agent_config.tool_execution_mode,
            ToolExecutionMode::Sequential
        );
        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 16_000,
                keep_recent_messages: 24,
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
                max_rounds: 5,
                max_retry_attempts: 5,
            })
        );
        assert!(agent_config.workspace_root.is_some());
        assert!(
            agent_config.instruction_registry.is_some(),
            "production agent config must enable path-scoped AGENTS instructions"
        );
    }

    #[test]
    fn agent_config_for_app_falls_back_to_model_max_output_tokens() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning: neo_ai::ReasoningSelection::Off,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::OneAtATime,
                follow_up_queue_mode: QueueMode::OneAtATime,
                tool_execution_mode: ToolExecutionMode::Sequential,
                compaction: None,
                ..RuntimeConfig::default()
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
            config_file_exists: true,
        };
        // Model declares max_output_tokens; runtime does not override.
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponse,
            capabilities: ModelCapabilities::tool_chat().with_max_output_tokens(64_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store, None).expect("agent config");

        assert_eq!(agent_config.max_tokens, Some(64_000));
    }

    #[test]
    fn create_session_path_uses_named_uuid_session_ids() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
            config_file_exists: true,
        };

        let neo_home = temp.path().join("neo-home");
        temp_env::with_vars([("NEO_HOME", Some(neo_home.as_os_str()))], || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build test runtime");
            runtime.block_on(async {
                let path = create_session_path(&config)
                    .await
                    .expect("session path is created");
                let session_dir = path
                    .parent()
                    .and_then(std::path::Path::parent)
                    .and_then(std::path::Path::parent)
                    .expect("session directory");
                let session_id = session_dir
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .expect("session id");

                assert!(session_id.starts_with("session_"));
                assert_eq!(session_id.len(), "session_".len() + 36);
                assert!(neo_agent_core::session::validate_session_id(session_id).is_ok());
                let indexed = neo_agent_core::session::SessionIndex::new(&neo_home)
                    .find(session_id)
                    .expect("read session index")
                    .expect("run session should be indexed");
                assert_eq!(
                    indexed.session_dir,
                    crate::config::workspace_sessions_dir(&config)
                );
                assert_eq!(indexed.workdir, config.project_dir);
                assert!(
                    path.ends_with(
                        std::path::Path::new("agents")
                            .join("main")
                            .join("wire.jsonl")
                    )
                );
                assert!(session_dir.join("state.json").is_file());
            });
        });
    }

    #[test]
    fn user_message_preserves_injection_origin() {
        let origin = MessageOrigin::injection("init");

        let message = user_message(
            vec![Content::text("<system-reminder>\ninit\n</system-reminder>")],
            origin.clone(),
        );

        assert!(message.is_injection());
        assert_eq!(
            message,
            AgentMessage::User {
                content: vec![Content::text("<system-reminder>\ninit\n</system-reminder>")],
                origin,
            }
        );
    }

    #[tokio::test]
    async fn session_root_from_wire_path_returns_session_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
            config_file_exists: true,
        };

        let wire_path = create_session_path(&config)
            .await
            .expect("session path is created");
        let session_root =
            session_root_from_wire_path(&wire_path).expect("session root from wire path");

        assert_eq!(
            neo_agent_core::session::main_agent_wire_path(&session_root),
            wire_path
        );
        assert_eq!(
            session_root.file_name().and_then(std::ffi::OsStr::to_str),
            session_id_from_path(&wire_path).ok().as_deref()
        );
    }

    #[test]
    fn latest_session_id_ignores_main_wire_directories() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
            config_file_exists: true,
        };
        let bucket_dir = crate::config::workspace_sessions_dir(&config);
        let valid_id = "session_00000000-0000-4000-8000-000000000001";
        let directory_wire_id = "session_00000000-0000-4000-8000-000000000999";
        let valid_wire = neo_agent_core::session::main_agent_wire_path(&bucket_dir.join(valid_id));
        std::fs::create_dir_all(valid_wire.parent().expect("valid wire parent"))
            .expect("create valid wire parent");
        std::fs::write(valid_wire, "{}\n").expect("write valid wire");
        std::fs::create_dir_all(neo_agent_core::session::main_agent_wire_path(
            &bucket_dir.join(directory_wire_id),
        ))
        .expect("create directory wire");

        assert_eq!(
            latest_session_id(&config).expect("latest session"),
            valid_id
        );
    }

    #[test]
    fn agent_config_for_app_scales_default_compaction_to_model_context_window() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "large-context-model".to_owned(),
            default_provider: "anthropic".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning: neo_ai::ReasoningSelection::Off,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::All,
                follow_up_queue_mode: QueueMode::All,
                tool_execution_mode: ToolExecutionMode::Parallel,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 32_000,
                    keep_recent_messages: 20,
                    trigger_ratio: 0.85,
                    reserved_context_tokens: 50_000,
                    max_recent_messages: 4,
                    micro_enabled: false,
                    micro_keep_recent: 20,
                    max_rounds: 5,
                    max_retry_attempts: 5,
                }),
                ..RuntimeConfig::default()
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
            config_file_exists: true,
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store, None).expect("agent config");

        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 800_000,
                keep_recent_messages: 20,
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
                max_rounds: 5,
                max_retry_attempts: 5,
            })
        );
    }

    #[test]
    fn agent_config_for_app_keeps_explicit_custom_compaction_threshold() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "large-context-model".to_owned(),
            default_provider: "anthropic".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                PermissionMode::default(),
            )),
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning: neo_ai::ReasoningSelection::Off,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::All,
                follow_up_queue_mode: QueueMode::All,
                tool_execution_mode: ToolExecutionMode::Parallel,
                compaction: Some(RuntimeCompactionConfig {
                    enabled: true,
                    max_estimated_tokens: 12_000,
                    keep_recent_messages: 16,
                    trigger_ratio: 0.85,
                    reserved_context_tokens: 50_000,
                    max_recent_messages: 4,
                    micro_enabled: false,
                    micro_keep_recent: 20,
                    max_rounds: 5,
                    max_retry_attempts: 5,
                }),
                ..RuntimeConfig::default()
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
            config_file_exists: true,
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store, None).expect("agent config");

        assert_eq!(
            agent_config.compaction,
            Some(CompactionSettings {
                enabled: true,
                max_estimated_tokens: 12_000,
                keep_recent_messages: 16,
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
                max_rounds: 5,
                max_retry_attempts: 5,
            })
        );
    }

    #[tokio::test]
    async fn agent_config_for_app_async_approval_channel_waits_for_ui_decision() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: temp.path().join(".neo/sessions"),
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
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
            config_file_exists: true,
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponse,
            capabilities: ModelCapabilities::tool_chat(),
        };
        let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, Some(approval_tx), &skill_store, None)
                .expect("agent config");
        let handler = agent_config
            .async_approval_handler
            .expect("async approval handler");

        let decision = tokio::spawn(handler(ApprovalRequest {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: PermissionOperation::Tool,
            subject: "Write".to_owned(),
            arguments: serde_json::json!({"path": "approved.txt"}),
            session_scope: None,
            prefix_rule: None,
            suggestions: Vec::new(),
        }));
        let PromptApprovalRequest {
            id,
            decision_tx,
            feedback_tx: _,
            selected_label_tx: _,
            session_option_label: _,
            prefix_option_label: _,
        } = approval_rx.recv().await.expect("approval waiter");

        assert_eq!(id, "tool-1");
        decision_tx
            .send(PermissionApprovalDecision::AllowOnce)
            .expect("send decision");
        assert_eq!(
            decision.await.expect("approval task joins"),
            PermissionApprovalDecision::AllowOnce
        );
    }

    #[tokio::test]
    async fn async_approval_handler_stores_plan_revision_feedback_in_current_config_map() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "test-provider".to_owned();
        config.default_model = "test-model".to_owned();
        config.providers.insert(
            "test-provider".to_owned(),
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
                base_url: Some("https://example.test/v1".to_owned()),
                api_key: Some("test-key".to_owned()),
                api_key_env: None,
            },
        );
        config.models.insert(
            "test-model".to_owned(),
            ModelConfig {
                provider: "test-provider".to_owned(),
                model: "test-model".to_owned(),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                ..ModelConfig::default()
            },
        );
        let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = runtime_for_config(
            &config,
            None,
            Some(approval_tx),
            None,
            Some(BTreeMap::new()),
            None,
            false,
            neo_agent_core::SteerInputHandle::new(),
            None,
            Arc::new(std::sync::Mutex::new(None)),
            None,
        )
        .await
        .expect("runtime");
        let current_feedback = Arc::clone(&runtime.config().plan_review_feedback);
        let handler = runtime
            .config()
            .async_approval_handler
            .clone()
            .expect("async approval handler");

        let decision = tokio::spawn(handler(ApprovalRequest {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: PermissionOperation::PlanTransition,
            subject: "Exit plan mode".to_owned(),
            arguments: serde_json::json!({"plan_summary": "Ready"}),
            session_scope: None,
            prefix_rule: None,
            suggestions: Vec::new(),
        }));
        let PromptApprovalRequest {
            id,
            decision_tx,
            feedback_tx,
            selected_label_tx: _,
            session_option_label: _,
            prefix_option_label: _,
        } = approval_rx.recv().await.expect("approval waiter");

        assert_eq!(id, "tool-1");
        feedback_tx
            .expect("feedback channel")
            .send(Some("tighten the implementation scope".to_owned()))
            .expect("send feedback");
        decision_tx
            .send(PermissionApprovalDecision::Reject)
            .expect("send decision");

        assert_eq!(
            decision.await.expect("approval task joins"),
            PermissionApprovalDecision::Reject
        );
        assert_eq!(
            current_feedback
                .lock()
                .expect("feedback lock")
                .get("tool-1")
                .map(String::as_str),
            Some("tighten the implementation scope")
        );
    }

    #[tokio::test]
    async fn run_prompt_with_runtime_appends_continuation_to_existing_session_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = temp
            .path()
            .join("session_00000000-0000-4000-8000-000000000501");
        let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
        tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
            .await
            .expect("create wire dir");
        let mut seed = JsonlSessionWriter::create(&session_path)
            .await
            .expect("create session");
        seed.append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("hello"),
        })
        .await
        .expect("append user");
        seed.append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("hi back")],
                Vec::new(),
                AgentStopReason::EndTurn,
            ),
        })
        .await
        .expect("append assistant");
        seed.append_event(&AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: AgentStopReason::EndTurn,
        })
        .await
        .expect("append turn finish");
        seed.flush().await.expect("flush seed");

        let context = JsonlSessionReader::replay_context(&session_path)
            .await
            .expect("replay context");
        let fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "continued answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let runtime =
            super::AgentRuntime::new(AgentConfig::for_model(fake_model()), Arc::new(fake.clone()));
        let mut writer = JsonlSessionWriter::open_append(&session_path)
            .await
            .expect("append session");

        let turn = run_prompt_with_runtime("continue".to_owned(), context, &mut writer, runtime)
            .await
            .expect("run continuation");

        assert_eq!(turn.assistant_text, "continued answer");
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let contents = requests[0]
            .messages
            .iter()
            .map(chat_message_text)
            .collect::<Vec<_>>();
        assert_eq!(contents, vec!["hello", "hi back", "continue"]);

        let messages = JsonlSessionReader::replay_messages(&session_path)
            .await
            .expect("replay appended messages");
        assert_eq!(messages.len(), 4);
        assert!(matches!(
            &messages[2],
            AgentMessage::User { content, .. } if content[0].as_text() == Some("continue")
        ));
        assert!(matches!(
            &messages[3],
            AgentMessage::Assistant { content, .. }
                if content[0].as_text() == Some("continued answer")
        ));
    }

    #[tokio::test]
    async fn startup_builds_one_registry_and_baseline_before_first_provider_request() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("AGENTS.md"), "root rules\n").expect("AGENTS.md");
        let session_dir = temp
            .path()
            .join("session_00000000-0000-4000-8000-000000000502");
        let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
        tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
            .await
            .expect("wire dir");
        let registry = Arc::new(
            InstructionRegistry::new(InstructionRegistryConfig {
                primary_workspace: workspace.clone(),
                neo_home: None,
                project_trusted: true,
            })
            .expect("registry"),
        );
        let fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut config = AgentConfig::for_model(fake_model())
            .with_workspace_root(&workspace)
            .expect("workspace root");
        config.instruction_registry = Some(registry);
        let runtime = super::AgentRuntime::new(config, Arc::new(fake.clone()));
        let mut writer = JsonlSessionWriter::create(&session_path)
            .await
            .expect("session writer");

        run_prompt_with_runtime(
            "first prompt".to_owned(),
            AgentContext::new(),
            &mut writer,
            runtime,
        )
        .await
        .expect("run prompt");

        let events = JsonlSessionReader::read_all(&session_path)
            .await
            .expect("read events");
        let epoch = events
            .iter()
            .position(|event| matches!(event, AgentEvent::InstructionEpoch { .. }))
            .expect("instruction epoch");
        let user = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageAppended {
                        message: AgentMessage::User { .. }
                    }
                )
            })
            .expect("user event");
        assert!(
            epoch < user,
            "persisted baseline must precede user: {events:?}"
        );
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let request_text = requests[0]
            .messages
            .iter()
            .map(chat_message_text)
            .collect::<Vec<_>>();
        let rules = request_text
            .iter()
            .position(|text| text.contains("root rules"))
            .expect("baseline rules in first provider request");
        let prompt = request_text
            .iter()
            .position(|text| text == "first prompt")
            .expect("first prompt in provider request");
        assert!(rules < prompt, "request order: {request_text:?}");
    }

    #[test]
    fn stable_json_redacts_instruction_metadata_paths_and_failure_detail() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let neo_home = temp.path().join("neo-home");
        let outside = temp.path().join("private/rules.md");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&neo_home).expect("neo home");
        let workspace = workspace.canonicalize().expect("canonical workspace");
        let neo_home = neo_home.canonicalize().expect("canonical neo home");
        #[cfg(unix)]
        let configured_neo_home = {
            let link = temp.path().join("neo-home-link");
            std::os::unix::fs::symlink(&neo_home, &link).expect("neo home symlink");
            link
        };
        #[cfg(not(unix))]
        let configured_neo_home = neo_home.clone();
        let nested = workspace.join("crates/neo-tui");
        let epoch = neo_agent_core::instructions::InstructionEpochData {
            agent_id: "main".to_owned(),
            generation: 7,
            outcome: neo_agent_core::instructions::InstructionEpochOutcome::Blocked,
            scopes: vec![neo_agent_core::instructions::InstructionScopeData {
                display_path: nested.clone(),
                kind: neo_agent_core::instructions::InstructionScopeKind::Nested,
                revision: Some("7af13c2e".to_owned()),
                token_estimate: 1_024,
            }],
            selected_bundles: vec![neo_agent_core::instructions::InstructionBundleMetadata {
                display_path: nested,
                revision: "7af13c2e".to_owned(),
                token_estimate: 1_024,
                byte_size: 4_096,
                source_count: 2,
                import_count: 2,
                import_paths: vec![neo_home.join("CX.md"), outside.clone()],
            }],
            ignored_bundles: Vec::new(),
            replacements: Vec::new(),
            failure: Some(neo_agent_core::instructions::InstructionFailure {
                fingerprint: "failure-fingerprint".to_owned(),
                display_path: outside,
                kind: neo_agent_core::instructions::InstructionFailureKind::MissingImport,
                detail: "PRIVATE FAILURE DETAIL".to_owned(),
            }),
            deferred_tool_ids: vec!["call-1".to_owned()],
            budget: neo_agent_core::instructions::InstructionBudget {
                nominal: 65_536,
                actual: 65_536,
            },
            model_content: Some("SECRET INSTRUCTION BODY".to_owned()),
        };
        let turn = super::PromptTurn {
            session_id: "session_00000000-0000-4000-8000-000000000607".to_owned(),
            events: vec![AgentEvent::InstructionEpoch { epoch }],
            assistant_text: String::new(),
        };
        let config = test_config(&workspace);

        let output = temp_env::with_var("NEO_HOME", Some(configured_neo_home.as_os_str()), || {
            super::output::stable_json_output(&turn, &config).expect("stable JSON")
        });
        let record = output
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON line"))
            .find(|value| value["type"] == "instruction_epoch")
            .expect("instruction epoch record");
        let encoded = record.to_string();

        assert_eq!(
            record["scopes"][0]["display_path"],
            Path::new("crates").join("neo-tui").display().to_string()
        );
        assert_eq!(
            record["selectedBundles"][0]["import_paths"],
            serde_json::json!([
                Path::new("$NEO_HOME").join("CX.md").display().to_string(),
                "<outside-workspace>"
            ])
        );
        assert_eq!(record["failure"]["display_path"], "<outside-workspace>");
        assert!(record["failure"].get("detail").is_none(), "{record}");
        for secret in [
            temp.path().display().to_string(),
            "PRIVATE FAILURE DETAIL".to_owned(),
            "SECRET INSTRUCTION BODY".to_owned(),
        ] {
            assert!(!encoded.contains(&secret), "leaked {secret}: {encoded}");
        }
    }

    #[test]
    fn events_output_projects_instruction_epoch_to_display_safe_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("private/rules.md");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let canonical_workspace = workspace.canonicalize().expect("canonical workspace");
        let epoch = neo_agent_core::instructions::InstructionEpochData {
            agent_id: "main".to_owned(),
            generation: 9,
            outcome: neo_agent_core::instructions::InstructionEpochOutcome::Blocked,
            scopes: vec![neo_agent_core::instructions::InstructionScopeData {
                display_path: canonical_workspace.clone(),
                kind: neo_agent_core::instructions::InstructionScopeKind::WorkspaceRoot,
                revision: None,
                token_estimate: 0,
            }],
            selected_bundles: Vec::new(),
            ignored_bundles: Vec::new(),
            replacements: Vec::new(),
            failure: Some(neo_agent_core::instructions::InstructionFailure {
                fingerprint: "failure-fingerprint".to_owned(),
                display_path: outside,
                kind: neo_agent_core::instructions::InstructionFailureKind::MissingImport,
                detail: "PRIVATE FAILURE DETAIL".to_owned(),
            }),
            deferred_tool_ids: vec!["call-1".to_owned()],
            budget: neo_agent_core::instructions::InstructionBudget {
                nominal: 65_536,
                actual: 65_536,
            },
            model_content: Some("SECRET INSTRUCTION BODY".to_owned()),
        };
        let turn = super::PromptTurn {
            session_id: "session_00000000-0000-4000-8000-000000000608".to_owned(),
            events: vec![AgentEvent::InstructionEpoch { epoch }],
            assistant_text: String::new(),
        };
        let config = test_config(&workspace);

        let output = super::events_output(&turn, &config).expect("events output");
        let record: serde_json::Value = serde_json::from_str(output.trim()).expect("event JSON");
        let encoded = record.to_string();

        assert_eq!(record["type"], "instruction_epoch");
        assert_eq!(record["scopes"][0]["display_path"], ".");
        assert_eq!(record["failure"]["display_path"], "<outside-workspace>");
        assert!(record["failure"].get("detail").is_none(), "{record}");
        assert!(!encoded.contains("PRIVATE FAILURE DETAIL"), "{encoded}");
        assert!(!encoded.contains("SECRET INSTRUCTION BODY"), "{encoded}");
        assert!(
            !encoded.contains(&temp.path().display().to_string()),
            "{encoded}"
        );
    }

    #[tokio::test]
    async fn unchanged_resume_replays_epoch_without_duplicate_message_or_card() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("AGENTS.md"), "rules v1\n").expect("AGENTS.md");
        let session_dir = temp
            .path()
            .join("session_00000000-0000-4000-8000-000000000504");
        let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
        tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
            .await
            .expect("wire dir");
        let registry = Arc::new(
            InstructionRegistry::new(InstructionRegistryConfig {
                primary_workspace: workspace.clone(),
                neo_home: None,
                project_trusted: true,
            })
            .expect("registry"),
        );
        let first_fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-1".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut first_config = AgentConfig::for_model(fake_model())
            .with_workspace_root(&workspace)
            .expect("workspace root");
        first_config.instruction_registry = Some(Arc::clone(&registry));
        let first_runtime = super::AgentRuntime::new(first_config, Arc::new(first_fake));
        let mut writer = JsonlSessionWriter::create(&session_path)
            .await
            .expect("session writer");
        run_prompt_with_runtime(
            "first prompt".to_owned(),
            AgentContext::new(),
            &mut writer,
            first_runtime,
        )
        .await
        .expect("first turn");
        drop(writer);

        let context = JsonlSessionReader::replay_context(&session_path)
            .await
            .expect("replay context");
        let resumed_fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-2".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut resumed_config = AgentConfig::for_model(fake_model())
            .with_workspace_root(&workspace)
            .expect("workspace root");
        resumed_config.instruction_registry = Some(registry);
        let resumed_runtime =
            super::AgentRuntime::new(resumed_config, Arc::new(resumed_fake.clone()));
        let mut writer = JsonlSessionWriter::open_append(&session_path)
            .await
            .expect("append session");

        let turn = run_prompt_with_runtime(
            "unchanged".to_owned(),
            context,
            &mut writer,
            resumed_runtime,
        )
        .await
        .expect("unchanged resumed turn");

        assert!(
            turn.events
                .iter()
                .all(|event| !matches!(event, AgentEvent::InstructionEpoch { .. })),
            "unchanged resume emitted duplicate epoch: {:?}",
            turn.events,
        );
        let requests = resumed_fake.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .messages
                .iter()
                .map(chat_message_text)
                .filter(|text| text.contains("rules v1"))
                .count(),
            1,
            "unchanged resume must replay one instruction snapshot",
        );
    }

    #[tokio::test]
    async fn changed_source_after_resume_appends_replacement_before_provider_call() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let agents_path = workspace.join("AGENTS.md");
        std::fs::write(&agents_path, "rules v1\n").expect("AGENTS.md v1");
        let session_dir = temp
            .path()
            .join("session_00000000-0000-4000-8000-000000000503");
        let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
        tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
            .await
            .expect("wire dir");
        let registry = Arc::new(
            InstructionRegistry::new(InstructionRegistryConfig {
                primary_workspace: workspace.clone(),
                neo_home: None,
                project_trusted: true,
            })
            .expect("registry"),
        );
        let first_fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-1".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut first_config = AgentConfig::for_model(fake_model())
            .with_workspace_root(&workspace)
            .expect("workspace root");
        first_config.instruction_registry = Some(Arc::clone(&registry));
        let first_runtime = super::AgentRuntime::new(first_config, Arc::new(first_fake));
        let mut writer = JsonlSessionWriter::create(&session_path)
            .await
            .expect("session writer");
        run_prompt_with_runtime(
            "first prompt".to_owned(),
            AgentContext::new(),
            &mut writer,
            first_runtime,
        )
        .await
        .expect("first turn");
        drop(writer);

        std::fs::write(&agents_path, "rules v2\n").expect("AGENTS.md v2");
        let context = JsonlSessionReader::replay_context(&session_path)
            .await
            .expect("replay updated context");
        let resumed_fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-3".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut resumed_config = AgentConfig::for_model(fake_model())
            .with_workspace_root(&workspace)
            .expect("workspace root");
        resumed_config.instruction_registry = Some(Arc::clone(&registry));
        let resumed_runtime =
            super::AgentRuntime::new(resumed_config, Arc::new(resumed_fake.clone()));
        let mut writer = JsonlSessionWriter::open_append(&session_path)
            .await
            .expect("append session");

        let turn =
            run_prompt_with_runtime("continue".to_owned(), context, &mut writer, resumed_runtime)
                .await
                .expect("resumed turn");

        let updated = turn
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::InstructionEpoch { epoch }
                        if epoch.outcome
                            == neo_agent_core::instructions::InstructionEpochOutcome::Updated
                )
            })
            .expect("updated instruction epoch");
        let user = turn
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageAppended {
                        message: AgentMessage::User { .. }
                    }
                )
            })
            .expect("resumed user event");
        assert!(
            updated < user,
            "replacement must precede user: {:?}",
            turn.events
        );
        let requests = resumed_fake.requests();
        assert_eq!(requests.len(), 1);
        let request_text = requests[0]
            .messages
            .iter()
            .map(chat_message_text)
            .collect::<Vec<_>>();
        let v2 = request_text
            .iter()
            .position(|text| text.contains("rules v2"))
            .expect("updated rules in first resumed request");
        let prompt = request_text
            .iter()
            .position(|text| text == "continue")
            .expect("resumed prompt");
        assert!(v2 < prompt, "request order: {request_text:?}");
        drop(writer);

        std::fs::remove_file(&agents_path).expect("remove AGENTS.md");
        let context = JsonlSessionReader::replay_context(&session_path)
            .await
            .expect("replay removed context");
        let removed_fake = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-4".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut removed_config = AgentConfig::for_model(fake_model())
            .with_workspace_root(&workspace)
            .expect("workspace root");
        removed_config.instruction_registry = Some(registry);
        let removed_runtime =
            super::AgentRuntime::new(removed_config, Arc::new(removed_fake.clone()));
        let mut writer = JsonlSessionWriter::open_append(&session_path)
            .await
            .expect("append removed session");

        let turn = run_prompt_with_runtime(
            "after removal".to_owned(),
            context,
            &mut writer,
            removed_runtime,
        )
        .await
        .expect("removed resumed turn");

        let removed = turn
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::InstructionEpoch { epoch }
                        if epoch.outcome
                            == neo_agent_core::instructions::InstructionEpochOutcome::Removed
                )
            })
            .expect("removed instruction epoch");
        let user = turn
            .events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    AgentEvent::MessageAppended {
                        message: AgentMessage::User { .. }
                    }
                )
            })
            .expect("removed resume user event");
        assert!(
            removed < user,
            "removal must precede user: {:?}",
            turn.events
        );
        let requests = removed_fake.requests();
        assert_eq!(requests.len(), 1);
        let request_text = requests[0]
            .messages
            .iter()
            .map(chat_message_text)
            .collect::<Vec<_>>();
        let v1 = request_text
            .iter()
            .position(|text| text.contains("rules v1"))
            .expect("historical v1 instruction snapshot");
        let v2 = request_text
            .iter()
            .position(|text| text.contains("rules v2"))
            .expect("historical v2 instruction snapshot");
        let empty_authority = request_text
            .iter()
            .rposition(|text| {
                text.contains("No path-scoped instruction bundles are currently active.")
            })
            .expect("removed authority snapshot");
        let prompt = request_text
            .iter()
            .position(|text| text == "after removal")
            .expect("removed resume prompt");
        assert!(
            v1 < v2 && v2 < empty_authority && empty_authority < prompt,
            "append-only authority order: {request_text:?}",
        );
    }

    #[tokio::test]
    async fn nested_scope_import_and_over_budget_warning_replan_without_breaking_turn() {
        const LOADED: &str = "NESTED-IMPORT-LOADED-7a31";
        const IGNORED: &str = "ROOT-BUNDLE-IGNORED-8c42";
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let nested = workspace.join("nested");
        std::fs::create_dir_all(&nested).expect("nested workspace");
        std::fs::write(
            workspace.join("AGENTS.md"),
            format!("{IGNORED}\n{}", "large root rules ".repeat(6_000)),
        )
        .expect("root AGENTS.md");
        std::fs::write(nested.join("AGENTS.md"), "@./imported.md\n").expect("nested AGENTS.md");
        std::fs::write(nested.join("imported.md"), format!("{LOADED}\n")).expect("nested import");
        std::fs::write(nested.join("data.txt"), "nested data\n").expect("nested data");
        let session_dir = temp
            .path()
            .join("session_00000000-0000-4000-8000-000000000505");
        let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
        tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
            .await
            .expect("wire dir");
        let registry = Arc::new(
            InstructionRegistry::new(InstructionRegistryConfig {
                primary_workspace: workspace.clone(),
                neo_home: None,
                project_trusted: true,
            })
            .expect("registry"),
        );
        let read_arguments = serde_json::json!({ "path": "nested/data.txt" }).to_string();
        let harness = FakeHarness::from_turns([
            vec![
                AiStreamEvent::MessageStart {
                    id: "msg-1".to_owned(),
                },
                AiStreamEvent::ToolCallStart {
                    id: "call-1".to_owned(),
                    name: "Read".to_owned(),
                },
                AiStreamEvent::ToolCallEnd {
                    id: "call-1".to_owned(),
                    raw_arguments: read_arguments.clone(),
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                },
            ],
            vec![
                AiStreamEvent::MessageStart {
                    id: "msg-2".to_owned(),
                },
                AiStreamEvent::ToolCallStart {
                    id: "call-2".to_owned(),
                    name: "Read".to_owned(),
                },
                AiStreamEvent::ToolCallEnd {
                    id: "call-2".to_owned(),
                    raw_arguments: read_arguments,
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                },
            ],
            vec![
                AiStreamEvent::MessageStart {
                    id: "msg-3".to_owned(),
                },
                AiStreamEvent::TextDelta {
                    text: "done".to_owned(),
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
        ]);
        let mut model = harness.model();
        model.capabilities.max_context_tokens = Some(32_768);
        let mut config = AgentConfig::for_model(model)
            .with_workspace_root(&workspace)
            .expect("workspace root");
        config.max_tokens = Some(1_024);
        config.instruction_registry = Some(registry);
        let runtime = super::AgentRuntime::with_tools(
            config,
            harness.client(),
            ToolRegistry::with_builtin_tools(),
        );
        let mut writer = JsonlSessionWriter::create(&session_path)
            .await
            .expect("session writer");

        let turn = run_prompt_with_runtime(
            "read nested data".to_owned(),
            AgentContext::new(),
            &mut writer,
            runtime,
        )
        .await
        .expect("nested over-budget turn");

        let requests = harness.requests();
        assert_eq!(
            turn.assistant_text,
            "done",
            "events: {:?}; provider requests: {}",
            turn.events,
            requests.len(),
        );
        let epoch = turn
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::InstructionEpoch { epoch }
                    if epoch.deferred_tool_ids == ["call-1".to_owned()] =>
                {
                    Some(epoch)
                }
                _ => None,
            })
            .expect("nested partially-loaded epoch");
        assert_eq!(
            epoch.outcome,
            neo_agent_core::instructions::InstructionEpochOutcome::PartiallyLoaded,
        );
        let canonical_workspace = workspace.canonicalize().expect("canonical workspace");
        let canonical_nested = nested.canonicalize().expect("canonical nested");
        assert!(
            epoch
                .selected_bundles
                .iter()
                .any(|bundle| bundle.display_path == canonical_nested
                    && bundle.import_count == 1
                    && bundle
                        .import_paths
                        .iter()
                        .any(|path| path.ends_with("imported.md"))),
            "loaded nested import metadata: {epoch:?}",
        );
        assert!(
            epoch
                .ignored_bundles
                .iter()
                .any(|bundle| bundle.display_path == canonical_workspace),
            "ignored root metadata: {epoch:?}",
        );
        let authority = epoch.model_content.as_deref().expect("nested authority");
        assert!(
            authority.contains(LOADED),
            "loaded import missing: {authority}"
        );
        assert!(
            authority.contains("over budget"),
            "warning missing: {authority}"
        );
        assert!(
            !authority.contains(IGNORED),
            "ignored body leaked: {authority}"
        );

        let deferred = turn.events.iter().find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, result, .. } if id == "call-1" => Some(result),
            _ => None,
        });
        let deferred = deferred.expect("deferred tool result");
        assert!(!deferred.is_error);
        assert_eq!(
            deferred.details.as_ref().expect("deferred details")["status"],
            "deferred",
        );
        let retried = turn.events.iter().find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, result, .. } if id == "call-2" => Some(result),
            _ => None,
        });
        assert!(retried.is_some_and(|result| !result.is_error));
        assert_eq!(requests.len(), 3);
        let after_defer = requests[1]
            .messages
            .iter()
            .map(chat_message_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(after_defer.contains(LOADED));
        assert!(!after_defer.contains(IGNORED));
        assert!(after_defer.contains("Tool call deferred"));
    }

    #[tokio::test]
    async fn prepare_existing_streaming_turn_uses_session_root_for_main_wire_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = test_config(temp.path());
        let session_id = "session_00000000-0000-4000-8000-000000000502";
        let session_dir = crate::config::workspace_sessions_dir(&config).join(session_id);
        let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
        tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
            .await
            .expect("create wire dir");
        let mut seed = JsonlSessionWriter::create(&session_path)
            .await
            .expect("create session");
        seed.append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("hello"),
        })
        .await
        .expect("append user");
        seed.flush().await.expect("flush seed");

        let prepared = super::prepare_existing_streaming_turn(
            session_id,
            &[Content::text("continue")],
            MessageOrigin::User,
            &config,
            None,
            None,
        )
        .await
        .expect("prepare existing streaming turn");

        assert_eq!(prepared.session_directory, session_dir);
    }

    #[test]
    fn streaming_event_effects_persist_user_message() {
        let user_message = AgentMessage::user_text("hello");
        let event = AgentEvent::MessageAppended {
            message: user_message,
        };

        let effect = super::streaming_event_effect(&event);

        assert!(effect.persist);
        assert!(effect.forward);
        assert_eq!(effect.assistant_text.as_deref(), None);
    }

    #[test]
    fn retry_scheduled_notice_is_plain_non_tty_stderr() {
        let mut stderr = Vec::new();

        super::write_retry_notice(
            &AgentEvent::RetryScheduled {
                turn: 1,
                retry: 1,
                max_retries: 5,
                delay_ms: 500,
                error_code: "provider.transport_error".to_owned(),
                message: "transport error: \u{1b}[31mbody closed\u{1b}[0m\r\nretry detail"
                    .to_owned(),
            },
            &mut stderr,
        )
        .expect("write retry notice");
        super::write_retry_notice(
            &AgentEvent::RetryStarted {
                turn: 1,
                retry: 1,
                max_retries: 5,
            },
            &mut stderr,
        )
        .expect("ignore non-scheduled retry event");

        let stderr = String::from_utf8(stderr).expect("plain UTF-8 notice");
        assert_eq!(
            stderr,
            "Reconnecting 1/5 in 500ms: Network error: body closed  retry detail\n"
        );
        assert!(!stderr.contains('\u{1b}'));
    }

    #[tokio::test]
    async fn append_streaming_event_persists_user_message_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_path = dir.path().join("session.jsonl");
        let mut writer = JsonlSessionWriter::create(&session_path)
            .await
            .expect("create session writer");
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let user_message = AgentMessage::user_text("hello");
        let event = AgentEvent::MessageAppended {
            message: user_message.clone(),
        };
        let mut assistant_text = String::new();
        let mut events = Vec::new();
        let mut persistence = super::SessionEventPersistence::default();

        super::append_streaming_event(
            &event,
            &mut writer,
            &mut assistant_text,
            &event_tx,
            &mut events,
            &mut persistence,
        )
        .await
        .expect("append streaming event");
        writer.flush().await.expect("flush writer");

        let forwarded = event_rx
            .try_recv()
            .expect("forwarded event")
            .expect("successful event");
        assert_eq!(forwarded, event);
        assert_eq!(events, vec![event]);
        assert!(assistant_text.is_empty());
        assert_eq!(
            JsonlSessionReader::replay_messages(&session_path)
                .await
                .expect("replay messages"),
            vec![user_message]
        );
    }

    #[test]
    fn streaming_event_effects_persist_assistant_text() {
        let event = AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("answer")],
                Vec::new(),
                AgentStopReason::EndTurn,
            ),
        };

        let effect = super::streaming_event_effect(&event);

        assert!(effect.persist);
        assert!(effect.forward);
        assert_eq!(effect.assistant_text.as_deref(), Some("answer"));
    }

    #[test]
    fn streaming_event_effects_persist_non_message_events_without_text() {
        let event = AgentEvent::TurnStarted { turn: 1 };

        let effect = super::streaming_event_effect(&event);

        assert!(effect.persist);
        assert!(effect.forward);
        assert_eq!(effect.assistant_text.as_deref(), None);
    }

    #[test]
    fn list_configured_models_formats_text_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "gpt-4.1".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "fast".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                max_context_tokens: Some(1_000_000),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                display_name: Some("GPT 4.1".to_owned()),
                ..ModelConfig::default()
            },
        );
        config.models.insert(
            "local/echo".to_owned(),
            ModelConfig {
                provider: "missing".to_owned(),
                model: "echo".to_owned(),
                capabilities: vec!["streaming".to_owned()],
                ..ModelConfig::default()
            },
        );

        let output = list_configured_models(&config, false).expect("models list");

        assert_eq!(
            output,
            concat!(
                "models:\n",
                "- fast -> openai/gpt-4.1 (openai_response default) ctx=1000000 [streaming,tools] - GPT 4.1\n",
                "- local/echo (unknown) ctx=? [streaming]\n",
            )
        );
    }

    #[test]
    fn list_configured_models_formats_json_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "fast".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "fast".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                max_context_tokens: Some(1_000_000),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                display_name: Some("GPT 4.1".to_owned()),
                ..ModelConfig::default()
            },
        );

        let output = list_configured_models(&config, true).expect("models json");
        let value: serde_json::Value = serde_json::from_str(&output).expect("json output");

        assert_eq!(
            value,
            serde_json::json!({
                "models": [{
                    "alias": "fast",
                    "provider": "openai",
                    "model": "gpt-4.1",
                    "type": "openai_response",
                    "capabilities": ["streaming", "tools"],
                    "max_context_tokens": 1_000_000,
                    "display_name": "GPT 4.1",
                    "default": true,
                }],
                "default_model": "fast",
            })
        );
    }

    #[test]
    fn select_config_model_accepts_default_model_alias() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "openai/gpt-large".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "openai/gpt-large".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-large".to_owned(),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                ..ModelConfig::default()
            },
        );

        let registry = model_registry_for_config(&config).expect("registry");
        let model = select_config_model(&registry, &config).expect("model resolves");

        assert_eq!(model.provider.0, "openai");
        assert_eq!(model.model, "gpt-large");
    }

    #[test]
    fn configured_model_registry_uses_typed_reasoning_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "reasoner".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "reasoner".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-reasoner".to_owned(),
                capabilities: vec![
                    "streaming".to_owned(),
                    "tools".to_owned(),
                    "reasoning".to_owned(),
                ],
                reasoning: neo_ai::ReasoningCapability::Effort {
                    values: vec![
                        neo_ai::ReasoningEffort::low(),
                        neo_ai::ReasoningEffort::high(),
                    ],
                    disable_supported: true,
                },
                ..ModelConfig::default()
            },
        );

        let registry = model_registry_for_config(&config).expect("registry");
        let model = select_config_model(&registry, &config).expect("model resolves");

        assert_eq!(
            model.capabilities.reasoning,
            neo_ai::ReasoningCapability::Effort {
                values: vec![
                    neo_ai::ReasoningEffort::low(),
                    neo_ai::ReasoningEffort::high()
                ],
                disable_supported: true,
            }
        );
    }

    #[test]
    fn select_config_model_accepts_unqualified_config_alias() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "fast".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
                ..ProviderConfig::default()
            },
        );
        config.models.insert(
            "fast".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
                ..ModelConfig::default()
            },
        );

        let registry = model_registry_for_config(&config).expect("registry");
        let model = select_config_model(&registry, &config).expect("alias resolves");

        assert_eq!(model.provider.0, "openai");
        assert_eq!(model.model, "gpt-4.1");
    }

    #[test]
    fn select_config_model_accepts_bare_default_model_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "gpt-4.1".to_owned();

        let registry = model_registry_for_config(&config).expect("registry");
        let model = select_config_model(&registry, &config).expect("builtin model resolves");

        assert_eq!(model.provider.0, "openai");
        assert_eq!(model.model, "gpt-4.1");
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
            api_key_env: None,
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
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
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

    #[tokio::test]
    async fn tool_registry_ignores_failed_mcp_server_startup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        let mut server = test_mcp_server("bad", McpTransport::Stdio, None);
        server.command = Some("neo-missing-mcp-binary-for-test".to_owned());
        server.startup_timeout_ms = Some(50);
        config.mcp.servers.push(server);

        let registry =
            tool_registry_for_config(&config, Arc::new(std::sync::Mutex::new(Vec::new())), None)
                .await
                .expect("bad MCP server should not abort registry construction");

        assert!(
            registry
                .specs()
                .iter()
                .all(|spec| !spec.name.starts_with("mcp__bad__")),
            "failed MCP tools must not be exposed"
        );
    }

    #[tokio::test]
    async fn shared_mcp_manager_does_not_relog_startup_failure_during_tool_registration() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        let mut server = test_mcp_server("bad", McpTransport::Stdio, None);
        server.command = Some("neo-missing-mcp-binary-for-test".to_owned());
        server.startup_timeout_ms = Some(50);
        config.mcp.servers.push(server);
        let manager = McpConnectionManager::new(ProcessSupervisor::default());
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let layer = crate::log_capture::CapturingLayer::new(event_tx);
        let _guard = tracing_subscriber::registry().with(layer).set_default();

        tool_registry_for_config(
            &config,
            Arc::new(std::sync::Mutex::new(Vec::new())),
            Some(&manager),
        )
        .await
        .expect("bad MCP server should not abort registry construction");

        let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            events.is_empty(),
            "startup failure was already surfaced by the shared MCP manager: {events:?}"
        );
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

    #[tokio::test]
    async fn auth_mcp_server_errors_for_missing_server() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = test_config(temp.path());
        let result = auth_mcp_server("missing".to_owned(), &config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn auth_mcp_server_errors_for_non_remote_transport() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config
            .mcp
            .servers
            .push(test_mcp_server("fs", McpTransport::Stdio, None));
        let result = auth_mcp_server("fs".to_owned(), &config).await;
        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(message.contains("HTTP/SSE"), "unexpected error: {message}");
    }
}
