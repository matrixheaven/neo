use neo_agent_core::session::{
    JsonlSessionReader, JsonlSessionWriter, SessionCompactionOptions, compact_jsonl_session,
};
use neo_agent_core::{
    AgentContext, AgentEvent, AgentMessage, AgentToolCall, CompactionSummary, Content, StopReason,
    TodoEventData,
};
use serde_json::json;

#[tokio::test]
async fn jsonl_session_appends_reads_and_replays_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    writer
        .append(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("remember this"),
        })
        .await
        .expect("append user");
    writer
        .append(&AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::EndTurn,
        })
        .await
        .expect("append finish");
    writer.flush().await.expect("flush");

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");
    assert_eq!(
        events,
        vec![
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("remember this"),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            },
        ]
    );

    let replayed = JsonlSessionReader::replay_messages(&path)
        .await
        .expect("replay");
    assert_eq!(replayed, vec![AgentMessage::user_text("remember this")]);
}

#[tokio::test]
async fn jsonl_session_create_writes_schema_metadata_without_replay_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    writer
        .append(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("metadata should not replay"),
        })
        .await
        .expect("append user");
    writer.flush().await.expect("flush");

    let content = std::fs::read_to_string(&path).expect("read session file");
    let lines = content.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);

    let metadata = serde_json::from_str::<serde_json::Value>(lines[0]).expect("metadata json");
    assert_eq!(
        metadata,
        json!({
            "kind": "session_metadata",
            "format": "neo.session.jsonl",
            "schema_version": 1,
            "created_at": metadata["created_at"],
        })
    );
    assert!(
        metadata["created_at"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");
    assert_eq!(
        events,
        vec![AgentEvent::MessageAppended {
            message: AgentMessage::user_text("metadata should not replay"),
        }]
    );

    let replayed = JsonlSessionReader::replay_messages(&path)
        .await
        .expect("replay");
    assert_eq!(
        replayed,
        vec![AgentMessage::user_text("metadata should not replay")]
    );
}

#[tokio::test]
async fn jsonl_session_replays_event_only_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("event-only.jsonl");
    let event = AgentEvent::MessageAppended {
        message: AgentMessage::user_text("event-only replay works"),
    };
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string(&event).expect("serialize event")
        ),
    )
    .expect("write event-only session");

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");
    assert_eq!(events, vec![event.clone()]);

    let replayed = JsonlSessionReader::replay_messages(&path)
        .await
        .expect("replay");
    assert_eq!(
        replayed,
        vec![AgentMessage::user_text("event-only replay works")]
    );
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

