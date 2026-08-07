use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentPathKind, AgentRole, AgentRunMode,
    AgentTerminalReason, AgentToolActivityPhase, AgentToolFileChange, AgentToolFileOperation,
    AgentToolFileStatus, AgentToolOutputPreview, MultiAgentRuntime, SwarmAggregate,
    SwarmChildProgress, SwarmChildSnapshot, SwarmSnapshot, apply_agent_progress,
};
use neo_agent_core::tools::ToolResult;
use serde_json::json;

#[test]
fn agent_tool_activity_uses_explicit_phase_and_output_preview() {
    let activity = AgentActivityKind::Tool {
        id: "call_1".to_owned(),
        name: "Bash".to_owned(),
        summary: Some("cargo nextest run -p neo-tui".to_owned()),
        phase: AgentToolActivityPhase::Ongoing,
        output: Some(AgentToolOutputPreview {
            text: "Compiling neo-tui v0.1.0".to_owned(),
            is_error: false,
            truncated: false,
            tail: true,
        }),
        files: Vec::new(),
        output_ref: None,
    };

    let serialized = serde_json::to_value(&activity).expect("serialize activity");

    assert_eq!(serialized["phase"], "ongoing");
    assert_eq!(serialized["output"]["tail"], true);
    assert!(
        serialized.get("failed").is_none(),
        "old failed bool must not remain in the canonical schema: {serialized}"
    );
}

#[test]
fn compact_delegate_progress_restores_and_resumes() {
    use neo_agent_core::multi_agent::{DelegateContext, DelegateRequest};

    let runtime = MultiAgentRuntime::new();
    let mut snapshot = runtime.start_foreground_delegate_for_test("audit compact progress");
    let agent_id = snapshot.id.as_str().to_owned();
    snapshot.state = AgentLifecycleState::Completed;
    snapshot.terminal_reason = Some(AgentTerminalReason::Completed);
    snapshot.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: "compact resume ok".to_owned(),
        is_error: false,
    });
    snapshot.latest_text = Some("finished compact audit".to_owned());
    snapshot.tool_count = 2;
    let progress = snapshot.progress_snapshot();
    let events = [
        AgentEvent::DelegateStarted {
            turn: 4,
            agent: runtime
                .agent_snapshot(&agent_id)
                .expect("started agent snapshot"),
            workflow_origin: None,
        },
        AgentEvent::DelegateProgressUpdated {
            turn: 4,
            progress,
            workflow_origin: None,
        },
    ];

    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay(events.iter());

    let restored_snapshot = restored
        .agent_snapshot(&agent_id)
        .expect("restored compact agent");
    assert_eq!(restored_snapshot.state, AgentLifecycleState::Completed);
    assert_eq!(restored_snapshot.tool_count, 2);
    assert_eq!(
        restored_snapshot.latest_text.as_deref(),
        Some("finished compact audit")
    );

    let request = DelegateRequest {
        task: "continue compact audit".to_owned(),
        resume: Some(agent_id.clone()),
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        output_schema: None,
    };
    let resumed = restored
        .start_resume_delegate(&agent_id, &request)
        .expect("resume compact restored agent");

    assert_eq!(resumed.run_count, 2);
    assert_eq!(
        resumed.previous_status,
        Some(AgentLifecycleState::Completed)
    );
}

#[test]
fn compact_running_delegate_progress_restores_as_interrupted() {
    let runtime = MultiAgentRuntime::new();
    let mut snapshot = runtime.start_foreground_delegate_for_test("resume interrupted compact");
    let agent_id = snapshot.id.as_str().to_owned();
    snapshot.latest_text = Some("halfway done".to_owned());
    let progress = snapshot.progress_snapshot();
    let events = [
        AgentEvent::DelegateStarted {
            turn: 5,
            agent: snapshot,
            workflow_origin: None,
        },
        AgentEvent::DelegateProgressUpdated {
            turn: 5,
            progress,
            workflow_origin: None,
        },
    ];

    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay(events.iter());

    let lost = restored
        .agent_snapshot(&agent_id)
        .expect("restored compact running agent");
    assert_eq!(lost.state, AgentLifecycleState::Interrupted);
    assert_eq!(
        lost.terminal_reason,
        Some(AgentTerminalReason::ProcessExited)
    );
    assert_eq!(lost.latest_text.as_deref(), Some("halfway done"));
}

