use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};
use std::collections::BTreeSet;
use unicode_width::UnicodeWidthChar;

use crate::{
    ApprovalModal, ChatTranscript, DiffLine, DiffModel, NeoTuiApp, Overlay, OverlayKind,
    PromptState, ToolRunTranscript, ToolStatus, ToolStatusKind, TranscriptItem, TranscriptLine,
    TranscriptRenderer, TranscriptSelection, TranscriptView, TuiTheme,
};

const COMPACTION_BAR_WIDTH: usize = 30;
const TOOL_PREVIEW_LINES: usize = 4;
const DIFF_PREVIEW_LINES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppLayout {
    pub body: Rect,
    pub status: Rect,
    pub approval: Rect,
    pub session_picker: Rect,
    pub prompt: Rect,
    pub footer: Rect,
}

#[must_use]
pub fn app_layout(app: &NeoTuiApp, area: Rect) -> AppLayout {
    let prompt_height = prompt_height(app.prompt(), area.width);
    let footer_bar_height = if area.height >= 12 {
        2
    } else if area.height >= 8 {
        1
    } else {
        0
    };
    let session_picker_height = match app.focused_overlay().map(|overlay| &overlay.kind) {
        Some(OverlayKind::SessionPicker(_)) => 16,
        _ => 0,
    }
    .min(area.height.saturating_sub(3));
    let approval_overlay = match app.focused_overlay().map(|overlay| &overlay.kind) {
        Some(OverlayKind::Approval(request)) => Some(request),
        _ => None,
    };
    let approval_height = approval_overlay
        .map(|request| approval_panel_height(&request.modal, area.width))
        .unwrap_or(0)
        .min(area.height.saturating_sub(3));
    let bottom_height = prompt_height
        .saturating_add(footer_bar_height)
        .saturating_add(session_picker_height)
        .saturating_add(approval_height);
    let body_y = area.y;
    let body_height = area.height.saturating_sub(bottom_height);
    let body = Rect {
        x: area.x,
        y: body_y,
        width: area.width,
        height: body_height,
    };
    let status = Rect {
        x: area.x,
        y: body.y.saturating_add(body.height),
        width: area.width,
        height: 0,
    };
    let approval = Rect {
        x: area.x,
        y: status.y.saturating_add(status.height),
        width: area.width,
        height: approval_height,
    };
    let session_picker = Rect {
        x: area.x,
        y: approval.y.saturating_add(approval.height),
        width: area.width,
        height: session_picker_height,
    };
    let prompt = Rect {
        x: area.x,
        y: session_picker.y.saturating_add(session_picker.height),
        width: area.width,
        height: prompt_height,
    };
    let footer = Rect {
        x: area.x,
        y: prompt.y.saturating_add(prompt.height),
        width: area.width,
        height: footer_bar_height,
    };

    AppLayout {
        body,
        status,
        approval,
        session_picker,
        prompt,
        footer,
    }
}

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
    expanded_items: Option<&'a BTreeSet<usize>>,
    activity_frame: usize,
    theme: TuiTheme,
}

impl<'a> TranscriptWidget<'a> {
    #[must_use]
    pub fn new(transcript: &'a ChatTranscript) -> Self {
        Self {
            transcript,
            view: None,
            selection: None,
            expanded_items: None,
            activity_frame: 0,
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
    pub const fn with_expanded_items(mut self, expanded_items: &'a BTreeSet<usize>) -> Self {
        self.expanded_items = Some(expanded_items);
        self
    }

    #[must_use]
    pub const fn with_activity_frame(mut self, activity_frame: usize) -> Self {
        self.activity_frame = activity_frame;
        self
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
    }

    #[must_use]
    pub fn row_count(&self, width: u16) -> usize {
        transcript_render_rows(
            self.transcript,
            self.selection,
            self.expanded_items,
            self.activity_frame,
            self.theme,
            usize::from(width.saturating_sub(2).max(1)),
        )
        .len()
    }
}

impl Widget for TranscriptWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let text_width = usize::from(area.width.saturating_sub(2).max(1));
        let rows = transcript_render_rows(
            self.transcript,
            self.selection,
            self.expanded_items,
            self.activity_frame,
            self.theme,
            text_width,
        );

        let range = self.view.map_or_else(
            || bottom_row_range(rows.len(), usize::from(area.height)),
            |view| view.visible_row_range(rows.len(), usize::from(area.height)),
        );
        let mut y = area.y;
        for row in &rows[range] {
            if y >= area.bottom() {
                break;
            }
            if let Some(fill) = row.fill {
                fill_line(area, buf, y, fill);
            }
            write_line(area, buf, y, &row.text, row.style);
            y = y.saturating_add(1);
        }
    }
}

