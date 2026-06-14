//! Convert [`NeoTuiApp`] state into `Vec<String>` for the custom diff renderer.
//!
//! This module renders the app **entirely** through the project's own [`ansi`]
//! module — no ratatui `Buffer` or `Widget` is involved. Each output line is a
//! complete ANSI-styled string suitable for [`InlineRenderer`](crate::renderer::InlineRenderer).
//!
//! ## Layout
//!
//! ```text
//! ┌───────────────────────────┐
//! │  transcript body          │  ← body_height rows
//! │  (scrolls into scrollback)│
//! ├───────────────────────────┤
//! │  ┌─ prompt ─────────────┐ │  ← prompt_height rows (border + content + border)
//! │  │ > type here…         │ │
//! │  └──────────────────────┘ │
//! ├───────────────────────────┤
//! │  [ask] ● working  ~ws     │  ← footer_height rows (0, 1, or 2)
//! │  enter send · / commands  │
//! └───────────────────────────┘
//! ```

use crate::ansi::{Color as AnsiColor, Style as AnsiStyle, StyledLine, paint, pad_to_width};
use unicode_width::UnicodeWidthChar;
use crate::renderer::{CursorPos, CURSOR_MARKER};
use crate::{
    NeoTuiApp, PromptState, ToolStatusKind, TranscriptItem, TranscriptLine,
    TranscriptRenderer, TuiTheme, truncate_width, visible_width, wrap_width,
};

// ─── Color conversion ───────────────────────────────────────────────────

/// Convert a ratatui [`Color`](ratatui::style::Color) to the project's own
/// [`AnsiColor`].
///
/// `TuiTheme` still carries ratatui color values, so every theme field must
/// pass through this helper before it can be used with [`paint`] or
/// [`AnsiStyle`].
#[must_use]
pub fn ratatui_color_to_ansi(c: ratatui::style::Color) -> AnsiColor {
    use ratatui::style::Color as R;
    match c {
        R::Reset => AnsiColor::Reset,
        R::Black => AnsiColor::Black,
        R::Red => AnsiColor::Red,
        R::Green => AnsiColor::Green,
        R::Yellow => AnsiColor::Yellow,
        R::Blue => AnsiColor::Blue,
        R::Magenta => AnsiColor::Magenta,
        R::Cyan => AnsiColor::Cyan,
        R::Gray => AnsiColor::Gray,
        R::DarkGray => AnsiColor::DarkGray,
        R::LightRed => AnsiColor::LightRed,
        R::LightGreen => AnsiColor::LightGreen,
        R::LightYellow => AnsiColor::LightYellow,
        R::LightBlue => AnsiColor::LightBlue,
        R::LightMagenta => AnsiColor::LightMagenta,
        R::LightCyan => AnsiColor::LightCyan,
        R::White => AnsiColor::White,
        R::Rgb(r, g, b) => AnsiColor::Rgb(r, g, b),
        R::Indexed(n) => AnsiColor::Indexed(n),
    }
}

/// Short alias to keep theme field conversions readable.
const fn tc(c: ratatui::style::Color) -> AnsiColor {
    // Delegate at runtime — `const fn` can't call non-const `ratatui_color_to_ansi`.
    match c {
        ratatui::style::Color::Reset => AnsiColor::Reset,
        ratatui::style::Color::Black => AnsiColor::Black,
        ratatui::style::Color::Red => AnsiColor::Red,
        ratatui::style::Color::Green => AnsiColor::Green,
        ratatui::style::Color::Yellow => AnsiColor::Yellow,
        ratatui::style::Color::Blue => AnsiColor::Blue,
        ratatui::style::Color::Magenta => AnsiColor::Magenta,
        ratatui::style::Color::Cyan => AnsiColor::Cyan,
        ratatui::style::Color::Gray => AnsiColor::Gray,
        ratatui::style::Color::DarkGray => AnsiColor::DarkGray,
        ratatui::style::Color::LightRed => AnsiColor::LightRed,
        ratatui::style::Color::LightGreen => AnsiColor::LightGreen,
        ratatui::style::Color::LightYellow => AnsiColor::LightYellow,
        ratatui::style::Color::LightBlue => AnsiColor::LightBlue,
        ratatui::style::Color::LightMagenta => AnsiColor::LightMagenta,
        ratatui::style::Color::LightCyan => AnsiColor::LightCyan,
        ratatui::style::Color::White => AnsiColor::White,
        ratatui::style::Color::Rgb(r, g, b) => AnsiColor::Rgb(r, g, b),
        ratatui::style::Color::Indexed(n) => AnsiColor::Indexed(n),
    }
}

