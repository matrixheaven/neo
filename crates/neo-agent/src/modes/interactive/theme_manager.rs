//! Controller-side theme manager adapter.
//!
//! Bridges the Task 2 `neo-tui` theme manager overlay to the Task 1 theme
//! repository: builds catalog snapshots, executes typed actions (apply,
//! startup default, import, duplicate, delete, refresh), drives the external
//! import-path / copy-name text-input dialogs, and owns the ephemeral
//! session-override marker that keeps an applied theme stable across config
//! refreshes.
//!
//! Side-effect rules enforced here:
//! - Apply-for-session only touches the current render state plus the
//!   controller-owned override marker; it never writes `config.toml`, never
//!   appends transcript/session events, and never alters model context.
//! - Set-startup-default persists only the logical id through the config
//!   mutation helper and leaves the current `TuiTheme` unchanged.
//! - Import/copy/delete/refresh keep the manager open after errors so the
//!   user can retry; only close/apply close the overlay.

use std::path::Path;

use anyhow::{Context as _, anyhow, bail};
use neo_tui::dialogs::{ChoiceItem, ChoicePickerOptions, TextInputOptions, TextInputResult};
use neo_tui::shell::{
    ThemeCatalogEntrySnapshot, ThemeConflictPolicy, ThemeManagerAction, ThemeManagerPending,
    ThemeManagerState,
};

use crate::config::AppConfig;
use crate::themes::{ThemeCatalog, ThemeEntry, ThemeId, ThemeRepository};

use super::{InteractiveController, slash_arg};

/// A theme import whose destination id already exists; the user must choose
/// overwrite or save-as-new before the repository mutates.
#[derive(Debug)]
pub(super) struct PendingThemeImport {
    pub(super) path: String,
}

/// A parsed `/theme` slash request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ThemeSlashRequest {
    /// Bare `/theme` — open the manager overlay (idle only).
    Manager,
    /// `/theme reload` — clear the session override and re-apply the resolved
    /// config theme.
    Reload,
    /// `/theme <reference>` — resolve the exact logical id first, then the
    /// unique exact display name, and apply directly.
    Apply(String),
}

/// Parse the `/theme` grammar. Recognizes only lowercase `/theme`; a
/// non-whitespace suffix (`/themeish`) and embedded prose stay normal
/// prompts. Boundary whitespace is trimmed and the argument is preserved as
/// one exact value.
///
/// The exact argument `reload` is reserved for the explicit reload command.
/// Spec allows any exact reference, so a theme whose display name is exactly
/// "reload" is shadowed by name here (it stays reachable by id and through
/// the manager); this is a documented edge, not a special case to extend.
pub(super) fn parse_theme_slash(prompt: &str) -> Option<ThemeSlashRequest> {
    let trimmed = prompt.trim();
    if trimmed == "/theme" {
        return Some(ThemeSlashRequest::Manager);
    }
    let argument = slash_arg(trimmed, "/theme")?;
    if argument == "reload" {
        return Some(ThemeSlashRequest::Reload);
    }
    Some(ThemeSlashRequest::Apply(argument.to_owned()))
}

/// The currently applied theme id: the session override when present, else
/// the id from the config startup resolution.
fn active_theme_id(config: &AppConfig, session_override: Option<&ThemeId>) -> Option<String> {
    if let Some(id) = session_override {
        return Some(id.as_str().to_owned());
    }
    match &config.theme_resolution {
        crate::themes::ThemeResolution::Explicit(entry)
        | crate::themes::ThemeResolution::Discovered(entry) => Some(entry.id.as_str().to_owned()),
        crate::themes::ThemeResolution::Default
        | crate::themes::ThemeResolution::Fallback { .. } => None,
    }
}

/// Derive the repository destination id for an imported source file: the
/// source file name with a forced `.json` extension (the catalog only
/// discovers `*.json` files).
fn import_destination_id(source: &Path) -> anyhow::Result<ThemeId> {
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("theme source {} has no usable file name", source.display()))?;
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or(file_name);
    ThemeId::new(&format!("{stem}.json"))
        .with_context(|| format!("theme source name {file_name:?} cannot become a theme id"))
}

