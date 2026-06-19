use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The current agent execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentMode {
    #[default]
    Default,
    Plan,
}

/// Serializable snapshot of the current plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanData {
    pub id: String,
    pub path: PathBuf,
    pub content: String,
}

/// Variants of the plan-mode reminder injected into the model context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlanInjectionVariant {
    #[default]
    Full,
    Sparse,
    Reentry,
    Exit,
}

/// Plan mode state.
#[derive(Debug, Clone, Default)]
pub struct PlanMode {
    is_active: bool,
    plan_id: Option<String>,
    plan_file_path: Option<PathBuf>,
    assistant_turns_since_injection: u32,
    last_variant: Option<PlanInjectionVariant>,
    exit_reminder_emitted: bool,
}

impl PlanMode {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    #[must_use]
    pub fn plan_id(&self) -> Option<&str> {
        self.plan_id.as_deref()
    }

    #[must_use]
    pub fn plan_file_path(&self) -> Option<&Path> {
        self.plan_file_path.as_deref()
    }

    /// Enter plan mode without creating a plan file.
    pub fn enter_in_memory(&mut self) {
        self.is_active = true;
        self.plan_id = Some(Self::create_plan_id());
        self.plan_file_path = None;
        self.assistant_turns_since_injection = 0;
        self.last_variant = None;
        self.exit_reminder_emitted = false;
    }

    /// Enter plan mode, creating the plans dir and optionally an empty plan file.
    pub fn enter(&mut self, plans_dir: &Path, create_file: bool) -> std::io::Result<PlanData> {
        std::fs::create_dir_all(plans_dir)?;
        let id = Self::create_plan_id();
        let path = plans_dir.join(format!("{id}.md"));
        if create_file {
            std::fs::write(&path, "")?;
        }
        self.is_active = true;
        self.plan_id = Some(id);
        self.plan_file_path = Some(path.clone());
        self.assistant_turns_since_injection = 0;
        self.last_variant = None;
        self.exit_reminder_emitted = false;
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        Ok(PlanData {
            id: self.plan_id.clone().expect("just set"),
            path,
            content,
        })
    }

    /// Restore plan mode from a persisted plan id (resume).
    pub fn restore_enter(&mut self, plans_dir: &Path, id: &str) {
        self.is_active = true;
        self.plan_id = Some(id.to_owned());
        self.plan_file_path = Some(plans_dir.join(format!("{id}.md")));
        self.assistant_turns_since_injection = 0;
        self.last_variant = None;
        self.exit_reminder_emitted = false;
    }

    /// Exit plan mode but retain `plan_file_path`/`plan_id`.
    pub fn exit(&mut self) {
        self.is_active = false;
        self.assistant_turns_since_injection = 0;
    }

    /// Cancel plan mode, dropping `plan_id`/`path`.
    pub fn cancel(&mut self) {
        self.is_active = false;
        self.plan_id = None;
        self.plan_file_path = None;
        self.assistant_turns_since_injection = 0;
        self.last_variant = None;
        self.exit_reminder_emitted = false;
    }

    /// Clear the plan file content (empty string).
    pub fn clear(&mut self) -> std::io::Result<()> {
        if let Some(path) = &self.plan_file_path {
            std::fs::write(path, "")?;
        }
        Ok(())
    }

    /// Read current plan data.
    pub fn data(&self) -> std::io::Result<Option<PlanData>> {
        let Some(path) = &self.plan_file_path else {
            return Ok(None);
        };
        let content = std::fs::read_to_string(path).unwrap_or_default();
        Ok(Some(PlanData {
            id: self.plan_id.clone().unwrap_or_default(),
            path: path.clone(),
            content,
        }))
    }

