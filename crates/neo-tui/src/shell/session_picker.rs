use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::theme::TuiTheme;
use crate::primitive::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPickerScope {
    Workspace,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPickerItem {
    pub id: String,
    pub title: String,
    pub last_prompt: Option<String>,
    pub work_dir: PathBuf,
    pub updated_at: SystemTime,
    pub is_current: bool,
}

impl SessionPickerItem {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        last_prompt: Option<String>,
        work_dir: impl Into<PathBuf>,
        updated_at: SystemTime,
        is_current: bool,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            last_prompt,
            work_dir: work_dir.into(),
            updated_at,
            is_current,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPickerState {
    items: Vec<SessionPickerItem>,
    current_session_id: String,
    scope: SessionPickerScope,
    filter: String,
    /// Selected index into the filtered list.
    selected: usize,
    max_visible: usize,
}

impl SessionPickerState {
    #[must_use]
    pub fn new(
        items: impl IntoIterator<Item = SessionPickerItem>,
        current_session_id: impl Into<String>,
        scope: SessionPickerScope,
        max_visible: usize,
    ) -> Self {
        Self {
            items: items.into_iter().collect(),
            current_session_id: current_session_id.into(),
            scope,
            filter: String::new(),
            selected: 0,
            max_visible: max_visible.max(1),
        }
    }

    fn filtered_items(&self) -> Vec<&SessionPickerItem> {
        if self.filter.is_empty() {
            self.items.iter().collect()
        } else {
            let q = self.filter.to_lowercase();
            self.items
                .iter()
                .filter(|item| {
                    item.title.to_lowercase().contains(&q)
                        || item.id.to_lowercase().contains(&q)
                        || item
                            .last_prompt
                            .as_deref()
                            .is_some_and(|p| p.to_lowercase().contains(&q))
                })
                .collect()
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        filter.clone_into(&mut self.filter);
        self.selected = 0;
    }

    pub fn move_up(&mut self) {
        let len = self.filtered_items().len();
        if len > 0 {
            self.selected = (self.selected + len - 1) % len;
        }
    }

    pub fn move_down(&mut self) {
        let len = self.filtered_items().len();
        if len > 0 {
            self.selected = (self.selected + 1) % len;
        }
    }

    pub fn page_up(&mut self) {
        let len = self.filtered_items().len();
        if len > 0 {
            self.selected = self.selected.saturating_sub(self.max_visible);
        }
    }

    pub fn page_down(&mut self) {
        let len = self.filtered_items().len();
        if len > 0 {
            self.selected = (self.selected + self.max_visible).min(len - 1);
        }
    }

    pub fn set_scope(&mut self, scope: SessionPickerScope) {
        self.scope = scope;
        self.selected = 0;
        self.filter.clear();
    }

    #[must_use]
    pub const fn scope(&self) -> SessionPickerScope {
        self.scope
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<SessionPickerItem> {
        self.filtered_items()
            .get(self.selected)
            .map(|item| (*item).clone())
    }

    #[must_use]
    pub fn confirm(&self) -> Option<SessionPickerItem> {
        self.selected_item()
    }

    /// Render the picker as ANSI-styled lines matching the Neo card layout.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn render_lines(&self, width: usize, theme: &TuiTheme) -> Vec<String> {
        let brand = theme.brand;
        let text_muted = theme.text_muted;
        let status_ok = theme.status_ok;
        let text_color = theme.text_primary;
        let border = crate::primitive::paint(
            &"─".repeat(width),
            crate::primitive::Style::default().fg(brand),
        )
        .clone();

        let mut lines = vec![border.clone()];

        let title = match self.scope {
            SessionPickerScope::Workspace => "Sessions",
            SessionPickerScope::All => "All sessions",
        };
        let title_suffix = if self.filter.is_empty() {
            format!(
                "  {}",
                crate::primitive::paint(
                    "(type to search)",
                    crate::primitive::Style::default().fg(text_muted)
                )
            )
        } else {
            String::new()
        };
        let title_line = format!(
            "{}{}",
            crate::primitive::paint(title, crate::primitive::Style::default().fg(brand).bold()),
            title_suffix
        );
        lines.push(truncate_styled_to_width(&title_line, width));

        // Hint line. When the terminal is too narrow to hold the full hint,
        // drop the lower-priority segments (keep navigate/Enter/Esc) so the
        // line never overflows the renderer's hard width check.
        let scope_hint = match self.scope {
            SessionPickerScope::Workspace => "Ctrl+A all",
            SessionPickerScope::All => "Ctrl+A current cwd",
        };
        let hint_parts: Vec<&str> = if self.filter.is_empty() {
            vec![
                "\u{2191}\u{2193} navigate",
                scope_hint,
                "Enter select",
                "Esc cancel",
            ]
        } else {
            vec![
                "Backspace clear",
                "\u{2191}\u{2193} navigate",
                scope_hint,
                "Enter select",
                "Esc cancel",
            ]
        };
        let hint_full = crate::primitive::paint(
            &hint_parts.join(" \u{00b7} "),
            crate::primitive::Style::default().fg(text_muted),
        );
        let hint_line = if crate::primitive::visible_width(&hint_full) <= width {
            hint_full
        } else {
            // Narrow terminal: keep only the essential segments, in priority
            // order, until the budget is exhausted.
            let essential = ["\u{2191}\u{2193} navigate", "Enter select", "Esc cancel"];
            let mut joined = String::new();
            for part in essential {
                let candidate = if joined.is_empty() {
                    part.to_owned()
                } else {
                    format!("{joined} \u{00b7} {part}")
                };
                if crate::primitive::visible_width(&candidate) <= width {
                    joined = candidate;
                } else {
                    break;
                }
            }
            crate::primitive::paint(&joined, crate::primitive::Style::default().fg(text_muted))
        };
        lines.push(hint_line);

        lines.push(String::new());

        if !self.filter.is_empty() {
            let search_line = format!(
                "{}{}",
                crate::primitive::paint("Search: ", crate::primitive::Style::default().fg(brand)),
                crate::primitive::paint(
                    &self.filter,
                    crate::primitive::Style::default().fg(text_color)
                )
            );
            lines.push(truncate_styled_to_width(&search_line, width));
        }

        let filtered = self.filtered_items();
        if filtered.is_empty() {
            let msg = if self.items.is_empty() {
                "No sessions found."
            } else {
                "No matches"
            };
            lines.push(crate::primitive::paint(
                msg,
                crate::primitive::Style::default().fg(text_muted),
            ));
            lines.push(border);
            return lines;
        }

        let visible_start = (self.selected / self.max_visible) * self.max_visible;
        let visible_end = (visible_start + self.max_visible).min(filtered.len());
        for (vi, item) in filtered
            .iter()
            .enumerate()
            .take(visible_end)
            .skip(visible_start)
        {
            let is_selected = vi == self.selected;
            for card_line in Self::render_card(
                item,
                is_selected,
                width,
                brand,
                text_muted,
                status_ok,
                text_color,
            ) {
                lines.push(card_line);
            }
            if vi < visible_end - 1 {
                lines.push(String::new());
            }
        }

        // Footer
        if filtered.len() > self.max_visible || !self.filter.is_empty() {
            lines.push(String::new());
            let total_suffix = if self.filter.is_empty() {
                format!("{} sessions", filtered.len())
            } else {
                format!("{} matches", filtered.len())
            };
            let footer = format!(
                "Showing {}-{} of {}",
                visible_start + 1,
                visible_end,
                total_suffix
            );
            lines.push(crate::primitive::paint(
                &footer,
                crate::primitive::Style::default().fg(text_muted),
            ));
        }

        lines.push(border);
        lines
    }

    #[allow(clippy::too_many_arguments)]
    fn render_card(
        item: &SessionPickerItem,
        is_selected: bool,
        width: usize,
        brand: Color,
        text_muted: Color,
        status_ok: Color,
        text_color: Color,
    ) -> Vec<String> {
        let pointer = if is_selected { "\u{276f} " } else { "  " };
        let pointer_style = if is_selected {
            crate::primitive::Style::default().fg(brand)
        } else {
            crate::primitive::Style::default().fg(text_muted)
        };

        // Relative time
        let time_str = format_relative_time(item.updated_at);

        // Current badge
        let badge = if item.is_current {
            " \u{2190} current"
        } else {
            ""
        };

        // Title with inline trailing
        let title_text = if item.title.is_empty() {
            &item.id
        } else {
            &item.title
        };
        let title_style = if is_selected {
            crate::primitive::Style::default().fg(brand).bold()
        } else {
            crate::primitive::Style::default().fg(text_color)
        };

        let mut header = crate::primitive::paint(pointer, pointer_style);
        header.push_str(&crate::primitive::paint(
            &single_line(title_text),
            title_style,
        ));
        if !time_str.is_empty() {
            header.push_str("  ");
            header.push_str(&crate::primitive::paint(
                &time_str,
                crate::primitive::Style::default().fg(text_muted),
            ));
        }
        if !badge.is_empty() {
            header.push_str("  ");
            header.push_str(&crate::primitive::paint(
                badge,
                crate::primitive::Style::default().fg(status_ok),
            ));
        }

        // Truncate header to the live terminal width, display-width aware
        // so wide glyphs (CJK, emoji, full-width punctuation) do not overflow.
        let mut card = vec![truncate_styled_to_width(&header, width)];

        // Meta line: session id + work_dir
        let id_str = &item.id;
        let dir_str = home_alias(&item.work_dir);
        let indent = "  ";
        let meta_gap = "   ";
        let meta_line = format!(
            "{}{}{}{}",
            indent,
            crate::primitive::paint(id_str, crate::primitive::Style::default().fg(text_muted)),
            crate::primitive::paint(meta_gap, crate::primitive::Style::default().fg(text_muted)),
            crate::primitive::paint(&dir_str, crate::primitive::Style::default().fg(text_muted))
        );
        let meta_visible = crate::primitive::visible_width(&meta_line);
        if meta_visible <= width {
            card.push(meta_line);
        } else {
            // Wrap: id on one line, dir on next. Both must still respect the
            // terminal width — session ids are long UUIDs that can exceed a
            // narrow terminal, so left-truncate the trailing (most distinctive)
            // portion.
            let id_budget = width.saturating_sub(indent.len());
            let truncated_id = truncate_left(id_str, id_budget);
            card.push(format!(
                "{}{}",
                indent,
                crate::primitive::paint(
                    &truncated_id,
                    crate::primitive::Style::default().fg(text_muted)
                )
            ));
            let dir_budget = width.saturating_sub(indent.len());
            let truncated_dir = truncate_left(&dir_str, dir_budget);
            card.push(format!(
                "{}{}",
                indent,
                crate::primitive::paint(
                    &truncated_dir,
                    crate::primitive::Style::default().fg(text_muted)
                )
            ));
        }

        // Last prompt preview
        if let Some(prompt) = &item.last_prompt {
            let trimmed = single_line(prompt);
            if !trimmed.is_empty() {
                let marker = "\u{203a} ";
                // Budget in *display columns*, not character count: wide glyphs
                // (CJK, emoji, full-width punctuation) take 2 columns each, so
                // counting chars under-counts and can overflow the renderer's
                // hard width check.
                let prefix_width = indent.len() + marker.len();
                let budget = width.saturating_sub(prefix_width);
                let truncated = truncate_plain_to_width(&trimmed, budget);
                card.push(format!(
                    "{}{}{}",
                    indent,
                    crate::primitive::paint(
                        marker,
                        crate::primitive::Style::default().fg(text_muted)
                    ),
                    crate::primitive::paint(
                        &truncated,
                        crate::primitive::Style::default().fg(text_muted)
                    )
                ));
            }
        }

        card
    }
}

fn format_relative_time(time: SystemTime) -> String {
    let now = SystemTime::now();
    let diff = now.duration_since(time).unwrap_or_default();
    let secs = diff.as_secs();
    if secs < 60 {
        "just now".to_owned()
    } else {
        let mins = secs / 60;
        if mins < 60 {
            format!("{mins}m ago")
        } else {
            let hours = mins / 60;
            if hours < 24 {
                format!("{hours}h ago")
            } else {
                let days = hours / 24;
                format!("{days}d ago")
            }
        }
    }
}

fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn home_alias(path: &Path) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(&home);
        if let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

/// Truncate a plain (unstyled) string to at most `max_width` display columns,
/// keeping the *leading* portion and appending an ellipsis ("…") if anything
/// was cut. Wide glyphs (CJK, emoji, full-width punctuation) are counted by
/// their display width, not character count.
fn truncate_plain_to_width(s: &str, max_width: usize) -> String {
    let total = crate::primitive::visible_width(s);
    if total <= max_width {
        return s.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "\u{2026}".to_owned();
    }
    let prefix = crate::primitive::clip_plain_to_width(s, max_width - 1);
    format!("{prefix}\u{2026}")
}

/// Truncate an ANSI-styled string to at most `max_width` display columns,
/// preserving the existing escape sequences of the kept leading portion.
/// Appends a plain "…" if anything was cut. ANSI-aware so styles do not get
/// stripped, and display-width aware so wide glyphs do not overflow.
fn truncate_styled_to_width(s: &str, max_width: usize) -> String {
    let total = crate::primitive::visible_width(s);
    if total <= max_width {
        return s.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "\u{2026}".to_owned();
    }
    let prefix = crate::primitive::clip_visible_to_width(s, max_width - 1);
    format!("{prefix}\u{2026}")
}

/// Truncate a plain (unstyled) string to at most `max_width` display columns,
/// keeping the *trailing* portion and prepending an ellipsis ("…") if anything
/// was cut. Used for path-like content where the end is more informative than
/// the start. Display-width aware so wide glyphs do not overflow.
fn truncate_left(s: &str, max_width: usize) -> String {
    let total = crate::primitive::visible_width(s);
    if total <= max_width {
        return s.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "\u{2026}".to_owned();
    }
    // Reserve one column for the leading ellipsis, then keep trailing graphemes
    // until we fill the remaining budget.
    let budget = max_width - 1;
    let graphemes: Vec<&str> =
        <str as unicode_segmentation::UnicodeSegmentation>::graphemes(s, true).collect();
    let mut kept = String::new();
    let mut width = 0usize;
    for g in graphemes.iter().rev() {
        let gw = crate::primitive::visible_width(g);
        if width + gw > budget {
            break;
        }
        kept = format!("{g}{kept}");
        width += gw;
    }
    format!("\u{2026}{kept}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::visible_width;

    /// Helper: build a picker with a single item and render it at `width`.
    fn render_single(item: SessionPickerItem, width: usize) -> Vec<String> {
        let picker =
            SessionPickerState::new(vec![item], "current", SessionPickerScope::Workspace, 8);
        picker.render_lines(width, &TuiTheme::default())
    }

    /// Every line of a rendered picker must stay within the terminal width —
    /// the regression behind the original `/resume` width crash.
    fn assert_all_lines_fit(lines: &[String], width: usize) {
        for (i, line) in lines.iter().enumerate() {
            let w = visible_width(line);
            assert!(
                w <= width,
                "rendered line {i} (w={w}) exceeds terminal width {width}: {:?}",
                crate::primitive::strip_ansi(line)
            );
        }
    }

    #[test]
    fn plain_truncation_counts_wide_glyphs_by_display_width() {
        // 5 CJK chars = 10 display columns. A budget of 5 must keep 4 columns
        // of content + "…", never 5 characters (which would be 10 columns).
        let s = "你好世界你好世界";
        assert_eq!(visible_width(s), 16);
        let out = truncate_plain_to_width(s, 5);
        assert!(visible_width(&out) <= 5);
        assert!(out.ends_with('\u{2026}'));
        // ASCII path is unaffected in shape.
        let ascii = truncate_plain_to_width("hello world", 5);
        assert_eq!(ascii, "hell\u{2026}");
    }

    #[test]
    fn left_truncation_counts_wide_glyphs_by_display_width() {
        // Path-like: keep the trailing portion, leading ellipsis.
        let s = "/very/long/路径/结尾";
        let out = truncate_left(s, 8);
        assert!(visible_width(&out) <= 8);
        assert!(out.starts_with('\u{2026}'));
        assert!(out.ends_with("结尾"));
    }

    #[test]
    fn picker_renders_cjk_prompt_without_overflowing_narrow_width() {
        // Reproduces the original crash: a CJK-heavy prompt preview under a
        // narrow terminal. Before the fix this rendered at 252 cols on a
        // 238-col terminal and panicked the renderer.
        let prompt = "请对当前的修改再来一次提交和 push 建议： fix(tools): add schemars descriptions to built-in tool input schemas - Add #[schemars(description)] to Bash/Read/Write fields";
        let item = SessionPickerItem::new(
            "session_65992064-e2f2-4ed9-b9eb-bad077b460f1",
            "Splitting workspace changes into multiple commits",
            Some(prompt.to_owned()),
            "~/Workspace/neo",
            SystemTime::now(),
            false,
        );

        for width in [40usize, 60, 80, 120, 200, 238] {
            let lines = render_single(item.clone(), width);
            assert_all_lines_fit(&lines, width);
        }
    }

    #[test]
    fn picker_truncates_long_title_under_narrow_width() {
        let long_title = "A very long session title that definitely exceeds a tiny terminal width and must be clipped";
        let item = SessionPickerItem::new(
            "session_title",
            long_title.to_owned(),
            None,
            "~/Workspace/neo",
            SystemTime::now(),
            false,
        );
        let lines = render_single(item, 30);
        assert_all_lines_fit(&lines, 30);
    }
}