#[test]
fn compact_swarm_child_progress_refreshes_aggregate_and_ordering() {
    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.new_swarm_id();
    let child = runtime.start_delegate(
        "write docs",
        Some("docs"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let mut completed = child.clone();
    completed.state = AgentLifecycleState::Completed;
    completed.terminal_reason = Some(AgentTerminalReason::Completed);
    completed.tool_count = 4;
    let started = SwarmSnapshot {
        swarm_id: swarm_id.clone(),
        description: "docs".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate: SwarmAggregate::from_states([AgentLifecycleState::Running]),
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "docs".to_owned(),
            agent: child,
        }],
    };
    let events = [
        AgentEvent::DelegateSwarmStarted {
            turn: 6,
            swarm: started,
            workflow_origin: None,
        },
        AgentEvent::DelegateSwarmProgressUpdated {
            turn: 6,
            swarm_id: swarm_id.clone(),
            state: AgentLifecycleState::Completed,
            aggregate: SwarmAggregate::from_states([AgentLifecycleState::Completed]),
            child_progress: SwarmChildProgress {
                item_index: 0,
                progress: completed.progress_snapshot(),
            },
            workflow_origin: None,
        },
    ];

    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay(events.iter());

    let swarm = restored.swarm_snapshot(&swarm_id).expect("restored swarm");
    assert_eq!(swarm.state, AgentLifecycleState::Completed);
    assert_eq!(swarm.children[0].item_index, 0);
    assert_eq!(swarm.children[0].agent.tool_count, 4);
    assert_eq!(
        swarm.children[0].agent.state,
        AgentLifecycleState::Completed
    );
}

#[test]
fn compact_progress_preserves_live_shell_output() {
    let runtime = MultiAgentRuntime::new();
    let started = runtime.start_foreground_delegate_for_test("run tests");
    let started_at = std::time::Instant::now();
    let _ = runtime.apply_child_event(
        &started.id,
        started_at,
        &AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "bash-live".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"command": "cargo test"}),
            workflow_origin: None,
            output_ref: None,
        },
    );
    let progress = runtime
        .apply_child_event(
            &started.id,
            started_at,
            &AgentEvent::ToolExecutionUpdate {
                turn: 1,
                id: "bash-live".to_owned(),
                name: "Bash".to_owned(),
                partial_result: ToolResult::ok("Compiling neo"),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("live progress");

    let mut projected = started;
    assert!(apply_agent_progress(&mut projected, &progress));
    let output = projected
        .activity
        .iter()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool { output, .. } => output.as_ref(),
            AgentActivityKind::Text { .. } => None,
        });
    assert_eq!(
        output.map(|preview| preview.text.as_str()),
        Some("Compiling neo")
    );
    assert!(output.is_some_and(|preview| preview.tail));
}

#[test]
fn older_terminal_progress_cannot_clear_a_newer_outcome() {
    let runtime = MultiAgentRuntime::new();
    let started = runtime.start_foreground_delegate_for_test("preserve result");
    let mut current = runtime.complete_delegate_for_test(&started.id, "complete result");
    let mut older = current.progress_snapshot();
    current.updated_at_ms = current.updated_at_ms.saturating_add(1);
    older.outcome = None;
    let expected = current.clone();

    assert!(!apply_agent_progress(&mut current, &older));
    assert_eq!(current, expected);
}

