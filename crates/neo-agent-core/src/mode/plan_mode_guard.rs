use std::path::{Component, Path, PathBuf};

use crate::mode::plan::PlanMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanModeGuard {
    Allow,
    Deny { message: String },
}

/// Returns `true` if `tool_path` refers to the active plan file.
/// Relative paths are resolved against `workspace_root` (if provided) before
/// comparison.
#[must_use]
pub fn is_active_plan_file_path(
    plan_mode: &PlanMode,
    workspace_root: Option<&Path>,
    tool_path: &str,
) -> bool {
    let Some(plan_path) = plan_mode.plan_file_path() else {
        return false;
    };
    let candidate = Path::new(tool_path);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else if let Some(root) = workspace_root {
        root.join(candidate)
    } else {
        candidate.to_path_buf()
    };
    paths_match(plan_path, &resolved)
}

#[must_use]
pub fn check_plan_mode_guard(plan_mode: &PlanMode, tool_name: &str) -> PlanModeGuard {
    if !plan_mode.is_active() {
        return PlanModeGuard::Allow;
    }
    match tool_name {
        // Write and Edit resolve to prepared target sets that this raw guard does
        // not own; the single-plan-file exception is applied on the prepared
        // execution path. Any raw Write/Edit is denied here.
        "Write" | "Edit" => plan_mode_write_deny(plan_mode),
        "TaskStop" | "CronCreate" | "CronDelete" => PlanModeGuard::Deny {
            message: format!("blocked by plan mode: {tool_name} is not allowed while planning"),
        },
        _ => PlanModeGuard::Allow,
    }
}

fn plan_mode_write_deny(plan_mode: &PlanMode) -> PlanModeGuard {
    let plan_path = plan_mode.plan_file_path().map_or_else(
        || "(no plan file selected yet)".to_owned(),
        |p| p.display().to_string(),
    );
    PlanModeGuard::Deny {
        message: format!(
            "Plan mode is active. You may only write to the current plan file: \
             {plan_path}. Call ExitPlanMode to exit plan mode before editing other files."
        ),
    }
}

fn paths_match(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_state(plan_path: &std::path::Path) -> PlanMode {
        let mut state = PlanMode::default();
        let dir = plan_path.parent().unwrap_or(std::path::Path::new("/tmp"));
        let id = plan_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("x");
        state.restore_enter(dir, id);
        state
    }

    #[test]
    fn inactive_allows() {
        let s = PlanMode::default();
        assert!(matches!(
            check_plan_mode_guard(&s, "Write"),
            PlanModeGuard::Allow
        ));
    }

    #[test]
    fn active_denies_write_and_edit() {
        let s = active_state(Path::new("/tmp/p.md"));
        for tool in ["Write", "Edit"] {
            assert!(
                matches!(check_plan_mode_guard(&s, tool), PlanModeGuard::Deny { .. }),
                "{tool} must be denied by the raw guard"
            );
        }
    }

    #[test]
    fn active_allows_bash() {
        let s = active_state(Path::new("/tmp/p.md"));
        assert!(matches!(
            check_plan_mode_guard(&s, "Bash"),
            PlanModeGuard::Allow
        ));
    }

    #[test]
    fn active_denies_task_stop() {
        let s = active_state(Path::new("/tmp/p.md"));
        assert!(matches!(
            check_plan_mode_guard(&s, "TaskStop"),
            PlanModeGuard::Deny { .. }
        ));
    }
}
