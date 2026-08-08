//! Interactive test fixtures: session and workspace scaffolding (moved from `mod.rs`).

use super::super::*;

use neo_agent_core::{AgentEvent, AgentMessage, Content, StopReason};
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::fixtures_config::*;

pub const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000601";
pub const SESSION_B: &str = "session_00000000-0000-4000-8000-000000000602";
pub const SESSION_CHILD: &str = "session_00000000-0000-4000-8000-000000000603";
pub const SESSION_NEW: &str = "session_00000000-0000-4000-8000-000000000604";

pub fn test_workspace_root() -> PathBuf {
    let dir = std::env::temp_dir().join("neo-test-workspace");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn main_wire_path_for_session(session_dir: impl AsRef<Path>) -> PathBuf {
    let path = neo_agent_core::session::main_agent_wire_path(session_dir.as_ref());
    fs::create_dir_all(path.parent().expect("wire parent")).expect("create wire dir");
    path
}

pub fn write_main_wire(bucket_dir: &Path, session_id: &str, content: &str) {
    let path = main_wire_path_for_session(bucket_dir.join(session_id));
    fs::write(path, content).expect("write main wire");
}

pub fn test_session_summary(
    id: impl Into<String>,
    title: impl Into<String>,
    work_dir: impl Into<PathBuf>,
    last_prompt: impl Into<String>,
) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        title: Some(title.into()),
        last_prompt: Some(last_prompt.into()),
        work_dir: work_dir.into(),
        updated_at: String::new(),
        metadata: None,
    }
}

pub fn interleaved_replay_tool_calls() -> Vec<neo_agent_core::AgentToolCall> {
    vec![
        neo_agent_core::AgentToolCall {
            id: "first-tool".into(),
            name: "Read".into(),
            raw_arguments: r#"{"path":"first-order.txt"}"#.into(),
        },
        neo_agent_core::AgentToolCall {
            id: "failed-delegate".into(),
            name: "Delegate".into(),
            raw_arguments: r#"{"task":"failed delegate marker"}"#.into(),
        },
        neo_agent_core::AgentToolCall {
            id: "later-tool".into(),
            name: "Bash".into(),
            raw_arguments: r#"{"command":"later-order-command"}"#.into(),
        },
    ]
}

pub fn interleaved_replay_prelude_events() -> Vec<AgentEvent> {
    let runtime = neo_agent_core::multi_agent::MultiAgentRuntime::new();
    let running = runtime.start_foreground_delegate_for_test("restored delegate card");
    let delegate_id = running.id.clone();
    let completed = runtime.complete_delegate_for_test(&delegate_id, "done");

    vec![
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("resume-user"),
        },
        AgentEvent::MessageStarted {
            phase: neo_ai::MessagePhase::Unknown,
            turn: 1,
            id: "assistant-one".to_owned(),
        },
        AgentEvent::ThinkingStarted {
            turn: 1,
            id: "thinking-one".to_owned(),
            kind: neo_ai::ThinkingKind::Unknown,
        },
        AgentEvent::ThinkingDelta {
            turn: 1,
            text: "resume-thinking".to_owned(),
        },
        AgentEvent::ThinkingFinished {
            turn: 1,
            signature: None,
            redacted: false,
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "resume-output".to_owned(),
        },
        AgentEvent::MessageFinished {
            phase: neo_ai::MessagePhase::Unknown,
            turn: 1,
            id: "assistant-one".to_owned(),
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::DelegateStarted {
            turn: 1,
            agent: running,
            workflow_origin: None,
        },
        AgentEvent::DelegateFinished {
            turn: 1,
            agent: completed,
            workflow_origin: None,
        },
    ]
}

pub fn interleaved_replay_execution_events() -> Vec<AgentEvent> {
    vec![
        AgentEvent::ToolExecutionStarted {
            turn: 2,
            id: "first-tool".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "first-order.txt" }),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::ToolExecutionFinished {
            turn: 2,
            id: "first-tool".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("first result"),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::ToolExecutionStarted {
            turn: 2,
            id: "failed-delegate".to_owned(),
            name: "Delegate".to_owned(),
            arguments: serde_json::json!({ "task": "failed delegate marker" }),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::ToolExecutionFinished {
            turn: 2,
            id: "failed-delegate".to_owned(),
            name: "Delegate".to_owned(),
            result: neo_agent_core::ToolResult::error("failed delegate marker"),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::ToolExecutionStarted {
            turn: 2,
            id: "later-tool".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({ "command": "later-order-command" }),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::ToolExecutionFinished {
            turn: 2,
            id: "later-tool".to_owned(),
            name: "Bash".to_owned(),
            result: neo_agent_core::ToolResult::ok("later result"),
            workflow_origin: None,
            output_ref: None,
        },
    ]
}

pub fn interleaved_replay_message_events(
    tool_calls: Vec<neo_agent_core::AgentToolCall>,
) -> Vec<AgentEvent> {
    let assistant_message = AgentMessage::assistant(
        [
            Content::thinking("resume-thinking", None, false),
            Content::text("resume-output"),
            Content::text("resume-summary"),
        ],
        tool_calls,
        StopReason::ToolUse,
    );

    vec![
        AgentEvent::TextDelta {
            turn: 2,
            text: "resume-summary".to_owned(),
        },
        AgentEvent::TurnFinished {
            turn: 2,
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::MessageAppended {
            message: assistant_message,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "first-tool",
                "Read",
                [Content::text("first result")],
                false,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "failed-delegate",
                "Delegate",
                [Content::text("failed delegate marker")],
                true,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "later-tool",
                "Bash",
                [Content::text("later result")],
                false,
            ),
        },
    ]
}

pub async fn write_interleaved_replay_session(config: &AppConfig) {
    let bucket_dir = workspace_sessions_dir(config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    let mut events = interleaved_replay_prelude_events();
    events.extend(interleaved_replay_execution_events());
    events.extend(interleaved_replay_message_events(
        interleaved_replay_tool_calls(),
    ));
    for event in &events {
        writer.append(event).await.expect("append replay event");
    }
    writer.flush().await.expect("flush session");
}

pub fn add_indexed_session_fixture(
    sessions_dir: &Path,
    project: &Path,
    session_id: &str,
    prompt: &str,
    timestamp: &str,
) -> AppConfig {
    fs::create_dir_all(project).expect("create project");
    let config = test_config(project, sessions_dir.to_path_buf());
    let bucket = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket).expect("create session bucket");
    write_main_wire(
        &bucket,
        session_id,
        r#"{"MessageAppended":{"message":{"User":{"content":[{"Text":{"text":"hello"}}]}}}}"#,
    );
    SessionMetadataStore::new(&bucket)
        .record_activity(
            session_id,
            Some(project.display().to_string()),
            Some(prompt.to_owned()),
            timestamp.to_owned(),
        )
        .expect("record session metadata");
    neo_agent_core::session::SessionIndex::new(sessions_dir.parent().expect("neo home"))
        .append(&neo_agent_core::session::SessionIndexEntry {
            session_id: session_id.to_owned(),
            session_dir: bucket,
            workdir: project.to_path_buf(),
        })
        .expect("index session");
    config
}
