//! Interactive approvals behavior (moved from `tests.rs`).

use std::path::PathBuf;

use neo_agent_core::{
    AgentEvent, AgentMessage, ApprovalAction, ApprovalCancelReason, ApprovalResolution,
    ApprovalResponse, Content, PermissionMode, PrefixApprovalRule, StopReason, ToolResult,
};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    transcript::{ApprovalDisplayState, MouseKind, TranscriptEntry, TranscriptPane},
};

use super::super::*;
use super::*;

#[tokio::test]
async fn approval_number_shortcut_confirms_session_approval() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let scope = file_write_session_scope("approved.txt");
    let (pending, response_rx) = make_pending_approval(ordinary_tool_request(
        "tool-1",
        "Write",
        "approved.txt",
        Some(scope),
    ));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Insert('2'))
        .await
        .expect("number shortcut handles approval");

    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitForSession { .. },
            ..
        }
    ));
    assert!(!controller.chrome().approval_is_pending());
    assert!(
        controller
            .render_snapshot()
            .contains("Approve writes to this file for this session")
            || controller
                .render_snapshot()
                .to_lowercase()
                .contains("approve")
    );
}

#[tokio::test]
async fn prefix_approval_choice_dispatches_prefix_decision() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let scope = shell_session_scope(&["cargo", "test"]);
    let rule = PrefixApprovalRule {
        prefix: vec!["cargo".to_owned(), "test".to_owned()],
        label: "cargo test".to_owned(),
    };
    let (pending, response_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "cargo test",
        Some(scope),
        Some(rule),
    ));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Insert('3'))
        .await
        .expect("number shortcut handles prefix approval");

    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitForPrefix { .. },
            ..
        }
    ));
    assert!(
        controller
            .render_snapshot()
            .contains("Approve commands starting with cargo test")
            || controller.render_snapshot().contains("cargo test")
    );
}

#[tokio::test]
async fn approval_uses_selection_priority_for_real_keys() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("draft");
    let scope = file_write_session_scope("approved.txt");
    let (pending, response_rx) = make_pending_approval(ordinary_tool_request(
        "tool-1",
        "Write",
        "approved.txt",
        Some(scope),
    ));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down selects approval option");
    assert!(matches!(
        controller.chrome().approval_selected_action(),
        Some(ApprovalAction::PermitForSession { .. })
    ));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("enter confirms approval");

    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitForSession { .. },
            ..
        }
    ));
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert!(!controller.chrome().approval_is_pending());
}

#[tokio::test]
async fn approval_mouse_wheel_scrolls_transcript_without_moving_selection() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    for index in 0..30 {
        controller
            .transcript_mut()
            .push_status(format!("approval-scroll-row-{index}"));
    }
    // Establish the viewport height through a bounded slice render.
    let _ = controller.transcript_mut().render_visible_slice(80, 6);
    let (pending, _response_rx) = make_pending_approval(ordinary_tool_request(
        "tool-1",
        "Write",
        "approved.txt",
        Some(file_write_session_scope("approved.txt")),
    ));
    controller.register_pending_approval(pending);
    let selected = controller.chrome().approval_selected_action().cloned();

    controller
        .handle_input_event(wheel_event(MouseKind::ScrollUp))
        .await
        .expect("wheel scrolls transcript while approval stays focused");

    assert!(transcript_view_locked(&controller));
    assert_eq!(
        controller.chrome().approval_selected_action(),
        selected.as_ref()
    );
}