/// First free id at `base`, `base-1`, `base-2`, ... for save-as-new imports.
fn fresh_import_id(catalog: &ThemeCatalog, base: &ThemeId) -> ThemeId {
    if catalog.by_id(base).is_none() {
        return base.clone();
    }
    let stem = base.as_str().strip_suffix(".json").unwrap_or(base.as_str());
    for counter in 1.. {
        let candidate = format!("{stem}-{counter}.json");
        if let Ok(id) = ThemeId::new(&candidate)
            && catalog.by_id(&id).is_none()
        {
            return id;
        }
    }
    unreachable!("an infinite sequence of suffixed ids cannot all collide with a finite catalog")
}

/// Build a repository id from a user-supplied display name. Path separators
/// become dashes; leading dots and trailing dots/spaces are removed so the
/// result always passes `ThemeId` validation.
fn id_from_display_name(display_name: &str) -> Option<ThemeId> {
    let mut candidate = display_name.trim().replace(['/', '\\'], "-");
    while candidate.ends_with(['.', ' ']) {
        candidate.pop();
    }
    if candidate.is_empty() || candidate.starts_with('.') {
        return None;
    }
    ThemeId::new(&format!("{candidate}.json")).ok()
}

/// First free id for a duplicate, derived from the requested display name.
/// The suffix sequence matches `fresh_import_id` (both start at `-1`).
fn fresh_duplicate_id(catalog: &ThemeCatalog, display_name: &str) -> Option<ThemeId> {
    let base = id_from_display_name(display_name)?;
    if catalog.by_id(&base).is_none() {
        return Some(base);
    }
    let stem = base.as_str().strip_suffix(".json").unwrap_or(base.as_str());
    for counter in 1.. {
        let candidate = format!("{stem}-{counter}.json");
        if let Ok(id) = ThemeId::new(&candidate)
            && catalog.by_id(&id).is_none()
        {
            return Some(id);
        }
    }
    None
}

impl InteractiveController {
    /// Open the theme manager overlay. The catalog is scanned through the
    /// repository service; the overlay itself never touches files or config.
    pub(super) fn open_theme_manager(&mut self) {
        let Some(config) = self.local_config.as_ref() else {
            self.push_status("Theme manager unavailable: no config");
            return;
        };
        let repo = ThemeRepository::from_config_path(&config.config_path);
        let entries = match self.theme_catalog_snapshot(config, &repo) {
            Ok(entries) => entries,
            Err(error) => {
                self.push_status(format!("Theme manager failed to load the catalog: {error}"));
                return;
            }
        };
        self.tui.chrome_mut().open_theme_manager(entries);
    }

    /// Manager entry point shared by bare `/theme` and the `theme.manager`
    /// palette command: open the overlay only when no model turn is running,
    /// otherwise keep the turn intact and report that the manager needs idle.
    pub(super) fn open_theme_manager_if_idle(&mut self) {
        if self.active_turn.is_some() {
            self.push_status(
                "Finish or interrupt the current turn before opening the theme manager.",
            );
            return;
        }
        self.open_theme_manager();
    }

    /// Clear the session override and re-apply the theme resolved from the
    /// config file. When the config file cannot be reloaded, reports a
    /// retryable error and leaves the current chrome theme untouched.
    pub(super) fn reload_theme_from_config(&mut self) {
        self.session_theme_override = None;
        if !self.refresh_config() {
            self.push_status(
                "Theme reload failed: the config file could not be reloaded; \
                 the current theme is kept until it succeeds",
            );
            return;
        }
        let Some(config) = self.local_config.as_ref() else {
            self.push_status("Theme error: no config available");
            return;
        };
        self.tui.chrome_mut().set_theme(config.theme.theme);
        self.push_status(format!("Theme reloaded from config: {}", config.theme.name));
    }

    /// Apply a `/theme <reference>` directly while busy or idle. Resolution is
    /// exact id first, then unique exact display name; errors are local and
    /// side-effect free.
    pub(super) fn apply_theme_from_slash(&mut self, reference: &str) {
        let result = self.resolve_theme_reference(reference);
        match result {
            Ok(entry) => self.apply_theme_entry(&entry),
            Err(error) => self.push_status(format!("Theme error: {error}")),
        }
    }

    fn resolve_theme_reference(&self, reference: &str) -> anyhow::Result<ThemeEntry> {
        let Some(config) = self.local_config.as_ref() else {
            bail!("no config available");
        };
        let repo = ThemeRepository::from_config_path(&config.config_path);
        repo.resolve_ref(reference)
    }