#[test]
fn child_activity_trim_preserves_visible_ongoing_tool_and_latest_text() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("long running bash");
    let started_at = std::time::Instant::now();

    let _ = runtime.apply_child_event(
        &snapshot.id,
        started_at,
        &AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "bash-live".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"cmd": "cargo nextest run -p neo-tui --test multi_agent_transcript"}),
            workflow_origin: None,
            output_ref: None,
        },
    );
    for index in 0..32 {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ThinkingDelta {
                turn: 1,
                text: format!("thinking chunk {index}"),
            },
        );
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::TextDelta {
                turn: 1,
                text: format!("body chunk {index}"),
            },
        );
    }

    let updated = runtime
        .snapshot(&snapshot.id)
        .expect("snapshot remains present");
    assert_eq!(updated.activity.len(), 24);
    assert_eq!(
        latest_tool_phase(&updated, "bash-live"),
        Some(AgentToolActivityPhase::Ongoing)
    );
    let latest_thinking = updated
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Text { text, thinking } if *thinking => Some(text.as_str()),
            _ => None,
        });
    assert_eq!(latest_thinking, Some("thinking chunk 31"));
}

#[test]
fn child_text_and_thinking_deltas_accumulate_into_live_activity() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("stream text");
    let started_at = std::time::Instant::now();

    for text in ["All", " ", "edits", " ", "applied."] {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::TextDelta {
                turn: 1,
                text: text.to_owned(),
            },
        );
    }
    for text in ["Let", " ", "me", " ", "verify."] {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ThinkingDelta {
                turn: 1,
                text: text.to_owned(),
            },
        );
    }

    let updated = runtime
        .snapshot(&snapshot.id)
        .expect("snapshot remains present");
    assert_eq!(updated.latest_text.as_deref(), Some("All edits applied."));
    let latest_body = updated
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Text { text, thinking } if !thinking => Some(text.as_str()),
            _ => None,
        });
    let latest_thinking = updated
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Text { text, thinking } if *thinking => Some(text.as_str()),
            _ => None,
        });
    assert_eq!(latest_body, Some("All edits applied."));
    assert_eq!(latest_thinking, Some("Let me verify."));
}

#[test]
fn child_text_delta_accumulation_preserves_repeated_fragments() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("stream repeated text");
    let started_at = std::time::Instant::now();

    for text in ["ha", "ha", "!"] {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::TextDelta {
                turn: 1,
                text: text.to_owned(),
            },
        );
    }

    let updated = runtime
        .snapshot(&snapshot.id)
        .expect("snapshot remains present");
    assert_eq!(updated.latest_text.as_deref(), Some("haha!"));
    let latest_body = updated
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Text { text, thinking } if !thinking => Some(text.as_str()),
            _ => None,
        });
    assert_eq!(latest_body, Some("haha!"));
}

