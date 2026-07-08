use unicode_segmentation::UnicodeSegmentation;

use super::pickers::PromptCompletionPrefix;

/// Display width of a grapheme for prompt cursor math. Tabs are treated as
/// four columns to match the visual expansion used by the renderer.
fn prompt_grapheme_width(grapheme: &str) -> usize {
    if grapheme == "\t" {
        4
    } else {
        crate::primitive::visible_width(grapheme)
    }
}

/// Wrap `text` into display rows of at most `body_width` columns, treating tabs
/// as four columns. The returned strings preserve the original graphemes (tabs
/// stay as tabs); only the segment boundaries depend on expanded widths.
fn wrap_prompt_lines(text: &str, body_width: usize) -> Vec<(usize, String)> {
    if body_width == 0 {
        return vec![(0, String::new())];
    }

    let mut result = Vec::new();
    let mut char_index = 0;

    for logical_line in text.split('\n') {
        if logical_line.is_empty() {
            result.push((char_index, String::new()));
        } else {
            let mut current = String::new();
            let mut current_width = 0;
            let mut active_sgr = String::new();
            let mut byte_index = 0;
            let mut segment_start = char_index;

            while byte_index < logical_line.len() {
                if let Some(sequence) = crate::primitive::next_sequence(logical_line, byte_index) {
                    current.push_str(sequence);
                    crate::primitive::update_active_sgr(sequence, &mut active_sgr);
                    byte_index += sequence.len();
                    continue;
                }

                let Some(grapheme) = logical_line[byte_index..].graphemes(true).next() else {
                    break;
                };

                let grapheme_width = prompt_grapheme_width(grapheme);
                if current_width > 0 && current_width + grapheme_width > body_width {
                    result.push((segment_start, std::mem::take(&mut current)));
                    segment_start = char_index;
                    current.push_str(&active_sgr);
                    current_width = 0;
                }

                current.push_str(grapheme);
                current_width += grapheme_width;
                byte_index += grapheme.len();
                char_index += grapheme.chars().count();
            }

            if !current.is_empty() {
                result.push((segment_start, current));
            }
        }
        char_index += 1; // for the '\n' separator
    }

    if result.is_empty() {
        result.push((0, String::new()));
    }
    result
}

/// Return the char index in `text` whose left edge is closest to but not
/// greater than `target_col` display columns. Tabs count as four columns and
/// ANSI sequences are skipped because they have zero display width.
fn char_index_at_visual_col(text: &str, target_col: usize) -> usize {
    let mut walked = 0;
    let mut chars = 0;
    for grapheme in text.graphemes(true) {
        let width = prompt_grapheme_width(grapheme);
        if width == 0 {
            chars += grapheme.chars().count();
            continue;
        }
        if walked + width > target_col {
            break;
        }
        walked += width;
        chars += grapheme.chars().count();
    }
    chars
}

