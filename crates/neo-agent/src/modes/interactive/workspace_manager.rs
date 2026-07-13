use std::path::{Path, PathBuf};

use anyhow::Context as _;
use neo_agent_core::WorkspaceAccessPolicy;
use neo_tui::dialogs::{
    ConfirmDialogOptions, ConfirmDialogResult, TextInputOptions, TextInputResult,
    WorkspaceManagerAction, WorkspaceManagerOptions, WorkspaceRow,
};

use crate::trust::ProjectTrustState;

use super::InteractiveController;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PendingWorkspaceMutation {
    Add { path: PathBuf },
    ToggleEnabled { path: PathBuf },
    ToggleRead { path: PathBuf },
    ToggleWrite { path: PathBuf },
    Delete { path: PathBuf },
}

impl InteractiveController {
    pub(super) fn open_workspace_manager(&mut self) {
        let theme = self.tui.chrome().theme();
        let trusted = self.workspace_access_trusted();
        let rows = if trusted {
            match self.workspace_rows() {
                Ok(rows) => rows,
                Err(error) => {
                    self.push_status(format!("Failed to read workspace access: {error}"));
                    return;
                }
            }
        } else {
            Vec::new()
        };
        self.tui
            .chrome_mut()
            .open_workspace_manager(&WorkspaceManagerOptions {
                trusted,
                rows,
                theme,
            });
    }

    pub(super) fn handle_workspace_manager_action(&mut self) {
        let Some(action) = self.tui.chrome_mut().take_workspace_manager_action() else {
            return;
        };
        match action {
            WorkspaceManagerAction::Close => {
                self.tui.chrome_mut().close_focused_overlay();
            }
            WorkspaceManagerAction::Add => {
                self.pending_workspace_add_input = true;
                self.tui.chrome_mut().open_text_input(TextInputOptions {
                    title: "Add Workspace Directory".to_owned(),
                    prompt: "Path".to_owned(),
                    submit_label: "Enter continue".to_owned(),
                });
            }
            WorkspaceManagerAction::ToggleEnabled(path) => {
                self.confirm_workspace_mutation(PendingWorkspaceMutation::ToggleEnabled {
                    path: PathBuf::from(path),
                });
            }
            WorkspaceManagerAction::ToggleRead(path) => {
                self.confirm_workspace_mutation(PendingWorkspaceMutation::ToggleRead {
                    path: PathBuf::from(path),
                });
            }
            WorkspaceManagerAction::ToggleWrite(path) => {
                self.confirm_workspace_mutation(PendingWorkspaceMutation::ToggleWrite {
                    path: PathBuf::from(path),
                });
            }
            WorkspaceManagerAction::Delete(path) => {
                self.confirm_workspace_mutation(PendingWorkspaceMutation::Delete {
                    path: PathBuf::from(path),
                });
            }
        }
    }