fn transcript_render_rows(
    transcript: &ChatTranscript,
    selection: Option<&TranscriptSelection>,
    expanded_items: Option<&BTreeSet<usize>>,
    activity_frame: usize,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<TranscriptRenderRow> {
    let items = transcript.items();
    let selected_range = selection.and_then(|selection| selection.range(transcript));

    let mut rows = Vec::new();
    for (item_index, item) in items.iter().enumerate() {
        if item_index > 0 {
            rows.push(TranscriptRenderRow::blank());
        }

        let selected = selected_range
            .as_ref()
            .is_some_and(|range| range.contains(&item_index));
        let expanded = expanded_items.is_some_and(|expanded| expanded.contains(&item_index));
        if let TranscriptItem::Tool { tool_run, .. } = item {
            rows.extend(tool_render_rows(
                tool_run,
                expanded,
                selected,
                activity_frame,
                theme,
                text_width,
            ));
            continue;
        }

        if let TranscriptItem::Banner {
            title,
            session_label,
            model_label,
            workspace_root,
        } = item
        {
            rows.extend(banner_render_rows(
                title,
                session_label,
                model_label,
                workspace_root,
                text_width,
                selected,
                theme,
            ));
            continue;
        }

        let (label, content, style) = transcript_row(item, theme, expanded);
        let style = selected_style(style, selected, theme);
        let fill = matches!(item, TranscriptItem::User { .. })
            .then_some(Style::default().bg(theme.user_bg));
        rows.push(TranscriptRenderRow::new(
            label,
            style.add_modifier(Modifier::BOLD),
            fill,
        ));

        if let TranscriptItem::Assistant { thinking, content } = item {
            if let Some(thinking) = thinking.as_deref().filter(|thinking| !thinking.is_empty()) {
                for line in wrap_width(thinking, text_width) {
                    rows.push(TranscriptRenderRow::new(
                        format!("  {line}"),
                        selected_style(
                            Style::default()
                                .fg(theme.thinking)
                                .add_modifier(Modifier::ITALIC),
                            selected,
                            theme,
                        ),
                        None,
                    ));
                }
                if !content.is_empty() {
                    rows.push(TranscriptRenderRow::blank());
                }
            }

            for line in TranscriptRenderer::new(text_width).render_markdownish(content) {
                rows.push(TranscriptRenderRow::new(
                    format!("  {}", line.display_text()),
                    selected_style(transcript_line_style(&line, style, theme), selected, theme),
                    None,
                ));
            }
        } else if let TranscriptItem::Compaction {
            phase,
            percent,
            compacted_message_count,
            tokens_before,
        } = item
        {
            let compact_rows = compaction_render_rows(
                *phase,
                *percent,
                *compacted_message_count,
                *tokens_before,
                text_width,
                selected,
                theme,
            );
            rows.extend(compact_rows);
        } else {
            for line in TranscriptRenderer::new(text_width).render_markdownish(&content) {
                rows.push(TranscriptRenderRow::new(
                    format!("  {}", line.display_text()),
                    selected_style(transcript_line_style(&line, style, theme), selected, theme),
                    None,
                ));
            }
        }
    }

    rows
}

fn compaction_render_rows(
    phase: Option<neo_agent_core::CompactionPhase>,
    percent: u8,
    compacted_message_count: usize,
    tokens_before: usize,
    text_width: usize,
    selected: bool,
    theme: TuiTheme,
) -> Vec<TranscriptRenderRow> {
    let bar_width = COMPACTION_BAR_WIDTH.min(text_width.saturating_sub(7).max(8));
    let percent = percent.min(100);
    let filled = bar_width.saturating_mul(usize::from(percent)) / 100;
    let bar = format!(
        "[{}{}] {percent}%",
        "#".repeat(filled),
        ".".repeat(bar_width.saturating_sub(filled))
    );
    let summary = format!(
        "Compacted {} messages · {} tokens before",
        compacted_message_count,
        format_token_count(tokens_before)
    );
    let tip = "Tip: Use /compact after a long task to keep Neo focused.";
    let muted = selected_style(Style::default().fg(theme.notice), selected, theme);
    let progress = selected_style(Style::default().fg(theme.accent), selected, theme);

    let mut rows = vec![
        TranscriptRenderRow::new("  Compacting conversation...", muted, None),
        TranscriptRenderRow::new(format!("  {bar}"), progress, None),
        TranscriptRenderRow::new(format!("  {}", compaction_phase_label(phase)), muted, None),
    ];
    for line in wrap_width(&summary, text_width) {
        rows.push(TranscriptRenderRow::new(format!("  {line}"), muted, None));
    }
    for line in wrap_width(tip, text_width) {
        rows.push(TranscriptRenderRow::new(format!("  {line}"), muted, None));
    }
    rows
}

fn compaction_phase_label(phase: Option<neo_agent_core::CompactionPhase>) -> &'static str {
    match phase {
        Some(neo_agent_core::CompactionPhase::Estimating) => "Estimating context size",
        Some(neo_agent_core::CompactionPhase::SelectingBoundary) => {
            "Selecting safe compaction boundary"
        }
        Some(neo_agent_core::CompactionPhase::Summarizing) => "Summarizing older context",
        Some(neo_agent_core::CompactionPhase::Applying) => "Applying compacted context",
        None => "Preparing compaction",
    }
}