/// Return the display width of the first `char_index` characters of `text`.
/// Tabs count as four columns and ANSI sequences contribute zero width.
fn visual_col_at_char_index(text: &str, char_index: usize) -> usize {
    let mut walked = 0;
    let mut chars = 0;
    for grapheme in text.graphemes(true) {
        if chars >= char_index {
            break;
        }
        walked += prompt_grapheme_width(grapheme);
        chars += grapheme.chars().count();
    }
    walked
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptState {
    pub text: String,
    pub cursor: usize,
    scroll_offset: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<PromptSnapshot>,
    undo_stack: Vec<PromptSnapshot>,
    kill_ring: Vec<String>,
    /// Byte range of a marker currently selected for deletion. The next
    /// backspace/delete while the same marker is selected removes it entirely.
    selected_marker: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptSnapshot {
    text: String,
    cursor: usize,
}

impl PromptState {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        Self {
            text,
            cursor,
            scroll_offset: 0,
            history: Vec::new(),
            history_index: None,
            history_draft: None,
            undo_stack: Vec::new(),
            kill_ring: Vec::new(),
            selected_marker: None,
        }
    }

    #[must_use]
    pub fn with_cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor.min(self.text.chars().count());
        self
    }

    /// Snapshot of the in-memory history (oldest → newest). Used by callers
    /// that seed history and by tests asserting on recalled entries.
    #[must_use]
    pub fn history_snapshot(&self) -> Vec<String> {
        self.history.clone()
    }

    pub fn remember_history(&mut self, entry: impl Into<String>) {
        let entry = entry.into().trim().to_owned();
        if entry.is_empty() {
            return;
        }
        // Skip consecutive duplicates: a repeat right after the same prompt
        // adds no recall value and would clutter both in-memory and persisted
        // history. Non-consecutive repeats are kept.
        if self.history.last().is_some_and(|last| last == &entry) {
            self.stop_history_navigation();
            return;
        }
        self.history.push(entry);
        self.stop_history_navigation();
    }

    /// Replace the in-memory history with the provided entries. Entries are
    /// trimmed and consecutive duplicates are collapsed via `remember_history`,
    /// matching the semantics of a single submit. Use this to seed a fresh
    /// controller with workspace-loaded prompt history.
    pub fn set_history(&mut self, entries: impl IntoIterator<Item = String>) {
        self.history.clear();
        self.history_index = None;
        self.history_draft = None;
        for entry in entries {
            self.remember_history(entry);
        }
    }

    pub fn clear_after_submit(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
        self.undo_stack.clear();
        self.kill_ring.clear();
        self.selected_marker = None;
        self.stop_history_navigation();
    }

    /// Byte range of the currently selected marker, if any.
    #[must_use]
    pub fn selected_marker(&self) -> Option<(usize, usize)> {
        self.selected_marker
    }

    /// Replace the composer text and move the cursor to the end. Used when
    /// pulling a queued message back into the composer for editing.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.char_len();
        self.scroll_offset = 0;
        self.selected_marker = None;
        self.stop_history_navigation();
    }

    pub fn recall_previous_history(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        // Do not overwrite a non-empty draft on the first Up. Once navigation
        // has started (history_index is set), Up keeps moving older as expected.
        if self.history_index.is_none() && !self.text.is_empty() {
            return false;
        }
        let index = if let Some(index) = self.history_index {
            index.saturating_sub(1)
        } else {
            self.history_draft = Some(self.snapshot());
            self.history.len() - 1
        };
        self.history_index = Some(index);
        self.replace_with_history_text(index);
        true
    }

    pub fn recall_next_history(&mut self) -> bool {
        let Some(index) = self.history_index else {
            return false;
        };
        let next = index + 1;
        if next < self.history.len() {
            self.history_index = Some(next);
            self.replace_with_history_text(next);
        } else {
            if let Some(snapshot) = self.history_draft.take() {
                self.text = snapshot.text;
                self.cursor = snapshot.cursor.min(self.char_len());
            } else {
                self.text.clear();
                self.cursor = 0;
            }
            self.history_index = None;
            self.undo_stack.clear();
        }
        true
    }

    pub fn apply_edit(&mut self, edit: PromptEdit<'_>) -> Option<String> {
        self.apply_edit_with_width(edit, 0)
    }

    #[allow(clippy::too_many_lines)]
    pub fn apply_edit_with_width(
        &mut self,
        edit: PromptEdit<'_>,
        body_width: usize,
    ) -> Option<String> {
        self.cursor = self.cursor.min(self.char_len());

        let result = match edit {
            PromptEdit::Insert(text) => {
                let inserted = text.to_string();
                if inserted.is_empty() {
                    return None;
                }
                self.stop_history_navigation();
                self.selected_marker = None;
                let before = self.snapshot();
                let index = self.byte_index(self.cursor);
                self.text.insert_str(index, &inserted);
                self.cursor += inserted.chars().count();
                self.push_undo(before);
                Some(inserted)
            }
            PromptEdit::Clear => {
                if self.text.is_empty() {
                    return None;
                }
                self.stop_history_navigation();
                self.selected_marker = None;
                let before = self.snapshot();
                let cleared = std::mem::take(&mut self.text);
                self.cursor = 0;
                self.scroll_offset = 0;
                self.push_undo(before);
                Some(cleared)
            }
            PromptEdit::Backspace => {
                if let Some(range) = self.marker_before_cursor() {
                    if self.selected_marker == Some(range) {
                        self.selected_marker = None;
                        self.delete_byte_range(range.0, range.1, DeleteDirection::Backward)
                    } else {
                        self.selected_marker = Some(range);
                        None
                    }
                } else {
                    self.selected_marker = None;
                    self.apply_delete(
                        self.cursor.saturating_sub(1),
                        self.cursor,
                        DeleteDirection::Backward,
                        false,
                    )
                }
            }
            PromptEdit::Delete => {
                if let Some(range) = self.marker_after_cursor() {
                    if self.selected_marker == Some(range) {
                        self.selected_marker = None;
                        self.delete_byte_range(range.0, range.1, DeleteDirection::Forward)
                    } else {
                        self.selected_marker = Some(range);
                        None
                    }
                } else {
                    self.selected_marker = None;
                    self.apply_delete(
                        self.cursor,
                        self.cursor + 1,
                        DeleteDirection::Forward,
                        false,
                    )
                }
            }
            PromptEdit::MoveLeft => {
                self.cursor = self.cursor.saturating_sub(1);
                self.selected_marker = None;
                None
            }
            PromptEdit::MoveRight => {
                self.cursor = (self.cursor + 1).min(self.char_len());
                self.selected_marker = None;
                None
            }
            PromptEdit::MoveHome => {
                self.cursor = self.current_line_start();
                self.selected_marker = None;
                None
            }
            PromptEdit::MoveEnd => {
                self.cursor = self.current_line_end();
                self.selected_marker = None;
                None
            }
            PromptEdit::MoveWordLeft => {
                self.cursor = find_word_backward(&self.text, self.cursor);
                self.selected_marker = None;
                None
            }
            PromptEdit::MoveWordRight => {
                self.cursor = find_word_forward(&self.text, self.cursor);
                self.selected_marker = None;
                None
            }
            PromptEdit::DeleteWordBackward => {
                let start = find_word_backward(&self.text, self.cursor);
                self.apply_delete(start, self.cursor, DeleteDirection::Backward, true)
            }
            PromptEdit::DeleteWordForward => {
                let end = find_word_forward(&self.text, self.cursor);
                self.apply_delete(self.cursor, end, DeleteDirection::Forward, true)
            }
            PromptEdit::DeleteToLineStart => {
                let line_start = self.current_line_start();
                if line_start == self.cursor && self.cursor > 0 {
                    self.apply_delete(
                        self.cursor - 1,
                        self.cursor,
                        DeleteDirection::Backward,
                        true,
                    )
                } else {
                    self.apply_delete(line_start, self.cursor, DeleteDirection::Backward, true)
                }
            }
            PromptEdit::DeleteToLineEnd => {
                let line_end = self.current_line_end();
                if line_end == self.cursor && self.cursor < self.char_len() {
                    self.apply_delete(self.cursor, self.cursor + 1, DeleteDirection::Forward, true)
                } else {
                    self.apply_delete(self.cursor, line_end, DeleteDirection::Forward, true)
                }
            }
            PromptEdit::Yank => {
                let yanked = self.kill_ring.last().cloned()?;
                self.stop_history_navigation();
                self.selected_marker = None;
                let before = self.snapshot();
                let index = self.byte_index(self.cursor);
                self.text.insert_str(index, &yanked);
                self.cursor += yanked.chars().count();
                self.push_undo(before);
                Some(yanked)
            }
            PromptEdit::Undo => {
                self.stop_history_navigation();
                self.selected_marker = None;
                if let Some(snapshot) = self.undo_stack.pop() {
                    self.text = snapshot.text;
                    self.cursor = snapshot.cursor.min(self.char_len());
                }
                None
            }
            PromptEdit::MoveUp(width) => {
                self.move_cursor_vertical(width, -1);
                self.selected_marker = None;
                None
            }
            PromptEdit::MoveDown(width) => {
                self.move_cursor_vertical(width, 1);
                self.selected_marker = None;
                None
            }
        };
        if body_width > 0 {
            self.clamp_scroll_offset(body_width);
        }
        result
    }

    /// Move the cursor up/down by one wrapped logical line, preserving the
    /// visual column when possible. `body_width` is the width available for the
    /// composer body (content width minus borders and padding).
    fn move_cursor_vertical(&mut self, body_width: usize, direction: isize) {
        if body_width == 0 || self.text.is_empty() {
            return;
        }
        let wrapped = wrap_prompt_lines(&self.text, body_width);
        if wrapped.len() <= 1 {
            return;
        }
        let current_idx = wrapped
            .partition_point(|(start, _)| *start <= self.cursor)
            .saturating_sub(1);
        let target_idx = if direction < 0 {
            current_idx.saturating_sub(1)
        } else {
            (current_idx + 1).min(wrapped.len() - 1)
        };
        if target_idx == current_idx {
            return;
        }
        let (current_start, _) = &wrapped[current_idx];
        let offset_in_current = self.cursor.saturating_sub(*current_start);
        let prefix_text: String = self
            .text
            .chars()
            .skip(*current_start)
            .take(offset_in_current)
            .collect();
        let visual_col = visual_col_at_char_index(&prefix_text, offset_in_current);
        let (target_start, target_line) = &wrapped[target_idx];
        let target_offset = char_index_at_visual_col(target_line, visual_col);
        self.cursor = target_start + target_offset;
        self.clamp_scroll_offset(body_width);
    }

    /// Keep the scroll offset within bounds and ensure the cursor line is
    /// visible in the viewport.
    fn clamp_scroll_offset(&mut self, body_width: usize) {
        if body_width == 0 {
            self.scroll_offset = 0;
            return;
        }
        let wrapped = wrap_prompt_lines(&self.text, body_width);
        let cursor_line = wrapped
            .partition_point(|(start, _)| *start <= self.cursor)
            .saturating_sub(1);
        if cursor_line < self.scroll_offset {
            self.scroll_offset = cursor_line;
        } else if cursor_line >= self.scroll_offset + super::MAX_PROMPT_VISIBLE_LINES {
            self.scroll_offset = cursor_line.saturating_sub(super::MAX_PROMPT_VISIBLE_LINES - 1);
        }
        let max_offset = wrapped
            .len()
            .saturating_sub(super::MAX_PROMPT_VISIBLE_LINES);
        self.scroll_offset = self.scroll_offset.min(max_offset);
    }

    #[must_use]
    pub const fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Whether the prompt is currently navigating history (true) or editing
    /// the current draft (false). Used by keybinding dispatch to decide whether
    /// ↑/↓ should move the cursor or recall the next/previous history entry.
    #[must_use]
    pub fn in_history_navigation(&self) -> bool {
        self.history_index.is_some()
    }

    #[must_use]
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    #[must_use]
    pub fn copy_text(&self) -> Option<String> {
        (!self.text.is_empty()).then(|| self.text.clone())
    }

    /// Byte range of the marker immediately before or overlapping the cursor,
    /// if any.
    fn marker_before_cursor(&self) -> Option<(usize, usize)> {
        let cursor_byte = self.byte_index(self.cursor);
        for cap in crate::paste::marker_regex().captures_iter(&self.text) {
            let m = cap.get(0).expect("regex match has group 0");
            if m.end() == cursor_byte {
                return Some((m.start(), m.end()));
            }
            if m.start() < cursor_byte && m.end() > cursor_byte {
                return Some((m.start(), m.end()));
            }
        }
        None
    }

    /// Byte range of the marker immediately at or after the cursor, if any.
    fn marker_after_cursor(&self) -> Option<(usize, usize)> {
        let cursor_byte = self.byte_index(self.cursor);
        for cap in crate::paste::marker_regex().captures_iter(&self.text) {
            let m = cap.get(0).expect("regex match has group 0");
            if m.start() == cursor_byte || (m.start() <= cursor_byte && m.end() > cursor_byte) {
                return Some((m.start(), m.end()));
            }
        }
        None
    }

    /// Delete a byte range directly, bypassing char-index logic.
    fn delete_byte_range(
        &mut self,
        start_byte: usize,
        end_byte: usize,
        direction: DeleteDirection,
    ) -> Option<String> {
        self.stop_history_navigation();
        let before = self.snapshot();
        if start_byte >= end_byte || end_byte > self.text.len() {
            return None;
        }
        let deleted = self.text[start_byte..end_byte].to_string();
        self.text.replace_range(start_byte..end_byte, "");
        match direction {
            DeleteDirection::Backward => {
                self.cursor = self.text[..start_byte].chars().count();
            }
            DeleteDirection::Forward => {
                self.cursor = self.cursor.min(self.char_len());
            }
        }
        self.push_undo(before);
        Some(deleted)
    }

    #[must_use]
    pub fn completion_prefix(&self) -> Option<PromptCompletionPrefix> {
        let chars = self.text.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(chars.len());
        let mut start = cursor;
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == cursor {
            return None;
        }
        if let Some(at_start) = at_reference_prefix_start(&chars, start, cursor) {
            return Some(PromptCompletionPrefix {
                start: at_start,
                end: cursor,
                text: chars[at_start..cursor].iter().collect(),
            });
        }
        Some(PromptCompletionPrefix {
            start,
            end: cursor,
            text: chars[start..cursor].iter().collect(),
        })
    }

    pub fn replace_completion_prefix(
        &mut self,
        prefix: &PromptCompletionPrefix,
        replacement: &str,
    ) -> Option<String> {
        if replacement.is_empty() {
            return None;
        }
        let len = self.char_len();
        if prefix.start > prefix.end || prefix.end > len {
            return None;
        }
        if self.slice_chars(prefix.start, prefix.end)? != prefix.text {
            return None;
        }

        self.stop_history_navigation();
        let before = self.snapshot();
        let start_byte = self.byte_index(prefix.start);
        let end_byte = self.byte_index(prefix.end);
        self.text.replace_range(start_byte..end_byte, replacement);
        self.cursor = prefix.start + replacement.chars().count();
        self.push_undo(before);
        Some(replacement.to_owned())
    }

    #[must_use]
    pub fn byte_index(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }

        self.text
            .char_indices()
            .nth(char_index)
            .map_or(self.text.len(), |(index, _)| index)
    }

    fn current_line_start(&self) -> usize {
        let cursor = self.cursor.min(self.char_len());
        self.text
            .chars()
            .take(cursor)
            .enumerate()
            .filter_map(|(index, ch)| (ch == '\n').then_some(index + 1))
            .last()
            .unwrap_or(0)
    }

    fn current_line_end(&self) -> usize {
        let cursor = self.cursor.min(self.char_len());
        self.text
            .chars()
            .skip(cursor)
            .position(|ch| ch == '\n')
            .map_or_else(|| self.char_len(), |offset| cursor + offset)
    }

    fn slice_chars(&self, start: usize, end: usize) -> Option<String> {
        if start > end || end > self.char_len() {
            return None;
        }
        let start_byte = self.byte_index(start);
        let end_byte = self.byte_index(end);
        Some(self.text[start_byte..end_byte].to_owned())
    }

    fn snapshot(&self) -> PromptSnapshot {
        PromptSnapshot {
            text: self.text.clone(),
            cursor: self.cursor,
        }
    }

    fn push_undo(&mut self, snapshot: PromptSnapshot) {
        self.undo_stack.push(snapshot);
    }

    fn replace_with_history_text(&mut self, index: usize) {
        self.text = self.history[index].clone();
        self.cursor = self.char_len();
        self.scroll_offset = 0;
        self.undo_stack.clear();
    }

    fn stop_history_navigation(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }

    fn apply_delete(
        &mut self,
        start: usize,
        end: usize,
        direction: DeleteDirection,
        record_kill: bool,
    ) -> Option<String> {
        self.stop_history_navigation();
        let before = self.snapshot();
        let deleted = self.delete_range(start, end, direction)?;
        self.push_undo(before);
        if record_kill {
            self.kill_ring.push(deleted.clone());
        }
        Some(deleted)
    }

    fn delete_range(
        &mut self,
        start: usize,
        end: usize,
        direction: DeleteDirection,
    ) -> Option<String> {
        let len = self.char_len();
        let start = start.min(len);
        let end = end.min(len);
        if start >= end {
            return None;
        }

        let start_byte = self.byte_index(start);
        let end_byte = self.byte_index(end);
        let deleted = self.text[start_byte..end_byte].to_string();
        self.text.replace_range(start_byte..end_byte, "");

        match direction {
            DeleteDirection::Backward => self.cursor = start,
            DeleteDirection::Forward => self.cursor = self.cursor.min(self.char_len()),
        }

        Some(deleted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteDirection {
    Backward,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptEdit<'a> {
    Insert(&'a str),
    Clear,
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
    MoveWordLeft,
    MoveWordRight,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    Yank,
    Undo,
    /// Move the cursor up one wrapped logical line. The `usize` is the body
    /// width used to compute the wrapped lines.
    MoveUp(usize),
    /// Move the cursor down one wrapped logical line. The `usize` is the body
    /// width used to compute the wrapped lines.
    MoveDown(usize),
}

fn find_word_backward(text: &str, cursor: usize) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = cursor.min(chars.len());

    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }

    if index == 0 {
        return 0;
    }

    let word_like = is_word_like(chars[index - 1]);
    while index > 0
        && is_word_like(chars[index - 1]) == word_like
        && !chars[index - 1].is_whitespace()
    {
        index -= 1;
    }

    index
}

fn find_word_forward(text: &str, cursor: usize) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = cursor.min(chars.len());

    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }

    if index >= chars.len() {
        return index;
    }

    let word_like = is_word_like(chars[index]);
    while index < chars.len()
        && is_word_like(chars[index]) == word_like
        && !chars[index].is_whitespace()
    {
        index += 1;
    }

    index
}

