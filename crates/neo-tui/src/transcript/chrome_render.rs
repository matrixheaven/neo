use crate::primitive::Line;
use crate::primitive::theme::{DevelopmentMode, GoalModeStatus, TuiTheme};
use crate::primitive::wrap_width;
use crate::primitive::{Color, Style, paint, truncate_to_width, visible_width};
use crate::screen_output::{CURSOR_MARKER, CursorPos};
use crate::shell::{MAX_PROMPT_VISIBLE_LINES, NeoChromeState, PromptState};
use crate::transcript::{ToolCallComponent, ToolCallState, ToolGroup, render_tool_group};
use crate::widgets::box_draw::{ROUNDED, repeat_char};
use crate::widgets::{PendingInputPreview, TodoPanel, box_draw};

const GITHUB_YELLOW: Color = Color::Rgb(191, 135, 0);
const GITHUB_GREEN: Color = Color::Rgb(26, 127, 55);
const GITHUB_RED: Color = Color::Rgb(207, 34, 46);
const GITHUB_BLUE: Color = Color::Rgb(9, 105, 218);

/// Uniform 1-column left/right gutter applied to ALL chrome (body, banner,
/// prompt box, footer). Matches Neo's `CHROME_GUTTER = 1`. Applied once
/// by [`apply_gutter`] after body + chrome are merged, so nothing renders
/// flush against the screen edge.
pub const CHROME_GUTTER: usize = 1;

/// Prepend `CHROME_GUTTER` spaces to every non-empty line. Empty separator
/// lines stay empty so vertical spacing isn't shifted.
pub fn apply_gutter(lines: &mut [String]) {
    if CHROME_GUTTER == 0 {
        return;
    }
    let lead = " ".repeat(CHROME_GUTTER);
    for line in lines.iter_mut() {
        if !line.is_empty() {
            line.insert_str(0, &lead);
        }
    }
}

/// Render an ordered slice of tool components, collapsing consecutive runs of
/// the same groupable tool (read/grep/glob/find) into a single tree card.
///
/// A run of length 1 still renders as a normal solo card. Any non-groupable
/// tool (bash/edit/write/...) breaks an in-progress run. Live output buffers
/// are preserved because we render from the components directly (not cloned
/// states).
pub(super) fn render_ordered_tools(
    ordered: &mut [ToolCallComponent],
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < ordered.len() {
        if !rows.is_empty() {
            rows.push(Line::raw(""));
        }
        let current_name = ordered[i].name().to_owned();
        let groupable = is_groupable(&current_name);
        if !groupable {
            rows.extend(ordered[i].render_with_theme(width, theme));
            i += 1;
            continue;
        }
        // Greedy run of consecutive same-name groupable tools.
        let mut j = i + 1;
        while j < ordered.len()
            && ordered[j].name() == current_name
            && is_groupable(ordered[j].name())
        {
            j += 1;
        }
        if j - i >= 2 {
            // Group of >= 2: render as a tree card. Only group tools that are
            // NOT still streaming live output (a running read shows solo).
            let any_live_output = ordered[i..j].iter().any(|t| !t.progress().is_empty());
            if any_live_output {
                for tool in &mut ordered[i..j] {
                    rows.extend(tool.render_with_theme(width, theme));
                }
            } else {
                let states: Vec<&ToolCallState> =
                    ordered[i..j].iter().map(ToolCallComponent::state).collect();
                let expanded = ordered[i..j].iter().all(ToolCallComponent::is_expanded);
                let group = ToolGroup {
                    tool: current_name.clone(),
                    states,
                };
                rows.extend(render_tool_group(&group, width, theme, expanded));
            }
        } else {
            rows.extend(ordered[i].render_with_theme(width, theme));
        }
        i = j;
    }
    rows
}

/// Whether a tool name is eligible for consecutive-call grouping.
fn is_groupable(name: &str) -> bool {
    matches!(name, "Read" | "Grep" | "Glob" | "Find" | "List")
}

/// Chrome lines, optional cursor position, and the row where the prompt box
/// starts within those lines.
pub struct ChromeRender {
    pub lines: Vec<String>,
    pub cursor: Option<CursorPos>,
    pub prompt_start_row: usize,
}

