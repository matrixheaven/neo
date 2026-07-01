use super::{BannerData, Style, TuiTheme};
use crate::primitive::{Color, Line, paint, visible_width, wrap_width};
use crate::widgets::box_draw;

const NEON_CYAN: Color = Color::Rgb(63, 247, 255);
const NEON_MAGENTA: Color = Color::Rgb(255, 79, 216);
const NEON_VIOLET: Color = Color::Rgb(138, 92, 255);

fn neon_logo_row(row: usize) -> String {
    let (left, center, right) = match row {
        0 => ("\u{2590}\u{2588}", "\u{2598} ", "\u{2588}\u{258c}"),
        1 => ("\u{2590}\u{2588}", " \u{2597}", "\u{2588}\u{258c}"),
        _ => return String::new(),
    };
    format!(
        "{}{}{}",
        paint(left, Style::default().fg(NEON_CYAN).bold()),
        paint(center, Style::default().fg(NEON_MAGENTA).bold()),
        paint(right, Style::default().fg(NEON_VIOLET).bold()),
    )
}

/// Wrap `text` and apply a bullet prefix to the first row, indenting the rest
/// to align under the body (prefix width of spaces). This is the Neo
/// "bullet + indented continuation" layout used by user/assistant messages.
fn bulleted_wrap(text: &str, width: usize, prefix: &str, style: Style) -> Vec<Line> {
    let prefix_width = visible_width(prefix);
    // BUGFIX: previously this wrapped at the full `width` without subtracting
    // the prefix, so the first rendered row was `prefix + width` columns wide
    // and overflowed the terminal. Long CJK prompts (each char is 2 columns)
    // hit this reliably and crashed the renderer's width invariant
    // (`renderer.rs` `check_line_widths`). The body budget must reserve space
    // for the prefix, mirroring `styled_wrap_with_prefix` and `render_markdown`.
    let body_width = width.saturating_sub(prefix_width).max(1);
    let indent = " ".repeat(prefix_width);
    let mut rows = Vec::new();
    for (i, line) in wrap_width(text, body_width).into_iter().enumerate() {
        if i == 0 {
            rows.push(Line::styled(format!("{prefix}{line}"), style));
        } else {
            rows.push(Line::styled(format!("{indent}{line}"), style));
        }
    }
    if rows.is_empty() {
        rows.push(Line::styled(prefix.to_owned(), style));
    }
    rows
}

/// Render the welcome banner as a rounded box with Neo's neon-N logo and
/// aligned metadata.
///
/// Layout:
/// ```text
/// ╭──────╮
/// │      │
/// │  ▐█▘ █▌  Welcome to Neo!
/// │  ▐█ ▗█▌  Send /help for help.
/// │      │
/// │  Directory: /path
/// │  Session:   abc
/// │  Model:     GLM
/// │  ...
/// │      │
/// ╰──────╯
/// ```
pub(super) fn render_welcome_banner(
    data: &BannerData,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    use std::fmt::Write as _;
    let gap = "  ";

    // Build the content lines (plain text with ANSI via paint, to be padded).
    let title_style = Style::default().fg(theme.brand).bold();
    let subtitle_style = Style::default().fg(theme.text_muted);
    let label_style = Style::default().fg(theme.text_muted).bold();
    let value_style = Style::default().fg(theme.text_primary);

    let mut content: Vec<String> = Vec::new();
    // blank line at top of box
    content.push(String::new());
    // logo + title / subtitle
    let mut line0 = String::new();
    let _ = write!(line0, "{}{}", neon_logo_row(0), gap);
    let mut rest0 = String::new();
    if !data.title.is_empty() {
        rest0.push_str(&paint(&data.title, title_style));
    }
    content.push(format!("{line0}{rest0}"));
    let mut line1 = String::new();
    let _ = write!(line1, "{}{}", neon_logo_row(1), gap);
    let mut rest1 = String::new();
    if !data.subtitle.is_empty() {
        rest1.push_str(&paint(&data.subtitle, subtitle_style));
    }
    content.push(format!("{line1}{rest1}"));
    // blank line between logo and metadata
    content.push(String::new());

    // Metadata rows: label padded to a fixed width so colons align.
    let label_w = 11usize;
    let mut meta: Vec<(&str, &str)> = Vec::new();
    if !data.directory.is_empty() {
        meta.push(("Directory:", data.directory.as_str()));
    }
    if !data.session.is_empty() {
        meta.push(("Session:", data.session.as_str()));
    }
    if !data.model.is_empty() {
        meta.push(("Model:", data.model.as_str()));
    }
    if !data.version.is_empty() {
        meta.push(("Version:", data.version.as_str()));
    }
    if let Some(m) = &data.mcp {
        meta.push(("MCP:", m.as_str()));
    }
    for (label, value) in &meta {
        let mut label_padded = label.to_string();
        while visible_width(&label_padded) < label_w {
            label_padded.push(' ');
        }
        let mut row = String::new();
        let _ = write!(
            row,
            "{} {}",
            paint(&label_padded, label_style),
            paint(value, value_style)
        );
        content.push(row);
    }
    // blank line at bottom of box
    content.push(String::new());

    let border_style = Style::default().fg(theme.brand);
    let mut rows = Vec::new();
    rows.push(Line::raw(box_draw::top_border(width, border_style)));
    for cline in &content {
        rows.push(Line::raw(box_draw::content_line(
            &format!(" {cline} "),
            width,
            border_style,
        )));
    }
    rows.push(Line::raw(box_draw::bottom_border(width, border_style)));
    rows.push(Line::raw(""));
    rows
}

pub(super) fn render_user_message(content: &str, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let style = Style::default().fg(theme.user_message);
    bulleted_wrap(content, width, "✨ ", style)
}

/// Render a queued/steered message. Steer uses `↳` (brand color) to signal an
/// immediate mid-turn injection; follow-up uses `↪` (muted) to signal a queued
/// turn that runs after the current one.
pub(super) fn render_queued_message(
    text: &str,
    is_steer: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let (prefix, style) = if is_steer {
        ("↳ ", Style::default().fg(theme.brand).italic())
    } else {
        ("↪ ", Style::default().fg(theme.text_muted))
    };
    bulleted_wrap(text, width, prefix, style)
}

pub(super) fn render_assistant_message(content: &str, width: usize, theme: &TuiTheme) -> Vec<Line> {
    if content.is_empty() {
        Vec::new()
    } else {
        crate::markdown::render_markdown(content, width, theme, "● ", "  ")
    }
}
