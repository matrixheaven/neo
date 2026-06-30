//! Extracted: catalog fetch/import — fetch models.dev catalog, import custom
//! registries, handle API-key submission, and drive the background fetch tasks.

use std::collections::BTreeMap;
use std::path::PathBuf;

use neo_tui::dialogs::{
    ApiKeyInputOptions, ApiKeyInputResult, ChoiceItem, ChoicePickerOptions,
    CustomRegistryImportOptions, CustomRegistryImportResult,
};

use super::InteractiveController;

pub(super) type CatalogEntries = BTreeMap<String, neo_ai::catalog::CatalogEntry>;

pub(super) struct PendingCustomRegistry {
    pub(super) source: neo_tui::dialogs::CustomRegistrySource,
    pub(super) catalog: CatalogEntries,
}

pub(super) enum CatalogFetchSource {
    Known,
    Custom(neo_tui::dialogs::CustomRegistrySource),
}

/// When set, the fetched catalog should be used to write a provider into config
/// (the API-key submit path) instead of opening a provider picker.
#[derive(Clone)]
pub(super) struct PendingCatalogAdd {
    pub(super) provider_id: String,
    pub(super) api_key: Option<String>,
    pub(super) config_path: PathBuf,
}

pub(super) struct PendingCatalogFetch {
    pub(super) source: CatalogFetchSource,
    pub(super) handle: tokio::task::JoinHandle<Result<CatalogEntries, neo_ai::error::AiError>>,
    pub(super) pending_add: Option<PendingCatalogAdd>,
}

/// Build choice-picker items from a fetched catalog.
pub(super) fn catalog_choice_items(catalog: &CatalogEntries) -> Vec<ChoiceItem> {
    catalog
        .iter()
        .map(|(id, entry)| {
            let label = entry.name.clone().unwrap_or_else(|| id.clone());
            let description = entry.api.clone().unwrap_or_default();
            ChoiceItem::new(format!("catalog:{id}"), label).with_description(description)
        })
        .collect()
}

/// Rewrap catalog choice items for the custom-registry picker.
pub(super) fn custom_catalog_choice_items(items: Vec<ChoiceItem>) -> Vec<ChoiceItem> {
    items
        .into_iter()
        .map(|mut item| {
            item.id = item.id.replacen("catalog:", "custom-catalog:", 1);
            item
        })
        .collect()
}

impl InteractiveController {
    pub(super) fn fetch_known_catalog(&mut self) {
        self.tui
            .chrome_mut()
            .set_custom_working_label(Some("Fetching models.dev catalog...".to_owned()));
        let _handle = tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
        let handle = tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
        self.pending_catalog_fetch = Some(PendingCatalogFetch {
            source: CatalogFetchSource::Known,
            handle,
            pending_add: None,
        });
    }

    pub(super) fn open_custom_registry_import(&mut self) {
        self.tui
            .chrome_mut()
            .open_custom_registry_import(CustomRegistryImportOptions {
                title: "Import Custom Registry".to_owned(),
            });
    }

    pub(super) fn open_catalog_api_key_input(&mut self, provider_id: &str) {
        self.pending_catalog_provider_id = Some(provider_id.to_owned());
        self.tui
            .chrome_mut()
            .open_api_key_input(ApiKeyInputOptions {
                title: "API Key".to_owned(),
                provider_name: provider_id.to_owned(),
            });
    }

    pub(super) fn import_custom_catalog_provider(&mut self, provider_id: &str) {
        let Some(pending) = self.pending_custom_registry.take() else {
            return;
        };
        let Some(entry) = pending.catalog.get(provider_id) else {
            self.push_status(format!(
                "Error: Provider '{provider_id}' not found in registry"
            ));
            return;
        };
        let Some(config_path) = self.config_path() else {
            self.push_status("No config available");
            return;
        };
        match crate::config::mutations::add_provider_from_catalog_entry(
            &config_path,
            provider_id,
            entry,
            Some(&pending.source.token),
            None,
        ) {
            Ok(message) => {
                self.push_status(message);
                self.refresh_config();
            }
            Err(error) => {
                self.push_status(format!("Error: Failed to import provider: {error}"));
            }
        }
    }