#[tokio::test]
async fn approval_revise_collects_feedback_without_editing_prompt() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("draft");
    let (pending, response_rx) = make_pending_approval(plan_review_request("tool-1"));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down selects revise option");
    assert!(matches!(
        controller.chrome().approval_selected_action(),
        Some(ApprovalAction::RevisePlan { .. })
    ));

    // First Enter enters feedback collection mode.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("enter begins feedback collection");

    controller
        .handle_input_event(InputEvent::Insert('n'))
        .await
        .expect("typed feedback is captured by approval dialog");
    controller
        .handle_input_event(InputEvent::Paste("o thanks".to_owned()))
        .await
        .expect("pasted feedback is captured by approval dialog");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
        .await
        .expect("backspace edits approval feedback");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("enter confirms revise");

    assert_eq!(controller.chrome().prompt().text, "draft");
    match response_rx.await.expect("approval response") {
        ApprovalResponse::Selected {
            action: ApprovalAction::RevisePlan { .. },
            feedback: Some(feedback),
            ..
        } => assert_eq!(feedback, "no thank"),
        other => panic!("expected revise response, got {other:?}"),
    }
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("Revision feedback: no thank"),
        "feedback should be surfaced after resolve: {snapshot}"
    );
}

#[tokio::test]
async fn approval_cancel_rejects_pending_approval() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (pending, response_rx) =
        make_pending_approval(ordinary_tool_request("tool-1", "Write", "denied.txt", None));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel rejects approval");

    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Cancelled {
            reason: ApprovalCancelReason::Escape,
            ..
        }
    ));
    let snapshot = controller.render_snapshot().to_lowercase();
    assert!(
        snapshot.contains("cancel") || snapshot.contains("reject"),
        "snapshot should show cancelled/rejected approval"
    );
}

#[tokio::test]
async fn approval_requests_are_handled_one_at_a_time() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (first, first_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    let (second, _second_rx) =
        make_pending_approval(ordinary_shell_request("tool-2", "printf two", None, None));
    controller.register_pending_approval(first);
    controller.register_pending_approval(second);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("first approval confirms");

    assert!(matches!(
        first_rx.await.expect("first response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitOnce,
            ..
        }
    ));
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|(id, _, _, _)| id),
        Some("tool-2")
    );
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("printf two"));
}

#[tokio::test]
async fn approval_focus_owns_visible_window_until_resolved() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (first, _first_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    let (second, _second_rx) =
        make_pending_approval(ordinary_shell_request("tool-2", "printf two", None, None));
    controller.register_pending_approval(first);
    controller.register_pending_approval(second);

    // The full document snapshot keeps every approval card in arrival order.
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("printf one"));
    assert!(snapshot.contains("printf two"));
    assert!(!snapshot.contains("queued:"));
    assert!(
        snapshot.find("printf one").unwrap() < snapshot.find("printf two").unwrap(),
        "approval cards keep arrival order in the document:\n{snapshot}"
    );

    // The visible slice is confined to the earliest unresolved card's
    // window; the later card stays in the document but outside the slice
    // until the blocking entry resolves.
    let slice = controller.tui.transcript_mut().render_visible_slice(80, 24);
    let text = slice
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("printf one"), "slice:\n{text}");
    assert!(
        !text.contains("printf two"),
        "later approval must stay outside the visible window:\n{text}"
    );
    assert!(
        controller.tui.transcript().document().total_rows() > slice.len(),
        "later approval card remains in the document"
    );
    assert_eq!(
        controller.tui.transcript().earliest_blocking_entry(),
        Some(neo_tui::transcript::BlockingEntryKind::Approval(
            "tool-1".to_owned()
        ))
    );

    // Resolving the earliest approval advances the blocking focus to the
    // next card, which then owns the visible window.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("first approval confirms");
    let advanced = controller.tui.transcript_mut().render_visible_slice(80, 24);
    let text = advanced
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("printf two"), "slice:\n{text}");
    assert!(
        !text.contains("printf one"),
        "resolved card leaves the visible window:\n{text}"
    );
    assert_eq!(
        controller.tui.transcript().earliest_blocking_entry(),
        Some(neo_tui::transcript::BlockingEntryKind::Approval(
            "tool-2".to_owned()
        ))
    );
}

