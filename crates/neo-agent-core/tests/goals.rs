use std::sync::Arc;

use neo_agent_core::{
    AgentContext, AgentMessage,
    goal::{Goal, GoalManager, GoalStatus, load_goal_store},
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
async fn start_rejects_an_active_goal_without_replacing_durable_state() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();

    manager.start(Goal::new("first")).await.unwrap();
    let first_id = manager.active().expect("first active goal").id;
    let second = Goal::new("second");
    let second_id = second.id.clone();

    let error = manager
        .start(second)
        .await
        .expect_err("an active goal must require explicit replacement");

    assert!(error.to_string().contains("active goal"));
    assert_eq!(
        manager.active().expect("first goal remains active").id,
        first_id
    );
    assert!(temp.path().join("agents/main/goals/active.json").is_file());
    assert!(
        !temp
            .path()
            .join("agents/main/goals/runs")
            .join(second_id)
            .exists()
    );

    let reloaded = load_goal_store(temp.path()).await.unwrap();
    assert_eq!(reloaded.active().expect("first goal restored").id, first_id);
}

#[tokio::test]
async fn concurrent_starts_install_exactly_one_goal() {
    let temp = tempfile::tempdir().unwrap();
    let manager = Arc::new(GoalManager::load(temp.path().to_path_buf()).await.unwrap());
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let first = Goal::new("first");
    let first_id = first.id.clone();
    let second = Goal::new("second");
    let second_id = second.id.clone();

    let spawn_start = |goal: Goal| {
        let manager = Arc::clone(&manager);
        let barrier = Arc::clone(&barrier);
        tokio::spawn(async move {
            barrier.wait().await;
            let id = goal.id.clone();
            (id, manager.start(goal).await)
        })
    };
    let first_start = spawn_start(first);
    let second_start = spawn_start(second);
    barrier.wait().await;

    let (first_result, second_result) = tokio::join!(first_start, second_start);
    let results = [first_result.unwrap(), second_result.unwrap()];
    let winner = results
        .iter()
        .find_map(|(id, result)| result.is_ok().then_some(id))
        .expect("one start succeeds");
    let loser = if winner == &first_id {
        &second_id
    } else {
        &first_id
    };

    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    assert!(results.iter().any(|(_, result)| {
        result
            .as_ref()
            .is_err_and(|error| error.to_string().contains("active goal"))
    }));
    assert_eq!(manager.active().expect("winner remains active").id, *winner);
    assert!(
        !temp
            .path()
            .join("agents/main/goals/runs")
            .join(loser)
            .exists()
    );

    let reloaded = load_goal_store(temp.path()).await.unwrap();
    assert_eq!(reloaded.active().expect("winner restored").id, *winner);
}

#[tokio::test]
async fn independently_loaded_managers_share_one_serialized_store() {
    let temp = tempfile::tempdir().unwrap();
    let first_manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    let second_manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let first = tokio::spawn({
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            first_manager.start(Goal::new("first")).await
        }
    });
    let second = tokio::spawn({
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            second_manager.start(Goal::new("second")).await
        }
    });
    barrier.wait().await;

    let (first, second) = tokio::join!(first, second);
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results.iter().any(|result| {
        result
            .as_ref()
            .is_err_and(|error| error.to_string().contains("active goal"))
    }));

    let store = load_goal_store(temp.path()).await.unwrap();
    assert!(store.active().is_some());
    assert!(store.queue().is_empty());
}

#[tokio::test]
async fn failed_start_save_does_not_install_active_goal() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    let goal = Goal::new("cannot persist");
    let goal_path = temp.path().join("agents/main/goals").join("active.json");
    std::fs::create_dir_all(&goal_path).unwrap();

    manager
        .start(goal)
        .await
        .expect_err("a goal whose JSON cannot be written must fail");

    assert!(manager.active().is_none());
}

#[tokio::test]
async fn failed_start_never_deletes_caller_supplied_artifact_directory() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let marker = outside.path().join("keep.txt");
    std::fs::write(&marker, "keep").unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    let mut goal = Goal::new("cannot persist");
    goal.artifact_dir = Some(outside.path().to_path_buf());
    let active_path = temp.path().join("agents/main/goals/active.json");
    std::fs::create_dir_all(&active_path).unwrap();

    manager
        .start(goal)
        .await
        .expect_err("an unwritable goal store must fail");

    assert_eq!(std::fs::read_to_string(marker).unwrap(), "keep");
}

#[tokio::test]
async fn replacement_cannot_overwrite_existing_goal_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    manager.start(Goal::new("first")).await.unwrap();
    let active = manager.active().expect("active goal");
    let marker = active
        .artifact_dir
        .as_ref()
        .expect("artifact directory")
        .join("GOAL.md");
    let original = std::fs::read_to_string(&marker).unwrap();
    let mut replacement = Goal::new("replacement");
    replacement.id = active.id.clone();

    manager
        .replace(replacement)
        .await
        .expect_err("an existing goal id must not reuse its artifact directory");

    assert_eq!(std::fs::read_to_string(marker).unwrap(), original);
    assert_eq!(
        manager.active().expect("original remains active").id,
        active.id
    );
}

