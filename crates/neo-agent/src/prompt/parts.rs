//! Expand composer placeholder markers into concrete [`Content`] parts.

use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::{Component, Path};
use std::sync::Arc;

use neo_agent_core::{Content, ImageRef};
use neo_tui::{
    paste::{
        FileReference, FileReferenceKind, FileReferenceStore, ImageAttachmentStore, Marker,
        parse_markers,
    },
    transcript::TranscriptImageAttachment,
};

const MAX_FILE_REFERENCE_BYTES: usize = 64 * 1024;
const MAX_DIRECTORY_REFERENCE_ENTRIES: usize = 80;

/// Expand composer markers in `text` into a mixed text/image content vector.
/// Adjacent text parts are coalesced.
pub fn expand_prompt_markers(
    text: &str,
    paste_store: &HashMap<usize, String>,
    image_store: &ImageAttachmentStore,
    file_store: &FileReferenceStore,
    workspace_root: &Path,
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
            Marker::File { id, .. } => {
                if let Some(reference) = file_store.get(id) {
                    parts.push(Content::Text {
                        text: expand_file_reference(reference, workspace_root).into(),
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
            Marker::Paste { .. } | Marker::File { .. } => None,
        })
        .collect()
}

fn expand_file_reference(reference: &FileReference, workspace_root: &Path) -> String {
    let path = display_relative_path(&reference.relative_path);
    let Some(absolute) = resolve_file_reference_path(&reference.relative_path, workspace_root)
    else {
        return format!("<file path=\"{path}\" error=\"outside workspace\" />");
    };
    if !absolute.exists() {
        return format!("<file path=\"{path}\" error=\"not found\" />");
    }
    match reference.kind {
        FileReferenceKind::File => expand_file_reference_file(&absolute, &reference.relative_path),
        FileReferenceKind::Directory => {
            expand_file_reference_directory(&absolute, &reference.relative_path)
        }
    }
}

fn resolve_file_reference_path(path: &Path, workspace_root: &Path) -> Option<std::path::PathBuf> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }

    let candidate = workspace_root.join(path);
    let Ok(canonical_root) = fs::canonicalize(workspace_root) else {
        return Some(candidate);
    };
    match fs::canonicalize(&candidate) {
        Ok(canonical_candidate) if canonical_candidate.starts_with(&canonical_root) => {
            Some(canonical_candidate)
        }
        Ok(_) => None,
        Err(_) => Some(candidate),
    }
}

