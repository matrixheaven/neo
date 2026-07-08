# At File Reference Composer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build inline `@` file and directory references in the Neo composer, rendered as atomic abbreviated chips and expanded into prompt context at submit time.

**Architecture:** Extend the existing composer marker path instead of adding a second text model. `neo-tui` owns marker parsing, chip labels, prompt rendering, and two-step atom deletion; `neo-agent` owns file discovery, reference registration, submit-time expansion, and removal of the old inline `@model` override path.

**Tech Stack:** Rust 2024, `skim::fuzzy_matcher::SkimMatcherV2`, `ignore` crate for workspace-aware discovery, existing `PromptState`, `PromptCompletionState`, `ImageAttachmentStore`, and `expand_prompt_markers` flow.

---

## Guardrails

- Do not use `git reset`, `git checkout --`, `git restore`, `git stash`, `git clean`, `git rebase`, or force-push commands.
- Do not run `git add` or `git commit` unless the user explicitly authorizes committing in the implementation session.
- Use exact, narrow Rust test commands. Do not use broad `cargo test` or package-wide `cargo nextest run` as completion evidence.
- Keep `@` canonical: after this plan, `@` means file/directory reference only. Delete the old inline model-override behavior instead of preserving a compatibility branch.

## File Structure

- Modify `crates/neo-tui/src/paste.rs`: add file-reference marker parsing, chip label formatting, and `FileReferenceStore`.
- Modify `crates/neo-tui/src/transcript/chrome_render.rs`: render file markers as `@[label]` chips while keeping raw marker text in `PromptState`.
- Modify `crates/neo-tui/src/shell/prompt.rs`: rely on the expanded marker regex for two-step deletion; add file/image/paste atom tests near the existing prompt tests.
- Modify `crates/neo-tui/src/shell/dialog_factory.rs`: expose prompt-completion prefix plus selected item and allow replacement with a caller-provided marker.
- Modify `crates/neo-agent/Cargo.toml`: add `ignore.workspace = true`.
- Modify `crates/neo-agent/src/modes/interactive/prompt_completion.rs`: replace `@` provider-model completion with fuzzy file-reference candidates.
- Modify `crates/neo-agent/src/modes/interactive/prompt_edit.rs`: sync `@` inline completion, confirm file references into markers, and avoid common-prefix path insertion for `@`.
- Modify `crates/neo-agent/src/modes/interactive/mod.rs`: add a `FileReferenceStore`, pass it into prompt expansion, and remove inline `@model` parsing from `PromptSubmission`.
- Modify `crates/neo-agent/src/prompt/parts.rs`: expand file and directory reference markers into bounded textual context.
- Modify `crates/neo-agent/src/modes/interactive/tests.rs`: replace provider-model `@` tests with file-reference tests.

## Task 1: Add File Reference Markers And Store

**Files:**
- Modify: `crates/neo-tui/src/paste.rs`

- [ ] **Step 1: Add failing marker and label tests**

Add these tests inside `#[cfg(test)] mod tests` in `crates/neo-tui/src/paste.rs`:

```rust
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
fn file_reference_chip_middle_truncates_long_names() {
    let label = file_reference_chip_label(
        "2026-07-07-skim-slash-fuzzy-completion-design.md",
        FileReferenceKind::File,
        32,
    );

    assert_eq!(label, "@[2026-07-07-skim-s…design.md]");
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
    assert_eq!(reference.relative_path, std::path::PathBuf::from("crates/neo-tui/src/paste.rs"));
    assert_eq!(reference.as_marker().as_placeholder(), "[file #1 paste.rs]");
}
```

- [ ] **Step 2: Run the first failing test**

Run:

```bash
cargo test --package neo-tui --lib paste::tests::parses_file_reference_marker -- --exact --nocapture
```

Expected: FAIL with missing `Marker::File` or missing file-reference parsing.

- [ ] **Step 3: Implement marker parsing, chip formatting, and store**

In `crates/neo-tui/src/paste.rs`, replace the marker regex and marker enum with this shape, keeping the existing paste/image behavior:

```rust
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    Paste { id: usize, lines: Option<usize> },
    Image { id: usize, width: u32, height: u32 },
    File { id: usize, display_name: String },
}

impl Marker {
    #[must_use]
    pub fn id(&self) -> usize {
        match self {
            Self::Paste { id, .. } | Self::Image { id, .. } | Self::File { id, .. } => *id,
        }
    }

    #[must_use]
    pub fn as_placeholder(&self) -> String {
        match self {
            Self::Paste { id, lines: Some(n), .. } => format!("[paste #{id} +{n} lines]"),
            Self::Paste { id, lines: None, .. } => format!("[paste #{id} chars]"),
            Self::Image { id, width, height } => format!("[image #{id} ({width}x{height})]"),
            Self::File { id, display_name } => format!("[file #{id} {display_name}]"),
        }
    }

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

#[derive(Debug, Default, Clone)]
pub struct FileReferenceStore {
    next_id: usize,
    by_id: std::collections::BTreeMap<usize, FileReference>,
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
```