fn banner_render_rows(
    title: &str,
    session_label: &str,
    model_label: &str,
    workspace_root: &std::path::Path,
    text_width: usize,
    selected: bool,
    theme: TuiTheme,
) -> Vec<TranscriptRenderRow> {
    let border_style = selected_style(Style::default().fg(theme.surface_border), selected, theme);
    let header_style = selected_style(
        Style::default()
            .fg(theme.header)
            .add_modifier(Modifier::BOLD),
        selected,
        theme,
    );
    let muted_style = selected_style(Style::default().fg(theme.muted), selected, theme);

    let inner_width = text_width.saturating_sub(2).max(1);
    let top = format!("┌{}┐", "─".repeat(inner_width));
    let bottom = format!("└{}┘", "─".repeat(inner_width));

    vec![
        TranscriptRenderRow::new(top, border_style, None),
        TranscriptRenderRow::new(
            banner_box_line('│', &format!("  {title}"), '│', inner_width),
            header_style,
            None,
        ),
        TranscriptRenderRow::new(
            banner_box_line(
                '│',
                &format!("  Session: {session_label}"),
                '│',
                inner_width,
            ),
            muted_style,
            None,
        ),
        TranscriptRenderRow::new(
            banner_box_line('│', &format!("  Model: {model_label}"), '│', inner_width),
            muted_style,
            None,
        ),
        TranscriptRenderRow::new(
            banner_box_line(
                '│',
                &format!("  Workspace: {}", workspace_root.display()),
                '│',
                inner_width,
            ),
            muted_style,
            None,
        ),
        TranscriptRenderRow::new(bottom, border_style, None),
    ]
}

fn banner_box_line(left: char, content: &str, right: char, inner_width: usize) -> String {
    let content = clip_width(content, inner_width);
    let content_width = visible_width(&content);
    let padding = inner_width.saturating_sub(content_width);
    format!("{left}{content}{}{right}", " ".repeat(padding))
}

