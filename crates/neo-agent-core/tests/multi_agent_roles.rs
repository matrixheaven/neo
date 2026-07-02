use neo_agent_core::multi_agent::{AgentProfile, AgentRole, ToolPolicy};
use neo_agent_core::tools::ToolRegistry;
use std::sync::{Arc, Mutex};

fn builtin_registry() -> ToolRegistry {
    ToolRegistry::with_builtin_tools_and_todos(Arc::new(Mutex::new(Vec::new())))
}

#[test]
fn built_in_profiles_have_expected_labels_and_tool_policies() {
    let coder = AgentProfile::for_role(AgentRole::Coder);
    assert_eq!(coder.display_label, "Coder");
    assert_eq!(coder.tool_policy, ToolPolicy::FULL_ACCESS);
    assert!(coder.allowed_tools.contains("Bash"));
    assert!(coder.allowed_tools.contains("Write"));
    assert!(coder.allowed_tools.contains("Edit"));

    let explorer = AgentProfile::for_role(AgentRole::Explorer);
    assert_eq!(explorer.display_label, "Explorer");
    assert_eq!(explorer.tool_policy, ToolPolicy::READ_ONLY_WITH_SHELL);
    assert!(explorer.allowed_tools.contains("Read"));
    assert!(explorer.allowed_tools.contains("Bash"));
    assert!(!explorer.allowed_tools.contains("Write"));
    assert!(!explorer.allowed_tools.contains("Edit"));

    let reviewer = AgentProfile::for_role(AgentRole::Reviewer);
    assert_eq!(reviewer.display_label, "Reviewer");
    assert_eq!(reviewer.tool_policy, ToolPolicy::READ_ONLY_WITH_SHELL);
    assert!(reviewer.allowed_tools.contains("Bash"));
    assert!(!reviewer.allowed_tools.contains("Write"));
    assert!(!reviewer.allowed_tools.contains("Edit"));

    let planner = AgentProfile::for_role(AgentRole::Planner);
    assert_eq!(planner.display_label, "Planner");
    assert_eq!(planner.tool_policy, ToolPolicy::READ_ONLY);
    assert!(!planner.allowed_tools.contains("Bash"));
    assert!(!planner.allowed_tools.contains("Write"));
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
fn profiles_carry_non_empty_when_to_use_guidance() {
    for role in AgentRole::ALL {
        let profile = AgentProfile::for_role(role);
        assert!(
            !profile.when_to_use.trim().is_empty(),
            "role {role:?} missing when_to_use"
        );
    }
}

#[test]
fn role_selection_guide_mentions_every_role() {
    let guide = AgentProfile::role_selection_guide();
    assert!(guide.contains("When to use each role:"));
    for role in AgentRole::ALL {
        assert!(
            guide.contains(&format!("- {}:", role.as_str())),
            "guide missing role {}",
            role.as_str()
        );
    }
}

#[test]
fn delegate_and_swarm_schemas_surface_role_guide() {
    let registry = builtin_registry();
    let by_name = |name: &str| {
        registry
            .specs()
            .into_iter()
            .find(|spec| spec.name == name)
            .map(|spec| spec.input_schema)
            .expect("tool registered")
    };

    for tool_name in ["Delegate", "DelegateSwarm"] {
        let schema = by_name(tool_name);
        let desc = schema
            .get("properties")
            .and_then(|p| p.get("role"))
            .and_then(|r| r.get("description"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{tool_name} role field has a description"));
        // The original per-field text is preserved alongside the appended guide.
        assert!(
            desc.contains("Defaults to coder"),
            "{tool_name} original role text dropped"
        );
        assert!(
            desc.contains("When to use each role:"),
            "{tool_name} missing guide"
        );
        for role in AgentRole::ALL {
            assert!(
                desc.contains(&format!("- {}:", role.as_str())),
                "{tool_name} guide missing role {}",
                role.as_str()
            );
        }
    }
}
