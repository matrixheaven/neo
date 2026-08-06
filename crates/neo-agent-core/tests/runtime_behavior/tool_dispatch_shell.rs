use super::fake_harness::end_turn_events;
use super::fake_harness::tool_call_turn;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode,
    ShellCommandOrigin, ShellCommandOutcome, ToolExecutionMode, ToolRegistry,
    harness::{FakeHarness, fake_model},
    session::JsonlSessionWriter,
};
use neo_ai::{AiError, AiStreamEvent, ChatRequest, MessagePhase, ModelClient};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn runtime_emits_shell_lifecycle_for_bash_tool() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "printf shell-ok" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonicalize");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(&workspace_root)
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("shell tool should succeed");

    assert!(events.contains(&AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "tool_1".to_owned(),
        command: "printf shell-ok".to_owned(),
        cwd: workspace_root,
        origin: ShellCommandOrigin::ModelBashTool,
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            turn: 1,
            id,
            exit_code: Some(0),
            stdout,
            stderr,
            ..
        } if id == "tool_1" && stdout.contains("shell-ok") && stderr.is_empty()
    )));
}

#[tokio::test]
async fn runtime_clamps_out_of_range_bash_timeout_and_returns_notice() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            raw_arguments: json!({
                "command": "printf shell-ran",
                "timeout_secs": 299
            })
            .to_string(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonicalize");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(&workspace_root)
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bounded shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should finish after running the tool");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            result,
            ..
        } if id == "tool_1"
            && !result.is_error
            && result.content.starts_with("shell-ran\n")
            && result.content.contains("timeout_secs must be between 300 and 3600")
            && result.content.contains("clamped to 300 seconds")
    )));
}

#[tokio::test]
async fn runtime_marks_model_background_bash_as_backgrounded_shell_event() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            raw_arguments: json!({
                "command": "sleep 5",
                "run_in_background": true,
                "description": "sleep in background"
            })
            .to_string(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("run background shell"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should finish with background task");

    let task_id = events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionFinished { result, .. } => result
            .details
            .as_ref()
            .and_then(|details| details["task_id"].as_str())
            .map(str::to_owned),
        _ => None,
    });
    let task_id = task_id.expect("background task id");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            id,
            exit_code: None,
            outcome: ShellCommandOutcome::Backgrounded { task_id: event_task_id },
            ..
        } if id == "tool_1" && event_task_id.as_ref() == task_id.as_str()
    )));
    let _ = runtime
        .config()
        .background_tasks
        .stop(&task_id, "test cleanup", 1024)
        .await;
}

#[tokio::test]
async fn runtime_events_and_session_jsonl_do_not_leak_capped_bash_output() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "command": "printf keep; printf '%s%s%s%s' runtime -bash -leak -tail",
                    "max_output_bytes": 4
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run capped shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("shell tool should succeed");
    let event_json = persist_events_to_jsonl_and_read_back(&events).await;

    assert!(!event_json.contains("runtime-bash-leak-tail"));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            stdout,
            truncated: true,
            ..
        } if stdout.len() <= 4
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            result,
            ..
        } if result
            .details
            .as_ref()
            .and_then(|details| details["stdout"].as_str())
            .is_some_and(|stdout| stdout.len() <= 4)
    )));
}

#[tokio::test]
async fn runtime_emits_terminal_lifecycle_events_for_terminal_tool() {
    let model = Arc::new(TerminalLifecycleModel::default());
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model())
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("open terminal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal tool turn should succeed");

    let handle = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::TerminalSessionStarted {
                handle,
                command,
                cols,
                rows,
                cwd,
                ..
            } if command == "bash --noprofile --norc"
                && *cols == 44
                && *rows == 9
                && cwd == &workspace_root =>
            {
                Some(handle.clone())
            }
            _ => None,
        })
        .expect("terminal start event should expose handle and PTY metadata");
    assert!(!handle.is_empty());

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TerminalSessionOutput {
            handle: event_handle,
            output,
            ..
        } if event_handle == &handle && output.contains("terminal-event-ok")
    )));

    let finished = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::TerminalSessionFinished {
                handle: event_handle,
                status,
                ..
            } if event_handle == &handle && status == "cancelled"
        )
    });
    assert!(
        finished,
        "terminal stop should emit a provider-neutral finished event"
    );
}

