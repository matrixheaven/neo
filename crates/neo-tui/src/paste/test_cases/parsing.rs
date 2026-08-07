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
fn parses_file_reference_marker() {
    let text = "read [file #7 prompt_completion.rs] now";
    let markers = parse_markers(text);
    assert_eq!(markers.len(), 1);
    assert!(
        matches!(
            &markers[0].1,
            Marker::File {
                id: 7,
                display_name
            } if display_name == "prompt_completion.rs"
        ),
        "expected file reference marker: {markers:?}"
    );
}

#[test]
fn file_reference_placeholder_roundtrips_display_name() {
    let marker = Marker::File {
        id: 3,
        display_name: "prompt_completion.rs".to_owned(),
    };

    assert_eq!(marker.as_placeholder(), "[file #3 prompt_completion.rs]");
    assert_eq!(marker.as_chip(), "@[prompt_completion.rs]");
}

#[test]
fn markers_as_chips_preserves_text_and_compacts_file_references() {
    let text = "compare [file #1 prompt_completion.rs] with [file #2 paste.rs]";

    assert_eq!(
        markers_as_chips(text),
        "compare @[prompt_completion.rs] with @[paste.rs]"
    );
}

#[test]
fn file_reference_chip_middle_truncates_long_names() {
    let label = file_reference_chip_label(
        "2026-07-07-skim-slash-fuzzy-completion-design.md",
        FileReferenceKind::File,
        32,
    );

    assert_eq!(label, "@[2026-07-07-skim-s…design.md]");
}

#[test]
fn file_reference_chip_truncates_long_extension_within_width() {
    let max_width = 8;
    let label =
        file_reference_chip_label("a.verylongextension", FileReferenceKind::File, max_width);
    let inner_label = label
        .strip_prefix("@[")
        .and_then(|label| label.strip_suffix(']'))
        .expect("chip label wraps inner label");

    assert!(crate::primitive::visible_width(inner_label) <= max_width);
}

#[test]
fn directory_reference_chip_keeps_trailing_slash() {
    let label = file_reference_chip_label("specs", FileReferenceKind::Directory, 32);

    assert_eq!(label, "@[specs/]");
}

#[test]
fn file_reference_store_allocates_parseable_markers() {
    let mut store = FileReferenceStore::new();
    let id = store.add(
        "workspace".to_owned(),
        std::path::PathBuf::from("crates/neo-tui/src/paste.rs"),
        FileReferenceKind::File,
        "paste.rs".to_owned(),
    );

    let reference = store.get(id).expect("stored reference");
    assert_eq!(
        reference.relative_path,
        std::path::PathBuf::from("crates/neo-tui/src/paste.rs")
    );
    assert_eq!(reference.as_marker().as_placeholder(), "[file #1 paste.rs]");
}

#[test]
fn file_reference_store_default_starts_at_one() {
    let mut store = FileReferenceStore::default();
    let id = store.add(
        "workspace".to_owned(),
        std::path::PathBuf::from("crates/neo-tui/src/paste.rs"),
        FileReferenceKind::File,
        "paste.rs".to_owned(),
    );

    assert_eq!(id, 1);
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
