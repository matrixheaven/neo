use neo_agent_core::instructions::{
    InstructionBundleMetadata, InstructionEpochData, InstructionEpochOutcome, InstructionScopeData,
    InstructionScopeKind,
};
use neo_agent_core::multi_agent::{
    AgentId, AgentLifecycleState, AgentProgressSnapshot, AgentToolActivityPhase,
    DelegateToolProgress, SwarmAggregate, SwarmChildProgress,
};
use neo_agent_core::session::{
    JsonlSessionReader, JsonlSessionWriter, SessionCompactionOptions, SessionEventPersistence,
    compact_jsonl_session,
};
use neo_agent_core::session::{
    SessionAgentKind, SessionAgentRecord, SessionState, SessionStateStore, agent_tasks_dir,
    agent_wire_path, agents_dir, main_agent_wire_path, relative_agent_record_dir,
    session_state_path,
};
use neo_agent_core::{
    AgentContext, AgentEvent, AgentMessage, AgentToolCall, CompactionSummary, Content,
    ContextWindowSource, PermissionOperation, StopReason, TodoEventData,
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

#[tokio::test]
async fn jsonl_session_preserves_newline_when_large_unflushed_event_is_followed_by_append() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");

    {
        let mut writer = JsonlSessionWriter::create(&path)
            .await
            .expect("create session");
        writer
            .append(&AgentEvent::ApprovalRequested {
                turn: 1,
                id: "call_approval".to_owned(),
                operation: PermissionOperation::FileWrite,
                subject: "docs/large.md".to_owned(),
                arguments: json!({ "content": "x".repeat(16 * 1024) }),
                session_scope: None,
                prefix_rule: None,
                suggestions: Vec::new(),
            })
            .await
            .expect("append large approval");
        // Simulate an interrupted process while blocked on approval. Large writes
        // must still leave the file ready for the next append.
    }

    let mut writer = JsonlSessionWriter::open_append(&path)
        .await
        .expect("open append");
    writer
        .append(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("continued"),
        })
        .await
        .expect("append continued message");
    writer.flush().await.expect("flush");

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");

    assert!(matches!(
        events.as_slice(),
        [
            AgentEvent::ApprovalRequested { .. },
            AgentEvent::MessageAppended { .. }
        ]
    ));
}

#[tokio::test]
async fn jsonl_session_reads_concatenated_records_from_interrupted_append() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let approval = AgentEvent::ApprovalRequested {
        turn: 1,
        id: "call_approval".to_owned(),
        operation: PermissionOperation::FileWrite,
        subject: "docs/large.md".to_owned(),
        arguments: json!({ "content": "x".repeat(16 * 1024) }),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    };
    let continued = AgentEvent::MessageAppended {
        message: AgentMessage::user_text("continued"),
    };
    std::fs::write(
        &path,
        format!(
            "{}{}\n",
            serde_json::to_string(&approval).expect("approval json"),
            serde_json::to_string(&continued).expect("continued json")
        ),
    )
    .expect("write concatenated session");

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");

    assert_eq!(events, vec![approval, continued]);
}

#[tokio::test]
async fn jsonl_session_drops_torn_final_line_on_replay() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let valid = AgentEvent::MessageAppended {
        message: AgentMessage::user_text("survives"),
    };
    std::fs::write(
        &path,
        format!(
            "{}\n{{\"MessageAppended\":{{\"message\"",
            serde_json::to_string(&valid).expect("valid json")
        ),
    )
    .expect("write torn session");

    let events = JsonlSessionReader::read_all(&path).await.expect("read all");

    assert_eq!(events, vec![valid]);
}

#[tokio::test]
async fn jsonl_session_rejects_corrupt_middle_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let valid = AgentEvent::MessageAppended {
        message: AgentMessage::user_text("survives"),
    };
    std::fs::write(
        &path,
        format!(
            "{}\n{{\"MessageAppended\":{{\"message\"\n{}\n",
            serde_json::to_string(&valid).expect("valid json"),
            serde_json::to_string(&valid).expect("valid json")
        ),
    )
    .expect("write corrupt session");

    let error = JsonlSessionReader::read_all(&path)
        .await
        .expect_err("middle corruption must fail");

    assert!(matches!(
        error,
        neo_agent_core::session::SessionError::Json { line: 2, .. }
    ));
}

