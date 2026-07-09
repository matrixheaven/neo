//! Model selector dialog — flat searchable model list with reasoning controls.

use std::collections::BTreeMap;

use crate::dialogs::choice_picker::{dialog_rgb, dialog_sgr_bg, dialog_sgr_fg};
use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::Color;
use crate::primitive::InputResult;
use crate::primitive::theme::TuiTheme;
use crate::primitive::{truncate_width, visible_width};
use crate::searchable_list::SearchableList;
use neo_ai::{ReasoningBudget, ReasoningCapability, ReasoningEffort, ReasoningSelection};

/// One model entry in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub alias: String,
    pub provider_id: String,
    pub display_name: String,
    pub model_id: String,
    pub capabilities: Vec<String>,
    pub reasoning: ReasoningCapability,
    pub max_context_tokens: Option<u32>,
}

/// The user's selection (alias + canonical reasoning selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub alias: String,
    /// Derived compatibility flag for surfaces that still display thinking as on/off.
    pub thinking: bool,
    pub reasoning: ReasoningSelection,
}

/// Options for the model selector.
pub struct ModelSelectorOptions {
    pub models: Vec<ModelEntry>,
    pub current_alias: String,
    pub selected_alias: Option<String>,
    pub current_reasoning: ReasoningSelection,
    pub theme: TuiTheme,
}

/// Result of interacting with the model selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSelectorResult {
    Selected(ModelSelection),
    Cancelled,
}

const TITLE: &str = "Models";
const HINT: &str = "↑↓ navigate · ←→ reasoning · / filter · Enter select · Esc cancel";

/// State for the flat model selector dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelectorState {
    list: SearchableList<ModelEntry>,
    theme: TuiTheme,
    current_alias: String,
    reasoning_drafts: BTreeMap<String, ReasoningDraft>,
    result: Option<ModelSelectorResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReasoningDraft {
    selection: ReasoningSelection,
    budget_input: String,
    editing_budget: bool,
}

fn entry_search_text(entry: &ModelEntry) -> String {
    format!("{} {}", entry.display_name, entry.provider_id)
}

impl ModelSelectorState {
    #[must_use]
    pub fn new(opts: ModelSelectorOptions) -> Self {
        let initial_index = opts
            .selected_alias
            .as_ref()
            .and_then(|alias| opts.models.iter().position(|m| &m.alias == alias))
            .or_else(|| {
                opts.models
                    .iter()
                    .position(|m| m.alias == opts.current_alias)
            });

        let reasoning_drafts = opts
            .models
            .iter()
            .map(|entry| {
                (
                    entry.alias.clone(),
                    ReasoningDraft::new(entry, &opts.current_reasoning),
                )
            })
            .collect();
        let list = SearchableList::new(crate::searchable_list::SearchableListOptions {
            items: opts.models,
            to_search_text: entry_search_text,
            page_size: Some(8),
            initial_index,
            searchable: true,
        });

        Self {
            list,
            theme: opts.theme,
            current_alias: opts.current_alias,
            reasoning_drafts,
            result: None,
        }
    }

    fn selected_entry(&self) -> Option<&ModelEntry> {
        self.list.selected()
    }

    fn selected_draft(&self) -> Option<&ReasoningDraft> {
        self.selected_entry()
            .and_then(|entry| self.reasoning_drafts.get(&entry.alias))
    }

    fn selected_draft_mut(&mut self) -> Option<&mut ReasoningDraft> {
        let alias = self.selected_entry()?.alias.clone();
        self.reasoning_drafts.get_mut(&alias)
    }

    fn effective_reasoning(&self, entry: &ModelEntry) -> ReasoningSelection {
        let capability = entry_reasoning_capability(entry);
        if matches!(capability, ReasoningCapability::None) {
            return ReasoningSelection::Off;
        }
        self.selected_draft()
            .map(|draft| draft.selection.clone())
            .filter(|selection| capability.supports(selection))
            .unwrap_or_else(|| default_reasoning_selection(&capability))
    }

