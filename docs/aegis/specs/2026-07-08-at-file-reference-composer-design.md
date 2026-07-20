# At File Reference Composer Design

## Goal

Add inline `@` file and directory references to Neo's composer. Typing an `@` token opens a fuzzy candidate list. Selecting a candidate inserts an atomic, abbreviated reference chip instead of a full path. Backspace uses the same two-step delete interaction for file references, large paste markers, and image markers.

## Background

Neo already has a prompt completion path in `crates/neo-agent/src/modes/interactive/prompt_completion.rs`. It routes `/` to slash commands, regular path prefixes to filesystem completion, and currently routes `@` to provider model completion. Slash completion already uses `skim` scoring, which is the right matching engine for picker-like inline completion.

Neo also has collapsed composer markers in `crates/neo-tui/src/paste.rs` and prompt expansion in `crates/neo-agent/src/prompt/parts.rs`. Large text pastes are represented as `[paste #N ...]`, and images are represented as `[image #N (WxH)]`. The desired file reference behavior should follow this existing "small visible marker, larger hidden payload" model, but it must also fix the missing image deletion behavior by making all composer attachments atomic.

## Scope

In scope:

- Make `@` the canonical inline file and directory reference trigger.
- Replace the current `@` provider-model completion branch instead of keeping both meanings.
- Show a fuzzy file/directory candidate list above the composer.
- Insert an abbreviated atomic reference chip on selection.
- Store the selected file or directory path separately from the visible chip text.
- Expand file references into prompt content at submit time.
- Expand directory references into a bounded directory tree summary at submit time.
- Add a shared two-step Backspace/Delete interaction for paste, image, and file reference markers.
- Add focused tests for matching, chip formatting, atomic deletion, and prompt expansion.

Out of scope:

- Full-screen file picker UI.
- Recursive directory content ingestion.
- Editing the path inside a reference chip after insertion.
- Provider model completion under `@`; model switching remains handled by `/model` and the model selector.
- Remote files, URLs, or MCP resources as `@` references.
- Syntax highlighting or matched-character highlighting inside the candidate list.

## UX Overview

The interaction is inline and non-modal. When the cursor is in a composer token that starts with `@`, Neo opens a candidate list directly above the composer. The typed text after `@` is the fuzzy query. `Enter` inserts the selected file or directory reference. `Esc` closes the list and leaves the literal text in place.

```text
╭─ @ reference ─────────────────────────────────────────────╮
│ @prom                                                     │
│ › F  prompt_completion.rs        crates/neo-agent/src/... │
│   F  prompt_templates.rs         crates/neo-agent/src/... │
│   D  prompts/                    .neo/                    │
│   F  skim-slash-fuzzy...md       docs/aegis/specs/  │
╰─ ↑↓ select · Enter insert · Esc close ────────────────────╯
> compare @prom▏
```

After selection, the composer shows a compact chip. The chip is not plain path text, even though the underlying reference keeps the workspace-relative path.

```text
> compare @[prompt_completion.rs] with @[2026-07-07-skim…design.md]▏
```

If two candidates share the same basename, the composer still uses the basename chip. The selected state and candidate list expose the path for disambiguation.

```text
> inspect @[tests.rs]▏
           crates/neo-agent/src/modes/interactive/tests.rs
```

## Activation Rules

An `@` reference query starts when `@` is the first character of the current token. It may appear at the beginning of the prompt or after whitespace/punctuation.

Examples that open the candidate list:

- `@`
- `compare @prompt`
- `read(@tests`
- `foo, @docs`

Examples that do not open the list:

- `email@example.com`
- `abc@def`
- `@` inside an existing atomic chip

When the query is empty, Neo shows the highest-signal recent/root candidates without fuzzy scoring. Once the user types after `@`, fuzzy scoring applies.

## Candidate Sources

The first version searches local workspace files and directories only:

- The primary workspace root.
- Additional trusted workspace roots already known to Neo, if available in the interactive runtime.

Candidate discovery must use `Path` and `PathBuf` throughout. Paths displayed to the user are workspace-relative. Absolute paths are never shown in the composer and should only appear in diagnostics when needed.

Discovery should respect normal project boundaries:

- Do not leave the configured workspace roots.
- Skip `.git` and Neo session/internal state directories.
- Respect gitignore-style ignore rules when the existing dependency stack makes that practical.
- Hide dotfiles and dot-directories unless the query segment starts with `.`.
- Cap candidate collection before scoring so large repositories do not block input rendering.

The candidate row has a stable shape:

```text
› F  prompt_completion.rs        crates/neo-agent/src/...
  D  specs/                      docs/aegis/
```