#[tokio::test]
async fn approval_cancel_advances_next_visible_request() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (first, first_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    let (second, _second_rx) =
        make_pending_approval(ordinary_shell_request("tool-2", "printf two", None, None));
    controller.register_pending_approval(first);
    controller.register_pending_approval(second);

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel rejects current approval");

    assert!(matches!(
        first_rx.await.expect("first response"),
        ApprovalResponse::Cancelled {
            reason: ApprovalCancelReason::Escape,
            ..
        }
    ));
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("printf two"));
    assert!(!snapshot.contains("queued:"));
}

#[tokio::test]
async fn approval_interrupt_cancels_all_pending_approvals() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (first, first_rx) =
        make_pending_approval(ordinary_shell_request("tool-1", "printf one", None, None));
    let (second, second_rx) =
        make_pending_approval(ordinary_shell_request("tool-2", "printf two", None, None));
    controller.register_pending_approval(first);
    controller.register_pending_approval(second);

    controller
        .handle_input_event(InputEvent::Interrupt)
        .await
        .expect("interrupt cancels pending approvals");

    assert!(matches!(
        first_rx.await.expect("first response"),
        ApprovalResponse::Cancelled {
            reason: ApprovalCancelReason::Interrupt,
            ..
        }
    ));
    assert!(matches!(
        second_rx.await.expect("second response"),
        ApprovalResponse::Cancelled {
            reason: ApprovalCancelReason::Interrupt,
            ..
        }
    ));
    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
}

#[test]
fn replay_exit_plan_mode_uses_only_persisted_snapshot_details() {
    // A live plan file must never become historical truth during replay.
    let temp = std::env::temp_dir().join(format!(
        "neo-plan-replay-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ));
    let plan_dir = temp.join("agents").join("main").join("plans");
    std::fs::create_dir_all(&plan_dir).expect("create plan dir");
    let plan_path = plan_dir.join("plan-1.md");
    std::fs::write(
        &plan_path,
        "# Live workspace plan\n\nDo not show this on replay.",
    )
    .expect("write live plan");
    let plan_path_text = plan_path.display().to_string();

    // Aggregate-only history: no ToolExecutionFinished details. Even with a
    // prior Write of the plan path and a live file on disk, only the header
    // may render.
    let mut aggregate_only = TranscriptPane::new(100, 24);
    let aggregate_loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new())
        .with_events([
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    [],
                    [neo_agent_core::AgentToolCall {
                        id: "write-1".into(),
                        name: "Write".into(),
                        raw_arguments: serde_json::json!({
                            "path": plan_path_text,
                            "content": "# Staged write content\n\nFabricated."
                        })
                        .to_string()
                        .into(),
                    }],
                    StopReason::ToolUse,
                ),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::tool_result(
                    "write-1",
                    "Write",
                    [Content::text("Wrote plan")],
                    false,
                ),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    [],
                    [neo_agent_core::AgentToolCall {
                        id: "exit-plan-1".into(),
                        name: "ExitPlanMode".into(),
                        raw_arguments: r#"{"plan_summary":"Ready"}"#.into(),
                    }],
                    StopReason::ToolUse,
                ),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::tool_result(
                    "exit-plan-1",
                    "ExitPlanMode",
                    [Content::text("Selected approach: Execute")],
                    false,
                ),
            },
        ]);
    replay_session_into_transcript(&mut aggregate_only, &aggregate_loaded);
    let aggregate_frame = aggregate_only
        .render_frame(100, 24)
        .expect("render aggregate-only exit plan")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        aggregate_frame.contains("Current plan"),
        "header must still render without details: {aggregate_frame}"
    );
    assert!(
        !aggregate_frame.contains("Live workspace plan"),
        "must not read live workspace plan file: {aggregate_frame}"
    );
    assert!(
        !aggregate_frame.contains("Staged write content"),
        "must not infer plan body from Write arguments: {aggregate_frame}"
    );
    assert!(
        !aggregate_frame.contains("plan: plan-1.md"),
        "plan box title must not appear without persisted details: {aggregate_frame}"
    );

    // Modern path: ToolExecutionFinished.result.details is the only durable
    // plan body source. Coverage skips the aggregate ToolResult message.
    let mut with_details = TranscriptPane::new(100, 24);
    let detailed_loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new())
        .with_events([
            AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: "exit-plan-2".to_owned(),
                name: "ExitPlanMode".to_owned(),
                arguments: serde_json::json!({"plan_summary": "Ready"}),
                workflow_origin: None,
                output_ref: None,
            },
            AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: "exit-plan-2".to_owned(),
                name: "ExitPlanMode".to_owned(),
                result: ToolResult::ok("Selected approach: Execute").with_details(
                    serde_json::json!({
                        "plan_content": "# Persisted snapshot plan\n\nShip only this body.",
                        "plan_path": plan_path_text,
                    }),
                ),
                workflow_origin: None,
                output_ref: None,
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::tool_result(
                    "exit-plan-2",
                    "ExitPlanMode",
                    [Content::text("Selected approach: Execute")],
                    false,
                ),
            },
        ]);
    replay_session_into_transcript(&mut with_details, &detailed_loaded);
    let detailed_frame = with_details
        .render_frame(100, 24)
        .expect("render detailed exit plan")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        detailed_frame.contains("Current plan"),
        "header must render with details: {detailed_frame}"
    );
    assert!(
        detailed_frame.contains("Persisted snapshot plan"),
        "must render plan body from persisted details: {detailed_frame}"
    );
    assert!(
        detailed_frame.contains("plan: plan-1.md"),
        "must render plan path basename from persisted details: {detailed_frame}"
    );
    assert!(
        !detailed_frame.contains("Live workspace plan"),
        "must not substitute live workspace content for persisted body: {detailed_frame}"
    );

    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn replay_renders_resolved_approval_without_reopening_it() {
    let request = replay_background_bash_request();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::ApprovalRequested {
            request: request.clone(),
        },
        AgentEvent::ApprovalResolved {
            turn: 1,
            request_id: request.id.clone(),
            resolution: ApprovalResolution::Selected {
                action: ApprovalAction::Reject,
                label: "Reject".to_owned(),
                feedback: None,
            },
        },
    ]);
    let mut transcript = TranscriptPane::new(80, 12);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Rejected"), "frame: {rendered}");
    assert!(
        transcript.transcript().entries().iter().all(|entry| {
            !matches!(
                entry,
                TranscriptEntry::ApprovalPrompt(data)
                    if matches!(data.state, ApprovalDisplayState::Pending)
            )
        }),
        "replay must not leave a pending approval card"
    );
}

