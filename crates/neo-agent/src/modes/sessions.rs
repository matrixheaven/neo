use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use neo_agent_core::AgentMessage;
use neo_agent_core::session::export::{ExportConversation, ExportMessage, HtmlExportOptions};
use neo_agent_core::session::{
    JsonlSessionReader, SessionCompactionOptions, SessionIndex, SessionIndexEntry,
    SessionMetadataStore, SessionState, SessionStateStore, SessionSummary, compact_jsonl_session,
    main_agent_wire_path, validate_session_id,
};
use serde::Serialize;
use uuid::Uuid;

use crate::config::{AppConfig, workspace_sessions_dir};

pub(crate) const MAX_RESUME_SESSION_BYTES: u64 = 512 * 1024 * 1024;

pub fn list(config: &AppConfig) -> anyhow::Result<String> {
    let bucket_dir = workspace_sessions_dir(config);
    let sessions = metadata_store(config)
        .list()
        .with_context(|| format!("failed to read sessions directory {}", bucket_dir.display()))?;

    if sessions.is_empty() {
        Ok("no sessions\n".to_owned())
    } else {
        let lines = sessions
            .iter()
            .map(|session| {
                let mut parts = vec![session.id.clone()];
                if let Some(name) = &session.name {
                    parts.push(name.clone());
                }
                if let Some(parent_id) = &session.parent_id {
                    parts.push(format!("parent={parent_id}"));
                }
                if let Some(summary) = &session.summary {
                    parts.push(format!("summary={summary}"));
                }
                if !session.children.is_empty() {
                    parts.push(format!("children={}", session.children.join(",")));
                }
                parts.join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(format!("{lines}\n"))
    }
}

/// Scope for the session picker data layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionPickerScope {
    Workspace,
    All,
}

pub(crate) struct CreatedSession {
    pub session_id: String,
    pub wire_path: PathBuf,
}

pub(crate) async fn create_new_session(config: &AppConfig) -> anyhow::Result<CreatedSession> {
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
        let session_id = format!("session_{}", Uuid::new_v4());
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
            let wire_path = main_agent_wire_path(&session_dir);
            if let Some(parent) = wire_path.parent() {
                tokio::fs::create_dir_all(parent).await.with_context(|| {
                    format!("failed to create main agent directory {}", parent.display())
                })?;
            }
            let mut state = SessionState::new();
            state.ensure_main_agent();
            SessionStateStore::new(&session_dir)
                .write(&state)
                .with_context(|| {
                    format!("failed to write session state {}", session_dir.display())
                })?;
            index_new_session(config, &session_id)?;
            return Ok(CreatedSession {
                session_id,
                wire_path,
            });
        }
    }
}

pub(crate) fn index_new_session(config: &AppConfig, session_id: &str) -> anyhow::Result<()> {
    let neo_home = crate::config::neo_home()
        .context("could not resolve Neo home directory for the global session index")?;
    SessionIndex::new(&neo_home)
        .append(&SessionIndexEntry {
            session_id: session_id.to_owned(),
            session_dir: workspace_sessions_dir(config),
            workdir: config.project_dir.clone(),
        })
        .map_err(|error| anyhow::anyhow!("failed to index session {session_id}: {error}"))
}

/// Return session summaries for the current workspace or every known workspace.
pub(crate) fn session_summaries(
    config: &AppConfig,
    scope: SessionPickerScope,
) -> anyhow::Result<Vec<SessionSummary>> {
    match scope {
        SessionPickerScope::Workspace => metadata_store(config)
            .list_summaries(&config.project_dir)
            .map_err(|error| anyhow::anyhow!("failed to list workspace sessions: {error}")),
        SessionPickerScope::All => {
            let neo_home = config.sessions_dir.parent().ok_or_else(|| {
                anyhow::anyhow!(
                    "sessions dir {} has no parent directory",
                    config.sessions_dir.display()
                )
            })?;
            let index = SessionIndex::new(neo_home);
            index
                .list_all_with_metadata(&config.sessions_dir)
                .map_err(|error| anyhow::anyhow!("failed to list all sessions: {error}"))
        }
    }
}

pub fn rename(session_ref: &str, name: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let session = metadata_store(config)
        .rename(&session_id, name.to_owned())
        .with_context(|| format!("failed to rename session {session_ref}"))?;
    Ok(format!(
        "renamed {} {}\n",
        session.id,
        session.name.unwrap_or_default()
    ))
}

pub fn fork(session_ref: &str, name: Option<&str>, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let session = metadata_store(config)
        .fork(&session_id, name.map(str::to_owned))
        .with_context(|| format!("failed to fork session {session_ref}"))?;
    let mut output = format!("forked {session_id} -> {}", session.id);
    if let Some(name) = session.name {
        output.push(' ');
        output.push_str(&name);
    }
    output.push('\n');
    Ok(output)
}