#[tokio::test]
async fn jsonl_session_replays_runtime_context_with_turns_and_terminal_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    for event in [
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("start"),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant([], Vec::new(), StopReason::EndTurn),
        },
        AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("stop"),
        },
        AgentEvent::TurnFinished {
            turn: 2,
            stop_reason: StopReason::Cancelled,
        },
    ] {
        writer.append(&event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");

    assert_eq!(
        context.messages(),
        &[
            AgentMessage::user_text("start"),
            AgentMessage::assistant([], Vec::new(), StopReason::EndTurn),
            AgentMessage::user_text("stop"),
        ]
    );
    assert_eq!(context.turns(), 2);

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");
    assert_eq!(AgentContext::from_replay(events.iter()), context);
}

#[tokio::test]
async fn jsonl_session_replay_context_applies_latest_todo_update() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    writer
        .append(&AgentEvent::TodoUpdated {
            turn: 1,
            todos: vec![TodoEventData {
                title: "Old".to_owned(),
                status: "done".to_owned(),
            }],
        })
        .await
        .expect("append non-empty todos");
    writer
        .append(&AgentEvent::TodoUpdated {
            turn: 2,
            todos: vec![],
        })
        .await
        .expect("append clear todos");
    writer.flush().await.expect("flush");

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");

    assert!(context.todos().is_empty());
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
async fn jsonl_session_replay_context_drops_incomplete_trailing_tool_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    for event in [
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("inspect project"),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [],
                [AgentToolCall {
                    id: "call-1".to_owned(),
                    name: "Read".to_owned(),
                    arguments: json!({ "path": "README.md" }),
                }],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::ToolUse,
        },
    ] {
        writer.append(&event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");

    assert_eq!(
        context.messages(),
        &[AgentMessage::user_text("inspect project")],
        "only the incomplete assistant tool_use tail should be dropped"
    );
    assert_eq!(context.turns(), 1);
}

#[tokio::test]
async fn jsonl_session_replay_context_keeps_complete_trailing_tool_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");
    let assistant = AgentMessage::assistant(
        [],
        [AgentToolCall {
            id: "call-1".to_owned(),
            name: "Read".to_owned(),
            arguments: json!({ "path": "README.md" }),
        }],
        StopReason::ToolUse,
    );
    let tool_result =
        AgentMessage::tool_result("call-1", "Read", [Content::text("README contents")], false);

    for event in [
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("inspect project"),
        },
        AgentEvent::MessageAppended {
            message: assistant.clone(),
        },
        AgentEvent::MessageAppended {
            message: tool_result.clone(),
        },
        AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::ToolUse,
        },
    ] {
        writer.append(&event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");

    assert_eq!(
        context.messages(),
        &[
            AgentMessage::user_text("inspect project"),
            assistant,
            tool_result,
        ]
    );
}

#[tokio::test]
async fn jsonl_session_replays_queues_and_compaction_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    let summary = CompactionSummary {
        summary: "Older work summarized".to_owned(),
        tokens_before: 4096,
        first_kept_message_index: 2,
    };
    for event in [
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("before"),
        },
        AgentEvent::SteeringQueued {
            message: AgentMessage::user_text("steer"),
        },
        AgentEvent::FollowUpQueued {
            message: AgentMessage::user_text("follow"),
        },
        AgentEvent::CompactionApplied {
            summary: summary.clone(),
        },
        AgentEvent::TurnFinished {
            turn: 3,
            stop_reason: StopReason::EndTurn,
        },
    ] {
        writer.append(&event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");

    assert_eq!(context.pending_steering_len(), 1);
    assert_eq!(context.pending_follow_up_len(), 1);
    assert_eq!(context.compaction_summary(), Some(&summary));
    assert_eq!(context.turns(), 3);
}

#[tokio::test]
async fn jsonl_session_compaction_appends_algorithmic_summary_and_replays_kept_context() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    for event in [
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("Investigate parser drift"),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [neo_agent_core::Content::text("Found JSONL mismatch")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("Keep the final request"),
        },
    ] {
        writer.append(&event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let result = compact_jsonl_session(
        &path,
        SessionCompactionOptions {
            keep_recent_messages: 1,
        },
    )
    .await
    .expect("compact session");

    assert_eq!(result.compacted_message_count, 2);
    assert_eq!(result.kept_message_count, 1);
    assert_eq!(result.summary.first_kept_message_index, 2);
    assert!(
        result
            .summary
            .summary
            .contains("Algorithmic transcript summary")
    );
    assert!(
        result
            .summary
            .summary
            .contains("user: Investigate parser drift")
    );
    assert!(
        result
            .summary
            .summary
            .contains("assistant: Found JSONL mismatch")
    );

    let events = JsonlSessionReader::read_all(&path)
        .await
        .expect("read events");
    assert!(matches!(
        events.last(),
        Some(AgentEvent::CompactionApplied { summary }) if summary == &result.summary
    ));

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay compacted context");
    assert_eq!(
        context.messages(),
        &[AgentMessage::user_text("Keep the final request")]
    );
    assert_eq!(context.compaction_summary(), Some(&result.summary));
}

#[tokio::test]
async fn jsonl_session_compaction_keeps_unsent_thinking_out_of_estimates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    for event in [
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [
                    Content::thinking("x".repeat(4_000), None, false),
                    Content::text("short answer"),
                ],
                Vec::new(),
                StopReason::EndTurn,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("keep this tiny follow-up"),
        },
    ] {
        writer.append(&event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let result = compact_jsonl_session(
        &path,
        SessionCompactionOptions {
            keep_recent_messages: 1,
        },
    )
    .await
    .expect("compact session");

    assert_eq!(result.compacted_message_count, 1);
    assert_eq!(result.summary.tokens_before, 9);
}