    fn move_reasoning(&mut self, forward: bool) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        let choices = reasoning_choices(&entry_reasoning_capability(&entry));
        if choices.is_empty() {
            return;
        }
        let current = self.effective_reasoning(&entry);
        let current_idx = choices
            .iter()
            .position(|choice| choice == &current)
            .unwrap_or(0);
        let next_idx = if forward {
            (current_idx + 1) % choices.len()
        } else if current_idx == 0 {
            choices.len() - 1
        } else {
            current_idx - 1
        };
        if let Some(draft) = self.selected_draft_mut() {
            draft.set_selection(choices[next_idx].clone());
        }
    }

    fn toggle_reasoning_off(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        let capability = entry_reasoning_capability(&entry);
        if matches!(capability, ReasoningCapability::None) {
            return;
        }
        let current = self.effective_reasoning(&entry);
        let next = if current.is_enabled() && capability.disable_supported() {
            ReasoningSelection::Off
        } else {
            default_enabled_reasoning_selection(&capability)
                .unwrap_or_else(|| default_reasoning_selection(&capability))
        };
        if let Some(draft) = self.selected_draft_mut() {
            draft.set_selection(next);
        }
    }

    fn begin_budget_edit(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let capability = entry_reasoning_capability(entry);
        let supports_budget = match &capability {
            ReasoningCapability::BudgetTokens { .. } => true,
            ReasoningCapability::Combined { effort, budget, .. } => {
                effort.is_empty() && budget.is_some()
            }
            ReasoningCapability::None
            | ReasoningCapability::Toggle { .. }
            | ReasoningCapability::Effort { .. } => false,
        };
        if !supports_budget {
            return;
        }
        if let Some(draft) = self.selected_draft_mut() {
            draft.editing_budget = true;
            draft.budget_input.clear();
            draft.selection = ReasoningSelection::BudgetTokens { budget_tokens: 0 };
        }
    }

    fn budget_error(&self, entry: &ModelEntry, draft: &ReasoningDraft) -> Option<String> {
        if !draft.editing_budget {
            return None;
        }
        let budget = draft.budget_input.parse::<u32>().ok()?;
        let bounds = budget_bounds(&entry_reasoning_capability(entry))?;
        if bounds.contains(budget) {
            return None;
        }
        Some(budget_error_text(&bounds))
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let inner_w = width.saturating_sub(2).max(1);
        let mut lines = Vec::new();

        // Top border
        lines.push(border_line(
            width,
            &BorderKind::Top,
            TITLE,
            self.theme.overlay_border,
        ));

        // Hint
        lines.push(style_line(
            &format!(" {HINT}"),
            inner_w,
            self.theme.text_muted,
            Color::Reset,
        ));

        let query = self.list.query();
        if query.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(style_line(
                &format!(" /{query}"),
                inner_w,
                self.theme.brand,
                Color::Reset,
            ));
        }

        // Model rows
        let view = self.list.view();
        let filtered: Vec<&ModelEntry> = self.list.filtered();
        let page_items = &filtered[view.start..view.end];

        for (offset, entry) in page_items.iter().enumerate() {
            let global_idx = view.start + offset;
            let is_selected = global_idx == self.list.selected_index();
            let is_current = entry.alias == self.current_alias;
            lines.push(self.render_model_row(entry, inner_w, is_selected, is_current));
        }

        // "more" indicator
        if view.end < self.list.total_filtered() {
            let remaining = self.list.total_filtered() - view.end;
            lines.push(style_line(
                &format!(" ▼ {remaining} more"),
                inner_w,
                self.theme.text_muted,
                Color::Reset,
            ));
        }

        // Reasoning controls
        if let Some(entry) = self.selected_entry() {
            lines.extend(self.render_reasoning_control(entry, inner_w));
        }

        // Bottom border
        lines.push(border_line(
            width,
            &BorderKind::Bottom,
            "",
            self.theme.overlay_border,
        ));

        lines
    }

    fn render_reasoning_control(&self, entry: &ModelEntry, width: usize) -> Vec<String> {
        let capability = entry_reasoning_capability(entry);
        let Some(draft) = self.selected_draft() else {
            return Vec::new();
        };
        match capability {
            ReasoningCapability::None => vec![style_line(
                " Reasoning: unavailable for this model",
                width,
                self.theme.text_muted,
                Color::Reset,
            )],
            ReasoningCapability::Toggle { disable_supported } => {
                let selection = self.effective_reasoning(entry);
                let mut labels = vec![segment("on", matches!(selection, ReasoningSelection::On))];
                if disable_supported {
                    labels.push(segment("off", matches!(selection, ReasoningSelection::Off)));
                }
                vec![style_line(
                    &format!(" Reasoning:  {}", labels.join("  ")),
                    width,
                    self.theme.brand,
                    Color::Reset,
                )]
            }
            ReasoningCapability::Effort {
                values,
                disable_supported,
            } => vec![style_line(
                &format!(
                    " Reasoning:  {}",
                    render_effort_segments(
                        &values,
                        disable_supported,
                        &self.effective_reasoning(entry)
                    )
                ),
                width,
                self.theme.brand,
                Color::Reset,
            )],
            ReasoningCapability::BudgetTokens { min, max, .. } => {
                self.render_budget_control(entry, draft, ReasoningBudget { min, max }, width)
            }
            ReasoningCapability::Combined {
                toggle,
                effort,
                budget,
                disable_supported,
            } => {
                if !effort.is_empty() {
                    return vec![style_line(
                        &format!(
                            " Reasoning:  {}",
                            render_effort_segments(
                                &effort,
                                disable_supported,
                                &self.effective_reasoning(entry)
                            )
                        ),
                        width,
                        self.theme.brand,
                        Color::Reset,
                    )];
                }
                if let Some(bounds) = budget {
                    return self.render_budget_control(entry, draft, bounds, width);
                }
                let selection = self.effective_reasoning(entry);
                let mut labels = Vec::new();
                if toggle {
                    labels.push(segment("on", matches!(selection, ReasoningSelection::On)));
                }
                if disable_supported {
                    labels.push(segment("off", matches!(selection, ReasoningSelection::Off)));
                }
                vec![style_line(
                    &format!(" Reasoning:  {}", labels.join("  ")),
                    width,
                    self.theme.brand,
                    Color::Reset,
                )]
            }
        }
    }

    fn render_budget_control(
        &self,
        entry: &ModelEntry,
        draft: &ReasoningDraft,
        bounds: ReasoningBudget,
        width: usize,
    ) -> Vec<String> {
        let selection = self.effective_reasoning(entry);
        let custom_selected = matches!(selection, ReasoningSelection::BudgetTokens { .. })
            && !budget_presets(&bounds).contains(&selection);
        let mut labels = Vec::new();
        if entry_reasoning_capability(entry).disable_supported() {
            labels.push(segment("off", matches!(selection, ReasoningSelection::Off)));
        }
        for preset in budget_presets(&bounds) {
            if let ReasoningSelection::BudgetTokens { budget_tokens } = preset {
                labels.push(segment(
                    &format_budget_label(budget_tokens),
                    selection == preset,
                ));
            }
        }
        labels.push(segment("custom", draft.editing_budget || custom_selected));

        let mut lines = vec![
            style_line(
                &format!(" Reasoning budget:  {}", labels.join("  ")),
                width,
                self.theme.brand,
                Color::Reset,
            ),
            style_line(
                &format!(
                    " Range: {} tokens       Custom: {}",
                    format_budget_range(&bounds),
                    draft.budget_input
                ),
                width,
                self.theme.text_muted,
                Color::Reset,
            ),
        ];
        if let Some(error) = self.budget_error(entry, draft) {
            lines.push(style_line(
                &format!(" Error: {error}"),
                width,
                self.theme.status_error,
                Color::Reset,
            ));
        }
        lines
    }

    fn render_model_row(
        &self,
        entry: &ModelEntry,
        width: usize,
        selected: bool,
        is_current: bool,
    ) -> String {
        let name_col: usize = width / 2;
        let prov_col: usize = width.saturating_sub(name_col);
        let name = truncate_width(&entry.display_name, name_col.saturating_sub(2), "…", false);
        let provider = truncate_width(&entry.provider_id, prov_col.saturating_sub(12), "…", false);

        let current_marker = if is_current { " ← current" } else { "" };

        let gap = name_col.saturating_sub(visible_width(&name));
        let row_content = format!("{name}{}  {provider}{current_marker}", " ".repeat(gap));

        let (fg, bg) = if selected {
            (self.theme.selected_fg, self.theme.selected_bg)
        } else if is_current {
            (self.theme.brand, Color::Reset)
        } else {
            (Color::Reset, Color::Reset)
        };

        let styled = format!(
            "\x1b[{};{}m {row_content}\x1b[0m",
            dialog_sgr_fg(fg),
            dialog_sgr_bg(bg)
        );
        styled
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }
        match input {
            InputEvent::Submit | InputEvent::Action(KeybindingAction::SelectConfirm) => {
                if let Some(entry) = self.selected_entry().cloned() {
                    let reasoning = self.effective_reasoning(&entry);
                    if let Some(draft) = self.selected_draft()
                        && self.budget_error(&entry, draft).is_some()
                    {
                        return InputResult::Handled;
                    }
                    self.result = Some(ModelSelectorResult::Selected(ModelSelection {
                        alias: entry.alias.clone(),
                        thinking: reasoning.is_enabled(),
                        reasoning,
                    }));
                    InputResult::Submitted
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                if self
                    .selected_draft()
                    .is_some_and(|draft| draft.editing_budget)
                {
                    if let Some(draft) = self.selected_draft_mut() {
                        draft.editing_budget = false;
                    }
                    InputResult::Handled
                } else if self.list.clear_query() {
                    InputResult::Handled
                } else {
                    self.result = Some(ModelSelectorResult::Cancelled);
                    InputResult::Cancelled
                }
            }
            InputEvent::Backspace => {
                if self
                    .selected_draft()
                    .is_some_and(|draft| draft.editing_budget)
                {
                    if let Some(draft) = self.selected_draft_mut() {
                        draft.budget_input.pop();
                        draft.sync_budget_selection();
                    }
                } else {
                    self.list.handle_key("backspace");
                }
                InputResult::Handled
            }
            InputEvent::Insert(ch) => {
                if self
                    .selected_draft()
                    .is_some_and(|draft| draft.editing_budget)
                {
                    if ch.is_ascii_digit()
                        && let Some(draft) = self.selected_draft_mut()
                    {
                        draft.budget_input.push(*ch);
                        draft.sync_budget_selection();
                    }
                } else if *ch == ' ' {
                    self.toggle_reasoning_off();
                } else if *ch == 'e' {
                    self.begin_budget_edit();
                } else {
                    self.list.handle_key(&ch.to_string());
                }
                InputResult::Handled
            }
            InputEvent::ScrollUp(1)
            | InputEvent::MoveLeft
            | InputEvent::ScrollDown(1)
            | InputEvent::MoveRight => {
                self.move_reasoning(matches!(
                    input,
                    InputEvent::ScrollDown(1) | InputEvent::MoveRight
                ));
                InputResult::Handled
            }
            InputEvent::Paste(text) => {
                if self
                    .selected_draft()
                    .is_some_and(|draft| draft.editing_budget)
                {
                    if let Some(draft) = self.selected_draft_mut() {
                        draft
                            .budget_input
                            .extend(text.chars().filter(|ch| ch.is_ascii_digit()));
                        draft.sync_budget_selection();
                    }
                } else {
                    self.list.handle_key(text);
                }
                InputResult::Handled
            }
            InputEvent::NewLine => InputResult::Ignored,
            // Arrow up/down from keybindings
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.list.move_up();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.list.move_down();
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
            _ => InputResult::Ignored,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<&ModelSelectorResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<ModelSelectorResult> {
        self.result.take()
    }

    #[must_use]
    pub(super) fn selected_reasoning_draft(&self) -> Option<(String, ReasoningSelection)> {
        let entry = self.selected_entry()?;
        Some((entry.alias.clone(), self.effective_reasoning(entry)))
    }

    pub(super) fn apply_reasoning_drafts(&mut self, drafts: &BTreeMap<String, ReasoningSelection>) {
        let items = self.list.view().items;
        for entry in items {
            let Some(selection) = drafts.get(&entry.alias) else {
                continue;
            };
            if entry_reasoning_capability(entry).supports(selection)
                && let Some(draft) = self.reasoning_drafts.get_mut(&entry.alias)
            {
                draft.set_selection(selection.clone());
            }
        }
    }
}

impl ReasoningDraft {
    fn new(entry: &ModelEntry, current_reasoning: &ReasoningSelection) -> Self {
        let capability = entry_reasoning_capability(entry);
        let selection = if capability.supports(current_reasoning) {
            current_reasoning.clone()
        } else {
            default_reasoning_selection(&capability)
        };
        let budget_input = match &selection {
            ReasoningSelection::BudgetTokens { budget_tokens } => budget_tokens.to_string(),
            ReasoningSelection::Off
            | ReasoningSelection::On
            | ReasoningSelection::Effort { .. } => String::new(),
        };
        Self {
            selection,
            budget_input,
            editing_budget: false,
        }
    }

    fn set_selection(&mut self, selection: ReasoningSelection) {
        if let ReasoningSelection::BudgetTokens { budget_tokens } = selection {
            self.budget_input = budget_tokens.to_string();
            self.selection = ReasoningSelection::BudgetTokens { budget_tokens };
        } else {
            self.selection = selection;
            self.editing_budget = false;
        }
    }

    fn sync_budget_selection(&mut self) {
        if let Ok(budget_tokens) = self.budget_input.parse::<u32>() {
            self.selection = ReasoningSelection::BudgetTokens { budget_tokens };
        }
    }
}

fn entry_reasoning_capability(entry: &ModelEntry) -> ReasoningCapability {
    if entry.reasoning.supports_reasoning() || matches!(entry.reasoning, ReasoningCapability::None)
    {
        if entry.reasoning.supports_reasoning()
            || !entry
                .capabilities
                .iter()
                .any(|capability| capability == "thinking" || capability == "reasoning")
        {
            return entry.reasoning.clone();
        }
    }
    if entry
        .capabilities
        .iter()
        .any(|capability| capability == "always_thinking")
    {
        ReasoningCapability::Toggle {
            disable_supported: false,
        }
    } else if entry
        .capabilities
        .iter()
        .any(|capability| capability == "thinking" || capability == "reasoning")
    {
        ReasoningCapability::Toggle {
            disable_supported: true,
        }
    } else {
        ReasoningCapability::None
    }
}

fn default_reasoning_selection(capability: &ReasoningCapability) -> ReasoningSelection {
    if matches!(capability, ReasoningCapability::None) || capability.disable_supported() {
        ReasoningSelection::Off
    } else {
        default_enabled_reasoning_selection(capability).unwrap_or(ReasoningSelection::Off)
    }
}

fn default_enabled_reasoning_selection(
    capability: &ReasoningCapability,
) -> Option<ReasoningSelection> {
    match capability {
        ReasoningCapability::None => None,
        ReasoningCapability::Toggle { .. } => Some(ReasoningSelection::On),
        ReasoningCapability::Effort { values, .. } => values
            .first()
            .copied()
            .map(|effort| ReasoningSelection::Effort { effort }),
        ReasoningCapability::BudgetTokens { min, max, .. } => {
            let bounds = ReasoningBudget {
                min: *min,
                max: *max,
            };
            budget_presets(&bounds).into_iter().next()
        }
        ReasoningCapability::Combined {
            toggle,
            effort,
            budget,
            ..
        } => {
            if !effort.is_empty() {
                return effort
                    .first()
                    .copied()
                    .map(|effort| ReasoningSelection::Effort { effort });
            }
            if let Some(bounds) = budget {
                return budget_presets(bounds).into_iter().next();
            }
            if *toggle {
                Some(ReasoningSelection::On)
            } else {
                None
            }
        }
    }
}

fn reasoning_choices(capability: &ReasoningCapability) -> Vec<ReasoningSelection> {
    let mut choices = Vec::new();
    if capability.disable_supported() {
        choices.push(ReasoningSelection::Off);
    }
    match capability {
        ReasoningCapability::None => {}
        ReasoningCapability::Toggle { .. } => choices.push(ReasoningSelection::On),
        ReasoningCapability::Effort { values, .. } => {
            choices.extend(
                values
                    .iter()
                    .copied()
                    .map(|effort| ReasoningSelection::Effort { effort }),
            );
        }
        ReasoningCapability::BudgetTokens { min, max, .. } => {
            choices.extend(budget_presets(&ReasoningBudget {
                min: *min,
                max: *max,
            }));
        }
        ReasoningCapability::Combined {
            toggle,
            effort,
            budget,
            ..
        } => {
            if !effort.is_empty() {
                choices.extend(
                    effort
                        .iter()
                        .copied()
                        .map(|effort| ReasoningSelection::Effort { effort }),
                );
            } else if let Some(bounds) = budget {
                choices.extend(budget_presets(bounds));
            } else if *toggle {
                choices.push(ReasoningSelection::On);
            }
        }
    }
    choices
}

fn budget_bounds(capability: &ReasoningCapability) -> Option<ReasoningBudget> {
    match capability {
        ReasoningCapability::BudgetTokens { min, max, .. } => Some(ReasoningBudget {
            min: *min,
            max: *max,
        }),
        ReasoningCapability::Combined { budget, .. } => budget.clone(),
        ReasoningCapability::None
        | ReasoningCapability::Toggle { .. }
        | ReasoningCapability::Effort { .. } => None,
    }
}

fn budget_presets(bounds: &ReasoningBudget) -> Vec<ReasoningSelection> {
    [1_024, 8_192, bounds.max.unwrap_or(24_576)]
        .into_iter()
        .filter(|budget_tokens| bounds.contains(*budget_tokens))
        .fold(Vec::new(), |mut values, budget_tokens| {
            let selection = ReasoningSelection::BudgetTokens { budget_tokens };
            if !values.contains(&selection) {
                values.push(selection);
            }
            values
        })
}

fn render_effort_segments(
    values: &[ReasoningEffort],
    disable_supported: bool,
    selection: &ReasoningSelection,
) -> String {
    let mut labels = Vec::new();
    if disable_supported {
        labels.push(segment("off", matches!(selection, ReasoningSelection::Off)));
    }
    labels.extend(values.iter().map(|effort| {
        segment(
            effort.as_str(),
            matches!(selection, ReasoningSelection::Effort { effort: selected } if selected == effort),
        )
    }));
    labels.join("  ")
}

fn segment(label: &str, selected: bool) -> String {
    if selected {
        format!("[{label}]")
    } else {
        label.to_owned()
    }
}

fn format_budget_label(budget_tokens: u32) -> String {
    if budget_tokens >= 1_000 {
        format!("{}k", budget_tokens / 1_000)
    } else {
        budget_tokens.to_string()
    }
}

fn format_budget_range(bounds: &ReasoningBudget) -> String {
    match (bounds.min, bounds.max) {
        (Some(min), Some(max)) => format!("{min}..{max}"),
        (Some(min), None) => format!("{min}.."),
        (None, Some(max)) => format!("..{max}"),
        (None, None) => "any".to_owned(),
    }
}

fn budget_error_text(bounds: &ReasoningBudget) -> String {
    match (bounds.min, bounds.max) {
        (Some(min), Some(max)) => format!("budget must be between {min} and {max} tokens"),
        (Some(min), None) => format!("budget must be at least {min} tokens"),
        (None, Some(max)) => format!("budget must be at most {max} tokens"),
        (None, None) => "budget must be a number of tokens".to_owned(),
    }
}

enum BorderKind {
    Top,
    Bottom,
}

fn border_line(width: usize, kind: &BorderKind, title: &str, color: Color) -> String {
    let left = match kind {
        BorderKind::Top => "╭",
        BorderKind::Bottom => "╰",
    };
    let right = match kind {
        BorderKind::Top => "╮",
        BorderKind::Bottom => "╯",
    };
    let fill = if matches!(kind, BorderKind::Top) && !title.is_empty() {
        let title_part = format!(" {title} ");
        let title_len = visible_width(&title_part);
        let remaining = width.saturating_sub(2 + title_len);
        format!("{title_part}{}", "─".repeat(remaining))
    } else {
        "─".repeat(width.saturating_sub(2))
    };
    format!("\x1b[38;2;{}m{left}{fill}{right}\x1b[0m", dialog_rgb(color))
}

fn style_line(text: &str, _width: usize, fg: Color, _bg: Color) -> String {
    format!("\x1b[38;2;{}m{text}\x1b[0m", dialog_rgb(fg))
}

// Re-export SearchableList from crate root for convenience
// (dialog modules import it from here)

#[cfg(test)]
mod tests {
    use super::*;
    use neo_ai::{ReasoningCapability, ReasoningEffort, ReasoningSelection};

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    fn models() -> Vec<ModelEntry> {
        vec![
            ModelEntry {
                alias: "openai/gpt-4o".into(),
                provider_id: "openai".into(),
                display_name: "GPT-4o".into(),
                model_id: "gpt-4o".into(),
                capabilities: vec!["thinking".into()],
                reasoning: ReasoningCapability::Toggle {
                    disable_supported: true,
                },
                max_context_tokens: Some(128_000),
            },
            ModelEntry {
                alias: "anthropic/claude-sonnet".into(),
                provider_id: "anthropic".into(),
                display_name: "Claude Sonnet".into(),
                model_id: "claude-sonnet".into(),
                capabilities: vec!["always_thinking".into()],
                reasoning: ReasoningCapability::Toggle {
                    disable_supported: false,
                },
                max_context_tokens: Some(200_000),
            },
            ModelEntry {
                alias: "google/gemini-flash".into(),
                provider_id: "google".into(),
                display_name: "Gemini Flash".into(),
                model_id: "gemini-flash".into(),
                capabilities: vec![],
                reasoning: ReasoningCapability::None,
                max_context_tokens: Some(1_000_000),
            },
        ]
    }

    fn reasoning_models() -> Vec<ModelEntry> {
        vec![
            ModelEntry {
                alias: "openai/gpt-reasoner".into(),
                provider_id: "openai".into(),
                display_name: "GPT Reasoner".into(),
                model_id: "gpt-reasoner".into(),
                capabilities: vec!["reasoning".into()],
                reasoning: ReasoningCapability::Effort {
                    values: vec![
                        ReasoningEffort::Low,
                        ReasoningEffort::Medium,
                        ReasoningEffort::High,
                        ReasoningEffort::XHigh,
                    ],
                    disable_supported: true,
                },
                max_context_tokens: Some(128_000),
            },
            ModelEntry {
                alias: "google/gemini-budget".into(),
                provider_id: "google".into(),
                display_name: "Gemini Budget".into(),
                model_id: "gemini-budget".into(),
                capabilities: vec!["reasoning".into()],
                reasoning: ReasoningCapability::BudgetTokens {
                    min: Some(0),
                    max: Some(24_576),
                    disable_supported: true,
                },
                max_context_tokens: Some(1_000_000),
            },
            ModelEntry {
                alias: "qwen/qwen-toggle".into(),
                provider_id: "qwen".into(),
                display_name: "Qwen Toggle".into(),
                model_id: "qwen-toggle".into(),
                capabilities: vec!["reasoning".into()],
                reasoning: ReasoningCapability::Toggle {
                    disable_supported: true,
                },
                max_context_tokens: Some(128_000),
            },
            ModelEntry {
                alias: "openai/gpt-plain".into(),
                provider_id: "openai".into(),
                display_name: "GPT Plain".into(),
                model_id: "gpt-plain".into(),
                capabilities: vec![],
                reasoning: ReasoningCapability::None,
                max_context_tokens: Some(128_000),
            },
        ]
    }

    #[test]
    fn effort_reasoning_renders_supported_values_and_returns_selection() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: reasoning_models(),
            current_alias: "openai/gpt-reasoner".into(),
            selected_alias: None,
            current_reasoning: ReasoningSelection::Effort {
                effort: ReasoningEffort::Medium,
            },
            theme: theme(),
        });

        let combined = state.render_lines(80).join("\n");
        assert!(combined.contains("Reasoning:"));
        assert!(combined.contains("off"));
        assert!(combined.contains("low"));
        assert!(combined.contains("[medium]"));
        assert!(combined.contains("xhigh"));
        assert!(!combined.contains("minimal"));
        assert!(!combined.contains("max"));

        state.handle_input(&InputEvent::MoveRight);
        state.handle_input(&InputEvent::Submit);

        assert_eq!(
            state.take_result(),
            Some(ModelSelectorResult::Selected(ModelSelection {
                alias: "openai/gpt-reasoner".to_owned(),
                thinking: true,
                reasoning: ReasoningSelection::Effort {
                    effort: ReasoningEffort::High,
                },
            }))
        );
    }

    #[test]
    fn budget_reasoning_supports_presets_custom_value_and_invalid_state() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: reasoning_models(),
            current_alias: "google/gemini-budget".into(),
            selected_alias: Some("google/gemini-budget".into()),
            current_reasoning: ReasoningSelection::BudgetTokens {
                budget_tokens: 8192,
            },
            theme: theme(),
        });

        let combined = state.render_lines(80).join("\n");
        assert!(combined.contains("Reasoning budget:"));
        assert!(combined.contains("off"));
        assert!(combined.contains("1k"));
        assert!(combined.contains("[8k]"));
        assert!(combined.contains("24k"));
        assert!(combined.contains("Range: 0..24576 tokens"));

        state.handle_input(&InputEvent::Insert('e'));
        for ch in "40000".chars() {
            state.handle_input(&InputEvent::Insert(ch));
        }

        let invalid = state.render_lines(80).join("\n");
        assert!(invalid.contains("Custom: 40000"));
        assert!(invalid.contains("budget must be between 0 and 24576 tokens"));
        assert_eq!(
            state.handle_input(&InputEvent::Submit),
            InputResult::Handled
        );
        assert!(state.result().is_none());

        for _ in 0.."40000".len() {
            state.handle_input(&InputEvent::Backspace);
        }
        for ch in "12000".chars() {
            state.handle_input(&InputEvent::Insert(ch));
        }
        state.handle_input(&InputEvent::Submit);

        assert_eq!(
            state.take_result(),
            Some(ModelSelectorResult::Selected(ModelSelection {
                alias: "google/gemini-budget".to_owned(),
                thinking: true,
                reasoning: ReasoningSelection::BudgetTokens {
                    budget_tokens: 12_000,
                },
            }))
        );
    }

    #[test]
    fn filtered_selection_preserves_selected_model_reasoning_draft() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: reasoning_models(),
            current_alias: "google/gemini-budget".into(),
            selected_alias: Some("google/gemini-budget".into()),
            current_reasoning: ReasoningSelection::BudgetTokens {
                budget_tokens: 8192,
            },
            theme: theme(),
        });

        state.handle_input(&InputEvent::Paste("Gemini".to_owned()));
        state.handle_input(&InputEvent::Submit);

        assert_eq!(
            state.take_result(),
            Some(ModelSelectorResult::Selected(ModelSelection {
                alias: "google/gemini-budget".to_owned(),
                thinking: true,
                reasoning: ReasoningSelection::BudgetTokens {
                    budget_tokens: 8192,
                },
            }))
        );
    }

    #[test]
    fn esc_exits_budget_edit_before_clearing_query_or_cancelling() {
        let budget_model = reasoning_models()
            .into_iter()
            .find(|model| model.alias == "google/gemini-budget")
            .expect("budget model");
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: vec![budget_model],
            current_alias: "google/gemini-budget".into(),
            selected_alias: Some("google/gemini-budget".into()),
            current_reasoning: ReasoningSelection::BudgetTokens {
                budget_tokens: 8192,
            },
            theme: theme(),
        });

        state.handle_input(&InputEvent::Paste("Gemini".to_owned()));
        state.handle_input(&InputEvent::Insert('e'));
        state.handle_input(&InputEvent::Insert('4'));
        assert!(
            state
                .selected_draft()
                .is_some_and(|draft| draft.editing_budget)
        );

        assert_eq!(
            state.handle_input(&InputEvent::Cancel),
            InputResult::Handled
        );
        assert_eq!(state.list.query(), "Gemini");
        assert!(
            state
                .selected_draft()
                .is_some_and(|draft| !draft.editing_budget)
        );
        assert!(state.result().is_none());

        assert_eq!(
            state.handle_input(&InputEvent::Cancel),
            InputResult::Handled
        );
        assert!(state.list.query().is_empty());
        assert!(state.result().is_none());

        assert_eq!(
            state.handle_input(&InputEvent::Cancel),
            InputResult::Cancelled
        );
        assert!(matches!(
            state.take_result(),
            Some(ModelSelectorResult::Cancelled)
        ));
    }

    #[test]
    fn toggle_and_no_reasoning_states_render_distinct_controls() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: reasoning_models(),
            current_alias: "qwen/qwen-toggle".into(),
            selected_alias: Some("qwen/qwen-toggle".into()),
            current_reasoning: ReasoningSelection::On,
            theme: theme(),
        });

        let combined = state.render_lines(80).join("\n");
        assert!(combined.contains("Reasoning:"));
        assert!(combined.contains("[on]"));
        assert!(combined.contains("off"));

        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        let unavailable = state.render_lines(80).join("\n");
        assert!(unavailable.contains("Reasoning: unavailable for this model"));

        state.handle_input(&InputEvent::Submit);
        assert_eq!(
            state.take_result(),
            Some(ModelSelectorResult::Selected(ModelSelection {
                alias: "openai/gpt-plain".to_owned(),
                thinking: false,
                reasoning: ReasoningSelection::Off,
            }))
        );
    }

    #[test]
    fn renders_title_and_rows() {
        let state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_reasoning: ReasoningSelection::Off,
            theme: theme(),
        });
        let lines = state.render_lines(60);
        let combined: String = lines.join("\n");
        assert!(combined.contains("Models"));
        assert!(combined.contains("GPT-4o"));
        assert!(combined.contains("Claude Sonnet"));
    }

    #[test]
    fn current_marker_shown() {
        let state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_reasoning: ReasoningSelection::Off,
            theme: theme(),
        });
        let lines = state.render_lines(60);
        let combined: String = lines.join("\n");
        assert!(combined.contains("← current"));
    }

    #[test]
    fn fuzzy_filter_reduces_items() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_reasoning: ReasoningSelection::Off,
            theme: theme(),
        });
        state.handle_input(&InputEvent::Insert('c'));
        state.handle_input(&InputEvent::Insert('l'));
        // Should match "Claude Sonnet"
        assert_eq!(state.list.total_filtered(), 1);
    }

    #[test]
    fn reasoning_control_respects_capabilities() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_reasoning: ReasoningSelection::Off,
            theme: theme(),
        });

        // First model (gpt-4o) supports thinking → toggles on
        let entry = state.selected_entry().cloned().unwrap();
        assert_eq!(entry.alias, "openai/gpt-4o");
        assert_eq!(state.effective_reasoning(&entry), ReasoningSelection::Off);
        state.handle_input(&InputEvent::MoveRight); // toggle
        let entry2 = state.selected_entry().cloned().unwrap();
        assert_eq!(state.effective_reasoning(&entry2), ReasoningSelection::On);

        // Second model (claude) always_thinking → stays on regardless
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        let entry3 = state.selected_entry().cloned().unwrap();
        assert_eq!(entry3.alias, "anthropic/claude-sonnet");
        assert_eq!(state.effective_reasoning(&entry3), ReasoningSelection::On);

        // Third model (gemini) no thinking → stays off
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        let entry4 = state.selected_entry().cloned().unwrap();
        assert_eq!(entry4.alias, "google/gemini-flash");
        assert_eq!(state.effective_reasoning(&entry4), ReasoningSelection::Off);
    }

    #[test]
    fn enter_returns_selected() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_reasoning: ReasoningSelection::Off,
            theme: theme(),
        });
        state.handle_input(&InputEvent::Submit);
        let result = state.take_result().unwrap();
        match result {
            ModelSelectorResult::Selected(sel) => {
                assert_eq!(sel.alias, "openai/gpt-4o");
                assert!(!sel.thinking);
                assert_eq!(sel.reasoning, ReasoningSelection::Off);
            }
            ModelSelectorResult::Cancelled => panic!("expected Selected"),
        }
    }

    #[test]
    fn esc_clears_query_then_cancels() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_reasoning: ReasoningSelection::Off,
            theme: theme(),
        });
        state.handle_input(&InputEvent::Insert('a'));
        assert!(!state.list.query().is_empty());

        state.handle_input(&InputEvent::Cancel);
        assert!(state.list.query().is_empty());
        assert!(state.result.is_none()); // first Esc just cleared

        state.handle_input(&InputEvent::Cancel);
        assert!(matches!(
            state.take_result(),
            Some(ModelSelectorResult::Cancelled)
        ));
    }
}
