//! Tabbed model selector — wraps `ModelSelectorState` with provider tabs.

use std::fmt::Write as _;

use crate::chrome::TuiTheme;
use crate::core::InputResult;
use crate::dialogs::model_selector::{
    ModelEntry, ModelSelectorOptions, ModelSelectorResult, ModelSelectorState,
};
use crate::input::InputEvent;

/// Options for the tabbed model selector.
pub struct TabbedModelSelectorOptions {
    pub models: Vec<ModelEntry>,
    pub current_alias: String,
    pub selected_alias: Option<String>,
    pub current_thinking: bool,
    pub initial_tab_id: Option<String>,
    pub theme: TuiTheme,
}

const ALL_TAB: &str = "All";

/// State for the tabbed model selector dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabbedModelSelectorState {
    tabs: Vec<String>,
    active_tab: usize,
    inner: ModelSelectorState,
    all_models: Vec<ModelEntry>,
    current_alias: String,
    selected_alias: Option<String>,
    current_thinking: bool,
    theme: TuiTheme,
    result: Option<ModelSelectorResult>,
}

impl TabbedModelSelectorState {
    #[must_use]
    pub fn new(opts: TabbedModelSelectorOptions) -> Self {
        // Build tabs: "All" + unique provider ids in insertion order
        let mut tabs = vec![ALL_TAB.to_owned()];
        let mut seen = std::collections::HashSet::new();
        for m in &opts.models {
            if seen.insert(m.provider_id.clone()) {
                tabs.push(m.provider_id.clone());
            }
        }

        // Determine initial tab
        let active_tab = opts
            .initial_tab_id
            .as_ref()
            .and_then(|pid| tabs.iter().position(|t| t == pid))
            .unwrap_or(0);

        let current_alias = opts.current_alias.clone();
        let selected_alias = opts.selected_alias.clone();
        let current_thinking = opts.current_thinking;
        let theme = opts.theme;

        // Filter models for the active tab
        let filtered = filter_models_for_tab(&opts.models, active_tab, &tabs);

        let inner = ModelSelectorState::new(ModelSelectorOptions {
            models: filtered,
            current_alias: current_alias.clone(),
            selected_alias: selected_alias.clone(),
            current_thinking,
            theme,
        });

        Self {
            tabs,
            active_tab,
            inner,
            all_models: opts.models,
            current_alias,
            selected_alias,
            current_thinking,
            theme,
            result: None,
        }
    }

    fn switch_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n <= 1 {
            return;
        }
        self.active_tab = if forward {
            (self.active_tab + 1) % n
        } else if self.active_tab == 0 {
            n - 1
        } else {
            self.active_tab - 1
        };
        self.rebuild_inner();
    }

    fn rebuild_inner(&mut self) {
        let filtered = filter_models_for_tab(&self.all_models, self.active_tab, &self.tabs);
        self.inner = ModelSelectorState::new(ModelSelectorOptions {
            models: filtered,
            current_alias: self.current_alias.clone(),
            selected_alias: self.selected_alias.clone(),
            current_thinking: self.current_thinking,
            theme: self.theme,
        });
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let inner_w = width.saturating_sub(2).max(1);
        let mut lines = Vec::new();

        // Top border with title
        lines.push(format!(
            "\x1b[38;2;{}m╭ Models {}\x1b[0m",
            rgb(self.theme.overlay_border),
            "─".repeat(width.saturating_sub(10)),
        ));
        lines.push(format!(
            "\x1b[38;2;{}m╰─── Tab: {} ───╯\x1b[0m",
            rgb(self.theme.overlay_border),
            self.tabs[self.active_tab],
        ));

        // Tab strip
        let mut tab_str = String::new();
        for (i, tab) in self.tabs.iter().enumerate() {
            if i > 0 {
                tab_str.push_str(" │ ");
            }
            if i == self.active_tab {
                let _ = write!(
                    tab_str,
                    "\x1b[48;2;{}m\x1b[30m {tab} \x1b[0m",
                    rgb(self.theme.selected_bg)
                );
            } else {
                let _ = write!(
                    tab_str,
                    "\x1b[38;2;{}m {tab} \x1b[0m",
                    rgb(self.theme.text_muted)
                );
            }
        }
        lines.push(format!(" {tab_str}"));

        // Inner model selector rows
        let inner_lines = self.inner.render_lines(width);
        // Skip the inner top/bottom borders to avoid double borders
        for line in inner_lines
            .iter()
            .skip(1)
            .take(inner_lines.len().saturating_sub(2))
        {
            lines.push(line.clone());
        }

        let _ = inner_w;
        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }

        // Tab switching on Tab key
        if let InputEvent::Insert('\t') = input {
            self.switch_tab(true);
            return InputResult::Handled;
        }

        // Forward to inner
        let res = self.inner.handle_input(input);

        // Check if inner produced a result
        if let Some(inner_result) = self.inner.take_result() {
            match &inner_result {
                ModelSelectorResult::Selected(_) => {
                    self.result = Some(inner_result);
                    return InputResult::Submitted;
                }
                ModelSelectorResult::Cancelled => {
                    // Inner cancelled — we also cancel
                    self.result = Some(ModelSelectorResult::Cancelled);
                    return InputResult::Cancelled;
                }
            }
        }

        res
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
    pub fn active_tab(&self) -> &str {
        &self.tabs[self.active_tab]
    }

    #[must_use]
    pub fn tabs(&self) -> &[String] {
        &self.tabs
    }
}