fn expand_file_reference_file(absolute: &Path, path: &Path) -> String {
    let path = display_relative_path(path);
    let Ok(metadata) = fs::metadata(absolute) else {
        return format!("<file path=\"{path}\" error=\"not found\" />");
    };
    if !metadata.is_file() {
        return format!("<file path=\"{path}\" error=\"not found\" />");
    }
    let bytes_len = metadata.len();
    let read_limit = usize::try_from(bytes_len)
        .unwrap_or(usize::MAX)
        .min(MAX_FILE_REFERENCE_BYTES.saturating_add(1));
    let Ok(file) = fs::File::open(absolute) else {
        return format!("<file path=\"{path}\" error=\"not found\" />");
    };
    let mut bytes = Vec::with_capacity(read_limit);
    if file
        .take(u64::try_from(read_limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .is_err()
    {
        return format!("<file path=\"{path}\" error=\"not found\" />");
    }
    if bytes.contains(&0) {
        return format!("<file path=\"{path}\" type=\"binary\" bytes=\"{bytes_len}\" />");
    }

    let truncated = bytes.len() > MAX_FILE_REFERENCE_BYTES;
    if truncated {
        bytes.truncate(MAX_FILE_REFERENCE_BYTES);
    }

    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_owned(),
        Err(err) if truncated && err.error_len().is_none() => {
            let valid_up_to = err.valid_up_to();
            bytes.truncate(valid_up_to);
            String::from_utf8(bytes).unwrap_or_default()
        }
        Err(_) => {
            return format!("<file path=\"{path}\" type=\"non-utf8\" bytes=\"{bytes_len}\" />");
        }
    };
    if truncated {
        format!("<file path=\"{path}\">\n{text}\n[truncated]\n</file>")
    } else {
        format!("<file path=\"{path}\">\n{text}</file>")
    }
}

fn expand_file_reference_directory(absolute: &Path, path: &Path) -> String {
    let path = display_relative_path(path);
    let Ok(metadata) = fs::metadata(absolute) else {
        return format!("<file path=\"{path}\" error=\"not found\" />");
    };
    if !metadata.is_dir() {
        return format!("<file path=\"{path}\" error=\"not found\" />");
    }
    let Ok(read_dir) = fs::read_dir(absolute) else {
        return format!("<directory path=\"{path}\">\n</directory>");
    };
    let mut entries = Vec::new();
    for entry in read_dir {
        let Some(name) = entry.ok().and_then(|entry| {
            let file_name = entry.file_name();
            if file_name == ".git" {
                return None;
            }
            let mut name = file_name.to_string_lossy().into_owned();
            if entry.file_type().ok()?.is_dir() {
                name.push('/');
            }
            Some(name)
        }) else {
            continue;
        };
        entries.push(name);
        if entries.len() > MAX_DIRECTORY_REFERENCE_ENTRIES {
            break;
        }
    }
    entries.sort();
    let truncated = entries.len() > MAX_DIRECTORY_REFERENCE_ENTRIES;
    entries.truncate(MAX_DIRECTORY_REFERENCE_ENTRIES);
    if truncated {
        entries.push("[truncated]".to_owned());
    }
    format!(
        "<directory path=\"{path}\">\n{}\n</directory>",
        entries.join("\n")
    )
}

fn display_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_owned()),
            Component::CurDir => None,
            Component::RootDir | Component::Prefix(_) => None,
        })
        .collect::<Vec<_>>()
        .join("/")
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
            &FileReferenceStore::new(),
            std::path::Path::new("."),
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
            &FileReferenceStore::new(),
            std::path::Path::new("."),
        );
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].as_text(), Some("before pasted content after"));
    }

    #[test]
    fn expand_file_reference_marker_to_file_block() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("src")).expect("mkdir");
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write file");

        let mut file_store = neo_tui::paste::FileReferenceStore::new();
        file_store.add(
            "workspace".to_owned(),
            std::path::PathBuf::from("src/main.rs"),
            neo_tui::paste::FileReferenceKind::File,
            "main.rs".to_owned(),
        );

        let parts = expand_prompt_markers(
            "review [file #1 main.rs]",
            &HashMap::new(),
            &ImageAttachmentStore::new(),
            &file_store,
            temp.path(),
        );

        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts[0].as_text(),
            Some("review <file path=\"src/main.rs\">\nfn main() {}\n</file>")
        );
    }

    #[test]
    fn expand_directory_reference_marker_to_bounded_directory_block() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("docs/nested")).expect("mkdir");
        std::fs::write(temp.path().join("docs/a.md"), "a").expect("write a");
        std::fs::write(temp.path().join("docs/nested/b.md"), "b").expect("write b");

        let mut file_store = neo_tui::paste::FileReferenceStore::new();
        file_store.add(
            "workspace".to_owned(),
            std::path::PathBuf::from("docs"),
            neo_tui::paste::FileReferenceKind::Directory,
            "docs/".to_owned(),
        );

        let parts = expand_prompt_markers(
            "[file #1 docs/]",
            &HashMap::new(),
            &ImageAttachmentStore::new(),
            &file_store,
            temp.path(),
        );
        let text = parts[0].as_text().expect("text part");

        assert!(text.contains("<directory path=\"docs\">"), "{text}");
        assert!(text.contains("a.md"), "{text}");
        assert!(text.contains("nested/"), "{text}");
        assert!(text.contains("</directory>"), "{text}");
    }

    #[test]
    fn expand_missing_file_reference_marker_to_error_block() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut file_store = neo_tui::paste::FileReferenceStore::new();
        file_store.add(
            "workspace".to_owned(),
            std::path::PathBuf::from("missing.rs"),
            neo_tui::paste::FileReferenceKind::File,
            "missing.rs".to_owned(),
        );

        let parts = expand_prompt_markers(
            "[file #1 missing.rs]",
            &HashMap::new(),
            &ImageAttachmentStore::new(),
            &file_store,
            temp.path(),
        );

        assert_eq!(
            parts[0].as_text(),
            Some("<file path=\"missing.rs\" error=\"not found\" />")
        );
    }

    #[test]
    fn expand_parent_directory_file_reference_to_error_block() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = temp.path().join("outside.rs");
        std::fs::write(&outside, "secret").expect("write outside");

        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        let mut file_store = neo_tui::paste::FileReferenceStore::new();
        file_store.add(
            "workspace".to_owned(),
            std::path::PathBuf::from("../outside.rs"),
            neo_tui::paste::FileReferenceKind::File,
            "outside.rs".to_owned(),
        );

        let parts = expand_prompt_markers(
            "[file #1 outside.rs]",
            &HashMap::new(),
            &ImageAttachmentStore::new(),
            &file_store,
            &workspace,
        );

        assert_eq!(
            parts[0].as_text(),
            Some("<file path=\"../outside.rs\" error=\"outside workspace\" />")
        );
    }

    #[test]
    fn expand_absolute_file_reference_to_error_block() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = temp.path().join("outside.rs");
        std::fs::write(&outside, "secret").expect("write outside");
        let display_path = display_relative_path(&outside);

        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("mkdir workspace");
        let mut file_store = neo_tui::paste::FileReferenceStore::new();
        file_store.add(
            "workspace".to_owned(),
            outside,
            neo_tui::paste::FileReferenceKind::File,
            "outside.rs".to_owned(),
        );

        let parts = expand_prompt_markers(
            "[file #1 outside.rs]",
            &HashMap::new(),
            &ImageAttachmentStore::new(),
            &file_store,
            &workspace,
        );

        let expected = format!("<file path=\"{display_path}\" error=\"outside workspace\" />");
        assert_eq!(parts[0].as_text(), Some(expected.as_str()));
    }
}