    /// Decide which reminder variant to inject, if any.
    pub fn next_injection_variant(
        &mut self,
        assistant_turn_count: u32,
        user_message_just_appended: bool,
    ) -> Option<PlanInjectionVariant> {
        if self.is_active {
            let variant = if self.last_variant.is_none() || user_message_just_appended {
                // First injection since entering (or after a user message).
                // Use Reentry if plan file already has content, else Full.
                if self.has_plan_content() {
                    PlanInjectionVariant::Reentry
                } else {
                    PlanInjectionVariant::Full
                }
            } else if assistant_turn_count >= 5 {
                PlanInjectionVariant::Full
            } else if assistant_turn_count >= 2 {
                PlanInjectionVariant::Sparse
            } else {
                return None;
            };
            self.assistant_turns_since_injection = 0;
            self.last_variant = Some(variant);
            Some(variant)
        } else if self.last_variant.is_some() && !self.exit_reminder_emitted {
            self.exit_reminder_emitted = true;
            Some(PlanInjectionVariant::Exit)
        } else {
            None
        }
    }

    /// Check if the plan file exists and has non-empty content.
    fn has_plan_content(&self) -> bool {
        if let Some(path) = &self.plan_file_path {
            std::fs::read_to_string(path).is_ok_and(|content| !content.trim().is_empty())
        } else {
            false
        }
    }

    pub fn increment_assistant_turns(&mut self) {
        self.assistant_turns_since_injection += 1;
    }

    fn create_plan_id() -> String {
        Uuid::new_v4().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_inactive() {
        let s = PlanMode::default();
        assert!(!s.is_active());
        assert!(s.plan_file_path().is_none());
        assert!(s.plan_id().is_none());
    }
    #[test]
    fn enter_creates_plan_file() {
        let d = tempfile::tempdir().unwrap();
        let mut s = PlanMode::default();
        let data = s.enter(d.path(), true).unwrap();
        assert!(s.is_active());
        assert_eq!(s.plan_id(), Some(data.id.as_str()));
        assert!(std::fs::read_to_string(&data.path).unwrap().is_empty());
    }
    #[test]
    fn exit_retains_path() {
        let d = tempfile::tempdir().unwrap();
        let mut s = PlanMode::default();
        let data = s.enter(d.path(), true).unwrap();
        s.exit();
        assert!(!s.is_active());
        assert_eq!(s.plan_file_path(), Some(data.path.as_ref()));
    }
    #[test]
    fn cancel_drops_path() {
        let d = tempfile::tempdir().unwrap();
        let mut s = PlanMode::default();
        s.enter(d.path(), true).unwrap();
        s.cancel();
        assert!(!s.is_active());
        assert!(s.plan_file_path().is_none());
    }
    #[test]
    fn clear_empties_file() {
        let d = tempfile::tempdir().unwrap();
        let mut s = PlanMode::default();
        let data = s.enter(d.path(), true).unwrap();
        std::fs::write(&data.path, "x").unwrap();
        s.clear().unwrap();
        assert!(std::fs::read_to_string(&data.path).unwrap().is_empty());
    }
    #[test]
    fn restore_enter_reconstructs() {
        let d = tempfile::tempdir().unwrap();
        let mut s = PlanMode::default();
        s.restore_enter(d.path(), "abc");
        assert!(s.is_active());
        assert_eq!(s.plan_id(), Some("abc"));
    }
    #[test]
    fn injection_cadence() {
        let mut s = PlanMode::default();
        s.enter(Path::new("/tmp"), false).unwrap();
        assert_eq!(
            s.next_injection_variant(0, false),
            Some(PlanInjectionVariant::Full)
        );
        assert_eq!(s.next_injection_variant(1, false), None);
        assert_eq!(
            s.next_injection_variant(2, false),
            Some(PlanInjectionVariant::Sparse)
        );
        assert_eq!(
            s.next_injection_variant(5, false),
            Some(PlanInjectionVariant::Full)
        );
    }
    #[test]
    fn injection_exit_once() {
        let mut s = PlanMode::default();
        s.enter(Path::new("/tmp"), false).unwrap();
        s.next_injection_variant(0, false);
        s.exit();
        assert_eq!(
            s.next_injection_variant(0, false),
            Some(PlanInjectionVariant::Exit)
        );
        assert_eq!(s.next_injection_variant(0, false), None);
    }
}
