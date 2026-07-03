use neo_agent_core::{
    AgentContext, AgentMessage,
    goal::{Goal, GoalManager, GoalStatus, load_goal_store, save_goal},
};

#[tokio::test]
async fn goal_manager_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();

    assert!(manager.active().is_none());

    let goal = Goal::new("fix tests");
    let objective = goal.objective.clone();
    manager.start(goal).await.unwrap();

    let active = manager.active().unwrap();
    assert_eq!(active.objective, objective);
    assert!(matches!(active.status, GoalStatus::Active));

    manager.pause().await.unwrap();
    assert!(matches!(
        manager.active().unwrap().status,
        GoalStatus::Paused
    ));

    manager.resume().await.unwrap();
    assert!(matches!(
        manager.active().unwrap().status,
        GoalStatus::Active
    ));

    manager
        .update_status(GoalStatus::Blocked, Some("need input".into()))
        .await
        .unwrap();
    assert!(matches!(
        manager.active().unwrap().status,
        GoalStatus::Blocked
    ));

    manager
        .update_status(GoalStatus::Complete, None)
        .await
        .unwrap();
    assert!(manager.active().is_none());
}

#[tokio::test]
async fn goal_persists_to_disk() {
    let temp = tempfile::tempdir().unwrap();
    let goal = Goal::new("persist me").with_completion_criterion("tests pass");
    save_goal(temp.path(), &goal).await.unwrap();

    let store = load_goal_store(temp.path()).await.unwrap();
    let active = store.active().unwrap();
    assert_eq!(active.objective, "persist me");
    assert_eq!(active.completion_criterion, Some("tests pass".into()));
    assert!(
        temp.path()
            .join("agents/main/goals")
            .join(format!("{}.json", active.id))
            .is_file()
    );
    assert!(
        !temp
            .path()
            .join("goals")
            .join(format!("{}.json", active.id))
            .exists()
    );
}

#[tokio::test]
async fn goal_start_creates_supergoal_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();

    manager.start(Goal::new("ship goal mode")).await.unwrap();

    let active = manager.active().unwrap();
    let artifact_dir = active.artifact_dir.as_ref().expect("artifact dir");
    assert!(artifact_dir.ends_with(&active.id));
    assert!(artifact_dir.starts_with(temp.path().join("agents/main/goals/runs")));
    for relative in [
        "GOAL.md",
        "ROADMAP.md",
        "STATE.md",
        "THINKING.md",
        "PROTOCOL.md",
        "phases/phase-1.md",
    ] {
        assert!(
            artifact_dir.join(relative).exists(),
            "missing artifact {relative}"
        );
    }
    assert_eq!(active.raw_prompt.as_deref(), Some("ship goal mode"));
    assert_eq!(active.approved_text.as_deref(), Some("ship goal mode"));
    assert_eq!(active.current_phase, Some(0));
    assert_eq!(active.failure_strikes, 0);
    assert_eq!(active.audit_rounds, 0);
}

#[tokio::test]
async fn goal_manager_queues_goals() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();

    let first = Goal::new("first");
    let second = Goal::new("second");
    manager.start(first).await.unwrap();
    manager.queue_next(second).await.unwrap();

    assert_eq!(manager.queue().len(), 1);
    assert_eq!(manager.queue()[0].objective, "second");
    assert!(matches!(manager.queue()[0].status, GoalStatus::Queued));

    let reloaded = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    assert_eq!(reloaded.active().unwrap().objective, "first");
    assert_eq!(reloaded.queue().len(), 1);
    assert_eq!(reloaded.queue()[0].objective, "second");
    assert!(matches!(reloaded.queue()[0].status, GoalStatus::Queued));
}

#[tokio::test]
async fn queue_next_starts_immediately_when_no_goal_is_active() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();

    manager.queue_next(Goal::new("first queued")).await.unwrap();

    assert_eq!(manager.active().unwrap().objective, "first queued");
    assert!(manager.queue().is_empty());
}

#[tokio::test]
async fn completing_goal_starts_next_queued_goal() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();

    manager.start(Goal::new("first")).await.unwrap();
    manager.queue_next(Goal::new("second")).await.unwrap();
    manager
        .update_status(GoalStatus::Complete, None)
        .await
        .unwrap();

    assert_eq!(manager.active().unwrap().objective, "second");
    assert!(manager.queue().is_empty());
}

#[test]
fn skill_context_is_injected_before_user_message() {
    let mut context = AgentContext::new();
    context.set_skill_context(AgentMessage::system_text("skill body".to_owned()));

    let skill_context = context.take_skill_context();
    assert!(skill_context.is_some());
    context.append_message(skill_context.unwrap());
    context.append_message(AgentMessage::user_text("user prompt".to_owned()));

    let messages: Vec<_> = context.messages().iter().collect();
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], AgentMessage::System { .. }));
    assert!(matches!(messages[1], AgentMessage::User { .. }));
}