#[must_use]
pub fn render_chrome_lines(app: &NeoChromeState, width: usize, height: usize) -> ChromeRender {
    let content_width = frame_content_width(width);
    let mut lines = Vec::new();
    if app.has_todos() {
        lines.extend(
            TodoPanel::new(app.todo_items())
                .with_theme(app.theme())
                .expanded(app.todo_panel_expanded())
                .render(content_width),
        );
    }
    if let Some(question) = app.question_dialog_state() {
        lines.extend(question.render_lines(content_width));
    }
    if let Some(btw_state) = app.btw_panel_state() {
        let terminal_rows = u16::try_from(height).unwrap_or(u16::MAX);
        let mut btw_state = btw_state.clone();
        lines.extend(
            crate::widgets::BtwPanel::new(&mut btw_state)
                .with_theme(app.theme())
                .render(content_width, terminal_rows),
        );
    }
    let pending_input = PendingInputPreview::new(
        app.pending_input().pending_steers(),
        app.pending_input().queued_follow_ups(),
        app.pending_input().queued_shell_commands(),
    )
    .with_theme(app.theme())
    .render(content_width);
    if !pending_input.is_empty() {
        lines.push(String::new());
        lines.extend(pending_input);
    }
    let prompt_start_row = lines.len();
    let (prompt_lines, prompt_cursor) = if app.focused_overlay_blocks_prompt() {
        (Vec::new(), None)
    } else {
        render_prompt_lines(app, content_width)
    };
    lines.extend(prompt_lines);
    if !app.focused_overlay_blocks_prompt()
        && let Some(dropdown) = render_prompt_completion_dropdown(app, content_width)
    {
        lines.extend(dropdown);
    }
    lines.extend(render_footer_lines(app, content_width));
    ChromeRender {
        lines,
        cursor: prompt_cursor,
        prompt_start_row,
    }
}

/// Mutable variant of [`render_chrome_lines`] that updates the `/btw` panel's
/// internal scroll and height state instead of discarding those updates.
#[must_use]
pub fn render_chrome_lines_mut(
    app: &mut NeoChromeState,
    width: usize,
    height: usize,
) -> ChromeRender {
    let content_width = frame_content_width(width);
    let mut lines = Vec::new();
    if app.has_todos() {
        lines.extend(
            TodoPanel::new(app.todo_items())
                .with_theme(app.theme())
                .expanded(app.todo_panel_expanded())
                .render(content_width),
        );
    }
    if let Some(question) = app.question_dialog_state() {
        lines.extend(question.render_lines(content_width));
    }
    let terminal_rows = u16::try_from(height).unwrap_or(u16::MAX);
    let theme = app.theme();
    if let Some(btw_state) = app.btw_panel_state_mut() {
        lines.extend(
            crate::widgets::BtwPanel::new(btw_state)
                .with_theme(theme)
                .render(content_width, terminal_rows),
        );
    }
    let pending_input = PendingInputPreview::new(
        app.pending_input().pending_steers(),
        app.pending_input().queued_follow_ups(),
        app.pending_input().queued_shell_commands(),
    )
    .with_theme(app.theme())
    .render(content_width);
    if !pending_input.is_empty() {
        lines.push(String::new());
        lines.extend(pending_input);
    }
    let prompt_start_row = lines.len();
    let (prompt_lines, prompt_cursor) = if app.focused_overlay_blocks_prompt() {
        (Vec::new(), None)
    } else {
        render_prompt_lines(app, content_width)
    };
    lines.extend(prompt_lines);
    if !app.focused_overlay_blocks_prompt()
        && let Some(dropdown) = render_prompt_completion_dropdown(app, content_width)
    {
        lines.extend(dropdown);
    }
    lines.extend(render_footer_lines(app, content_width));
    ChromeRender {
        lines,
        cursor: prompt_cursor,
        prompt_start_row,
    }
}

/// Render only the footer status line, without the prompt box. Used when a
/// session picker overlay replaces the prompt/editor area.
#[must_use]
pub fn render_footer_only_lines(app: &NeoChromeState, width: usize) -> Vec<String> {
    let content_width = frame_content_width(width);
    render_footer_lines(app, content_width)
}

