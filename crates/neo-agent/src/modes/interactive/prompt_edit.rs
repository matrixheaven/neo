use std::fmt::Write as _;

use anyhow::Result;

use super::{
    Content, InputEvent, InteractiveController, KeybindingAction, OverlayKind, PromptEdit,
    frame_content_width, longest_common_completion_prefix, prompt_completions, size,
};

pub(super) fn content_to_display_text(content: &[Content]) -> String {
    let mut image_idx = 0;
    let mut out = String::new();
    for part in content {
        match part {
            Content::Text { text } => out.push_str(text),
            Content::Image { mime_type, data } => {
                image_idx += 1;
                let (w, h) = image_dimensions_from_data(mime_type, data);
                let _ = write!(out, "[image #{image_idx} ({w}x{h})]");
            }
            Content::Thinking { .. } => {}
        }
    }
    out
}

pub(super) fn image_dimensions_from_data(
    mime_type: &str,
    data: &neo_agent_core::ImageRef,
) -> (u32, u32) {
    let bytes = match data {
        neo_agent_core::ImageRef::Base64(b64) => {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()
        }
        _ => None,
    };
    bytes
        .as_deref()
        .and_then(|b| crate::image_blob::detect_image_dimensions(b, mime_type))
        .unwrap_or((0, 0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InlineSkillInvocation {
    pub(super) name: String,
    pub(super) args: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InlineSkillDirectives {
    pub(super) invocations: Vec<InlineSkillInvocation>,
    pub(super) body: String,
}

#[cfg(test)]
impl InlineSkillDirectives {
    pub(super) fn names(&self) -> Vec<&str> {
        self.invocations
            .iter()
            .map(|invocation| invocation.name.as_str())
            .collect()
    }
}

pub(super) fn parse_inline_skill_directives(input: &str) -> Option<InlineSkillDirectives> {
    let mut invocations = Vec::new();
    let mut body_lines = Vec::new();

    for line in input.lines() {
        let mut cursor = 0;
        let mut body_line = String::new();
        while let Some(pos) = next_skill_directive(line, cursor) {
            body_line.push_str(&line[cursor..pos]);

            let name_start = pos + "/skill:".len();
            let name_end = line[name_start..]
                .find(char::is_whitespace)
                .map_or(line.len(), |offset| name_start + offset);
            let name = line[name_start..name_end].to_owned();
            let args_start = skip_inline_whitespace(line, name_end);
            let args_end = next_skill_directive(line, args_start).unwrap_or(line.len());
            let args = line[args_start..args_end].trim().to_owned();

            if !args.is_empty() {
                body_line.push_str(&args);
            }
            invocations.push(InlineSkillInvocation { name, args });
            cursor = args_end;
        }
        body_line.push_str(&line[cursor..]);
        body_lines.push(body_line);
    }

    (!invocations.is_empty()).then(|| InlineSkillDirectives {
        invocations,
        body: body_lines.join("\n").trim().to_owned(),
    })
}

fn next_skill_directive(line: &str, start: usize) -> Option<usize> {
    let mut search_from = start.min(line.len());
    while let Some(offset) = line[search_from..].find("/skill:") {
        let pos = search_from + offset;
        if pos == 0
            || line[..pos]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            return Some(pos);
        }
        search_from = pos + "/skill:".len();
    }
    None
}

fn skip_inline_whitespace(line: &str, start: usize) -> usize {
    line[start..]
        .char_indices()
        .find_map(|(offset, ch)| (!ch.is_whitespace()).then_some(start + offset))
        .unwrap_or(line.len())
}

pub(super) fn expand_slash_skill(
    name: &str,
    args_str: &str,
    skill: &neo_agent_core::skills::LoadedSkill,
) -> Result<(String, String)> {
    let mut invocation = neo_agent_core::skills::parse_skill_invocation(args_str)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    name.clone_into(&mut invocation.name);
    let expanded = neo_agent_core::skills::expand_skill_body(skill, &invocation)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok((expanded, invocation.raw_arguments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_skill_directive_parser_handles_single_skill() {
        let parsed = parse_inline_skill_directives("/skill:foo hello").expect("skill directive");

        assert_eq!(parsed.names(), ["foo"]);
        assert_eq!(parsed.body, "hello");
        assert_eq!(parsed.invocations[0].name, "foo");
        assert_eq!(parsed.invocations[0].args, "hello");
    }

    #[test]
    fn inline_skill_directive_parser_keeps_prefix_text() {
        let parsed =
            parse_inline_skill_directives("foo bar /skill:foo hello").expect("skill directive");

        assert_eq!(parsed.names(), ["foo"]);
        assert_eq!(parsed.body, "foo bar hello");
        assert_eq!(parsed.invocations[0].args, "hello");
    }

    #[test]
    fn inline_skill_directive_parser_aggregates_multiple_skills() {
        let input = "\
foo
bar
/skill:skill_one test test test
bonjour
hello
/skill:skill_two test test test test
hola
amigo";

        let parsed = parse_inline_skill_directives(input).expect("skill directives");

        assert_eq!(parsed.names(), ["skill_one", "skill_two"]);
        assert_eq!(
            parsed.body,
            "\
foo
bar
test test test
bonjour
hello
test test test test
hola
amigo"
        );
        assert_eq!(parsed.invocations[0].args, "test test test");
        assert_eq!(parsed.invocations[1].args, "test test test test");
    }

    #[test]
    fn inline_skill_directive_parser_requires_prompt_start_or_whitespace_prefix() {
        assert!(parse_inline_skill_directives("abc/skill:foo test").is_none());

        let parsed = parse_inline_skill_directives("abc /skill:foo test").expect("skill directive");

        assert_eq!(parsed.names(), ["foo"]);
        assert_eq!(parsed.body, "abc test");
        assert_eq!(parsed.invocations[0].args, "test");
    }
}

pub(super) const fn prompt_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    if let Some(edit) = prompt_cursor_edit_for_action(action) {
        return Some(edit);
    }
    prompt_delete_edit_for_action(action)
}

pub(super) const fn prompt_cursor_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorCursorLeft => Some(PromptEdit::MoveLeft),
        KeybindingAction::EditorCursorRight => Some(PromptEdit::MoveRight),
        KeybindingAction::EditorCursorWordLeft => Some(PromptEdit::MoveWordLeft),
        KeybindingAction::EditorCursorWordRight => Some(PromptEdit::MoveWordRight),
        KeybindingAction::EditorCursorLineStart => Some(PromptEdit::MoveHome),
        KeybindingAction::EditorCursorLineEnd => Some(PromptEdit::MoveEnd),
        // Up/Down are handled directly in handle_prompt_keybinding_action, where
        // the composer body width is known, so we do not map them here.
        _ => None,
    }
}

pub(super) const fn prompt_delete_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    if let Some(edit) = prompt_delete_range_edit_for_action(action) {
        return Some(edit);
    }
    prompt_undo_yank_edit_for_action(action)
}

pub(super) const fn prompt_delete_range_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    if let Some(edit) = prompt_delete_char_edit_for_action(action) {
        return Some(edit);
    }
    if let Some(edit) = prompt_delete_word_edit_for_action(action) {
        return Some(edit);
    }
    prompt_delete_line_edit_for_action(action)
}

