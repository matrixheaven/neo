use std::time::Duration;

use futures::StreamExt;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchResolver;
use neo_agent_core::workflow::WorkflowRuntime;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode, ToolRegistry,
};
use neo_ai::{AiStreamEvent, ChatMessage, ContentPart, StopReason};
use serde_json::{Value, json};

const SCRIPT: &str = r#"
neo.phase("run")
local verified = neo.verify(false, "verification failed")
local unknown = neo.tool({ name = "MissingTool", input = {} })
local forbidden = neo.tool({ name = "Workflow", input = {} })
    return {
        verified = {
            status = verified.status,
            verified = verified.details.verified,
            message = verified.details.message,
        },
    unknown = {
        status = unknown.status,
        code = unknown.details.code,
    },
    forbidden = {
        status = forbidden.status,
        code = forbidden.details.code,
    },
}
"#;

fn output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verified", "unknown", "forbidden"],
        "properties": {
            "verified": {
                "type": "object",
                "additionalProperties": false,
                "required": ["status", "verified", "message"],
                "properties": {
                    "status": {"type": "string"},
                    "verified": {"type": "boolean"},
                    "message": {"type": "string"},
                },
            },
            "unknown": {
                "type": "object",
                "additionalProperties": false,
                "required": ["status", "code"],
                "properties": {
                    "status": {"type": "string"},
                    "code": {"type": "string"},
                },
            },
            "forbidden": {
                "type": "object",
                "additionalProperties": false,
                "required": ["status", "code"],
                "properties": {
                    "status": {"type": "string"},
                    "code": {"type": "string"},
                },
            },
        },
    })
}

fn inline_workflow_input() -> Value {
    json!({
        "action": "run_inline",
        "name": "recoverable-outcomes",
        "description": "Return ordinary host failures as data",
        "phases": [{"id": "run", "description": "Run the checks"}],
        "script": SCRIPT,
        "input_schema": {
            "type": "object",
            "additionalProperties": false
        },
        "output_schema": output_schema(),
    })
}

fn tool_call_turn(id: &str, name: &str, arguments: Value, message_id: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: message_id.to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: id.to_owned(),
            name: name.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: id.to_owned(),
            raw_arguments: arguments.to_string(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
            usage: None,
        },
    ]
}

fn done_turn(id: &str, text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart { id: id.to_owned() },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]
}

async fn run_turn(
    runtime: &AgentRuntime,
    context: &mut AgentContext,
    message: AgentMessage,
) -> Vec<AgentEvent> {
    runtime
        .run_turn(context, message)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("agent turn succeeds")
}

