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
    session_id_from_path,
};

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::goal::GoalManager;
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter};
use neo_agent_core::{
    AgentContext, AgentEvent, AgentMessage, AgentRuntime, AskUserTool, Content, CreateSkillTool,
    ListSkillsTool, McpConnectionManager, MoveSkillTool, PendingQuestion,
    PermissionApprovalDecision, SteerInputHandle, SummarizeSessionsTool, mode::PlanMode,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::RunOutput,
    config::{AppConfig, neo_home, workspace_sessions_dir},
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
    let turn = if no_session {
        run_prompt_ephemeral(prompt, config).await?
    } else if continue_latest {
        let session_id = latest_session_id(config)?;
        run_prompt_in_session(&session_id, prompt, config).await?
    } else {
        run_prompt(prompt, config).await?
    };
    match output {
        RunOutput::Json => output::stable_json_output(&turn, config),
        RunOutput::Text => Ok(format!("{}\n", turn.assistant_text)),
        RunOutput::Events => {
            let mut output = String::new();
            for event in turn.events {
                output.push_str(&serde_json::to_string(&event)?);
                output.push('\n');
            }
            Ok(output)
        }
    }
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
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(&prompt_text)];
    let session_path = create_session_path(config).await?;
    let session_id = session_id_from_path(&session_path)?;
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(content, &mut writer).await?;
    record_session_activity(config, &session_id, &prompt_text);
    let runtime = runtime_for_config(
        config,
        session_path.parent().map(Path::to_path_buf),
        None,
        None,
        None,
        None,
        false,
        SteerInputHandle::new(),
        None,
        Arc::new(Mutex::new(None)),
    )
    .await?;
    let turn = finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        session_id,
    )
    .await?;
    record_initial_session_title(config, &turn, &prompt_text).await;
    Ok(turn)
}

pub async fn run_prompt_ephemeral(
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let prompt_text = prompt.join(" ");
    let content = vec![Content::text(&prompt_text)];
    let mut writer = SessionEventWriter::memory();
    let (user_message, events) = append_user_event(content, &mut writer).await?;
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
    )
    .await?;
    finish_prompt_turn(
        user_message,
        AgentContext::new(),
        &mut writer,
        runtime,
        events,
        "ephemeral".to_owned(),
    )
    .await
}

