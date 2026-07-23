use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{StreamExt, stream};
use neo_agent_core::harness::{FakeHarness, fake_model};
use neo_agent_core::session::{
    JsonlSessionReader, JsonlSessionWriter, SessionEventPersistence,
    workflow_notification_projection_ids,
};
use neo_agent_core::workflow::{
    WorkflowActor, WorkflowLaunchRequest, WorkflowLimits, WorkflowPhase, WorkflowRuntime,
    WorkflowState,
};
use neo_agent_core::{
    ActiveTurnInput, AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime,
    BackgroundTaskManager, MessageOrigin, SteerInputHandle, WorkflowNotification,
};
use neo_ai::{
    AiError, AiStreamEvent, ChatMessage, ChatRequest, ContentPart, ModelClient, StopReason,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn launch_request() -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: "notification test".to_owned(),
        description: "prove natural-turn workflow notification delivery".to_owned(),
        phases: vec![WorkflowPhase {
            id: "done".to_owned(),
            description: "finish".to_owned(),
        }],
        script: "return true".to_owned(),
        args: serde_json::json!({}),
        launch_source: "test".to_owned(),
        parent_run_id: None,
    }
}

async fn wait_for_terminal(handle: &neo_agent_core::workflow::WorkflowHandle) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if handle.snapshot().await.state.is_terminal() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("workflow terminal state");
}

struct CompletingWorkflowClient {
    requests: Mutex<Vec<ChatRequest>>,
    finish: Arc<Notify>,
    workflow_runtime: WorkflowRuntime,
    session_dir: PathBuf,
}

impl CompletingWorkflowClient {
    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl ModelClient for CompletingWorkflowClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let request_index = {
            let mut requests = self.requests.lock().expect("request lock");
            let index = requests.len();
            requests.push(request);
            index
        };
        let id = format!("turn-{request_index}");
        let end = AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        };
        if request_index == 0 {
            self.finish.notify_one();
            let workflow_runtime = self.workflow_runtime.clone();
            let session_dir = self.session_dir.clone();
            return stream::once(async move {
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if !workflow_runtime
                            .notification_queue()
                            .pending_for_session(&session_dir)
                            .is_empty()
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("workflow notification queued during first request");
                Ok(AiStreamEvent::MessageStart { id })
            })
            .chain(stream::iter([Ok(end)]))
            .boxed();
        }
        stream::iter([Ok(AiStreamEvent::MessageStart { id }), Ok(end)]).boxed()
    }
}

fn request_contains(request: &ChatRequest, expected: &str) -> bool {
    request.messages.iter().any(|message| {
        let content = match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content,
        };
        content
            .iter()
            .any(|part| matches!(part, ContentPart::Text { text } if text.contains(expected)))
    })
}