    fn apply_theme_entry(&mut self, entry: &ThemeEntry) {
        if let Some(error) = &entry.error {
            self.push_status(format!(
                "Theme error: theme {} is not usable: {error}",
                entry.id.as_str()
            ));
            return;
        }
        self.tui.chrome_mut().set_theme(entry.theme);
        self.session_theme_override = Some(entry.id.clone());
        self.push_status(format!("Theme applied: {}", entry.display_name()));
    }

    /// Poll the overlay for one typed action plus any external-dialog step it
    /// needs (import path / copy name). Called after every rich-dialog input.
    pub(super) fn handle_theme_manager_dialog_step(&mut self) {
        if let Some(action) = self.tui.chrome_mut().take_theme_manager_action() {
            self.execute_theme_manager_action(action);
            return;
        }
        let pending = self
            .tui
            .chrome()
            .theme_manager_state()
            .and_then(ThemeManagerState::pending);
        match pending {
            Some(ThemeManagerPending::ImportPath) if !self.theme_import_path_dialog => {
                self.theme_import_path_dialog = true;
                self.tui.chrome_mut().open_text_input(TextInputOptions {
                    title: "Import Theme".to_owned(),
                    prompt: "Path to the theme file".to_owned(),
                    submit_label: "Enter import".to_owned(),
                });
            }
            Some(ThemeManagerPending::CopyName) if !self.theme_copy_name_dialog => {
                self.theme_copy_name_dialog = true;
                self.tui.chrome_mut().open_text_input(TextInputOptions {
                    title: "Copy Theme".to_owned(),
                    prompt: "Display name for the copy".to_owned(),
                    submit_label: "Enter copy".to_owned(),
                });
            }
            _ => {}
        }
    }

    /// Feed a text-input dialog result back to the theme manager. Returns
    /// `true` when the result belonged to the import-path or copy-name flow.
    pub(super) fn handle_theme_manager_text_input_result(
        &mut self,
        result: TextInputResult,
    ) -> bool {
        let import_dialog = self.theme_import_path_dialog;
        let copy_dialog = self.theme_copy_name_dialog;
        if !import_dialog && !copy_dialog {
            return false;
        }
        self.theme_import_path_dialog = false;
        self.theme_copy_name_dialog = false;
        self.tui.chrome_mut().close_focused_overlay();
        let value = match result {
            TextInputResult::Submitted(value) => Some(value),
            TextInputResult::Cancelled => None,
        };
        let action = self
            .tui
            .chrome_mut()
            .theme_manager_state_mut()
            .and_then(|state| {
                if import_dialog {
                    state.submit_import_path(value)
                } else {
                    state.submit_copy_name(value)
                }
            });
        if let Some(action) = action {
            self.execute_theme_manager_action(action);
        }
        true
    }

    fn execute_theme_manager_action(&mut self, action: ThemeManagerAction) {
        match action {
            ThemeManagerAction::Close => {
                self.tui.chrome_mut().close_focused_overlay();
            }
            ThemeManagerAction::ApplySession(id) => self.apply_theme_for_session(&id),
            ThemeManagerAction::SetStartupDefault(id) => self.set_theme_startup_default(&id),
            ThemeManagerAction::Import {
                path,
                conflict_policy,
            } => {
                self.import_theme(&path, conflict_policy);
            }
            ThemeManagerAction::Duplicate {
                id,
                new_display_name,
            } => {
                self.duplicate_theme(&id, &new_display_name);
            }
            ThemeManagerAction::Delete(id) => self.delete_theme(&id),
            ThemeManagerAction::Refresh => self.refresh_theme_manager(),
        }
    }

    fn apply_theme_for_session(&mut self, id: &str) {
        let entry = match self.resolve_theme_id(id) {
            Ok(entry) => entry,
            Err(error) => {
                self.push_status(format!("Theme error: {error}"));
                return;
            }
        };
        self.apply_theme_entry(&entry);
    }

    fn resolve_theme_id(&self, id: &str) -> anyhow::Result<ThemeEntry> {
        let Some(config) = self.local_config.as_ref() else {
            bail!("no config available");
        };
        let repo = ThemeRepository::from_config_path(&config.config_path);
        repo.resolve(&ThemeId::new(id)?)
    }

