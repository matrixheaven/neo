use crate::chrome::ToolStatusKind;
use crate::chrome::TuiTheme;
use crate::core::Line;
use crate::transcript::{ToolCallComponent, ToolCallState};

use super::entry::{ApprovalPromptData, ThinkingPhase, TranscriptEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptSelection {
    start: usize,
    end: usize,
}

impl TranscriptSelection {
    #[must_use]
    pub const fn new(index: usize) -> Self {
        Self {
            start: index,
            end: index,
        }
    }

    fn extend_up(&mut self, len: usize, count: usize) {
        let max_index = len.saturating_sub(1);
        self.start = self.start.saturating_sub(count).min(max_index);
        self.end = self.end.min(max_index);
    }

    fn extend_down(&mut self, len: usize, count: usize) {
        let max_index = len.saturating_sub(1);
        self.start = self.start.min(max_index);
        self.end = self.end.saturating_add(count).min(max_index);
    }

    #[must_use]
    fn range(&self, len: usize) -> Option<std::ops::Range<usize>> {
        if len == 0 {
            return None;
        }
        let max_index = len - 1;
        let start = self.start.min(max_index).min(self.end.min(max_index));
        let end = self.start.min(max_index).max(self.end.min(max_index)) + 1;
        Some(start..end)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptViewport {
    scroll_offset_rows: usize,
    content_rows: usize,
    viewport_rows: usize,
    follow_tail: bool,
    selection: Option<TranscriptSelection>,
}

impl TranscriptViewport {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            scroll_offset_rows: 0,
            content_rows: 0,
            viewport_rows: 0,
            follow_tail: true,
            selection: None,
        }
    }

    #[must_use]
    pub const fn selection(&self) -> Option<&TranscriptSelection> {
        self.selection.as_ref()
    }

    #[must_use]
    pub const fn scrollback(&self) -> usize {
        self.scroll_offset_rows
    }

    #[must_use]
    pub const fn is_following_tail(&self) -> bool {
        self.follow_tail
    }

    pub fn follow_bottom(&mut self) {
        self.scroll_offset_rows = 0;
        self.follow_tail = true;
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.follow_tail = false;
        self.scroll_offset_rows = self.scroll_offset_rows.saturating_add(rows);
        if self.has_synced_dimensions() {
            self.scroll_offset_rows = self.scroll_offset_rows.min(self.max_scroll_offset());
        }
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.scroll_offset_rows = self.scroll_offset_rows.saturating_sub(rows);
        if self.scroll_offset_rows == 0 {
            self.follow_tail = true;
        }
    }

    pub fn sync(&mut self, content_rows: usize, viewport_rows: usize) {
        self.content_rows = content_rows;
        self.viewport_rows = viewport_rows;
        if self.follow_tail {
            self.scroll_offset_rows = 0;
        } else {
            self.scroll_offset_rows = self.scroll_offset_rows.min(self.max_scroll_offset());
        }
    }

    #[must_use]
    pub fn visible_row_range(&self, row_count: usize, height: usize) -> std::ops::Range<usize> {
        if height == 0 || row_count == 0 {
            return 0..0;
        }
        let window = height.min(row_count);
        let scrollback = self
            .scroll_offset_rows
            .min(row_count.saturating_sub(window));
        let bottom = row_count.saturating_sub(scrollback).max(window);
        bottom - window..bottom
    }

    fn has_synced_dimensions(&self) -> bool {
        self.viewport_rows > 0
    }

    fn max_scroll_offset(&self) -> usize {
        self.content_rows.saturating_sub(self.viewport_rows)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TranscriptStore {
    entries: Vec<TranscriptEntry>,
    active_assistant: Option<usize>,
    active_thinking: Option<usize>,
    viewport: TranscriptViewport,
}

impl TranscriptStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: TranscriptEntry) {
        self.entries.push(entry);
        self.viewport.follow_bottom();
    }

    pub fn start_assistant(&mut self) {
        if self.active_assistant.is_some() {
            return;
        }
        self.entries.push(TranscriptEntry::assistant_message(""));
        self.active_assistant = Some(self.entries.len() - 1);
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        self.start_assistant();
        let Some(index) = self.active_assistant else {
            return;
        };
        if let Some(TranscriptEntry::AssistantMessage { content }) = self.entries.get_mut(index) {
            content.push_str(text);
        }
    }

    pub fn finish_assistant(&mut self) {
        self.active_assistant = None;
    }

    pub fn start_thinking(&mut self) {
        if self.active_thinking.is_some() {
            return;
        }
        self.entries
            .push(TranscriptEntry::thinking_streaming(String::new()));
        self.active_thinking = Some(self.entries.len() - 1);
    }

    pub fn append_thinking_delta(&mut self, text: &str) {
        self.start_thinking();
        let Some(index) = self.active_thinking else {
            return;
        };
        if let Some(TranscriptEntry::ThinkingBlock { content, .. }) = self.entries.get_mut(index) {
            content.push_str(text);
        }
    }

    pub fn finish_thinking(&mut self) {
        if let Some(index) = self.active_thinking.take()
            && let Some(TranscriptEntry::ThinkingBlock { phase, .. }) = self.entries.get_mut(index)
        {
            *phase = ThinkingPhase::Complete;
        }
    }

    pub fn push_tool_run(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Option<String>,
    ) {
        let component = ToolCallComponent::new(ToolCallState {
            id: id.into(),
            name: name.into(),
            arguments,
            result: None,
            details: None,
            status: ToolStatusKind::Running,
            exit_code: None,
        });
        self.push(TranscriptEntry::tool_run(component));
    }

    pub fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCallComponent> {
        self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::ToolRun { component } if component.id() == id => Some(component),
            _ => None,
        })
    }

    pub fn approval_mut(&mut self, id: &str) -> Option<&mut ApprovalPromptData> {
        self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::ApprovalPrompt(data) if data.id == id => Some(data),
            _ => None,
        })
    }

    pub fn insert_approval_after_tool_or_push(&mut self, data: ApprovalPromptData) {
        let insert_at = self
            .entries
            .iter()
            .rposition(
                |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == data.id),
            )
            .map(|index| index + 1);
        if let Some(index) = insert_at {
            self.entries
                .insert(index, TranscriptEntry::ApprovalPrompt(data));
            self.viewport.follow_bottom();
        } else {
            self.push(TranscriptEntry::ApprovalPrompt(data));
        }
    }

    #[must_use]
    pub fn has_tool(&self, id: &str) -> bool {
        self.entries.iter().any(
            |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == id),
        )
    }

    #[must_use]
    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [TranscriptEntry] {
        &mut self.entries
    }

    /// Remove the entry at `index`, shifting later entries down. Returns the
    /// removed entry. Used to pop a queued follow-up when it is promoted to a
    /// steer.
    pub fn remove(&mut self, index: usize) -> Option<TranscriptEntry> {
        if index >= self.entries.len() {
            return None;
        }
        let entry = self.entries.remove(index);
        self.viewport.follow_bottom();
        Some(entry)
    }

    #[must_use]
    pub const fn viewport(&self) -> &TranscriptViewport {
        &self.viewport
    }

    pub fn viewport_mut(&mut self) -> &mut TranscriptViewport {
        &mut self.viewport
    }

    pub fn select_visible_entry(&mut self) {
        let range = self.viewport.visible_row_range(self.entries.len(), 1);
        let Some(index) = range.end.checked_sub(1) else {
            self.viewport.selection = None;
            return;
        };
        self.viewport.selection =
            (index < self.entries.len()).then(|| TranscriptSelection::new(index));
    }

    pub fn clear_selection(&mut self) {
        self.viewport.selection = None;
    }

    pub fn extend_selection_up(&mut self, count: usize) {
        if self.viewport.selection.is_none() {
            self.select_visible_entry();
        }
        if let Some(selection) = &mut self.viewport.selection {
            selection.extend_up(self.entries.len(), count);
        }
    }

    pub fn extend_selection_down(&mut self, count: usize) {
        if self.viewport.selection.is_none() {
            self.select_visible_entry();
        }
        if let Some(selection) = &mut self.viewport.selection {
            selection.extend_down(self.entries.len(), count);
        }
    }

    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.viewport.selection.is_some()
    }

    #[must_use]
    pub fn copy_selection(&self) -> Option<String> {
        let range = self.viewport.selection?.range(self.entries.len())?;
        let mut copied = String::new();
        for (offset, entry) in self.entries[range].iter().enumerate() {
            if offset > 0 {
                copied.push_str("\n\n");
            }
            let (label, content) = entry.copy_parts();
            copied.push_str(label);
            copied.push('\n');
            copied.push_str(&content);
        }
        Some(copied)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn render_rows(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        self.entries
            .iter()
            .flat_map(|entry| entry.render(width, theme))
            .collect()
    }
}