#[tokio::test]
async fn runtime_streams_terminal_prompt_updates_before_read() {
    let prompt = "Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?";
    let model = Arc::new(TerminalStreamingModel::default());
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model()).with_permission_mode(PermissionMode::Yolo),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("open terminal prompt"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionUpdate {
                name,
                partial_result,
                ..
            } if name == "Terminal" && partial_result.content.contains(prompt)
        )),
        "expected a Terminal streaming update carrying the prompt before read returned; events: {events:?}"
    );
}

#[tokio::test]
async fn runtime_events_and_session_jsonl_do_not_leak_capped_terminal_output() {
    let model = Arc::new(CappedTerminalOutputModel::default());
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model())
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("read capped terminal"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal tool turn should succeed");
    let event_json = persist_events_to_jsonl_and_read_back(&events).await;

    assert!(!event_json.contains("terminal-runtime-leak-tail"));
    assert!(
        events.iter().any(|event| matches!(
        event,
        AgentEvent::TerminalSessionOutput {
            output,
            truncated: true,
            ..
        } if output.len() <= 4
        )),
        "events should include capped terminal output: {event_json}"
    );
    assert!(
        events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            name,
            result,
            ..
        } if name == "Terminal"
            && result
                .details
                .as_ref()
                .and_then(|details| details["output"].as_str())
                .is_some_and(|output| output.len() <= 4)
        )),
        "events should include capped terminal ToolExecutionFinished: {event_json}"
    );
}

async fn persist_events_to_jsonl_and_read_back(events: &[AgentEvent]) -> String {
    let temp = tempfile::tempdir().expect("session dir");
    let path = temp.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session writer");
    for event in events {
        writer
            .append_event(event)
            .await
            .expect("append session event");
    }
    writer.flush().await.expect("flush session writer");
    std::fs::read_to_string(path).expect("read session jsonl")
}

#[derive(Default)]
struct TerminalLifecycleModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for TerminalLifecycleModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let next = terminal_lifecycle_events_for_request(&request);
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

#[derive(Default)]
struct CappedTerminalOutputModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for CappedTerminalOutputModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let next = capped_terminal_output_events_for_request(&request);
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

#[derive(Default)]
struct TerminalStreamingModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for TerminalStreamingModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request.clone());
        let next = terminal_streaming_events_for_request(&request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

fn terminal_streaming_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    const PROMPT: &str = "Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?";
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    let handle = tool_results
        .iter()
        .find_map(|content| terminal_handle(content));
    let last = tool_results.last().map(String::as_str).unwrap_or_default();

    match handle {
        None => terminal_tool_turn(
            turn_index,
            "tool_1",
            json!({
                "mode": "start",
                "command": format!(
                    "python3 - <<'PY'\nimport sys, time\nsys.stdout.write('{PROMPT} ')\nsys.stdout.flush()\ntime.sleep(1)\nPY"
                ),
                "cols": 100,
                "rows": 24
            }),
        ),
        Some(handle) if !last.contains("status: cancelled") && !last.contains("output:") => {
            terminal_tool_turn(
                turn_index,
                "tool_2",
                json!({
                    "mode": "read",
                    "handle": handle,
                    "max_output_bytes": 1024
                }),
            )
        }
        _ => end_turn_done(turn_index),
    }
}