fn tool_render_rows(
    tool: &ToolRunTranscript,
    expanded: bool,
    selected: bool,
    activity_frame: usize,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<TranscriptRenderRow> {
    if let Some(diff) = tool.result.as_deref().and_then(DiffModel::parse_unified) {
        return diff_tool_render_rows(tool, &diff, expanded, selected, theme, text_width);
    }

    let header = format!(
        "{} Use {}{}",
        tool_status_symbol(tool.status, activity_frame),
        tool_call_label(tool),
        tool_status_suffix(tool.status)
    );
    let style = selected_style(status_style(tool.status, theme), selected, theme);
    let muted = selected_style(Style::default().fg(theme.notice), selected, theme);
    let mut rows = vec![TranscriptRenderRow::new(header, style, None)];
    let detail = tool.display_detail();
    if detail.is_empty() {
        return rows;
    }

    let detail_lines = detail.lines().collect::<Vec<_>>();
    let visible_count = if expanded {
        detail_lines.len()
    } else {
        detail_lines.len().min(TOOL_PREVIEW_LINES)
    };
    for line in detail_lines.iter().take(visible_count) {
        for wrapped in wrap_width(line, text_width.saturating_sub(4).max(1)) {
            rows.push(TranscriptRenderRow::new(
                format!("  └─ {wrapped}"),
                muted,
                None,
            ));
        }
    }
    if !expanded && detail_lines.len() > visible_count {
        rows.push(TranscriptRenderRow::new(
            format!(
                "     … {} more lines, ctrl+o expand",
                detail_lines.len() - visible_count
            ),
            muted,
            None,
        ));
    }
    rows
}

fn diff_tool_render_rows(
    tool: &ToolRunTranscript,
    diff: &DiffModel,
    expanded: bool,
    selected: bool,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<TranscriptRenderRow> {
    let stats = diff.stats();
    let file = diff.files().first();
    let path = file
        .map(|file| {
            if file.new_path.is_empty() {
                file.old_path.as_str()
            } else {
                file.new_path.as_str()
            }
        })
        .unwrap_or(tool.name.as_str());
    let header = format!("◌ Edited {path} +{} -{}", stats.added, stats.removed);
    let mut rows = vec![TranscriptRenderRow::new(
        header,
        selected_style(Style::default().fg(theme.diff_added), selected, theme),
        None,
    )];
    let Some(file) = file else {
        return rows;
    };

    let mut emitted = 0usize;
    for hunk in &file.hunks {
        rows.push(TranscriptRenderRow::new(
            format!("  {}", hunk.header),
            selected_style(
                Style::default()
                    .fg(theme.diff_hunk)
                    .add_modifier(Modifier::BOLD),
                selected,
                theme,
            ),
            None,
        ));
        let mut old_line = diff_old_start(&hunk.header).unwrap_or(1);
        let mut new_line = diff_new_start(&hunk.header).unwrap_or(old_line);
        for line in &hunk.lines {
            if !expanded && emitted >= DIFF_PREVIEW_LINES {
                rows.push(TranscriptRenderRow::new(
                    "     … more lines, ctrl+o expand",
                    selected_style(Style::default().fg(theme.notice), selected, theme),
                    None,
                ));
                return rows;
            }
            let (line_no, text, style) = match line {
                DiffLine::Context(text) => {
                    let current = new_line;
                    old_line += 1;
                    new_line += 1;
                    (
                        current,
                        format!(" {text}"),
                        Style::default().fg(theme.diff_context),
                    )
                }
                DiffLine::Removed(text) => {
                    let current = old_line;
                    old_line += 1;
                    (
                        current,
                        format!("-{text}"),
                        Style::default().fg(theme.diff_removed),
                    )
                }
                DiffLine::Added(text) => {
                    let current = new_line;
                    new_line += 1;
                    (
                        current,
                        format!("+{text}"),
                        Style::default().fg(theme.diff_added),
                    )
                }
            };
            rows.push(TranscriptRenderRow::new(
                fit_tool_line(&format!("  {line_no:<3}{text}"), text_width),
                selected_style(style, selected, theme),
                None,
            ));
            emitted += 1;
        }
    }
    rows
}

fn tool_call_label(tool: &ToolRunTranscript) -> String {
    let arguments = tool.arguments.as_deref().unwrap_or_default().trim();
    if arguments.is_empty() {
        return format!("{}()", tool.name);
    }
    format!("{}({})", tool.name, one_line(arguments))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tool_status_symbol(status: ToolStatusKind, activity_frame: usize) -> &'static str {
    match status {
        ToolStatusKind::Failed => "×",
        ToolStatusKind::Cancelled => "×",
        ToolStatusKind::Pending | ToolStatusKind::Running => {
            const FRAMES: [&str; 4] = ["·", "∙", "•", "∙"];
            FRAMES[activity_frame % FRAMES.len()]
        }
        ToolStatusKind::Succeeded => "●",
    }
}

fn tool_status_suffix(status: ToolStatusKind) -> &'static str {
    match status {
        ToolStatusKind::Pending => "      pending",
        ToolStatusKind::Running => "      running",
        _ => "",
    }
}

fn diff_old_start(header: &str) -> Option<usize> {
    diff_range_start(header, '-')
}

fn diff_new_start(header: &str) -> Option<usize> {
    diff_range_start(header, '+')
}

fn diff_range_start(header: &str, marker: char) -> Option<usize> {
    let marker_index = header.find(marker)?;
    let range = header.get(marker_index + 1..)?.split_whitespace().next()?;
    let start = range.split(',').next()?;
    start.parse().ok()
}

fn fit_tool_line(text: &str, width: usize) -> String {
    clip_width(text, width.max(1))
}

struct TranscriptRenderRow {
    text: String,
    style: Style,
    fill: Option<Style>,
}

impl TranscriptRenderRow {
    fn new(text: impl Into<String>, style: Style, fill: Option<Style>) -> Self {
        Self {
            text: text.into(),
            style,
            fill,
        }
    }

    fn blank() -> Self {
        Self::new("", Style::default(), None)
    }
}

fn bottom_row_range(row_count: usize, height: usize) -> std::ops::Range<usize> {
    if height == 0 || row_count == 0 {
        return 0..0;
    }

    let window = height.min(row_count);
    row_count - window..row_count
}

fn selected_style(style: Style, selected: bool, theme: TuiTheme) -> Style {
    if selected {
        style.bg(theme.selection_bg)
    } else {
        style
    }
}

fn transcript_row(
    item: &TranscriptItem,
    theme: TuiTheme,
    expanded: bool,
) -> (&'static str, String, Style) {
    match item {
        TranscriptItem::User { content } => {
            ("You", content.clone(), Style::default().fg(theme.user))
        }
        TranscriptItem::Assistant { thinking, content } => (
            "Assistant",
            assistant_display_text(thinking.as_deref(), content),
            Style::default().fg(theme.assistant),
        ),
        TranscriptItem::Tool {
            name,
            detail,
            status,
            ..
        } => {
            let detail = if expanded {
                detail.clone()
            } else {
                collapsed_tool_detail(*status, detail)
            };
            (
                "Tool",
                format!("{} {} ({})", status.marker(), name, detail),
                status_style(*status, theme),
            )
        }
        TranscriptItem::Image { metadata, .. } => {
            ("Image", metadata.clone(), Style::default().fg(theme.notice))
        }
        TranscriptItem::Compaction { .. } => {
            ("Compact", String::new(), Style::default().fg(theme.accent))
        }
        TranscriptItem::Notice { content } => {
            ("Notice", content.clone(), Style::default().fg(theme.notice))
        }
        TranscriptItem::Banner { title, .. } => {
            ("Banner", title.clone(), Style::default().fg(theme.header))
        }
    }
}

fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{}m", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

fn collapsed_tool_detail(status: ToolStatusKind, detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() {
        return status.label().to_owned();
    }

    let line_count = detail.lines().count().max(1);
    if line_count == 1 {
        format!("{} · 1 line", status.label())
    } else {
        format!("{} · {line_count} lines", status.label())
    }
}

fn assistant_display_text(thinking: Option<&str>, content: &str) -> String {
    match thinking {
        Some(thinking) if !content.is_empty() => format!("{thinking}\n\n{content}"),
        Some(thinking) => thinking.to_owned(),
        None => content.to_owned(),
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
        if area.width == 0 || area.height == 0 {
            return;
        }

        buf.set_style(area, Style::default().bg(self.theme.composer_bg));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.overlay_border));
        let inner = block.inner(area);
        block.render(area, buf);

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

        let width = usize::from(inner.width.max(1));
        for (row, line) in wrap_width(&display, width)
            .into_iter()
            .enumerate()
            .take(usize::from(inner.height))
        {
            let Ok(row) = u16::try_from(row) else {
                break;
            };
            write_line(
                inner,
                buf,
                inner.y + row,
                &line,
                Style::default().fg(self.theme.prompt),
            );
        }
    }
}

