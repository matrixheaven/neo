//! Image blob resolution — replaces `ImageRef::Blob` with `ImageRef::Base64`
//! by reading `<session_dir>/blobs/<sha256>.*`.

use crate::{AgentMessage, Content, ImageRef};

/// Recursively replace `ImageRef::Blob` with `ImageRef::Base64` by reading
/// `<session_dir>/blobs/<sha256>.*`. If the blob file is missing or the
/// session directory is unknown, the blob is replaced with an empty base64.
pub(crate) async fn resolve_image_blobs(
    messages: Vec<AgentMessage>,
    session_dir: Option<&std::path::Path>,
) -> Vec<AgentMessage> {
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        out.push(match message {
            AgentMessage::User {
                content,
                display_text,
                origin,
            } => AgentMessage::User {
                content: resolve_content_blobs(content, session_dir).await,
                display_text,
                origin,
            },
            AgentMessage::Assistant {
                content,
                tool_calls,
                stop_reason,
            } => AgentMessage::Assistant {
                content: resolve_content_blobs(content, session_dir).await,
                tool_calls,
                stop_reason,
            },
            AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
            } => AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                content: resolve_content_blobs(content, session_dir).await,
                is_error,
            },
            AgentMessage::System { content } => AgentMessage::System {
                content: resolve_content_blobs(content, session_dir).await,
            },
            // Pinned instruction context is exact text: pass it through
            // unchanged.
            message @ AgentMessage::Instruction { .. } => message,
            AgentMessage::ShellCommand {
                command,
                stdout,
                stderr,
                exit_code,
                outcome,
                truncated,
            } => AgentMessage::ShellCommand {
                command,
                stdout,
                stderr,
                exit_code,
                outcome,
                truncated,
            },
        });
    }
    out
}

pub(super) async fn resolve_content_blobs(
    content: Vec<Content>,
    session_dir: Option<&std::path::Path>,
) -> Vec<Content> {
    let mut out = Vec::with_capacity(content.len());
    for part in content {
        out.push(match part {
            Content::Image {
                mime_type,
                data: ImageRef::Blob(sha256),
            } => {
                let bytes = if let Some(dir) = session_dir {
                    read_blob_bytes(dir, &sha256).await.unwrap_or_default()
                } else {
                    Vec::new()
                };
                Content::Image {
                    mime_type,
                    data: ImageRef::Base64(
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes)
                            .into(),
                    ),
                }
            }
            other => other,
        });
    }
    out
}

pub(super) async fn read_blob_bytes(
    session_dir: &std::path::Path,
    sha256: &str,
) -> Option<Vec<u8>> {
    let blob_dir = session_dir.join("blobs");

    // Fast path: try direct file name `<sha256>.bin` to avoid directory scan.
    let direct_path = blob_dir.join(format!("{sha256}.bin"));
    if let Ok(bytes) = tokio::fs::read(&direct_path).await {
        return Some(bytes);
    }

    // Fallback: directory scan for any file starting with <sha256>.
    let mut entries = tokio::fs::read_dir(&blob_dir).await.ok()?;
    while let Some(entry) = entries.next_entry().await.ok()? {
        let name = entry.file_name();
        let name = name.to_str()?;
        if name.starts_with(sha256) {
            return tokio::fs::read(entry.path()).await.ok();
        }
    }
    None
}