fn tool_result(events: &[AgentEvent], name: &str) -> neo_agent_core::ToolResult {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished {
                name: event_name,
                result,
                ..
            } if event_name == name => Some(result.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {name} result"))
}

fn finished_call(events: &[AgentEvent], name: &str) -> neo_agent_core::AgentToolCall {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCallFinished { tool_call, .. } if tool_call.name.as_ref() == name => {
                Some(tool_call.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {name} call"))
}

fn tool_result_text(request: &neo_ai::ChatRequest, call_id: &str) -> String {
    request
        .messages
        .iter()
        .find_map(|message| match message {
            ChatMessage::ToolResult {
                tool_call_id,
                content,
                ..
            } if tool_call_id == call_id => Some(
                content
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing tool result {call_id} in model request"))
}

#[tokio::test]
async fn production_workflow_result_reaches_the_next_model_request() {
    let workspace = tempfile::tempdir().expect("workspace");
    let session = tempfile::tempdir().expect("session");
    let resolver = WorkflowDispatchResolver::default();
    let workflow_runtime = WorkflowRuntime::default();
    let first_harness = FakeHarness::from_turns([
        tool_call_turn(
            "workflow-call",
            "Workflow",
            inline_workflow_input(),
            "message-1",
        ),
        done_turn("message-2", "workflow started"),
    ]);
    let config = AgentConfig::for_model(first_harness.model())
        .with_permission_mode(PermissionMode::Yolo)
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_session_directory(session.path())
        .with_agent_id("main")
        .with_workflow_runtime(workflow_runtime)
        .with_workflow_dispatch_resolver(resolver.clone());
    assert!(
        config
            .workflow_dispatch_resolver
            .shares_state_with(&resolver)
    );

    let first_runtime = AgentRuntime::with_tools(
        config.clone(),
        first_harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let first_events = run_turn(
        &first_runtime,
        &mut context,
        AgentMessage::user_text("run the workflow"),
    )
    .await;

    let launch = tool_result(&first_events, "Workflow");
    assert!(!launch.is_error, "{}", launch.content);
    let launch_content: Value = serde_json::from_str(&launch.content).expect("launch JSON");
    let task_id = launch_content["task"]["task_id"]
        .as_str()
        .expect("workflow task id")
        .to_owned();
    assert_eq!(launch_content["status"], "started");
    assert_eq!(launch_content["next_actions"][0]["tool"], "TaskOutput");
    assert_eq!(
        launch_content["next_actions"][0]["arguments"]["task_id"],
        task_id
    );

    let handle = config
        .background_tasks
        .workflow_handle(&task_id)
        .await
        .expect("workflow task handle");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = handle.snapshot().await;
            if snapshot.state.is_terminal() {
                assert_eq!(
                    snapshot.state,
                    neo_agent_core::workflow::WorkflowState::Completed,
                    "workflow snapshot: {snapshot:?}"
                );
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("workflow completes");

    let second_harness = FakeHarness::from_turns([
        tool_call_turn(
            "task-output-call",
            "TaskOutput",
            json!({"task_id": task_id, "view": "result", "block": true}),
            "message-3",
        ),
        done_turn("message-4", "result received"),
    ]);
    let second_runtime = AgentRuntime::with_tools(
        config,
        second_harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let second_events = run_turn(
        &second_runtime,
        &mut context,
        AgentMessage::user_text("read the workflow result"),
    )
    .await;

    let task_call = finished_call(&second_events, "TaskOutput");
    let task_arguments: Value =
        serde_json::from_str(task_call.raw_arguments.as_ref()).expect("TaskOutput arguments JSON");
    assert_eq!(
        task_arguments,
        json!({"task_id": task_id, "view": "result", "block": true})
    );

    let task_result = tool_result(&second_events, "TaskOutput");
    assert!(!task_result.is_error, "{}", task_result.content);
    let task_content: Value = serde_json::from_str(&task_result.content).expect("TaskOutput JSON");
    assert_eq!(task_content["status"], "completed");
    let final_value = &task_content["result"]["body"]["inline"]["value"];
    assert_eq!(final_value["verified"]["status"], "completed");
    assert_eq!(final_value["verified"]["verified"], false);
    assert_eq!(final_value["verified"]["message"], "verification failed");
    assert_eq!(final_value["unknown"]["status"], "failed");
    assert_eq!(final_value["unknown"]["code"], "unknown_tool");
    assert_eq!(final_value["forbidden"]["status"], "failed");
    assert_eq!(
        final_value["forbidden"]["code"],
        "tool_not_workflow_eligible"
    );

    let requests = second_harness.requests();
    assert_eq!(
        requests.len(),
        2,
        "TaskOutput must be followed by the next model request"
    );
    let replayed_launch: Value =
        serde_json::from_str(&tool_result_text(&requests[0], "workflow-call"))
            .expect("launch JSON remains in the next turn context");
    assert_eq!(replayed_launch, launch_content);
    let next_model_content = tool_result_text(&requests[1], "task-output-call");
    let next_model_json: Value = serde_json::from_str(&next_model_content)
        .expect("next model request contains TaskOutput JSON");
    assert_eq!(next_model_json, task_content);
    assert_eq!(
        next_model_json["result"]["body"]["inline"]["value"],
        *final_value
    );
}

const VERIFY_SCRIPT: &str = r#"
neo.phase("run")
local verified = neo.verify(false, "evidence incomplete")
return {
    verified = {
        status = verified.status,
        verified = verified.details.verified,
        message = verified.details.message,
    },
}
"#;

fn verify_workflow_input() -> Value {
    json!({
        "action": "run_inline",
        "name": "verify-as-data",
        "description": "Return false verification as completed data",
        "phases": [{"id": "run", "description": "Run the verification"}],
        "script": VERIFY_SCRIPT,
        "input_schema": {
            "type": "object",
            "additionalProperties": false
        },
        "output_schema": {
            "type": "object",
            "additionalProperties": false,
            "required": ["verified"],
            "properties": {
                "verified": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["status", "verified", "message"],
                    "properties": {
                        "status": {"type": "string"},
                        "verified": {"type": "boolean"},
                        "message": {"type": "string"},
                    },
                },
            },
        },
    })
}

/// `neo.verify(false, ...)` is completed business data: the workflow stays
/// `completed`, the final value exposes host execution `status` separately from
/// `details.verified = false`, and no abort or repair turn occurs.
#[tokio::test]
async fn workflow_result_exposes_status_and_business_data() {
    let workspace = tempfile::tempdir().expect("workspace");
    let session = tempfile::tempdir().expect("session");
    let resolver = WorkflowDispatchResolver::default();
    let workflow_runtime = WorkflowRuntime::default();
    let harness = FakeHarness::from_turns([
        tool_call_turn(
            "workflow-call",
            "Workflow",
            verify_workflow_input(),
            "message-1",
        ),
        done_turn("message-2", "workflow started"),
    ]);
    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Yolo)
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_session_directory(session.path())
        .with_agent_id("main")
        .with_workflow_runtime(workflow_runtime)
        .with_workflow_dispatch_resolver(resolver.clone());
    let runtime = AgentRuntime::with_tools(
        config.clone(),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = run_turn(
        &runtime,
        &mut context,
        AgentMessage::user_text("run the workflow"),
    )
    .await;

    let launch = tool_result(&events, "Workflow");
    assert!(!launch.is_error, "{}", launch.content);
    let launch_content: Value = serde_json::from_str(&launch.content).expect("launch JSON");
    let task_id = launch_content["task"]["task_id"]
        .as_str()
        .expect("workflow task id")
        .to_owned();

    // The workflow must complete; verify(false, ...) is data, not an abort.
    let handle = config
        .background_tasks
        .workflow_handle(&task_id)
        .await
        .expect("workflow task handle");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = handle.snapshot().await;
            if snapshot.state.is_terminal() {
                assert_eq!(
                    snapshot.state,
                    neo_agent_core::workflow::WorkflowState::Completed,
                    "workflow snapshot: {snapshot:?}"
                );
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("workflow completes");

    // The persisted final value exposes execution status and business data.
    let second_harness = FakeHarness::from_turns([
        tool_call_turn(
            "task-output-call",
            "TaskOutput",
            json!({"task_id": task_id, "view": "result", "block": true}),
            "message-3",
        ),
        done_turn("message-4", "result received"),
    ]);
    let second_runtime = AgentRuntime::with_tools(
        config,
        second_harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let second_events = run_turn(
        &second_runtime,
        &mut context,
        AgentMessage::user_text("read the workflow result"),
    )
    .await;

    let task_result = tool_result(&second_events, "TaskOutput");
    assert!(!task_result.is_error, "{}", task_result.content);
    let task_content: Value = serde_json::from_str(&task_result.content).expect("TaskOutput JSON");
    assert_eq!(task_content["status"], "completed");
    let final_value = &task_content["result"]["body"]["inline"]["value"];
    assert_eq!(final_value["verified"]["status"], "completed");
    assert_eq!(final_value["verified"]["verified"], false);
    assert_eq!(final_value["verified"]["message"], "evidence incomplete");
}