fn is_word_like(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn at_reference_prefix_start(chars: &[char], token_start: usize, cursor: usize) -> Option<usize> {
    (token_start..cursor)
        .rev()
        .find(|index| chars[*index] == '@' && (*index == 0 || !is_word_like(chars[*index - 1])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_prefix_after_whitespace_replaces_only_current_slash_token() {
        let mut prompt = PromptState::new("foo /sk");
        let prefix = prompt.completion_prefix().expect("completion prefix");

        assert_eq!(prefix.start, 4);
        assert_eq!(prefix.end, 7);
        assert_eq!(prefix.text, "/sk");

        let replaced = prompt
            .replace_completion_prefix(&prefix, "/skill:bar")
            .expect("replace prefix");

        assert_eq!(replaced, "/skill:bar");
        assert_eq!(prompt.text, "foo /skill:bar");
        assert_eq!(prompt.cursor, "foo /skill:bar".chars().count());
    }

    #[test]
    fn completion_prefix_without_whitespace_keeps_whole_token() {
        let prompt = PromptState::new("foo/sk");
        let prefix = prompt.completion_prefix().expect("completion prefix");

        assert_eq!(prefix.text, "foo/sk");
    }

    #[test]
    fn completion_prefix_after_punctuation_replaces_at_token() {
        let prompt = PromptState::new("read(@tests");
        let prefix = prompt.completion_prefix().expect("completion prefix");

        assert_eq!(prefix.start, 5);
        assert_eq!(prefix.end, 11);
        assert_eq!(prefix.text, "@tests");
    }

    #[test]
    fn completion_prefix_rejects_embedded_at_token() {
        let prompt = PromptState::new("email@example.com");
        let prefix = prompt.completion_prefix().expect("completion prefix");

        assert_eq!(prefix.start, 0);
        assert_eq!(prefix.text, "email@example.com");
    }
}