#[must_use]
pub fn frame_content_width(width: usize) -> usize {
    width.saturating_sub(CHROME_GUTTER + 1).max(1)
}

/// Render the `/` command dropdown below the prompt box, if active.
fn render_prompt_completion_dropdown(app: &NeoChromeState, width: usize) -> Option<Vec<String>> {
    let overlay = app.focused_overlay()?;
    let crate::shell::OverlayKind::PromptCompletion(state) = &overlay.kind else {
        return None;
    };
    let inner_width = width.saturating_sub(2).max(1);
    let theme = app.theme();
    let raw_lines = state.render_lines(inner_width, &theme);
    if raw_lines.is_empty() {
        return None;
    }
    let border_style = Style::default().fg(theme.brand);
    let mut lines = Vec::with_capacity(raw_lines.len() + 1);
    for raw in raw_lines {
        lines.push(box_draw::side_bordered_line(&raw, width, border_style));
    }
    lines.push(box_draw::bottom_border(width, border_style));
    Some(lines)
}

/// Render the rounded prompt input box. The first content line carries the
/// `> ` prompt symbol; continuation lines use a 4-space hanging indent so
/// wrapped/explicit-newline text aligns under the body (matching Neo's
/// `paddingX: 4` editor). Border color is weak by default and switches to
/// the brand color when text is present or plan mode is active.
fn render_prompt_lines(app: &NeoChromeState, width: usize) -> (Vec<String>, Option<CursorPos>) {
    let theme = app.theme();
    let prompt = app.prompt();
    let highlighted = app.is_plan_mode() || app.shell_mode_active() || !prompt.text.is_empty();
    let border_color = if highlighted {
        if app.shell_mode_active() {
            theme.shell_mode
        } else {
            theme.brand
        }
    } else {
        theme.text_muted
    };
    let border_style = Style::default().fg(border_color);
    let text_style = Style::default().fg(theme.text_primary);

    let inner_width = width.saturating_sub(2).max(1);
    let body_width = inner_width.saturating_sub(4).max(1);

    let logical_lines = build_prompt_logical_lines(prompt, body_width);

    // Total wrapped lines, counting empty logical lines as one display row.
    // Tabs must be expanded first so the count matches what build_prompt_logical_lines renders.
    let total_lines: usize = prompt
        .text
        .split('\n')
        .map(|line| {
            wrap_width(&expand_prompt_tabs(line), body_width)
                .len()
                .max(1)
        })
        .sum();
    let scroll_offset = prompt.scroll_offset();
    let lines_below = total_lines.saturating_sub(scroll_offset + logical_lines.len());

    let mut lines = Vec::with_capacity(logical_lines.len() + 2);
    lines.push(if scroll_offset > 0 {
        scroll_indicator_top_border(width, scroll_offset, border_style)
    } else if app.shell_mode_active() {
        box_draw::top_border_with_label(
            width,
            "! shell mode",
            border_style,
            Style::default().fg(theme.shell_mode).bold(),
        )
    } else {
        box_draw::top_border(width, border_style)
    });
    for (idx, line) in logical_lines.iter().enumerate() {
        let prefix = if idx == 0 && app.shell_mode_active() {
            " ! "
        } else if idx == 0 {
            " > "
        } else {
            "   "
        };
        let content = paint(&format!("{prefix}{line}"), text_style);
        lines.push(box_draw::content_line(&content, width, border_style));
    }
    lines.push(if lines_below > 0 {
        scroll_indicator_bottom_border(width, lines_below, border_style)
    } else {
        box_draw::bottom_border(width, border_style)
    });

    let cursor = find_cursor(&lines);
    let lines = lines
        .into_iter()
        .map(|line| line.replace(CURSOR_MARKER, ""))
        .collect();
    (lines, cursor)
}

