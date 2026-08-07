//! Input composer paste markers and image attachments.

use regex::Regex;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Matches `[paste #ID +N lines]`, `[paste #ID chars]`, `[image #N (WxH)]`,
/// or `[file #N display-name]`.
pub fn marker_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\[(?:(paste)\s+#(\d+)\s+(?:\+(\d+)\s+lines|chars)|(image)\s+#(\d+)\s+\((\d+)x(\d+)\)|(file)\s+#(\d+)\s+([^\]]+))\]",
        )
            .expect("marker regex is valid")
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileReferenceKind {
    File,
    Directory,
}

/// A paste placeholder marker embedded in composer text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// Collapsed multi-line or large text paste.
    Paste { id: usize, lines: Option<usize> },
    /// Image attachment placeholder.
    Image { id: usize, width: u32, height: u32 },
    /// File reference placeholder.
    File { id: usize, display_name: String },
}

impl Marker {
    /// Return the attachment id.
    #[must_use]
    pub fn id(&self) -> usize {
        match self {
            Self::Paste { id, .. } | Self::Image { id, .. } | Self::File { id, .. } => *id,
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
            Self::File { id, display_name } => format!("[file #{id} {display_name}]"),
        }
    }

    /// Render the marker as a composer chip label.
    #[must_use]
    pub fn as_chip(&self) -> String {
        match self {
            Self::File { display_name, .. } => format!("@[{display_name}]"),
            _ => self.as_placeholder(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReference {
    pub id: usize,
    pub root_label: String,
    pub relative_path: PathBuf,
    pub kind: FileReferenceKind,
    pub display_name: String,
}

impl FileReference {
    #[must_use]
    pub fn as_marker(&self) -> Marker {
        Marker::File {
            id: self.id,
            display_name: self.display_name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileReferenceStore {
    next_id: usize,
    by_id: std::collections::BTreeMap<usize, FileReference>,
}

impl Default for FileReferenceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FileReferenceStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 1,
            by_id: std::collections::BTreeMap::new(),
        }
    }

    pub fn add(
        &mut self,
        root_label: String,
        relative_path: PathBuf,
        kind: FileReferenceKind,
        display_name: String,
    ) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.by_id.insert(
            id,
            FileReference {
                id,
                root_label,
                relative_path,
                kind,
                display_name,
            },
        );
        id
    }

    #[must_use]
    pub fn get(&self, id: usize) -> Option<&FileReference> {
        self.by_id.get(&id)
    }

    pub fn remove(&mut self, id: usize) -> Option<FileReference> {
        self.by_id.remove(&id)
    }

    pub fn clear(&mut self) {
        self.by_id.clear();
        self.next_id = 1;
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
        } else if cap.get(8).is_some() {
            let id = cap[9].parse().unwrap_or(0);
            let display_name = cap[10].to_owned();
            out.push((m.start(), Marker::File { id, display_name }));
        }
    }
    out
}

/// Replace composer markers with their compact presentation labels.
#[must_use]
pub fn markers_as_chips(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, marker) in parse_markers(text) {
        out.push_str(&text[cursor..start]);
        out.push_str(&marker.as_chip());
        cursor = start + marker.as_placeholder().len();
    }
    out.push_str(&text[cursor..]);
    out
}

#[must_use]
pub fn file_reference_chip_label(
    display_name: &str,
    kind: FileReferenceKind,
    max_name_width: usize,
) -> String {
    let mut name = display_name.trim().to_owned();
    if matches!(kind, FileReferenceKind::Directory) && !name.ends_with('/') {
        name.push('/');
    }
    let clipped = middle_truncate_filename(&name, max_name_width);
    format!("@[{clipped}]")
}

fn middle_truncate_filename(name: &str, max_width: usize) -> String {
    if crate::primitive::visible_width(name) <= max_width {
        return name.to_owned();
    }
    if max_width <= 1 {
        return "…".to_owned();
    }
    let extension = name
        .rsplit_once('.')
        .filter(|(stem, ext)| !stem.is_empty() && !ext.is_empty())
        .map(|(_, ext)| format!(".{ext}"))
        .unwrap_or_default();
    let suffix = extension
        .is_empty()
        .then_some("")
        .or_else(|| {
            let stem = name.strip_suffix(&extension)?;
            let (_, tail) = stem.rsplit_once('-')?;
            Some(&name[name.len() - tail.len() - extension.len()..])
        })
        .unwrap_or(extension.as_str());
    let suffix_width = crate::primitive::visible_width(suffix);
    if suffix_width + 2 > max_width {
        let suffix_budget = max_width.saturating_sub(1);
        let suffix_tail = if suffix_width <= suffix_budget {
            suffix.to_owned()
        } else {
            take_trailing_visible_width(suffix, suffix_budget)
        };
        return format!("…{suffix_tail}");
    }
    let available_head_width = max_width
        .saturating_sub(suffix_width)
        .saturating_sub(1)
        .max(1);
    let balanced_head_width = max_width.saturating_sub(3) * 3 / 5;
    let head_width = available_head_width.min(balanced_head_width.max(1));
    let mut head = String::new();
    let mut width = 0;
    for ch in name.chars() {
        let ch_width = crate::primitive::visible_width(&ch.to_string());
        if width + ch_width > head_width {
            break;
        }
        head.push(ch);
        width += ch_width;
    }
    format!("{head}…{suffix}")
}

fn take_trailing_visible_width(text: &str, max_width: usize) -> String {
    let mut tail = String::new();
    let mut width = 0;
    for ch in text.chars().rev() {
        let ch_width = crate::primitive::visible_width(&ch.to_string());
        if width + ch_width > max_width {
            break;
        }
        tail.insert(0, ch);
        width += ch_width;
    }
    tail
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
#[path = "paste/test_cases/parsing.rs"]
mod parsing;