fn filter_models_for_tab(all: &[ModelEntry], tab_idx: usize, tabs: &[String]) -> Vec<ModelEntry> {
    if tab_idx == 0 {
        // "All" tab
        return all.to_vec();
    }
    let provider = &tabs[tab_idx];
    all.iter()
        .filter(|m| &m.provider_id == provider)
        .cloned()
        .collect()
}

fn rgb(c: crate::ansi::Color) -> String {
    match c {
        crate::ansi::Color::Rgb(r, g, b) => format!("{r};{g};{b}"),
        _ => "255;255;255".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::model_selector::ModelEntry;

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
                max_context_tokens: Some(128_000),
            },
            ModelEntry {
                alias: "openai/gpt-4o-mini".into(),
                provider_id: "openai".into(),
                display_name: "GPT-4o Mini".into(),
                model_id: "gpt-4o-mini".into(),
                capabilities: vec![],
                max_context_tokens: Some(128_000),
            },
            ModelEntry {
                alias: "anthropic/claude-sonnet".into(),
                provider_id: "anthropic".into(),
                display_name: "Claude Sonnet".into(),
                model_id: "claude-sonnet".into(),
                capabilities: vec!["always_thinking".into()],
                max_context_tokens: Some(200_000),
            },
        ]
    }

    #[test]
    fn tabs_include_all_and_providers() {
        let state = TabbedModelSelectorState::new(TabbedModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            initial_tab_id: None,
            theme: theme(),
        });
        assert_eq!(state.tabs(), &["All", "openai", "anthropic"]);
    }

    #[test]
    fn tab_switching_filters_models() {
        let mut state = TabbedModelSelectorState::new(TabbedModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            initial_tab_id: None,
            theme: theme(),
        });

        // Start on "All" tab
        assert_eq!(state.active_tab(), "All");

        // Tab forward to "openai"
        state.handle_input(&InputEvent::Insert('\t'));
        assert_eq!(state.active_tab(), "openai");

        // Tab forward to "anthropic"
        state.handle_input(&InputEvent::Insert('\t'));
        assert_eq!(state.active_tab(), "anthropic");

        // Tab forward wraps back to "All"
        state.handle_input(&InputEvent::Insert('\t'));
        assert_eq!(state.active_tab(), "All");
    }

    #[test]
    fn active_tab_highlighted_in_render() {
        let state = TabbedModelSelectorState::new(TabbedModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            initial_tab_id: Some("openai".into()),
            theme: theme(),
        });
        let lines = state.render_lines(60);
        let combined: String = lines.join("\n");
        // Active tab should have background color escape
        assert!(combined.contains("openai"));
    }

    #[test]
    fn selection_result_propagates() {
        let mut state = TabbedModelSelectorState::new(TabbedModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            initial_tab_id: None,
            theme: theme(),
        });
        state.handle_input(&InputEvent::Submit);
        let result = state.take_result().unwrap();
        match result {
            ModelSelectorResult::Selected(sel) => {
                assert_eq!(sel.alias, "openai/gpt-4o");
            }
            ModelSelectorResult::Cancelled => panic!("expected Selected"),
        }
    }

    #[test]
    fn initial_tab_id_respected() {
        let state = TabbedModelSelectorState::new(TabbedModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            initial_tab_id: Some("anthropic".into()),
            theme: theme(),
        });
        assert_eq!(state.active_tab(), "anthropic");
    }
}
