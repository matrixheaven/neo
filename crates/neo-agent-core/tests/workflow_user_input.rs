//! Durable AwaitingUser and typed answer control (Task 14 / design §29).

use std::sync::Arc;
use std::time::Duration;

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchHandle;
use neo_agent_core::tools::{BackgroundTaskManager, ProcessSupervisor, ToolContext, ToolRegistry};
use neo_agent_core::workflow::journal::{JournalPayload, collect_journal_v2, run_dir};
use neo_agent_core::workflow::{
    CompiledSchema, FinalResultBody, LuaWorkflowRunner, UserAnswerPolicy, WorkflowActor,
    WorkflowErrorCode, WorkflowHandle, WorkflowLaunchRequest, WorkflowLimits, WorkflowRuntime,
    WorkflowState, request_id_for_call_index,
};
use neo_agent_core::{AgentContext, ToolAccess};
use serde_json::json;

fn answer_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" }
        },
        "required": ["ok"],
        "additionalProperties": false
    })
}

#[test]
fn task_answer_schema_accepts_scalar_array_and_object_answers() {
    let schema = ToolRegistry::with_builtin_tools()
        .specs()
        .into_iter()
        .find(|spec| spec.name == "TaskAnswer")
        .expect("TaskAnswer spec")
        .input_schema;
    let schema = CompiledSchema::compile(&schema).expect("compile TaskAnswer schema");

    for answer in [json!(true), json!(["approve", 1]), json!({ "ok": true })] {
        schema
            .validate_instance(&json!({
                "task_id": "workflow-1",
                "request_id": "req-1",
                "answer": answer,
            }))
            .expect("TaskAnswer schema must accept every JSON answer shape");
    }
}

fn await_user_script(policy: &str) -> String {
    format!(
        r#"
local answer = neo.await_user({{
  prompt = "Continue with deploy?",
  answer_schema = {{
    type = "object",
    properties = {{ ok = {{ type = "boolean" }} }},
    required = {{ "ok" }},
    additionalProperties = false
  }},
  default = {{ ok = true }},
  title = "Deploy",
  answer_policy = "{policy}"
}})
return answer
"#
    )
}

fn limits_one_worker() -> WorkflowLimits {
    WorkflowLimits {
        max_active_workers: 1,
        max_active_vms: 1,
        max_active_executors: 4,
        global_storage_bytes: 256 * 1024 * 1024,
        ..WorkflowLimits::default()
    }
}

fn launch(script: String) -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: "await-user".to_owned(),
        description: "durable user input".to_owned(),
        phases: Vec::new(),
        script,
        args: json!({}),
        launch_source: "/workflow".to_owned(),
        parent_run_id: None,
        output_schema: None,
        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    }
}

async fn wait_state(handle: &WorkflowHandle, want: WorkflowState) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle.snapshot().await.state == want {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for state {}", want.as_str()));
}

struct LiveFixture {
    dir: tempfile::TempDir,
    runtime: WorkflowRuntime,
    handle: WorkflowHandle,
}

async fn live_await_user_fixture(script: String) -> LiveFixture {
    let dir = tempfile::tempdir().unwrap();
    let limits = limits_one_worker();
    let harness = FakeHarness::from_turns([]);
    let config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path().to_path_buf())
        .expect("workspace root")
        .with_permission_mode(neo_agent_core::PermissionMode::Yolo);
    let model_client = harness.client();
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let process_supervisor = ProcessSupervisor::default();
    let context = AgentContext::new();
    let runtime = WorkflowRuntime::new(limits.clone());
    runtime
        .bind_runner({
            let config = config.clone();
            let model_client = Arc::clone(&model_client);
            let registry = Arc::clone(&registry);
            let process_supervisor = process_supervisor.clone();
            let context = context.clone();
            let limits = limits.clone();
            move |handle, metadata, _session| {
                let dispatch = WorkflowDispatchHandle {
                    config: config.clone(),
                    model_client: Arc::clone(&model_client),
                    registry: Arc::clone(&registry),
                    process_supervisor: process_supervisor.clone(),
                    context: context.clone(),
                };
                let limits = limits.clone();
                async move {
                    let runner = LuaWorkflowRunner::new(dispatch, handle, limits);
                    runner.execute(&metadata.script, metadata.args).await?;
                    Ok(())
                }
            }
        })
        .expect("bind runner");

    let handle = runtime
        .create_run(dir.path(), launch(script))
        .await
        .expect("create");
    runtime
        .start_worker(&handle.run_id)
        .await
        .expect("start worker");
    wait_state(&handle, WorkflowState::AwaitingUser).await;
    LiveFixture {
        dir,
        runtime,
        handle,
    }
}

