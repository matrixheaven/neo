//! Input composer paste markers and image attachments.

use regex::Regex;
use std::sync::OnceLock;

/// Matches `[paste #ID +N lines]`, `[paste #ID chars]`, or `[image #N (WxH)]`.
pub fn marker_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[(?:(paste)\s+#(\d+)\s+(?:\+(\d+)\s+lines|chars)|(image)\s+#(\d+)\s+\((\d+)x(\d+)\))\]")
            .expect("marker regex is valid")
    })
}

/// A paste placeholder marker embedded in composer text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// Collapsed multi-line or large text paste.
    Paste { id: usize, lines: Option<usize> },
    /// Image attachment placeholder.
    Image { id: usize, width: u32, height: u32 },
}

impl Marker {
    /// Return the attachment id.
    #[must_use]
    pub fn id(&self) -> usize {
        match self {
            Self::Paste { id, .. } | Self::Image { id, .. } => *id,
        }
    }

    /// Render the marker as it appears in the composer.
    #[must_use]
    pub fn as_placeholder(&self) -> String {
        match self {
            Self::Paste {
                id, lines: Some(n), ..
            } => format!("[paste #{id} +{n} lines]"),
            Self::Paste {
                id, lines: None, ..
            } => format!("[paste #{id} chars]"),
            Self::Image { id, width, height } => {
                format!("[image #{id} ({width}x{height})]")
            }
        }
    }
}

/// Parse markers in source order, returning the byte start position and marker.
#[must_use]
pub fn parse_markers(text: &str) -> Vec<(usize, Marker)> {
    let mut out = Vec::new();
    for cap in marker_regex().captures_iter(text) {
        let m = cap.get(0).expect("regex match has group 0");
        if cap.get(1).is_some() {
            // Paste marker: [paste #ID +N lines] or [paste #ID chars]
            let id = cap
                .get(2)
                .and_then(|c| c.as_str().parse().ok())
                .unwrap_or_else(|| out.len() + 1);
            if let Some(lines) = cap.get(3).and_then(|c| c.as_str().parse().ok()) {
                out.push((
                    m.start(),
                    Marker::Paste {
                        id,
                        lines: Some(lines),
                    },
                ));
            } else {
                out.push((m.start(), Marker::Paste { id, lines: None }));
            }
        } else if cap.get(4).is_some() {
            // Image marker: [image #ID (WxH)]
            let id = cap[5].parse().unwrap_or(0);
            let width = cap[6].parse().unwrap_or(0);
            let height = cap[7].parse().unwrap_or(0);
            out.push((m.start(), Marker::Image { id, width, height }));
        }
    }
    out
}

/// Metadata for an image attachment stored in the session blobs directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    pub id: usize,
    pub sha256: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
}

/// In-memory store for image attachments referenced by composer placeholders.
///
/// When images are pasted before a session exists, the raw bytes are kept in
/// `pending_bytes` and flushed to the session blob directory on submit.
#[derive(Debug, Default, Clone)]
pub struct ImageAttachmentStore {
    next_id: usize,
    by_id: std::collections::BTreeMap<usize, ImageAttachment>,
    pending_bytes: std::collections::BTreeMap<usize, Vec<u8>>,
}

impl ImageAttachmentStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 1,
            by_id: std::collections::BTreeMap::new(),
            pending_bytes: std::collections::BTreeMap::new(),
        }
    }

    /// Register an image and return its attachment id.
    /// If `bytes` are provided, they are stored for lazy blob writing.
    pub fn add(
        &mut self,
        sha256: String,
        mime_type: String,
        width: u32,
        height: u32,
        bytes: Option<Vec<u8>>,
    ) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.by_id.insert(
            id,
            ImageAttachment {
                id,
                sha256,
                mime_type,
                width,
                height,
            },
        );
        if let Some(b) = bytes {
            self.pending_bytes.insert(id, b);
        }
        id
    }

    /// Look up an attachment by id.
    #[must_use]
    pub fn get(&self, id: usize) -> Option<&ImageAttachment> {
        self.by_id.get(&id)
    }

    /// Get pending (unsaved) bytes for an attachment.
    #[must_use]
    pub fn pending_bytes(&self, id: usize) -> Option<&Vec<u8>> {
        self.pending_bytes.get(&id)
    }

    /// Remove an attachment by id.
    pub fn remove(&mut self, id: usize) -> Option<ImageAttachment> {
        self.by_id.remove(&id)
    }

    /// Find an attachment by SHA-256.
    #[must_use]
    pub fn find_by_sha256(&self, sha256: &str) -> Option<&ImageAttachment> {
        self.by_id.values().find(|a| a.sha256 == sha256)
    }

    /// Clear all attachments and reset the id counter.
    pub fn clear(&mut self) {
        self.by_id.clear();
        self.next_id = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paste_lines_marker() {
        let text = "hello [paste #1 +15 lines] world";
        let markers = parse_markers(text);
        assert_eq!(markers.len(), 1);
        assert!(
            matches!(
                markers[0].1,
                Marker::Paste {
                    id: 1,
                    lines: Some(15),
                }
            ),
            "expected paste lines marker"
        );
    }

    #[test]
    fn parses_paste_chars_marker() {
        let text = "hello [paste #3 chars] world";
        let markers = parse_markers(text);
        assert_eq!(markers.len(), 1);
        assert!(
            matches!(markers[0].1, Marker::Paste { id: 3, lines: None }),
            "expected paste chars marker"
        );
    }

    #[test]
    fn parses_image_marker() {
        let text = "look [image #3 (640x480)] here";
        let markers = parse_markers(text);
        assert_eq!(markers.len(), 1);
        assert!(
            matches!(
                markers[0].1,
                Marker::Image {
                    id: 3,
                    width: 640,
                    height: 480
                }
            ),
            "expected image marker"
        );
    }

    #[test]
    fn parses_multiple_markers() {
        let text = "[paste #1 +1 lines][image #1 (10x20)][paste #2 chars]";
        let markers = parse_markers(text);
        assert_eq!(markers.len(), 3);
    }

    #[test]
    fn attachment_store_assigns_incrementing_ids() {
        let mut store = ImageAttachmentStore::new();
        let id1 = store.add("a".into(), "image/png".into(), 100, 100, None);
        let id2 = store.add("b".into(), "image/jpeg".into(), 200, 200, None);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(store.get(id1).unwrap().sha256, "a");
    }

    #[test]
    fn attachment_store_finds_by_sha256() {
        let mut store = ImageAttachmentStore::new();
        store.add("abc".into(), "image/png".into(), 100, 100, None);
        assert!(store.find_by_sha256("abc").is_some());
        assert!(store.find_by_sha256("missing").is_none());
    }
}
