//! Expand composer placeholder markers into concrete [`Content`] parts.

use std::collections::HashMap;

use neo_agent_core::{Content, ImageRef};
use neo_tui::paste::{ImageAttachmentStore, Marker, parse_markers};

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
                        text: original.clone(),
                    });
                }
            }
            Marker::Image { id, .. } => {
                if let Some(att) = image_store.get(id) {
                    // If we have the raw bytes (pasted before session existed),
                    // inline-encode as Base64 to avoid needing a session dir.
                    // Otherwise use a Blob reference (resolved by the runtime).
                    let data = if let Some(bytes) = image_store.pending_bytes(id) {
                        ImageRef::Base64(base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            bytes,
                        ))
                    } else {
                        ImageRef::Blob(att.sha256.clone())
                    };
                    parts.push(Content::Image {
                        mime_type: att.mime_type.clone(),
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

fn coalesce_text_parts(parts: Vec<Content>) -> Vec<Content> {
    let mut out = Vec::new();
    for part in parts {
        match (out.last_mut(), &part) {
            (Some(Content::Text { text: last }), Content::Text { text: next }) => {
                last.push_str(next);
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
            "before [paste 1 chars] after",
            &paste_store,
            &ImageAttachmentStore::new(),
        );
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].as_text(), Some("before pasted content after"));
    }
}
