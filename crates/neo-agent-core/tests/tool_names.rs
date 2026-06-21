use std::sync::Arc;

use neo_agent_core::{
    CreateSkillTool, ExitGoalModeTool, GetGoalStatusTool, ListSkillsTool, MoveSkillTool,
    StartGoalTool, SummarizeSessionsTool, Tool, ToolRegistry, UpdateGoalStatusTool,
    goal::GoalManager,
};

fn assert_model_function_name_safe(name: &str) {
    assert!(
        name.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'),
        "tool name `{name}` must be safe for production model function-name APIs"
    );
}

#[test]
fn built_in_and_skill_management_tool_names_are_model_function_safe() {
    for spec in ToolRegistry::with_builtin_tools().specs() {
        assert_model_function_name_safe(&spec.name);
    }

    let list_skills = ListSkillsTool::new(".", None, Vec::new());
    assert_model_function_name_safe(list_skills.name());

    let create_skill = CreateSkillTool::new(".");
    assert_model_function_name_safe(create_skill.name());

    let move_skill = MoveSkillTool::new(".");
    assert_model_function_name_safe(move_skill.name());

    let summarize_sessions = SummarizeSessionsTool::new(".");
    assert_model_function_name_safe(summarize_sessions.name());
}

#[tokio::test]
async fn optional_tool_names_use_model_facing_kimi_style_casing() {
    let workspace = tempfile::tempdir().expect("workspace");
    let goal_manager = Arc::new(
        GoalManager::load(workspace.path().to_path_buf())
            .await
            .unwrap(),
    );
    let tools: Vec<Box<dyn Tool>> = vec![
        Box::new(ListSkillsTool::new(".", None, Vec::new())),
        Box::new(CreateSkillTool::new(".")),
        Box::new(MoveSkillTool::new(".")),
        Box::new(SummarizeSessionsTool::new(".")),
        Box::new(StartGoalTool::new(Arc::clone(&goal_manager))),
        Box::new(ExitGoalModeTool::new(Arc::clone(&goal_manager))),
        Box::new(UpdateGoalStatusTool::new(Arc::clone(&goal_manager))),
        Box::new(GetGoalStatusTool::new(goal_manager)),
    ];

    let mut names = tools
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect::<Vec<_>>();
    names.sort();

    assert_eq!(
        names,
        vec![
            "CreateSkill",
            "ExitGoalMode",
            "GetGoalStatus",
            "ListSkills",
            "MoveSkill",
            "StartGoal",
            "SummarizeSessions",
            "UpdateGoalStatus",
        ]
    );
}
