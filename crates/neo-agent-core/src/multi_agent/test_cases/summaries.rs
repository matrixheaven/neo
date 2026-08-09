use super::*;
use crate::ToolResult;

#[test]
fn shell_tool_summary_preserves_head_and_tail_within_budget() {
    let command = format!(
        "cargo test --package neo-agent-core --lib {} --exact --nocapture",
        "multi_agent::runtime::tests::very_long_filter_".repeat(4)
    );
    for (name, arguments) in [
        ("Bash", serde_json::json!({"command": command.clone()})),
        (
            "Terminal",
            serde_json::json!({"mode": "start", "command": command.clone()}),
        ),
    ] {
        let summary = summarize_tool_arguments(name, &arguments).expect("shell summary");
        assert_eq!(summary.chars().count(), 96, "{summary}");
        assert!(
            summary.starts_with("cargo test --package neo-agent-core"),
            "{summary}"
        );
        assert!(summary.contains(" … "), "{summary}");
        assert!(summary.ends_with("--exact --nocapture"), "{summary}");
    }

    let summary = summarize_tool_arguments(
        "Terminal",
        &serde_json::json!({"mode": "write", "command": command}),
    )
    .expect("terminal write summary");
    assert_eq!(summary.chars().count(), 96, "{summary}");
    assert!(summary.ends_with("..."), "{summary}");
    assert!(!summary.contains(" … "), "{summary}");

    let path = format!("/workspace/{}", "very-long-path-segment/".repeat(8));
    let summary =
        summarize_tool_arguments("Read", &serde_json::json!({"path": path})).expect("read summary");
    assert_eq!(summary.chars().count(), 96, "{summary}");
    assert!(
        summary.starts_with("/workspace/very-long-path"),
        "{summary}"
    );
    assert!(summary.ends_with("..."), "{summary}");
    assert!(!summary.contains('…'), "{summary}");

    let summary = summarize_tool_arguments(
        "Bash",
        &serde_json::json!({"command": "printf\t'audit'\x1b[31m danger\x03"}),
    )
    .expect("unsafe bash summary");
    assert!(summary.contains("printf 'audit'"), "{summary}");
    assert!(summary.contains(r"\u{1b}[31m danger\u{3}"), "{summary}");
    assert!(
        summary.chars().all(|character| !character.is_control()),
        "{summary:?}"
    );
}

#[test]
fn edit_tool_summary_shows_single_path_within_budget() {
    let summary = summarize_tool_arguments(
        "Edit",
        &serde_json::json!({
            "path": "src/a.rs", "old": "a", "new": "A"
        }),
    )
    .expect("edit summary");
    assert!(summary.contains("1 files"), "{summary}");
    assert!(summary.contains("1 replacements"), "{summary}");
    assert!(summary.contains("src/a.rs"), "{summary}");
    assert!(summary.chars().count() <= 160, "{summary}");
}

#[test]
fn edit_tool_summary_prefers_structured_partial_progress() {
    let mut activity = Vec::new();
    let mut tool_args = HashMap::new();
    let started = AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "e1".to_owned(),
        name: "Edit".to_owned(),
        arguments: serde_json::json!({"path":"a.rs","old":"a","new":"A"}),
        workflow_origin: None,
        output_ref: None,
    };
    assert!(apply_tool_activity_event(
        &mut activity,
        &mut tool_args,
        &started
    ));
    let update = AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "e1".to_owned(),
        name: "Edit".to_owned(),
        partial_result: ToolResult::ok("progress").with_details(serde_json::json!({
            "kind": "edit_progress",
            "committed": 2,
            "total": 5,
            "latest_path": "src/lib.rs",
            "added": 9,
            "removed": 4
        })),
        workflow_origin: None,
        output_ref: None,
    };
    assert!(apply_tool_activity_event(
        &mut activity,
        &mut tool_args,
        &update
    ));
    let summary = last_tool_summary(&activity, "e1").expect("summary");
    assert!(summary.contains("committing 2/5"), "{summary}");
    assert!(summary.contains("src/lib.rs"), "{summary}");
}

