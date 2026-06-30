use neo_agent_core::multi_agent::{
    AgentProfile, AgentRole, ToolPolicy, is_git_mutation_command, is_read_only_shell_command,
};
use neo_agent_core::tools::ToolRegistry;
use std::sync::{Arc, Mutex};

fn builtin_registry() -> ToolRegistry {
    ToolRegistry::with_builtin_tools_and_todos(Arc::new(Mutex::new(Vec::new())))
}

#[test]
fn built_in_profiles_have_expected_labels_and_tool_policies() {
    let explorer = AgentProfile::for_role(AgentRole::Explorer);
    assert_eq!(explorer.display_label, "Explorer");
    assert_eq!(explorer.tool_policy, ToolPolicy::ReadOnlyShell);
    assert!(explorer.allowed_tools.contains("Read"));
    assert!(explorer.allowed_tools.contains("Bash"));
    assert!(!explorer.allowed_tools.contains("Write"));
    assert!(!explorer.allowed_tools.contains("Edit"));

    let planner = AgentProfile::for_role(AgentRole::Planner);
    assert_eq!(planner.display_label, "Planner");
    assert_eq!(planner.tool_policy, ToolPolicy::NoShell);
    assert!(!planner.allowed_tools.contains("Bash"));
    assert!(!planner.allowed_tools.contains("Write"));

    let orchestrator = AgentProfile::for_role(AgentRole::Orchestrator);
    assert_eq!(orchestrator.display_label, "Orchestrator");
    assert!(orchestrator.allowed_tools.contains("Delegate"));
    assert!(orchestrator.allowed_tools.contains("DelegateSwarm"));
    assert!(!orchestrator.allowed_tools.contains("Bash"));
    assert!(!orchestrator.allowed_tools.contains("Edit"));
}

#[test]
fn child_tool_registry_for_planner_excludes_bash_and_edit_tools() {
    let registry = builtin_registry();
    let filtered = registry.filtered_for_agent_role(AgentRole::Planner);
    let names = filtered
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    assert!(names.contains(&"Read".to_owned()));
    assert!(!names.contains(&"Bash".to_owned()));
    assert!(!names.contains(&"Write".to_owned()));
    assert!(!names.contains(&"Edit".to_owned()));
}

#[test]
fn child_tool_registry_for_orchestrator_contains_coordination_tools_only() {
    let registry = builtin_registry();
    let filtered = registry.filtered_for_agent_role(AgentRole::Orchestrator);
    let names = filtered
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    assert!(names.contains(&"Delegate".to_owned()));
    assert!(names.contains(&"DelegateSwarm".to_owned()));
    assert!(names.contains(&"WaitDelegate".to_owned()));
    assert!(!names.contains(&"Bash".to_owned()));
    assert!(!names.contains(&"Write".to_owned()));
    assert!(!names.contains(&"Edit".to_owned()));
}

#[test]
fn read_only_shell_classifier_allows_known_read_commands() {
    assert!(is_read_only_shell_command("ls crates"));
    assert!(is_read_only_shell_command("find crates -name '*.rs'"));
    assert!(is_read_only_shell_command(
        "rg Delegate crates/neo-agent-core/src"
    ));
    assert!(is_read_only_shell_command("git status --short"));
    assert!(is_read_only_shell_command(
        "git diff -- crates/neo-agent-core/src/tools/delegate.rs"
    ));
    assert!(is_read_only_shell_command("git log -1 --oneline"));
    assert!(is_read_only_shell_command(
        "git blame crates/neo-agent-core/src/lib.rs"
    ));
    assert!(is_read_only_shell_command("git branch --show-current"));
}

#[test]
fn read_only_shell_classifier_rejects_mutating_commands() {
    assert!(!is_read_only_shell_command("git add ."));
    assert!(!is_read_only_shell_command("git commit -m change"));
    assert!(!is_read_only_shell_command(
        "git checkout -- crates/neo-agent-core/src/lib.rs"
    ));
    assert!(!is_read_only_shell_command("rm -rf target/tmp"));
    assert!(!is_read_only_shell_command("git branch -D stale-work"));
    assert!(!is_read_only_shell_command("rg stale crates | xargs rm -f"));
    assert!(!is_read_only_shell_command(
        "python - <<'PY'\nopen('x','w').write('x')\nPY"
    ));
    assert!(!is_read_only_shell_command("cargo fmt"));
}

#[test]
fn git_mutation_classifier_rejects_wrapped_git_mutations() {
    assert!(is_git_mutation_command("bash -lc 'git add .'"));
    assert!(is_git_mutation_command("sh -c \"git commit -m nope\""));
    assert!(is_git_mutation_command("env git reset --hard"));
    assert!(!is_git_mutation_command("git status --short"));
}