#[test]
fn compact_delegate_progress_events_deserialize_and_do_not_replay_messages() {
    let progress = AgentProgressSnapshot {
        agent_id: AgentId::from_suffix_for_test("compact"),
        state: AgentLifecycleState::Running,
        mode: neo_agent_core::multi_agent::AgentRunMode::Foreground,
        detached_from_foreground: false,
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
        last_tool: Some(DelegateToolProgress {
            id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("crates/neo-agent-core/src/session/mod.rs".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
        }),
        outcome: None,
    };
    let event = AgentEvent::DelegateProgressUpdated {
        turn: 9,
        progress: progress.clone(),
    };
    let json = serde_json::to_string(&event).expect("serialize compact event");

    let reparsed: AgentEvent = serde_json::from_str(&json).expect("deserialize compact event");
    assert_eq!(reparsed, event);

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
                last_tool: None,
                outcome: None,
            },
        },
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

#[test]
fn replay_drops_incomplete_trailing_tool_exchange_before_budgeting() {
    let events = [
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                Vec::new(),
                vec![
                    AgentToolCall {
                        id: "a".into(),
                        name: "Read".into(),
                        raw_arguments: "{}".into(),
                    },
                    AgentToolCall {
                        id: "b".into(),
                        name: "Read".into(),
                        raw_arguments: "{}".into(),
                    },
                ],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result("a", "Read", vec![Content::text("done")], false),
        },
    ];

    let context = AgentContext::from_replay(events.iter());

    assert!(context.messages().is_empty());
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
                    id: "call-1".into(),
                    name: "Read".into(),
                    raw_arguments: json!({ "path": "README.md" }).to_string().into(),
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
            id: "call-1".into(),
            name: "Read".into(),
            raw_arguments: json!({ "path": "README.md" }).to_string().into(),
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
        tokens_after: 2048,
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
    let session_dir = dir
        .path()
        .join("session_00000000-0000-0000-0000-000000000001");
    let path = main_agent_wire_path(&session_dir);
    std::fs::create_dir_all(path.parent().expect("wire parent")).expect("mkdir wire parent");
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
    // The compaction summary is now injected as a system message at the start
    // of the kept messages, so the model has context after compaction.
    assert_eq!(context.messages().len(), 2);
    assert!(matches!(
        context.messages().first(),
        Some(AgentMessage::System { content }) if content.iter().any(|c| c.as_text().is_some_and(|t| t.contains("compaction_summary")))
    ));
    assert!(matches!(
        context.messages().get(1),
        Some(AgentMessage::User { .. })
    ));
    assert_eq!(context.compaction_summary(), Some(&result.summary));
}

#[tokio::test]
async fn jsonl_session_compaction_keeps_unsent_thinking_out_of_estimates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_dir = dir
        .path()
        .join("session_00000000-0000-0000-0000-000000000001");
    let path = main_agent_wire_path(&session_dir);
    std::fs::create_dir_all(path.parent().expect("wire parent")).expect("mkdir wire parent");
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
    assert_eq!(result.summary.tokens_before, 13);
}

#[tokio::test]
async fn jsonl_session_replays_queue_drained_clears_queues() {
    use neo_agent_core::QueueKind;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    for event in [
        AgentEvent::SteeringQueued {
            message: AgentMessage::user_text("steer one"),
        },
        AgentEvent::FollowUpQueued {
            message: AgentMessage::user_text("follow one"),
        },
        AgentEvent::QueueDrained {
            kind: QueueKind::Steering,
            count: 1,
        },
        AgentEvent::QueueDrained {
            kind: QueueKind::FollowUp,
            count: 1,
        },
    ] {
        writer.append(&event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");

    assert_eq!(
        context.pending_steering_len(),
        0,
        "QueueDrained(Steering) should clear the steering queue on replay"
    );
    assert_eq!(
        context.pending_follow_up_len(),
        0,
        "QueueDrained(FollowUp) should clear the follow-up queue on replay"
    );
}

#[test]
fn session_layout_paths_are_agent_scoped() {
    let session_dir = std::path::Path::new("/tmp/neo-session");

    assert_eq!(
        session_state_path(session_dir),
        session_dir.join("state.json")
    );
    assert_eq!(agents_dir(session_dir), session_dir.join("agents"));
    assert_eq!(
        main_agent_wire_path(session_dir),
        session_dir.join("agents").join("main").join("wire.jsonl")
    );
    assert_eq!(
        agent_wire_path(session_dir, "agent_abc"),
        session_dir
            .join("agents")
            .join("agent_abc")
            .join("wire.jsonl")
    );
    assert_eq!(
        agent_tasks_dir(session_dir, "agent_abc"),
        session_dir.join("agents").join("agent_abc").join("tasks")
    );
}

#[tokio::test]
async fn session_state_store_round_trips_main_and_subagent_records() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionStateStore::new(temp.path());
    let mut state = SessionState::new();
    state.ensure_main_agent();
    state.upsert_agent(SessionAgentRecord {
        kind: SessionAgentKind::Sub,
        record_dir: relative_agent_record_dir("agent_abc"),
        parent_agent_id: Some("main".to_owned()),
        role: Some("coder".to_owned()),
        swarm_id: Some("swarm_1".to_owned()),
        swarm_item: Some("crate-a".to_owned()),
    });

    store.write(&state).expect("write state");
    let loaded = store.read().await.expect("read state");

    assert_eq!(loaded.schema_version, 1);
    assert_eq!(
        loaded.agents.get("main").expect("main").record_dir,
        relative_agent_record_dir("main")
    );
    assert_eq!(
        loaded
            .agents
            .get("agent_abc")
            .expect("child")
            .parent_agent_id
            .as_deref(),
        Some("main")
    );
}

#[tokio::test]
async fn session_state_store_reads_missing_state_with_main_agent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionStateStore::new(temp.path());

    let loaded = store.read().await.expect("read default state");

    assert_eq!(loaded.schema_version, 1);
    let main = loaded.agents.get("main").expect("main");
    assert_eq!(main.record_dir, relative_agent_record_dir("main"));
    assert_eq!(main.parent_agent_id, None);
    assert_eq!(main.role, None);
    assert_eq!(main.swarm_id, None);
    assert_eq!(main.swarm_item, None);
}

