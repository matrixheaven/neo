use neo_agent_core::{
    AgentEvent, AgentMessage, AgentToolCall, ApprovalAction, ApprovalOption, ApprovalPresentation,
    ApprovalRequest, ApprovalResolution, Content, PermissionOperation, StopReason,
    multi_agent::{
        AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
        AgentPath, AgentProgressSnapshot, AgentRole, AgentRunMode, AgentSnapshot,
        AgentToolActivityPhase, DelegateContext, SwarmAggregate, SwarmChildProgress,
        SwarmChildSnapshot, SwarmSnapshot,
    },
    session::{JsonlSessionReader, JsonlSessionWriter, SessionEventPersistence},
    workflow::{WorkflowExecutionOrigin, WorkflowId},
};
use serde_json::json;

pub(crate) fn background_bash_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
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
    }
}

#[tokio::test]
async fn jsonl_session_round_trips_requested_and_resolved_approval() {
    let request = background_bash_request();
    let requested = AgentEvent::ApprovalRequested {
        request: request.clone(),
    };
    let resolved = AgentEvent::ApprovalResolved {
        turn: 1,
        request_id: request.id.clone(),
        resolution: ApprovalResolution::Selected {
            action: ApprovalAction::Reject,
            label: "Reject".to_owned(),
            feedback: None,
        },
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");
    writer.append(&requested).await.expect("append request");
    writer.append(&resolved).await.expect("append resolution");
    writer.flush().await.expect("flush");
    assert_eq!(
        JsonlSessionReader::read_all(&path).await.expect("read"),
        vec![requested, resolved]
    );
}

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
async fn user_display_text_roundtrips_and_old_user_event_remains_readable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let event = AgentEvent::MessageAppended {
        message: AgentMessage::user_content_with_display(
            [Content::text("<file path=\"src/main.rs\">snapshot</file>")],
            "review @[main.rs]",
        ),
    };
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");
    writer.append(&event).await.expect("append event");
    writer.flush().await.expect("flush");

    let events = JsonlSessionReader::read_all(&path)
        .await
        .expect("read event");
    assert_eq!(events, vec![event]);
    let legacy: AgentEvent = serde_json::from_str(
        r#"{"MessageAppended":{"message":{"User":{"content":[{"Text":{"text":"legacy"}}]}}}}"#,
    )
    .expect("deserialize legacy user event");
    let AgentEvent::MessageAppended { message } = legacy else {
        panic!("expected appended message");
    };
    assert_eq!(message.display_text(), None);
    assert_eq!(message.text(), "legacy");
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
async fn cancelled_session_lock_wait_leaves_no_waiter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            JsonlSessionWriter::open_append(&path),
        )
        .await
        .is_err(),
        "second writer should wait for the live writer"
    );
    drop(writer);

    let next = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        JsonlSessionWriter::open_append(&path),
    )
    .await
    .expect("cancelled wait must not retain the lock")
    .expect("open writer after cancellation");
    drop(next);
}

#[test]
fn session_persists_queue_transition_but_not_live_queue_updates() {
    let mut persistence = SessionEventPersistence::default();
    let queued = AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: "call-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: json!({"command": "printf ready"}),
        workflow_origin: None,
    };
    let update = AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "call-1".to_owned(),
        position: 2,
        waiting_ms: 18_000,
    };
    assert_eq!(persistence.persisted_events(&queued), vec![queued]);
    assert!(persistence.persisted_events(&update).is_empty());
}