#[test]
fn retry_activity_stays_inside_child_snapshot() {
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("retry model stream");
    let started_at = std::time::Instant::now();

    runtime
        .apply_child_event(
            &child.id,
            started_at,
            &AgentEvent::ThinkingDelta {
                turn: 1,
                text: "failed reasoning".to_owned(),
            },
        )
        .expect("failed attempt thinking activity");
    runtime
        .apply_child_event(
            &child.id,
            started_at,
            &AgentEvent::TextDelta {
                turn: 1,
                text: "failed partial".to_owned(),
            },
        )
        .expect("failed attempt activity");
    runtime
        .apply_child_event(
            &child.id,
            started_at,
            &AgentEvent::RetryScheduled {
                turn: 1,
                retry: 1,
                max_retries: 5,
                delay_ms: 500,
                error_code: "provider.transport_error".to_owned(),
                message: "transport error: body closed".to_owned(),
            },
        )
        .expect("scheduled retry activity");

    let scheduled = runtime.snapshot(&child.id).expect("scheduled snapshot");
    assert_eq!(scheduled.latest_text.as_deref(), Some("Reconnecting 1/5"));
    assert_eq!(
        scheduled
            .activity
            .iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                AgentActivityKind::Text { text, thinking } if !thinking => Some(text.as_str()),
                _ => None,
            }),
        Some("Reconnecting 1/5")
    );
    assert!(scheduled.activity.iter().all(|entry| !matches!(
        &entry.kind,
        AgentActivityKind::Text { text, .. } if text.contains("failed partial")
    )));
    assert!(scheduled.activity.iter().all(|entry| !matches!(
        &entry.kind,
        AgentActivityKind::Text { text, thinking: true } if !text.is_empty()
    )));
    runtime
        .apply_child_event(
            &child.id,
            started_at,
            &AgentEvent::RetryResumed { turn: 1, retry: 1 },
        )
        .expect("resumed retry clears activity");

    let resumed = runtime.snapshot(&child.id).expect("resumed snapshot");
    assert_eq!(resumed.latest_text, None);
    assert!(resumed.activity.iter().all(|entry| !matches!(
        &entry.kind,
        AgentActivityKind::Text { text, .. } if text.starts_with("Reconnecting ")
    )));

    runtime
        .apply_child_event(
            &child.id,
            started_at,
            &AgentEvent::TextDelta {
                turn: 1,
                text: "winning answer".to_owned(),
            },
        )
        .expect("winning activity");
    let winning = runtime.snapshot(&child.id).expect("winning snapshot");
    assert_eq!(winning.latest_text.as_deref(), Some("winning answer"));
    assert_eq!(
        winning
            .activity
            .iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                AgentActivityKind::Text { text, thinking } if !thinking => Some(text.as_str()),
                _ => None,
            }),
        Some("winning answer")
    );
}

#[tokio::test]
async fn child_activity_keeps_same_name_tool_failures_on_their_own_ids() {
    let runtime = MultiAgentRuntime::new();
    let agent = runtime.start_delegate(
        "same tool ids",
        None,
        neo_agent_core::multi_agent::AgentRole::Coder,
        neo_agent_core::multi_agent::AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    let started_at = std::time::Instant::now();

    for (id, path) in [("read_ok", "ok.rs"), ("read_fail", "missing.rs")] {
        let _ = runtime.apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: id.to_owned(),
                name: "Read".to_owned(),
                arguments: json!({ "path": path }),
                workflow_origin: None,
                output_ref: None,
            },
        );
    }
    let _ = runtime.apply_child_event(
        &agent.id,
        started_at,
        &AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "read_fail".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::error("missing file"),
            workflow_origin: None,
            output_ref: None,
        },
    );

    let snapshot = runtime.snapshot(&agent.id).expect("agent snapshot");
    let tools = snapshot
        .activity
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                id, summary, phase, ..
            } => Some((id.as_str(), summary.as_deref(), *phase)),
            AgentActivityKind::Text { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tools,
        vec![
            ("read_ok", Some("ok.rs"), AgentToolActivityPhase::Ongoing),
            (
                "read_fail",
                Some("missing.rs"),
                AgentToolActivityPhase::Failed
            )
        ]
    );
}