impl Widget for ApprovalModal {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        buf.set_style(area, Style::default().bg(self.theme.approval_bg));

        let block = Block::default()
            .title(" Action required ")
            .title_alignment(Alignment::Left)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.approval_border));
        let inner = block.inner(area);
        block.render(area, buf);

        let content_area = Rect {
            x: inner.x.saturating_add(1),
            y: inner.y,
            width: inner.width.saturating_sub(2).max(1),
            height: inner.height,
        };
        let text_width = usize::from(content_area.width.max(1));
        let mut y = inner.y;
        write_line(
            content_area,
            buf,
            y,
            &self.title,
            Style::default()
                .fg(self.theme.approval_title)
                .add_modifier(Modifier::BOLD),
        );
        y = y.saturating_add(1);

        for line in wrap_width(&self.body, text_width) {
            if y >= content_area.bottom() {
                return;
            }
            write_line(content_area, buf, y, &line, Style::default());
            y = y.saturating_add(1);
        }

        y = y.saturating_add(1);
        if y < content_area.bottom() {
            let hints = self
                .options
                .iter()
                .enumerate()
                .map(|(index, option)| format!("{}. {}", index + 1, option.label))
                .collect::<Vec<_>>()
                .join(" · ");
            write_line(
                content_area,
                buf,
                y,
                &hints,
                Style::default().fg(self.theme.notice),
            );
            y = y.saturating_add(1);
        }
        for (index, option) in self.options.iter().enumerate() {
            if y >= content_area.bottom() {
                break;
            }
            let marker = if index == self.selected { ">" } else { " " };
            let style = if index == self.selected {
                Style::default()
                    .fg(self.theme.selected_fg)
                    .bg(self.theme.selected_bg)
            } else {
                Style::default().fg(self.theme.prompt)
            };
            write_line(
                content_area,
                buf,
                y,
                &format!("{marker} {}", option.label),
                style,
            );
            y = y.saturating_add(1);
        }
    }
}