pub fn show(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let path = session_path(session_ref, config)?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session {}", path.display()))?;
    Ok(format!("{content}\n"))
}

pub async fn transcript(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let path = session_path(&session_id, config)?;
    ensure_session_can_be_replayed(&session_id, &path)?;
    let context = JsonlSessionReader::replay_context(path)
        .await
        .with_context(|| format!("failed to replay session {session_ref}"))?;
    let mut lines = Vec::new();
    if let Some(summary) = context.compaction_summary() {
        lines.push(format!("compaction: {}", summary.summary));
    }
    if let Some(summary) = metadata_store(config).list().ok().and_then(|sessions| {
        sessions
            .into_iter()
            .find(|session| session.id == session_id)
            .and_then(|session| session.summary)
    }) {
        lines.push(format!("branch summary: {summary}"));
    }
    lines.extend(
        context
            .messages()
            .iter()
            .map(format_message)
            .collect::<Vec<_>>(),
    );
    let lines = lines
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");

    if lines.is_empty() {
        Ok("empty session\n".to_owned())
    } else {
        Ok(format!("{lines}\n"))
    }
}

pub(crate) fn ensure_session_can_be_replayed(session_id: &str, path: &Path) -> anyhow::Result<()> {
    let size = fs::metadata(path)
        .with_context(|| format!("failed to stat session {}", path.display()))?
        .len();
    if size > MAX_RESUME_SESSION_BYTES {
        anyhow::bail!(
            "session {session_id} main wire is too large to resume safely ({size} bytes)."
        );
    }
    Ok(())
}

pub async fn compact(
    session_ref: &str,
    keep_recent: usize,
    config: &AppConfig,
) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let result = compact_jsonl_session(
        session_path(&session_id, config)?,
        SessionCompactionOptions {
            keep_recent_messages: keep_recent,
        },
    )
    .await
    .with_context(|| format!("failed to compact session {session_ref}"))?;

    Ok(format!(
        "compacted {session_id}: compacted {}, kept {}\n{}\n",
        result.compacted_message_count, result.kept_message_count, result.summary.summary
    ))
}

pub async fn export_html(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let messages = JsonlSessionReader::replay_messages(session_path(&session_id, config)?)
        .await
        .with_context(|| format!("failed to replay session {session_ref}"))?;
    render_messages_html(format!("neo session {session_id}"), &messages)
}

pub async fn export_json(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let artifact = export_json_artifact(session_ref, config).await?;
    let mut json = serde_json::to_string_pretty(&artifact)?;
    json.push('\n');
    Ok(json)
}

pub(crate) async fn export_json_artifact(
    session_ref: &str,
    config: &AppConfig,
) -> anyhow::Result<SessionExportJsonArtifact> {
    let session_id = resolve_session_id(session_ref, config)?;
    let path = session_path(&session_id, config)?;
    anyhow::ensure!(path.exists(), "session {session_ref:?} does not exist");

    let bucket_dir = workspace_sessions_dir(config);
    let record = metadata_store(config)
        .list()
        .with_context(|| format!("failed to read sessions directory {}", bucket_dir.display()))?
        .into_iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| anyhow::anyhow!("session {session_ref:?} does not exist"))?;
    let messages = JsonlSessionReader::replay_messages(path)
        .await
        .with_context(|| format!("failed to replay session {session_ref}"))?;

    Ok(SessionExportJsonArtifact {
        format: "neo.session.export_json",
        schema_version: 1,
        metadata: SessionExportJsonMetadata {
            id: record.id,
            name: record.name,
            summary: record.summary,
            parent_id: record.parent_id,
            children: record.children,
            message_count: messages.len(),
        },
        messages,
    })
}

fn render_messages_html(title: String, messages: &[AgentMessage]) -> anyhow::Result<String> {
    let export_messages = messages
        .iter()
        .map(|message| ExportMessage::new(message_role(message), message.presentation_text()))
        .collect();
    let conversation = ExportConversation::new(title, export_messages);
    neo_agent_core::session::export::export_html(&conversation, &HtmlExportOptions::default())
        .map_err(anyhow::Error::from)
}

pub fn session_path(session_ref: &str, config: &AppConfig) -> anyhow::Result<PathBuf> {
    let session_id = resolve_session_id(session_ref, config)?;
    let bucket_dir = workspace_sessions_dir(config);
    Ok(neo_agent_core::session::main_agent_wire_path(
        &bucket_dir.join(&session_id),
    ))
}

pub fn resolve_session_id(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    if let Some(session_id) = session_id_from_jsonl_path(session_ref, config)? {
        return Ok(session_id);
    }

    validate_session_id(session_ref)
        .map_err(|_| anyhow::anyhow!("invalid session id {session_ref:?}"))?;
    Ok(session_ref.to_owned())
}

