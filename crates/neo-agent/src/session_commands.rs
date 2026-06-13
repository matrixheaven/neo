use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use neo_agent_core::session::{
    JsonlSessionReader, SessionCompactionOptions, SessionMetadataStore, compact_jsonl_session,
    validate_session_id,
};
use neo_agent_core::{AgentMessage, Content};
use neo_sdk::{ExportConversation, ExportMessage, HtmlExportOptions, export_html as render_html};
use serde::Serialize;

use crate::config::AppConfig;

pub fn list(config: &AppConfig) -> anyhow::Result<String> {
    let sessions = metadata_store(config).list().with_context(|| {
        format!(
            "failed to read sessions directory {}",
            config.sessions_dir.display()
        )
    })?;

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

pub async fn summarize(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let messages = JsonlSessionReader::replay_messages(session_path(&session_id, config)?)
        .await
        .with_context(|| format!("failed to replay session {session_ref}"))?;
    let summary = summarize_messages(&messages);
    let session = metadata_store(config)
        .summarize(&session_id, summary.clone())
        .with_context(|| format!("failed to store summary for session {session_ref}"))?;
    Ok(format!(
        "summarized {}: {}\n",
        session.id,
        session.summary.unwrap_or(summary)
    ))
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

    let record = metadata_store(config)
        .list()
        .with_context(|| {
            format!(
                "failed to read sessions directory {}",
                config.sessions_dir.display()
            )
        })?
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

pub async fn export_html_file(input_path: &Path, output_path: &Path) -> anyhow::Result<()> {
    let messages = JsonlSessionReader::replay_messages(input_path)
        .await
        .with_context(|| format!("failed to replay session {}", input_path.display()))?;
    let title = input_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map_or_else(
            || "neo session".to_owned(),
            |stem| format!("neo session {stem}"),
        );
    let html = render_messages_html(title, &messages)?;
    std::fs::write(output_path, html)
        .with_context(|| format!("failed to write export {}", output_path.display()))?;
    Ok(())
}

fn render_messages_html(title: String, messages: &[AgentMessage]) -> anyhow::Result<String> {
    let export_messages = messages
        .iter()
        .map(|message| ExportMessage::new(message_role(message), message_text(message)))
        .collect();
    let conversation = ExportConversation::new(title, export_messages);
    render_html(&conversation, &HtmlExportOptions::default()).map_err(anyhow::Error::from)
}

pub fn session_path(session_ref: &str, config: &AppConfig) -> anyhow::Result<PathBuf> {
    let session_id = resolve_session_id(session_ref, config)?;
    Ok(config.sessions_dir.join(format!("{session_id}.jsonl")))
}

pub fn resolve_session_id(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    if let Some(session_id) = session_id_from_jsonl_path(session_ref, config)? {
        return Ok(session_id);
    }

    let exact_id = validate_session_id(session_ref).is_ok();
    if exact_id
        && config
            .sessions_dir
            .join(format!("{session_ref}.jsonl"))
            .is_file()
    {
        return Ok(session_ref.to_owned());
    }

    if exact_id {
        let matches = metadata_store(config)
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|session| session.id.starts_with(session_ref))
            .map(|session| session.id)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [session_id] => return Ok(session_id.clone()),
            [] => {}
            _ => {
                anyhow::bail!(
                    "ambiguous session id {session_ref:?}: {}",
                    matches.join(", ")
                );
            }
        }
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
    if raw.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return Ok(None);
    }

    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        env::current_dir()?.join(raw)
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve session path {}", raw.display()))?;
    let sessions_dir = config.sessions_dir.canonicalize().with_context(|| {
        format!(
            "failed to resolve sessions dir {}",
            config.sessions_dir.display()
        )
    })?;
    anyhow::ensure!(
        path.starts_with(&sessions_dir),
        "session path {} is outside sessions dir {}",
        path.display(),
        sessions_dir.display()
    );
    let session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid session path {}", path.display()))?;
    validate_session_id(session_id)
        .map_err(|_| anyhow::anyhow!("invalid session id {session_id:?}"))?;
    Ok(Some(session_id.to_owned()))
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
    SessionMetadataStore::new(&config.sessions_dir)
}

fn format_message(message: &AgentMessage) -> String {
    let role = message_role(message);

    format!("{role}: {}", message_text(message))
}

fn summarize_messages(messages: &[AgentMessage]) -> String {
    if messages.is_empty() {
        return "Local branch summary: empty session".to_owned();
    }

    let lines = messages
        .iter()
        .take(6)
        .map(|message| {
            format!(
                "{}: {}",
                message_role(message),
                one_line_message_text(message)
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let suffix = if messages.len() > 6 {
        format!(" | ... {} more messages", messages.len() - 6)
    } else {
        String::new()
    };
    format!(
        "Local branch summary ({} messages): {lines}{suffix}",
        messages.len()
    )
}

fn message_role(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::System { .. } => "system",
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant { .. } => "assistant",
        AgentMessage::ToolResult { .. } => "tool",
    }
}

fn message_text(message: &AgentMessage) -> String {
    let content = match message {
        AgentMessage::System { content }
        | AgentMessage::User { content }
        | AgentMessage::Assistant { content, .. }
        | AgentMessage::ToolResult { content, .. } => content,
    };
    text_content(content)
}

fn one_line_message_text(message: &AgentMessage) -> String {
    message_text(message)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn text_content(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("")
}
