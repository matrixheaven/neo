use std::path::{Component, Path, PathBuf};

use crate::mode::plan::PlanMode;
use crate::permissions::PermissionDecision;

#[must_use]
pub fn check_plan_mode_guard(
    plan_mode: &PlanMode,
    tool_name: &str,
    args: &serde_json::Value,
) -> PermissionDecision {
    if !plan_mode.is_active() {
        return PermissionDecision::Allow;
    }
    match tool_name {
        "Write" | "Edit" => {
            if let Some(path) = args.get("path").and_then(serde_json::Value::as_str)
                && let Some(plan_path) = plan_mode.plan_file_path()
                && paths_match(plan_path, Path::new(path))
            {
                return PermissionDecision::Allow;
            }
            PermissionDecision::Deny
        }
        "Bash" | "Terminal" | "TaskStop" | "CronCreate" | "CronDelete" => PermissionDecision::Deny,
        _ => PermissionDecision::Allow,
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
    use serde_json::json;

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
        assert_eq!(
            check_plan_mode_guard(&s, "Write", &json!({"path":"a"})),
            PermissionDecision::Allow
        );
    }
    #[test]
    fn active_denies_write() {
        let s = active_state(Path::new("/tmp/p.md"));
        assert_eq!(
            check_plan_mode_guard(&s, "Write", &json!({"path":"a"})),
            PermissionDecision::Deny
        );
    }
    #[test]
    fn active_allows_plan_file_write() {
        let s = active_state(Path::new("/h/p.md"));
        assert_eq!(
            check_plan_mode_guard(&s, "Write", &json!({"path":"/h/p.md"})),
            PermissionDecision::Allow
        );
    }
    #[test]
    fn active_denies_bash() {
        let s = active_state(Path::new("/tmp/p.md"));
        assert_eq!(
            check_plan_mode_guard(&s, "Bash", &json!({})),
            PermissionDecision::Deny
        );
    }
}