    /// Persist the logical id as the startup default through the config
    /// mutation helper. The current `TuiTheme` stays unchanged; on write
    /// failure both the runtime and the config state are left untouched.
    fn set_theme_startup_default(&mut self, id: &str) {
        let Some(config_path) = self.config_path() else {
            self.push_status("Theme manager unavailable: no config path");
            return;
        };
        let result = (|| -> anyhow::Result<ThemeEntry> {
            let config = self
                .local_config
                .as_ref()
                .ok_or_else(|| anyhow!("no config available"))?;
            let repo = ThemeRepository::from_config_path(&config.config_path);
            let entry = repo.resolve(&ThemeId::new(id)?)?;
            if let Some(error) = &entry.error {
                bail!("theme {} is not usable: {error}", entry.id.as_str());
            }
            crate::config::mutations::set_startup_theme(&config_path, id)?;
            Ok(entry)
        })();
        match result {
            Ok(entry) => {
                if let Some(config) = self.local_config.as_mut() {
                    config.tui.theme = Some(id.to_owned());
                }
                self.push_status(format!("Startup theme set: {}", entry.display_name()));
                self.rescan_theme_manager_after_mutation(Some(id));
            }
            Err(error) => {
                self.push_status(format!("Failed to set startup theme: {error}"));
            }
        }
    }

    fn import_theme(&mut self, path: &str, conflict_policy: ThemeConflictPolicy) {
        let result = self.import_theme_inner(path, conflict_policy);
        match result {
            Ok(Some(entry)) => {
                self.push_status(format!("Theme imported: {}", entry.display_name()));
                self.rescan_theme_manager_after_mutation(Some(entry.id.as_str()));
            }
            Ok(None) => {
                // Conflict resolution is pending; the choice picker is open.
            }
            Err(error) => {
                self.push_status(format!("Theme import failed: {error}"));
            }
        }
    }

    fn import_theme_inner(
        &mut self,
        path: &str,
        conflict_policy: ThemeConflictPolicy,
    ) -> anyhow::Result<Option<ThemeEntry>> {
        let source = Path::new(path);
        let Some(config) = self.local_config.as_ref() else {
            bail!("no config available");
        };
        let repo = ThemeRepository::from_config_path(&config.config_path);
        let destination = import_destination_id(source)?;
        let catalog = repo.catalog()?;
        if conflict_policy == ThemeConflictPolicy::Ask && catalog.by_id(&destination).is_some() {
            self.pending_theme_import = Some(PendingThemeImport {
                path: path.to_owned(),
            });
            self.open_import_conflict_picker(&destination);
            return Ok(None);
        }
        let id = match conflict_policy {
            ThemeConflictPolicy::SaveAsNew => fresh_import_id(&catalog, &destination),
            ThemeConflictPolicy::Ask | ThemeConflictPolicy::Overwrite => destination,
        };
        repo.import(&id, source).map(Some)
    }

    fn open_import_conflict_picker(&mut self, destination: &ThemeId) {
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(ChoicePickerOptions {
                title: format!("Theme {} already exists", destination.as_str()),
                items: vec![
                    ChoiceItem::new("theme-import-overwrite", "Overwrite the existing theme")
                        .with_description("Replace the file in the theme repository"),
                    ChoiceItem::new("theme-import-save-as-new", "Save as a new theme")
                        .with_description("Keep the existing theme and add a suffixed copy"),
                ],
                initial_id: None,
                theme,
                page_size: 0,
                current_id: None,
            });
    }

    /// Resolve an import conflict choice picked in the dialog.
    pub(super) fn handle_theme_choice_item(&mut self, id: &str) -> bool {
        let Some(pending) = self.pending_theme_import.take() else {
            return false;
        };
        match id {
            "theme-import-overwrite" => {
                self.import_theme(&pending.path, ThemeConflictPolicy::Overwrite);
            }
            "theme-import-save-as-new" => {
                self.import_theme(&pending.path, ThemeConflictPolicy::SaveAsNew);
            }
            _ => return false,
        }
        true
    }

    /// Cancel the pending import when the conflict picker is dismissed.
    pub(super) fn clear_pending_theme_import(&mut self) {
        self.pending_theme_import = None;
    }

