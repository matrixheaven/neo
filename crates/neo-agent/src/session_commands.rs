use std::{fs, path::PathBuf};

use anyhow::Context;
use neo_agent_core::session::{
    JsonlSessionReader, SessionCompactionOptions, SessionMetadataStore, compact_jsonl_session,
    validate_session_id,
};
use neo_agent_core::{AgentMessage, Content};
use neo_sdk::{ExportConversation, ExportMessage, HtmlExportOptions, export_html as render_html};

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

pub fn rename(session_id: &str, name: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session = metadata_store(config)
        .rename(session_id, name.to_owned())
        .with_context(|| format!("failed to rename session {session_id}"))?;
    Ok(format!(
        "renamed {} {}\n",
        session.id,
        session.name.unwrap_or_default()
    ))
}

pub fn fork(session_id: &str, name: Option<&str>, config: &AppConfig) -> anyhow::Result<String> {
    let session = metadata_store(config)
        .fork(session_id, name.map(str::to_owned))
        .with_context(|| format!("failed to fork session {session_id}"))?;
    let mut output = format!("forked {session_id} -> {}", session.id);
    if let Some(name) = session.name {
        output.push(' ');
        output.push_str(&name);
    }
    output.push('\n');
    Ok(output)
}

pub fn show(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let path = session_path(session_id, config)?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session {}", path.display()))?;
    Ok(format!("{content}\n"))
}

pub async fn transcript(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let context = JsonlSessionReader::replay_context(session_path(session_id, config)?)
        .await
        .with_context(|| format!("failed to replay session {session_id}"))?;
    let mut lines = Vec::new();
    if let Some(summary) = context.compaction_summary() {
        lines.push(format!("compaction: {}", summary.summary));
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
    session_id: &str,
    keep_recent: usize,
    config: &AppConfig,
) -> anyhow::Result<String> {
    let result = compact_jsonl_session(
        session_path(session_id, config)?,
        SessionCompactionOptions {
            keep_recent_messages: keep_recent,
        },
    )
    .await
    .with_context(|| format!("failed to compact session {session_id}"))?;

    Ok(format!(
        "compacted {session_id}: compacted {}, kept {}\n{}\n",
        result.compacted_message_count, result.kept_message_count, result.summary.summary
    ))
}

pub async fn export_html(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let messages = JsonlSessionReader::replay_messages(session_path(session_id, config)?)
        .await
        .with_context(|| format!("failed to replay session {session_id}"))?;
    let export_messages = messages
        .iter()
        .map(|message| ExportMessage::new(message_role(message), message_text(message)))
        .collect();
    let conversation =
        ExportConversation::new(format!("neo session {session_id}"), export_messages);
    render_html(&conversation, &HtmlExportOptions::default()).map_err(anyhow::Error::from)
}

pub fn session_path(session_id: &str, config: &AppConfig) -> anyhow::Result<PathBuf> {
    validate_session_id(session_id)
        .map_err(|_| anyhow::anyhow!("invalid session id {session_id:?}"))?;
    Ok(config.sessions_dir.join(format!("{session_id}.jsonl")))
}

fn metadata_store(config: &AppConfig) -> SessionMetadataStore {
    SessionMetadataStore::new(&config.sessions_dir)
}

fn format_message(message: &AgentMessage) -> String {
    let role = message_role(message);

    format!("{role}: {}", message_text(message))
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

fn text_content(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("")
}
