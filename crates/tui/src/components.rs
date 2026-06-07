use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    ApprovalModal, ChatTranscript, PromptState, ToolStatus, ToolStatusKind, TranscriptItem,
};

#[must_use]
pub fn visible_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

#[must_use]
pub fn truncate_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    let text_width = visible_width(text);
    if text_width <= max_width {
        if pad {
            let mut padded = text.to_string();
            padded.push_str(&" ".repeat(max_width - text_width));
            return padded;
        }
        return text.to_string();
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let clipped = clip_width(ellipsis, max_width);
        if pad {
            let clipped_width = visible_width(&clipped);
            return format!("{clipped}{}", " ".repeat(max_width - clipped_width));
        }
        return clipped;
    }

    let prefix_width = max_width - ellipsis_width;
    let prefix = clip_width(text, prefix_width);
    let mut truncated = format!("{prefix}{ellipsis}");
    if pad {
        let truncated_width = visible_width(&truncated);
        truncated.push_str(&" ".repeat(max_width - truncated_width));
    }
    truncated
}

#[must_use]
pub fn wrap_width(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for logical_line in text.split('\n') {
        if logical_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        wrap_single_line(logical_line, max_width, &mut lines);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_single_line(text: &str, max_width: usize, lines: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_width = 0;

    for character in text.chars() {
        let character_width = character.width().unwrap_or(0);
        if current_width > 0 && current_width + character_width > max_width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }

        current.push(character);
        current_width += character_width;

        if current_width >= max_width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
}

pub struct TranscriptWidget<'a> {
    transcript: &'a ChatTranscript,
}

impl<'a> TranscriptWidget<'a> {
    #[must_use]
    pub const fn new(transcript: &'a ChatTranscript) -> Self {
        Self { transcript }
    }
}

impl Widget for TranscriptWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut y = area.y;
        let text_width = usize::from(area.width.saturating_sub(2).max(1));

        for item in self.transcript.items() {
            if y >= area.bottom() {
                break;
            }

            let (label, content, style) = transcript_row(item);
            write_line(area, buf, y, label, style.add_modifier(Modifier::BOLD));
            y = y.saturating_add(1);

            for line in wrap_width(&content, text_width) {
                if y >= area.bottom() {
                    break;
                }
                write_line(area, buf, y, &format!("  {line}"), style);
                y = y.saturating_add(1);
            }
        }
    }
}

fn transcript_row(item: &TranscriptItem) -> (&'static str, String, Style) {
    match item {
        TranscriptItem::User { content } => {
            ("You", content.clone(), Style::default().fg(Color::Cyan))
        }
        TranscriptItem::Assistant { content } => (
            "Assistant",
            content.clone(),
            Style::default().fg(Color::Green),
        ),
        TranscriptItem::Tool {
            name,
            detail,
            status,
        } => (
            "Tool",
            format!("{} {} ({})", status.marker(), name, detail),
            status_style(*status),
        ),
        TranscriptItem::Notice { content } => {
            ("Notice", content.clone(), Style::default().fg(Color::Gray))
        }
    }
}

pub struct StatusWidget<'a> {
    statuses: &'a [ToolStatus],
}

impl<'a> StatusWidget<'a> {
    #[must_use]
    pub const fn new(statuses: &'a [ToolStatus]) -> Self {
        Self { statuses }
    }
}

impl Widget for StatusWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (index, status) in self
            .statuses
            .iter()
            .enumerate()
            .take(usize::from(area.height))
        {
            let Ok(row) = u16::try_from(index) else {
                break;
            };
            let detail = status.detail.as_deref().unwrap_or("");
            let separator = if detail.is_empty() { "" } else { " - " };
            let line = format!(
                "{} {} {}{}{}",
                status.kind.marker(),
                status.name,
                status.kind.label(),
                separator,
                detail
            );
            write_line(area, buf, area.y + row, &line, status_style(status.kind));
        }
    }
}

pub struct PromptWidget<'a> {
    prompt: &'a PromptState,
}

impl<'a> PromptWidget<'a> {
    #[must_use]
    pub const fn new(prompt: &'a PromptState) -> Self {
        Self { prompt }
    }
}

impl Widget for PromptWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut display = String::from("> ");
        for (index, character) in self.prompt.text.chars().enumerate() {
            if index == self.prompt.cursor {
                display.push('▏');
            }
            display.push(character);
        }
        if self.prompt.cursor >= self.prompt.text.chars().count() {
            display.push('▏');
        }

        let width = usize::from(area.width.max(1));
        for (row, line) in wrap_width(&display, width)
            .into_iter()
            .enumerate()
            .take(usize::from(area.height))
        {
            let Ok(row) = u16::try_from(row) else {
                break;
            };
            write_line(
                area,
                buf,
                area.y + row,
                &line,
                Style::default().fg(Color::White),
            );
        }
    }
}

impl Widget for ApprovalModal {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let block = Block::default()
            .title(self.title.as_str())
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let inner = block.inner(area);
        block.render(area, buf);

        let text_width = usize::from(inner.width.saturating_sub(2).max(1));
        let mut y = inner.y;
        for line in wrap_width(&self.body, text_width) {
            if y >= inner.bottom() {
                return;
            }
            write_line(inner, buf, y, &line, Style::default());
            y = y.saturating_add(1);
        }

        y = y.saturating_add(1);
        for (index, option) in self.options.iter().enumerate() {
            if y >= inner.bottom() {
                break;
            }
            let marker = if index == self.selected { ">" } else { " " };
            let style = if index == self.selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            };
            write_line(inner, buf, y, &format!("{marker} {}", option.label), style);
            y = y.saturating_add(1);
        }
    }
}

fn status_style(kind: ToolStatusKind) -> Style {
    match kind {
        ToolStatusKind::Pending => Style::default().fg(Color::Gray),
        ToolStatusKind::Running => Style::default().fg(Color::Yellow),
        ToolStatusKind::Succeeded => Style::default().fg(Color::Green),
        ToolStatusKind::Failed => Style::default().fg(Color::Red),
        ToolStatusKind::Cancelled => Style::default().fg(Color::DarkGray),
    }
}

fn write_line(area: Rect, buf: &mut Buffer, y: u16, text: &str, style: Style) {
    if area.width == 0 || y >= area.bottom() {
        return;
    }

    let clipped = clip_width(text, usize::from(area.width));
    buf.set_string(area.x, y, clipped, style);
}

fn clip_width(text: &str, max_width: usize) -> String {
    let mut clipped = String::new();
    let mut width = 0;

    for character in text.chars() {
        let character_width = character.width().unwrap_or(0);
        if width + character_width > max_width {
            break;
        }
        clipped.push(character);
        width += character_width;
    }

    clipped
}