#[test]
fn replay_marks_unresolved_approval_abandoned_without_reopening_it() {
    let request = replay_background_bash_request();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new())
        .with_events([AgentEvent::ApprovalRequested { request }]);
    let mut transcript = TranscriptPane::new(80, 12);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Abandoned"), "frame: {rendered}");
    assert!(
        transcript.transcript().entries().iter().all(|entry| {
            !matches!(
                entry,
                TranscriptEntry::ApprovalPrompt(data)
                    if matches!(data.state, ApprovalDisplayState::Pending)
            )
        }),
        "unresolved replay cards must be Abandoned, not Pending"
    );
}

#[test]
fn replay_preserves_all_unresolved_approval_cards_as_abandoned() {
    let first = replay_background_bash_request();
    let mut second = replay_background_bash_request();
    second.id = "background-bash-2".to_owned();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::ApprovalRequested { request: first },
        AgentEvent::ApprovalRequested { request: second },
    ]);
    let mut transcript = TranscriptPane::new(80, 12);

    replay_session_into_transcript(&mut transcript, &loaded);

    let approvals = transcript
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::ApprovalPrompt(data) => Some(data),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        approvals.len(),
        2,
        "every requested approval must survive replay"
    );
    assert!(
        approvals
            .iter()
            .all(|data| matches!(data.state, ApprovalDisplayState::Abandoned))
    );
}