// ─── Main entry point ────────────────────────────────────────────────────

/// Render the full app into a list of ANSI-styled terminal lines.
///
/// Returns `(lines, cursor)` where:
/// - `lines` — one [`String`] per terminal row, each containing embedded ANSI
///   escape codes and **no** [`CURSOR_MARKER`] (it is stripped after the
///   position is extracted).
/// - `cursor` — the prompt cursor position in `(row, col)` within `lines`, or
///   `None` when the prompt should not capture the cursor.
#[must_use]
pub fn render_app_lines(
    app: &NeoTuiApp,
    width: u16,
    height: u16,
) -> (Vec<String>, Option<CursorPos>) {
    if width == 0 || height == 0 {
        return (vec![String::new()], None);
    }

    let w = usize::from(width);

    // ── Layout calculation ───────────────────────────────────────────
    let footer_rows = footer_height(height);
    let prompt_rows = calc_prompt_height(&app.prompt().text, width);
    let body_height = usize::from(
        height
            .saturating_sub(footer_rows)
            .saturating_sub(prompt_rows),
    );

    // ── Body (transcript) ────────────────────────────────────────────
    let body_lines = render_body(app, body_height, w);

    // ── Prompt ───────────────────────────────────────────────────────
    let (prompt_lines, prompt_cursor) = render_prompt(app, w);

    // ── Footer ───────────────────────────────────────────────────────
    let footer_lines = render_footer(app, w, footer_rows);

    // ── Assemble ─────────────────────────────────────────────────────
    let body_len = body_lines.len();
    let mut lines = Vec::with_capacity(body_len + prompt_lines.len() + footer_lines.len());
    lines.extend(body_lines);
    lines.extend(prompt_lines);
    lines.extend(footer_lines);

    // The prompt cursor row is relative to the prompt block.
    // Offset it by the number of body lines so it points into the final list.
    let cursor = prompt_cursor.map(|c| CursorPos {
        row: body_len + c.row,
        col: c.col,
    });

    (lines, cursor)
}

// ─── Layout helpers ──────────────────────────────────────────────────────

/// Footer occupies 2 rows when there's enough vertical space, 1 row on short
/// terminals, and 0 rows on very short terminals.
#[allow(clippy::bool_to_int_with_if)]
fn footer_height(total: u16) -> u16 {
    if total >= 12 {
        2
    } else if total >= 8 {
        1
    } else {
        0
    }
}

/// Prompt height = content lines (clamped to 1–6) + 2 border rows.
fn calc_prompt_height(text: &str, width: u16) -> u16 {
    let inner_width = usize::from(width.saturating_sub(2).max(1));
    let display = format!("> {text}");
    let content_lines = wrap_width(&display, inner_width).len().clamp(1, 6);
    u16::try_from(content_lines.saturating_add(2)).unwrap_or(8)
}

// ─── Body / Transcript rendering ─────────────────────────────────────────

/// Render the visible transcript items into styled lines.
fn render_body(app: &NeoTuiApp, body_height: usize, text_width: usize) -> Vec<String> {
    let transcript = app.transcript();
    let view = app.transcript_view();
    let theme = app.theme();

    if body_height == 0 || transcript.is_empty() {
        return Vec::new();
    }

    // `visible_range` gives us a window of *item indices* based on the
    // configured scroll offset. Each item may expand to multiple display lines.
    let range = view.visible_range(transcript, body_height);

    let mut lines = Vec::new();
    for index in range {
        let Some(item) = transcript.items().get(index) else {
            continue;
        };

        // Blank separator between items.
        if !lines.is_empty() {
            lines.push(String::new());
        }

        let styled = transcript_item_to_lines(item, theme, text_width);
        for sl in styled {
            lines.push(sl.to_ansi());
        }
    }

    lines
}

