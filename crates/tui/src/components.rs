use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Widget},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    ApprovalModal, ChatTranscript, NeoTuiApp, Overlay, OverlayKind, PromptState, ToolStatus,
    ToolStatusKind, TranscriptItem, TranscriptLine, TranscriptRenderer, TranscriptSelection,
    TranscriptView, TuiTheme,
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
    selection: Option<&'a TranscriptSelection>,
    theme: TuiTheme,
}

impl<'a> TranscriptWidget<'a> {
    #[must_use]
    pub fn new(transcript: &'a ChatTranscript) -> Self {
        Self {
            transcript,
            view: None,
            selection: None,
            theme: TuiTheme::default(),
        }
    }

    #[must_use]
    pub const fn with_view(mut self, view: &'a TranscriptView) -> Self {
        self.view = Some(view);
        self
    }

    #[must_use]
    pub const fn with_selection(mut self, selection: Option<&'a TranscriptSelection>) -> Self {
        self.selection = selection;
        self
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
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
        let selected_range = self
            .selection
            .and_then(|selection| selection.range(self.transcript));
        for (index, item) in items[range.clone()].iter().enumerate() {
            if y >= area.bottom() {
                break;
            }

            let item_index = range.start + index;
            let selected = selected_range
                .as_ref()
                .is_some_and(|range| range.contains(&item_index));
            let (label, content, style) = transcript_row(item, self.theme);
            let style = selected_style(style, selected, self.theme);
            write_line(area, buf, y, label, style.add_modifier(Modifier::BOLD));
            y = y.saturating_add(1);

            for line in TranscriptRenderer::new(text_width).render_markdownish(&content) {
                if y >= area.bottom() {
                    break;
                }
                write_line(
                    area,
                    buf,
                    y,
                    &format!("  {}", line.display_text()),
                    selected_style(
                        transcript_line_style(&line, style, self.theme),
                        selected,
                        self.theme,
                    ),
                );
                y = y.saturating_add(1);
            }
        }
    }
}

fn selected_style(style: Style, selected: bool, theme: TuiTheme) -> Style {
    if selected {
        style.bg(theme.selection_bg)
    } else {
        style
    }
}

fn transcript_row(item: &TranscriptItem, theme: TuiTheme) -> (&'static str, String, Style) {
    match item {
        TranscriptItem::User { content } => {
            ("You", content.clone(), Style::default().fg(theme.user))
        }
        TranscriptItem::Assistant { content } => (
            "Assistant",
            content.clone(),
            Style::default().fg(theme.assistant),
        ),
        TranscriptItem::Tool {
            name,
            detail,
            status,
        } => (
            "Tool",
            format!("{} {} ({})", status.marker(), name, detail),
            status_style(*status, theme),
        ),
        TranscriptItem::Notice { content } => {
            ("Notice", content.clone(), Style::default().fg(theme.notice))
        }
    }
}

fn transcript_line_style(line: &TranscriptLine, base: Style, theme: TuiTheme) -> Style {
    match line {
        TranscriptLine::DiffFileHeader { marker: '+', .. } | TranscriptLine::DiffAdded { .. } => {
            Style::default().fg(theme.diff_added)
        }
        TranscriptLine::DiffFileHeader { marker: '-', .. } | TranscriptLine::DiffRemoved { .. } => {
            Style::default().fg(theme.diff_removed)
        }
        TranscriptLine::DiffHunk { .. } => Style::default()
            .fg(theme.diff_hunk)
            .add_modifier(Modifier::BOLD),
        TranscriptLine::DiffContext { .. } => Style::default().fg(theme.diff_context),
        _ => base,
    }
}

pub struct StatusWidget<'a> {
    statuses: &'a [ToolStatus],
    theme: TuiTheme,
}

impl<'a> StatusWidget<'a> {
    #[must_use]
    pub fn new(statuses: &'a [ToolStatus]) -> Self {
        Self {
            statuses,
            theme: TuiTheme::default(),
        }
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
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
            write_line(
                area,
                buf,
                area.y + row,
                &line,
                status_style(status.kind, self.theme),
            );
        }
    }
}

pub struct PromptWidget<'a> {
    prompt: &'a PromptState,
    theme: TuiTheme,
}

impl<'a> PromptWidget<'a> {
    #[must_use]
    pub fn new(prompt: &'a PromptState) -> Self {
        Self {
            prompt,
            theme: TuiTheme::default(),
        }
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
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
                Style::default().fg(self.theme.prompt),
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
                .fg(self.theme().header)
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
            .with_selection(self.transcript_selection())
            .with_theme(self.theme())
            .render(body, buf);

        let status_y = body.y.saturating_add(body.height);
        let statuses = self.tool_statuses();
        if status_height > 0 {
            StatusWidget::new(&statuses)
                .with_theme(self.theme())
                .render(
                    Rect {
                        x: area.x,
                        y: status_y,
                        width: area.width,
                        height: status_height,
                    },
                    buf,
                );
        }

        PromptWidget::new(self.prompt())
            .with_theme(self.theme())
            .render(
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

fn status_style(kind: ToolStatusKind, theme: TuiTheme) -> Style {
    match kind {
        ToolStatusKind::Pending => Style::default().fg(theme.pending),
        ToolStatusKind::Running => Style::default().fg(theme.running),
        ToolStatusKind::Succeeded => Style::default().fg(theme.succeeded),
        ToolStatusKind::Failed => Style::default().fg(theme.failed),
        ToolStatusKind::Cancelled => Style::default().fg(theme.cancelled),
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