#[test]
fn child_activity_projects_edit_write_file_rows() {
    let runtime = MultiAgentRuntime::new();
    let agent = runtime.start_foreground_delegate_for_test("edit files");
    let started_at = std::time::Instant::now();

    runtime
        .apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: "edit-1".to_owned(),
                name: "Edit".to_owned(),
                arguments: json!({ "path": "src/a.rs", "old": "a", "new": "b" }),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("Edit start update");
    let running = runtime.snapshot(&agent.id).expect("running snapshot");
    let running_files = latest_tool_files(&running, "edit-1");
    assert_eq!(
        running_files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["src/a.rs"]
    );
    assert!(running_files.iter().all(|file| {
        file.operation == Some(AgentToolFileOperation::Edited)
            && file.status == AgentToolFileStatus::Pending
    }));

    runtime
        .apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: "edit-1".to_owned(),
                name: "Edit".to_owned(),
                result: ToolResult::ok("edited 1 files").with_details(json!({
                    "kind": "edit",
                    "status": "committed",
                    "files": 1,
                    "added": 5,
                    "removed": 1,
                    "changes": [
                        { "path": "src/a.rs", "status": "committed", "added": 5, "removed": 1 }
                    ]
                })),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("Edit finish update");
    let edited = runtime.snapshot(&agent.id).expect("edited snapshot");
    let edited_files = latest_tool_files(&edited, "edit-1");
    assert_eq!(edited_files[0].added, Some(5));
    assert!(
        edited_files
            .iter()
            .all(|file| file.status == AgentToolFileStatus::Committed)
    );

    runtime
        .apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: "write-1".to_owned(),
                name: "Write".to_owned(),
                arguments: json!({ "path": "docs/new.md", "content": "new" }),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("Write start update");
    runtime
        .apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionUpdate {
                turn: 1,
                id: "write-1".to_owned(),
                name: "Write".to_owned(),
                partial_result: ToolResult::ok("prepared Write").with_details(json!({
                    "kind": "write_prepared",
                    "files": 1,
                    "changes": [
                        { "path": "docs/new.md", "operation": "created", "line_count": 4, "added": 4, "removed": 0 }
                    ]
                })),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("Write prepared update");
    runtime
        .apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionUpdate {
                turn: 1,
                id: "write-1".to_owned(),
                name: "Write".to_owned(),
                partial_result: ToolResult::ok("committed 1/1").with_details(json!({
                    "kind": "write_progress",
                    "committed": 1,
                    "total": 1,
                    "latest_path": "docs/new.md"
                })),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("Write progress update");
    let progress = runtime
        .snapshot(&agent.id)
        .expect("prepared snapshot")
        .progress_snapshot();
    assert_eq!(
        progress.last_tool.as_ref().map(|tool| tool.files.len()),
        Some(1)
    );

    runtime
        .apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: "write-1".to_owned(),
                name: "Write".to_owned(),
                result: ToolResult::ok("wrote 1 files").with_details(json!({
                    "kind": "write",
                    "status": "committed",
                    "files": 1,
                    "changes": [
                        { "path": "docs/new.md", "operation": "created", "status": "committed", "line_count": 4, "added": 4, "removed": 0 }
                    ]
                })),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("Write finish update");
    let written = runtime.snapshot(&agent.id).expect("written snapshot");
    let files = latest_tool_files(&written, "write-1");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].operation, Some(AgentToolFileOperation::Created));
    assert_eq!(files[0].status, AgentToolFileStatus::Committed);
    assert_eq!(files[0].line_count, Some(4));
}

#[test]
fn child_tool_events_preserve_ongoing_done_and_failed_phase() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_delegate(
        "run tests",
        Some("Run tests"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::Root,
    );
    let started_at = std::time::Instant::now();

    runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionStarted {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                arguments: json!({ "command": "cargo nextest run -p neo-tui" }),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("started update");
    let started = runtime.snapshot(&snapshot.id).expect("started snapshot");

    let tool = started
        .activity
        .iter()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                phase,
                summary,
                output,
                ..
            } => Some((*phase, summary.clone(), output.clone())),
            AgentActivityKind::Text { .. } => None,
        })
        .expect("tool row");

    assert_eq!(tool.0, AgentToolActivityPhase::Ongoing);
    assert_eq!(tool.1.as_deref(), Some("cargo nextest run -p neo-tui"));
    assert!(tool.2.is_none());

    runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionUpdate {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                partial_result: ToolResult::ok("Compiling neo-tui v0.1.0"),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("live output update");
    let updated = runtime.snapshot(&snapshot.id).expect("updated snapshot");
    let output = latest_tool_output(&updated, "call_bash").expect("output preview");
    assert!(updated.activity.iter().any(|entry| matches!(
        &entry.kind,
        AgentActivityKind::Tool { id, summary, .. }
            if id == "call_bash" && summary.as_deref() == Some("cargo nextest run -p neo-tui")
    )));
    assert!(output.text.contains("Compiling neo-tui"));
    assert!(output.tail);

    runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionFinished {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                result: ToolResult::ok("Finished test profile"),
                workflow_origin: None,
                output_ref: None,
            },
        )
        .expect("finished update");
    let finished = runtime.snapshot(&snapshot.id).expect("finished snapshot");
    assert_eq!(
        latest_tool_phase(&finished, "call_bash"),
        Some(AgentToolActivityPhase::Done)
    );
    assert!(finished.activity.iter().any(|entry| matches!(
        &entry.kind,
        AgentActivityKind::Tool { id, summary, .. }
            if id == "call_bash" && summary.as_deref() == Some("cargo nextest run -p neo-tui")
    )));
    assert!(
        latest_tool_output(&finished, "call_bash")
            .expect("final output preview")
            .text
            .contains("Finished test profile")
    );
    assert_eq!(finished.tool_count, 1);
}

