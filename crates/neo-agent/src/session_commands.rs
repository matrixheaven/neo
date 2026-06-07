use std::{ffi::OsStr, fs, path::PathBuf};

use anyhow::{Context, bail};
use neo_agent_core::session::JsonlSessionReader;
use neo_agent_core::{AgentMessage, Content};
use neo_sdk::{ExportConversation, ExportMessage, HtmlExportOptions, export_html as render_html};

use crate::config::AppConfig;

pub fn list(config: &AppConfig) -> anyhow::Result<String> {
    if !config.sessions_dir.exists() {
        return Ok("no sessions\n".to_owned());
    }

    let mut sessions = fs::read_dir(&config.sessions_dir)
        .with_context(|| {
            format!(
                "failed to read sessions directory {}",
                config.sessions_dir.display()
            )
        })?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension() == Some(OsStr::new("jsonl")))
                .then(|| path.file_stem().map(OsStr::to_owned))
                .flatten()
        })
        .map(|name| name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    sessions.sort_unstable();

    if sessions.is_empty() {
        Ok("no sessions\n".to_owned())
    } else {
        Ok(format!("{}\n", sessions.join("\n")))
    }
}

pub fn show(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let path = session_path(session_id, config)?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session {}", path.display()))?;
    Ok(format!("{content}\n"))
}

pub async fn transcript(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let messages = JsonlSessionReader::replay_messages(session_path(session_id, config)?)
        .await
        .with_context(|| format!("failed to replay session {session_id}"))?;
    let lines = messages
        .iter()
        .map(format_message)
        .collect::<Vec<_>>()
        .join("\n");

    if lines.is_empty() {
        Ok("empty session\n".to_owned())
    } else {
        Ok(format!("{lines}\n"))
    }
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
    if !is_safe_session_id(session_id) {
        bail!("invalid session id {session_id:?}");
    }
    Ok(config.sessions_dir.join(format!("{session_id}.jsonl")))
}

fn is_safe_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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