#[test]
fn live_edit_summary_uses_structured_progress_and_terminal_partial() {
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("edit files");
    let started_at = Instant::now();
    let progress = AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "e-live".to_owned(),
        name: "Edit".to_owned(),
        partial_result: ToolResult::ok("progress").with_details(serde_json::json!({
            "kind": "edit_progress",
            "committed": 1,
            "total": 3,
            "latest_path": "a.rs",
            "added": 1,
            "removed": 1
        })),
        workflow_origin: None,
        output_ref: None,
    };
    runtime
        .apply_child_event(&child.id, started_at, &progress)
        .expect("progress update");
    let progress_snapshot = runtime.agent_snapshot(child.id.as_str()).expect("snapshot");
    let progress_summary =
        last_tool_summary(&progress_snapshot.activity, "e-live").expect("progress summary");
    assert!(
        progress_summary.contains("committing 1/3"),
        "{progress_summary}"
    );

    let finished = AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "e-live".to_owned(),
        name: "Edit".to_owned(),
        result: ToolResult::error("partial").with_details(serde_json::json!({
            "kind": "edit",
            "status": "partial_commit",
            "files": 3,
            "replacements": 3,
            "added": 1,
            "removed": 1,
            "changes": [
                {"path":"a.rs","status":"committed"},
                {"path": format!("{}b.rs", "very-long-path/".repeat(20)), "status":"failed"},
                {"path":"c.rs","status":"not_attempted"}
            ]
        })),
        workflow_origin: None,
        output_ref: None,
    };
    runtime
        .apply_child_event(&child.id, started_at, &finished)
        .expect("finished update");
    let finished_snapshot = runtime.agent_snapshot(child.id.as_str()).expect("snapshot");
    let finished_summary =
        last_tool_summary(&finished_snapshot.activity, "e-live").expect("finished summary");
    assert!(
        finished_summary.contains("partial · 1/3 committed"),
        "{finished_summary}"
    );
    assert!(finished_summary.contains("failed at"), "{finished_summary}");
    assert!(
        finished_summary.chars().count() <= 160,
        "{finished_summary}"
    );
}

#[test]
fn replayed_unfinished_edit_is_interrupted_and_not_resumed() {
    // Replay projects unfinished progress as a terminal interrupted card
    // without re-submitting PreparedEdit to runtime. Activity summary alone
    // never starts commit.
    let mut activity = Vec::new();
    let mut tool_args = HashMap::new();
    let update = AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "e2".to_owned(),
        name: "Edit".to_owned(),
        partial_result: ToolResult::ok("progress").with_details(serde_json::json!({
            "kind": "edit_progress",
            "committed": 1,
            "total": 3,
            "latest_path": "a.rs",
            "added": 1,
            "removed": 1
        })),
        workflow_origin: None,
        output_ref: None,
    };
    assert!(apply_tool_activity_event(
        &mut activity,
        &mut tool_args,
        &update
    ));
    assert!(
        activity.iter().all(|entry| match &entry.kind {
            AgentActivityKind::Tool { phase, .. } => *phase == AgentToolActivityPhase::Ongoing,
            AgentActivityKind::Text { .. } | AgentActivityKind::Instruction { .. } => true,
        }),
        "progress alone must not invent a completed commit"
    );
    // No execution attempt is recorded beyond the projected activity entry.
    assert_eq!(tool_args.len(), 0);
}

