use std::path::{Path, PathBuf};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::session::SessionMetadataStore;
use neo_ai::{ChatMessage, ContentPart, RequestOptions};
use uuid::Uuid;

use crate::config::{AppConfig, workspace_sessions_dir};

use super::PromptTurn;

pub(super) async fn create_session_path(config: &AppConfig) -> anyhow::Result<PathBuf> {
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
            return Ok(session_dir.join("transcript.jsonl"));
        }
    }
}

pub(super) fn session_id_from_path(path: &Path) -> anyhow::Result<String> {
    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session path {}", path.display()))?;

    if file_name != "transcript.jsonl" {
        anyhow::bail!("invalid session path {}", path.display());
    }

    let session_dir = path.parent().with_context(|| {
        format!(
            "session transcript has no parent directory {}",
            path.display()
        )
    })?;
    let dir_name = session_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session directory name {}", session_dir.display()))?;
    if dir_name.starts_with("session_") {
        return Ok(dir_name.to_owned());
    }

    anyhow::bail!("invalid session path {}", path.display())
}

pub(crate) fn latest_session_id(config: &AppConfig) -> anyhow::Result<String> {
    let bucket_dir = workspace_sessions_dir(config);
    let mut latest: Option<(std::time::SystemTime, String)> = None;
    let entries = std::fs::read_dir(&bucket_dir)
        .with_context(|| format!("failed to read sessions directory {}", bucket_dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("session_") {
            continue;
        }
        let transcript = path.join("transcript.jsonl");
        if !transcript.exists() {
            continue;
        }
        let Ok(session_id) = session_id_from_path(&transcript) else {
            continue;
        };
        if neo_agent_core::session::validate_session_id(&session_id).is_err() {
            continue;
        }
        let modified = std::fs::metadata(&transcript)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let should_replace = latest.as_ref().is_none_or(|(latest_modified, latest_id)| {
            modified > *latest_modified || (modified == *latest_modified && session_id > *latest_id)
        });
        if should_replace {
            latest = Some((modified, session_id));
        }
    }

    latest
        .map(|(_, session_id)| session_id)
        .with_context(|| format!("no sessions found in {}", bucket_dir.display()))
}

pub(super) fn record_session_activity(config: &AppConfig, session_id: &str, prompt: &str) {
    let bucket_dir = workspace_sessions_dir(config);
    let _ = SessionMetadataStore::new(&bucket_dir).record_activity(
        session_id,
        Some(config.project_dir.display().to_string()),
        Some(one_line(prompt, 240)),
        super::output::current_unix_timestamp(),
    );
}

pub(super) async fn record_initial_session_title(
    config: &AppConfig,
    turn: &PromptTurn,
    prompt: &str,
) {
    let bucket_dir = workspace_sessions_dir(config);
    let store = SessionMetadataStore::new(&bucket_dir);
    let Ok(sessions) = store.list() else {
        return;
    };
    let Some(record) = sessions
        .into_iter()
        .find(|session| session.id == turn.session_id)
    else {
        return;
    };
    if record.name.is_some() || record.title_model.is_some() {
        return;
    }

    let fallback = one_line(prompt, 40);
    let (title, model_label) =
        match generate_session_title(config, prompt, &turn.assistant_text).await {
            Ok((title, model_label)) if !title.is_empty() => (title, Some(model_label)),
            _ => (fallback, None),
        };
    let _ = store.record_title(
        &turn.session_id,
        title,
        model_label,
        super::output::current_unix_timestamp(),
    );
}

async fn generate_session_title(
    config: &AppConfig,
    prompt: &str,
    assistant_text: &str,
) -> anyhow::Result<(String, String)> {
    let model = super::runtime::resolve_model(config)?;
    let client = super::runtime::resolve_model_client(config, &model)?;
    let model_label = format!("{}/{}", model.provider.0, model.model);
    let request = neo_ai::ChatRequest {
        model,
        messages: vec![
            ChatMessage::System {
                content: vec![ContentPart::Text {
                    text: "Generate a concise session title. Return only the title, no quotes."
                        .to_owned(),
                }],
            },
            ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: format!(
                        "User prompt:\n{}\n\nAssistant response:\n{}",
                        one_line(prompt, 500),
                        one_line(assistant_text, 500)
                    ),
                }],
            },
        ],
        tools: Vec::new(),
        options: RequestOptions {
            max_tokens: Some(32),
            temperature: Some(0.2),
            ..RequestOptions::default()
        },
    };
    let events = client.stream_chat(request).collect::<Vec<_>>().await;
    let mut title = String::new();
    for event in events {
        if let neo_ai::AiStreamEvent::TextDelta { text } = event? {
            title.push_str(&text);
        }
    }
    Ok((clean_session_title(&title), model_label))
}

fn clean_session_title(title: &str) -> String {
    one_line(title.trim().trim_matches(['"', '\'', '`']), 40)
        .trim_matches(['*', '#'])
        .trim()
        .to_owned()
}

fn one_line(text: &str, max_chars: usize) -> String {
    let mut line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > max_chars {
        line = line.chars().take(max_chars.saturating_sub(1)).collect();
        line.push('…');
    }
    line
}