    pub(super) fn handle_workspace_text_input_result(&mut self, result: TextInputResult) -> bool {
        if !self.pending_workspace_add_input {
            return false;
        }
        self.pending_workspace_add_input = false;
        match result {
            TextInputResult::Submitted(value) => {
                self.tui.chrome_mut().close_focused_overlay();
                let path = expand_workspace_path(&self.workspace_root, value.trim());
                match self.validate_workspace_path(&path) {
                    Ok(entry) => {
                        self.confirm_workspace_mutation(PendingWorkspaceMutation::Add {
                            path: entry.path,
                        });
                    }
                    Err(error) => {
                        self.push_status(format!("Workspace path error: {error}"));
                        self.open_workspace_manager();
                    }
                }
                true
            }
            TextInputResult::Cancelled => {
                self.tui.chrome_mut().close_focused_overlay();
                self.open_workspace_manager();
                true
            }
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn handle_workspace_confirm_result(&mut self, result: ConfirmDialogResult) -> bool {
        let approved = matches!(result, ConfirmDialogResult::Approved { .. });
        self.tui.chrome_mut().close_focused_overlay();
        let Some(mutation) = self.pending_workspace_mutation.take() else {
            self.open_workspace_manager();
            return true;
        };
        if approved && let Err(error) = self.apply_workspace_mutation(mutation) {
            self.push_status(format!("Failed to update workspace access: {error}"));
        }
        self.refresh_workspace_policy_from_store();
        self.open_workspace_manager();
        true
    }

    pub(super) fn refresh_workspace_policy_from_store(&mut self) {
        let Ok(primary_policy) = WorkspaceAccessPolicy::new(&self.workspace_root) else {
            return;
        };
        let policy = if self.workspace_access_trusted() {
            self.workspace_store
                .as_ref()
                .and_then(|store| store.read_project(&self.workspace_root).ok())
                .and_then(|project| {
                    WorkspaceAccessPolicy::with_roots(
                        &self.workspace_root,
                        crate::workspaces::access_roots_from_project(&project),
                    )
                    .ok()
                })
                .unwrap_or(primary_policy)
        } else {
            primary_policy
        };
        if let Ok(mut guard) = self.workspace_policy.write() {
            *guard = Some(policy);
        }
    }

    fn workspace_access_trusted(&self) -> bool {
        matches!(
            self.local_config
                .as_ref()
                .map(|config| &config.project_trust),
            Some(ProjectTrustState::Trusted { .. })
        )
    }

    fn workspace_rows(&self) -> anyhow::Result<Vec<WorkspaceRow>> {
        let Some(store) = self.workspace_store.as_ref() else {
            return Ok(Vec::new());
        };
        let project = store.read_project(&self.workspace_root)?;
        Ok(project
            .entries
            .into_iter()
            .map(|entry| WorkspaceRow {
                path: entry.path.display().to_string(),
                enabled: entry.enabled,
                read: entry.read,
                write: entry.write,
                missing: !entry.path.is_dir(),
            })
            .collect())
    }

    fn validate_workspace_path(
        &self,
        path: &Path,
    ) -> anyhow::Result<crate::workspaces::WorkspaceEntry> {
        let project = self
            .workspace_store
            .as_ref()
            .map(|store| store.read_project(&self.workspace_root))
            .transpose()?
            .unwrap_or_default();
        crate::workspaces::validate_new_workspace_entry(&self.workspace_root, &project, path)
    }

    fn confirm_workspace_mutation(&mut self, mutation: PendingWorkspaceMutation) {
        let opts = confirm_options_for_mutation(&mutation, self.tui.chrome().theme());
        self.pending_workspace_mutation = Some(mutation);
        self.tui.chrome_mut().open_confirm_dialog(opts);
    }

    fn apply_workspace_mutation(
        &mut self,
        mutation: PendingWorkspaceMutation,
    ) -> anyhow::Result<()> {
        let Some(store) = self.workspace_store.as_ref() else {
            anyhow::bail!("workspace store unavailable");
        };
        let mut project = store.read_project(&self.workspace_root)?;
        match mutation {
            PendingWorkspaceMutation::Add { path } => {
                let entry = crate::workspaces::validate_new_workspace_entry(
                    &self.workspace_root,
                    &project,
                    &path,
                )
                .with_context(|| format!("invalid workspace path {}", path.display()))?;
                project.entries.push(entry);
            }
            PendingWorkspaceMutation::ToggleEnabled { path } => {
                if let Some(entry) = project.entries.iter_mut().find(|entry| entry.path == path) {
                    entry.enabled = !entry.enabled;
                }
            }
            PendingWorkspaceMutation::ToggleRead { path } => {
                if let Some(entry) = project.entries.iter_mut().find(|entry| entry.path == path) {
                    entry.read = !entry.read;
                    if !entry.read {
                        entry.write = false;
                    }
                }
            }
            PendingWorkspaceMutation::ToggleWrite { path } => {
                if let Some(entry) = project.entries.iter_mut().find(|entry| entry.path == path) {
                    entry.write = !entry.write;
                    if entry.write {
                        entry.read = true;
                    }
                }
            }
            PendingWorkspaceMutation::Delete { path } => {
                project.entries.retain(|entry| entry.path != path);
            }
        }
        store.write_project(&self.workspace_root, project)
    }
}

fn expand_workspace_path(workspace_root: &Path, raw: &str) -> PathBuf {
    let path = crate::config::expand_user_path(PathBuf::from(raw));
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

fn confirm_options_for_mutation(
    mutation: &PendingWorkspaceMutation,
    theme: neo_tui::primitive::theme::TuiTheme,
) -> ConfirmDialogOptions {
    match mutation {
        PendingWorkspaceMutation::Add { path } => ConfirmDialogOptions {
            id: format!("add:{}", path.display()),
            title: "Confirm Workspace Access".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Add this directory to the current trusted project?".to_owned(),
                String::new(),
                " Directory".to_owned(),
                format!("   {}", path.display()),
                String::new(),
                " Access".to_owned(),
                "   enabled, read-only".to_owned(),
                String::new(),
                " Neo file tools will be able to read files under this directory.".to_owned(),
            ],
            theme,
        },
        PendingWorkspaceMutation::ToggleWrite { path } => ConfirmDialogOptions {
            id: format!("toggle-write:{}", path.display()),
            title: "Confirm Write Access".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Enable or disable write access for this directory?".to_owned(),
                String::new(),
                format!(" {}", path.display()),
                String::new(),
                " Neo file tools may be able to edit and create files under this root.".to_owned(),
                " Tool permission mode still applies.".to_owned(),
            ],
            theme,
        },
        PendingWorkspaceMutation::Delete { path } => ConfirmDialogOptions {
            id: format!("delete:{}", path.display()),
            title: "Remove Workspace Directory".to_owned(),
            hint: "Y remove · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Remove this workspace access entry?".to_owned(),
                String::new(),
                format!(" {}", path.display()),
                String::new(),
                " Files on disk are not deleted. Only Neo's persisted access entry changes."
                    .to_owned(),
            ],
            theme,
        },
        PendingWorkspaceMutation::ToggleEnabled { path }
        | PendingWorkspaceMutation::ToggleRead { path } => ConfirmDialogOptions {
            id: format!("toggle:{}", path.display()),
            title: "Confirm Workspace Access Change".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Change access for this workspace directory?".to_owned(),
                String::new(),
                format!(" {}", path.display()),
            ],
            theme,
        },
    }
}
