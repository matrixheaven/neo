use std::ops::Range;

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Style, paint, truncate_width, visible_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl SelectItem {
    #[must_use]
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<impl Into<String>>,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectListState {
    items: Vec<SelectItem>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
    max_visible: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct VisibleSelectItem<'a> {
    pub item: &'a SelectItem,
    pub selected: bool,
}

impl SelectListState {
    #[must_use]
    pub fn new(items: impl IntoIterator<Item = SelectItem>, max_visible: usize) -> Self {
        let items = items.into_iter().collect::<Vec<_>>();
        let filtered_indices = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            selected_index: 0,
            max_visible: max_visible.max(1),
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        let filter = filter.to_lowercase();
        self.filtered_indices = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| select_item_matches(item, &filter).then_some(index))
            .collect();
        self.selected_index = 0;
    }

    #[must_use]
    pub fn filtered_len(&self) -> usize {
        self.filtered_indices.len()
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|index| self.items.get(*index))
    }

    pub fn move_up(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_index = 0;
        } else if self.selected_index == 0 {
            self.selected_index = len - 1;
        } else {
            self.selected_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_index = 0;
        } else {
            self.selected_index = (self.selected_index + 1) % len;
        }
    }

    pub fn page_up(&mut self) {
        if self.filtered_len() == 0 {
            self.selected_index = 0;
        } else {
            self.selected_index = self.selected_index.saturating_sub(self.max_visible);
        }
    }

    pub fn page_down(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_index = 0;
        } else {
            self.selected_index = (self.selected_index + self.max_visible).min(len - 1);
        }
    }

    #[must_use]
    pub fn visible_range(&self) -> Range<usize> {
        let len = self.filtered_len();
        if len == 0 {
            return 0..0;
        }

        let visible = self.max_visible.min(len);
        let half = visible / 2;
        let max_start = len.saturating_sub(visible);
        let start = self.selected_index.saturating_sub(half).min(max_start);
        start..start + visible
    }

    #[must_use]
    pub fn render_lines(&self, width: usize, theme: &TuiTheme) -> Vec<String> {
        if self.filtered_indices.is_empty() {
            let message = truncate_width("  No matching commands", width, "", false);
            return vec![paint(&message, Style::default().fg(theme.text_muted))];
        }

        let range = self.visible_range();
        let mut lines = Vec::new();
        for filtered_index in range.clone() {
            let Some(item) = self
                .filtered_indices
                .get(filtered_index)
                .and_then(|index| self.items.get(*index))
            else {
                continue;
            };
            lines.push(render_select_item(
                item,
                filtered_index == self.selected_index,
                width,
                theme,
            ));
        }

        if range.start > 0 || range.end < self.filtered_len() {
            let info = format!("  ({}/{})", self.selected_index + 1, self.filtered_len());
            let info = truncate_width(&info, width, "", false);
            lines.push(paint(&info, Style::default().fg(theme.text_muted)));
        }

        lines
    }
}

fn render_select_item(item: &SelectItem, selected: bool, width: usize, theme: &TuiTheme) -> String {
    let prefix = if selected { "> " } else { "  " };
    let label = if item.label.is_empty() {
        &item.value
    } else {
        &item.label
    };
    let prefix_width = visible_width(prefix);
    let label_style = if selected {
        Style::default()
            .fg(theme.selected_fg)
            .bg(theme.selected_bg)
            .bold()
    } else {
        Style::default().fg(theme.text_primary)
    };
    let prefix_style = if selected {
        label_style
    } else {
        Style::default().fg(theme.text_muted)
    };
    let description_style = if selected {
        Style::default().fg(theme.text_muted).bg(theme.selected_bg)
    } else {
        Style::default().fg(theme.text_muted)
    };
    let description = item
        .description
        .as_deref()
        .map(|description| description.replace(['\r', '\n'], " ").trim().to_string())
        .filter(|description| !description.is_empty());

    if let Some(description) = description.filter(|_| width > 40) {
        let primary_width = 32usize.min(width.saturating_sub(prefix_width + 4)).max(1);
        let label = truncate_width(label, primary_width.saturating_sub(2).max(1), "", false);
        let spacing = " ".repeat(primary_width.saturating_sub(visible_width(&label)).max(1));
        let used = prefix_width + visible_width(&label) + spacing.len();
        let remaining = width.saturating_sub(used + 2);
        if remaining > 10 {
            let description = truncate_width(&description, remaining, "", false);
            let spacing = if selected {
                paint(&spacing, Style::default().bg(theme.selected_bg))
            } else {
                spacing
            };
            return format!(
                "{}{}{}{}",
                paint(prefix, prefix_style),
                paint(&label, label_style),
                spacing,
                paint(&description, description_style)
            );
        }
    }

    let max_label_width = width.saturating_sub(prefix_width + 2).max(1);
    format!(
        "{}{}",
        paint(prefix, prefix_style),
        paint(
            &truncate_width(label, max_label_width, "", false),
            label_style
        )
    )
}

fn select_item_matches(item: &SelectItem, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }

    item.value.to_lowercase().contains(filter)
        || item.label.to_lowercase().contains(filter)
        || item
            .description
            .as_deref()
            .is_some_and(|description| description.to_lowercase().contains(filter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::theme::TuiTheme;

    #[test]
    fn selected_item_styles_label_and_description_separately() {
        let theme = TuiTheme::default();
        let list = SelectListState::new(
            vec![SelectItem::new("/ask", "/ask", Some("ask permission mode"))],
            8,
        );

        let line = list.render_lines(80, &theme).remove(0);
        assert!(line.contains("/ask"), "{line}");
        assert!(line.contains("ask permission mode"), "{line}");
        assert!(line.contains("\x1b["), "expected ANSI styling: {line:?}");
        let selected_label = paint(
            "/ask",
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .bold(),
        );
        let selected_description = paint(
            "ask permission mode",
            Style::default().fg(theme.text_muted).bg(theme.selected_bg),
        );
        assert!(
            line.contains(&selected_label),
            "expected selected label styling in {line:?}"
        );
        assert!(
            line.contains(&selected_description),
            "expected muted selected description styling in {line:?}"
        );
        let plain = crate::primitive::strip_ansi(&line);
        assert!(plain.starts_with("> /ask"), "{plain}");
    }

    #[test]
    fn select_list_never_invents_metadata() {
        let theme = TuiTheme::default();
        let list = SelectListState::new(
            vec![SelectItem::new("/ask", "/ask", Some("ask permission mode"))],
            8,
        );

        let plain = crate::primitive::strip_ansi(&list.render_lines(80, &theme).remove(0));
        assert!(!plain.contains("provider:"), "{plain}");
        assert!(!plain.contains("trust:"), "{plain}");
        assert!(!plain.contains("source:"), "{plain}");
    }
}