/// Convert a single [`TranscriptItem`] into zero or more [`StyledLine`]s.
///
/// This is the per-item analogue of `transcript_render_rows()` in
/// `components.rs`, but expressed entirely in [`ansi`] types.
#[allow(clippy::too_many_lines)]
fn transcript_item_to_lines(
    item: &TranscriptItem,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<StyledLine> {
    let w = text_width.max(1);

    match item {
        // ── User message ─────────────────────────────────────────────
        TranscriptItem::User { content } => {
            let base = AnsiStyle::default().fg(tc(theme.user));
            render_markdownish(content, "✨ ", base, theme, w)
        }

        // ── Assistant message ────────────────────────────────────────
        TranscriptItem::Assistant { thinking, content } => {
            let mut lines = Vec::new();
            let think_style = AnsiStyle::default().fg(tc(theme.thinking)).italic();
            let content_style = AnsiStyle::default().fg(tc(theme.assistant));

            // Thinking block (if present and non-empty).
            if let Some(thinking) = thinking.as_deref().filter(|t| !t.is_empty()) {
                let wrapped = wrap_width(thinking, w.saturating_sub(2).max(1));
                for (i, line) in wrapped.iter().enumerate() {
                    let text = if i == 0 {
                        format!("● {line}")
                    } else {
                        format!("  {line}")
                    };
                    lines.push(StyledLine::new(text, think_style));
                }
                if !content.is_empty() {
                    lines.push(StyledLine::new(String::new(), AnsiStyle::default()));
                }
            }

            // Content body.
            let prefix = if thinking.as_deref().is_none_or(str::is_empty) {
                "● "
            } else {
                ""
            };
            lines.extend(render_markdownish(content, prefix, content_style, theme, w));
            lines
        }

        // ── Tool call ────────────────────────────────────────────────
        TranscriptItem::Tool {
            name, tool_run, ..
        } => render_tool_lines(name, tool_run, theme, w),

        // ── Image ────────────────────────────────────────────────────
        TranscriptItem::Image { metadata, .. } => {
            let style = AnsiStyle::default().fg(tc(theme.notice));
            let mut lines = Vec::new();
            for line in wrap_width(metadata, w) {
                lines.push(StyledLine::new(line, style));
            }
            lines
        }

        // ── Compaction ───────────────────────────────────────────────
        TranscriptItem::Compaction {
            phase,
            percent,
            compacted_message_count,
            tokens_before,
        } => render_compaction_lines(
            *phase,
            *percent,
            *compacted_message_count,
            *tokens_before,
            theme,
            w,
        ),

        // ── Notice ───────────────────────────────────────────────────
        TranscriptItem::Notice { content } => {
            let style = AnsiStyle::default().fg(tc(theme.notice));
            let mut lines = Vec::new();
            for line in wrap_width(content, w) {
                lines.push(StyledLine::new(line, style));
            }
            lines
        }

        // ── Banner ───────────────────────────────────────────────────
        TranscriptItem::Banner {
            title,
            session_label,
            model_label,
            workspace_root,
        } => render_banner_lines(
            title,
            session_label,
            model_label,
            workspace_root,
            theme,
            w,
        ),
    }
}

/// Render markdown-ish content through [`TranscriptRenderer`] and apply styles.
///
/// `prefix` is prepended to the first non-blank display line (e.g. `"● "` for
/// assistant, `"✨ "` for user). Diff/code lines within the content get their
/// own theme colors.
fn render_markdownish(
    content: &str,
    prefix: &str,
    base: AnsiStyle,
    theme: TuiTheme,
    width: usize,
) -> Vec<StyledLine> {
    let lines = TranscriptRenderer::new(width).render_markdownish(content);
    let mut out = Vec::with_capacity(lines.len());
    let mut first_content = true;

    for tl in &lines {
        let display = tl.display_text();

        // Skip leading blanks.
        if first_content && display.is_empty() {
            continue;
        }
        first_content = false;

        let text = if !prefix.is_empty() && out.is_empty() {
            // The prefix replaces the indentation of the very first line.
            // `display_text()` may already include leading spaces (e.g. code);
            // trim them so the prefix aligns at column 0.
            format!("{prefix}{}", display.trim_start())
        } else {
            display
        };

        let style = transcript_line_style(tl, base, theme);
        out.push(StyledLine::new(text, style));
    }

    // If everything was blank, return a single empty line for spacing.
    if out.is_empty() {
        out.push(StyledLine::new(String::new(), AnsiStyle::default()));
    }

    out
}

/// Determine the [`AnsiStyle`] for a [`TranscriptLine`], with diff lines
/// getting their own theme colors and everything else inheriting `base`.
fn transcript_line_style(line: &TranscriptLine, base: AnsiStyle, theme: TuiTheme) -> AnsiStyle {
    match line {
        TranscriptLine::DiffAdded { .. } => AnsiStyle::default().fg(tc(theme.diff_added)),
        TranscriptLine::DiffRemoved { .. } => AnsiStyle::default().fg(tc(theme.diff_removed)),
        TranscriptLine::DiffHunk { .. } => AnsiStyle::default()
            .fg(tc(theme.diff_hunk))
            .bold(),
        TranscriptLine::DiffContext { .. } => AnsiStyle::default().fg(tc(theme.diff_context)),
        TranscriptLine::DiffFileHeader { marker: '+', .. } => {
            AnsiStyle::default().fg(tc(theme.diff_added))
        }
        TranscriptLine::DiffFileHeader { marker: '-', .. } => {
            AnsiStyle::default().fg(tc(theme.diff_removed))
        }
        TranscriptLine::Heading { .. } => base.bold(),
        TranscriptLine::Code { .. } => AnsiStyle::default().fg(tc(theme.notice)),
        TranscriptLine::Quote { .. } => AnsiStyle::default().italic(),
        _ => base,
    }
}

// ── Tool rendering ───────────────────────────────────────────────────────

/// Unicode symbol for each tool status.
fn tool_symbol(status: ToolStatusKind) -> &'static str {
    match status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "●",
        ToolStatusKind::Succeeded => "✓",
        ToolStatusKind::Failed => "✗",
        ToolStatusKind::Cancelled => "⊘",
    }
}

