use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    User {
        content: String,
    },
    Assistant {
        content: String,
    },
    Tool {
        name: String,
        detail: String,
        status: ToolStatusKind,
    },
    Notice {
        content: String,
    },
}

impl TranscriptItem {
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
        }
    }

    #[must_use]
    pub fn tool(
        name: impl Into<String>,
        detail: impl Into<String>,
        status: ToolStatusKind,
    ) -> Self {
        Self::Tool {
            name: name.into(),
            detail: detail.into(),
            status,
        }
    }

    #[must_use]
    pub fn notice(content: impl Into<String>) -> Self {
        Self::Notice {
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatTranscript {
    items: Vec<TranscriptItem>,
}

impl ChatTranscript {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_items(items: impl IntoIterator<Item = TranscriptItem>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    pub fn push(&mut self, item: TranscriptItem) {
        self.items.push(item);
    }

    pub fn update_last_assistant(&mut self, content: impl Into<String>) -> bool {
        let Some(TranscriptItem::Assistant { content: existing }) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, TranscriptItem::Assistant { .. }))
        else {
            return false;
        };

        *existing = content.into();
        true
    }

    #[must_use]
    pub fn items(&self) -> &[TranscriptItem] {
        &self.items
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptView {
    scrollback: usize,
}

impl TranscriptView {
    #[must_use]
    pub const fn new() -> Self {
        Self { scrollback: 0 }
    }

    #[must_use]
    pub const fn scrollback(&self) -> usize {
        self.scrollback
    }

    pub fn follow_bottom(&mut self) {
        self.scrollback = 0;
    }

    pub fn scroll_up(&mut self, lines: usize, transcript: &ChatTranscript, height: usize) {
        let max_scrollback = transcript.len().saturating_sub(height);
        self.scrollback = self.scrollback.saturating_add(lines).min(max_scrollback);
    }

    pub fn scroll_down(&mut self, lines: usize, transcript: &ChatTranscript, height: usize) {
        let max_scrollback = transcript.len().saturating_sub(height);
        self.scrollback = self.scrollback.saturating_sub(lines).min(max_scrollback);
    }

    #[must_use]
    pub fn visible_range(&self, transcript: &ChatTranscript, height: usize) -> Range<usize> {
        if height == 0 || transcript.is_empty() {
            return 0..0;
        }

        let len = transcript.len();
        let window = height.min(len);
        let bottom = len.saturating_sub(self.scrollback).max(window);
        bottom - window..bottom
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatusKind {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl ToolStatusKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Pending => "-",
            Self::Running => "*",
            Self::Succeeded => "+",
            Self::Failed => "!",
            Self::Cancelled => "x",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    pub name: String,
    pub kind: ToolStatusKind,
    pub detail: Option<String>,
}

impl ToolStatus {
    #[must_use]
    pub fn new(name: impl Into<String>, kind: ToolStatusKind) -> Self {
        Self {
            name: name.into(),
            kind,
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptState {
    pub text: String,
    pub cursor: usize,
}

impl PromptState {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        Self { text, cursor }
    }

    #[must_use]
    pub fn with_cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor.min(self.text.chars().count());
        self
    }

    pub fn apply_edit(&mut self, edit: PromptEdit<'_>) -> Option<String> {
        self.cursor = self.cursor.min(self.char_len());

        match edit {
            PromptEdit::Insert(text) => {
                let inserted = text.to_string();
                let index = self.byte_index(self.cursor);
                self.text.insert_str(index, &inserted);
                self.cursor += inserted.chars().count();
                Some(inserted)
            }
            PromptEdit::Backspace => self.delete_range(
                self.cursor.saturating_sub(1),
                self.cursor,
                DeleteDirection::Backward,
            ),
            PromptEdit::Delete => {
                self.delete_range(self.cursor, self.cursor + 1, DeleteDirection::Forward)
            }
            PromptEdit::MoveLeft => {
                self.cursor = self.cursor.saturating_sub(1);
                None
            }
            PromptEdit::MoveRight => {
                self.cursor = (self.cursor + 1).min(self.char_len());
                None
            }
            PromptEdit::MoveHome => {
                self.cursor = 0;
                None
            }
            PromptEdit::MoveEnd => {
                self.cursor = self.char_len();
                None
            }
            PromptEdit::MoveWordLeft => {
                self.cursor = find_word_backward(&self.text, self.cursor);
                None
            }
            PromptEdit::MoveWordRight => {
                self.cursor = find_word_forward(&self.text, self.cursor);
                None
            }
            PromptEdit::DeleteWordBackward => {
                let start = find_word_backward(&self.text, self.cursor);
                self.delete_range(start, self.cursor, DeleteDirection::Backward)
            }
            PromptEdit::DeleteWordForward => {
                let end = find_word_forward(&self.text, self.cursor);
                self.delete_range(self.cursor, end, DeleteDirection::Forward)
            }
            PromptEdit::DeleteToLineStart => {
                self.delete_range(0, self.cursor, DeleteDirection::Backward)
            }
            PromptEdit::DeleteToLineEnd => {
                self.delete_range(self.cursor, self.char_len(), DeleteDirection::Forward)
            }
        }
    }

    #[must_use]
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }

        self.text
            .char_indices()
            .nth(char_index)
            .map_or(self.text.len(), |(index, _)| index)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalChoice {
    Approve,
    Deny,
    AlwaysApprove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalOption {
    pub choice: ApprovalChoice,
    pub label: String,
}

impl ApprovalOption {
    #[must_use]
    pub fn new(choice: ApprovalChoice, label: impl Into<String>) -> Self {
        Self {
            choice,
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalModal {
    pub title: String,
    pub body: String,
    pub options: Vec<ApprovalOption>,
    pub selected: usize,
}

impl ApprovalModal {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        options: impl IntoIterator<Item = ApprovalOption>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            options: options.into_iter().collect(),
            selected: 0,
        }
    }

    #[must_use]
    pub fn with_selected(mut self, selected: usize) -> Self {
        if !self.options.is_empty() {
            self.selected = selected.min(self.options.len() - 1);
        }
        self
    }

    #[must_use]
    pub fn selected_choice(&self) -> Option<ApprovalChoice> {
        self.options.get(self.selected).map(|option| option.choice)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl SelectItem {
    #[must_use]
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<impl Into<String>>,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectListState {
    items: Vec<SelectItem>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
    max_visible: usize,
}

impl SelectListState {
    #[must_use]
    pub fn new(items: impl IntoIterator<Item = SelectItem>, max_visible: usize) -> Self {
        let items = items.into_iter().collect::<Vec<_>>();
        let filtered_indices = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            selected_index: 0,
            max_visible: max_visible.max(1),
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        let filter = filter.to_lowercase();
        self.filtered_indices = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                item.value
                    .to_lowercase()
                    .starts_with(&filter)
                    .then_some(index)
            })
            .collect();
        self.selected_index = 0;
    }

    #[must_use]
    pub fn filtered_len(&self) -> usize {
        self.filtered_indices.len()
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|index| self.items.get(*index))
    }

    pub fn move_up(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_index = 0;
        } else if self.selected_index == 0 {
            self.selected_index = len - 1;
        } else {
            self.selected_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_index = 0;
        } else {
            self.selected_index = (self.selected_index + 1) % len;
        }
    }

    #[must_use]
    pub fn visible_range(&self) -> Range<usize> {
        let len = self.filtered_len();
        if len == 0 {
            return 0..0;
        }

        let visible = self.max_visible.min(len);
        let half = visible / 2;
        let max_start = len.saturating_sub(visible);
        let start = self.selected_index.saturating_sub(half).min(max_start);
        start..start + visible
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        use crate::truncate_width;

        if self.filtered_indices.is_empty() {
            return vec![truncate_width("  No matching commands", width, "", false)];
        }

        let range = self.visible_range();
        let mut lines = Vec::new();
        for filtered_index in range.clone() {
            let Some(item) = self
                .filtered_indices
                .get(filtered_index)
                .and_then(|index| self.items.get(*index))
            else {
                continue;
            };
            lines.push(render_select_item(
                item,
                filtered_index == self.selected_index,
                width,
            ));
        }

        if range.start > 0 || range.end < self.filtered_len() {
            let info = format!("  ({}/{})", self.selected_index + 1, self.filtered_len());
            lines.push(truncate_width(&info, width, "", false));
        }

        lines
    }
}

fn render_select_item(item: &SelectItem, selected: bool, width: usize) -> String {
    use crate::{truncate_width, visible_width};

    let prefix = if selected { "> " } else { "  " };
    let label = if item.label.is_empty() {
        &item.value
    } else {
        &item.label
    };
    let prefix_width = visible_width(prefix);
    let description = item
        .description
        .as_deref()
        .map(|description| description.replace(['\r', '\n'], " ").trim().to_string())
        .filter(|description| !description.is_empty());

    if let Some(description) = description.filter(|_| width > 40) {
        let primary_width = 32usize.min(width.saturating_sub(prefix_width + 4)).max(1);
        let label = truncate_width(label, primary_width.saturating_sub(2).max(1), "", false);
        let spacing = " ".repeat(primary_width.saturating_sub(visible_width(&label)).max(1));
        let used = prefix_width + visible_width(&label) + spacing.len();
        let remaining = width.saturating_sub(used + 2);
        if remaining > 10 {
            let description = truncate_width(&description, remaining, "", false);
            return format!("{prefix}{label}{spacing}{description}");
        }
    }

    let max_label_width = width.saturating_sub(prefix_width + 2).max(1);
    format!(
        "{prefix}{}",
        truncate_width(label, max_label_width, "", false)
    )
}
