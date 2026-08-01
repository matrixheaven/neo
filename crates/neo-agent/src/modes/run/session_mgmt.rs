use std::path::{Path, PathBuf};

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::session::{SessionMetadataStore, main_agent_wire_path};
use neo_ai::{ChatMessage, ChatRequest, ContentPart, ModelSpec, RequestOptions};

use crate::config::{AppConfig, workspace_sessions_dir};

use super::PromptTurn;

pub(super) fn session_id_from_path(path: &Path) -> anyhow::Result<String> {
    let session_dir = session_root_from_wire_path(path)?;
    let dir_name = session_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session directory name {}", session_dir.display()))?;

    Ok(dir_name.to_owned())
}

pub(super) fn session_root_from_wire_path(path: &Path) -> anyhow::Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session path {}", path.display()))?;

    if file_name != "wire.jsonl" {
        anyhow::bail!("invalid session wire path {}", path.display());
    }

    let main_dir = path
        .parent()
        .with_context(|| format!("session wire has no parent directory {}", path.display()))?;
    if main_dir.file_name().and_then(std::ffi::OsStr::to_str) != Some("main") {
        anyhow::bail!("invalid main agent wire path {}", path.display());
    }
    let agents_dir = main_dir
        .parent()
        .with_context(|| format!("main agent directory has no parent {}", main_dir.display()))?;
    if agents_dir.file_name().and_then(std::ffi::OsStr::to_str) != Some("agents") {
        anyhow::bail!("invalid agents directory {}", agents_dir.display());
    }
    let session_dir = agents_dir.parent().with_context(|| {
        format!(
            "agents directory has no session parent {}",
            agents_dir.display()
        )
    })?;
    let dir_name = session_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session directory name {}", session_dir.display()))?;

    neo_agent_core::session::validate_session_id(dir_name)
        .map_err(|_| anyhow::anyhow!("invalid session id {dir_name:?}"))?;
    Ok(session_dir.to_path_buf())
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
        let transcript = main_agent_wire_path(&path);
        if !transcript.is_file() {
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
    let Ok(bucket_dir) = crate::modes::sessions::session_bucket_dir(session_id, config) else {
        return;
    };
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
    if record.name.is_some() || record.title_updated_at.is_some() {
        return;
    }

    let fallback = one_line(prompt, 40);
    let (title, model_label) =
        match generate_session_title(config, prompt, &turn.assistant_text).await {
            Ok((title, model_label)) if !title.is_empty() => (title, Some(model_label)),
            Ok((_, _)) => {
                tracing::warn!(
                    "session {}: title generation returned an empty title; \
                     using a prompt truncation fallback",
                    turn.session_id
                );
                (fallback, None)
            }
            Err(error) => {
                tracing::warn!(
                    error = ?error,
                    "session {}: title generation failed; using a prompt truncation fallback",
                    turn.session_id
                );
                (fallback, None)
            }
        };
    let _ = store.record_title(
        &turn.session_id,
        title,
        model_label,
        super::output::current_unix_timestamp(),
    );
}

fn title_request(model: ModelSpec, prompt: &str, assistant_text: &str) -> ChatRequest {
    ChatRequest {
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
            max_tokens: Some(512),
            temperature: Some(0.2),
            disable_reasoning: true,
            ..RequestOptions::default()
        },
    }
}

async fn generate_session_title(
    config: &AppConfig,
    prompt: &str,
    assistant_text: &str,
) -> anyhow::Result<(String, String)> {
    let model = super::runtime::resolve_model(config)?;
    let client = super::runtime::resolve_model_client(config, &model)?;
    let model_label = format!("{}/{}", model.provider.0, model.model);
    let request = title_request(model, prompt, assistant_text);
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

#[cfg(test)]
mod tests {
    use super::*;
    use neo_ai::types::ApiKind;
    use neo_ai::{ModelCapabilities, ProviderId};

    fn spec() -> ModelSpec {
        ModelSpec {
            provider: ProviderId("deepseek".to_owned()),
            model: "deepseek-test".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat(),
        }
    }

    fn text_of(message: &ChatMessage) -> String {
        let content = match message {
            ChatMessage::System { content } | ChatMessage::User { content } => content,
            _ => return String::new(),
        };
        content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn title_request_is_fast_and_deterministic() {
        let request = title_request(
            spec(),
            "read the handoff and complete it",
            "I completed the handoff.",
        );

        assert_eq!(request.options.max_tokens, Some(512));
        assert_eq!(request.options.temperature, Some(0.2));
        assert!(
            request.options.disable_reasoning,
            "title requests must not trigger provider reasoning"
        );
        assert!(request.tools.is_empty(), "title requests carry no tools");

        let mut system = String::new();
        let mut user = String::new();
        for message in &request.messages {
            match message {
                ChatMessage::System { .. } => system = text_of(message),
                ChatMessage::User { .. } => user = text_of(message),
                _ => {}
            }
        }
        assert_eq!(
            system,
            "Generate a concise session title. Return only the title, no quotes."
        );
        assert_eq!(
            user,
            "User prompt:\nread the handoff and complete it\n\nAssistant response:\nI completed the handoff."
        );
    }
}
