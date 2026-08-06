use super::*;

#[tokio::test]
async fn manager_lists_active_and_completed_questions() {
    let manager = BackgroundTaskManager::new();
    manager
        .start_question("question-test".to_owned(), "Pick one".to_owned())
        .await;

    let active = manager.list(true, 10).await;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].kind, BackgroundTaskKind::Question);
    assert_eq!(active[0].status, BackgroundTaskStatus::WaitingForUser);

    manager
        .complete_question("question-test", vec!["Project config".to_owned()])
        .await;

    assert!(manager.list(true, 10).await.is_empty());
    let all = manager.list(false, 10).await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].status, BackgroundTaskStatus::Completed);
    assert_eq!(all[0].answers, Some(vec!["Project config".to_owned()]));
}

#[tokio::test]
async fn manager_stops_question_and_ignores_late_answer() {
    let manager = BackgroundTaskManager::new();
    manager
        .start_question("question-stop".to_owned(), "Pick one".to_owned())
        .await;

    let stopped = manager
        .stop("question-stop", "Cancelled by test", 1024)
        .await
        .expect("question should stop");
    assert_eq!(stopped.details.as_ref().unwrap()["status"], "cancelled");

    manager
        .complete_question("question-stop", vec!["Too late".to_owned()])
        .await;

    let output = manager
        .output("question-stop", false, Duration::from_millis(1), 1024)
        .await
        .expect("stopped question should be readable");
    let details = output.details.expect("details");
    assert_eq!(details["status"], "cancelled");
    assert!(details.get("answers").is_none());
}