impl Widget for &NeoTuiApp {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        buf.set_style(area, Style::default().bg(self.theme().background));
        let mut header_parts = vec![
            self.title().to_owned(),
            format!("session:{}", self.session_label()),
            format!("model:{}", self.model_label()),
        ];
        if let Some(context) = self.context_window_label() {
            header_parts.push(context);
        }
        if let Some(working) = self.working_label() {
            header_parts.push(format!("● {working}"));
        }
        let header = header_parts.join("  ");
        write_line(
            area,
            buf,
            area.y,
            &header,
            Style::default()
                .fg(self.theme().header)
                .add_modifier(Modifier::BOLD),
        );

        let approval_overlay = match self.focused_overlay().map(|overlay| &overlay.kind) {
            Some(OverlayKind::Approval(request)) => Some(request),
            _ => None,
        };
        let layout = app_layout(self, area);
        TranscriptWidget::new(self.transcript())
            .with_view(self.transcript_view())
            .with_selection(self.transcript_selection())
            .with_expanded_items(self.expanded_transcript_items())
            .with_activity_frame(self.activity_frame())
            .with_theme(self.theme())
            .render(layout.body, buf);

        if let Some(request) = approval_overlay {
            request
                .modal
                .clone()
                .with_theme(self.theme())
                .render(layout.approval, buf);
        }

