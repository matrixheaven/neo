use super::*;
use crate::ToolAccess;
use crate::ToolContext;
use serde_json::json;
use tokio::sync::mpsc;

fn make_ctx() -> ToolContext {
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all())
}

#[tokio::test]
async fn ask_user_receives_response() {
    let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let origin = crate::workflow::WorkflowExecutionOrigin {
        run_id: crate::workflow::WorkflowId("workflow-run".into()),
        human_handle: None,
        definition_name: "workflow".into(),
        definition_revision: None,
        phase_id: Some("phase".into()),
        invocation_id: Some("invocation".into()),
        swarm_item_id: None,
    };
    let ctx = make_ctx().with_workflow_origin(Some(origin.clone()));

    let input = json!({
        "questions": [{
            "question": "Which framework?",
            "header": "Framework",
            "options": [
                { "label": "React", "description": "UI library" },
                { "label": "Vue", "description": "Progressive framework" }
            ],
            "multi_select": false
        }]
    });

    // Spawn a responder that answers the first question.
    tokio::spawn(async move {
        let pending = rx.recv().await.expect("should receive question");
        assert_eq!(pending.questions.len(), 1);
        assert_eq!(pending.questions[0].question, "Which framework?");
        assert_eq!(pending.questions[0].options.len(), 2);
        assert_eq!(pending.workflow_origin.as_ref(), Some(&origin));
        let _ = pending.response_tx.send(QuestionResponse {
            answers: vec!["React".to_owned()],
        });
    });

    let result = tool.execute(&ctx, input).await.expect("execute");
    assert!(!result.is_error);
    assert_eq!(result.content, "React");
    let details = result.details.expect("details");
    assert_eq!(details["question_id"].as_str().unwrap().len(), 36); // UUID
}

#[tokio::test]
async fn ask_user_multiple_questions() {
    let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let input = json!({
        "questions": [
            {
                "question": "Dark or light?",
                "options": [{ "label": "Dark" }, { "label": "Light" }],
                "multi_select": false
            },
            {
                "question": "Tabs or spaces?",
                "options": [{ "label": "Tabs" }, { "label": "Spaces" }],
                "multi_select": false
            }
        ]
    });

    tokio::spawn(async move {
        let pending = rx.recv().await.expect("should receive");
        assert_eq!(pending.questions.len(), 2);
        let _ = pending.response_tx.send(QuestionResponse {
            answers: vec!["Dark".to_owned(), "Spaces".to_owned()],
        });
    });

    let result = tool.execute(&ctx, input).await.expect("execute");
    assert!(!result.is_error);
    assert!(result.content.contains("1. Dark"));
    assert!(result.content.contains("2. Spaces"));
}

