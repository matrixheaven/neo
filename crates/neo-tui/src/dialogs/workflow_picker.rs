use crate::input::{InputEvent, KeybindingAction, MouseEvent, MouseKind};
use crate::primitive::InputResult;
use crate::primitive::Style;
use crate::primitive::paint;
use crate::primitive::theme::TuiTheme;
use crate::primitive::{truncate_width, visible_width, wrap_text};
use crate::screen_output::CURSOR_MARKER;
use crate::shell::{SelectItem, SelectListState};
use crate::transcript::WHEEL_SCROLL_ROWS;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowPickerItem {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub source: String,
    pub required_inputs: Vec<String>,
}

pub struct WorkflowPickerOptions {
    pub items: Vec<WorkflowPickerItem>,
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowPickerResult {
    Selected(WorkflowPickerItem),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowPickerState {
    items: Vec<WorkflowPickerItem>,
    list: SelectListState,
    query: String,
    result: Option<WorkflowPickerResult>,
    theme: TuiTheme,
}

impl WorkflowPickerState {
    #[must_use]
    pub fn new(options: WorkflowPickerOptions) -> Self {
        let list = SelectListState::new(
            options.items.iter().map(|item| {
                SelectItem::new(
                    item.name.clone(),
                    item.display_name.clone(),
                    Some(search_text(item)),
                )
            }),
            3,
        );
        Self {
            items: options.items,
            list,
            query: String::new(),
            result: None,
            theme: options.theme,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<&WorkflowPickerResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<WorkflowPickerResult> {
        self.result.take()
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub fn filtered_len(&self) -> usize {
        self.filtered_items().count()
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }
        match input {
            InputEvent::Insert(character) if !character.is_control() => {
                self.query.push(*character);
                self.list.set_filter(&self.query);
                InputResult::Handled
            }
            InputEvent::Paste(text) => {
                self.query
                    .extend(text.chars().filter(|character| !character.is_control()));
                self.list.set_filter(&self.query);
                InputResult::Handled
            }
            InputEvent::Backspace => {
                if let Some(grapheme) = self.query.graphemes(true).next_back() {
                    let new_len = self.query.len().saturating_sub(grapheme.len());
                    self.query.truncate(new_len);
                }
                self.list.set_filter(&self.query);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.list.move_up();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.list.move_down();
                InputResult::Handled
            }
            InputEvent::Mouse(MouseEvent {
                kind: MouseKind::ScrollUp,
                ..
            }) => {
                for _ in 0..WHEEL_SCROLL_ROWS {
                    self.list.move_up();
                }
                InputResult::Handled
            }
            InputEvent::Mouse(MouseEvent {
                kind: MouseKind::ScrollDown,
                ..
            }) => {
                for _ in 0..WHEEL_SCROLL_ROWS {
                    self.list.move_down();
                }
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageUp) => {
                self.list.page_up();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageDown) => {
                self.list.page_down();
                InputResult::Handled
            }
            InputEvent::Submit | InputEvent::Action(KeybindingAction::SelectConfirm) => {
                let Some(selected) = self.list.selected_item() else {
                    return InputResult::Ignored;
                };
                let Some(item) = self.items.iter().find(|item| item.name == selected.value) else {
                    return InputResult::Ignored;
                };
                self.result = Some(WorkflowPickerResult::Selected(item.clone()));
                InputResult::Submitted
            }
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                self.result = Some(WorkflowPickerResult::Cancelled);
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        if width < 4 {
            return Vec::new();
        }
        let inner_width = width.saturating_sub(2).max(1);
        let border = Style::default().fg(self.theme.overlay_border);
        let muted = Style::default().fg(self.theme.text_muted);
        let selected = Style::default()
            .fg(self.theme.selected_fg)
            .bg(self.theme.selected_bg)
            .bold();
        let normal = Style::default().fg(self.theme.text_primary);
        let body_width = inner_width.saturating_sub(2).max(1);
        let mut lines = vec![border_line("─ Run a workflow ", inner_width, border)];
        if !self.items.is_empty() {
            lines.push(box_line(
                &format!("Search  {}{CURSOR_MARKER}", self.query),
                inner_width,
                muted,
                border,
            ));
            lines.push(separator_line(inner_width, border));
        }

        if self.items.is_empty() {
            lines.push(box_line(
                "No workflows are available.",
                inner_width,
                normal,
                border,
            ));
            lines.push(box_line("", inner_width, normal, border));
            lines.push(box_line("Create one with:", inner_width, muted, border));
            lines.push(box_line(
                "/skill:create-workflow",
                inner_width,
                normal,
                border,
            ));
        } else if self.filtered_len() == 0 {
            lines.push(box_line(
                "No matching workflows.",
                inner_width,
                muted,
                border,
            ));
            lines.push(box_line("Try another search.", inner_width, muted, border));
        } else {
            let selected_name = self.list.selected_item().map(|item| item.value.as_str());
            let narrow = width < 80;
            let filtered_items = self.filtered_items().collect::<Vec<_>>();
            let selected_position = self.list.selected_position().unwrap_or_default();
            let mut body = Vec::new();
            let mut item_starts = Vec::with_capacity(filtered_items.len());
            let mut selected_line = 0;
            let mut selected_end = 0;
            for (index, item) in filtered_items.into_iter().enumerate() {
                item_starts.push(body.len());
                if index == selected_position {
                    selected_line = body.len();
                }
                body.extend(render_workflow_item_lines(
                    item,
                    WorkflowItemRenderOptions {
                        selected: selected_name == Some(item.name.as_str()),
                        narrow,
                        body_width,
                        inner_width,
                        selected_style: selected,
                        normal_style: normal,
                        muted_style: muted,
                        border_style: border,
                    },
                ));
                body.push(box_line("", inner_width, normal, border));
                if index == selected_position {
                    selected_end = body.len();
                }
            }
            let body_start = item_starts
                .into_iter()
                .take(selected_position + 1)
                .find(|start| selected_end.saturating_sub(*start) <= 10)
                .unwrap_or(selected_line);
            for line in body.into_iter().skip(body_start).take(10) {
                lines.push(line);
            }
        }
        lines.push(separator_line(inner_width, border));
        lines.push(box_line(
            if self.items.is_empty() {
                "Esc close"
            } else if self.filtered_len() == 0 {
                "Esc cancel"
            } else if width < 80 {
                "↑↓ · Enter choose · Esc cancel"
            } else {
                "↑↓ navigate · Enter choose · Esc cancel"
            },
            inner_width,
            muted,
            border,
        ));
        lines.push(bottom_border_line(inner_width, border));
        lines
    }

    fn filtered_items(&self) -> impl Iterator<Item = &WorkflowPickerItem> {
        let query = self.query.to_lowercase();
        self.items.iter().filter(move |item| {
            query.is_empty() || search_text(item).to_lowercase().contains(&query)
        })
    }
}

#[derive(Clone, Copy)]
struct WorkflowItemRenderOptions {
    selected: bool,
    narrow: bool,
    body_width: usize,
    inner_width: usize,
    selected_style: Style,
    normal_style: Style,
    muted_style: Style,
    border_style: Style,
}

fn render_workflow_item_lines(
    item: &WorkflowPickerItem,
    options: WorkflowItemRenderOptions,
) -> Vec<String> {
    let WorkflowItemRenderOptions {
        selected,
        narrow,
        body_width,
        inner_width,
        selected_style,
        normal_style,
        muted_style,
        border_style,
    } = options;
    let name_style = if selected {
        selected_style
    } else {
        normal_style
    };
    let prefix = if selected { "> " } else { "  " };
    let mut lines = vec![box_line(
        &format!("{prefix}{}", item.display_name),
        inner_width,
        name_style,
        border_style,
    )];
    lines.extend(
        wrap_text(&item.description, body_width.saturating_sub(2).max(1))
            .into_iter()
            .map(|description| {
                box_line(
                    &format!("  {description}"),
                    inner_width,
                    normal_style,
                    border_style,
                )
            }),
    );
    let required = if item.required_inputs.is_empty() {
        "None".to_owned()
    } else {
        item.required_inputs.join(", ")
    };
    if narrow {
        lines.push(box_line(
            &format!("  {}", item.source),
            inner_width,
            muted_style,
            border_style,
        ));
        lines.push(box_line(
            &format!("  Required: {required}"),
            inner_width,
            muted_style,
            border_style,
        ));
    } else {
        lines.push(box_line(
            &format!("  {} · Required: {required}", item.source),
            inner_width,
            muted_style,
            border_style,
        ));
    }
    lines
}

fn search_text(item: &WorkflowPickerItem) -> String {
    format!(
        "{} {} {} {} {}",
        item.name,
        item.display_name,
        item.description,
        item.source,
        item.required_inputs.join(" ")
    )
}

fn border_line(title: &str, width: usize, style: Style) -> String {
    let title = truncate_width(title, width.saturating_sub(2), "", false);
    let fill = "─".repeat(width.saturating_sub(visible_width(&title)));
    paint(&format!("╭{title}{fill}╮"), style)
}

fn bottom_border_line(width: usize, style: Style) -> String {
    paint(&format!("╰{}╯", "─".repeat(width)), style)
}

fn separator_line(width: usize, style: Style) -> String {
    paint(&format!("├{}╢", "─".repeat(width)), style)
}

fn box_line(content: &str, width: usize, content_style: Style, border_style: Style) -> String {
    let content = truncate_width(content, width, "…", true);
    format!(
        "{}{}{}",
        paint("│", border_style),
        paint(&content, content_style),
        paint("│", border_style)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton};

    fn item(name: &str, display_name: &str, description: &str) -> WorkflowPickerItem {
        WorkflowPickerItem {
            name: name.to_owned(),
            display_name: display_name.to_owned(),
            description: description.to_owned(),
            source: "Built-in".to_owned(),
            required_inputs: vec!["topic".to_owned()],
        }
    }

    #[test]
    fn workflow_picker_filters_and_returns_selected_name() {
        let mut picker = WorkflowPickerState::new(WorkflowPickerOptions {
            items: vec![item("deep-research", "Deep Research", "Research topics")],
            theme: TuiTheme::default(),
        });
        picker.handle_input(&InputEvent::Paste("research".to_owned()));
        assert_eq!(picker.filtered_len(), 1);
        assert_eq!(
            picker.handle_input(&InputEvent::Submit),
            InputResult::Submitted
        );
        assert!(matches!(
            picker.take_result(),
            Some(WorkflowPickerResult::Selected(item)) if item.name == "deep-research"
        ));
    }

    #[test]
    fn workflow_picker_cancel_and_empty_state_are_non_actionable() {
        let mut picker = WorkflowPickerState::new(WorkflowPickerOptions {
            items: Vec::new(),
            theme: TuiTheme::default(),
        });
        let rendered = picker
            .render_lines(80)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>();
        assert!(
            rendered
                .first()
                .is_some_and(|line| line.starts_with('╭') && line.ends_with('╮'))
        );
        assert!(
            rendered
                .last()
                .is_some_and(|line| line.starts_with('╰') && line.ends_with('╯'))
        );
        assert!(!rendered.iter().any(|line| line.contains("Search")));
        assert!(
            rendered
                .iter()
                .any(|line| line.starts_with('├') && line.ends_with('╢'))
        );
        assert!(rendered.iter().any(|line| line.contains("Esc close")));
        assert_eq!(
            picker.handle_input(&InputEvent::Submit),
            InputResult::Ignored
        );
        assert_eq!(
            picker.handle_input(&InputEvent::Cancel),
            InputResult::Cancelled
        );
        assert!(matches!(
            picker.take_result(),
            Some(WorkflowPickerResult::Cancelled)
        ));

        let mut no_match = WorkflowPickerState::new(WorkflowPickerOptions {
            items: vec![item("deep-research", "Deep Research", "Research topics")],
            theme: TuiTheme::default(),
        });
        no_match.handle_input(&InputEvent::Paste("missing".to_owned()));
        let no_match_rendered = no_match
            .render_lines(80)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>();
        assert!(no_match_rendered.iter().any(|line| line.contains("Search")));
        assert!(
            no_match_rendered
                .iter()
                .any(|line| line.contains("Esc cancel"))
        );
    }

    #[test]
    fn workflow_picker_narrow_rows_wrap_without_overflow() {
        let picker = WorkflowPickerState::new(WorkflowPickerOptions {
            items: vec![item(
                "long",
                "Long workflow",
                "A description that must wrap cleanly at a narrow terminal width",
            )],
            theme: TuiTheme::default(),
        });
        for line in picker.render_lines(40) {
            assert!(visible_width(&line) <= 40, "{line:?}");
        }
    }

    #[test]
    fn workflow_picker_scroll_and_backspace_follow_visible_items_and_graphemes() {
        let mut picker = WorkflowPickerState::new(WorkflowPickerOptions {
            items: vec![
                item("first", "First", ""),
                item("second", "Second", ""),
                item("third", "Third", ""),
                item("fourth", "Fourth", ""),
            ],
            theme: TuiTheme::default(),
        });

        picker.handle_input(&InputEvent::Mouse(MouseEvent {
            kind: MouseKind::ScrollDown,
            button: MouseButton::Left,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        }));
        let rendered = picker.render_lines(40).join("\n");
        assert!(rendered.contains("Fourth"), "{rendered}");
        assert!(rendered.lines().count() <= 16);

        picker.handle_input(&InputEvent::Paste("🇺🇸".to_owned()));
        picker.handle_input(&InputEvent::Backspace);
        assert_eq!(picker.query(), "");
    }

    #[test]
    fn workflow_picker_keeps_the_first_item_visible_while_the_selection_fits() {
        let mut picker = WorkflowPickerState::new(WorkflowPickerOptions {
            items: vec![
                item("first", "First", "First description"),
                item("second", "Second", "Second description"),
                item("third", "Third", "Third description"),
                item("fourth", "Fourth", "Fourth description"),
            ],
            theme: TuiTheme::default(),
        });

        picker.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        let rendered = picker
            .render_lines(120)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("First"), "{rendered}");
        assert!(rendered.contains("> Second"), "{rendered}");

        picker.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        let rendered = picker
            .render_lines(120)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("First"), "{rendered}");
        assert!(rendered.contains("Second"), "{rendered}");
        assert!(rendered.contains("> Third"), "{rendered}");

        picker.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        let rendered = picker
            .render_lines(120)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("Second"), "{rendered}");
        assert!(rendered.contains("Third"), "{rendered}");
        assert!(rendered.contains("> Fourth"), "{rendered}");
    }
}