    fn duplicate_theme(&mut self, id: &str, new_display_name: &str) {
        let result = (|| -> anyhow::Result<ThemeEntry> {
            let Some(config) = self.local_config.as_ref() else {
                bail!("no config available");
            };
            let repo = ThemeRepository::from_config_path(&config.config_path);
            let catalog = repo.catalog()?;
            let source_id = ThemeId::new(id)?;
            let source = catalog
                .by_id(&source_id)
                .ok_or_else(|| anyhow!("theme {id:?} not found"))?;
            if let Some(error) = &source.error {
                bail!("theme {id:?} is not usable: {error}");
            }
            let display_name = new_display_name.trim();
            if display_name.is_empty() {
                bail!("the duplicate display name must not be empty");
            }
            let new_id = fresh_duplicate_id(&catalog, display_name)
                .ok_or_else(|| anyhow!("cannot derive a unique theme id from {display_name:?}"))?;
            repo.save_as_new(&new_id, display_name, &source.theme)
        })();
        match result {
            Ok(entry) => {
                self.push_status(format!("Theme duplicated: {}", entry.display_name()));
                self.rescan_theme_manager_after_mutation(Some(entry.id.as_str()));
            }
            Err(error) => {
                self.push_status(format!("Theme copy failed: {error}"));
            }
        }
    }

    fn delete_theme(&mut self, id: &str) {
        let result = (|| -> anyhow::Result<ThemeEntry> {
            let Some(config) = self.local_config.as_ref() else {
                bail!("no config available");
            };
            let repo = ThemeRepository::from_config_path(&config.config_path);
            let catalog = repo.catalog()?;
            let theme_id = ThemeId::new(id)?;
            let entry = catalog
                .by_id(&theme_id)
                .cloned()
                .ok_or_else(|| anyhow!("theme {id:?} not found"))?;
            if let Some(active) = active_theme_id(config, self.session_theme_override.as_ref())
                && active == entry.id.as_str()
            {
                bail!("the active theme cannot be deleted");
            }
            if config.tui.theme.as_deref() == Some(entry.id.as_str()) {
                bail!("the startup default cannot be deleted");
            }
            repo.delete(&theme_id)?;
            Ok(entry)
        })();
        match result {
            Ok(entry) => {
                self.push_status(format!("Theme deleted: {}", entry.display_name()));
                self.rescan_theme_manager_after_mutation(None);
            }
            Err(error) => {
                self.push_status(format!("Theme delete failed: {error}"));
            }
        }
    }

    fn refresh_theme_manager(&mut self) {
        let Some(config) = self.local_config.as_ref() else {
            self.push_status("Theme manager unavailable: no config");
            return;
        };
        let repo = ThemeRepository::from_config_path(&config.config_path);
        match self.theme_catalog_snapshot(config, &repo) {
            Ok(entries) => {
                if let Some(state) = self.tui.chrome_mut().theme_manager_state_mut() {
                    state.apply_snapshot(entries);
                }
            }
            Err(error) => {
                self.push_status(format!("Theme catalog refresh failed: {error}"));
            }
        }
    }

    /// Re-scan the repository and re-apply the catalog snapshot with stable
    /// selection: the new id after import/copy, the nearest remaining entry
    /// after a delete, and the previous id after a refresh.
    fn rescan_theme_manager_after_mutation(&mut self, select_id: Option<&str>) {
        let Some(config) = self.local_config.as_ref() else {
            return;
        };
        let repo = ThemeRepository::from_config_path(&config.config_path);
        let entries = match self.theme_catalog_snapshot(config, &repo) {
            Ok(entries) => entries,
            Err(error) => {
                self.push_status(format!("Theme catalog refresh failed: {error}"));
                return;
            }
        };
        let Some(state) = self.tui.chrome_mut().theme_manager_state_mut() else {
            return;
        };
        state.apply_snapshot(entries);
        if let Some(id) = select_id {
            state.select_id(id);
        }
    }

    fn theme_catalog_snapshot(
        &self,
        config: &AppConfig,
        repo: &ThemeRepository,
    ) -> anyhow::Result<Vec<ThemeCatalogEntrySnapshot>> {
        let catalog = repo.catalog()?;
        let active_id = active_theme_id(config, self.session_theme_override.as_ref());
        let startup_default_id = config.tui.theme.clone();
        Ok(catalog
            .entries
            .iter()
            .map(|entry| ThemeCatalogEntrySnapshot {
                id: entry.id.as_str().to_owned(),
                display_name: entry.display_name().to_owned(),
                theme: entry.error.is_none().then_some(entry.theme),
                error: entry.error.clone(),
                active: active_id.as_deref() == Some(entry.id.as_str()),
                startup_default: startup_default_id.as_deref() == Some(entry.id.as_str()),
            })
            .collect())
    }
}
