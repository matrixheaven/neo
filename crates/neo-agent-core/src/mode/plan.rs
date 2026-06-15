use std::path::{Path, PathBuf};

use uuid::Uuid;

/// The current agent execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentMode {
    /// Normal execution mode — all tools are available subject to the
    /// configured permission policy.
    #[default]
    Default,
    /// Plan mode — read-only exploration plus plan file writes.
    Plan,
}

/// Plan mode state — tracks whether plan mode is active and the plan file
/// path.
///
/// The runtime holds this state and consults it (via
/// [`check_plan_mode_guard`][super::plan_mode_guard::check_plan_mode_guard])
/// before executing tool calls.
#[derive(Debug, Clone, Default)]
pub struct PlanModeState {
    /// Whether plan mode is currently active.
    pub is_active: bool,
    /// Path to the plan file created when plan mode was entered.
    /// Retained after [`exit`][Self::exit] so callers can still read the plan.
    pub plan_file_path: Option<PathBuf>,
}

impl PlanModeState {
    /// Enter plan mode.
    ///
    /// Creates a `plans/` directory under `homedir`, writes an initial plan
    /// template, and records the path.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the directory cannot be created or the
    /// template file cannot be written.
    pub fn enter(&mut self, homedir: &Path) -> std::io::Result<PathBuf> {
        let plans_dir = homedir.join("plans");
        std::fs::create_dir_all(&plans_dir)?;
        let plan_id = Uuid::new_v4();
        let plan_path = plans_dir.join(format!("{plan_id}.md"));
        std::fs::write(
            &plan_path,
            "# Plan\n\n<!-- Describe the implementation plan here -->\n",
        )?;
        self.is_active = true;
        self.plan_file_path = Some(plan_path.clone());
        Ok(plan_path)
    }

    /// Exit plan mode.
    ///
    /// Sets `is_active` to `false` but retains `plan_file_path` so callers
    /// can still read the plan content after exiting.
    pub fn exit(&mut self) {
        self.is_active = false;
    }

    /// Read the current plan file content, if any.
    #[must_use]
    pub fn read_plan(&self) -> Option<String> {
        let path = self.plan_file_path.as_ref()?;
        std::fs::read_to_string(path).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_inactive() {
        let state = PlanModeState::default();
        assert!(!state.is_active);
        assert!(state.plan_file_path.is_none());
    }

    #[test]
    fn enter_creates_plan_file_and_sets_active() {
        let homedir = tempfile::tempdir().expect("tempdir");
        let mut state = PlanModeState::default();

        let plan_path = state.enter(homedir.path()).expect("enter");

        assert!(state.is_active);
        assert_eq!(state.plan_file_path, Some(plan_path.clone()));
        assert!(plan_path.starts_with(homedir.path().join("plans")));
        assert!(plan_path.extension().is_some_and(|ext| ext == "md"));
        let content = std::fs::read_to_string(&plan_path).expect("read plan file");
        assert!(content.starts_with("# Plan"));
    }

    #[test]
    fn enter_is_idempotent_creates_new_file() {
        let homedir = tempfile::tempdir().expect("tempdir");
        let mut state = PlanModeState::default();

        let first = state.enter(homedir.path()).expect("enter");
        let second = state.enter(homedir.path()).expect("enter again");

        assert_ne!(first, second);
        assert_eq!(state.plan_file_path, Some(second));
    }

    #[test]
    fn exit_clears_active_but_retains_path() {
        let homedir = tempfile::tempdir().expect("tempdir");
        let mut state = PlanModeState::default();
        let plan_path = state.enter(homedir.path()).expect("enter");

        state.exit();

        assert!(!state.is_active);
        assert_eq!(state.plan_file_path, Some(plan_path));
    }

    #[test]
    fn read_plan_returns_content() {
        let homedir = tempfile::tempdir().expect("tempdir");
        let mut state = PlanModeState::default();
        state.enter(homedir.path()).expect("enter");

        let content = state.read_plan().expect("read_plan");
        assert!(content.contains("# Plan"));
    }

    #[test]
    fn read_plan_returns_none_without_path() {
        let state = PlanModeState::default();
        assert!(state.read_plan().is_none());
    }

    #[test]
    fn read_plan_returns_none_for_missing_file() {
        let homedir = tempfile::tempdir().expect("tempdir");
        let mut state = PlanModeState::default();
        state.enter(homedir.path()).expect("enter");
        // Delete the plan file to simulate a missing file.
        std::fs::remove_file(state.plan_file_path.as_ref().unwrap()).expect("remove");
        assert!(state.read_plan().is_none());
    }

    #[test]
    fn enter_creates_plans_subdirectory() {
        let homedir = tempfile::tempdir().expect("tempdir");
        let plans_dir = homedir.path().join("plans");
        assert!(!plans_dir.exists());

        let mut state = PlanModeState::default();
        state.enter(homedir.path()).expect("enter");

        assert!(plans_dir.exists());
        assert!(plans_dir.is_dir());
    }

    #[test]
    fn agent_mode_default_is_default() {
        assert_eq!(AgentMode::default(), AgentMode::Default);
    }

    #[test]
    fn agent_mode_variants() {
        assert_ne!(AgentMode::Default, AgentMode::Plan);
    }
}
