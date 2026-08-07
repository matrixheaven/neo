use crate::markdown::{highlight_code_lines, render_markdown, wrap_spans};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style, truncate_to_width, visible_width};

/// Uniform left margin so the plan box aligns with other tool-card children.
const LEFT_MARGIN: usize = 2;
/// Space between the side border and the content on each side.
const SIDE_PADDING: usize = 1;

/// Renders plan content inside a bordered box, displayed within the
/// `ExitPlanMode` tool card.
#[derive(Debug, Clone)]
pub struct PlanBoxComponent {
    content: String,
    path: Option<String>,
    status: Option<String>,
    source_language: Option<&'static str>,
}

impl PlanBoxComponent {
    #[must_use]
    pub fn new(content: impl Into<String>, path: Option<String>) -> Self {
        Self {
            content: content.into(),
            path,
            status: None,
            source_language: None,
        }
    }

    /// Render source without Markdown/plain-text whitespace normalization.
    #[must_use]
    pub fn source(content: impl Into<String>, language: &'static str) -> Self {
        Self {
            content: content.into(),
            path: None,
            status: None,
            source_language: Some(language),
        }
    }

    /// Set a status suffix (e.g. "Rejected") shown in the title bar.
    #[must_use]
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// Render the plan box as styled lines, fitting within `width` columns.
    #[must_use]
    pub fn render(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        if width < LEFT_MARGIN + 4 {
            return vec![];
        }

        let border_style = Style::default().fg(theme.status_ok);
        let content_style = Style::default().fg(theme.text_primary);
        let muted_style = Style::default().fg(theme.text_muted);

        let indent = " ".repeat(LEFT_MARGIN);

        // Box layout with a uniform left margin:
        // "  ┌──...──┐"
        // "  │ content │"
        // "  └──...──┘"
        // width = LEFT_MARGIN + 1 (corner/border) + horz_len + 1 (corner/border)
        let horz_len = width.saturating_sub(LEFT_MARGIN + 2).max(2);
        let content_width = horz_len.saturating_sub(2 * SIDE_PADDING).max(1);

        // Title
        let title = if let Some(language) = self.source_language {
            let total = self.content.split('\n').count().max(1);
            format!(" {language} source · lines 1-{total} / {total} ")
        } else {
            let basename = self.path.as_deref().map_or("plan", display_basename);
            if let Some(status) = &self.status {
                format!(" plan: {basename} · {status} ")
            } else {
                format!(" plan: {basename} ")
            }
        };

        let mut lines = vec![Self::titled_border(&indent, &title, horz_len, border_style)];

        // Content lines — render as markdown if the file is .md, plain text otherwise
        let is_markdown = self
            .path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).extension())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if let Some(language) = self.source_language {
            let path = format!("workflow.{language}");
            for logical_line in highlight_code_lines(&self.content, &path, theme) {
                for visual_line in wrap_spans(&logical_line, content_width) {
                    let padding = " ".repeat(
                        content_width
                            .saturating_sub(visual_line.iter().map(Span::visible_width).sum()),
                    );
                    let mut spans = vec![
                        Span::styled(indent.clone(), Style::default()),
                        Span::styled("\u{2502} ", border_style),
                    ];
                    spans.extend(visual_line);
                    spans.push(Span::styled(padding, Style::default()));
                    spans.push(Span::styled(" \u{2502}", border_style));
                    lines.push(Line::from_spans(spans));
                }
            }
        } else if is_markdown && !self.content.trim().is_empty() {
            let md_lines = render_markdown(&self.content, content_width, theme, "", "");
            for md_line in md_lines {
                let visible_w = md_line.visible_width();
                let padding = " ".repeat(content_width.saturating_sub(visible_w));
                let mut spans = vec![
                    Span::styled(indent.clone(), Style::default()),
                    Span::styled("\u{2502} ", border_style),
                ];
                spans.extend(md_line.into_spans());
                spans.push(Span::styled(padding, Style::default()));
                spans.push(Span::styled(" \u{2502}", border_style));
                lines.push(Line::from_spans(spans));
            }
        } else {
            for raw_line in self.content.lines() {
                for chunk in Self::wrap_text(raw_line, content_width) {
                    let padded = Self::pad_to(&chunk, content_width);
                    lines.push(Line::from_spans(vec![
                        Span::styled(indent.clone(), Style::default()),
                        Span::styled("\u{2502} ", border_style),
                        Span::styled(padded, content_style),
                        Span::styled(" \u{2502}", border_style),
                    ]));
                }
            }
            if self.content.trim().is_empty() {
                let padded = " ".repeat(content_width);
                lines.push(Line::from_spans(vec![
                    Span::styled(indent.clone(), Style::default()),
                    Span::styled("\u{2502} ", border_style),
                    Span::styled(padded, muted_style),
                    Span::styled(" \u{2502}", border_style),
                ]));
            }
        }

        // Bottom border
        let bottom_inner = "\u{2500}".repeat(horz_len);
        lines.push(Line::from_spans(vec![
            Span::styled(indent, Style::default()),
            Span::styled(format!("\u{2514}{bottom_inner}"), border_style),
            Span::styled("\u{2519}", border_style),
        ]));

        lines
    }

    fn titled_border(indent: &str, title: &str, horz_len: usize, border_style: Style) -> Line {
        let title_width = visible_width(title);
        let title_fitted = if title_width <= horz_len {
            format!("{title}{}", "\u{2500}".repeat(horz_len - title_width))
        } else {
            truncate_to_width(title, horz_len)
        };
        Line::from_spans(vec![
            Span::styled(indent.to_owned(), Style::default()),
            Span::styled(format!("\u{250c}{title_fitted}\u{2510}"), border_style),
        ])
    }

    fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        if text.is_empty() {
            return vec![String::new()];
        }
        let mut result = Vec::new();
        let mut current = String::new();
        let mut current_len = 0usize;
        for word in text.split_whitespace() {
            let word_len = word.chars().count();
            if current_len == 0 {
                current = word.to_string();
                current_len = word_len;
            } else if current_len + 1 + word_len <= max_width {
                current.push(' ');
                current.push_str(word);
                current_len += 1 + word_len;
            } else {
                result.push(std::mem::take(&mut current));
                current = word.to_string();
                current_len = word_len;
            }
        }
        if !current.is_empty() {
            result.push(current);
        }
        if result.is_empty() {
            result.push(String::new());
        }
        result
    }

    fn pad_to(text: &str, width: usize) -> String {
        let char_count = text.chars().count();
        if char_count >= width {
            text.chars().take(width).collect()
        } else {
            let mut result = text.to_string();
            result.push_str(&" ".repeat(width - char_count));
            result
        }
    }
}

fn display_basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("plan")
}

#[cfg(test)]
#[path = "test_cases/render.rs"]
mod render;
