//! Extracted: startup workflow — types, apply startup action/options, trust
//! dialog resolution, and session loading at startup.

use std::{env, time::Duration};

use anyhow::{Context, Result};

use crate::trust;

use neo_tui::terminal_image::ImageRenderPolicy;

use super::InteractiveController;
use super::{TerminalEvents, startup_notices, terminal_image_capabilities_for_policy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupAction {
    None,
    OpenSessionPicker,
    LoadSession(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractiveOptions {
    pub verbose_startup: bool,
}

impl InteractiveController {
    pub fn apply_startup_action(&mut self, startup: &StartupAction) {
        match startup {
            StartupAction::OpenSessionPicker => self.open_session_picker(),
            StartupAction::None | StartupAction::LoadSession(_) => {
                // `LoadSession` is loaded asynchronously before the terminal loop.
            }
        }
    }

    pub(super) async fn load_session_at_startup(&mut self, session_id: &str) -> Result<()> {
        let loaded = (self.load_session)(session_id.to_owned())
            .await
            .with_context(|| format!("failed to load session {session_id}"))?;
        self.tui
            .chrome_mut()
            .set_session_label(loaded.label.clone());
        self.rebuild_transcript_from_session(&loaded);
        self.active_session_id = Some(session_id.to_owned());
        Ok(())
    }

    /// Run the workspace trust dialog until the user makes a choice, then persist
    /// and apply the decision. Cancel/close without a choice is treated as
    /// untrusted.
    pub(super) async fn resolve_trust_dialog_at_startup(
        &mut self,
        data: neo_tui::dialogs::TrustDialogData,
        mut events: impl TerminalEvents,
        mut render: impl FnMut(&mut neo_tui::NeoTui) -> Result<()>,
    ) -> Result<()> {
        self.tui.chrome_mut().open_trust_dialog(data);
        render(&mut self.tui)?;
        loop {
            let result = self.tui.chrome_mut().take_trust_dialog_result();
            if let Some(result) = result {
                self.tui.chrome_mut().close_focused_overlay();
                self.apply_trust_dialog_result(result)?;
                return Ok(());
            }
            match events.poll_input_event(Duration::from_millis(50))? {
                Some(event) => {
                    let exit = self.handle_input_event(event).await?;
                    if exit {
                        // Treat an early loop exit (e.g. double Ctrl-C) as
                        // untrusted so the workspace is never silently trusted.
                        let target = self.local_config.as_ref().map_or_else(
                            || self.workspace_root.clone(),
                            |config| config.project_dir.clone(),
                        );
                        self.apply_trust_dialog_result(
                            neo_tui::dialogs::TrustDialogResult::Untrusted { target },
                        )?;
                        return Ok(());
                    }
                }
                None => tokio::task::yield_now().await,
            }
            self.tui.chrome_mut().advance_activity_frame();
            render(&mut self.tui)?;
        }
    }

    pub(super) fn apply_trust_dialog_result(
        &mut self,
        result: neo_tui::dialogs::TrustDialogResult,
    ) -> Result<()> {
        let (trusted, target) = match result {
            neo_tui::dialogs::TrustDialogResult::Trust { target } => (true, target),
            neo_tui::dialogs::TrustDialogResult::Untrusted { target } => (false, target),
        };

        if let Some(store) = self.trust_store.as_ref() {
            store.set(&target, Some(trusted))?;
        }

        let status_message = if trusted {
            format!("Workspace trusted: {}", target.display())
        } else {
            "Workspace untrusted: project context disabled".to_owned()
        };

        if let Some(config) = self.local_config.as_mut() {
            config.project_trusted = trusted;
            config.project_trust = if trusted {
                trust::ProjectTrustState::Trusted {
                    target: target.clone(),
                }
            } else {
                trust::ProjectTrustState::Untrusted {
                    target: target.clone(),
                }
            };
        }

        self.push_status(status_message);
        Ok(())
    }

    pub fn apply_startup_options(
        &mut self,
        config: &crate::config::AppConfig,
        options: InteractiveOptions,
    ) {
        self.tui.chrome_mut().set_theme(config.theme.theme);
        self.permission_mode = config.permission_mode;
        if let Ok(mut live) = self.live_permission_mode.write() {
            *live = config.permission_mode;
        }
        self.tui
            .chrome_mut()
            .set_permission_mode(config.permission_mode);
        self.tui
            .chrome_mut()
            .set_image_render_policy(ImageRenderPolicy::new(
                config.tui.image_protocol,
                config.tui.fetch_remote_images,
            ));
        self.tui
            .chrome_mut()
            .set_image_capabilities(terminal_image_capabilities_for_policy(
                config.tui.image_protocol,
                |name| env::var(name),
            ));
        if !options.verbose_startup {
            return;
        }
        self.push_status(startup_notices(config).join("\n"));
    }
}
