use neo_agent_core::{
    AgentContext, AgentEvent, AgentMessage, AgentToolCall, ApprovalAction, ApprovalOption,
    ApprovalPresentation, ApprovalRequest, CompactionSummary, Content, PermissionOperation,
    StopReason, TodoEventData,
    session::{
        JsonlSessionReader, JsonlSessionWriter, SessionCompactionOptions, compact_jsonl_session,
        main_agent_wire_path,
    },
};
use serde_json::json;

#[tokio::test]
async fn jsonl_session_reads_concatenated_records_from_interrupted_append() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let approval = AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "call_approval".to_owned(),
            operation: PermissionOperation::FileWrite,
            presentation: ApprovalPresentation::Tool {
                title: "Write file?".to_owned(),
                details: vec!["docs/large.md".to_owned(), "x".repeat(16 * 1024)],
            },
            options: vec![
                ApprovalOption {
                    label: "Approve once".to_owned(),
                    description: None,
                    action: ApprovalAction::PermitOnce,
                },
                ApprovalOption {
                    label: "Reject".to_owned(),
                    description: None,
                    action: ApprovalAction::Reject,
                },
            ],
            workflow_origin: None,
        },
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
    drop(writer);

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
    drop(writer);

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
async fn jsonl_session_compaction_waits_for_live_writer_before_reading() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");
    writer
        .append(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("first"),
        })
        .await
        .expect("append first");
    writer.flush().await.expect("flush first");

    let compact_path = path.clone();
    let compaction = tokio::spawn(async move {
        compact_jsonl_session(
            compact_path,
            SessionCompactionOptions {
                keep_recent_messages: 0,
            },
        )
        .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tokio::time::sleep(std::time::Duration::from_millis(10)),
    )
    .await
    .expect("lock contention must not block the async runtime");
    for _ in 0..100 {
        assert!(
            !compaction.is_finished(),
            "compaction must wait while the live writer owns the session"
        );
        tokio::task::yield_now().await;
    }

    writer
        .append(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("second"),
        })
        .await
        .expect("append second");
    writer.flush().await.expect("flush second");
    drop(writer);

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), compaction)
        .await
        .expect("compaction should acquire the released lock")
        .expect("compaction task")
        .expect("compact session");
    assert_eq!(result.compacted_message_count, 2);
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
