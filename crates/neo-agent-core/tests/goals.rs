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
