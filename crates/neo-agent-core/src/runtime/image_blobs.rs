//! Media blob resolution — replaces `MediaRef::Blob` with `MediaRef::Base64`
//! by reading `<session_dir>/blobs/<sha256>.*`.
//!
//! Missing or unreadable blobs never become empty encodings: the media part
//! is replaced with a deterministic "media unavailable" text part so the
//! request copy is stable per lane and never claims media that was not read.

use std::sync::Arc;

use crate::{AgentMessage, Content, MediaRef};

/// Recursively replace `MediaRef::Blob` with `MediaRef::Base64` by reading
/// `<session_dir>/blobs/<sha256>.*`. If the blob file is missing or the
/// session directory is unknown, the media part is replaced with a
/// deterministic "media unavailable" text part.
pub(crate) async fn resolve_media_blobs(
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
                data: MediaRef::Blob(sha256),
            } => resolve_media_blob_part("image", mime_type, sha256, session_dir).await,
            Content::Video {
                mime_type,
                data: MediaRef::Blob(sha256),
            } => resolve_media_blob_part("video", mime_type, sha256, session_dir).await,
            other => other,
        });
    }
    out
}

async fn resolve_media_blob_part(
    kind: &str,
    mime_type: Arc<str>,
    sha256: Arc<str>,
    session_dir: Option<&std::path::Path>,
) -> Content {
    let bytes = if let Some(dir) = session_dir {
        read_blob_bytes(dir, &sha256).await
    } else {
        None
    };
    let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) else {
        // Deterministic "media unavailable" state for missing or corrupt
        // blobs: never an empty encoding.
        return Content::text(format!("[unavailable {kind}: blob {sha256}]"));
    };
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes).into();
    if kind == "video" {
        Content::Video {
            mime_type,
            data: MediaRef::Base64(encoded),
        }
    } else {
        Content::Image {
            mime_type,
            data: MediaRef::Base64(encoded),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_content_blobs_missing_blob_becomes_unavailable_text_not_empty_encoding() {
        let resolved = resolve_content_blobs(
            vec![
                Content::Image {
                    mime_type: "image/png".into(),
                    data: MediaRef::Blob("deadbeef".into()),
                },
                Content::Video {
                    mime_type: "video/mp4".into(),
                    data: MediaRef::Blob("deadbeef".into()),
                },
            ],
            None,
        )
        .await;

        assert_eq!(
            resolved,
            vec![
                Content::text("[unavailable image: blob deadbeef]"),
                Content::text("[unavailable video: blob deadbeef]"),
            ],
            "missing blobs must resolve to deterministic unavailable text, never empty encodings"
        );
    }

    #[tokio::test]
    async fn resolve_content_blobs_unreadable_blob_dir_becomes_unavailable_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No `blobs` directory exists: the read must fail closed.
        let resolved = resolve_content_blobs(
            vec![Content::Video {
                mime_type: "video/mp4".into(),
                data: MediaRef::Blob("missing".into()),
            }],
            Some(dir.path()),
        )
        .await;

        assert_eq!(
            resolved,
            vec![Content::text("[unavailable video: blob missing]")]
        );
    }

    #[tokio::test]
    async fn resolve_content_blobs_reads_image_and_video_blobs_to_base64() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blob_dir = dir.path().join("blobs");
        tokio::fs::create_dir(&blob_dir)
            .await
            .expect("create blob dir");
        tokio::fs::write(blob_dir.join("abc123.bin"), b"image-bytes")
            .await
            .expect("write image blob");
        tokio::fs::write(blob_dir.join("def456.bin"), b"video-bytes")
            .await
            .expect("write video blob");

        let resolved = resolve_content_blobs(
            vec![
                Content::Image {
                    mime_type: "image/png".into(),
                    data: MediaRef::Blob("abc123".into()),
                },
                Content::Video {
                    mime_type: "video/mp4".into(),
                    data: MediaRef::Blob("def456".into()),
                },
            ],
            Some(dir.path()),
        )
        .await;

        let encoded_image =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"image-bytes");
        let encoded_video =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"video-bytes");
        assert_eq!(
            resolved,
            vec![
                Content::Image {
                    mime_type: "image/png".into(),
                    data: MediaRef::Base64(encoded_image.into()),
                },
                Content::Video {
                    mime_type: "video/mp4".into(),
                    data: MediaRef::Base64(encoded_video.into()),
                },
            ]
        );
    }

    #[tokio::test]
    async fn resolve_content_blobs_empty_blob_file_becomes_unavailable_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blob_dir = dir.path().join("blobs");
        tokio::fs::create_dir(&blob_dir)
            .await
            .expect("create blob dir");
        tokio::fs::write(blob_dir.join("empty.bin"), b"")
            .await
            .expect("write empty blob");

        let resolved = resolve_content_blobs(
            vec![Content::Video {
                mime_type: "video/mp4".into(),
                data: MediaRef::Blob("empty".into()),
            }],
            Some(dir.path()),
        )
        .await;

        assert_eq!(
            resolved,
            vec![Content::text("[unavailable video: blob empty]")],
            "an existing but empty blob is corrupt and must not produce an empty encoding"
        );
    }
}