pub(super) const fn prompt_delete_char_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorDeleteCharBackward => Some(PromptEdit::Backspace),
        KeybindingAction::EditorDeleteCharForward => Some(PromptEdit::Delete),
        _ => None,
    }
}

pub(super) const fn prompt_delete_word_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorDeleteWordBackward => Some(PromptEdit::DeleteWordBackward),
        KeybindingAction::EditorDeleteWordForward => Some(PromptEdit::DeleteWordForward),
        _ => None,
    }
}

pub(super) const fn prompt_delete_line_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorDeleteToLineStart => Some(PromptEdit::DeleteToLineStart),
        KeybindingAction::EditorDeleteToLineEnd => Some(PromptEdit::DeleteToLineEnd),
        _ => None,
    }
}

pub(super) const fn prompt_undo_yank_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorYank => Some(PromptEdit::Yank),
        KeybindingAction::EditorUndo => Some(PromptEdit::Undo),
        _ => None,
    }
}

impl InteractiveController {
    pub(super) fn handle_prompt_edit_event(&mut self, event: &InputEvent) -> bool {
        match event {
            InputEvent::Insert(character) => {
                self.handle_insert_prompt_event(*character);
            }
            InputEvent::Paste(text) => {
                self.handle_paste_text(text);
            }
            InputEvent::Backspace => {
                if self.tui.chrome().shell_mode_active()
                    && self.tui.chrome().prompt().text.is_empty()
                {
                    self.tui.chrome_mut().exit_shell_mode();
                    return true;
                }
                self.apply_prompt_edit(PromptEdit::Backspace);
            }
            InputEvent::Delete => self.apply_prompt_edit(PromptEdit::Delete),
            InputEvent::MoveLeft => self.apply_prompt_edit(PromptEdit::MoveLeft),
            InputEvent::MoveRight => self.apply_prompt_edit(PromptEdit::MoveRight),
            InputEvent::MoveHome => self.apply_prompt_edit(PromptEdit::MoveHome),
            InputEvent::MoveEnd => self.apply_prompt_edit(PromptEdit::MoveEnd),
            InputEvent::NewLine => self.apply_prompt_edit(PromptEdit::Insert("\n")),
            _ => return false,
        }
        true
    }