`F` means file. `D` means directory. The basename is the primary visual target. The parent path is secondary context.

## Matching And Ranking

Use `SkimMatcherV2::default().smart_case()` for fuzzy scoring, with Neo-local tiers and tie-breakers around it.

Each candidate has search keys:

- Basename, such as `prompt_completion.rs`.
- Extensionless basename, such as `prompt_completion`.
- Workspace-relative path, such as `crates/neo-agent/src/modes/interactive/prompt_completion.rs`.
- Path segments joined with spaces, such as `crates neo-agent interactive prompt completion`.

Ranking:

1. Exact basename match.
2. Basename prefix match.
3. Path segment prefix match.
4. Skim fuzzy match.
5. Files before directories for equal score.
6. Shorter workspace-relative path for equal score.
7. Lexicographic workspace-relative path as the final stable tie-breaker.

The visible list is capped to a small number of rows that fit above the composer, with an internal candidate cap of 100 for the first version.

## Chip Format

The visible chip uses the selected basename, not the path:

```text
@[filename.ext]
@[very-long…name.ext]
@[directory/]
```

Formatting rules:

- Keep the `@[` and `]` wrapper stable.
- Directories end with `/`.
- Preserve the extension when truncating long filenames.
- Use middle ellipsis for long basenames.
- Do not show the internal reference id in the composer.
- Do not show absolute or workspace-relative paths in the composer by default.

Suggested width cap: 32 visible columns for the basename portion before adding `@[` and `]`. The renderer may tighten this cap when the terminal is narrow.

## Internal Representation

Introduce a shared composer attachment model rather than adding another ad hoc marker path.

Conceptual model:

```rust
enum ComposerAtomKind {
    Paste,
    Image,
    FileReference,
}

struct ComposerAtom {
    kind: ComposerAtomKind,
    id: usize,
    raw_marker: String,
    display_label: String,
    delete_hint: String,
}
```

File references should have a store similar to `ImageAttachmentStore`:

```rust
struct FileReference {
    id: usize,
    root_label: String,
    relative_path: PathBuf,
    kind: FileReferenceKind,
    display_name: String,
}

enum FileReferenceKind {
    File,
    Directory,
}
```

The raw composer text may keep a parseable marker such as `[file #N display]`, but rendering should convert it to the visible `@[display]` chip. The prompt expansion layer should resolve the id through `FileReferenceStore`; it must not infer paths from the visible label.

This keeps paste/image/file references consistent:

- Raw text remains serializable and parseable.
- Display is compact and user-friendly.
- Submit-time expansion uses stores keyed by id.
- Deletion can operate on parsed marker spans instead of character-by-character text.

## Backspace And Delete UX

Atomic markers are deleted in two steps. This applies to file references, large paste markers, and image markers.

When the cursor is immediately after an atom, the first Backspace selects the atom:

```text
> summarize ‹@[prompt_completion.rs]›▏
             Backspace again removes · Esc keep
```

The second Backspace removes the whole atom and removes any backing store entry that is no longer referenced.

Rules:

- Backspace at the right boundary selects the atom.
- Delete at the left boundary selects the atom.
- A second Backspace/Delete removes the selected atom.
- `Esc`, `Left`, `Right`, mouse movement, or ordinary typing clears the selection without deleting.
- If the user moves the cursor into the middle of a raw marker through existing editing commands, the renderer and deletion logic should still treat the marker span as atomic.
- Removing an image atom also removes its pending image attachment if no other marker references it.
- Removing a file atom removes only the reference, never the underlying file.

This explicitly fixes the current missing image-paste deletion affordance by making image markers use the same atom selection path.

## Submit-Time Expansion

On submit, Neo expands file reference atoms into textual context blocks. The visible chip text is not sent as-is as the only context.

### Transcript Projection

The submitted user message has two projections owned by one durable message:

- The model projection contains the expanded file or directory snapshot.
- The presentation projection preserves the prompt text with file markers rendered as the same compact `@[display]` chips used by the composer.

The transcript, transcript copy, session replay, and human-readable transcript export must use the presentation projection. They must never replace a selected file chip with the expanded file contents. Queue and steer messages use the same contract.

The durable user message stores the optional presentation text alongside the canonical expanded `content`. Provider conversion, context replay, compaction, and retries continue to consume only `content`; presentation text must not change provider-visible bytes. Older session events without presentation text fall back to their existing content projection and are not heuristically rewritten or migrated.

The presentation text is derived from the resolved markerized prompt before submit-time expansion by reusing the canonical marker parser and `Marker::as_chip()`. Neo must not add a second attachment event, transcript card, or resurrect the removed `PromptSubmission` abstraction for this behavior.