#[test]
fn write_tool_summary_prefers_structured_progress_and_terminal_partial() {
    let mut activity = Vec::new();
    let mut tool_args = HashMap::new();
    let started = AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "w1".to_owned(),
        name: "Write".to_owned(),
        arguments: serde_json::json!({
            "path": "src/a.rs", "content": "fn main() {}"
        }),
        workflow_origin: None,
        output_ref: None,
    };
    assert!(apply_tool_activity_event(
        &mut activity,
        &mut tool_args,
        &started
    ));
    // Raw argument summary is available before structured details arrive.
    let raw_summary = last_tool_summary(&activity, "w1").expect("raw summary");
    assert!(raw_summary.contains("Write 1 files"), "{raw_summary}");

    // Structured progress overrides raw argument summary.
    let update = AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "w1".to_owned(),
        name: "Write".to_owned(),
        partial_result: ToolResult::ok("progress").with_details(serde_json::json!({
            "kind": "write_progress",
            "committed": 1,
            "total": 2,
            "latest_path": "src/a.rs",
            "latest_operation": "created",
            "added": 1,
            "removed": 0
        })),
        workflow_origin: None,
        output_ref: None,
    };
    assert!(apply_tool_activity_event(
        &mut activity,
        &mut tool_args,
        &update
    ));
    let progress_summary = last_tool_summary(&activity, "w1").expect("progress summary");
    assert!(
        progress_summary.contains("committing 1/2"),
        "{progress_summary}"
    );
    assert!(progress_summary.contains("src/a.rs"), "{progress_summary}");

    // Terminal partial_commit overrides both raw and progress summaries.
    let finished = AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "w1".to_owned(),
        name: "Write".to_owned(),
        result: ToolResult::error("partial").with_details(serde_json::json!({
            "kind": "write",
            "status": "partial_commit",
            "files": 2,
            "created": 1,
            "overwritten": 1,
            "added": 1,
            "removed": 0,
            "changes": [
                {"path": "src/a.rs", "status": "committed"},
                {"path": "src/b.rs", "status": "not_attempted"}
            ]
        })),
        workflow_origin: None,
        output_ref: None,
    };
    assert!(apply_tool_activity_event(
        &mut activity,
        &mut tool_args,
        &finished
    ));
    let final_summary = last_tool_summary(&activity, "w1").expect("final summary");
    assert!(final_summary.contains("partial 1/2"), "{final_summary}");
    assert!(final_summary.contains("+1 -0"), "{final_summary}");
}

#[test]
fn live_write_summary_is_bounded_without_content() {
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("write files");
    let started_at = Instant::now();

    // Large file contents in arguments must not leak into the summary.
    let large_content = "x".repeat(10_000);
    let started = AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "w-live".to_owned(),
        name: "Write".to_owned(),
        arguments: serde_json::json!({
            "path": "src/first.rs", "content": large_content
        }),
        workflow_origin: None,
        output_ref: None,
    };
    runtime
        .apply_child_event(&child.id, started_at, &started)
        .expect("started update");
    let snapshot = runtime.agent_snapshot(child.id.as_str()).expect("snapshot");
    let summary = last_tool_summary(&snapshot.activity, "w-live").expect("summary");

    // Summary must not contain any file content.
    assert!(!summary.contains("xxx"), "{summary}");
    assert!(summary.contains("Write 1 files"), "{summary}");
    assert!(summary.contains("src/first.rs"), "{summary}");
    // Summary must be within the 160-char bound.
    assert!(summary.chars().count() <= 160, "{summary}");

    // Structured committed result also stays bounded.
    let finished = AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "w-live".to_owned(),
        name: "Write".to_owned(),
        result: ToolResult::ok("wrote 3 files").with_details(serde_json::json!({
            "kind": "write",
            "status": "committed",
            "files": 3,
            "created": 2,
            "overwritten": 1,
            "added": 300,
            "removed": 50,
            "changes": [
                {"path": "src/first.rs", "status": "committed"},
                {"path": "src/middle.rs", "status": "committed"},
                {"path": "src/last.rs", "status": "committed"}
            ]
        })),
        workflow_origin: None,
        output_ref: None,
    };
    runtime
        .apply_child_event(&child.id, started_at, &finished)
        .expect("finished update");
    let final_snapshot = runtime
        .agent_snapshot(child.id.as_str())
        .expect("final snapshot");
    let final_summary =
        last_tool_summary(&final_snapshot.activity, "w-live").expect("final summary");
    assert!(final_summary.contains("wrote 3 files"), "{final_summary}");
    assert!(final_summary.contains("2 created"), "{final_summary}");
    assert!(final_summary.chars().count() <= 160, "{final_summary}");
}