    pub(super) fn handle_insert_prompt_event(&mut self, character: char) {
        if self.try_choose_approval_number(character) {
            return;
        }
        if character == '!'
            && !self.tui.chrome().shell_mode_active()
            && self.tui.chrome().prompt().text.is_empty()
        {
            self.tui.chrome_mut().enter_shell_mode();
            self.sync_inline_prompt_completion();
            return;
        }
        self.apply_prompt_edit(PromptEdit::Insert(&character.to_string()));
    }

    pub(super) fn handle_paste_text(&mut self, text: &str) {
        let cleaned = Self::clean_pasted_text(text);
        if !self.tui.chrome().shell_mode_active()
            && self.tui.chrome().prompt().text.is_empty()
            && let Some(command) = cleaned.strip_prefix('!')
        {
            self.tui.chrome_mut().enter_shell_mode();
            if !command.is_empty() {
                self.apply_prompt_edit(PromptEdit::Insert(command));
            }
            return;
        }
        // When the terminal intercepts Ctrl+V (e.g. Ghostty on macOS) it sends
        // a bracketed-paste event. If the clipboard contains an image (not
        // text), the paste content may be empty or contain non-text artifacts.
        // Try to read an image from the clipboard in that case.
        if cleaned.is_empty()
            && self.model_supports_images()
            && let Ok(image) = crate::clipboard::read_clipboard_image()
        {
            let (width, height) =
                crate::image_blob::detect_image_dimensions(&image.bytes, &image.mime_type)
                    .unwrap_or((0, 0));
            let sha256 = crate::image_blob::sha256_hex(&image.bytes);
            let id = self.image_attachment_store.add(
                sha256,
                image.mime_type,
                width,
                height,
                Some(image.bytes),
            );
            let placeholder = format!("[image #{id} ({width}x{height})]");
            self.apply_prompt_edit(PromptEdit::Insert(&placeholder));
            return;
        }

        let line_count = cleaned.split('\n').count();
        if line_count > 10 || cleaned.len() > 1000 {
            let id = self.next_paste_id;
            self.next_paste_id += 1;
            self.paste_store.insert(id, cleaned);
            let marker = if line_count > 10 {
                format!("[paste +{line_count} lines]")
            } else {
                format!("[paste {id} chars]")
            };
            self.apply_prompt_edit(PromptEdit::Insert(&marker));
        } else {
            self.apply_prompt_edit(PromptEdit::Insert(&cleaned));
        }
    }