#[test]
fn delegate_persistence_strips_live_shell_queue_metadata_and_preserves_workflow_origin() {
    let mut agent = queued_shell_agent_snapshot("call-1", Some(2), 2_000);
    let mut persistence = SessionEventPersistence::default();
    let origin = WorkflowExecutionOrigin {
        run_id: WorkflowId("workflow-run".to_owned()),
        human_handle: None,
        definition_name: "workflow".to_owned(),
        definition_revision: None,
        phase_id: Some("verify".to_owned()),
        invocation_id: Some("delegate-call".to_owned()),
        swarm_item_id: None,
    };

    let finished = AgentEvent::DelegateFinished {
        turn: 1,
        agent: agent.clone(),
        workflow_origin: None,
    };
    let persisted = persistence.persisted_events(&finished);
    assert_eq!(persisted.len(), 1);
    assert_queued_phase_stripped(&persisted[0]);

    let swarm = SwarmSnapshot {
        swarm_id: "swarm-queue".to_owned(),
        description: "queued child".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate: SwarmAggregate {
            total: 1,
            running: 1,
            ..SwarmAggregate::default()
        },
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "item-0".to_owned(),
            agent: agent.clone(),
        }],
    };
    let swarm_finished = AgentEvent::DelegateSwarmFinished {
        turn: 1,
        swarm: swarm.clone(),
        workflow_origin: None,
    };
    let mut swarm_persistence = SessionEventPersistence::default();
    let persisted_swarm = swarm_persistence.persisted_events(&swarm_finished);
    assert_eq!(persisted_swarm.len(), 1);
    assert_queued_phase_stripped(&persisted_swarm[0]);

    // Compact progress uses the same projection: first update persists the
    // stripped snapshot, and later rank/wait-only changes coalesce away.
    let mut progress_persistence = SessionEventPersistence::default();
    let updated = AgentEvent::DelegateUpdated {
        turn: 2,
        agent: agent.clone(),
        workflow_origin: Some(origin.clone()),
    };
    let progress_events = progress_persistence.persisted_events(&updated);
    assert_eq!(progress_events.len(), 1);
    assert_queued_phase_stripped(&progress_events[0]);
    assert!(matches!(
        &progress_events[0],
        AgentEvent::DelegateProgressUpdated {
            workflow_origin: Some(persisted),
            ..
        } if persisted == &origin
    ));
    let restored: AgentEvent = serde_json::from_str(
        &serde_json::to_string(&progress_events[0]).expect("serialize delegate progress"),
    )
    .expect("restore delegate progress");
    assert_eq!(restored, progress_events[0]);

    if let AgentActivityKind::Tool { phase, .. } = &mut agent.activity[0].kind {
        *phase = AgentToolActivityPhase::Queued {
            position: Some(1),
            queued_at_ms: 5_000,
        };
    }
    agent.updated_at_ms = 5_000;
    assert!(
        progress_persistence
            .persisted_events(&AgentEvent::DelegateUpdated {
                turn: 3,
                agent,
                workflow_origin: None,
            })
            .is_empty()
    );

    let mut direct_progress = swarm.children[0].agent.progress_snapshot();
    let direct = AgentEvent::DelegateSwarmProgressUpdated {
        turn: 4,
        swarm_id: swarm.swarm_id.clone(),
        state: swarm.state,
        aggregate: swarm.aggregate,
        child_progress: SwarmChildProgress {
            item_index: 0,
            progress: direct_progress.clone(),
        },
        workflow_origin: Some(origin.clone()),
    };
    let mut direct_persistence = SessionEventPersistence::default();
    let direct_events = direct_persistence.persisted_events(&direct);
    assert_eq!(direct_events.len(), 1);
    assert_queued_phase_stripped(&direct_events[0]);
    assert!(matches!(
        &direct_events[0],
        AgentEvent::DelegateSwarmProgressUpdated {
            workflow_origin: Some(persisted),
            ..
        } if persisted == &origin
    ));
    let restored: AgentEvent = serde_json::from_str(
        &serde_json::to_string(&direct_events[0]).expect("serialize swarm progress"),
    )
    .expect("restore swarm progress");
    assert_eq!(restored, direct_events[0]);

    let Some(tool) = &mut direct_progress.last_tool else {
        panic!("expected queued tool progress");
    };
    tool.phase = AgentToolActivityPhase::Queued {
        position: Some(1),
        queued_at_ms: 5_000,
    };
    direct_progress.updated_at_ms = 5_000;
    assert!(
        direct_persistence
            .persisted_events(&AgentEvent::DelegateSwarmProgressUpdated {
                turn: 5,
                swarm_id: swarm.swarm_id,
                state: swarm.state,
                aggregate: swarm.aggregate,
                child_progress: SwarmChildProgress {
                    item_index: 0,
                    progress: direct_progress,
                },
                workflow_origin: None,
            })
            .is_empty()
    );
}