#[tokio::test]
async fn failed_replace_save_preserves_previous_goal() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    manager.start(Goal::new("first")).await.unwrap();
    let first = manager.active().expect("first active goal");
    let active_path = temp.path().join("agents/main/goals/active.json");
    let first_store = std::fs::read(&active_path).unwrap();
    std::fs::remove_file(&active_path).unwrap();
    let replacement = Goal::new("cannot persist");
    std::fs::create_dir(&active_path).unwrap();

    manager
        .replace(replacement)
        .await
        .expect_err("an unwritable replacement must fail");

    assert_eq!(
        manager.active().expect("first goal remains active").id,
        first.id
    );
    std::fs::remove_dir(&active_path).unwrap();
    std::fs::write(&active_path, first_store).unwrap();
    let reloaded = load_goal_store(temp.path()).await.unwrap();
    assert_eq!(reloaded.active().expect("first goal restored").id, first.id);
}

#[tokio::test]
async fn goal_persists_to_disk() {
    let temp = tempfile::tempdir().unwrap();
    let goal = Goal::new("persist me").with_completion_criterion("tests pass");
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    manager.start(goal).await.unwrap();

    let store = load_goal_store(temp.path()).await.unwrap();
    let active = store.active().unwrap();
    assert_eq!(active.objective, "persist me");
    assert_eq!(active.completion_criterion, Some("tests pass".into()));
    assert!(
        temp.path()
            .join("agents/main/goals")
            .join("active.json")
            .is_file()
    );
    assert!(!temp.path().join("goals").join("active.json").exists());
}

#[tokio::test]
async fn goal_store_uses_one_authoritative_active_file() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    manager.start(Goal::new("first")).await.unwrap();
    manager.queue_next(Goal::new("second")).await.unwrap();

    let goals_dir = temp.path().join("agents/main/goals");
    let json_files = std::fs::read_dir(&goals_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    assert_eq!(json_files, vec![goals_dir.join("active.json")]);

    let reloaded = load_goal_store(temp.path()).await.unwrap();
    assert_eq!(reloaded.active().unwrap().objective, "first");
    assert_eq!(reloaded.queue().len(), 1);
    assert_eq!(reloaded.queue()[0].objective, "second");
}

#[tokio::test]
async fn stale_legacy_goal_json_is_never_loaded() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    manager.start(Goal::new("authoritative")).await.unwrap();
    let authoritative_id = manager.active().unwrap().id;
    let stale = Goal::new("stale legacy");
    let stale_path = temp
        .path()
        .join("agents/main/goals")
        .join(format!("{}.json", stale.id));
    std::fs::write(&stale_path, serde_json::to_vec_pretty(&stale).unwrap()).unwrap();

    let reloaded = load_goal_store(temp.path()).await.unwrap();
    assert_eq!(reloaded.active().unwrap().id, authoritative_id);
    assert!(reloaded.queue().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn goal_store_rejects_symlinked_active_json() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let goal = Goal::new("do not follow links");
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    let goal_path = temp.path().join("agents/main/goals").join("active.json");
    let outside_goal = outside.path().join("goal.json");
    std::fs::create_dir_all(goal_path.parent().expect("goal parent")).unwrap();
    std::fs::write(&outside_goal, "outside").unwrap();
    std::os::unix::fs::symlink(&outside_goal, &goal_path).unwrap();

    let error = manager
        .start(goal)
        .await
        .expect_err("goal save should reject symlinked target");

    assert!(
        error.to_string().contains("symlink"),
        "error should name symlink risk: {error}"
    );
    assert_eq!(std::fs::read_to_string(&outside_goal).unwrap(), "outside");
}

#[cfg(unix)]
#[tokio::test]
async fn goal_store_load_rejects_symlinked_active_json() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let goals_dir = temp.path().join("agents/main/goals");
    let active_path = goals_dir.join("active.json");
    let outside_store = outside.path().join("goal.json");
    std::fs::create_dir_all(&goals_dir).unwrap();
    std::fs::write(&outside_store, r#"{"active":null,"queue":[]}"#).unwrap();
    std::os::unix::fs::symlink(&outside_store, &active_path).unwrap();

    let error = GoalManager::load(temp.path().to_path_buf())
        .await
        .expect_err("goal load should reject a symlinked authority file");

    assert!(error.to_string().contains("symlink"), "{error:#}");
}

#[cfg(unix)]
#[tokio::test]
async fn goal_store_load_rejects_symlinked_artifact_directory() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    manager.start(Goal::new("authoritative")).await.unwrap();
    let artifact_dir = manager
        .active()
        .expect("active goal")
        .artifact_dir
        .expect("artifact directory");
    std::fs::remove_dir_all(&artifact_dir).unwrap();
    std::os::unix::fs::symlink(outside.path(), &artifact_dir).unwrap();

    let error = load_goal_store(temp.path())
        .await
        .expect_err("goal load should reject a symlinked artifact directory");

    assert!(error.to_string().contains("symlink"), "{error:#}");
}

#[tokio::test]
async fn goal_store_supports_repeated_atomic_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();
    manager.start(Goal::new("first")).await.unwrap();
    manager.pause().await.unwrap();

    let reloaded = load_goal_store(temp.path()).await.unwrap();
    assert!(matches!(
        reloaded.active().expect("active goal").status,
        GoalStatus::Paused
    ));
}

#[tokio::test]
async fn goal_start_creates_supergoal_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(temp.path().to_path_buf()).await.unwrap();

    manager.start(Goal::new("ship goal mode")).await.unwrap();

    let active = manager.active().unwrap();
    let artifact_dir = active.artifact_dir.as_ref().expect("artifact dir");
    assert!(artifact_dir.ends_with(&active.id));
    assert!(
        artifact_dir.starts_with(
            temp.path()
                .canonicalize()
                .unwrap()
                .join("agents/main/goals/runs")
        )
    );
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