        if let Some(overlay) = self.focused_overlay()
            && let OverlayKind::SessionPicker(state) = &overlay.kind
        {
            render_session_picker(state, layout.session_picker, buf, self.theme());
        }

        PromptWidget::new(self.prompt())
            .with_theme(self.theme())
            .render(layout.prompt, buf);

        render_footer(self, layout.footer, buf);

        if let Some(overlay) = self.focused_overlay()
            && !matches!(
                overlay.kind,
                OverlayKind::Approval(_) | OverlayKind::SessionPicker(_)
            )
        {
            render_overlay(overlay, area, layout.prompt.y, buf);
        }
    }
}

fn render_session_picker(
    state: &crate::SessionPickerState,
    area: Rect,
    buf: &mut Buffer,
    theme: TuiTheme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    Clear.render(area, buf);
    let block = Block::default()
        .title(" Sessions ")
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.overlay_border));
    let inner = block.inner(area);
    block.render(area, buf);

    let mut y = inner.y;
    for item in state.list().visible_items().iter().take(4) {
        if y.saturating_add(2) >= inner.bottom() {
            break;
        }
        let selected = item.selected;
        let marker = if selected { ">" } else { " " };
        let mut fields = item
            .item
            .description
            .as_deref()
            .unwrap_or_default()
            .split(" | ");
        let id = fields.next().unwrap_or(&item.item.value);
        let time = fields.next().unwrap_or("");
        let workspace = fields.next().unwrap_or("");
        let prompt = fields.next().unwrap_or("");
        let title_line = format!("{marker} {}  {time}", item.item.label);
        let meta_line = format!("  {id}  {workspace}");
        let prompt_line = format!("  > {prompt}");
        let title_style = if selected {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.prompt)
        };
        write_line(inner, buf, y, &title_line, title_style);
        y = y.saturating_add(1);
        write_line(inner, buf, y, &meta_line, Style::default().fg(theme.muted));
        y = y.saturating_add(1);
        write_line(
            inner,
            buf,
            y,
            &prompt_line,
            Style::default().fg(theme.muted),
        );
        y = y.saturating_add(1);
    }

    if inner.height > 0 {
        let hint_y = inner.bottom().saturating_sub(1);
        write_line(
            inner,
            buf,
            hint_y,
            "↑↓ navigate · Enter resume · Esc cancel · Ctrl+N fork",
            Style::default().fg(theme.notice),
        );
    }
}