pub async fn run_prompt_in_session(
    session_id: &str,
    prompt: &[String],
    config: &AppConfig,
) -> anyhow::Result<PromptTurn> {
    let prompt_text = prompt.join(" ");
    let user_content = vec![Content::text(&prompt_text)];
    let session_path = sessions::session_path(session_id, config)?;
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    let mut writer = SessionEventWriter::jsonl(&mut writer);
    let (user_message, events) = append_user_event(user_content, &mut writer).await?;
    record_session_activity(config, session_id, &prompt_text);
    let runtime = runtime_for_config(
        config,
        session_path.parent().map(Path::to_path_buf),
        None,
        None,
        None,
        None,
        false,
        SteerInputHandle::new(),
        None,
        Arc::new(Mutex::new(None)),
    )
    .await?;
    runtime.restore_plan_mode(&context);
    finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        session_id.to_owned(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_prompt_streaming(
    prompt: &[Content],
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
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let prepared = prepare_new_streaming_turn(prompt, config, session_id_tx, skill_context).await?;
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
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let prepared =
        prepare_existing_streaming_turn(session_id, prompt, config, session_id_tx, skill_context)
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
    )
    .await?;
    runtime.restore_plan_mode(&prepared.context);
    run_prepared_streaming_turn(prepared, runtime, event_tx, cancel_token, compaction_only).await
}

async fn prepare_new_streaming_turn(
    prompt: &[Content],
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
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .with_context(|| format!("failed to create session {}", session_path.display()))?;
    send_streaming_session_id(session_id_tx, &session_id);
    let (user_message, initial_events) =
        append_user_event_jsonl(prompt.to_vec(), &mut writer).await?;
    record_session_activity(config, &session_id, &prompt_text);
    let session_directory = session_path
        .parent()
        .map_or_else(
            || workspace_sessions_dir(config).join(&session_id),
            Path::to_path_buf,
        );
    Ok(PreparedStreamingTurn {
        prompt: prompt_text,
        session_id,
        session_directory,
        context: streaming_context(skill_context),
        writer,
        user_message,
        initial_events,
    })
}

async fn prepare_existing_streaming_turn(
    session_id: &str,
    prompt: &[Content],
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
    let session_directory = session_path
        .parent()
        .map_or_else(
            || workspace_sessions_dir(config).join(session_id),
            Path::to_path_buf,
        );
    let mut context = JsonlSessionReader::replay_context(&session_path)
        .await
        .with_context(|| format!("failed to replay session {}", session_path.display()))?;
    apply_skill_context(&mut context, skill_context);
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .with_context(|| format!("failed to append session {}", session_path.display()))?;
    send_streaming_session_id(session_id_tx, session_id);
    let (user_message, initial_events) =
        append_user_event_jsonl(prompt.to_vec(), &mut writer).await?;
    record_session_activity(config, session_id, &prompt_text);
    Ok(PreparedStreamingTurn {
        prompt: prompt_text,
        session_id: session_id.to_owned(),
        session_directory,
        context,
        writer,
        user_message,
        initial_events,
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
) -> anyhow::Result<AgentRuntime> {
    let model = runtime::resolve_model(config)?;
    let client = runtime::resolve_model_client(config, &model)?;
    let skill_store = resources::load_skill_store(
        neo_home().as_deref(),
        &config.extra_skill_dirs,
        &config.skill_path,
    )?;
    let mut agent_config = runtime::agent_config_for_app(model, config, approval_tx, &skill_store)?;
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
        agent_config.plan_review_feedback =
            Arc::new(std::sync::Mutex::new(feedback.into_iter().collect()));
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
        tools.register(MoveSkillTool::new(home.clone()));
        tools.register(CreateSkillTool::new(home.clone()));
        tools.register(SummarizeSessionsTool::new(home));
    }
    let mut runtime = AgentRuntime::with_tools_and_skills(agent_config, client, tools, skill_store);
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

#[cfg(test)]
async fn run_prompt_with_runtime(
    prompt: String,
    context: AgentContext,
    writer: &mut JsonlSessionWriter,
    runtime: AgentRuntime,
) -> anyhow::Result<PromptTurn> {
    let mut writer = SessionEventWriter::jsonl(writer);
    let (user_message, events) =
        append_user_event(vec![Content::text(prompt)], &mut writer).await?;
    finish_prompt_turn(
        user_message,
        context,
        &mut writer,
        runtime,
        events,
        "test-session".to_owned(),
    )
    .await
}

async fn append_user_event(
    content: Vec<Content>,
    writer: &mut SessionEventWriter<'_>,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let user_message = AgentMessage::User { content };
    let user_event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    writer.append_event(&user_event).await?;
    writer.flush().await?;
    Ok((user_message, vec![user_event]))
}

async fn append_user_event_jsonl(
    content: Vec<Content>,
    writer: &mut JsonlSessionWriter,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let mut writer = SessionEventWriter::jsonl(writer);
    append_user_event(content, &mut writer).await
}

async fn finish_prompt_turn(
    user_message: AgentMessage,
    mut context: AgentContext,
    writer: &mut SessionEventWriter<'_>,
    runtime: AgentRuntime,
    mut events: Vec<AgentEvent>,
    session_id: String,
) -> anyhow::Result<PromptTurn> {
    let mut assistant_text = String::new();
    let turn_events = runtime
        .run_turn(&mut context, user_message.clone())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    for event in turn_events {
        let is_duplicate_user_message = matches!(
            &event,
            AgentEvent::MessageAppended { message } if message == &user_message
        );
        if is_duplicate_user_message {
            events.push(event);
            continue;
        }
        if let AgentEvent::MessageAppended { message } = &event
            && matches!(message, AgentMessage::Assistant { .. })
        {
            assistant_text.push_str(&message.text());
        }
        writer.append_event(&event).await?;
        events.push(event);
    }
    writer.flush().await?;

    Ok(PromptTurn {
        session_id,
        events,
        assistant_text,
    })
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
    let mut stream =
        runtime.run_turn_with_cancel(&mut context, user_message.clone(), streaming.cancel_token);
    while let Some(event) = stream.next().await {
        let event = streaming_event_or_bail(event, &streaming.event_tx)?;
        append_streaming_event(
            &event,
            &user_message,
            writer,
            &mut assistant_text,
            &streaming.event_tx,
            &mut events,
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
    let mut stream = runtime.run_compaction_turn_with_cancel(&mut context, streaming.cancel_token);
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
    user_message: &AgentMessage,
    writer: &mut JsonlSessionWriter,
    assistant_text: &mut String,
    event_tx: &mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    events: &mut Vec<AgentEvent>,
) -> anyhow::Result<()> {
    let effect = streaming_event_effect(event, user_message);
    if let Some(text) = effect.assistant_text {
        assistant_text.push_str(&text);
    }
    if effect.persist {
        writer.append_event(event).await?;
    }
    if effect.forward {
        let _ = event_tx.send(Ok(event.clone()));
        events.push(event.clone());
    }
    Ok(())
}

fn streaming_event_effect(event: &AgentEvent, user_message: &AgentMessage) -> StreamingEventEffect {
    if is_duplicate_user_message_event(event, user_message) {
        return StreamingEventEffect {
            persist: false,
            forward: false,
            assistant_text: None,
        };
    }
    StreamingEventEffect {
        persist: true,
        forward: true,
        assistant_text: assistant_text_from_event(event),
    }
}

fn is_duplicate_user_message_event(event: &AgentEvent, user_message: &AgentMessage) -> bool {
    matches!(
        event,
        AgentEvent::MessageAppended { message } if message == user_message
    )
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
    use std::{collections::BTreeMap, sync::Arc};

    use neo_agent_core::{
        AgentConfig, AgentEvent, AgentMessage, ApprovalRequest, CompactionSettings, Content,
        PermissionApprovalDecision, PermissionMode, PermissionOperation, QueueMode,
        StopReason as AgentStopReason, ToolExecutionMode,
        session::{JsonlSessionReader, JsonlSessionWriter},
        skills::SkillStore,
    };
    use neo_ai::{
        AiStreamEvent, ApiKind, ApiType, ChatMessage, ContentPart, ModelCapabilities, ModelSpec,
        ProviderId, StopReason, providers::fake::FakeModelClient,
    };

    use super::mcp_cli::auth_mcp_server;
    use super::models_cli::list_configured_models;
    use super::runtime::{
        agent_config_for_app, model_registry_for_config, select_config_model,
        tool_registry_for_config,
    };
    use super::session_mgmt::create_session_path;
    use super::{PromptApprovalRequest, run_prompt_with_runtime};
    use crate::config::{
        AppConfig, Defaults, McpConfig, McpTransport, ModelConfig, ProviderConfig,
        RuntimeCompactionConfig, RuntimeConfig, TuiConfig,
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
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: Some(0.35),
                max_tokens: Some(512),
                reasoning_effort: Some(neo_ai::ReasoningEffort::High),
                replay_reasoning: true,
                steering_queue_mode: QueueMode::OneAtATime,
                follow_up_queue_mode: QueueMode::OneAtATime,
                tool_execution_mode: ToolExecutionMode::Sequential,
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
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat(),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store).expect("agent config");

        assert_eq!(agent_config.temperature, Some(0.35));
        assert_eq!(agent_config.max_tokens, Some(512));
        assert_eq!(
            agent_config.reasoning_effort,
            Some(neo_ai::ReasoningEffort::High)
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
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
                replay_reasoning: true,
                steering_queue_mode: QueueMode::OneAtATime,
                follow_up_queue_mode: QueueMode::OneAtATime,
                tool_execution_mode: ToolExecutionMode::Sequential,
                compaction: None,
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        // Model declares max_output_tokens; runtime does not override.
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat().with_max_output_tokens(64_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store).expect("agent config");

        assert_eq!(agent_config.max_tokens, Some(64_000));
    }

    #[tokio::test]
    async fn create_session_path_uses_named_uuid_session_ids() {
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
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };

        let path = create_session_path(&config)
            .await
            .expect("session path is created");
        let session_dir = path.parent().expect("session directory");
        let session_id = session_dir
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("session id");

        assert!(session_id.starts_with("session_"));
        assert_eq!(session_id.len(), "session_".len() + 36);
        assert!(neo_agent_core::session::validate_session_id(session_id).is_ok());
        assert!(path.ends_with("transcript.jsonl"));
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
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
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
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store).expect("agent config");

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
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig {
                temperature: None,
                max_tokens: None,
                reasoning_effort: None,
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
            },
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "large-context-model".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        };

        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config =
            agent_config_for_app(model, &config, None, &skill_store).expect("agent config");

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
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: temp.path().to_path_buf(),
            config_path: temp.path().join(".neo/config.toml"),
        };
        let model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat(),
        };
        let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
        let skill_store = SkillStore::load(&[], &[], Vec::new()).expect("skill store");
        let agent_config = agent_config_for_app(model, &config, Some(approval_tx), &skill_store)
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
    async fn run_prompt_with_runtime_appends_continuation_to_existing_session_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = temp
            .path()
            .join("session_00000000-0000-4000-8000-000000000501");
        let session_path = session_dir.join("transcript.jsonl");
        tokio::fs::create_dir_all(&session_dir)
            .await
            .expect("create session dir");
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
            AgentMessage::User { content } if content[0].as_text() == Some("continue")
        ));
        assert!(matches!(
            &messages[3],
            AgentMessage::Assistant { content, .. }
                if content[0].as_text() == Some("continued answer")
        ));
    }

    #[test]
    fn streaming_event_effects_skip_duplicate_user_message() {
        let user_message = AgentMessage::user_text("hello");
        let event = AgentEvent::MessageAppended {
            message: user_message.clone(),
        };

        let effect = super::streaming_event_effect(&event, &user_message);

        assert!(!effect.persist);
        assert!(!effect.forward);
        assert_eq!(effect.assistant_text.as_deref(), None);
    }

    #[tokio::test]
    async fn append_streaming_event_suppresses_duplicate_user_message_externally() {
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

        super::append_streaming_event(
            &event,
            &user_message,
            &mut writer,
            &mut assistant_text,
            &event_tx,
            &mut events,
        )
        .await
        .expect("append streaming event");
        writer.flush().await.expect("flush writer");

        assert!(event_rx.try_recv().is_err());
        assert!(events.is_empty());
        assert!(assistant_text.is_empty());
        assert!(
            JsonlSessionReader::replay_messages(&session_path)
                .await
                .expect("replay messages")
                .is_empty()
        );
    }

    #[test]
    fn streaming_event_effects_persist_assistant_text() {
        let user_message = AgentMessage::user_text("hello");
        let event = AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("answer")],
                Vec::new(),
                AgentStopReason::EndTurn,
            ),
        };

        let effect = super::streaming_event_effect(&event, &user_message);

        assert!(effect.persist);
        assert!(effect.forward);
        assert_eq!(effect.assistant_text.as_deref(), Some("answer"));
    }

    #[test]
    fn streaming_event_effects_persist_non_message_events_without_text() {
        let user_message = AgentMessage::user_text("hello");
        let event = AgentEvent::TurnStarted { turn: 1 };

        let effect = super::streaming_event_effect(&event, &user_message);

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
                provider_type: Some(ApiType::OpenAiResponses),
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
                "- fast -> openai/gpt-4.1 (openai-responses default) ctx=1000000 [streaming,tools] - GPT 4.1\n",
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
                provider_type: Some(ApiType::OpenAiResponses),
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
                    "type": "openai-responses",
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
                provider_type: Some(ApiType::OpenAiResponses),
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
    fn select_config_model_accepts_unqualified_config_alias() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.default_provider = "openai".to_owned();
        config.default_model = "fast".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                provider_type: Some(ApiType::OpenAiResponses),
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
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: project_dir.to_path_buf(),
            config_path: project_dir.join(".neo/config.toml"),
        }
    }

    #[tokio::test]
    async fn tool_registry_ignores_failed_mcp_server_startup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path());
        config.mcp.servers.push(crate::config::McpServerConfig {
            id: "bad".to_owned(),
            enabled: true,
            transport: McpTransport::Stdio,
            command: Some("neo-missing-mcp-binary-for-test".to_owned()),
            url: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            startup_timeout_ms: Some(50),
            tool_timeout_ms: None,
        });

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
