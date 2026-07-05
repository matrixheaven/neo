//! Expand composer placeholder markers into concrete [`Content`] parts.

use std::collections::HashMap;
use std::sync::Arc;

use neo_agent_core::{Content, ImageRef};
use neo_tui::{
    paste::{ImageAttachmentStore, Marker, parse_markers},
    transcript::TranscriptImageAttachment,
};

/// Expand `[paste ...]` and `[image #N ...]` markers in `text` into a mixed
/// text/image content vector. Adjacent text parts are coalesced.
pub fn expand_prompt_markers(
    text: &str,
    paste_store: &HashMap<usize, String>,
    image_store: &ImageAttachmentStore,
) -> Vec<Content> {
    let mut parts = Vec::new();
    let mut cursor = 0;
    for (start, marker) in parse_markers(text) {
        if start > cursor {
            parts.push(Content::Text {
                text: text[cursor..start].into(),
            });
        }
        match marker {
            Marker::Paste { id, .. } => {
                if let Some(original) = paste_store.get(&id) {
                    parts.push(Content::Text {
                        text: original.as_str().into(),
                    });
                }
            }
            Marker::Image { id, .. } => {
                if let Some(att) = image_store.get(id) {
                    // If we have the raw bytes (pasted before session existed),
                    // inline-encode as Base64 to avoid needing a session dir.
                    // Otherwise use a Blob reference (resolved by the runtime).
                    let data = if let Some(bytes) = image_store.pending_bytes(id) {
                        ImageRef::Base64(Arc::from(
                            base64::Engine::encode(
                                &base64::engine::general_purpose::STANDARD,
                                bytes,
                            )
                            .as_str(),
                        ))
                    } else {
                        ImageRef::Blob(att.sha256.as_str().into())
                    };
                    parts.push(Content::Image {
                        mime_type: att.mime_type.as_str().into(),
                        data,
                    });
                }
            }
        }
        cursor = start + marker.as_placeholder().len();
    }
    if cursor < text.len() {
        parts.push(Content::Text {
            text: text[cursor..].into(),
        });
    }
    coalesce_text_parts(parts)
}

pub fn transcript_image_attachments(
    text: &str,
    image_store: &ImageAttachmentStore,
) -> Vec<TranscriptImageAttachment> {
    parse_markers(text)
        .into_iter()
        .filter_map(|(_, marker)| match marker {
            Marker::Image { id, .. } => {
                let attachment = image_store.get(id)?;
                let bytes = image_store.pending_bytes(id)?.clone();
                Some(TranscriptImageAttachment::new(
                    format!("image-{id}"),
                    attachment.mime_type.clone(),
                    attachment.width,
                    attachment.height,
                    marker.as_placeholder(),
                    bytes,
                ))
            }
            Marker::Paste { .. } => None,
        })
        .collect()
}

fn coalesce_text_parts(parts: Vec<Content>) -> Vec<Content> {
    let mut out = Vec::new();
    for part in parts {
        match (out.last_mut(), &part) {
            (Some(Content::Text { text: last }), Content::Text { text: next }) => {
                let mut s = String::from(&**last);
                s.push_str(next);
                *last = Arc::from(s);
            }
            _ => out.push(part),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_mixed_text_and_image() {
        let mut image_store = ImageAttachmentStore::new();
        image_store.add("abc".into(), "image/png".into(), 100, 100, None);
        let parts = expand_prompt_markers(
            "hello [image #1 (100x100)] world",
            &HashMap::new(),
            &image_store,
        );
        assert_eq!(parts.len(), 3);
        assert!(matches!(parts[1], Content::Image { .. }));
    }

    #[test]
    fn expand_paste_marker_to_text() {
        let mut paste_store = HashMap::new();
        paste_store.insert(1, "pasted content".into());
        let parts = expand_prompt_markers(
            "before [paste #1 chars] after",
            &paste_store,
            &ImageAttachmentStore::new(),
        );
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].as_text(), Some("before pasted content after"));
    }
}