fn session_id_from_jsonl_path(
    session_ref: &str,
    config: &AppConfig,
) -> anyhow::Result<Option<String>> {
    let raw = Path::new(session_ref);

    // Determine the effective path to check.
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        env::current_dir()?.join(raw)
    };

    let bucket_dir = workspace_sessions_dir(config);
    if !path_is_inside_session_bucket(&path, &bucket_dir) {
        return Ok(None);
    }

    if path.file_name().and_then(|n| n.to_str()) == Some("wire.jsonl")
        && path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            == Some("main")
        && path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            == Some("agents")
        && let Some(session_id) = path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
    {
        validate_session_id(session_id)
            .map_err(|_| anyhow::anyhow!("invalid session id {session_id:?}"))?;
        return Ok(Some(session_id.to_owned()));
    }

    Ok(None)
}

fn path_is_inside_session_bucket(path: &Path, bucket_dir: &Path) -> bool {
    match path.canonicalize() {
        Ok(canonical_path) => bucket_dir.canonicalize().map_or_else(
            |_| path.starts_with(bucket_dir),
            |canonical_bucket| canonical_path.starts_with(canonical_bucket),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.starts_with(bucket_dir),
        Err(_) => false,
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionExportJsonArtifact {
    pub format: &'static str,
    pub schema_version: u32,
    pub metadata: SessionExportJsonMetadata,
    pub messages: Vec<AgentMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionExportJsonMetadata {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub children: Vec<String>,
    pub message_count: usize,
}

fn metadata_store(config: &AppConfig) -> SessionMetadataStore {
    let bucket_dir = workspace_sessions_dir(config);
    SessionMetadataStore::new(&bucket_dir)
}

fn format_message(message: &AgentMessage) -> String {
    let role = message_role(message);
    let text = message.presentation_text();

    format!("{role}: {text}")
}

fn message_role(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::System { .. } => "system",
        AgentMessage::Instruction { .. } => "instruction",
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant { .. } => "assistant",
        AgentMessage::ToolResult { .. } => "tool",
        AgentMessage::ShellCommand { .. } => "shell",
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use neo_agent_core::PermissionMode;

    use crate::config::{Defaults, McpConfig, RuntimeConfig, TuiConfig};

    use super::*;

    #[test]
    fn create_new_session_initializes_shared_state_and_index() {
        let temp = tempfile::tempdir().expect("tempdir");
        let neo_home = temp.path().join("neo-home");
        let config = AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: neo_home.join("sessions"),
            permission_mode: PermissionMode::default(),
            live_permission_mode: Arc::new(std::sync::RwLock::new(PermissionMode::default())),
            workspace_policy: Arc::new(std::sync::RwLock::new(None)),
            defaults: Defaults {
                mode: "events".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            workflow_capability: neo_agent_core::workflow::WorkflowCapability::default(),
            workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver::default(
            ),
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
            project_dir: temp.path().join("project"),
            config_path: neo_home.join("config.toml"),
            config_file_exists: true,
        };

        temp_env::with_var("NEO_HOME", Some(neo_home.as_os_str()), || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build test runtime");
            runtime.block_on(async {
                let created = create_new_session(&config).await.expect("create session");
                let session_dir = workspace_sessions_dir(&config).join(&created.session_id);

                assert_eq!(created.wire_path, main_agent_wire_path(&session_dir));
                validate_session_id(&created.session_id).expect("valid generated session id");
                let state = SessionStateStore::new(&session_dir)
                    .read()
                    .await
                    .expect("read session state");
                assert_eq!(
                    state.agents.get("main").map(|main| &main.record_dir),
                    Some(&neo_agent_core::session::relative_agent_record_dir("main"))
                );
                let indexed = SessionIndex::new(&neo_home)
                    .find(&created.session_id)
                    .expect("read session index")
                    .expect("session is indexed");
                assert_eq!(indexed.session_dir, workspace_sessions_dir(&config));
                assert_eq!(indexed.workdir, config.project_dir);
            });
        });
    }

    #[test]
    fn human_transcript_outputs_prefer_user_display_text() {
        let message = AgentMessage::user_content_with_display(
            [neo_agent_core::Content::text(
                "<file path=\"src/main.rs\">snapshot</file>",
            )],
            "review @[main.rs]",
        );

        assert_eq!(format_message(&message), "user: review @[main.rs]");
        let html =
            render_messages_html("session".to_owned(), &[message]).expect("render transcript html");
        assert!(html.contains("review @[main.rs]"), "{html}");
        assert!(!html.contains("snapshot"), "{html}");
    }
}
