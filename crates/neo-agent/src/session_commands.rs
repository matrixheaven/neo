use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use anyhow::Context;
use neo_agent_core::session::{
    JsonlSessionReader, SessionCompactionOptions, SessionMetadataStore, SessionRecord,
    compact_jsonl_session, validate_session_id,
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

pub fn tree(config: &AppConfig) -> anyhow::Result<String> {
    let sessions = metadata_store(config).list().with_context(|| {
        format!(
            "failed to read sessions directory {}",
            config.sessions_dir.display()
        )
    })?;

    if sessions.is_empty() {
        return Ok("no sessions\n".to_owned());
    }

    let lines = tree_order_sessions(&sessions)
        .iter()
        .map(format_tree_record)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!("{lines}\n"))
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

pub async fn summarize(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let messages = JsonlSessionReader::replay_messages(session_path(session_id, config)?)
        .await
        .with_context(|| format!("failed to replay session {session_id}"))?;
    let summary = summarize_messages(&messages);
    let session = metadata_store(config)
        .summarize(session_id, summary.clone())
        .with_context(|| format!("failed to store summary for session {session_id}"))?;
    Ok(format!(
        "summarized {}: {}\n",
        session.id,
        session.summary.unwrap_or(summary)
    ))
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionTreeRecord {
    pub record: SessionRecord,
    pub depth: usize,
}

pub(crate) fn tree_order_sessions(sessions: &[SessionRecord]) -> Vec<SessionTreeRecord> {
    let by_id = sessions
        .iter()
        .cloned()
        .map(|session| (session.id.clone(), session))
        .collect::<BTreeMap<_, _>>();
    let mut visited = BTreeSet::new();
    let mut ordered = Vec::new();

    for session in sessions.iter().filter(|session| {
        session
            .parent_id
            .as_ref()
            .is_none_or(|parent_id| !by_id.contains_key(parent_id))
    }) {
        append_tree_record(&session.id, 0, &by_id, &mut visited, &mut ordered);
    }

    for session in sessions {
        append_tree_record(&session.id, 0, &by_id, &mut visited, &mut ordered);
    }

    ordered
}

fn metadata_store(config: &AppConfig) -> SessionMetadataStore {
    SessionMetadataStore::new(&config.sessions_dir)
}

fn append_tree_record(
    session_id: &str,
    depth: usize,
    by_id: &BTreeMap<String, SessionRecord>,
    visited: &mut BTreeSet<String>,
    ordered: &mut Vec<SessionTreeRecord>,
) {
    if !visited.insert(session_id.to_owned()) {
        return;
    }
    let Some(record) = by_id.get(session_id).cloned() else {
        return;
    };
    let children = record.children.clone();
    ordered.push(SessionTreeRecord { record, depth });
    for child_id in children {
        append_tree_record(&child_id, depth + 1, by_id, visited, ordered);
    }
}

fn format_tree_record(tree_record: &SessionTreeRecord) -> String {
    let mut parts = vec![format!(
        "{}{}",
        "  ".repeat(tree_record.depth),
        tree_record.record.id
    )];
    if let Some(name) = &tree_record.record.name {
        parts.push(name.clone());
    }
    if let Some(summary) = &tree_record.record.summary {
        parts.push(format!("summary={summary}"));
    }
    parts.join("\t")
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