#[tokio::test]
async fn terminal_workflow_notification_waits_for_natural_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path().join("session_terminal");
    let workflow_runtime = WorkflowRuntime::new(WorkflowLimits::default());
    workflow_runtime
        .bind_runner(|_, _, _| async { Ok(()) })
        .expect("bind runner");
    let handle = workflow_runtime
        .create_run(&session_dir, launch_request())
        .await
        .expect("create workflow");
    workflow_runtime
        .start_worker(&handle.run_id)
        .await
        .expect("start worker");
    wait_for_terminal(&handle).await;

    let notifications = workflow_runtime
        .notification_queue()
        .pending_for_session(&session_dir);
    assert_eq!(notifications.len(), 1);
    let notification = &notifications[0];
    let notification_id = notification.id.clone();
    assert_eq!(notification.state, WorkflowState::Completed);
    assert_eq!(notification.run_id, handle.run_id);
    assert_eq!(
        notification.id,
        WorkflowNotification::new(
            &session_dir,
            handle.run_id.clone(),
            WorkflowState::Completed,
            notification.reason.clone(),
        )
        .id,
        "notification identity must be stable across processes"
    );

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "injected-turn".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "natural-turn".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    assert!(
        harness.requests().is_empty(),
        "notification must not start a model turn"
    );

    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_session_directory(session_dir.clone())
            .with_workflow_runtime(workflow_runtime.clone()),
        harness.client(),
    );
    let mut context = AgentContext::new();
    let injected_events = runtime
        .run_turn(
            &mut context,
            AgentMessage::injection_text("background answer", "background_question"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("injected turn");
    assert!(injected_events.iter().all(|event| {
        !matches!(event, AgentEvent::MessageAppended { message }
            if WorkflowNotification::projection_id(message).is_some())
    }));
    assert_eq!(harness.requests().len(), 1);

    let events = runtime
        .run_turn_with_cancel(
            &mut context,
            AgentMessage::user_text("natural user prompt"),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("natural turn");
    assert_eq!(harness.requests().len(), 2);

    let projected = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended { message }
                if WorkflowNotification::projection_id(message).is_some() =>
            {
                Some(message)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(projected.len(), 1);
    assert!(matches!(
        projected[0],
        AgentMessage::User {
            display_text: None,
            origin: MessageOrigin::Injection { .. },
            ..
        }
    ));
    assert!(projected[0].text().contains("TaskOutput"));
    assert!(
        !workflow_runtime
            .notification_queue()
            .pending_for_session(&session_dir)
            .is_empty(),
        "building the turn must not consume an uncommitted projection"
    );

    let wire_path = session_dir.join("agents/main/wire.jsonl");
    tokio::fs::create_dir_all(wire_path.parent().expect("wire parent"))
        .await
        .expect("wire directory");
    let mut writer = JsonlSessionWriter::create(&wire_path)
        .await
        .expect("session writer");
    let mut persistence = SessionEventPersistence::default();
    for event in &events {
        for persisted in persistence.persisted_events(event) {
            writer
                .append_event(&persisted)
                .await
                .expect("persist event");
        }
    }
    writer.flush().await.expect("durable projection");
    let replayed = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("replay session");
    let projection_ids = workflow_notification_projection_ids(&replayed);
    assert_eq!(projection_ids, vec![notification_id]);

    let restarted = WorkflowRuntime::new(WorkflowLimits::default());
    restarted
        .notification_queue()
        .restore_projected(projection_ids);
    restarted
        .rehydrate(&session_dir)
        .await
        .expect("rehydrate terminal workflow");
    assert!(
        restarted
            .notification_queue()
            .pending_for_session(&session_dir)
            .is_empty(),
        "persisted projection must deduplicate restart recovery"
    );
}

#[tokio::test]
async fn same_process_rehydrate_preserves_live_run_and_control() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path().join("session_live");
    let workflow_runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let original = workflow_runtime
        .create_run(&session_dir, launch_request())
        .await
        .expect("create live workflow");

    let recovered = workflow_runtime
        .rehydrate(&session_dir)
        .await
        .expect("idempotent rehydrate")
        .pop()
        .expect("existing workflow handle");
    assert_eq!(recovered.snapshot().await.state, WorkflowState::Running);
    assert_eq!(original.snapshot().await.state, WorkflowState::Running);

    recovered
        .stop(WorkflowActor::Human)
        .await
        .expect("stop through recovered handle");
    assert!(
        original.is_stop_requested(),
        "rehydrate must preserve the original live control"
    );
    assert_eq!(original.snapshot().await.state, WorkflowState::Cancelled);
}

#[tokio::test]
async fn active_turn_completion_notifies_queued_follow_up() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path().join("session_follow_up");
    let finish = Arc::new(Notify::new());
    let workflow_runtime = WorkflowRuntime::new(WorkflowLimits::default());
    workflow_runtime
        .bind_runner({
            let finish = Arc::clone(&finish);
            move |_, _, _| {
                let finish = Arc::clone(&finish);
                async move {
                    finish.notified().await;
                    Ok(())
                }
            }
        })
        .expect("bind runner");
    let handle = workflow_runtime
        .create_run(&session_dir, launch_request())
        .await
        .expect("create workflow");
    workflow_runtime
        .start_worker(&handle.run_id)
        .await
        .expect("start worker");

    let client = Arc::new(CompletingWorkflowClient {
        requests: Mutex::new(Vec::new()),
        finish,
        workflow_runtime: workflow_runtime.clone(),
        session_dir: session_dir.clone(),
    });
    let steer_input = SteerInputHandle::new();
    steer_input.push(ActiveTurnInput::FollowUp(AgentMessage::user_text(
        "queued follow up",
    )));
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(fake_model())
            .with_session_directory(session_dir.clone())
            .with_workflow_runtime(workflow_runtime.clone()),
        client.clone(),
    )
    .with_steer_input(steer_input);
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("initial prompt"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("active turn with follow-up");
    wait_for_terminal(&handle).await;

    let requests = client.requests();
    assert_eq!(requests.len(), 2, "follow-up starts exactly one model turn");
    assert!(!request_contains(&requests[0], "TaskOutput"));
    assert!(request_contains(&requests[1], "TaskOutput"));
    let projected = events
        .iter()
        .filter(|event| {
            matches!(event, AgentEvent::MessageAppended { message }
                if WorkflowNotification::projection_id(message).is_some())
        })
        .count();
    assert_eq!(projected, 1, "follow-up receives one typed reminder");
}

#[tokio::test]
async fn host_exit_recovery_does_not_start_model_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path().join("session_recovery");
    let creator = WorkflowRuntime::new(WorkflowLimits::default());
    let created = creator
        .create_run(&session_dir, launch_request())
        .await
        .expect("create running workflow");

    let recovered = WorkflowRuntime::new(WorkflowLimits::default());
    let tasks = BackgroundTaskManager::new();
    for _ in 0..2 {
        for handle in recovered
            .rehydrate(&session_dir)
            .await
            .expect("rehydrate workflow")
        {
            let task_id = handle.run_id.0.clone();
            if tasks.workflow_handle(&task_id).await.is_none() {
                let description = handle.snapshot().await.name;
                tasks
                    .start_workflow(task_id, description, handle)
                    .await
                    .expect("register workflow handle");
            }
        }
    }

    let handle = tasks
        .workflow_handle(&created.run_id.0)
        .await
        .expect("recovered workflow handle");
    let snapshot = handle.snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::Paused);
    assert_eq!(snapshot.terminal_reason.as_deref(), Some("host_exit"));
    assert_eq!(tasks.list(false, 10).await.len(), 1);
    assert_eq!(
        recovered
            .notification_queue()
            .pending_for_session(&session_dir)
            .len(),
        1,
        "duplicate recovery must not duplicate queued notification"
    );

    let harness = FakeHarness::from_turns([Vec::new()]);
    assert!(
        harness.requests().is_empty(),
        "rehydration must not execute Lua or open a model turn"
    );
}