fn render_footer(app: &NeoTuiApp, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    buf.set_style(area, Style::default().bg(app.theme().background));

    let theme = app.theme();
    let (permission_label, permission_color) = app.permission_badge();
    let muted = Style::default().fg(theme.muted);
    let working_style = Style::default().fg(theme.footer_working);
    let hint_style = Style::default().fg(theme.footer_hint);
    let context_style = Style::default().fg(app.context_color());

    // Two-line footer when space allows; otherwise just the hints/context line.
    if area.height >= 2 {
        let mut status_spans = vec![
            Span::styled(
                format!("[{permission_label}]"),
                Style::default().fg(permission_color),
            ),
            Span::raw(" "),
        ];
        if let Some(working) = app.working_label() {
            status_spans.push(Span::styled(format!("● {working}"), working_style));
            status_spans.push(Span::raw(" "));
        }
        status_spans.push(Span::styled(app.cwd_label(), muted));

        let status_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        render_truncated_line(status_spans, status_area, buf);
    }

    let hints_y = area.y.saturating_add(area.height.saturating_sub(1));
    let narrow = area.width < 50;
    let mut hints = if narrow {
        "enter send · esc interrupt".to_owned()
    } else {
        "enter send · shift+enter newline · / commands".to_owned()
    };
    if !app.transcript_view().is_following_tail() && area.width >= 60 {
        hints.push_str(" · new output below · end to jump");
    }

    let context_label = app.context_window_label();
    let context_width = context_label
        .as_ref()
        .map(|label| visible_width(label))
        .unwrap_or(0);
    let gap: u16 = if context_width > 0 { 1 } else { 0 };
    let left_width = area
        .width
        .saturating_sub(context_width as u16)
        .saturating_sub(gap);

    let hints_area = Rect {
        x: area.x,
        y: hints_y,
        width: left_width,
        height: 1,
    };
    render_truncated_line(vec![Span::styled(hints, hint_style)], hints_area, buf);

    if let Some(context_label) = context_label {
        let context_area = Rect {
            x: area.x.saturating_add(left_width).saturating_add(gap),
            y: hints_y,
            width: context_width as u16,
            height: 1,
        };
        render_truncated_line_right(
            vec![Span::styled(context_label, context_style)],
            context_area,
            buf,
        );
    }
}

fn render_truncated_line(spans: Vec<Span<'_>>, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line = Line::from(truncate_spans(spans, usize::from(area.width)));
    Paragraph::new(line).render(area, buf);
}

fn render_truncated_line_right(spans: Vec<Span<'_>>, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line = Line::from(truncate_spans(spans, usize::from(area.width)));
    Paragraph::new(line)
        .alignment(Alignment::Right)
        .render(area, buf);
}

fn truncate_spans(spans: Vec<Span<'_>>, max_width: usize) -> Vec<Span<'_>> {
    let mut used = 0;
    let mut out = Vec::new();
    for span in spans {
        let width = visible_width(&span.content);
        if used + width <= max_width {
            used += width;
            out.push(span);
        } else {
            let remaining = max_width.saturating_sub(used);
            if remaining > 0 {
                out.push(Span::styled(
                    clip_width(&span.content, remaining),
                    span.style,
                ));
            }
            break;
        }
    }
    out
}

fn approval_panel_height(modal: &ApprovalModal, width: u16) -> u16 {
    let content_width = usize::from(width.saturating_sub(4).max(1));
    let body_lines = wrap_width(&modal.body, content_width).len();
    let total = 2usize
        .saturating_add(1)
        .saturating_add(body_lines)
        .saturating_add(1)
        .saturating_add(1)
        .saturating_add(modal.options.len());
    u16::try_from(total.clamp(5, 10)).unwrap_or(10)
}

fn prompt_height(prompt: &PromptState, width: u16) -> u16 {
    let display_width = usize::from(width.saturating_sub(2).max(1));
    let lines = wrap_width(&format!("> {}", prompt.text), display_width)
        .len()
        .clamp(1, 6);
    u16::try_from(lines.saturating_add(2)).unwrap_or(8)
}

fn render_overlay(overlay: &Overlay, area: Rect, composer_y: u16, buf: &mut Buffer) {
    let width = area.width.saturating_sub(4).clamp(20, 56);
    let lines = overlay_lines(overlay, usize::from(width.saturating_sub(2).max(1)));
    let content_height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let height = content_height.saturating_add(2).min(area.height).max(3);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = if matches!(overlay.kind, OverlayKind::PromptCompletion(_)) {
        composer_y
            .saturating_sub(height)
            .max(area.y.saturating_add(1))
    } else {
        area.y + area.height.saturating_sub(height) / 2
    };
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

fn fill_line(area: Rect, buf: &mut Buffer, y: u16, style: Style) {
    if area.width == 0 || y >= area.bottom() {
        return;
    }

    buf.set_style(
        Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
        style,
    );
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
