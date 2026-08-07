use super::instructions::instruction_epoch;
use neo_agent_core::{
    AgentContext, AgentEvent, AgentMessage, CompactionSummary, ContextWindowSource,
    instructions::InstructionFailureKind,
    multi_agent::{
        AgentId, AgentLifecycleState, AgentProgressSnapshot, AgentToolActivityPhase,
        AgentToolFileChange, AgentToolFileOperation, AgentToolFileStatus, DelegateToolProgress,
        SwarmAggregate, SwarmChildProgress,
    },
    session::JsonlSessionReader,
};
use serde_json::json;

#[tokio::test]
async fn jsonl_session_reads_legacy_token_usage_without_cache_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    std::fs::write(
        &path,
        serde_json::to_string(&json!({
            "TokenUsage": {
                "turn": 1,
                "usage": {
                    "input_tokens": 33_900,
                    "output_tokens": 2_800
                }
            }
        }))
        .expect("legacy token usage json"),
    )
    .expect("write legacy session");

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");

    assert_eq!(
        events,
        vec![AgentEvent::TokenUsage {
            turn: 1,
            usage: neo_agent_core::AgentTokenUsage {
                input_tokens: 33_900,
                output_tokens: 2_800,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            },
        }]
    );
}

#[test]
fn compact_delegate_progress_events_deserialize_and_do_not_replay_messages() {
    let progress = AgentProgressSnapshot {
        agent_id: AgentId::from_suffix_for_test("compact"),
        state: AgentLifecycleState::Running,
        mode: neo_agent_core::multi_agent::AgentRunMode::Foreground,
        detached_from_foreground: false,
        started_at_ms: Some(41),
        updated_at_ms: 42,
        terminal_at_ms: None,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        tool_count: 1,
        token_count: 128,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed_ms: 500,
        latest_text: Some("reading files".to_owned()),
        latest_thinking: None,
        last_tool: Some(DelegateToolProgress {
            id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("crates/neo-agent-core/src/session/mod.rs".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: vec![AgentToolFileChange {
                path: "crates/neo-agent-core/src/session/mod.rs".to_owned(),
                operation: Some(AgentToolFileOperation::Edited),
                status: AgentToolFileStatus::Pending,
                line_count: None,
                added: None,
                removed: None,
                message: None,
            }],
            output_ref: None,
        }),
        outcome: None,
    };
    let event = AgentEvent::DelegateProgressUpdated {
        turn: 9,
        progress: progress.clone(),
        workflow_origin: None,
    };
    let json = serde_json::to_string(&event).expect("serialize compact event");

    let reparsed: AgentEvent = serde_json::from_str(&json).expect("deserialize compact event");
    assert_eq!(reparsed, event);

    let mut legacy = serde_json::to_value(&event).expect("serialize legacy-shaped event");
    legacy
        .pointer_mut("/DelegateProgressUpdated/progress")
        .and_then(serde_json::Value::as_object_mut)
        .expect("progress object")
        .remove("started_at_ms");
    legacy
        .pointer_mut("/DelegateProgressUpdated/progress/last_tool")
        .and_then(serde_json::Value::as_object_mut)
        .expect("last tool object")
        .remove("files");
    let legacy_event: AgentEvent =
        serde_json::from_value(legacy).expect("old progress event without files");
    let AgentEvent::DelegateProgressUpdated { progress, .. } = legacy_event else {
        panic!("expected delegate progress event");
    };
    assert_eq!(progress.started_at_ms, None);
    assert!(
        progress.last_tool.is_some_and(|tool| tool.files.is_empty()),
        "old progress events must default to no file rows"
    );

    let context = AgentContext::from_replay([reparsed].iter());
    assert!(context.messages().is_empty());
}

#[test]
fn compact_swarm_progress_events_deserialize_and_do_not_replay_messages() {
    let event = AgentEvent::DelegateSwarmProgressUpdated {
        turn: 3,
        swarm_id: "swarm-test".to_owned(),
        state: AgentLifecycleState::Running,
        aggregate: SwarmAggregate {
            total: 1,
            running: 1,
            ..SwarmAggregate::default()
        },
        child_progress: SwarmChildProgress {
            item_index: 0,
            progress: AgentProgressSnapshot {
                agent_id: AgentId::from_suffix_for_test("swarm-child"),
                state: AgentLifecycleState::Running,
                mode: neo_agent_core::multi_agent::AgentRunMode::Foreground,
                detached_from_foreground: false,
                started_at_ms: Some(6),
                updated_at_ms: 7,
                terminal_at_ms: None,
                terminal_reason: None,
                run_count: 1,
                live_messages_received: 0,
                tool_count: 0,
                token_count: 0,
                cache_read_token_count: 0,
                cache_write_token_count: 0,
                elapsed_ms: 0,
                latest_text: None,
                latest_thinking: None,
                last_tool: None,
                outcome: None,
            },
        },
        workflow_origin: None,
    };
    let json = serde_json::to_string(&event).expect("serialize compact swarm event");

    let reparsed: AgentEvent = serde_json::from_str(&json).expect("deserialize compact event");
    assert_eq!(reparsed, event);

    let context = AgentContext::from_replay([reparsed].iter());
    assert!(context.messages().is_empty());
}

#[test]
fn replay_accepts_old_context_window_updated_shape() {
    let json = r#"{"ContextWindowUpdated":{"turn":1,"used_tokens":123}}"#;
    let event: AgentEvent = serde_json::from_str(json).expect("old event should parse");
    assert!(matches!(
        event,
        AgentEvent::ContextWindowUpdated {
            turn: 1,
            used_tokens: 123,
            ..
        }
    ));
}

#[test]
fn replay_accepts_compaction_summary_without_new_metadata() {
    let json = r#"{
        "summary":"old summary",
        "tokens_before":100,
        "tokens_after":50,
        "first_kept_message_index":2
    }"#;
    let summary: CompactionSummary = serde_json::from_str(json).expect("old summary should parse");
    assert_eq!(summary.summary, "old summary");
    assert_eq!(summary.first_kept_message_index, 2);
}