#[tokio::test]
async fn revise_exit_plan_mode_feedback_is_forwarded_with_current_approval() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (pending, response_rx) = make_pending_approval(plan_review_request("exit-plan-1"));
    controller.register_pending_approval(pending);

    // Select "Reject with feedback" (index 1) and enter feedback, then confirm.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to revise");
    // First confirm enters feedback collection mode.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("begin feedback collection");
    controller
        .handle_input_event(InputEvent::Insert('r'))
        .await
        .expect("type feedback");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm revise");

    assert!(transcript_has_status(&controller, "Revision feedback: r"));
    match response_rx.await.expect("response") {
        ApprovalResponse::Selected {
            action: ApprovalAction::RevisePlan { .. },
            feedback: Some(feedback),
            ..
        } => assert_eq!(feedback, "r"),
        other => panic!("expected revise with feedback, got {other:?}"),
    }
}

#[tokio::test]
async fn approve_for_session_does_not_globally_skip_later_ask_prompt() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (first, first_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    controller.register_pending_approval(first);

    // Select "Approve for this session" (index 1) and confirm.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to always-approve");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm always-approve");

    assert!(matches!(
        first_rx.await.expect("first response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitForSession { .. },
            ..
        }
    ));

    // Tool-session approval is scoped by the runtime. The TUI must not
    // turn one approval into a global bypass for later ask prompts.
    let (second, mut second_rx) =
        make_pending_approval(ordinary_tool_request("tool-2", "Write", "later.txt", None));
    controller.register_pending_approval(second);
    assert!(
        second_rx.try_recv().is_err(),
        "later approval requests should remain pending in the TUI"
    );
    assert!(controller.pending_approvals.contains_key("tool-2"));
}

