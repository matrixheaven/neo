//! Media blob resolution — replaces `MediaRef::Blob` with `MediaRef::Base64`
//! by reading `<session_dir>/blobs/<sha256>.*`.
//!
//! Missing or unreadable blobs never become empty encodings: the media part
//! is replaced with a deterministic "media unavailable" text part so the
//! request copy is stable per lane and never claims media that was not read.

use std::sync::Arc;

use neo_ai::AiError;

use crate::{AgentMessage, Content, MediaRef};

/// Upper bound for one inline video payload, in raw blob bytes. Videos are the
/// only media kind that may legally be large, so inlining is bounded: reading
/// stops at `MAX_INLINE_VIDEO_BYTES + 1` and an over-limit video fails the
/// request with a typed error before any provider call. Base64 expansion
/// (4/3) is included in the bound's intent: 32 MiB raw stays under ~43 MiB
/// encoded, leaving headroom inside the 300–500 MB runtime memory target.
pub(crate) const MAX_INLINE_VIDEO_BYTES: usize = 32 * 1024 * 1024;

/// Recursively replace `MediaRef::Blob` with `MediaRef::Base64` by reading
/// `<session_dir>/blobs/<sha256>.*`. If the blob file is missing or the
/// session directory is unknown, the media part is replaced with a
/// deterministic "media unavailable" text part. Video blobs are read with a
/// bounded read; a video larger than [`MAX_INLINE_VIDEO_BYTES`] fails with a
/// typed error instead of producing an oversized request copy.
pub(crate) async fn resolve_media_blobs(
    messages: Vec<AgentMessage>,
    session_dir: Option<&std::path::Path>,
) -> Result<Vec<AgentMessage>, AiError> {
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        out.push(match message {
            AgentMessage::User {
                content,
                display_text,
                origin,
            } => AgentMessage::User {
                content: resolve_content_blobs(content, session_dir).await?,
                display_text,
                origin,
            },
            AgentMessage::Assistant {
                content,
                tool_calls,
                stop_reason,
            } => AgentMessage::Assistant {
                content: resolve_content_blobs(content, session_dir).await?,
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
                content: resolve_content_blobs(content, session_dir).await?,
                is_error,
            },
            AgentMessage::System { content } => AgentMessage::System {
                content: resolve_content_blobs(content, session_dir).await?,
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
    Ok(out)
}

pub(super) async fn resolve_content_blobs(
    content: Vec<Content>,
    session_dir: Option<&std::path::Path>,
) -> Result<Vec<Content>, AiError> {
    let mut out = Vec::with_capacity(content.len());
    for part in content {
        out.push(match part {
            Content::Image {
                mime_type,
                data: MediaRef::Blob(sha256),
            } => resolve_media_blob_part("image", mime_type, sha256, session_dir, None).await?,
            Content::Video {
                mime_type,
                data: MediaRef::Blob(sha256),
            } => {
                resolve_media_blob_part(
                    "video",
                    mime_type,
                    sha256,
                    session_dir,
                    Some(MAX_INLINE_VIDEO_BYTES),
                )
                .await?
            }
            other => other,
        });
    }
    Ok(out)
}

async fn resolve_media_blob_part(
    kind: &str,
    mime_type: Arc<str>,
    sha256: Arc<str>,
    session_dir: Option<&std::path::Path>,
    max_bytes: Option<usize>,
) -> Result<Content, AiError> {
    let bytes = if let Some(dir) = session_dir {
        match max_bytes {
            Some(max) => read_blob_bytes_bounded(dir, &sha256, max).await,
            None => read_blob_bytes(dir, &sha256).await,
        }
    } else {
        None
    };
    let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) else {
        // Deterministic "media unavailable" state for missing or corrupt
        // blobs: never an empty encoding.
        return Ok(Content::text(format!(
            "[unavailable {kind}: blob {sha256}]"
        )));
    };
    if let Some(max) = max_bytes
        && bytes.len() > max
    {
        return Err(AiError::Configuration {
            message: format!(
                "{kind} blob {sha256} exceeds the inline size limit of {max} bytes; \
                 the request was not sent"
            ),
        });
    }
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes).into();
    if kind == "video" {
        Ok(Content::Video {
            mime_type,
            data: MediaRef::Base64(encoded),
        })
    } else {
        Ok(Content::Image {
            mime_type,
            data: MediaRef::Base64(encoded),
        })
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

/// Bounded variant of [`read_blob_bytes`]: reads at most `max_bytes + 1` raw
/// bytes so the caller can detect an over-limit blob without ever holding the
/// full payload in memory.
async fn read_blob_bytes_bounded(
    session_dir: &std::path::Path,
    sha256: &str,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    let blob_dir = session_dir.join("blobs");
    let direct_path = blob_dir.join(format!("{sha256}.bin"));
    if let Ok(file) = tokio::fs::File::open(&direct_path).await {
        return read_bounded(file, max_bytes).await;
    }
    let mut entries = tokio::fs::read_dir(&blob_dir).await.ok()?;
    while let Some(entry) = entries.next_entry().await.ok()? {
        let name = entry.file_name();
        let name = name.to_str()?;
        if name.starts_with(sha256) {
            let file = tokio::fs::File::open(entry.path()).await.ok()?;
            return read_bounded(file, max_bytes).await;
        }
    }
    None
}

async fn read_bounded(file: tokio::fs::File, max_bytes: usize) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let mut limited = file.take((max_bytes as u64).saturating_add(1));
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes).await.ok()?;
    Some(bytes)
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
        .await
        .expect("resolve");

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
        .await
        .expect("resolve");

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
        .await
        .expect("resolve");

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
        .await
        .expect("resolve");

        assert_eq!(
            resolved,
            vec![Content::text("[unavailable video: blob empty]")],
            "an existing but empty blob is corrupt and must not produce an empty encoding"
        );
    }
}