/// Full worker path: await_user releases VM/worker permits and survives restart
/// with prompt/schema/default/policy intact. Ordinary resume cannot bypass.
#[tokio::test]
async fn await_user_releases_permits_and_survives_restart() {
    let fixture = live_await_user_fixture(await_user_script("human")).await;
    let handle = &fixture.handle;
    let runtime = &fixture.runtime;
    let dir = fixture.dir.path();

    assert_eq!(
        runtime.admission().occupancy().active_workers,
        0,
        "await_user must release worker permits"
    );
    assert_eq!(
        runtime.admission().occupancy().active_vms,
        0,
        "await_user must release VM permits"
    );

    let pending = handle
        .pending_user_input()
        .await
        .expect("pending")
        .expect("open request");
    assert_eq!(pending.prompt, "Continue with deploy?");
    assert_eq!(pending.title.as_deref(), Some("Deploy"));
    assert_eq!(pending.answer_policy, UserAnswerPolicy::Human);
    assert_eq!(pending.default, Some(json!({ "ok": true })));
    assert_eq!(pending.answer_schema, answer_schema());
    assert!(pending.answer.is_none());
    assert_eq!(pending.request_id, request_id_for_call_index(0));

    let journal_path = run_dir(dir, &handle.run_id).join("journal.jsonl");
    let envelopes = collect_journal_v2(&journal_path, Some(&handle.run_id)).unwrap();
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::UserInputRequested { request_id, .. }
                if request_id == &pending.request_id
        )),
        "UserInputRequested must be durable"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::StateChanged {
                new: WorkflowState::AwaitingUser,
                ..
            }
        )),
        "AwaitingUser transition must be durable"
    );

    // Restart: rehydrate preserves AwaitingUser and request fields; no worker.
    let runtime2 = WorkflowRuntime::new(limits_one_worker());
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    runtime2
        .bind_runner({
            let starts = Arc::clone(&starts);
            move |_h, _m, _s| {
                let starts = Arc::clone(&starts);
                async move {
                    starts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    panic!("rehydrate must not auto-start worker");
                }
            }
        })
        .expect("bind");
    let handles = runtime2.rehydrate(dir).await.expect("rehydrate");
    assert_eq!(handles.len(), 1);
    assert_eq!(
        handles[0].snapshot().await.state,
        WorkflowState::AwaitingUser
    );
    assert_eq!(starts.load(std::sync::atomic::Ordering::Acquire), 0);
    assert_eq!(runtime2.admission().occupancy().active_workers, 0);

    let rehydrated = handles[0]
        .pending_user_input()
        .await
        .expect("pending")
        .expect("open after restart");
    assert_eq!(rehydrated.prompt, "Continue with deploy?");
    assert_eq!(rehydrated.title.as_deref(), Some("Deploy"));
    assert_eq!(rehydrated.answer_policy, UserAnswerPolicy::Human);
    assert_eq!(rehydrated.default, Some(json!({ "ok": true })));
    assert_eq!(rehydrated.answer_schema, answer_schema());
    assert_eq!(rehydrated.request_id, pending.request_id);

    // Stop while awaiting is allowed and must not erase request history.
    handles[0]
        .stop(WorkflowActor::Human)
        .await
        .expect("stop while awaiting");
    assert_eq!(handles[0].snapshot().await.state, WorkflowState::Cancelled);
    let after_stop = collect_journal_v2(&journal_path, Some(&handle.run_id)).unwrap();
    assert!(
        after_stop
            .iter()
            .any(|e| matches!(&e.payload, JournalPayload::UserInputRequested { .. })),
        "stop must not lose request history"
    );
}