#[tokio::test]
async fn queue_metadata_never_enters_tool_result_or_replayed_model_messages() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");
    let mut persistence = SessionEventPersistence::default();

    let events = [
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("run shell"),
        },
        AgentEvent::ToolExecutionQueued {
            turn: 1,
            id: "call-queue".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"command": "printf ready"}),
            workflow_origin: None,
        },
        AgentEvent::ToolExecutionQueueUpdated {
            turn: 1,
            id: "call-queue".to_owned(),
            position: 3,
            waiting_ms: 12_500,
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "call-queue".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"command": "printf ready"}),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "call-queue".to_owned(),
            name: "Bash".to_owned(),
            result: neo_agent_core::ToolResult::ok("ready").with_details(json!({
                "exit_code": 0,
                "outcome": "completed",
                "stdout": "ready",
                "stderr": "",
            })),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                Vec::new(),
                vec![AgentToolCall {
                    id: "call-queue".into(),
                    name: "Bash".into(),
                    raw_arguments: json!({"command": "printf ready"}).to_string().into(),
                }],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "call-queue",
                "Bash",
                vec![Content::text("ready")],
                false,
            ),
        },
    ];

    for event in &events {
        for persisted in persistence.persisted_events(event) {
            writer
                .append_event(&persisted)
                .await
                .expect("append persisted event");
        }
    }
    writer.flush().await.expect("flush");

    let on_disk = JsonlSessionReader::read_all(&path).await.expect("read all");
    assert!(
        on_disk
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolExecutionQueued { .. })),
        "queued transition itself remains durable"
    );
    assert!(
        on_disk
            .iter()
            .all(|event| !matches!(event, AgentEvent::ToolExecutionQueueUpdated { .. })),
        "live queue rank/wait updates must never be persisted"
    );
    for event in &on_disk {
        if let AgentEvent::ToolExecutionFinished { result, .. } = event {
            let visible = format!("{} {:?}", result.content, result.details);
            assert!(
                !visible.contains("position") && !visible.contains("waiting_ms"),
                "tool result must not carry queue metadata: {visible}"
            );
        }
    }

    let replayed = JsonlSessionReader::replay_messages(&path)
        .await
        .expect("replay model messages");
    let replay_text = format!("{replayed:?}");
    assert!(
        !replay_text.contains("position")
            && !replay_text.contains("waiting_ms")
            && !replay_text.contains("queued_at_ms"),
        "replayed model messages must not include live queue metadata: {replay_text}"
    );
    assert!(
        replayed.iter().any(|message| {
            matches!(message, AgentMessage::ToolResult { .. }) && message.text().contains("ready")
        }),
        "tool result content itself should still replay: {replayed:?}"
    );
}

fn queued_shell_agent_snapshot(
    tool_id: &str,
    position: Option<usize>,
    queued_at_ms: u64,
) -> AgentSnapshot {
    let name = AgentDisplayName::new("Gibbs");
    AgentSnapshot {
        id: AgentId::from_suffix_for_test("queued-shell"),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "run tests".to_owned(),
        task_title: "run tests".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 1,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        input_token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: std::time::Duration::from_secs(1),
        latest_text: None,
        activity: vec![AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: tool_id.to_owned(),
                name: "Bash".to_owned(),
                summary: Some("cargo test".to_owned()),
                phase: AgentToolActivityPhase::Queued {
                    position,
                    queued_at_ms,
                },
                output: None,
                files: Vec::new(),
                output_ref: None,
            },
        }],
        prior_messages: Vec::new(),
        outcome: None,
    }
}

fn assert_queued_phase_stripped(event: &AgentEvent) {
    match event {
        AgentEvent::DelegateFinished { agent, .. } | AgentEvent::DelegateStarted { agent, .. } => {
            assert_agent_queue_stripped(agent);
        }
        AgentEvent::DelegateProgressUpdated { progress, .. } => {
            assert_progress_queue_stripped(progress);
        }
        AgentEvent::DelegateSwarmFinished { swarm, .. }
        | AgentEvent::DelegateSwarmStarted { swarm, .. } => {
            for child in &swarm.children {
                assert_agent_queue_stripped(&child.agent);
            }
        }
        AgentEvent::DelegateSwarmProgressUpdated { child_progress, .. } => {
            assert_progress_queue_stripped(&child_progress.progress);
        }
        other => panic!("unexpected persisted event: {other:?}"),
    }
}

fn assert_agent_queue_stripped(agent: &AgentSnapshot) {
    for entry in &agent.activity {
        if let AgentActivityKind::Tool { phase, .. } = &entry.kind {
            assert_phase_queue_stripped(phase);
        }
    }
    assert_progress_queue_stripped(&agent.progress_snapshot());
}

fn assert_progress_queue_stripped(progress: &AgentProgressSnapshot) {
    if let Some(tool) = &progress.last_tool {
        assert_phase_queue_stripped(&tool.phase);
    }
}