/// Build the per-line content (already wrapped) for the prompt, inserting the
/// cursor marker on the active line. Each returned string is the body text
/// (without the ` > `/`    ` prefix, which is added by the caller).
fn build_prompt_logical_lines(prompt: &PromptState, body_width: usize) -> Vec<String> {
    let text = &prompt.text;
    let cursor = prompt.cursor.min(prompt.char_len());

    // Highlight selected marker before inserting the cursor marker.
    let styled_text = if let Some((start_byte, end_byte)) = prompt.selected_marker() {
        let start_char = text[..start_byte].chars().count();
        let end_char = text[..end_byte].chars().count();
        let before = &text[..start_byte];
        let selected = &text[start_byte..end_byte];
        let after = &text[end_byte..];
        let highlighted = paint(selected, Style::default().bg(Color::Rgb(60, 60, 60)));
        let mut styled = String::with_capacity(text.len() + highlighted.len() - selected.len());
        styled.push_str(before);
        styled.push_str(&highlighted);
        styled.push_str(after);

        // Insert the cursor marker at the correct position in the styled text.
        let cursor_byte = if cursor <= start_char {
            prompt.byte_index(cursor)
        } else if cursor >= end_char {
            prompt.byte_index(cursor) + highlighted.len() - selected.len()
        } else {
            // Cursor inside the selected range: place it at the start of the
            // highlighted region.
            start_byte
        };
        let mut with_cursor = String::with_capacity(styled.len() + CURSOR_MARKER.len());
        with_cursor.push_str(&styled[..cursor_byte]);
        with_cursor.push_str(CURSOR_MARKER);
        with_cursor.push_str(&styled[cursor_byte..]);
        with_cursor
    } else {
        let chars: Vec<char> = text.chars().collect();
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars[cursor..].iter().collect();
        format!("{before}{CURSOR_MARKER}{after}")
    };

    let marked = expand_prompt_tabs(&styled_text);
    let mut all_lines = Vec::new();
    for logical in marked.split('\n') {
        let wrapped = wrap_width(logical, body_width);
        if wrapped.is_empty() {
            all_lines.push(String::new());
        } else {
            all_lines.extend(wrapped);
        }
    }
    if all_lines.len() <= MAX_PROMPT_VISIBLE_LINES {
        return all_lines;
    }
    let max_offset = all_lines.len().saturating_sub(MAX_PROMPT_VISIBLE_LINES);
    let scroll_offset = prompt.scroll_offset().min(max_offset);
    all_lines
        .into_iter()
        .skip(scroll_offset)
        .take(MAX_PROMPT_VISIBLE_LINES)
        .collect()
}

fn scroll_indicator_top_border(width: usize, count: usize, style: Style) -> String {
    scroll_indicator_border(
        width,
        count,
        "↑",
        style,
        ROUNDED.top_left,
        ROUNDED.top_right,
    )
}

fn scroll_indicator_bottom_border(width: usize, count: usize, style: Style) -> String {
    scroll_indicator_border(
        width,
        count,
        "↓",
        style,
        ROUNDED.bottom_left,
        ROUNDED.bottom_right,
    )
}

fn scroll_indicator_border(
    width: usize,
    count: usize,
    arrow: &str,
    style: Style,
    left_corner: char,
    right_corner: char,
) -> String {
    if width < 4 {
        return format!(
            "{}{}{}",
            paint(&left_corner.to_string(), style),
            paint(
                &repeat_char(ROUNDED.horizontal, width.saturating_sub(2)),
                style
            ),
            paint(&right_corner.to_string(), style)
        );
    }
    let label = format!(" {arrow} {count} more ");
    let label_width = visible_width(&label);
    let inner = width.saturating_sub(2);
    if label_width >= inner {
        return format!(
            "{}{}{}",
            paint(&left_corner.to_string(), style),
            paint(&repeat_char(ROUNDED.horizontal, inner), style),
            paint(&right_corner.to_string(), style)
        );
    }
    let bars = inner.saturating_sub(label_width);
    // Left-aligned: the indicator sits at the left corner, matching natural
    // left-to-right reading order.
    let right_bars = bars;
    format!(
        "{}{}{}{}",
        paint(&left_corner.to_string(), style),
        paint(&label, style),
        paint(&repeat_char(ROUNDED.horizontal, right_bars), style),
        paint(&right_corner.to_string(), style)
    )
}

fn expand_prompt_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_owned();
    }
    text.replace('\t', "    ")
}

fn find_cursor(lines: &[String]) -> Option<CursorPos> {
    for (row, line) in lines.iter().enumerate() {
        if let Some(byte_pos) = line.find(CURSOR_MARKER) {
            let col = visible_width(&line[..byte_pos]);
            return Some(CursorPos { row, col });
        }
    }
    None
}