/// Past-tense verb for each tool status.
fn tool_verb(status: ToolStatusKind) -> &'static str {
    match status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "Using",
        ToolStatusKind::Succeeded => "Used",
        ToolStatusKind::Failed => "Failed",
        ToolStatusKind::Cancelled => "Cancelled",
    }
}

/// Extract the most relevant argument (path / command / pattern / …) from the
/// tool's JSON arguments string for display in the tool header.
fn tool_key_argument(arguments: Option<&str>) -> String {
    let args = match arguments {
        Some(a) => a.trim(),
        None => return String::new(),
    };
    if args.is_empty() {
        return String::new();
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(args) {
        for key in &["path", "command", "pattern", "glob", "query"] {
            if let Some(v) = value.get(key).and_then(|v| v.as_str()) {
                return one_line(v);
            }
        }
    }

    one_line(args)
}

/// Collapse all whitespace runs into single spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Short result chip appended to the tool header (e.g. `· 12 lines`).
fn tool_result_chip(name: &str, result: Option<&str>, exit_code: Option<i32>) -> String {
    let result = match result {
        Some(r) if !r.is_empty() => r,
        _ => return String::new(),
    };

    let lower = name.to_lowercase();
    if lower == "read" || lower == "write" || lower == "edit" {
        let lines = result.lines().count();
        return format!(" · {lines} lines");
    }
    if lower == "grep" {
        let matches_count = result.lines().count();
        return format!(" · {matches_count} matches");
    }
    if lower == "find" || lower == "glob" || lower == "list" {
        let files = result.lines().count();
        return format!(" · {files} files");
    }
    if lower == "bash" || lower == "shell" {
        if let Some(code) = exit_code
            && code != 0
        {
            return format!(" · exit {code}");
        }
        let bytes = result.len();
        return format!(" · {bytes} bytes");
    }

    let bytes = result.len();
    format!(" · {bytes} bytes")
}

/// Render a tool call as styled lines: header + optional body preview.
fn render_tool_lines(
    name: &str,
    tool_run: &crate::ToolRunTranscript,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<StyledLine> {
    let w = text_width.max(1);
    let symbol = tool_symbol(tool_run.status);
    let verb = tool_verb(tool_run.status);
    let key_arg = tool_key_argument(tool_run.arguments.as_deref());
    let chip = tool_result_chip(
        &tool_run.name,
        tool_run.result.as_deref(),
        tool_run.metadata.exit_code,
    );

    let header_fg = match tool_run.status {
        ToolStatusKind::Pending => tc(theme.pending),
        ToolStatusKind::Running => tc(theme.accent),
        ToolStatusKind::Succeeded => tc(theme.succeeded),
        ToolStatusKind::Failed => tc(theme.failed),
        ToolStatusKind::Cancelled => tc(theme.cancelled),
    };
    let header_style = AnsiStyle::default().fg(header_fg).bold();
    let muted_style = AnsiStyle::default().fg(tc(theme.muted));
    let body_style = if tool_run.status == ToolStatusKind::Failed {
        AnsiStyle::default().fg(tc(theme.failed))
    } else {
        AnsiStyle::default().fg(tc(theme.notice))
    };

    let mut lines = Vec::new();

    // ── Header line ─────────────────────────────────────────────────
    let mut header_parts = Vec::new();
    header_parts.push(paint(
        &format!("{symbol} {verb} "),
        header_style,
    ));
    header_parts.push(paint(&tool_run.name, header_style));
    if !key_arg.is_empty() {
        header_parts.push(paint(&format!(" ({key_arg})"), muted_style));
    }
    if !chip.is_empty() {
        header_parts.push(paint(&chip, muted_style));
    }
    let header_text = header_parts.concat();
    lines.push(StyledLine::new(header_text, AnsiStyle::default()));

    // ── Body preview ────────────────────────────────────────────────
    let detail = tool_run.display_detail();
    if !detail.is_empty() {
        let preview_limit = match name.to_lowercase().as_str() {
            "write" => 10,
            "bash" | "shell" => 6,
            _ => 3,
        };
        let detail_lines: Vec<&str> = detail.lines().collect();
        let visible_count = detail_lines.len().min(preview_limit);
        let inner_w = w.saturating_sub(4).max(1);

        for line in detail_lines.iter().take(visible_count) {
            for wrapped in wrap_width(line, inner_w) {
                lines.push(StyledLine::new(format!("  {wrapped}"), body_style));
            }
        }

        if detail_lines.len() > visible_count {
            lines.push(StyledLine::new(
                format!(
                    "  ... ({} more lines, ctrl+o to expand)",
                    detail_lines.len() - visible_count
                ),
                body_style,
            ));
        }
    }

    lines
}

// ── Compaction rendering ─────────────────────────────────────────────────

/// Render the compaction progress block.
fn render_compaction_lines(
    phase: Option<neo_agent_core::CompactionPhase>,
    percent: u8,
    compacted_message_count: usize,
    tokens_before: usize,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<StyledLine> {
    let w = text_width.max(1);
    let bar_width = 30usize.min(w.saturating_sub(7).max(8));
    let pct = percent.min(100);
    let filled = bar_width.saturating_mul(usize::from(pct)) / 100;
    let bar = format!(
        "[{}{}] {pct}%",
        "#".repeat(filled),
        ".".repeat(bar_width.saturating_sub(filled))
    );

    let tokens_str = if tokens_before >= 1_000_000 {
        format!("{}m", tokens_before / 1_000_000)
    } else if tokens_before >= 1_000 {
        format!("{}k", tokens_before / 1_000)
    } else {
        tokens_before.to_string()
    };

    let summary = format!(
        "Compacted {compacted_message_count} messages · {tokens_str} tokens before"
    );

    let muted = AnsiStyle::default().fg(tc(theme.notice));
    let progress = AnsiStyle::default().fg(tc(theme.accent));

    vec![
        StyledLine::new("  Compacting conversation...".to_owned(), muted),
        StyledLine::new(format!("  {bar}"), progress),
        StyledLine::new(format!("  {}", compaction_phase_label(phase)), muted),
        StyledLine::new(format!("  {summary}"), muted),
    ]
}

/// Human-readable label for a compaction phase.
fn compaction_phase_label(phase: Option<neo_agent_core::CompactionPhase>) -> &'static str {
    use neo_agent_core::CompactionPhase;
    match phase {
        Some(CompactionPhase::Estimating) => "Estimating context size",
        Some(CompactionPhase::SelectingBoundary) => "Selecting safe compaction boundary",
        Some(CompactionPhase::Summarizing) => "Summarizing older context",
        Some(CompactionPhase::Applying) => "Applying compacted context",
        None => "Preparing compaction",
    }
}

// ── Banner rendering ─────────────────────────────────────────────────────

/// Render the session banner as a bordered box.
fn render_banner_lines(
    title: &str,
    session_label: &str,
    model_label: &str,
    workspace_root: &std::path::Path,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<StyledLine> {
    let inner_width = text_width.saturating_sub(2).max(1);
    let border_style = AnsiStyle::default().fg(tc(theme.surface_border));
    let header_style = AnsiStyle::default()
        .fg(tc(theme.header))
        .bold();
    let muted_style = AnsiStyle::default().fg(tc(theme.muted));

    let top = format!("┌{}┐", "─".repeat(inner_width));
    let bottom = format!("└{}┘", "─".repeat(inner_width));

    let rows = [
        (format!("  {title}"), header_style),
        (format!("  Session: {session_label}"), muted_style),
        (format!("  Model: {model_label}"), muted_style),
        (
            format!("  Workspace: {}", workspace_root.display()),
            muted_style,
        ),
    ];

    let mut lines = Vec::with_capacity(rows.len() + 2);
    lines.push(StyledLine::new(top, border_style));
    for (content, style) in rows {
        let padded = pad_to_width(&clip_to_width(&content, inner_width), inner_width);
        lines.push(StyledLine::new(
            format!("│{padded}│"),
            style,
        ));
    }
    lines.push(StyledLine::new(bottom, border_style));
    lines
}

/// Clip a string to a maximum visible width (plain-text, no ANSI).
fn clip_to_width(text: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0usize;
    for c in text.chars() {
        let cw = c.width().unwrap_or(0);
        if width + cw > max_width {
            break;
        }
        result.push(c);
        width += cw;
    }
    result
}

// ── Footer rendering ─────────────────────────────────────────────────────

/// Render the footer (status bar) as 0, 1, or 2 styled lines.
fn render_footer(app: &NeoTuiApp, width: usize, footer_rows: u16) -> Vec<String> {
    if footer_rows == 0 {
        return Vec::new();
    }

    let theme = app.theme();
    let (perm_label, perm_color) = app.permission_badge();
    let perm_ansi = ratatui_color_to_ansi(perm_color);

    // ── Build the status (left) portion ─────────────────────────────
    let mut left_parts: Vec<String> = Vec::new();

    // Permission badge: [ask]
    left_parts.push(paint(
        &format!("[{perm_label}]"),
        AnsiStyle::default().fg(perm_ansi),
    ));

    // Model label (muted).
    let model = app.model_label();
    if !model.is_empty() {
        left_parts.push(paint(
            model,
            AnsiStyle::default().fg(tc(theme.muted)),
        ));
    }

    // Plan mode badge.
    if app.is_plan_mode() {
        left_parts.push(paint(
            "[PLAN MODE]",
            AnsiStyle::default()
                .fg(tc(theme.warning))
                .bold(),
        ));
    }

    // Working indicator.
    if let Some(working) = app.working_label() {
        left_parts.push(paint(
            &format!("● {working}"),
            AnsiStyle::default().fg(tc(theme.footer_working)),
        ));
    }

    // CWD label.
    left_parts.push(paint(
        &app.cwd_label(),
        AnsiStyle::default().fg(tc(theme.muted)),
    ));

    // Join with single spaces.
    let left_text = left_parts.join(" ");
    let left_width = visible_width(&left_text);

    // ── Context label (right-aligned) ───────────────────────────────
    let context_label = app.context_window_label();
    let context_ansi = ratatui_color_to_ansi(app.context_color());
    let context_styled = context_label
        .as_ref()
        .map(|label| paint(label, AnsiStyle::default().fg(context_ansi)));

    // ── Row 1: status left, context right ───────────────────────────
    let row1 = match &context_styled {
        Some(ctx) => {
            let ctx_width = visible_width(ctx);
            let total = left_width + ctx_width;
            if total < width {
                let gap = width - total;
                format!("{left_text}{}{ctx}", " ".repeat(gap))
            } else {
                // Not enough room — truncate the left part.
                let room = width.saturating_sub(ctx_width).saturating_sub(1);
                let truncated = truncate_width(&left_text, room, "", false);
                format!("{truncated} {ctx}")
            }
        }
        None => truncate_width(&left_text, width, "", false),
    };

    let mut lines = vec![row1];

    // ── Row 2: hints (only when footer_rows >= 2) ───────────────────
    if footer_rows >= 2 {
        let hint_style = AnsiStyle::default().fg(tc(theme.footer_hint));
        let hints = if width < 50 {
            "enter send · esc interrupt"
        } else {
            "enter send · shift+enter/ctrl+j newline · / commands"
        };
        lines.push(paint(hints, hint_style));
    }

    lines
}

// ── Prompt rendering ─────────────────────────────────────────────────────

/// Render the prompt input box with border and embedded cursor marker.
///
/// Returns `(lines, cursor)` where `cursor` is relative to the first line of
/// the prompt block (row 0 = top border).
fn render_prompt(app: &NeoTuiApp, width: usize) -> (Vec<String>, Option<CursorPos>) {
    let prompt = app.prompt();
    let theme = app.theme();

    let border_style = AnsiStyle::default().fg(tc(theme.overlay_border));
    let text_style = AnsiStyle::default().fg(tc(theme.prompt));

    // Inner content width (subtract left + right border).
    let inner_width = width.saturating_sub(2).max(1);

    // Build the display string with CURSOR_MARKER at the cursor position.
    // `cursor` is a *char* index into `text`.
    let display = build_prompt_display(prompt);

    // Wrap to inner_width.  `wrap_width` is ANSI-aware and correctly treats
    // CURSOR_MARKER as a zero-width escape sequence.
    let wrapped = wrap_width(&display, inner_width);
    let content_lines: Vec<String> = wrapped.into_iter().take(6).collect();

    let mut lines = Vec::with_capacity(content_lines.len() + 2);

    // ── Top border ──────────────────────────────────────────────────
    let top = format!("┌{}┐", "─".repeat(inner_width));
    lines.push(paint(&top, border_style));

    // ── Content lines with side borders ─────────────────────────────
    let left_border = paint("│", border_style);
    let right_border = paint("│", border_style);

    for cl in &content_lines {
        // Pad content to inner_width so the right border aligns.
        let content_w = visible_width(cl);
        let pad = inner_width.saturating_sub(content_w);
        let padded = format!("{cl}{}", " ".repeat(pad));
        let styled = paint(&padded, text_style);
        lines.push(format!("{left_border}{styled}{right_border}"));
    }

    // ── Bottom border ───────────────────────────────────────────────
    let bottom = format!("└{}┘", "─".repeat(inner_width));
    lines.push(paint(&bottom, border_style));

    // ── Locate cursor marker ────────────────────────────────────────
    let cursor = find_cursor(&lines);

    // ── Strip cursor marker from output ─────────────────────────────
    let clean_lines: Vec<String> = lines
        .iter()
        .map(|l| l.replace(CURSOR_MARKER, ""))
        .collect();

    (clean_lines, cursor)
}

/// Build the prompt display string with [`CURSOR_MARKER`] inserted at the
/// cursor position.
///
/// The prompt text is char-indexed; `cursor` is a char count, not a byte
/// offset.
fn build_prompt_display(prompt: &PromptState) -> String {
    let prefix = "> ";
    let chars: Vec<char> = prompt.text.chars().collect();
    let cursor = prompt.cursor.min(chars.len());

    let before: String = chars[..cursor].iter().collect();
    let after: String = chars[cursor..].iter().collect();

    format!("{prefix}{before}{CURSOR_MARKER}{after}")
}

/// Find [`CURSOR_MARKER`] in the rendered lines and compute its
/// [`CursorPos`].
///
/// The column is the *visible* width of everything before the marker on that
/// line (ANSI escapes stripped), which accounts for border characters and
/// styled prefixes.
fn find_cursor(lines: &[String]) -> Option<CursorPos> {
    for (row, line) in lines.iter().enumerate() {
        if let Some(byte_pos) = line.find(CURSOR_MARKER) {
            let col = visible_width(&line[..byte_pos]);
            return Some(CursorPos { row, col });
        }
    }
    None
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PromptState;
    use std::path::PathBuf;

    #[test]
    fn color_conversion_rgb() {
        let ansi = ratatui_color_to_ansi(ratatui::style::Color::Rgb(1, 2, 3));
        assert_eq!(ansi, AnsiColor::Rgb(1, 2, 3));
    }

    #[test]
    fn color_conversion_named() {
        assert_eq!(
            ratatui_color_to_ansi(ratatui::style::Color::Cyan),
            AnsiColor::Cyan
        );
        assert_eq!(
            ratatui_color_to_ansi(ratatui::style::Color::White),
            AnsiColor::White
        );
        assert_eq!(
            ratatui_color_to_ansi(ratatui::style::Color::Reset),
            AnsiColor::Reset
        );
    }

    #[test]
    fn footer_height_thresholds() {
        assert_eq!(footer_height(0), 0);
        assert_eq!(footer_height(7), 0);
        assert_eq!(footer_height(8), 1);
        assert_eq!(footer_height(11), 1);
        assert_eq!(footer_height(12), 2);
        assert_eq!(footer_height(100), 2);
    }

    #[test]
    fn prompt_height_minimum() {
        // Empty prompt → 1 content line + 2 borders = 3.
        assert_eq!(calc_prompt_height("", 80), 3);
    }

    #[test]
    fn prompt_display_inserts_cursor_marker() {
        let prompt = PromptState::new("hello");
        let display = build_prompt_display(&prompt);
        assert!(display.contains(CURSOR_MARKER));
        assert!(display.starts_with("> hello"));
    }

    #[test]
    fn prompt_display_cursor_at_start() {
        let prompt = PromptState::new("hello").with_cursor(0);
        let display = build_prompt_display(&prompt);
        // Marker should be right after "> ".
        assert_eq!(
            display,
            format!("> {CURSOR_MARKER}hello")
        );
    }

    #[test]
    fn prompt_display_cursor_in_middle() {
        let prompt = PromptState::new("hello").with_cursor(2);
        let display = build_prompt_display(&prompt);
        assert_eq!(
            display,
            format!("> he{CURSOR_MARKER}llo")
        );
    }

    #[test]
    fn render_app_lines_basic() {
        let app = NeoTuiApp::new(
            "Test",
            "s1",
            "gpt-4.1",
            PathBuf::from("/tmp/test"),
        );
        let (lines, cursor) = render_app_lines(&app, 80, 24);
        // Should have at least footer + prompt lines.
        assert!(!lines.is_empty());
        // Cursor should be in the prompt area.
        assert!(cursor.is_some());
        if let Some(c) = cursor {
            assert!(c.row < lines.len());
        }
    }

    #[test]
    fn render_app_lines_zero_dimensions() {
        let app = NeoTuiApp::new("T", "s", "m", PathBuf::from("/tmp"));
        let (lines, cursor) = render_app_lines(&app, 0, 0);
        assert_eq!(lines.len(), 1);
        assert!(cursor.is_none());
    }

    #[test]
    fn tool_symbol_and_verb() {
        assert_eq!(tool_symbol(ToolStatusKind::Running), "●");
        assert_eq!(tool_symbol(ToolStatusKind::Succeeded), "✓");
        assert_eq!(tool_verb(ToolStatusKind::Succeeded), "Used");
        assert_eq!(tool_verb(ToolStatusKind::Failed), "Failed");
    }

    #[test]
    fn tool_key_argument_extracts_path() {
        let args = r#"{"path": "/foo/bar.rs", "content": "x"}"#;
        assert_eq!(tool_key_argument(Some(args)), "/foo/bar.rs");
    }

    #[test]
    fn tool_key_argument_empty() {
        assert_eq!(tool_key_argument(None), "");
        assert_eq!(tool_key_argument(Some("")), "");
    }

    #[test]
    fn tool_result_chip_lines() {
        assert_eq!(
            tool_result_chip("read", Some("a\nb\nc"), None),
            " · 3 lines"
        );
        assert_eq!(
            tool_result_chip("bash", Some("output"), Some(0)),
            " · 6 bytes"
        );
        assert_eq!(
            tool_result_chip("bash", Some("output"), Some(1)),
            " · exit 1"
        );
    }

    #[test]
    fn one_line_collapses_whitespace() {
        assert_eq!(one_line("hello   world\n\nfoo"), "hello world foo");
    }

    #[test]
    fn clip_to_width_truncates() {
        assert_eq!(clip_to_width("hello", 3), "hel");
        assert_eq!(clip_to_width("hi", 10), "hi");
    }

    #[test]
    fn banner_renders_box() {
        let theme = TuiTheme::default();
        let lines = render_banner_lines(
            "My App",
            "session-1",
            "gpt-4.1",
            std::path::Path::new("/tmp/project"),
            theme,
            40,
        );
        // Top border, 4 content rows, bottom border.
        assert_eq!(lines.len(), 6);
        // First and last lines are borders.
        let first = &lines[0].text;
        assert!(first.starts_with('┌'));
        assert!(first.ends_with('┐'));
        let last = &lines[lines.len() - 1].text;
        assert!(last.starts_with('└'));
        assert!(last.ends_with('┘'));
    }

    #[test]
    fn find_cursor_locates_marker() {
        let lines = vec![
            "┌────┐".to_owned(),
            format!("│> {CURSOR_MARKER}hello│"),
            "└────┘".to_owned(),
        ];
        let cursor = find_cursor(&lines);
        assert!(cursor.is_some());
        let c = cursor.unwrap();
        assert_eq!(c.row, 1);
        // visible width of "│> " before the marker = 3.
        assert_eq!(c.col, 3);
    }
}