fn capped_terminal_output_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    let handle = tool_results
        .iter()
        .find_map(|content| terminal_handle(content));
    let last = tool_results.last().map(String::as_str).unwrap_or_default();
    match handle {
        None => terminal_tool_turn(
            turn_index,
            "tool_start",
            json!({
                "mode": "start",
                "command": "printf term; printf '%s%s%s%s' inal -runtime -leak -tail; sleep 1",
                "max_output_bytes": 4
            }),
        ),
        Some(_) if last.contains("status: cancelled") => vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: format!("msg_{turn_index}"),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        Some(handle) if last.contains("truncated: true") => terminal_tool_turn(
            turn_index,
            "tool_stop",
            json!({
                "mode": "stop",
                "handle": handle,
                "max_output_bytes": 4
            }),
        ),
        Some(handle) if !last.contains("status: cancelled") => terminal_tool_turn(
            turn_index,
            "tool_read",
            json!({
                "mode": "read",
                "handle": handle,
                "max_output_bytes": 4
            }),
        ),
        _ => end_turn_done(turn_index),
    }
}

fn terminal_lifecycle_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    match tool_results
        .last()
        .and_then(|content| terminal_handle(content))
    {
        None => terminal_tool_turn(
            turn_index,
            "tool_start",
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 44,
                "rows": 9
            }),
        ),
        Some(_)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("status: cancelled")) =>
        {
            end_turn_done(turn_index)
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("terminal-event-ok")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_stop",
                json!({
                    "mode": "stop",
                    "handle": handle
                }),
            )
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("written: true")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_read",
                json!({
                    "mode": "read",
                    "handle": handle,
                    "max_output_bytes": 4096
                }),
            )
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("status: running")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_write",
                json!({
                    "mode": "write",
                    "handle": handle,
                    "input": [{"text": "printf terminal-event-ok\\n\n"}]
                }),
            )
        }
        _ => end_turn_done(turn_index),
    }
}

fn end_turn_done(turn_index: usize) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: format!("msg_{turn_index}"),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

#[allow(clippy::needless_pass_by_value)]
fn terminal_tool_turn(
    turn_index: usize,
    tool_id: &str,
    arguments: serde_json::Value,
) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: format!("msg_{turn_index}"),
        },
        AiStreamEvent::ToolCallStart {
            id: tool_id.to_owned(),
            name: "Terminal".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: tool_id.to_owned(),
            raw_arguments: arguments.to_string(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]
}

fn tool_result_text(message: &neo_ai::ChatMessage) -> Option<String> {
    match message {
        neo_ai::ChatMessage::ToolResult { content, .. } => Some(
            content
                .iter()
                .filter_map(|part| match part {
                    neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

fn terminal_handle(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("handle: "))
        .map(str::trim)
        .filter(|handle| !handle.is_empty())
        .map(ToOwned::to_owned)
}

#[tokio::test]
async fn background_bash_finished_event_carries_output_reference() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "tool_bg",
            "Bash",
            json!({
                "command": "printf background-captured",
                "run_in_background": true,
                "description": "background capture probe"
            }),
        )]),
        end_turn_events("done"),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_session_directory(workspace.path().join("session")),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run background bash"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let (finished_ref, result) = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished {
                id,
                output_ref,
                result,
                ..
            } if id == "tool_bg" => Some((output_ref, result)),
            _ => None,
        })
        .expect("backgrounded bash finished event");
    let reference = finished_ref
        .as_ref()
        .expect("backgrounded bash Finished must carry the typed reference");
    assert_eq!(reference.agent_id, neo_agent_core::session::MAIN_AGENT_ID);
    // The reference is keyed by the same task id the model-visible result
    // reports, but the reference itself never enters the result payload.
    let model_task_id = result
        .details
        .as_ref()
        .and_then(|details| details.get("task_id"))
        .and_then(serde_json::Value::as_str)
        .expect("model-visible background task id");
    assert_eq!(reference.task_id, model_task_id);
    assert!(
        !serde_json::to_string(&result)
            .expect("serialize result")
            .contains("output_ref"),
        "the reference must not enter ToolResult"
    );
    // The artifact was opened before the start result returned.
    let log = workspace
        .path()
        .join("session")
        .join("agents")
        .join(&reference.agent_id)
        .join("tasks")
        .join(format!("{}.log", reference.task_id));
    assert!(log.exists(), "{}", log.display());
}
