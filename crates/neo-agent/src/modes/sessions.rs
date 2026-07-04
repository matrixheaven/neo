use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use neo_agent_core::AgentMessage;
use neo_agent_core::session::export::{ExportConversation, ExportMessage, HtmlExportOptions};
use neo_agent_core::session::{
    JsonlSessionReader, SessionCompactionOptions, SessionIndex, SessionMetadataStore,
    SessionSummary, compact_jsonl_session, validate_session_id,
};
use serde::Serialize;

use crate::config::{AppConfig, workspace_sessions_dir};

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
    let context = JsonlSessionReader::replay_context(session_path(&session_id, config)?)
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
        .map(|message| ExportMessage::new(message_role(message), message.text()))
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

    format!("{role}: {}", message.text())
}

fn message_role(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::System { .. } => "system",
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant { .. } => "assistant",
        AgentMessage::ToolResult { .. } => "tool",
        AgentMessage::ShellCommand { .. } => "shell",
    }
}
