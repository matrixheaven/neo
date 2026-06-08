use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Widget},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    ApprovalModal, ChatTranscript, NeoTuiApp, Overlay, OverlayKind, PromptState, ToolStatus,
    ToolStatusKind, TranscriptItem, TranscriptView,
};

#[must_use]
pub fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut index = 0;
    while index < text.len() {
        if let Some(sequence) = ansi_escape_sequence(text, index) {
            index += sequence.len();
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };
        width += character.width().unwrap_or(0);
        index += character.len_utf8();
    }
    width
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
    let mut active_sgr = String::new();
    let mut index = 0;

    while index < text.len() {
        if let Some(sequence) = ansi_escape_sequence(text, index) {
            current.push_str(sequence);
            update_active_sgr(sequence, &mut active_sgr);
            index += sequence.len();
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };

        let character_width = character.width().unwrap_or(0);
        if current_width > 0 && current_width + character_width > max_width {
            lines.push(std::mem::take(&mut current));
            current.push_str(&active_sgr);
            current_width = 0;
        }

        current.push(character);
        current_width += character_width;
        index += character.len_utf8();
    }

    if !current.is_empty() {
        lines.push(current);
    }
}

fn update_active_sgr(sequence: &str, active_sgr: &mut String) {
    if !sequence.starts_with("\x1b[") || !sequence.ends_with('m') {
        return;
    }

    let action = sgr_style_action(sequence);
    if action.resets {
        active_sgr.clear();
    }
    if action.sets_style {
        active_sgr.push_str(sequence);
    }
}

struct SgrStyleAction {
    resets: bool,
    sets_style: bool,
}

fn sgr_style_action(sequence: &str) -> SgrStyleAction {
    let Some(parameters) = sequence
        .strip_prefix("\x1b[")
        .and_then(|sequence| sequence.strip_suffix('m'))
    else {
        return SgrStyleAction {
            resets: false,
            sets_style: false,
        };
    };

    let mut action = SgrStyleAction {
        resets: parameters.is_empty(),
        sets_style: false,
    };

    for parameter in parameters.split(';') {
        if parameter == "0" {
            action.resets = true;
        } else if !parameter.is_empty() {
            action.sets_style = true;
        }
    }

    action
}

pub struct TranscriptWidget<'a> {
    transcript: &'a ChatTranscript,
    view: Option<&'a TranscriptView>,
}

impl<'a> TranscriptWidget<'a> {
    #[must_use]
    pub const fn new(transcript: &'a ChatTranscript) -> Self {
        Self {
            transcript,
            view: None,
        }
    }

    #[must_use]
    pub const fn with_view(mut self, view: &'a TranscriptView) -> Self {
        self.view = Some(view);
        self
    }
}

