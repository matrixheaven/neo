//! Extracted: model picker — open the model selector dialog, resolve model
//! entries from config, and map picker items back to context-window info.

use crate::config::{self, AppConfig};
use crate::modes::sessions::{SessionPickerScope as SessionDataScope, session_summaries};

use neo_tui::shell::PickerItem;

use super::InteractiveController;
use super::PickerCatalogs;

pub(super) struct SessionCatalog {
    pub(super) items: Vec<neo_agent_core::session::SessionSummary>,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelPickerCatalog {
    pub(super) items: Vec<PickerItem>,
    pub(super) error: Option<String>,
}

pub(super) fn picker_catalogs_for_config(config: &AppConfig) -> PickerCatalogs {
    let sessions = session_catalog_for_config(config);
    let models = model_picker_catalog_for_config(config);
    PickerCatalogs {
        session_items: sessions.items,
        session_error: sessions.error,
        model_items: models.items,
    }
}

pub(super) fn session_catalog_for_config(config: &AppConfig) -> SessionCatalog {
    match session_summaries(config, SessionDataScope::Workspace) {
        Ok(items) => SessionCatalog { items, error: None },
        Err(error) => SessionCatalog {
            items: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

pub(super) fn model_picker_catalog_for_config(config: &AppConfig) -> ModelPickerCatalog {
    if !config.models.is_empty() {
        return ModelPickerCatalog {
            items: model_picker_items_from_config(config),
            error: None,
        };
    }
    match crate::modes::run::model_registry_for_config(config) {
        Ok(registry) => {
            let models = registry.list();
            let models = config::scoped_models(models.iter(), &config.model_scope);
            ModelPickerCatalog {
                items: models.iter().map(model_to_picker_item).collect(),
                error: None,
            }
        }
        Err(error) => ModelPickerCatalog {
            items: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

pub(super) fn model_picker_items_from_config(config: &AppConfig) -> Vec<PickerItem> {
    config
        .models
        .iter()
        .map(|(alias, model)| {
            let description = model.max_context_tokens.map_or_else(
                || model.provider.clone(),
                |max_context_tokens| format!("{} · ctx {max_context_tokens}", model.provider),
            );
            PickerItem::new(alias.clone(), alias.clone(), Some(description))
        })
        .collect()
}

pub(super) fn model_to_picker_item(model: &neo_ai::ModelSpec) -> PickerItem {
    let value = format!("{}/{}", model.provider.0, model.model);
    let description = match model.capabilities.max_context_tokens {
        Some(max_context_tokens) => {
            format!("{:?} · ctx {max_context_tokens}", model.api)
        }
        None => format!("{:?}", model.api),
    };
    PickerItem::new(value.clone(), value, Some(description))
}

/// Build `ModelEntry` list directly from `[models.*]` in config, falling back to
/// the seeded model registry when no inline models are configured so the picker
/// is still usable before the user has created a config file.
pub(super) fn model_entries_from_config(config: &AppConfig) -> Vec<neo_tui::dialogs::ModelEntry> {
    if !config.models.is_empty() {
        return config
            .models
            .iter()
            .map(|(alias, model)| {
                let provider_id = model.provider.clone();
                let mut capabilities = model.capabilities.clone();
                if capabilities.iter().any(|c| c == "reasoning")
                    && !capabilities.iter().any(|c| c == "thinking")
                {
                    capabilities.push("thinking".to_owned());
                }
                neo_tui::dialogs::ModelEntry {
                    alias: alias.clone(),
                    provider_id,
                    display_name: model.display_name.clone().unwrap_or_else(|| alias.clone()),
                    model_id: model.model.clone(),
                    capabilities,
                    reasoning: model.reasoning.clone(),
                    max_context_tokens: model.max_context_tokens,
                }
            })
            .collect();
    }
    match crate::modes::run::model_registry_for_config(config) {
        Ok(registry) => registry
            .list()
            .iter()
            .map(model_spec_to_model_entry)
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn capabilities_from_model_capabilities(capabilities: &neo_ai::ModelCapabilities) -> Vec<String> {
    let mut caps = Vec::new();
    if capabilities.streaming {
        caps.push("streaming".to_owned());
    }
    if capabilities.tools {
        caps.push("tools".to_owned());
    }
    if capabilities.images {
        caps.push("images".to_owned());
    }
    if capabilities.supports_reasoning() {
        caps.push("reasoning".to_owned());
    }
    if capabilities.embeddings {
        caps.push("embeddings".to_owned());
    }
    caps
}

fn model_spec_to_model_entry(model: &neo_ai::ModelSpec) -> neo_tui::dialogs::ModelEntry {
    let provider_id = model.provider.0.clone();
    let alias = format!("{}/{}", provider_id, model.model);
    let mut capabilities = capabilities_from_model_capabilities(&model.capabilities);
    if capabilities.iter().any(|c| c == "reasoning")
        && !capabilities.iter().any(|c| c == "thinking")
    {
        capabilities.push("thinking".to_owned());
    }
    neo_tui::dialogs::ModelEntry {
        alias: alias.clone(),
        provider_id,
        display_name: alias,
        model_id: model.model.clone(),
        capabilities,
        reasoning: model.capabilities.reasoning.clone(),
        max_context_tokens: model.capabilities.max_context_tokens,
    }
}

pub(super) fn context_window_from_picker_item(item: &PickerItem) -> Option<u32> {
    let description = item.description.as_deref()?;
    let (_, context) = description.rsplit_once("ctx ")?;
    parse_token_count(context.trim())
}

fn parse_token_count(value: &str) -> Option<u32> {
    let value = value.trim().to_ascii_lowercase();
    let (number, multiplier) = match value.strip_suffix('m') {
        Some(number) => (number, 1_000_000u32),
        None => match value.strip_suffix('k') {
            Some(number) => (number, 1_000u32),
            None => (value.as_str(), 1u32),
        },
    };
    number
        .parse::<u32>()
        .ok()
        .and_then(|count| count.checked_mul(multiplier))
}

impl InteractiveController {
    pub(super) fn open_model_picker(&mut self) {
        let Some(config) = &self.local_config else {
            self.push_status("No config available");
            return;
        };
        let entries = model_entries_from_config(config);
        let current_alias = self
            .active_model
            .as_ref()
            .map(|m| format!("{}/{}", m.provider, m.model))
            .unwrap_or_default();
        let theme = self.tui.chrome().theme();
        self.tui.chrome_mut().open_tabbed_model_selector(
            neo_tui::dialogs::TabbedModelSelectorOptions {
                models: entries,
                current_alias,
                selected_alias: None,
                current_reasoning: self.current_reasoning.clone(),
                initial_tab_id: None,
                theme,
            },
        );
    }

    /// Open the model picker with a specific alias pre-selected.
    pub(super) fn open_model_picker_with_alias(&mut self, alias: &str) {
        let Some(config) = &self.local_config else {
            self.push_status("No config available");
            return;
        };
        let entries = model_entries_from_config(config);
        let current_alias = self
            .active_model
            .as_ref()
            .map(|m| format!("{}/{}", m.provider, m.model))
            .unwrap_or_default();
        let initial_tab_id = entries
            .iter()
            .find(|e| e.alias == alias)
            .map(|e| e.provider_id.clone());
        let theme = self.tui.chrome().theme();
        self.tui.chrome_mut().open_tabbed_model_selector(
            neo_tui::dialogs::TabbedModelSelectorOptions {
                models: entries,
                current_alias,
                selected_alias: Some(alias.to_owned()),
                current_reasoning: self.current_reasoning.clone(),
                initial_tab_id,
                theme,
            },
        );
    }
}