#[test]
fn replay_ignores_old_context_window_event_for_authority() {
    let events = [
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("real history ".repeat(1_000)),
        },
        AgentEvent::ContextWindowUpdated {
            turn: 1,
            used_tokens: 1,
            projected_tokens: Some(1),
            max_tokens: Some(1_000_000),
            trigger_tokens: Some(800_000),
            remaining_tokens: Some(799_999),
            source: Some(ContextWindowSource::Configured),
        },
    ];

    let context = AgentContext::from_replay(events.iter());

    assert!(context.estimated_tokens() > 1);
}

#[tokio::test]
async fn jsonl_session_rejects_future_metadata_schema_version_before_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("future-schema.jsonl");
    let event = AgentEvent::MessageAppended {
        message: AgentMessage::user_text("must not replay"),
    };
    write_jsonl_lines(
        &path,
        [
            json!({
                "kind": "session_metadata",
                "format": "neo.session.jsonl",
                "schema_version": 999,
                "created_at": "1.000000000Z",
            }),
            serde_json::to_value(&event).expect("event json"),
        ],
    );

    let err = JsonlSessionReader::read_all(&path)
        .await
        .expect_err("future metadata schema version should fail closed");
    let message = err.to_string();
    assert!(
        message.contains("unsupported session metadata schema version 999"),
        "unexpected error: {message}"
    );
}

#[tokio::test]
async fn jsonl_session_rejects_future_metadata_schema_version_among_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("future-schema-midstream.jsonl");
    let first_event = AgentEvent::MessageAppended {
        message: AgentMessage::user_text("before metadata"),
    };
    let second_event = AgentEvent::MessageAppended {
        message: AgentMessage::user_text("after metadata"),
    };
    write_jsonl_lines(
        &path,
        [
            serde_json::to_value(&first_event).expect("first event json"),
            json!({
                "kind": "session_metadata",
                "format": "neo.session.jsonl",
                "schema_version": 999,
                "created_at": "1.000000000Z",
            }),
            serde_json::to_value(&second_event).expect("second event json"),
        ],
    );

    let err = JsonlSessionReader::read_all(&path)
        .await
        .expect_err("future metadata schema version should fail closed");
    let message = err.to_string();
    assert!(
        message.contains("unsupported session metadata schema version 999"),
        "unexpected error: {message}"
    );
}

fn write_jsonl_lines(path: &std::path::Path, lines: impl IntoIterator<Item = serde_json::Value>) {
    let content = lines
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("serialize jsonl line"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, format!("{content}\n")).expect("write jsonl session");
}

#[tokio::test]
async fn jsonl_session_reads_historical_include_cycle_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut historical = serde_json::to_value(AgentEvent::InstructionEpoch {
        epoch: instruction_epoch(1, "rev-1", None),
    })
    .expect("serialize epoch envelope");
    historical["InstructionEpoch"]["epoch"]["outcome"] = serde_json::json!("blocked");
    historical["InstructionEpoch"]["epoch"]["failure"] = serde_json::json!({
        "display_path": "/workspace/AGENTS.md",
        "kind": "include_cycle",
        "fingerprint": "historical-cycle",
        "detail": "historical include cycle",
    });
    write_jsonl_lines(&path, [historical]);

    let events = JsonlSessionReader::read_all(&path)
        .await
        .expect("read historical epoch");
    let Some(AgentEvent::InstructionEpoch { epoch }) = events.first() else {
        panic!("expected historical instruction epoch");
    };
    assert_eq!(
        epoch.failure.as_ref().map(|failure| failure.kind),
        Some(InstructionFailureKind::IncludeCycle),
    );
}
