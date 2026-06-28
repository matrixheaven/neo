//! Extracted: permission mode, development mode (plan/goal) state transitions and commands.

use std::path::PathBuf;
use std::sync::Arc;

use neo_agent_core::{
    PermissionMode,
    goal::GoalManager,
};
use neo_tui::shell::{DevelopmentMode, GoalModeStatus};

use super::InteractiveController;

fn permission_mode_items() -> Vec<neo_tui::dialogs::ChoiceItem> {
    vec![
        neo_tui::dialogs::ChoiceItem::new(
            "permission:ask",
            "Ask",
        )
        .with_description("Ask before commands, edits, and other risky actions. Read/search tools run directly; session approval rules are respected."),
        neo_tui::dialogs::ChoiceItem::new(
            "permission:auto",
            "Auto",
        )
        .with_description("Run fully non-interactively. Tool actions are approved automatically, and agent questions are skipped so it can decide on its own."),
        neo_tui::dialogs::ChoiceItem::new(
            "permission:yolo",
            "YOLO",
        )
        .with_description("Automatically approve tool actions and plan transitions. The agent can still ask you explicit questions when your input is needed."),
    ]
}

impl InteractiveController {
    pub(super) fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_mode = mode;
        if let Ok(mut live) = self.live_permission_mode.write() {
            *live = mode;
        }
        self.tui.chrome_mut().set_permission_mode(mode);
        self.push_status(format!("Permission Mode: {}", mode.label()));
    }

    pub(super) fn open_permission_picker(&mut self) {
        let current_id = format!("permission:{}", self.permission_mode.label());
        let items = permission_mode_items();
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
                title: "Select permission mode".to_owned(),
                items,
                initial_id: Some(current_id.clone()),
                theme,
                page_size: 3,
                current_id: Some(current_id),
            });
    }

    pub(super) fn set_plan_mode_from_user(&mut self, active: bool) {
        self.sync_runtime_plan_mode(active);
        self.tui.chrome_mut().set_plan_mode(active);
        self.push_status(if active {
            "Plan Mode On"
        } else {
            "Plan Mode Off"
        });
    }

    pub(super) fn sync_runtime_plan_mode(&mut self, active: bool) {
        let Ok(mut plan_mode) = self.plan_mode.write() else {
            self.push_status("Plan mode state unavailable");
            return;
        };
        if active {
            if plan_mode.is_active() {
                return;
            }
            if let Some(plans_dir) = self.plan_mode_plans_dir() {
                if plan_mode.enter(&plans_dir, true).is_err() {
                    plan_mode.enter_in_memory();
                }
            } else {
                plan_mode.enter_in_memory();
            }
        } else if plan_mode.is_active() {
            plan_mode.exit();
        }
    }

    fn plan_mode_plans_dir(&self) -> Option<PathBuf> {
        Some(self.active_session_directory()?.join("plans"))
    }

    pub(super) fn cycle_development_mode(&mut self) {
        match self.tui.chrome().development_mode() {
            DevelopmentMode::Normal => self.set_plan_mode_from_user(true),
            DevelopmentMode::Plan => {
                self.set_plan_mode_from_user(false);
                self.tui
                    .chrome_mut()
                    .set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Pending));
                self.push_status("Goal Mode On");
            }
            DevelopmentMode::Goal(_) => {
                self.tui
                    .chrome_mut()
                    .set_development_mode(DevelopmentMode::Normal);
                self.push_status("Goal Mode Off");
            }
        }
    }

    pub(super) fn toggle_plan_mode_from_user(&mut self) {
        let next = !self.tui.chrome_mut().is_plan_mode();
        self.set_plan_mode_from_user(next);
    }

    pub(super) fn push_unknown_plan_argument(&mut self, arg: &str) {
        self.push_status(format!(
            "Unknown /plan argument: '{arg}'. Usage: /plan [on|off|clear]"
        ));
    }

    pub(super) fn clear_plan_file(&mut self) {
        let cleared = self
            .plan_mode
            .write()
            .ok()
            .and_then(|mut plan_mode| plan_mode.clear().ok())
            .is_some();
        self.push_status(if cleared {
            "Plan file cleared"
        } else {
            "No plan file to clear"
        });
    }

    pub(super) async fn handle_goal_command(&mut self, arg: &str) -> bool {
        let Some(manager) = self.goal_manager().await else {
            return true;
        };
        if self.handle_goal_lifecycle_command(&manager, arg).await {
            return true;
        }
        self.handle_goal_objective_command(&manager, arg.trim())
            .await
    }

    async fn handle_goal_lifecycle_command(&mut self, manager: &GoalManager, arg: &str) -> bool {
        if self.handle_goal_status_command(manager, arg) {
            return true;
        }
        self.handle_goal_state_command(manager, arg).await
    }

    fn handle_goal_status_command(&mut self, manager: &GoalManager, arg: &str) -> bool {
        if matches!(arg, "" | "status") {
            self.show_goal_status(manager);
            return true;
        }
        false
    }

    async fn handle_goal_state_command(&mut self, manager: &GoalManager, arg: &str) -> bool {
        match arg {
            "pause" => {
                self.pause_goal(manager).await;
                true
            }
            "resume" => {
                self.resume_goal(manager).await;
                true
            }
            "cancel" => {
                self.cancel_goal(manager).await;
                true
            }
            _ => false,
        }
    }

    async fn goal_manager(&mut self) -> Option<Arc<GoalManager>> {
        if self.goal_manager.is_none()
            && let Some(session_dir) = self.active_session_directory()
        {
            match GoalManager::load(session_dir).await {
                Ok(manager) => self.goal_manager = Some(Arc::new(manager)),
                Err(err) => {
                    self.push_status(format!("Failed to load goal manager: {err}"));
                    return None;
                }
            }
        }
        let Some(manager) = self.goal_manager.clone() else {
            self.push_status("Goal mode is not available");
            return None;
        };
        Some(manager)
    }

    fn show_goal_status(&mut self, manager: &GoalManager) -> bool {
        match manager.active() {
            Some(goal) => self.push_status(format!(
                "Goal: {} | status: {:?}",
                goal.objective, goal.status
            )),
            None => self.push_status("No active goal."),
        }
        true
    }

    async fn pause_goal(&mut self, manager: &GoalManager) {
        match manager.pause().await {
            Ok(Some(goal)) => self.push_goal_status("⏸ Goal paused", &goal.objective),
            Ok(None) => self.push_status("No active goal to pause"),
            Err(err) => self.push_status(format!("Failed to pause goal: {err}")),
        }
    }

    async fn resume_goal(&mut self, manager: &GoalManager) {
        match manager.resume().await {
            Ok(Some(goal)) => self.push_goal_status("▶ Goal resumed", &goal.objective),
            Ok(None) => self.push_status("No active goal to resume"),
            Err(err) => self.push_status(format!("Failed to resume goal: {err}")),
        }
    }

    async fn cancel_goal(&mut self, manager: &GoalManager) {
        match manager.cancel().await {
            Ok(Some(goal)) => self.push_goal_status("⏹ Goal cancelled", &goal.objective),
            Ok(None) => self.push_status("No active goal to cancel"),
            Err(err) => self.push_status(format!("Failed to cancel goal: {err}")),
        }
    }

    async fn handle_goal_objective_command(
        &mut self,
        manager: &GoalManager,
        command: &str,
    ) -> bool {
        if let Some(objective) = command.strip_prefix("replace ") {
            return self.replace_goal(manager, objective.trim()).await;
        }
        if let Some(objective) = command.strip_prefix("next ") {
            return self.queue_next_goal(manager, objective.trim()).await;
        }
        self.start_goal(manager, command).await
    }

    async fn replace_goal(&mut self, manager: &GoalManager, objective: &str) -> bool {
        let goal = neo_agent_core::goal::Goal::new(objective);
        match manager.replace(goal).await {
            Ok(Some(_previous)) => self.push_status(format!("Replaced goal with: {objective}")),
            Ok(None) => self.push_status(format!("Started goal: {objective}")),
            Err(err) => {
                self.push_status(format!("Failed to replace goal: {err}"));
                return true;
            }
        }
        false
    }

    async fn queue_next_goal(&mut self, manager: &GoalManager, objective: &str) -> bool {
        let goal = neo_agent_core::goal::Goal::new(objective);
        match manager.queue_next(goal).await {
            Ok(()) => self.push_status(format!("Queued goal: {objective}")),
            Err(err) => {
                self.push_status(format!("Failed to queue goal: {err}"));
                return true;
            }
        }
        true
    }

    async fn start_goal(&mut self, manager: &GoalManager, objective: &str) -> bool {
        let goal = neo_agent_core::goal::Goal::new(objective);
        let objective = goal.objective.clone();
        match manager.start(goal).await {
            Ok(Some(_previous)) => {
                self.push_status(format!(
                    "Started goal: {objective} (previous goal superseded)"
                ));
            }
            Ok(None) => self.push_status(format!("Started goal: {objective}")),
            Err(err) => {
                self.push_status(format!("Failed to start goal: {err}"));
                return true;
            }
        }
        self.push_goal_status("▶ Goal started", &objective);
        false
    }

    fn push_goal_status(&mut self, prefix: &str, objective: &str) {
        self.transcript_mut()
            .push_transcript(neo_tui::transcript::TranscriptEntry::status(format!(
                "{prefix}: {objective}"
            )));
    }
}