#[test]
fn child_shell_activity_keeps_command_and_output_with_or_without_queue() {
    for starts_queued in [false, true] {
        let runtime = MultiAgentRuntime::new();
        let child = runtime.start_foreground_delegate_for_test("run tests");
        let started_at = std::time::Instant::now();
        let mut events = Vec::new();
        if starts_queued {
            events.extend([
                AgentEvent::ToolExecutionQueued {
                    turn: 1,
                    id: "call-1".to_owned(),
                    name: "Bash".to_owned(),
                    arguments: json!({"command": "cargo test"}),
                    workflow_origin: None,
                },
                AgentEvent::ToolExecutionQueueUpdated {
                    turn: 1,
                    id: "call-1".to_owned(),
                    position: 2,
                    waiting_ms: 18_000,
                },
            ]);
        }
        events.extend([
            AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: "call-1".to_owned(),
                name: "Bash".to_owned(),
                arguments: json!({"command": "cargo test"}),
                workflow_origin: None,
                output_ref: None,
            },
            AgentEvent::ToolExecutionUpdate {
                turn: 1,
                id: "call-1".to_owned(),
                name: "Bash".to_owned(),
                partial_result: ToolResult::ok("test output"),
                workflow_origin: None,
                output_ref: None,
            },
            AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: "call-1".to_owned(),
                name: "Bash".to_owned(),
                result: ToolResult::ok("done"),
                workflow_origin: None,
                output_ref: None,
            },
        ]);
        for event in events {
            let _ = runtime.apply_child_event(&child.id, started_at, &event);
        }
        let snapshot = runtime
            .agent_snapshot(child.id.as_str())
            .expect("child snapshot");
        let tools = snapshot
            .activity
            .iter()
            .filter_map(|entry| match &entry.kind {
                AgentActivityKind::Tool {
                    summary,
                    phase,
                    output,
                    ..
                } => Some((summary, phase, output)),
                AgentActivityKind::Text { .. } => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].0.as_deref(), Some("cargo test"));
        assert_eq!(*tools[0].1, AgentToolActivityPhase::Done);
        assert_eq!(tools[0].2.as_ref().map(|o| o.text.as_str()), Some("done"));
    }
}

fn latest_tool_phase(
    snapshot: &neo_agent_core::multi_agent::AgentSnapshot,
    id: &str,
) -> Option<AgentToolActivityPhase> {
    snapshot
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                id: entry_id,
                phase,
                ..
            } if entry_id == id => Some(*phase),
            _ => None,
        })
}

fn latest_tool_output(
    snapshot: &neo_agent_core::multi_agent::AgentSnapshot,
    id: &str,
) -> Option<AgentToolOutputPreview> {
    snapshot
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                id: entry_id,
                output,
                ..
            } if entry_id == id => output.clone(),
            _ => None,
        })
}

fn latest_tool_files(
    snapshot: &neo_agent_core::multi_agent::AgentSnapshot,
    id: &str,
) -> Vec<AgentToolFileChange> {
    snapshot
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                id: entry_id,
                files,
                ..
            } if entry_id == id => Some(files.clone()),
            _ => None,
        })
        .unwrap_or_default()
}