Update `parse_markers` so the file branch uses groups 8, 9, and 10 from the regex:

```rust
        } else if cap.get(8).is_some() {
            let id = cap[9].parse().unwrap_or(0);
            let display_name = cap[10].to_owned();
            out.push((m.start(), Marker::File { id, display_name }));
        }
```

Add these label helpers below `parse_markers`:

```rust
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
    let suffix = if extension.is_empty() { "" } else { extension.as_str() };
    let suffix_width = crate::primitive::visible_width(suffix);
    let head_width = max_width.saturating_sub(suffix_width).saturating_sub(1).max(1);
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
```

- [ ] **Step 4: Run the marker test set**

Run:

```bash
cargo test --package neo-tui --lib paste::tests::parses_file_reference_marker -- --exact --nocapture
cargo test --package neo-tui --lib paste::tests::file_reference_placeholder_roundtrips_display_name -- --exact --nocapture
cargo test --package neo-tui --lib paste::tests::file_reference_chip_middle_truncates_long_names -- --exact --nocapture
cargo test --package neo-tui --lib paste::tests::directory_reference_chip_keeps_trailing_slash -- --exact --nocapture
cargo test --package neo-tui --lib paste::tests::file_reference_store_allocates_parseable_markers -- --exact --nocapture
```

Expected: all five tests PASS.

## Task 2: Render File Markers As Chips And Keep Two-Step Atom Deletion

**Files:**
- Modify: `crates/neo-tui/src/transcript/chrome_render.rs`
- Modify: `crates/neo-tui/src/shell/mod.rs`

- [ ] **Step 1: Add failing render and deletion tests**

In `crates/neo-tui/src/transcript/chrome_render.rs`, add this test to the existing test module:

```rust
#[test]
fn prompt_renders_file_marker_as_reference_chip() {
    let mut app = NeoChromeState::new("neo", "session", "model", "/tmp");
    app.prompt_mut()
        .set_text("read [file #1 prompt_completion.rs] now");

    let (lines, _) = render_prompt_lines(&app, 80);
    let visible = lines
        .into_iter()
        .map(|line| crate::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(visible.contains("@[prompt_completion.rs]"), "{visible}");
    assert!(!visible.contains("[file #1"), "{visible}");
}
```

In `crates/neo-tui/src/shell/mod.rs`, add these tests near the existing prompt marker deletion tests:

```rust
#[test]
fn backspace_selects_image_marker_first_then_deletes() {
    let mut prompt = PromptState::new("look [image #1 (640x480)]");
    prompt.cursor = prompt.char_len();

    assert!(prompt.apply_edit(PromptEdit::Backspace).is_none());
    assert_eq!(prompt.text, "look [image #1 (640x480)]");
    assert!(prompt.selected_marker().is_some());

    assert_eq!(
        prompt.apply_edit(PromptEdit::Backspace).as_deref(),
        Some("[image #1 (640x480)]")
    );
    assert_eq!(prompt.text, "look ");
}

#[test]
fn backspace_selects_file_marker_first_then_deletes() {
    let mut prompt = PromptState::new("read [file #1 prompt_completion.rs]");
    prompt.cursor = prompt.char_len();

    assert!(prompt.apply_edit(PromptEdit::Backspace).is_none());
    assert_eq!(prompt.text, "read [file #1 prompt_completion.rs]");
    assert!(prompt.selected_marker().is_some());

    assert_eq!(
        prompt.apply_edit(PromptEdit::Backspace).as_deref(),
        Some("[file #1 prompt_completion.rs]")
    );
    assert_eq!(prompt.text, "read ");
}
```

- [ ] **Step 2: Run the render test to verify it fails**

Run:

```bash
cargo test --package neo-tui --lib transcript::chrome_render::tests::prompt_renders_file_marker_as_reference_chip -- --exact --nocapture
```

Expected: FAIL because the raw file marker is still rendered.

- [ ] **Step 3: Add prompt atom rendering helper**

In `crates/neo-tui/src/transcript/chrome_render.rs`, replace the `styled_text` construction inside `build_prompt_logical_lines` with a call to a new helper:

```rust
    let styled_text = prompt_text_with_atom_display(prompt, cursor);
```

Add this helper near `build_prompt_logical_lines`:

```rust
fn prompt_text_with_atom_display(prompt: &PromptState, cursor: usize) -> String {
    let text = &prompt.text;
    let cursor_byte = prompt.byte_index(cursor);
    let selected_marker = prompt.selected_marker();
    let mut rendered = String::new();
    let mut raw_cursor = 0;
    let mut cursor_inserted = false;

    for (start, marker) in crate::paste::parse_markers(text) {
        let raw_marker = marker.as_placeholder();
        let end = start + raw_marker.len();
        if start < raw_cursor || end > text.len() {
            continue;
        }

        append_cursor_if_needed(
            &mut rendered,
            &text[raw_cursor..start],
            raw_cursor,
            cursor_byte,
            &mut cursor_inserted,
        );

        let display = marker.as_chip();
        let display = if selected_marker == Some((start, end)) {
            paint(&display, Style::default().bg(Color::Rgb(60, 60, 60)))
        } else {
            display
        };

        if !cursor_inserted && cursor_byte >= start && cursor_byte <= end {
            rendered.push_str(CURSOR_MARKER);
            cursor_inserted = true;
        }
        rendered.push_str(&display);
        raw_cursor = end;
    }

    append_cursor_if_needed(
        &mut rendered,
        &text[raw_cursor..],
        raw_cursor,
        cursor_byte,
        &mut cursor_inserted,
    );
    if !cursor_inserted {
        rendered.push_str(CURSOR_MARKER);
    }
    rendered
}

fn append_cursor_if_needed(
    rendered: &mut String,
    raw_segment: &str,
    raw_segment_start: usize,
    cursor_byte: usize,
    cursor_inserted: &mut bool,
) {
    if *cursor_inserted || cursor_byte < raw_segment_start {
        rendered.push_str(raw_segment);
        return;
    }
    let raw_segment_end = raw_segment_start + raw_segment.len();
    if cursor_byte > raw_segment_end {
        rendered.push_str(raw_segment);
        return;
    }
    let local = cursor_byte - raw_segment_start;
    rendered.push_str(&raw_segment[..local]);
    rendered.push_str(CURSOR_MARKER);
    rendered.push_str(&raw_segment[local..]);
    *cursor_inserted = true;
}
```

This helper keeps paste/image markers visually unchanged through `Marker::as_chip`, but renders file markers as `@[display]`.

- [ ] **Step 4: Run the render and deletion tests**

Run:

```bash
cargo test --package neo-tui --lib transcript::chrome_render::tests::prompt_renders_file_marker_as_reference_chip -- --exact --nocapture
cargo test --package neo-tui --lib shell::tests::backspace_selects_marker_first_then_deletes -- --exact --nocapture
cargo test --package neo-tui --lib shell::tests::backspace_selects_image_marker_first_then_deletes -- --exact --nocapture
cargo test --package neo-tui --lib shell::tests::backspace_selects_file_marker_first_then_deletes -- --exact --nocapture
```

Expected: all four tests PASS.

## Task 3: Add File Reference Prompt Expansion

**Files:**
- Modify: `crates/neo-agent/src/prompt/parts.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`

- [ ] **Step 1: Add failing expansion tests**

In `crates/neo-agent/src/prompt/parts.rs`, add these tests inside the existing test module:

```rust
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
        Some(
            "review <file path=\"src/main.rs\">\nfn main() {}\n</file>"
        )
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
```

- [ ] **Step 2: Run the first expansion test to verify it fails**

Run:

```bash
cargo test --package neo-agent --bin neo -- prompt::parts::tests::expand_file_reference_marker_to_file_block --exact --nocapture --include-ignored
```

Expected: FAIL because `expand_prompt_markers` does not accept a file reference store yet.

- [ ] **Step 3: Extend `expand_prompt_markers` signature and file expansion**

In `crates/neo-agent/src/prompt/parts.rs`, change the function signature to:

```rust
pub fn expand_prompt_markers(
    text: &str,
    paste_store: &HashMap<usize, String>,
    image_store: &ImageAttachmentStore,
    file_store: &neo_tui::paste::FileReferenceStore,
    workspace_root: &std::path::Path,
) -> Vec<Content> {
```

Inside the marker match, add this file branch:

```rust
            Marker::File { id, .. } => {
                if let Some(reference) = file_store.get(id) {
                    parts.push(Content::Text {
                        text: expand_file_reference(reference, workspace_root).into(),
                    });
                }
            }
```

Add these helpers below `transcript_image_attachments`:

```rust
const MAX_FILE_REFERENCE_BYTES: usize = 64 * 1024;
const MAX_DIRECTORY_REFERENCE_ENTRIES: usize = 80;

fn expand_file_reference(
    reference: &neo_tui::paste::FileReference,
    workspace_root: &std::path::Path,
) -> String {
    let absolute = workspace_root.join(&reference.relative_path);
    let path = display_relative_path(&reference.relative_path);
    if !absolute.exists() {
        return format!("<file path=\"{path}\" error=\"not found\" />");
    }
    match reference.kind {
        neo_tui::paste::FileReferenceKind::File => expand_file_reference_file(&absolute, &path),
        neo_tui::paste::FileReferenceKind::Directory => {
            expand_file_reference_directory(&absolute, &path)
        }
    }
}

fn expand_file_reference_file(absolute: &std::path::Path, path: &str) -> String {
    let Ok(metadata) = std::fs::metadata(absolute) else {
        return format!("<file path=\"{path}\" error=\"unreadable\" />");
    };
    if !metadata.is_file() {
        return format!("<file path=\"{path}\" error=\"not a file\" />");
    }
    let Ok(bytes) = std::fs::read(absolute) else {
        return format!("<file path=\"{path}\" error=\"unreadable\" />");
    };
    if bytes.iter().take(1024).any(|byte| *byte == 0) {
        return format!(
            "<file path=\"{path}\" type=\"binary\" bytes=\"{}\" />",
            bytes.len()
        );
    }
    let truncated = bytes.len() > MAX_FILE_REFERENCE_BYTES;
    let slice = if truncated {
        &bytes[..MAX_FILE_REFERENCE_BYTES]
    } else {
        &bytes
    };
    let Ok(mut body) = String::from_utf8(slice.to_vec()) else {
        return format!(
            "<file path=\"{path}\" type=\"non-utf8\" bytes=\"{}\" />",
            bytes.len()
        );
    };
    if truncated {
        body.push_str("\n[truncated]\n");
    }
    format!("<file path=\"{path}\">\n{body}</file>")
}

fn expand_file_reference_directory(absolute: &std::path::Path, path: &str) -> String {
    let Ok(entries) = std::fs::read_dir(absolute) else {
        return format!("<directory path=\"{path}\" error=\"unreadable\" />");
    };
    let mut names = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_name == ".git" {
                return None;
            }
            let suffix = entry.file_type().ok().filter(|kind| kind.is_dir()).map(|_| "/").unwrap_or("");
            Some(format!("{file_name}{suffix}"))
        })
        .collect::<Vec<_>>();
    names.sort();
    let truncated = names.len() > MAX_DIRECTORY_REFERENCE_ENTRIES;
    names.truncate(MAX_DIRECTORY_REFERENCE_ENTRIES);
    if truncated {
        names.push("[truncated]".to_owned());
    }
    format!("<directory path=\"{path}\">\n{}\n</directory>", names.join("\n"))
}

fn display_relative_path(path: &std::path::Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
```

Update the existing image and paste tests in this file to pass `&FileReferenceStore::new()` and a temp/workspace path.

- [ ] **Step 4: Pass the store from interactive submit**

In `crates/neo-agent/src/modes/interactive/mod.rs`, add a controller field next to `image_attachment_store`:

```rust
    /// Stored file references inserted by composer `[file #N display-name]` placeholders.
    file_reference_store: neo_tui::paste::FileReferenceStore,
```

Initialize it wherever `image_attachment_store` is initialized:

```rust
file_reference_store: neo_tui::paste::FileReferenceStore::new(),
```

Update `start_turn_from_submitted_prompt`:

```rust
        let content = crate::prompt::parts::expand_prompt_markers(
            &prompt,
            &self.paste_store,
            &self.image_attachment_store,
            &self.file_reference_store,
            &self.completion_root,
        );
```

Update any other `expand_prompt_markers` call sites with an empty `FileReferenceStore` and a workspace root.

- [ ] **Step 5: Run the expansion tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- prompt::parts::tests::expand_file_reference_marker_to_file_block --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- prompt::parts::tests::expand_directory_reference_marker_to_bounded_directory_block --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- prompt::parts::tests::expand_missing_file_reference_marker_to_error_block --exact --nocapture --include-ignored
```

Expected: all three tests PASS.

## Task 4: Replace `@model` Completion With Fuzzy File Reference Candidates

**Files:**
- Modify: `crates/neo-agent/Cargo.toml`
- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add the `ignore` dependency**

In `crates/neo-agent/Cargo.toml`, add:

```toml
ignore.workspace = true
```

- [ ] **Step 2: Add failing completion tests**

In `crates/neo-agent/src/modes/interactive/tests.rs`, replace the old provider-model `@` completion assertions with these exact tests:

```rust
#[test]
fn at_file_reference_completion_fuzzy_ranks_basename_matches() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("crates/neo-agent/src/modes/interactive");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("prompt_completion.rs"), "").expect("write prompt completion");
    fs::write(src.join("prompt_templates.rs"), "").expect("write prompt templates");

    let catalog = CompletionCatalog::default();
    let candidates =
        completion_source_candidates(temp.path(), "@prom", &catalog).expect("file references");

    assert_eq!(candidates[0].value, "@crates/neo-agent/src/modes/interactive/prompt_completion.rs");
    assert_eq!(candidates[0].label, "prompt_completion.rs");
    assert_eq!(candidates[0].description.as_deref(), Some("crates/neo-agent/src/modes/interactive/"));
    assert_eq!(candidates[0].source, CompletionSource::FileReference);
}

#[test]
fn at_file_reference_completion_hides_dotfiles_until_dot_query() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join(".env"), "secret").expect("write env");
    fs::write(temp.path().join("Cargo.toml"), "").expect("write cargo");

    let catalog = CompletionCatalog::default();
    let hidden =
        completion_source_candidates(temp.path(), "@e", &catalog).expect("hidden query");
    assert!(hidden.iter().all(|candidate| candidate.label != ".env"));

    let visible =
        completion_source_candidates(temp.path(), "@.e", &catalog).expect("dot query");
    assert!(visible.iter().any(|candidate| candidate.label == ".env"));
}

#[test]
fn at_file_reference_completion_no_longer_returns_provider_models() {
    let temp = tempfile::tempdir().expect("tempdir");
    let catalog = CompletionCatalog {
        model_items: vec![PickerItem::new(
            "anthropic/claude-sonnet",
            "anthropic/claude-sonnet",
            Some("Messages"),
        )],
        ..CompletionCatalog::default()
    };

    let candidates =
        completion_source_candidates(temp.path(), "@anth", &catalog).expect("file references");

    assert!(candidates.iter().all(|candidate| candidate.value != "@anthropic/claude-sonnet"));
    assert!(candidates.iter().all(|candidate| candidate.source == CompletionSource::FileReference));
}
```

- [ ] **Step 3: Run the first completion test to verify it fails**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::at_file_reference_completion_fuzzy_ranks_basename_matches --exact --nocapture --include-ignored
```

Expected: FAIL because `@` still routes to provider-model completion or prefix-only filesystem completion.

- [ ] **Step 4: Implement file-reference completion scoring**

In `crates/neo-agent/src/modes/interactive/prompt_completion.rs`:

1. Add `use ignore::WalkBuilder;`.
2. Remove `model_completion_candidates`.
3. Change `CompletionSource` to include `FileReference` and remove `ProviderModel` after Task 6 updates tests:

```rust
pub(super) enum CompletionSource {
    LocalFile,
    FileReference,
    SlashPrompt,
    PromptPackage,
    SessionCommand,
}
```

4. Change the `@` branch in `completion_source_candidates`:

```rust
    let mut candidates = if prefix.starts_with('@') {
        file_reference_completion_candidates(root, prefix)?
    } else {
        filesystem_completion_candidates(root, prefix)?
    };
```

5. Add these types and functions near the existing filesystem completion code:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileReferenceCandidate {
    value: String,
    label: String,
    parent: String,
    is_dir: bool,
    score: FileReferenceScore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FileReferenceTier {
    ExactBasename,
    BasenamePrefix,
    SegmentPrefix,
    Fuzzy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileReferenceScore {
    tier: FileReferenceTier,
    skim_score: i64,
    path_len: usize,
}

fn file_reference_completion_candidates(
    root: &Path,
    prefix: &str,
) -> Result<Vec<CompletionCandidate>> {
    let query = prefix.strip_prefix('@').unwrap_or(prefix).trim();
    let matcher = SkimMatcherV2::default().smart_case();
    let mut scored = Vec::new();

    for result in WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .max_depth(Some(6))
        .build()
    {
        let Ok(entry) = result else {
            continue;
        };
        let path = entry.path();
        if path == root {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == ".git" || name == ".neo" {
            continue;
        }
        if !query.starts_with('.') && name.starts_with('.') {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative_text = display_completion_path(relative);
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
        let Some(score) = score_file_reference(&matcher, query, name, &relative_text) else {
            continue;
        };
        let parent = relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(display_completion_path)
            .map(|mut parent| {
                parent.push('/');
                parent
            })
            .unwrap_or_default();
        scored.push(FileReferenceCandidate {
            value: format!("@{relative_text}"),
            label: if is_dir { format!("{name}/") } else { name.to_owned() },
            parent,
            is_dir,
            score,
        });
    }

    scored.sort_by(|left, right| {
        left.score
            .tier
            .cmp(&right.score.tier)
            .then_with(|| right.score.skim_score.cmp(&left.score.skim_score))
            .then_with(|| left.is_dir.cmp(&right.is_dir))
            .then_with(|| left.score.path_len.cmp(&right.score.path_len))
            .then_with(|| left.value.cmp(&right.value))
    });
    scored.truncate(100);

    Ok(scored
        .into_iter()
        .map(|candidate| {
            CompletionCandidate::new(
                candidate.value,
                candidate.label,
                (!candidate.parent.is_empty()).then_some(candidate.parent),
                CompletionSource::FileReference,
            )
        })
        .collect())
}

fn score_file_reference(
    matcher: &SkimMatcherV2,
    query: &str,
    basename: &str,
    relative_path: &str,
) -> Option<FileReferenceScore> {
    let path_len = relative_path.chars().count();
    if query.is_empty() {
        return Some(FileReferenceScore {
            tier: FileReferenceTier::Fuzzy,
            skim_score: 0,
            path_len,
        });
    }
    let basename_without_extension = basename.rsplit_once('.').map_or(basename, |(stem, _)| stem);
    let tier_and_score = if basename == query || basename_without_extension == query {
        Some((FileReferenceTier::ExactBasename, 0))
    } else if basename.starts_with(query) || basename_without_extension.starts_with(query) {
        Some((FileReferenceTier::BasenamePrefix, 0))
    } else if relative_path
        .split(['/', '-', '_', '.'])
        .any(|segment| segment.starts_with(query))
    {
        Some((FileReferenceTier::SegmentPrefix, 0))
    } else {
        let keys = [
            basename.to_owned(),
            basename_without_extension.to_owned(),
            relative_path.to_owned(),
            relative_path.replace(['/', '-', '_', '.'], " "),
        ];
        keys.iter()
            .filter_map(|key| matcher.fuzzy_match(key, query))
            .max()
            .map(|score| (FileReferenceTier::Fuzzy, score))
    }?;
    Some(FileReferenceScore {
        tier: tier_and_score.0,
        skim_score: tier_and_score.1,
        path_len,
    })
}

fn display_completion_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
```

- [ ] **Step 5: Run completion tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::at_file_reference_completion_fuzzy_ranks_basename_matches --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::at_file_reference_completion_hides_dotfiles_until_dot_query --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::at_file_reference_completion_no_longer_returns_provider_models --exact --nocapture --include-ignored
```

Expected: all three tests PASS.

## Task 5: Confirm `@` Completion Into File Reference Markers

**Files:**
- Modify: `crates/neo-tui/src/shell/dialog_factory.rs`
- Modify: `crates/neo-agent/src/modes/interactive/prompt_edit.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing event-loop test**

In `crates/neo-agent/src/modes/interactive/tests.rs`, replace `event_loop_tab_completes_provider_model_prefix` with:

```rust
#[tokio::test]
async fn event_loop_tab_inserts_file_reference_chip_marker() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write file");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs::default(),
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@main");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab inserts file reference");

    assert_eq!(controller.chrome().prompt().text, "[file #1 main.rs]");
    assert!(controller.chrome().focused_overlay().is_none());
}
```

- [ ] **Step 2: Run the failing event-loop test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_tab_inserts_file_reference_chip_marker --exact --nocapture --include-ignored
```

Expected: FAIL because selected `@` completions are still inserted as path text.

- [ ] **Step 3: Add replacement-capable prompt completion methods**

In `crates/neo-tui/src/shell/dialog_factory.rs`, add:

```rust
    #[must_use]
    pub fn selected_prompt_completion_with_prefix(&self) -> Option<(PromptCompletionPrefix, PickerItem)> {
        let OverlayKind::PromptCompletion(completions) = &self.focused_overlay()?.kind else {
            return None;
        };
        Some((completions.prefix().clone(), completions.selected_item()?))
    }

    pub fn confirm_prompt_completion_with_replacement(
        &mut self,
        replacement: &str,
    ) -> Option<PickerItem> {
        let id = self.focused_overlay;
        let (prefix, item) = {
            let OverlayKind::PromptCompletion(completions) = &self.focused_overlay()?.kind else {
                return None;
            };
            (completions.prefix().clone(), completions.confirm()?)
        };
        self.prompt.replace_completion_prefix(&prefix, replacement)?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(item)
    }
```

Keep the existing `confirm_prompt_completion` by implementing it through the new method:

```rust
    pub fn confirm_prompt_completion(&mut self) -> Option<PickerItem> {
        let item = self.selected_prompt_completion()?;
        self.confirm_prompt_completion_with_replacement(&item.value)
    }
```

- [ ] **Step 4: Add controller helper for file reference replacement**

In `crates/neo-agent/src/modes/interactive/prompt_edit.rs`, add this helper inside `impl InteractiveController`:

```rust
    fn confirm_prompt_completion_or_file_reference(&mut self) -> bool {
        let Some((prefix, item)) = self.tui.chrome().selected_prompt_completion_with_prefix() else {
            return false;
        };
        if prefix.text.starts_with('@') {
            let Some(marker) = self.file_reference_marker_for_completion(&item) else {
                return false;
            };
            let _ = self
                .tui
                .chrome_mut()
                .confirm_prompt_completion_with_replacement(&marker);
            return true;
        }
        let _ = self.tui.chrome_mut().confirm_prompt_completion();
        true
    }

    fn file_reference_marker_for_completion(&mut self, item: &PickerItem) -> Option<String> {
        let relative = item.value.strip_prefix('@')?;
        let relative_path = std::path::PathBuf::from(relative);
        let absolute = self.completion_root.join(&relative_path);
        let metadata = std::fs::metadata(&absolute).ok()?;
        let kind = if metadata.is_dir() {
            neo_tui::paste::FileReferenceKind::Directory
        } else {
            neo_tui::paste::FileReferenceKind::File
        };
        let display_name = item.label.trim_end_matches('/').to_owned();
        let display_name = match kind {
            neo_tui::paste::FileReferenceKind::File => display_name,
            neo_tui::paste::FileReferenceKind::Directory => format!("{display_name}/"),
        };
        let display_name = neo_tui::paste::file_reference_chip_label(&display_name, kind, 32)
            .trim_start_matches("@[")
            .trim_end_matches(']')
            .to_owned();
        let id = self.file_reference_store.add(
            "workspace".to_owned(),
            relative_path,
            kind,
            display_name,
        );
        self.file_reference_store
            .get(id)
            .map(|reference| reference.as_marker().as_placeholder())
    }
```

Replace both existing direct calls to `confirm_prompt_completion()` in `prompt_edit.rs` and `input.rs` prompt-completion handling with `confirm_prompt_completion_or_file_reference()`.

In `complete_prompt_or_insert_tab`, change the selected-completion branch:

```rust
        if self.confirm_prompt_completion_or_file_reference() {
            return;
        }
```

For `@` completions, skip `longest_common_completion_prefix`. Use this branch before the common-prefix branch:

```rust
        if prefix.text.starts_with('@') {
            if completions.len() == 1 {
                if let Some(marker) = self.file_reference_marker_for_completion(&completions[0]) {
                    let _ = self
                        .tui
                        .chrome_mut()
                        .prompt_mut()
                        .replace_completion_prefix(&prefix, &marker);
                }
                return;
            }
            self.tui
                .chrome_mut()
                .open_prompt_completion_picker(prefix, completions);
            return;
        }
```

In `sync_inline_prompt_completion`, allow `/` and `@`:

```rust
        if !prefix.text.starts_with('/') && !prefix.text.starts_with('@') {
            self.close_inline_prompt_completion();
            return;
        }
```

- [ ] **Step 5: Run the event-loop test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_tab_inserts_file_reference_chip_marker --exact --nocapture --include-ignored
```

Expected: PASS.

## Task 6: Remove Inline `@model` Override Behavior

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing no-model-override submit test**

In `crates/neo-agent/src/modes/interactive/tests.rs`, replace `event_loop_inline_provider_model_prefix_overrides_submitted_turn` with:

```rust
#[tokio::test]
async fn event_loop_at_model_token_submits_as_plain_text() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests.lock().expect("record request").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("Messages"),
            )],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@anthropic/claude-sonnet explain this file");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("@anthropic/claude-sonnet explain this file")]
    );
    assert_eq!(requests[0].model, None);
}
```

- [ ] **Step 2: Run the failing no-model-override test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_at_model_token_submits_as_plain_text --exact --nocapture --include-ignored
```

Expected: FAIL because `PromptSubmission::from_text` still strips a leading provider model token.

- [ ] **Step 3: Delete inline model parsing from `PromptSubmission::from_text`**

In `crates/neo-agent/src/modes/interactive/mod.rs`, replace `PromptSubmission::from_text` with:

```rust
impl PromptSubmission {
    fn from_text(
        prompt: String,
        _model_items: &[PickerItem],
        config: Option<&AppConfig>,
        fallback_project_dir: &Path,
    ) -> Result<Self> {
        Ok(Self {
            prompt: expand_interactive_prompt(&prompt, config, fallback_project_dir)?,
            model_override: None,
        })
    }
}
```

Keep the `_model_items` parameter for now so callers stay small; remove it in a later cleanup only if no clippy warning appears.

Update the comment above prompt history in `start_turn_from_submitted_prompt` from:

```rust
// Persist the resolved user prompt (after @model/prompt-template
// expansion) to the workspace history.
```

to:

```rust
// Persist the resolved user prompt after prompt-template and attachment
// expansion to the workspace history.
```

- [ ] **Step 4: Run the no-model-override tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_at_model_token_submits_as_plain_text --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_inline_model_token_without_prompt_does_not_override_model --exact --nocapture --include-ignored
```

Expected: both tests PASS. If the second test was renamed or removed during replacement, run only the new exact test and note the deletion in the implementation summary.

## Task 7: Submit Referenced File Content In A Turn

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing end-to-end submit test**

Add this test in `crates/neo-agent/src/modes/interactive/tests.rs`:

```rust
#[tokio::test]
async fn event_loop_submits_file_reference_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write file");

    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests.lock().expect("record request").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs::default(),
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("review @main");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("insert file reference");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("submit turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("recorded requests");
    let text = requests[0].prompt[0].as_text().expect("text content");
    assert!(text.contains("review <file path=\"src/main.rs\">"), "{text}");
    assert!(text.contains("fn main() {}"), "{text}");
    assert!(text.contains("</file>"), "{text}");
}
```

- [ ] **Step 2: Run the end-to-end submit test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_submits_file_reference_content --exact --nocapture --include-ignored
```

Expected: PASS after Tasks 3 through 6. If it fails, inspect whether the file reference store is initialized in the test controller constructor and whether `completion_root` is the temp path.

## Task 8: Narrow Final Verification

**Files:**
- No new files.

- [ ] **Step 1: Run targeted neo-tui verification**

Run:

```bash
cargo test --package neo-tui --lib paste::tests::parses_file_reference_marker -- --exact --nocapture
cargo test --package neo-tui --lib paste::tests::file_reference_chip_middle_truncates_long_names -- --exact --nocapture
cargo test --package neo-tui --lib shell::tests::backspace_selects_image_marker_first_then_deletes -- --exact --nocapture
cargo test --package neo-tui --lib shell::tests::backspace_selects_file_marker_first_then_deletes -- --exact --nocapture
cargo test --package neo-tui --lib transcript::chrome_render::tests::prompt_renders_file_marker_as_reference_chip -- --exact --nocapture
```

Expected: all listed tests PASS.

- [ ] **Step 2: Run targeted neo-agent verification**

Run:

```bash
cargo test --package neo-agent --bin neo -- prompt::parts::tests::expand_file_reference_marker_to_file_block --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- prompt::parts::tests::expand_directory_reference_marker_to_bounded_directory_block --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::at_file_reference_completion_fuzzy_ranks_basename_matches --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_tab_inserts_file_reference_chip_marker --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_at_model_token_submits_as_plain_text --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_submits_file_reference_content --exact --nocapture --include-ignored
```

Expected: all listed tests PASS.

- [ ] **Step 3: Run formatting and diff checks**

Run:

```bash
cargo fmt --all --check
git diff --check -- crates/neo-tui/src/paste.rs crates/neo-tui/src/transcript/chrome_render.rs crates/neo-tui/src/shell/prompt.rs crates/neo-tui/src/shell/mod.rs crates/neo-tui/src/shell/dialog_factory.rs crates/neo-agent/Cargo.toml crates/neo-agent/src/modes/interactive/prompt_completion.rs crates/neo-agent/src/modes/interactive/prompt_edit.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/prompt/parts.rs crates/neo-agent/src/modes/interactive/tests.rs
```

Expected: both commands exit 0.

- [ ] **Step 4: Store ICM completion memory**

Run:

```bash
icm store -t context-neo -c "Implemented inline @ file/directory references in Neo's composer. @ now opens fuzzy file reference completion, selected entries insert atomic file markers rendered as @[name] chips, paste/image/file markers share two-step deletion, and submit-time expansion reads referenced file content or bounded directory listings." -i high -k "at-file-reference,composer,atomic-chip,file-expansion,backspace"
```

Expected: command prints a line starting with `Stored:`.

## Self-Review

- Spec coverage: The plan covers canonical `@` file references, fuzzy candidate UI, abbreviated chips, hidden path storage, prompt expansion, two-step atom deletion for paste/image/file, and old `@model` removal.
- Placeholder scan: No placeholder tokens, deferred implementation notes, or unnamed tests are required to execute this plan.
- Type consistency: The same `FileReferenceStore`, `FileReferenceKind`, `Marker::File`, `file_reference_chip_label`, and `confirm_prompt_completion_or_file_reference` names are used across tasks.
- Scope check: Full-screen picker, recursive directory ingestion, remote resources, and provider-model `@` compatibility are not included.
