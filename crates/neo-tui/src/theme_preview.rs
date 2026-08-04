//! Pure representative theme preview renderer.
//!
//! Renders a fixed sample of the real TUI surface (welcome banner, user and
//! assistant messages, tool status, diff roles, approval box, and the footer
//! permission/context states) with a given [`TuiTheme`] value.
//!
//! The renderer is pure by construction: it owns no transcript, chrome,
//! filesystem, config, or model-context handles and never appends events. Both
//! the theme manager preview pane and the future structured
//! `theme_draft_preview` tool card render through this one renderer so their
//! sample content cannot drift.

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style, visible_width, wrap_text};

/// Renders a representative Neo TUI surface with a `TuiTheme` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePreviewRenderer {
    theme: TuiTheme,
    width: usize,
    height: usize,
    sample_model: String,
}

impl ThemePreviewRenderer {
    #[must_use]
    pub fn new(
        theme: TuiTheme,
        width: usize,
        height: usize,
        sample_model: impl Into<String>,
    ) -> Self {
        Self {
            theme,
            width,
            height,
            sample_model: sample_model.into(),
        }
    }

    /// Render the sample surface.
    ///
    /// Returns exactly `height` rows (blank-padded); zero `width` or `height`
    /// yields an empty vector. Every row is truncated to `width` visible
    /// columns, so CJK and long sample values can never overflow the row.
    #[must_use]
    pub fn render(&self) -> Vec<String> {
        if self.width == 0 || self.height == 0 {
            return Vec::new();
        }
        let mut rows = self.sections();
        rows.truncate(self.height);
        while rows.len() < self.height {
            rows.push(String::new());
        }
        rows
    }

    /// Build the sample sections top-to-bottom. Never padded; the caller
    /// (`render`) truncates to the requested height.
    fn sections(&self) -> Vec<String> {
        let theme = &self.theme;
        let mut rows = Vec::new();

        // Welcome / banner.
        rows.push(self.fit(&Line::from_spans(vec![
            Span::styled("Neo", Style::default().fg(theme.brand).bold()),
            Span::styled(
                format!("  {}", self.sample_model),
                Style::default().fg(theme.text_primary),
            ),
        ])));
        rows.push(self.fit(&Line::styled(
            "  Welcome back · continue a session or start a new one",
            Style::default().fg(theme.text_muted),
        )));

        // User and assistant messages.
        rows.push(self.fit(&Line::styled(
            "  you  Add a two-pane theme manager with one shared preview.",
            Style::default().fg(theme.user_message),
        )));
        rows.extend(self.assistant_rows());

        // Tool status and working footer.
        rows.push(self.fit(&Line::from_spans(vec![
            Span::styled("  tool", Style::default().fg(theme.status_ok)),
            Span::styled("  bash", Style::default().fg(theme.shell_mode)),
            Span::styled(
                "  cargo test --lib",
                Style::default().fg(theme.text_primary),
            ),
        ])));
        rows.push(self.fit(&Line::styled(
            "  working · esc interrupt",
            Style::default().fg(theme.footer_working),
        )));

        // Diff roles: hunk, context, added, removed.
        rows.push(self.fit(&Line::styled(
            "  @@ -1,3 +1,4 @@  fn main() {",
            Style::default().fg(theme.diff_hunk),
        )));
        rows.push(self.fit(&Line::styled(
            "   let value = 41;",
            Style::default().fg(theme.diff_context),
        )));
        rows.push(self.fit(&Line::styled(
            "+  let value = 42;",
            Style::default().fg(theme.diff_added),
        )));
        rows.push(self.fit(&Line::styled(
            "-  let value = 41;",
            Style::default().fg(theme.diff_removed),
        )));

        // Approval border/title/selection.
        rows.push(self.fit(&Line::styled(
            "  ┌─ Approve write access?",
            Style::default().fg(theme.approval_border),
        )));
        rows.push(
            self.fit(&Line::styled(
                "  │  1 · Yes, allow once",
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .bold(),
            )),
        );
        rows.push(self.fit(&Line::styled(
            "  │  2 · No, deny",
            Style::default().fg(theme.text_primary),
        )));
        rows.push(self.fit(&Line::styled(
            "  └─",
            Style::default().fg(theme.approval_border),
        )));

        // Footer permission and context states.
        rows.push(self.footer_row());
        rows
    }