impl Widget for TranscriptWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut y = area.y;
        let text_width = usize::from(area.width.saturating_sub(2).max(1));

        let items = self.transcript.items();
        let range = self.view.map_or(0..items.len(), |view| {
            view.visible_range(self.transcript, usize::from(area.height))
        });
        for item in &items[range] {
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

impl Widget for &NeoTuiApp {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let header = format!(
            "{} | session: {} | model: {} | {:?}",
            self.title(),
            self.session_label(),
            self.model_label(),
            self.mode()
        );
        write_line(
            area,
            buf,
            area.y,
            &header,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

        let prompt_height = prompt_height(self.prompt(), area.width);
        let status_height = self
            .tool_statuses()
            .len()
            .min(usize::from(area.height.saturating_sub(2)))
            .try_into()
            .unwrap_or(0);
        let body_y = area.y.saturating_add(1);
        let footer_height = prompt_height.saturating_add(status_height);
        let body_height = area.height.saturating_sub(1).saturating_sub(footer_height);

        let body = Rect {
            x: area.x,
            y: body_y,
            width: area.width,
            height: body_height,
        };
        TranscriptWidget::new(self.transcript())
            .with_view(self.transcript_view())
            .render(body, buf);

        let status_y = body.y.saturating_add(body.height);
        let statuses = self.tool_statuses();
        if status_height > 0 {
            StatusWidget::new(&statuses).render(
                Rect {
                    x: area.x,
                    y: status_y,
                    width: area.width,
                    height: status_height,
                },
                buf,
            );
        }

        PromptWidget::new(self.prompt()).render(
            Rect {
                x: area.x,
                y: status_y.saturating_add(status_height),
                width: area.width,
                height: prompt_height,
            },
            buf,
        );

        if let Some(overlay) = self.focused_overlay() {
            render_overlay(overlay, area, buf);
        }
    }
}

fn prompt_height(prompt: &PromptState, width: u16) -> u16 {
    let display_width = usize::from(width.max(1));
    let lines = wrap_width(&format!("> {}", prompt.text), display_width)
        .len()
        .max(1);
    u16::try_from(lines).unwrap_or(u16::MAX)
}

fn render_overlay(overlay: &Overlay, area: Rect, buf: &mut Buffer) {
    let width = area.width.saturating_sub(4).clamp(20, 56);
    let lines = overlay_lines(overlay, usize::from(width.saturating_sub(2).max(1)));
    let content_height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let height = content_height.saturating_add(2).min(area.height).max(3);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let overlay_area = Rect {
        x,
        y,
        width,
        height,
    };

    if let OverlayKind::Approval(request) = &overlay.kind {
        request.modal.clone().render(overlay_area, buf);
        return;
    }

    Clear.render(overlay_area, buf);
    let title = overlay_title(overlay);
    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(overlay_area);
    block.render(overlay_area, buf);

    for (index, line) in lines.iter().enumerate().take(usize::from(inner.height)) {
        let Ok(row) = u16::try_from(index) else {
            break;
        };
        write_line(inner, buf, inner.y + row, line, Style::default());
    }
}

fn overlay_title(overlay: &Overlay) -> &str {
    match overlay.kind {
        OverlayKind::CommandPalette(_) => "Command Palette",
        OverlayKind::SessionPicker(_) => "Sessions",
        OverlayKind::ModelPicker(_) => "Models",
        OverlayKind::PromptCompletion(_) => "Completions",
        OverlayKind::Approval(_) => "Approval",
        OverlayKind::Message(_) => overlay.title.as_str(),
    }
}

fn overlay_lines(overlay: &Overlay, width: usize) -> Vec<String> {
    match &overlay.kind {
        OverlayKind::CommandPalette(state) => state.render_lines(width),
        OverlayKind::SessionPicker(state) | OverlayKind::ModelPicker(state) => {
            state.render_lines(width)
        }
        OverlayKind::PromptCompletion(state) => state.render_lines(width),
        OverlayKind::Approval(request) => wrap_width(&request.modal.body, width),
        OverlayKind::Message(message) => wrap_width(message, width),
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
    let mut index = 0;

    while index < text.len() {
        if let Some(sequence) = ansi_escape_sequence(text, index) {
            clipped.push_str(sequence);
            index += sequence.len();
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };

        let character_width = character.width().unwrap_or(0);
        if width + character_width > max_width {
            break;
        }
        clipped.push(character);
        width += character_width;
        index += character.len_utf8();
    }

    clipped
}

fn ansi_escape_sequence(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.get(start).copied()? != 0x1b {
        return None;
    }
    let introducer = *bytes.get(start + 1)?;
    match introducer {
        b'[' => csi_sequence(text, start),
        b']' | b'P' | b'_' | b'^' | b'X' => string_escape_sequence(text, start),
        b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/' => fixed_escape_sequence(text, start, 3),
        0x40..=0x5f => fixed_escape_sequence(text, start, 2),
        _ => None,
    }
}

fn csi_sequence(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut index = start + 2;
    while index < bytes.len() {
        if (0x40..=0x7e).contains(&bytes[index]) {
            return text.get(start..index + 1);
        }
        index += 1;
    }
    None
}

fn string_escape_sequence(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut index = start + 2;
    while index < bytes.len() {
        match bytes[index] {
            0x07 => return text.get(start..index + 1),
            0x1b if bytes.get(index + 1).copied() == Some(b'\\') => {
                return text.get(start..index + 2);
            }
            _ => index += 1,
        }
    }
    None
}

fn fixed_escape_sequence(text: &str, start: usize, byte_len: usize) -> Option<&str> {
    text.get(start..start + byte_len)
}