/// Answer validates schema/policy before queueing; bad answers leave state
/// unchanged; a valid human answer queues the run.
#[tokio::test]
async fn answer_validates_request_schema_before_queueing() {
    let fixture = live_await_user_fixture(await_user_script("human")).await;
    let handle = &fixture.handle;

    let request_id = handle
        .pending_user_input()
        .await
        .unwrap()
        .unwrap()
        .request_id;

    let bad_schema = handle
        .answer(&request_id, json!({ "ok": "nope" }), WorkflowActor::Human)
        .await
        .expect_err("schema mismatch");
    assert_eq!(bad_schema.code(), WorkflowErrorCode::InvalidUserAnswer);
    assert_eq!(handle.snapshot().await.state, WorkflowState::AwaitingUser);

    let model_err = handle
        .answer(&request_id, json!({ "ok": true }), WorkflowActor::Model)
        .await
        .expect_err("human-only");
    assert_eq!(model_err.code(), WorkflowErrorCode::InvalidUserAnswer);
    assert_eq!(handle.snapshot().await.state, WorkflowState::AwaitingUser);

    let stale = handle
        .answer("req_missing", json!({ "ok": true }), WorkflowActor::Human)
        .await
        .expect_err("stale");
    assert_eq!(stale.code(), WorkflowErrorCode::StaleUserRequest);
    assert_eq!(handle.snapshot().await.state, WorkflowState::AwaitingUser);

    handle
        .answer(&request_id, json!({ "ok": true }), WorkflowActor::Human)
        .await
        .expect("valid answer");

    handle
        .answer(&request_id, json!({ "ok": true }), WorkflowActor::Human)
        .await
        .expect("idempotent identical answer");

    let conflict = handle
        .answer(&request_id, json!({ "ok": false }), WorkflowActor::Human)
        .await
        .expect_err("conflicting duplicate");
    assert_eq!(conflict.code(), WorkflowErrorCode::StaleUserRequest);

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = handle.snapshot().await.state;
            if state == WorkflowState::Completed || state.is_terminal() {
                return state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("run finishes after answer");

    let output = handle.output().await.expect("output");
    assert_eq!(output.state, WorkflowState::Completed);
    let final_value = match output.final_result.expect("final").body {
        FinalResultBody::Inline { value } => value,
        other => panic!("expected inline final, got {other:?}"),
    };
    assert_eq!(final_value, json!({ "ok": true }));

    let journal_path = run_dir(fixture.dir.path(), &handle.run_id).join("journal.jsonl");
    let envelopes = collect_journal_v2(&journal_path, Some(&handle.run_id)).unwrap();
    assert!(envelopes.iter().any(|e| matches!(
        &e.payload,
        JournalPayload::UserInputAnswered {
            request_id: rid,
            answer: Some(ans),
        } if rid == &request_id && ans == &json!({ "ok": true })
    )));
}

#[tokio::test]
async fn task_answer_adapter_uses_runtime_model_policy() {
    let registry = ToolRegistry::with_builtin_tools();

    let denied_fixture = live_await_user_fixture(await_user_script("human")).await;
    let denied_task_id = denied_fixture.handle.run_id.0.clone();
    let denied_request_id = denied_fixture
        .handle
        .pending_user_input()
        .await
        .expect("pending request")
        .expect("open request")
        .request_id;
    let denied_tasks = BackgroundTaskManager::new();
    denied_tasks
        .start_workflow(
            denied_task_id.clone(),
            "await test".to_owned(),
            denied_fixture.handle.clone(),
        )
        .await
        .expect("register");
    let denied_context = ToolContext::new(denied_fixture.dir.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_background_tasks(denied_tasks);
    let denied = registry
        .run(
            "TaskAnswer",
            &denied_context,
            json!({
                "task_id": denied_task_id,
                "request_id": denied_request_id,
                "answer": { "ok": true },
            }),
        )
        .await
        .expect_err("human-only request must reject model TaskAnswer");
    assert!(
        denied
            .to_string()
            .contains("answer_policy human rejects actor Model")
    );
    assert_eq!(
        denied_fixture.handle.snapshot().await.state,
        WorkflowState::AwaitingUser
    );
    let denied_output = registry
        .run(
            "TaskOutput",
            &denied_context,
            json!({ "task_id": denied_task_id }),
        )
        .await
        .expect("human-only TaskOutput");
    assert!(
        denied_output
            .content
            .contains("next_action: wait_for_human")
    );
    assert!(!denied_output.content.contains("wait_for_human("));

    let allowed_fixture = live_await_user_fixture(await_user_script("human_or_model")).await;
    let allowed_task_id = allowed_fixture.handle.run_id.0.clone();
    let allowed_request_id = allowed_fixture
        .handle
        .pending_user_input()
        .await
        .expect("pending request")
        .expect("open request")
        .request_id;
    let allowed_tasks = BackgroundTaskManager::new();
    allowed_tasks
        .start_workflow(
            allowed_task_id.clone(),
            "await test".to_owned(),
            allowed_fixture.handle.clone(),
        )
        .await
        .expect("register");
    let allowed_context = ToolContext::new(allowed_fixture.dir.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_background_tasks(allowed_tasks);
    let accepted = registry
        .run(
            "TaskAnswer",
            &allowed_context,
            json!({
                "task_id": allowed_task_id,
                "request_id": allowed_request_id,
                "answer": { "ok": true },
            }),
        )
        .await
        .expect("model-allowed request should accept TaskAnswer");
    assert!(!accepted.is_error, "{}", accepted.content);
    wait_state(&allowed_fixture.handle, WorkflowState::Completed).await;
}

#[tokio::test]
async fn task_output_exposes_actionable_pending_request_without_journal_view() {
    let fixture = live_await_user_fixture(await_user_script("human_or_model")).await;
    let task_id = fixture.handle.run_id.0.clone();
    let tasks = BackgroundTaskManager::new();
    tasks
        .start_workflow(
            task_id.clone(),
            "await test".to_owned(),
            fixture.handle.clone(),
        )
        .await
        .expect("register");
    let context = ToolContext::new(fixture.dir.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_background_tasks(tasks);
    let registry = ToolRegistry::with_builtin_tools();

    let output = registry
        .run("TaskOutput", &context, json!({ "task_id": task_id }))
        .await
        .expect("default TaskOutput");
    assert!(!output.is_error, "{}", output.content);
    assert!(output.content.contains("view: summary"));
    assert!(output.content.contains("pending_request_id: req_c0"));
    assert!(output.content.contains("answer_policy: human_or_model"));
    assert!(output.content.contains("answer_schema:"));
    assert!(output.content.contains("default_answer: {\"ok\":true}"));
    assert!(output.content.contains("next_action: TaskAnswer("));

    let details = output.details.expect("TaskOutput details");
    let pending = details
        .get("pending_user")
        .expect("top-level pending request");
    let request_id = pending
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .expect("request_id");
    assert_eq!(pending.get("answer_schema"), Some(&answer_schema()));
    assert_eq!(pending.get("default"), Some(&json!({ "ok": true })));
    assert_eq!(
        pending
            .get("next_action")
            .and_then(serde_json::Value::as_str),
        Some("TaskAnswer")
    );

    let result_view = registry
        .run(
            "TaskOutput",
            &context,
            json!({ "task_id": fixture.handle.run_id.0, "view": "result" }),
        )
        .await
        .expect("result TaskOutput");
    assert!(result_view.content.contains("view: result"));
    assert!(result_view.content.contains("pending_request_id: req_c0"));
    assert!(result_view.content.contains("next_action: TaskAnswer("));

    let answered = registry
        .run(
            "TaskAnswer",
            &context,
            json!({
                "task_id": fixture.handle.run_id.0,
                "request_id": request_id,
                "answer": { "ok": true },
            }),
        )
        .await
        .expect("TaskAnswer built only from TaskOutput");
    assert!(!answered.is_error, "{}", answered.content);
    wait_state(&fixture.handle, WorkflowState::Completed).await;
}

/// TaskResume / ordinary resume cannot bypass a missing answer.
#[tokio::test]
async fn task_resume_cannot_bypass_missing_answer() {
    let fixture = live_await_user_fixture(await_user_script("human")).await;
    let handle = &fixture.handle;
    let runtime = &fixture.runtime;

    let err = handle
        .resume(WorkflowActor::Human)
        .await
        .expect_err("resume must not bypass awaiting_user");
    assert_eq!(err.code(), WorkflowErrorCode::AwaitingUser);
    assert_eq!(handle.snapshot().await.state, WorkflowState::AwaitingUser);

    let manager = BackgroundTaskManager::new();
    let task_id = handle.run_id.0.clone();
    manager
        .start_workflow(task_id.clone(), "await test".to_owned(), handle.clone())
        .await
        .expect("register");
    let resume_err = manager
        .resume_workflow(&task_id, WorkflowActor::Human)
        .await
        .expect_err("TaskResume must fail while awaiting_user");
    let msg = resume_err.to_string();
    assert!(
        msg.contains("awaiting_user") || msg.contains("resume failed"),
        "unexpected resume error: {msg}"
    );
    assert_eq!(handle.snapshot().await.state, WorkflowState::AwaitingUser);
    assert_eq!(
        runtime.admission().occupancy().active_workers,
        0,
        "failed resume must not re-admit worker"
    );

    let request_id = handle
        .pending_user_input()
        .await
        .unwrap()
        .unwrap()
        .request_id;
    let answered = manager
        .answer_workflow(
            &task_id,
            &request_id,
            json!({ "ok": true }),
            WorkflowActor::Human,
        )
        .await
        .expect("answer tool");
    assert!(!answered.is_error, "{}", answered.content);

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle.snapshot().await.state == WorkflowState::Completed {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("completed after TaskAnswer");
}