    /// Handle an API key input result.
    pub(super) fn handle_api_key_input_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().api_key_input_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            ApiKeyInputResult::Submitted(key) => {
                self.handle_api_key_submitted(&key);
            }
            ApiKeyInputResult::Cancelled => {
                self.pending_catalog_provider_id = None;
            }
        }
    }

    pub(super) fn handle_api_key_submitted(&mut self, key: &str) {
        let Some(provider_id) = self.pending_catalog_provider_id.take() else {
            self.push_status("API key saved.");
            return;
        };
        let Some(config_path) = self.config_path() else {
            self.push_status("No config available");
            return;
        };
        // Fetch the catalog off the main loop so the footer spinner can animate
        // instead of freezing the UI for the duration of the network request.
        self.tui
            .chrome_mut()
            .set_custom_working_label(Some(format!("Importing provider {provider_id}...")));
        let _handle = tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
        let handle = tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
        self.pending_catalog_fetch = Some(PendingCatalogFetch {
            source: CatalogFetchSource::Known,
            handle,
            pending_add: Some(PendingCatalogAdd {
                provider_id,
                api_key: Some(key.to_owned()),
                config_path,
            }),
        });
    }

    /// Handle a custom registry import result.
    pub(super) fn handle_custom_registry_import_result(&mut self) {
        let Some(result) = self
            .tui
            .chrome_mut()
            .custom_registry_import_result()
            .cloned()
        else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            CustomRegistryImportResult::Submitted(source) => {
                self.tui
                    .chrome_mut()
                    .set_custom_working_label(Some("Fetching custom registry...".to_owned()));
                let url = source.url.clone();
                let handle =
                    tokio::spawn(async move { neo_ai::catalog::fetch_catalog_from(&url).await });
                self.pending_catalog_fetch = Some(PendingCatalogFetch {
                    source: CatalogFetchSource::Custom(source),
                    handle,
                    pending_add: None,
                });
            }
            CustomRegistryImportResult::Cancelled => {}
        }
    }

    /// Poll a pending catalog fetch. If it has finished, clear the working
    /// indicator and open the provider picker; if not, leave it in place.
    pub(super) async fn poll_pending_catalog_fetch(&mut self) {
        let Some(pending) = self.pending_catalog_fetch.take() else {
            return;
        };
        if !pending.handle.is_finished() {
            self.pending_catalog_fetch = Some(pending);
            return;
        }
        self.tui.chrome_mut().set_custom_working_label(None);
        match pending.handle.await {
            Ok(Ok(catalog)) => {
                // API-key submit path: write the provider into config and report.
                if let Some(add) = pending.pending_add {
                    match catalog.get(&add.provider_id) {
                        Some(entry) => {
                            match crate::config::mutations::add_provider_from_catalog_entry(
                                &add.config_path,
                                &add.provider_id,
                                entry,
                                add.api_key.as_deref(),
                                None,
                            ) {
                                Ok(message) => {
                                    self.push_status(message);
                                    self.refresh_config();
                                }
                                Err(error) => {
                                    self.push_status(format!(
                                        "Error: Failed to add provider: {error}"
                                    ));
                                }
                            }
                        }
                        None => {
                            self.push_status(format!(
                                "Error: provider '{}' not found in models.dev catalog",
                                add.provider_id
                            ));
                        }
                    }
                    return;
                }
                let items = catalog_choice_items(&catalog);
                if items.is_empty() {
                    self.push_status("No providers found in catalog.");
                    return;
                }
                self.open_catalog_fetch_result(pending.source, catalog, items);
            }
            Ok(Err(error)) => {
                self.push_status(format!("Error: Failed to fetch catalog: {error}"));
            }
            Err(join_error) => {
                self.push_status(format!("Error: Failed to fetch catalog: {join_error}"));
            }
        }
    }

    fn open_catalog_fetch_result(
        &mut self,
        source: CatalogFetchSource,
        catalog: CatalogEntries,
        items: Vec<ChoiceItem>,
    ) {
        match source {
            CatalogFetchSource::Known => self.open_provider_choice_picker(items),
            CatalogFetchSource::Custom(source) => {
                self.pending_custom_registry = Some(PendingCustomRegistry { source, catalog });
                self.open_provider_choice_picker(custom_catalog_choice_items(items));
            }
        }
    }

    pub(super) fn open_provider_choice_picker(&mut self, items: Vec<ChoiceItem>) {
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(ChoicePickerOptions {
                title: "Select a provider".to_owned(),
                items,
                initial_id: None,
                theme,
                page_size: 0,
                current_id: None,
            });
    }
}
