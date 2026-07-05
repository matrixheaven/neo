//! Extracted: session lifecycle — load, fork, rebuild transcript, reset/new session.

use anyhow::{Context, Result};
use crossterm::terminal::size;

use neo_tui::shell::{OverlayKind, SessionPickerItem, SessionPickerScope};
use neo_tui::transcript::TranscriptPane;

use crate::modes::sessions::{SessionPickerScope as SessionDataScope, session_summaries};

use super::InteractiveController;
use super::{LoadedSessionTranscript, replay_session_into_transcript, same_work_dir};

impl InteractiveController {
    pub(super) async fn load_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.tui.chrome_mut().confirm_session_picker() else {
            return Ok(());
        };

        if same_work_dir(&session.work_dir, &self.workspace_root) {
            let loaded = (self.load_session)(session.id.clone())
                .await
                .with_context(|| format!("failed to load session {}", session.id))?;
            self.tui
                .chrome_mut()
                .set_session_label(loaded.label.clone());
            self.rebuild_transcript_from_session(&loaded);
            self.active_session_id = Some(session.id);
            return Ok(());
        }

        let command = format!(
            "cd '{}' && neo --resume '{}'",
            session.work_dir.display(),
            session.id
        );
        self.push_status(command.clone());
        if let Err(error) = (self.clipboard_writer)(&command) {
            tracing::warn!("failed to copy resume command to clipboard: {error}");
        }
        Ok(())
    }

    pub(super) fn toggle_session_picker_scope(&mut self) {
        let current_scope = {
            let Some(overlay) = self.tui.chrome_mut().focused_overlay() else {
                return;
            };
            let OverlayKind::SessionPicker(picker) = &overlay.kind else {
                return;
            };
            picker.scope()
        };
        let new_scope = match current_scope {
            SessionPickerScope::Workspace => SessionDataScope::All,
            SessionPickerScope::All => SessionDataScope::Workspace,
        };
        let Some(config) = self.local_config.as_ref() else {
            return;
        };
        match session_summaries(config, new_scope) {
            Ok(summaries) => {
                if summaries.is_empty() {
                    self.tui.chrome_mut().close_focused_overlay();
                    self.open_empty_session_picker_with_scope(new_scope);
                    self.push_status(empty_scope_toggle_message(new_scope));
                    return;
                }
                self.session_items = summaries;
                self.tui.chrome_mut().close_focused_overlay();
                self.open_session_picker_with_scope(new_scope);
            }
            Err(error) => {
                self.push_status(format!("Error loading sessions: {error}"));
            }
        }
    }

    fn open_empty_session_picker_with_scope(&mut self, scope: SessionDataScope) {
        let current_session_id = self.active_session_id.clone().unwrap_or_default();
        let picker_scope = match scope {
            SessionDataScope::Workspace => SessionPickerScope::Workspace,
            SessionDataScope::All => SessionPickerScope::All,
        };
        self.tui.chrome_mut().open_session_picker(
            &current_session_id,
            picker_scope,
            Vec::<SessionPickerItem>::new(),
        );
    }

    pub(super) async fn fork_selected_session(&mut self) -> Result<()> {
        let Some(parent) = self.tui.chrome_mut().confirm_session_picker() else {
            return Ok(());
        };
        let forked = (self.fork_session)(parent.id.clone())
            .await
            .with_context(|| format!("failed to fork session {}", parent.id))?;
        self.tui
            .chrome_mut()
            .set_session_label(forked.transcript.label.clone());
        self.rebuild_transcript_from_session(&forked.transcript);
        self.active_session_id = Some(forked.session_id.clone());
        self.push_status(format!("fork from session {}", parent.id));
        self.push_status(format!("switch to fork session {}", forked.session_id));
        Ok(())
    }

    pub(super) async fn fork_current_session(&mut self) -> Result<()> {
        let Some(parent_id) = self.active_session_id.clone() else {
            self.push_status("No active session to fork");
            return Ok(());
        };
        let forked = (self.fork_session)(parent_id.clone())
            .await
            .with_context(|| format!("failed to fork session {parent_id}"))?;
        self.tui
            .chrome_mut()
            .set_session_label(forked.transcript.label.clone());
        self.rebuild_transcript_from_session(&forked.transcript);
        self.active_session_id = Some(forked.session_id.clone());
        self.push_status(format!("fork from session {parent_id}"));
        self.push_status(format!("switch to fork session {}", forked.session_id));
        Ok(())
    }

    pub(super) fn rebuild_transcript_from_session(&mut self, loaded: &LoadedSessionTranscript) {
        self.tui
            .chrome_mut()
            .set_main_agent_token_usage(loaded.main_agent_token_usage);

        if let Some(used_tokens) = loaded.estimated_context_tokens
            && let Some(window) = self.tui.chrome().context_window()
        {
            self.tui
                .chrome_mut()
                .set_context_window(Some(window.with_used_tokens(used_tokens)));
        }

        let (cols, rows) = size().unwrap_or((80, 24));
        let mut transcript = TranscriptPane::new(usize::from(cols), usize::from(rows));
        transcript.set_theme(self.tui.chrome().theme());
        transcript.push_welcome_banner(
            self.tui.chrome().title(),
            self.tui.chrome().session_label(),
            self.tui.chrome().model_label(),
            &self.tui.chrome().cwd_label(),
            env!("CARGO_PKG_VERSION"),
            None,
        );
        replay_session_into_transcript(&mut transcript, loaded);
        *self.tui.transcript_mut() = transcript;
    }

    /// Rebuild the transcript pane from scratch with only the welcome banner,
    /// matching the startup layout for an unsaved `new` session. Used by
    /// `/new` / `/clear` to wipe visible transcript state without deleting the
    /// previous JSONL session.
    fn rebuild_empty_welcome_transcript(&mut self) {
        let (cols, rows) = size().unwrap_or((80, 24));
        let mut transcript = TranscriptPane::new(usize::from(cols), usize::from(rows));
        transcript.set_theme(self.tui.chrome().theme());
        transcript.push_welcome_banner(
            self.tui.chrome().title(),
            self.tui.chrome().session_label(),
            self.tui.chrome().model_label(),
            &self.tui.chrome().cwd_label(),
            env!("CARGO_PKG_VERSION"),
            None,
        );
        *self.tui.transcript_mut() = transcript;
    }

    /// Reset the in-memory TUI/runtime state so the next prompt starts a fresh
    /// workspace-scoped session. Preserves user-facing choices (model, thinking,
    /// permission mode, plan/goal development mode, workspace root) and only
    /// clears transient turn/overlay/transcript state.
    fn reset_for_new_session(&mut self) {
        self.active_turn = None;
        self.pending_approvals.clear();
        self.resolved_approvals.clear();
        self.pending_questions.clear();
        self.pending_question_prompts.clear();
        self.pending_background_question_followups.clear();
        self.pending_skill_context = None;
        self.pending_skill_user_message_to_suppress = None;
        self.pending_plan_review_feedback.clear();
        self.clear_pending_exit_confirmation();
        self.close_inline_prompt_completion();
        self.tui.chrome_mut().clear_interrupted_turn_state();
        self.tui.chrome_mut().clear_todos();
        self.tui
            .chrome_mut()
            .set_main_agent_token_usage(neo_tui::shell::MainAgentTokenUsage::default());
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
        self.goal_manager = None;
        self.active_session_id = None;
        self.tui.chrome_mut().set_session_label("new");
        self.rebuild_empty_welcome_transcript();
    }

    /// Begin a fresh session transition from `/new` / `/clear`. Blocked (with a
    /// status message and no state change) when a turn is still running so we
    /// never drop an in-flight session's tool/approval state on the floor.
    pub(super) fn start_new_session_from_slash(&mut self) {
        if self.active_turn.is_some() {
            self.push_status(
                "Cannot start a new session while a turn is running. Press Esc to interrupt first.",
            );
            return;
        }
        self.close_inline_prompt_completion();
        self.reset_for_new_session();
        self.push_status("Started fresh session");
    }
}

fn empty_scope_toggle_message(scope: SessionDataScope) -> &'static str {
    match scope {
        SessionDataScope::Workspace => {
            "No sessions in current workspace. Press Ctrl+A again to switch back to all sessions."
        }
        SessionDataScope::All => {
            "No sessions in all sessions. Press Ctrl+A again to switch back to current workspace."
        }
    }
}
