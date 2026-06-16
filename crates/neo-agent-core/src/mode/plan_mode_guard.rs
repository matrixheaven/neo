use std::path::{Component, Path, PathBuf};

use crate::mode::plan::PlanModeState;
use crate::permissions::PermissionDecision;

/// Check whether a tool call should be allowed under plan mode.
///
/// When plan mode is active:
/// - **Write tools** (`write`, `edit`) are denied unless the target path
///   matches the plan file.
/// - **Shell tools** (`bash`, `terminal`) are always denied.
/// - **Read-only tools** and all other tools (including `enter_plan_mode` and
///   `exit_plan_mode`) are allowed.
///
/// When plan mode is **not** active this returns
/// [`PermissionDecision::Allow`] — the caller is expected to consult the
/// normal permission policy.
#[must_use]
pub fn check_plan_mode_guard(
    plan_mode: &PlanModeState,
    tool_name: &str,
    args: &serde_json::Value,
) -> PermissionDecision {
    if !plan_mode.is_active {
        return PermissionDecision::Allow;
    }

    match tool_name {
        "write" | "edit" => {
            if let Some(path) = args.get("path").and_then(serde_json::Value::as_str)
                && let Some(plan_path) = &plan_mode.plan_file_path
                && paths_match(plan_path, Path::new(path))
            {
                return PermissionDecision::Allow;
            }
            PermissionDecision::Deny
        }
        "bash" | "terminal" => PermissionDecision::Deny,
        _ => PermissionDecision::Allow,
    }
}

/// Compare two paths for equality after normalising `.` and `..` components.
fn paths_match(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}

/// Normalise a path by removing `.` components and resolving `..` components.
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

    fn active_state(plan_path: &std::path::Path) -> PlanModeState {
        PlanModeState {
            is_active: true,
            plan_file_path: Some(plan_path.to_path_buf()),
        }
    }

    #[test]
    fn inactive_mode_allows_everything() {
        let state = PlanModeState::default();
        assert!(!state.is_active);

        assert_eq!(
            check_plan_mode_guard(&state, "write", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            check_plan_mode_guard(&state, "bash", &json!({"command": "rm -rf /"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            check_plan_mode_guard(&state, "read", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn active_mode_allows_read_tools() {
        let state = active_state(Path::new("/tmp/plan.md"));

        assert_eq!(
            check_plan_mode_guard(&state, "read", &json!({"path": "foo.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            check_plan_mode_guard(&state, "list", &json!({"path": "."})),
            PermissionDecision::Allow
        );
        assert_eq!(
            check_plan_mode_guard(&state, "grep", &json!({"pattern": "foo"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            check_plan_mode_guard(&state, "find", &json!({"pattern": "*.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            check_plan_mode_guard(&state, "glob", &json!({"pattern": "*.rs"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn active_mode_denies_write() {
        let state = active_state(Path::new("/tmp/plan.md"));

        assert_eq!(
            check_plan_mode_guard(&state, "write", &json!({"path": "src/main.rs"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            check_plan_mode_guard(&state, "edit", &json!({"path": "src/main.rs"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn active_mode_denies_bash_and_terminal() {
        let state = active_state(Path::new("/tmp/plan.md"));

        assert_eq!(
            check_plan_mode_guard(&state, "bash", &json!({"command": "ls"})),
            PermissionDecision::Deny
        );
        assert_eq!(
            check_plan_mode_guard(&state, "terminal", &json!({"mode": "start"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn active_mode_allows_write_to_plan_file() {
        let plan_path = Path::new("/home/user/.neo/plans/abc123.md");
        let state = active_state(plan_path);

        assert_eq!(
            check_plan_mode_guard(
                &state,
                "write",
                &json!({"path": "/home/user/.neo/plans/abc123.md"})
            ),
            PermissionDecision::Allow
        );
        assert_eq!(
            check_plan_mode_guard(
                &state,
                "edit",
                &json!({"path": "/home/user/.neo/plans/abc123.md"})
            ),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn active_mode_denies_write_to_non_plan_file() {
        let plan_path = Path::new("/home/user/.neo/plans/abc123.md");
        let state = active_state(plan_path);

        assert_eq!(
            check_plan_mode_guard(
                &state,
                "write",
                &json!({"path": "/home/user/.neo/plans/abc123.md.bak"})
            ),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn active_mode_allows_mode_switch_tools() {
        let state = active_state(Path::new("/tmp/plan.md"));

        assert_eq!(
            check_plan_mode_guard(&state, "enter_plan_mode", &json!({})),
            PermissionDecision::Allow
        );
        assert_eq!(
            check_plan_mode_guard(
                &state,
                "exit_plan_mode",
                &json!({"plan_summary": "do stuff"})
            ),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn active_mode_allows_todo_tool() {
        let state = active_state(Path::new("/tmp/plan.md"));

        assert_eq!(
            check_plan_mode_guard(&state, "todo", &json!({"action": "add"})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn write_without_path_denied_in_active_mode() {
        let state = active_state(Path::new("/tmp/plan.md"));

        assert_eq!(
            check_plan_mode_guard(&state, "write", &json!({})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn write_with_non_string_path_denied_in_active_mode() {
        let state = active_state(Path::new("/tmp/plan.md"));

        assert_eq!(
            check_plan_mode_guard(&state, "write", &json!({"path": 42})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn normalize_handles_dot_dot() {
        assert_eq!(normalize(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
    }

    #[test]
    fn normalize_handles_dot() {
        assert_eq!(normalize(Path::new("/a/./b")), PathBuf::from("/a/b"));
    }

    #[test]
    fn paths_match_with_normalization() {
        assert!(paths_match(
            Path::new("/a/b/c.md"),
            Path::new("/a/b/../b/c.md")
        ));
    }

    #[test]
    fn paths_differ() {
        assert!(!paths_match(Path::new("/a/b/c.md"), Path::new("/a/b/d.md")));
    }
}