#[tokio::test]
async fn inactive_session_workflow_approval_is_backlogged_until_reactivated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);

    controller.set_active_session_id(SESSION_B.to_owned());
    let (pending, mut response_rx) = make_pending_approval(ordinary_shell_request(
        "session-a-approval",
        "sudo --version",
        None,
        None,
    ));
    controller
        .workflow_approval_ingress
        .send(SessionWorkflowApproval {
            session_id: SESSION_A.to_owned(),
            pending,
        })
        .expect("session A delivery");

    assert_eq!(controller.drain_workflow_approvals(), FrameRequest::None);
    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
    assert_eq!(controller.workflow_approval_backlog[SESSION_A].len(), 1);
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    controller.set_active_session_id(SESSION_A.to_owned());

    assert!(
        controller
            .pending_approvals
            .contains_key("session-a-approval")
    );
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|value| value.0),
        Some("session-a-approval")
    );
    assert!(!controller.workflow_approval_backlog.contains_key(SESSION_A));
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn registered_workflow_approval_is_parked_and_restored_across_session_navigation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    let (pending, mut response_rx) = make_pending_approval(ordinary_shell_request(
        "parked-approval",
        "sudo --version",
        None,
        None,
    ));
    controller
        .workflow_approval_ingress
        .send(SessionWorkflowApproval {
            session_id: SESSION_A.to_owned(),
            pending,
        })
        .expect("session A delivery");
    assert_eq!(
        controller.drain_workflow_approvals(),
        FrameRequest::Immediate
    );
    assert!(controller.chrome().approval_is_pending());

    controller.set_active_session_id(SESSION_B.to_owned());

    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
    assert_eq!(controller.workflow_approval_backlog[SESSION_A].len(), 1);
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    controller.set_active_session_id(SESSION_A.to_owned());

    assert!(controller.pending_approvals.contains_key("parked-approval"));
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|value| value.0),
        Some("parked-approval")
    );
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn idle_workflow_approval_uses_current_controller_route_without_model_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    controller.set_active_session_id(SESSION_B.to_owned());
    controller.set_active_session_id(SESSION_A.to_owned());

    let session_directory = workspace_sessions_dir(&config).join(SESSION_A);
    let harness = neo_agent_core::harness::FakeHarness::from_turns([]);
    let stale_handler_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called = Arc::clone(&stale_handler_called);
    let agent_config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(temp.path())
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Ask)
        .with_workflow_dispatch_resolver(config.workflow_dispatch_resolver.clone())
        .with_async_approval_handler(move |request| {
            let called = Arc::clone(&called);
            async move {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                ApprovalResponse::Cancelled {
                    request_id: request.id,
                    reason: ApprovalCancelReason::SessionEnded,
                }
            }
        });
    let handle = neo_agent_core::runtime::WorkflowDispatchHandle {
        config: agent_config,
        model_client: harness.client(),
        registry: Arc::new(neo_agent_core::ToolRegistry::with_builtin_tools()),
        process_supervisor: neo_agent_core::ProcessSupervisor::default(),
        context: neo_agent_core::AgentContext::new(),
    };
    let dispatch = tokio::spawn(async move {
        handle
            .run_one(
                neo_agent_core::workflow::WorkflowInvocationContext {
                    invocation_id: "idle-workflow-bash".to_owned(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                },
                "Bash",
                serde_json::json!({"command": "sudo --version"}),
            )
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            controller.drain_workflow_approvals();
            if controller
                .pending_approvals
                .contains_key("idle-workflow-bash")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("idle workflow approval reaches controller");
    controller.resolve_approval(ApprovalResponse::Selected {
        request_id: "idle-workflow-bash".to_owned(),
        action: ApprovalAction::Reject,
        feedback: None,
    });

    let outcome = dispatch.await.expect("workflow dispatch task");
    assert_eq!(
        outcome.status,
        neo_agent_core::workflow::WorkflowOutcomeStatus::Denied
    );
    assert!(!stale_handler_called.load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        harness.requests().is_empty(),
        "approval must not run a model turn"
    );
}

#[tokio::test]
async fn approval_up_down_does_not_recall_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    store.append(None, "old prompt").expect("seed history");
    let mut controller = controller_with_history_store(store);
    // Composer is empty so any leaked Up would otherwise recall "old prompt".

    let (pending, _response_rx) = make_pending_approval(ordinary_tool_request(
        "tool-1",
        "Write",
        "approved.txt",
        None,
    ));
    controller.register_pending_approval(pending);

    // Up/Down while approval is focused must move the dialog, not history.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up moves approval selection");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down moves approval selection");

    assert_eq!(
        controller.chrome().prompt().text,
        "",
        "approval Up/Down must not leak into PromptState"
    );
    drop(dir);
}

#[tokio::test]
#[ignore = "controller regression: pending approval keeps input while later delegate events arrive"]
async fn pending_approval_keeps_input_while_later_delegate_events_arrive() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (pending, response_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    controller.register_pending_approval(pending);

    // Later Delegate events arrive while the approval is pending.
    let config = test_config(
        &test_workspace_root(),
        test_workspace_root().join("sessions"),
    );
    let running = config
        .multi_agent
        .start_foreground_delegate_for_test("later delegate work");
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::DelegateStarted {
            turn: 1,
            agent: running,
            workflow_origin: None,
        });

    // The earliest blocking entry is still the approval: selection keys
    // target its option list, and the delegate card cannot displace it.
    assert_eq!(
        controller.tui.transcript().earliest_blocking_entry(),
        Some(neo_tui::transcript::BlockingEntryKind::Approval(
            "tool-1".to_owned()
        ))
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select moves within the approval");
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|(id, _, _, _)| id),
        Some("tool-1")
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm resolves the approval");
    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Selected { .. }
    ));
    // The later delegate card remains visible in the document after
    // resolution.
    assert!(
        controller.render_snapshot().contains("later delegate work"),
        "delegate card must remain in the transcript"
    );
    let slice = controller.tui.transcript_mut().render_visible_slice(80, 24);
    assert!(
        slice
            .iter()
            .any(|line| neo_tui::primitive::strip_ansi(line).contains("later delegate work")),
        "delegate card must remain in the document slice"
    );
}