#[tokio::test]
async fn ask_user_channel_closed_returns_error() {
    let (tx, rx) = mpsc::unbounded_channel::<PendingQuestion>();
    drop(rx); // Close the receiver.
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let input = json!({
        "questions": [{
            "question": "Test?",
            "options": [{ "label": "A" }, { "label": "B" }],
            "multi_select": false
        }]
    });

    let result = tool.execute(&ctx, input).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ask_user_response_dropped_returns_cancelled() {
    let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let input = json!({
        "questions": [{
            "question": "Test?",
            "options": [{ "label": "A" }, { "label": "B" }],
            "multi_select": false
        }]
    });

    // Drop the response sender without answering.
    tokio::spawn(async move {
        let pending = rx.recv().await.expect("should receive");
        drop(pending.response_tx);
    });

    let result = tool.execute(&ctx, input).await.expect("execute");
    assert!(result.is_error);
    assert!(result.content.contains("cancelled"));
}

#[tokio::test]
async fn ask_user_invalid_input() {
    let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let result = tool.execute(&ctx, json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ask_user_rejects_too_many_questions() {
    let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let questions: Vec<_> = (0..5)
        .map(|i| {
            json!({
                "question": format!("Question {i}?"),
                "options": [{"label": "A"}, {"label": "B"}]
            })
        })
        .collect();
    let result = tool.execute(&ctx, json!({"questions": questions})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ask_user_rejects_too_few_options() {
    let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let result = tool
        .execute(
            &ctx,
            json!({
                "questions": [{
                    "question": "Only one option?",
                    "options": [{"label": "A"}]
                }]
            }),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ask_user_rejects_empty_option_label() {
    let (tx, _rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let result = tool
        .execute(
            &ctx,
            json!({
                "questions": [{
                    "question": "Bad option?",
                    "options": [{"label": ""}, {"label": "B"}]
                }]
            }),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ask_user_background_returns_task_without_waiting() {
    let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let result = tool
        .execute(
            &ctx,
            json!({
                "background": true,
                "questions": [{
                    "question": "Where should config live?",
                    "header": "Config",
                    "options": [{ "label": "Project" }, { "label": "User" }],
                    "multi_select": false
                }]
            }),
        )
        .await
        .expect("background question should start");

    assert!(!result.is_error);
    let details = result.details.expect("details");
    let task_id = details["task_id"].as_str().expect("task id");
    assert!(task_id.starts_with("question-"));
    assert_eq!(details["kind"], "question");
    assert_eq!(details["status"], "waiting_for_user");
    assert_eq!(details["automatic_notification"], true);

    let pending = rx.try_recv().expect("question should be visible to host");
    assert_eq!(pending.id, task_id);
    assert_eq!(pending.questions[0].question, "Where should config live?");
}

#[tokio::test]
async fn ask_user_background_answer_is_visible_through_task_output() {
    let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let result = tool
        .execute(
            &ctx,
            json!({
                "background": true,
                "questions": [{
                    "question": "Where should config live?",
                    "options": [{ "label": "Project" }, { "label": "User" }],
                    "multi_select": false
                }]
            }),
        )
        .await
        .expect("background question should start");
    let task_id = result.details.as_ref().unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let pending = rx.recv().await.expect("pending question");
    pending
        .response_tx
        .send(QuestionResponse {
            answers: vec!["Project".to_owned()],
        })
        .expect("send response");
    for _ in 0..20 {
        let output = ctx
            .background_tasks
            .output(
                &task_id,
                false,
                std::time::Duration::from_secs(0),
                ctx.max_output_bytes,
            )
            .await
            .expect("TaskOutput result");
        if output.details.as_ref().unwrap()["status"] == "completed" {
            let details = output.details.unwrap();
            assert_eq!(details["kind"], "question");
            assert_eq!(details["answers"], json!(["Project"]));
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("background question should complete");
}

#[tokio::test]
async fn ask_user_background_stopped_question_ignores_late_answer() {
    let (tx, mut rx) = mpsc::unbounded_channel::<PendingQuestion>();
    let tool = AskUserTool::new(tx);
    let ctx = make_ctx();

    let result = tool
        .execute(
            &ctx,
            json!({
                "background": true,
                "questions": [{
                    "question": "Continue?",
                    "options": [{ "label": "Yes" }, { "label": "No" }],
                    "multi_select": false
                }]
            }),
        )
        .await
        .expect("background question should start");
    let task_id = result.details.as_ref().unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let pending = rx.recv().await.expect("pending question");

    ctx.background_tasks
        .stop(&task_id, "no longer needed", ctx.max_output_bytes)
        .await
        .expect("TaskStop should stop question");
    pending
        .response_tx
        .send(QuestionResponse {
            answers: vec!["Yes".to_owned()],
        })
        .expect("late response can still be sent");
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let output = ctx
        .background_tasks
        .output(
            &task_id,
            false,
            std::time::Duration::from_secs(0),
            ctx.max_output_bytes,
        )
        .await
        .expect("TaskOutput result");
    let details = output.details.unwrap();
    assert_eq!(details["status"], "cancelled");
    assert!(details.get("answers").is_none());
}