fn assert_phase_queue_stripped(phase: &AgentToolActivityPhase) {
    match phase {
        AgentToolActivityPhase::Queued {
            position,
            queued_at_ms,
        } => {
            assert_eq!(*position, None);
            assert_eq!(*queued_at_ms, 0);
        }
        AgentToolActivityPhase::Ongoing
        | AgentToolActivityPhase::Done
        | AgentToolActivityPhase::Failed => {}
    }
}

#[tokio::test]
async fn tool_output_reference_is_optional_and_round_trips() {
    use neo_agent_core::ShellCommandOrigin;
    use neo_agent_core::ShellCommandOutcome;
    use neo_agent_core::ToolResult;
    use neo_agent_core::session::ToolOutputRef;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");
    let complete = ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "bash-1".to_owned(),
        byte_len: 4096,
        line_count: 12,
        complete: true,
    };
    let incomplete = ToolOutputRef {
        agent_id: "child-a".to_owned(),
        task_id: "terminal-abc".to_owned(),
        byte_len: 1024,
        line_count: 3,
        complete: false,
    };
    let events = vec![
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "call-bash".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"command": "printf round-trip"}),
            workflow_origin: None,
            output_ref: Some(complete.clone()),
        },
        AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "call-bash".to_owned(),
            name: "Bash".to_owned(),
            partial_result: ToolResult::ok("progress"),
            workflow_origin: None,
            output_ref: Some(incomplete.clone()),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "call-bash".to_owned(),
            name: "Bash".to_owned(),
            result: ToolResult::ok("done"),
            workflow_origin: None,
            output_ref: Some(complete.clone()),
        },
        AgentEvent::ShellCommandFinished {
            turn: 1,
            id: "call-bash".to_owned(),
            exit_code: Some(0),
            signal: None,
            stdout: "done".to_owned(),
            stderr: String::new(),
            truncated: false,
            origin: ShellCommandOrigin::ModelBashTool,
            outcome: ShellCommandOutcome::Completed,
            output_ref: Some(complete.clone()),
        },
        AgentEvent::TerminalSessionStarted {
            turn: 1,
            id: "call-term".to_owned(),
            handle: "abc".to_owned(),
            command: "sleep 1".to_owned(),
            cwd: std::path::PathBuf::from("/workspace"),
            cols: 80,
            rows: 24,
            output_ref: Some(incomplete.clone()),
        },
        AgentEvent::TerminalSessionOutput {
            turn: 1,
            id: "call-term".to_owned(),
            handle: "abc".to_owned(),
            output: "partial".to_owned(),
            truncated: false,
            output_ref: Some(incomplete.clone()),
        },
        AgentEvent::TerminalSessionFinished {
            turn: 1,
            id: "call-term".to_owned(),
            handle: "abc".to_owned(),
            status: "completed".to_owned(),
            exit_code: Some(0),
            output_ref: Some(complete),
        },
        // A legacy-shaped finished event must deserialize with `None`.
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "legacy".to_owned(),
            name: "Bash".to_owned(),
            result: ToolResult::ok("old"),
            workflow_origin: None,
            output_ref: None,
        },
    ];
    for event in &events {
        writer.append(event).await.expect("append event");
    }
    writer.flush().await.expect("flush");

    let replayed = JsonlSessionReader::read_all(&path)
        .await
        .expect("read events");
    assert_eq!(replayed, events);

    // Old records without the field stay readable and deserialize to `None`.
    let legacy = json!({
        "ToolExecutionFinished": {
            "turn": 2,
            "id": "legacy-raw",
            "name": "Bash",
            "result": {"content": "old", "is_error": false, "terminate": false},
            "workflow_origin": null,
        }
    });
    let deserialized: AgentEvent = serde_json::from_value(legacy).expect("legacy record");
    assert!(matches!(
        deserialized,
        AgentEvent::ToolExecutionFinished {
            output_ref: None,
            ..
        }
    ));

    // The reference is presentation metadata: it must never serialize inside
    // the model-visible `ToolResult` payload of a finished event.
    let finished = events
        .iter()
        .find(|event| matches!(event, AgentEvent::ToolExecutionFinished { id, .. } if id == "call-bash"))
        .expect("finished event");
    let serialized = serde_json::to_value(finished).expect("serialize finished");
    assert_eq!(
        serialized["ToolExecutionFinished"]["result"]["content"],
        "done"
    );
    assert!(
        serialized["ToolExecutionFinished"]["result"]
            .get("output_ref")
            .is_none()
    );
    assert!(
        serialized["ToolExecutionFinished"]["result"]
            .get("byte_len")
            .is_none()
    );
}