    pub(super) fn expand_marker_at_cursor(&mut self) -> bool {
        let prompt = self.tui.chrome().prompt();
        let text = prompt.text.clone();
        let cursor_byte = prompt.byte_index(prompt.cursor);
        for cap in neo_tui::paste::marker_regex().captures_iter(&text) {
            let m = cap.get(0).expect("regex match has group 0");
            if m.start() <= cursor_byte && m.end() >= cursor_byte {
                let id = cap
                    .get(2)
                    .or_else(|| cap.get(3))
                    .or_else(|| cap.get(5))
                    .and_then(|m| m.as_str().parse::<usize>().ok());
                if let Some((id, original)) = id.and_then(|id| {
                    self.paste_store
                        .get(&id)
                        .cloned()
                        .map(|original| (id, original))
                }) {
                    let before = &text[..m.start()];
                    let after = &text[m.end()..];
                    let new_text = format!("{before}{original}{after}");
                    self.tui.chrome_mut().prompt_mut().set_text(new_text);
                    self.paste_store.remove(&id);
                    return true;
                }
            }
        }
        false
    }

    #[allow(clippy::unused_async)] // sync API kept for symmetry with submit_shell_command; no awaits inside.
    pub(super) async fn handle_paste_image(&mut self) -> Result<()> {
        if !self.model_supports_images() {
            // Model doesn't support images — fall through to text paste.
            self.fallback_text_paste();
            return Ok(());
        }

        if self.expand_marker_at_cursor() {
            return Ok(());
        }

        let image = match crate::clipboard::read_clipboard_image() {
            Ok(img) => img,
            Err(crate::clipboard::ClipboardError::NoImage) => {
                // No image in clipboard — fall through to text paste (like
                // kimi-code: Ctrl+V pastes text when no image is available).
                self.fallback_text_paste();
                return Ok(());
            }
            Err(err) => {
                self.push_status(format!("读取剪贴板图片失败: {err}"));
                return Ok(());
            }
        };

        let (width, height) =
            crate::image_blob::detect_image_dimensions(&image.bytes, &image.mime_type)
                .unwrap_or((0, 0));

        // Use the SHA-256 as a dedup key and the blob path for persistence,
        // but store raw bytes in the attachment for inline base64 encoding.
        // This avoids requiring a session directory at paste time.
        let sha256 = crate::image_blob::sha256_hex(&image.bytes);

        let id = self.image_attachment_store.add(
            sha256,
            image.mime_type,
            width,
            height,
            Some(image.bytes),
        );
        let placeholder = format!("[image #{id} ({width}x{height})]");
        self.apply_prompt_edit(PromptEdit::Insert(&placeholder));
        Ok(())
    }

    pub(super) fn fallback_text_paste(&mut self) {
        let text = crate::clipboard::read_text_clipboard();
        if let Some(text) = text
            && !text.is_empty()
        {
            self.handle_paste_text(&text);
        }
    }

    pub(super) fn model_supports_images(&self) -> bool {
        self.active_model
            .as_ref()
            .and_then(|m| self.model_capabilities.get(&m.alias))
            .is_some_and(|c| c.images)
    }

    pub(super) fn apply_prompt_edit(&mut self, edit: PromptEdit<'_>) {
        self.clear_pending_exit_confirmation();
        let body_width = Self::prompt_body_width();
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit_with_width(edit, body_width);
        self.sync_inline_prompt_completion();
    }

    pub(super) fn clean_pasted_text(text: &str) -> String {
        text.replace('\r', "")
            .chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .collect()
    }

