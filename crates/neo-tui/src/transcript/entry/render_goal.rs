use super::{Color, GoalCardKind, Style, TuiTheme};
use crate::primitive::{Line, paint, wrap_width};
use crate::widgets::box_draw;

pub(super) fn render_goal_card(
    kind: GoalCardKind,
    objective: &str,
    detail: Option<&str>,
    turns: Option<u32>,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let chrome = GoalCardChrome::new(kind, theme);
    let content = goal_card_content(&chrome, objective, detail, turns);
    render_goal_card_rows(&content, width, &chrome, theme)
}

struct GoalCardChrome {
    icon: &'static str,
    label: &'static str,
    color: Color,
}

impl GoalCardChrome {
    fn new(kind: GoalCardKind, theme: &TuiTheme) -> Self {
        Self {
            icon: goal_card_icon(kind),
            label: goal_card_label(kind),
            color: goal_card_color(kind, theme),
        }
    }

    fn header(&self) -> String {
        format!("{} {}", self.icon, self.label)
    }
}

const GOAL_CARD_ICONS: [&str; 5] = ["▶", "⏸", "▶", "⏹", "✓"];
const GOAL_CARD_LABELS: [&str; 5] = [
    "GOAL STARTED",
    "GOAL PAUSED",
    "GOAL RESUMED",
    "GOAL BLOCKED",
    "GOAL COMPLETE",
];

fn goal_card_icon(kind: GoalCardKind) -> &'static str {
    GOAL_CARD_ICONS[kind as usize]
}

fn goal_card_label(kind: GoalCardKind) -> &'static str {
    GOAL_CARD_LABELS[kind as usize]
}

fn goal_card_color(kind: GoalCardKind, theme: &TuiTheme) -> Color {
    [
        theme.brand,
        theme.status_warn,
        theme.brand,
        theme.status_error,
        theme.status_ok,
    ][kind as usize]
}

fn goal_card_content(
    chrome: &GoalCardChrome,
    objective: &str,
    detail: Option<&str>,
    turns: Option<u32>,
) -> Vec<String> {
    let mut content: Vec<String> = Vec::new();
    content.push(chrome.header());
    content.push(String::new());
    content.push(objective.to_owned());
    if let Some(detail) = detail {
        content.push(String::new());
        content.push(detail.to_owned());
    }
    if let Some(turns) = turns {
        content.push(String::new());
        content.push(format!("Turns used: {turns}"));
    }
    content
}

fn render_goal_card_rows(
    content: &[String],
    width: usize,
    chrome: &GoalCardChrome,
    theme: &TuiTheme,
) -> Vec<Line> {
    let border_style = Style::default().fg(chrome.color);
    let header_style = Style::default().fg(chrome.color).bold();
    let body_style = Style::default().fg(theme.text_primary);
    let inner_width = width.saturating_sub(4).max(1);
    let mut rows: Vec<Line> = Vec::new();
    rows.push(Line::raw(box_draw::top_border(width, border_style)));
    for (idx, line) in content.iter().enumerate() {
        let style = if idx == 0 { header_style } else { body_style };
        rows.extend(render_goal_card_content_line(
            line,
            inner_width,
            width,
            border_style,
            style,
        ));
    }
    rows.push(Line::raw(box_draw::bottom_border(width, border_style)));
    rows.push(Line::raw(""));
    rows
}

fn render_goal_card_content_line(
    line: &str,
    inner_width: usize,
    width: usize,
    border_style: Style,
    style: Style,
) -> Vec<Line> {
    let wrapped = wrap_width(line, inner_width);
    if wrapped.is_empty() {
        return vec![Line::raw(paint(
            &box_draw::content_line("", width, border_style),
            style,
        ))];
    }
    wrapped
        .into_iter()
        .map(|part| {
            Line::raw(paint(
                &box_draw::content_line(&format!(" {part} "), width, border_style),
                style,
            ))
        })
        .collect()
}