fn render_footer_lines(app: &NeoChromeState, width: usize) -> Vec<String> {
    let theme = app.theme();
    let (perm_label, perm_color) = app.permission_badge();
    let mut left_parts = vec![paint(
        &format!("[{perm_label}]"),
        Style::default().fg(perm_color),
    )];
    if let Some(label) = development_mode_badge(app.development_mode()) {
        left_parts.push(paint(label, Style::default().fg(theme.status_warn).bold()));
    }
    if app.shell_mode_active() {
        left_parts.push(paint(
            "[shell]",
            Style::default().fg(theme.shell_mode).bold(),
        ));
    }
    if !app.model_label().is_empty() {
        left_parts.push(paint(
            app.model_label(),
            Style::default().fg(theme.text_muted),
        ));
    }
    if app.thinking_enabled() {
        left_parts.push(paint(
            "thinking",
            Style::default().fg(theme.footer_working).italic(),
        ));
    }
    if let Some(exit) = app.exit_confirmation_label() {
        left_parts.push(paint(exit, Style::default().fg(theme.status_warn).bold()));
    }
    if let Some(working) = app.working_label() {
        const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let spinner = SPINNER[app.activity_frame() % SPINNER.len()];
        left_parts.push(paint(
            &format!("{spinner} {working}"),
            Style::default().fg(theme.footer_working),
        ));
    }
    left_parts.push(paint(
        &app.cwd_label(),
        Style::default().fg(theme.text_muted),
    ));
    if let Some(git_status) = app.git_status_label() {
        left_parts.push(render_git_status_label(git_status, theme));
    }

    let left_text = left_parts.join(" ");
    let row = if let Some(context_usage) = app.footer_context_usage_label() {
        let context_usage = paint(&context_usage, Style::default().fg(app.context_color()));
        let context_usage_width = visible_width(&context_usage);
        let total = visible_width(&left_text) + context_usage_width;
        if total < width {
            format!("{left_text}{}{context_usage}", " ".repeat(width - total))
        } else if context_usage_width >= width {
            truncate_to_width(&context_usage, width)
        } else {
            let room = width.saturating_sub(context_usage_width).saturating_sub(1);
            format!("{} {context_usage}", truncate_to_width(&left_text, room))
        }
    } else {
        truncate_to_width(&left_text, width)
    };

    vec![row]
}

fn development_mode_badge(mode: DevelopmentMode) -> Option<&'static str> {
    match mode {
        DevelopmentMode::Normal => None,
        DevelopmentMode::Plan => Some("[plan]"),
        DevelopmentMode::Goal(GoalModeStatus::Pending) => Some("[goal]"),
        DevelopmentMode::Goal(GoalModeStatus::Active) => Some("[goal•]"),
        DevelopmentMode::Goal(GoalModeStatus::Paused) => Some("[goal◌]"),
        DevelopmentMode::Goal(GoalModeStatus::Blocked) => Some("[goal✗]"),
    }
}

fn render_git_status_label(label: &str, theme: TuiTheme) -> String {
    let Some((branch, rest)) = label.rsplit_once(" [") else {
        return paint(label, Style::default().fg(GITHUB_YELLOW));
    };
    let status = rest.strip_suffix(']').unwrap_or(rest);
    let mut rendered = paint(branch, Style::default().fg(GITHUB_YELLOW));
    rendered.push_str(&paint(" [", Style::default().fg(theme.text_muted)));
    let mut first = true;
    for part in status.split(' ').filter(|part| !part.is_empty()) {
        if first {
            first = false;
        } else {
            rendered.push_str(&paint(" ", Style::default().fg(theme.text_muted)));
        }
        rendered.push_str(&render_git_status_part(part, theme));
    }
    rendered.push_str(&paint("]", Style::default().fg(theme.text_muted)));
    rendered
}