    pub(super) fn complete_prompt_or_insert_tab(&mut self) {
        self.clear_pending_exit_confirmation();
        if self.tui.chrome_mut().selected_prompt_completion().is_some() {
            let _ = self.tui.chrome_mut().confirm_prompt_completion();
            return;
        }
        let Some(prefix) = self.tui.chrome_mut().prompt().completion_prefix() else {
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit(PromptEdit::Insert("\t"));
            return;
        };
        let completions = match prompt_completions(
            &self.completion_root,
            &prefix.text,
            &self.model_items,
            self.skill_store.as_ref(),
            self.project_trusted(),
        ) {
            Ok(completions) => completions,
            Err(error) => {
                self.push_status(format!("Completion error: {error}"));
                return;
            }
        };

        if completions.is_empty() {
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit(PromptEdit::Insert("\t"));
            return;
        }

        if let Some(common_prefix) = longest_common_completion_prefix(&completions)
            && common_prefix.chars().count() > prefix.text.chars().count()
        {
            let _ = self
                .tui
                .chrome_mut()
                .prompt_mut()
                .replace_completion_prefix(&prefix, &common_prefix);
            return;
        }

        if completions.len() == 1 {
            let _ = self
                .tui
                .chrome_mut()
                .prompt_mut()
                .replace_completion_prefix(&prefix, &completions[0].value);
            return;
        }

        self.tui
            .chrome_mut()
            .open_prompt_completion_picker(prefix, completions);
    }

    pub(super) fn sync_inline_prompt_completion(&mut self) {
        let Some(prefix) = self.tui.chrome_mut().prompt().completion_prefix() else {
            self.close_inline_prompt_completion();
            return;
        };

        if !prefix.text.starts_with('/') {
            self.close_inline_prompt_completion();
            return;
        }

        let completions = match prompt_completions(
            &self.completion_root,
            &prefix.text,
            &self.model_items,
            self.skill_store.as_ref(),
            self.project_trusted(),
        ) {
            Ok(completions) => completions,
            Err(error) => {
                self.close_inline_prompt_completion();
                self.push_status(format!("Completion error: {error}"));
                return;
            }
        };

        if completions.is_empty() {
            self.close_inline_prompt_completion();
            return;
        }

        let focused_is_prompt_completion = self
            .tui
            .chrome_mut()
            .focused_overlay()
            .is_some_and(|overlay| matches!(overlay.kind, OverlayKind::PromptCompletion(_)));
        if focused_is_prompt_completion {
            let _ = self.tui.chrome_mut().close_focused_overlay();
        } else if self.tui.chrome_mut().focused_overlay_id().is_some() {
            return;
        }

        self.tui
            .chrome_mut()
            .open_prompt_completion_picker(prefix, completions);
    }

    pub(super) fn close_inline_prompt_completion(&mut self) {
        if self
            .tui
            .chrome_mut()
            .focused_overlay()
            .is_some_and(|overlay| matches!(overlay.kind, OverlayKind::PromptCompletion(_)))
        {
            let _ = self.tui.chrome_mut().close_focused_overlay();
        }
    }

    pub(super) fn prompt_body_width() -> usize {
        let (cols, _) = size().unwrap_or((80, 24));
        let content_width = frame_content_width(usize::from(cols));
        content_width.saturating_sub(2).saturating_sub(4).max(1)
    }

    pub(super) fn copy_prompt_to_clipboard(&mut self) {
        let Some(copied) = self.tui.chrome_mut().copy_prompt_text() else {
            return;
        };
        self.write_clipboard_text(&copied);
    }

    pub(super) fn copy_transcript_selection_to_clipboard(&mut self) {
        let Some(copied) = self.transcript_mut().copy_selected_transcript_text() else {
            return;
        };
        self.write_clipboard_text(&copied);
    }

    pub(super) fn write_clipboard_text(&mut self, copied: &str) {
        if let Err(error) = (self.clipboard_writer)(copied) {
            self.push_status(format!("Clipboard copy failed: {error}"));
        }
    }
}