#[tokio::test]
async fn session_state_store_adds_missing_main_agent_when_reading_existing_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionStateStore::new(temp.path());
    store
        .write(&SessionState::new())
        .expect("write state without main");

    let loaded = store.read().await.expect("read state");

    assert_eq!(loaded.schema_version, 1);
    assert_eq!(
        loaded.agents.get("main").expect("main").record_dir,
        relative_agent_record_dir("main")
    );
}

fn instruction_epoch(
    generation: u64,
    revision: &str,
    model_content: Option<&str>,
) -> InstructionEpochData {
    let scope = std::path::PathBuf::from("/workspace");
    InstructionEpochData {
        agent_id: "main".to_owned(),
        generation,
        outcome: InstructionEpochOutcome::Activated,
        scopes: vec![InstructionScopeData {
            display_path: scope.clone(),
            kind: InstructionScopeKind::WorkspaceRoot,
            revision: Some(revision.to_owned()),
            token_estimate: 12,
        }],
        selected_bundles: vec![InstructionBundleMetadata {
            display_path: scope,
            revision: revision.to_owned(),
            token_estimate: 12,
            byte_size: 64,
            source_count: 1,
            import_count: 0,
            import_paths: Vec::new(),
        }],
        ignored_bundles: Vec::new(),
        replacements: Vec::new(),
        failure: None,
        deferred_tool_ids: Vec::new(),
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        model_content: model_content.map(str::to_owned),
    }
}

#[tokio::test]
async fn instruction_epoch_persists_once_and_replays_model_context() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let epoch = instruction_epoch(1, "rev-1", Some("scoped rules body"));
    let event = AgentEvent::InstructionEpoch { epoch };

    // The epoch event is the single persisted source: the persistence layer
    // emits it exactly once and never synthesizes a MessageAppended copy.
    let mut persistence = SessionEventPersistence::default();
    let persisted = persistence.persisted_events(&event);
    assert_eq!(persisted, vec![event]);

    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");
    for persisted_event in &persisted {
        writer.append(persisted_event).await.expect("append epoch");
    }
    writer.flush().await.expect("flush");

    let wire = std::fs::read_to_string(&path).expect("read wire");
    assert_eq!(
        wire.matches("\"InstructionEpoch\"").count(),
        1,
        "epoch persisted exactly once: {wire}"
    );
    assert!(
        !wire.contains("MessageAppended"),
        "no duplicate MessageAppended copy: {wire}"
    );

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");
    assert_eq!(context.instruction_state().visible_generation, 1);
    assert_eq!(
        context
            .instruction_state()
            .visible_revisions
            .get(std::path::Path::new("/workspace"))
            .map(String::as_str),
        Some("rev-1")
    );
    assert_eq!(context.messages().len(), 1);
    let Some(AgentMessage::Instruction {
        generation,
        content,
    }) = context.messages().first()
    else {
        panic!("expected one pinned instruction message");
    };
    assert_eq!(*generation, 1);
    assert_eq!(
        content
            .iter()
            .filter_map(Content::as_text)
            .collect::<String>(),
        "scoped rules body"
    );
}