fn render_git_status_part(part: &str, theme: TuiTheme) -> String {
    let color = if part.starts_with('+') {
        GITHUB_GREEN
    } else if part.starts_with('-') {
        GITHUB_RED
    } else if part.starts_with('↑') || part.starts_with('↓') {
        GITHUB_BLUE
    } else {
        theme.text_muted
    };
    paint(part, Style::default().fg(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::theme::TuiTheme;
    use crate::shell::{NeoChromeState, PickerItem, PromptCompletionPrefix, PromptEdit};

    #[test]
    fn prompt_box_lines_are_exact_width() {
        let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
        app.set_theme(TuiTheme::default());
        app.prompt_mut()
            .apply_edit(PromptEdit::Insert("hello world"));
        let render = render_chrome_lines(&app, 40, 24);
        // Lines render below terminal width so the caller can apply
        // CHROME_GUTTER without triggering terminal autowrap.
        let expected_width = frame_content_width(40);
        for line in &render.lines {
            assert!(
                crate::primitive::visible_width(line) <= expected_width,
                "line: {line:?}"
            );
        }
        // The prompt box borders and content rows must be exactly content_width.
        let prompt_box_lines: Vec<&String> = render
            .lines
            .iter()
            .filter(|l| {
                let s = crate::primitive::strip_ansi(l);
                s.starts_with('│') || s.starts_with('╭') || s.starts_with('╰')
            })
            .collect();
        assert!(!prompt_box_lines.is_empty(), "prompt box lines missing");
        for line in prompt_box_lines {
            assert_eq!(
                crate::primitive::visible_width(line),
                expected_width,
                "line: {line:?}"
            );
        }
    }

    #[test]
    fn completion_dropdown_is_below_prompt() {
        let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
        app.prompt_mut().apply_edit(PromptEdit::Insert("/"));
        app.open_prompt_completion_picker(
            PromptCompletionPrefix {
                start: 0,
                end: 1,
                text: "/".to_owned(),
            },
            vec![
                PickerItem::new("/model", "model", Some("switch model")),
                PickerItem::new("/plan", "plan", Some("toggle plan")),
            ],
        );
        let render = render_chrome_lines(&app, 60, 24);
        // First line is the prompt top border.
        assert!(render.lines[0].contains('╭'));
        let dropdown_start = render
            .lines
            .iter()
            .position(|l| l.contains("model"))
            .expect("dropdown missing");
        assert!(dropdown_start > 1);
        // The line immediately before the dropdown must be the prompt bottom border.
        assert!(render.lines[dropdown_start - 1].contains('╰'));
        // Dropdown items are side-bordered.
        let stripped = crate::primitive::strip_ansi(&render.lines[dropdown_start]);
        assert!(stripped.starts_with('│'));
        assert!(stripped.ends_with('│'));
    }

    #[test]
    fn pending_input_preview_spacer_sits_above_preview_not_prompt() {
        let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
        app.pending_input_mut().queue_follow_up("queued follow-up");

        let render = render_chrome_lines(&app, 80, 24);
        let plain = render
            .lines
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>();
        let pending_header = plain
            .iter()
            .position(|line| line.contains("Queued follow-up inputs"))
            .expect("pending follow-up preview should render");
        let prompt_top = plain
            .iter()
            .position(|line| line.contains('╭'))
            .expect("prompt should render");

        assert!(
            pending_header > 0,
            "pending preview needs a spacer above it"
        );
        assert_eq!(plain[pending_header - 1], "");
        assert!(prompt_top > pending_header);
        assert_ne!(
            plain[prompt_top - 1],
            "",
            "pending preview should sit flush against the prompt box"
        );
    }

    #[test]
    fn mutable_pending_input_preview_spacer_sits_above_preview_not_prompt() {
        let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
        app.pending_input_mut().queue_steer("steer now");

        let render = render_chrome_lines_mut(&mut app, 80, 24);
        let plain = render
            .lines
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>();
        let pending_header = plain
            .iter()
            .position(|line| line.contains("Messages to be submitted after next tool call"))
            .expect("pending steer preview should render");
        let prompt_top = plain
            .iter()
            .position(|line| line.contains('╭'))
            .expect("prompt should render");

        assert!(
            pending_header > 0,
            "pending preview needs a spacer above it"
        );
        assert_eq!(plain[pending_header - 1], "");
        assert!(prompt_top > pending_header);
        assert_ne!(
            plain[prompt_top - 1],
            "",
            "pending preview should sit flush against the prompt box"
        );
    }
}