    /// Assistant message: wrapped at narrow widths, truncated otherwise.
    fn assistant_rows(&self) -> Vec<String> {
        let theme = &self.theme;
        let prefix = "  neo  ";
        let inner = self.width.saturating_sub(visible_width(prefix) + 1).max(8);
        let text = "The preview uses the selected theme; the manager chrome stays readable.";
        let style = Style::default().fg(theme.text_primary);
        let mut rows = Vec::new();
        for (index, line) in wrap_text(text, inner).into_iter().enumerate() {
            let row_prefix = if index == 0 { prefix } else { "       " };
            rows.push(self.fit(&Line::styled(format!("{row_prefix}{line}"), style)));
        }
        rows
    }

    fn footer_row(&self) -> String {
        let theme = &self.theme;
        self.fit(&Line::from_spans(vec![
            Span::styled(
                "ask",
                Style::default().fg(theme.footer_permission_ask).bold(),
            ),
            Span::styled(" 12.4k/200k", Style::default().fg(theme.footer_context_ok)),
            Span::styled("  working", Style::default().fg(theme.footer_working)),
            Span::styled("  shell off", Style::default().fg(theme.shell_mode)),
        ]))
    }

    /// Truncate a styled line to `width` visible columns. `Line` truncation
    /// preserves each span's style, so truncated sample rows never carry
    /// clipped graphemes or unbalanced escape sequences.
    fn fit(&self, line: &Line) -> String {
        line.truncate_to_width(self.width).to_ansi()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::strip_ansi;

    #[test]
    fn renderer_covers_all_representative_sections() {
        let theme = TuiTheme::default();
        let rows = ThemePreviewRenderer::new(theme, 120, 40, "openai/gpt-4.1").render();
        assert_eq!(rows.len(), 40);
        let plain = rows
            .iter()
            .map(|row| strip_ansi(row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plain.contains("Neo"), "{plain}");
        assert!(plain.contains("Welcome back"), "{plain}");
        assert!(
            plain.contains("you  Add a two-pane theme manager"),
            "{plain}"
        );
        assert!(
            plain.contains("The preview uses the selected theme"),
            "{plain}"
        );
        assert!(plain.contains("tool"), "{plain}");
        assert!(plain.contains("working · esc interrupt"), "{plain}");
        assert!(plain.contains("@@ -1,3 +1,4 @@"), "{plain}");
        assert!(plain.contains("+  let value = 42;"), "{plain}");
        assert!(plain.contains("-  let value = 41;"), "{plain}");
        assert!(plain.contains("Approve write access"), "{plain}");
        assert!(plain.contains("Yes, allow once"), "{plain}");
        assert!(plain.contains("ask"), "{plain}");
        assert!(plain.contains("12.4k/200k"), "{plain}");
    }

    #[test]
    fn renderer_never_overflows_any_width_or_height() {
        let theme = TuiTheme::default();
        for width in [0, 1, 10, 32, 60, 80, 100, 120] {
            for height in [0, 1, 3, 8, 18, 40] {
                let rows = ThemePreviewRenderer::new(theme, width, height, "model").render();
                let expected = if width == 0 || height == 0 { 0 } else { height };
                assert_eq!(rows.len(), expected, "width={width} height={height}");
                assert!(
                    rows.iter()
                        .all(|row| crate::primitive::visible_width(row) <= width),
                    "width={width} height={height}:\n{}",
                    rows.join("\n")
                );
            }
        }
    }

    #[test]
    fn renderer_truncates_long_and_cjk_sample_model() {
        let theme = TuiTheme::default();
        let model = "超级长的模型名称-abcdefghijklmnopqrstuvwxyz-0123456789";
        let rows = ThemePreviewRenderer::new(theme, 30, 6, model).render();
        assert!(rows.iter().all(|row| visible_width(row) <= 30));
        assert!(strip_ansi(&rows[0]).contains("Neo"));
    }
}