File expansion:

```text
<file path="crates/neo-agent/src/modes/interactive/prompt_completion.rs">
...file contents...
</file>
```

Directory expansion:

```text
<directory path="docs/aegis/specs">
2026-07-07-skim-slash-fuzzy-completion-design.md
2026-07-08-at-file-reference-composer-design.md
</directory>
```

Directory expansion is a bounded tree/list summary only. It does not recursively inline file contents in the first version. This keeps `@directory/` useful as orientation without surprising token explosions.

Expansion guardrails:

- Read only files/directories inside allowed workspace roots.
- If a referenced file is deleted before submit, insert a short missing-file notice instead of failing the entire prompt.
- If a file is too large, include a truncation notice and a bounded prefix rather than blocking submission.
- For invalid UTF-8 or likely-binary files, include a metadata notice instead of lossy binary text.
- Use cross-platform path handling. Display paths with `/` only after converting from `Path` for UI text.

## State Model

Reference completion and atom deletion are separate states.

```text
plain input
  └─ token starts with @
       └─ reference search open
            ├─ printable char -> update query
            ├─ Backspace      -> update query, or close when empty
            ├─ ↑/↓            -> move selection
            ├─ Enter/Tab      -> insert selected atom
            └─ Esc            -> close, keep literal @query
```

```text
normal atom
  └─ Backspace/Delete at boundary -> selected atom

selected atom
  ├─ Backspace/Delete -> remove atom
  ├─ Esc/Left/Right   -> clear selection
  └─ printable char   -> clear selection, then insert normally
```

The candidate list must close when the cursor leaves the active `@` token, when a blocking dialog opens, or when the prompt is submitted.

## Error Handling

Candidate discovery errors should be quiet in the main composer path. If a directory cannot be read, omit it from results. If the root itself cannot be read, show an empty list with a muted one-line status:

```text
╭─ @ reference ─────────────────────────────╮
│ no readable files                         │
╰───────────────────────────────────────────╯
```

Submit-time expansion errors should be represented inside the generated prompt content so the model sees what happened:

```text
<file path="crates/example.rs" error="not found" />
```

Neo should not panic or abort the whole turn because a referenced path disappeared after selection.

## Testing

Focused tests should cover the behavior at the smallest useful boundary.

Prompt completion tests in `crates/neo-agent/src/modes/interactive/tests.rs`:

- `@prom` ranks `prompt_completion.rs` above weaker fuzzy matches.
- Empty `@` query returns capped file/directory candidates.
- Hidden files are excluded unless the query segment starts with `.`.
- `@` no longer returns provider model candidates.

TUI/parser tests in `crates/neo-tui`:

- Long file chips middle-truncate and preserve extensions.
- Directory chips end in `/`.
- Backspace after a file atom selects first and deletes second.
- Backspace after an image atom selects first and deletes second.
- Backspace after a paste atom selects first and deletes second.

Prompt expansion tests in `crates/neo-agent/src/prompt/parts.rs` or a nearby module:

- File reference expands to a `<file path="...">` block.
- Directory reference expands to a bounded `<directory path="...">` block.
- Missing files produce an inline error block.
- Binary or invalid UTF-8 files produce a metadata notice.

Transcript projection tests:

- A submitted file reference renders `@[display]` while the model receives the expanded `<file>` block.
- Queue and steer messages preserve the same compact presentation text.
- Session JSONL replay preserves the compact presentation text while provider conversion still uses expanded content.
- Older user-message events without presentation text continue to deserialize and replay.

Verification should use exact, narrow commands, for example:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::<exact_test_name> --exact --nocapture --include-ignored
cargo test --package neo-tui --lib <exact_test_name> -- --exact --nocapture
```

## Migration Notes

`@` should have one canonical meaning after this change: file/directory reference. The old provider-model `@` completion path should be deleted, not retained as a compatibility branch. Model selection remains available through `/model`.

Existing paste and image markers should migrate to the shared atom rendering/deletion path. The old raw marker syntax can remain as the serialized backing form, but the visible composer interaction should be governed by the atom parser.

## Non-Goals

This feature is not a general attachment framework, a project search UI, or a token-budget planner. It is an inline composer reference mechanism with a tight rendering and deletion contract.

## Self-Review

- Placeholder scan: no placeholders remain.
- Consistency check: `@` has one canonical meaning, matching the no-compatibility project preference.
- Scope check: file/directory references, candidate UI, atom deletion, and submit expansion are in scope; full-screen picking and recursive directory ingestion are out of scope.
- Ambiguity check: directory references expand to bounded tree summaries only, and selected chips never rely on visible labels for path resolution.