#[test]
fn replayed_unfinished_write_is_interrupted_and_not_resumed() {
    // Replay projects unfinished Write progress as a terminal interrupted card
    // without re-submitting PreparedWrite to runtime. Activity summary alone
    // never starts commit.
    let mut activity = Vec::new();
    let mut tool_args = HashMap::new();
    let update = AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "w2".to_owned(),
        name: "Write".to_owned(),
        partial_result: ToolResult::ok("progress").with_details(serde_json::json!({
            "kind": "write_progress",
            "committed": 1,
            "total": 3,
            "latest_path": "src/lib.rs",
            "latest_operation": "overwritten",
            "added": 10,
            "removed": 5
        })),
        workflow_origin: None,
        output_ref: None,
    };
    assert!(apply_tool_activity_event(
        &mut activity,
        &mut tool_args,
        &update
    ));
    assert!(
        activity.iter().all(|entry| match &entry.kind {
            AgentActivityKind::Tool { phase, .. } => *phase == AgentToolActivityPhase::Ongoing,
            AgentActivityKind::Text { .. } | AgentActivityKind::Instruction { .. } => true,
        }),
        "progress alone must not invent a completed commit"
    );
    // No execution attempt is recorded beyond the projected activity entry.
    assert_eq!(tool_args.len(), 0);
    // Summary reflects the interrupted progress, not a terminal state.
    let summary = last_tool_summary(&activity, "w2").expect("summary");
    assert!(summary.contains("committing 1/3"), "{summary}");
}

#[test]
fn summarized_child_activity_preserves_whitespace_deltas_without_duplicate_body() {
    let events = [
        AgentEvent::TextDelta {
            turn: 1,
            text: "All".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: " ".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "edits applied.".to_owned(),
        },
        AgentEvent::ThinkingDelta {
            turn: 1,
            text: "Let".to_owned(),
        },
        AgentEvent::ThinkingDelta {
            turn: 1,
            text: " ".to_owned(),
        },
        AgentEvent::ThinkingDelta {
            turn: 1,
            text: "me verify.".to_owned(),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("All edits applied.")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        },
    ];

    let activity = summarize_child_activity(&events);
    let body = latest_text_activity(&activity, false);
    let thinking = latest_text_activity(&activity, true);

    assert_eq!(body.as_deref(), Some("All edits applied."));
    assert_eq!(thinking.as_deref(), Some("Let me verify."));
}

#[test]
fn summarized_retry_activity_keeps_only_winning_attempt() {
    let events = [
        AgentEvent::TextDelta {
            turn: 1,
            text: "prior answer".to_owned(),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("prior answer")],
                Vec::new(),
                StopReason::ToolUse,
            ),
        },
        AgentEvent::ThinkingDelta {
            turn: 1,
            text: "failed reasoning one".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "failed partial one".to_owned(),
        },
        AgentEvent::ThinkingDelta {
            turn: 1,
            text: "failed reasoning two".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "failed partial two".to_owned(),
        },
        AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 5,
            delay_ms: 500,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: body closed".to_owned(),
        },
        AgentEvent::RetryStarted {
            turn: 1,
            retry: 1,
            max_retries: 5,
        },
        AgentEvent::RetryResumed { turn: 1, retry: 1 },
        AgentEvent::TextDelta {
            turn: 1,
            text: "winning answer".to_owned(),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("winning answer")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        },
    ];

    let activity = summarize_child_activity(&events);

    assert_eq!(
        latest_text_activity(&activity, false).as_deref(),
        Some("winning answer")
    );
    assert_eq!(latest_text_activity(&activity, true), None);
    assert!(activity.iter().any(|entry| matches!(
        &entry.kind,
        AgentActivityKind::Text { text, thinking: false } if text == "prior answer"
    )));
    assert!(activity.iter().all(|entry| !matches!(
        &entry.kind,
        AgentActivityKind::Text { text, .. }
            if text.contains("failed") || text.starts_with("Reconnecting ")
    )));
}
