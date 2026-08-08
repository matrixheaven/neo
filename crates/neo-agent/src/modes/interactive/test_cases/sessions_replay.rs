//! Session replay/rebuild behavior (split from `sessions.rs`).

use std::{fs, path::Path};

use neo_agent_core::{
    AgentEvent, AgentMessage, ApprovalAction, ApprovalResolution, Content, StopReason,
};
use neo_tui::transcript::{TranscriptEntry, TranscriptPane};

use super::super::*;
use super::*;

#[test]
fn rebuild_transcript_from_session_replays_tool_calls_and_results() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new(
        "alpha",
        ["branch summary: inspected project".to_owned()],
        [
            AgentMessage::user_text("inspect"),
            AgentMessage::assistant(
                [Content::text("reading")],
                [neo_agent_core::AgentToolCall {
                    id: "tool-1".into(),
                    name: "Read".into(),
                    raw_arguments: r#"{"path":"README.md"}"#.into(),
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tool-1", "Read", [Content::text("README contents")], false),
        ],
    );

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("branch summary: inspected project"));
    assert!(rendered.contains("inspect"));
    assert!(rendered.contains("reading"));
    assert!(rendered.contains("Used Read (README.md)"));
    assert!(rendered.contains("README contents"));
    assert!(!rendered.contains("Using Read"));
}

#[test]
fn replay_session_into_transcript_uses_persisted_skill_invocation_outcome() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::SkillInvocation {
            names: vec!["missing".to_owned()],
            source: neo_agent_core::SkillInvocationSource::Auto,
            outcome: neo_agent_core::SkillInvocationOutcome::Failed,
            body: "skill `missing` is not available".to_owned(),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);

    assert!(matches!(
        transcript.transcript().entries(),
        [TranscriptEntry::SkillActivation {
            names,
            source: neo_agent_core::SkillInvocationSource::Auto,
            outcome: neo_agent_core::SkillInvocationOutcome::Failed,
            ..
        }] if names == &["missing".to_owned()]
    ));
}

#[test]
fn replay_session_into_transcript_restores_persisted_shell_command() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::shell_command(
                "printf shell-resume",
                "shell-resume-output",
                "",
                Some(0),
                neo_agent_core::ShellCommandOutcome::Completed,
                false,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replayed shell command")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("printf shell-resume"), "{rendered}");
    assert!(rendered.contains("shell-resume-output"), "{rendered}");
}

#[test]
fn replay_session_into_transcript_restores_aggregate_messages_when_no_detail_events_exist() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("aggregate-user"),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("aggregate-assistant")],
                [neo_agent_core::AgentToolCall {
                    id: "aggregate-tool".into(),
                    name: "Read".into(),
                    raw_arguments: r#"{"path":"aggregate.txt"}"#.into(),
                }],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "aggregate-tool",
                "Read",
                [Content::text("aggregate-result")],
                false,
            ),
        },
        AgentEvent::TokenUsage {
            turn: 1,
            usage: neo_agent_core::AgentTokenUsage {
                input_tokens: 1,
                output_tokens: 1,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            },
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(100, 20)
        .expect("render aggregate-only replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("aggregate-user"), "{rendered}");
    assert!(rendered.contains("aggregate-assistant"), "{rendered}");
    assert!(rendered.contains("aggregate.txt"), "{rendered}");
    assert!(rendered.contains("aggregate-result"), "{rendered}");
}

#[test]
fn replay_session_into_transcript_prefers_user_display_text() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::user_content_with_display(
                [Content::text("<file path=\"src/main.rs\">snapshot</file>")],
                "review @[main.rs]",
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);

    assert!(matches!(
        transcript.transcript().entries(),
        [TranscriptEntry::UserMessage { content, .. }] if content == "review @[main.rs]"
    ));
}

#[test]
fn replay_session_into_transcript_does_not_duplicate_text_delta_aggregate_without_finish() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::TextDelta {
            turn: 1,
            text: "truncated-assistant".to_owned(),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("truncated-assistant")],
                [],
                StopReason::EndTurn,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(100, 20)
        .expect("render truncated replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        rendered.matches("truncated-assistant").count(),
        1,
        "{rendered}"
    );
}

#[test]
fn rebuild_transcript_sets_workspace_root_before_replaying_instruction_cards() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let workspace = workspace.canonicalize().expect("canonical workspace");
    let config = test_config(&workspace, temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    let nested = workspace.join("crates/neo-tui");
    let epoch = neo_agent_core::instructions::InstructionEpochData {
        agent_id: "main".to_owned(),
        generation: 1,
        outcome: neo_agent_core::instructions::InstructionEpochOutcome::Activated,
        scopes: vec![neo_agent_core::instructions::InstructionScopeData {
            display_path: nested.clone(),
            kind: neo_agent_core::instructions::InstructionScopeKind::Nested,
            revision: Some("7af13c2e".to_owned()),
            token_estimate: 1_024,
        }],
        selected_bundles: vec![neo_agent_core::instructions::InstructionBundleMetadata {
            display_path: nested,
            revision: "7af13c2e".to_owned(),
            token_estimate: 1_024,
            byte_size: 4_096,
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
        body_revisions: None,
        model_content: Some("SECRET INSTRUCTION BODY".to_owned()),
    };
    let loaded = LoadedSessionTranscript::new(SESSION_A, Vec::new(), Vec::new())
        .with_events([AgentEvent::InstructionEpoch { epoch }]);

    controller.rebuild_transcript_from_session(&loaded);

    let card = controller
        .tui
        .transcript()
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::InstructionEpoch { component } => Some(component.copy_text()),
            _ => None,
        })
        .expect("instruction card");
    let nested_label = format!("{}/**", Path::new("crates").join("neo-tui").display());
    assert!(card.contains(&nested_label), "{card}");
    assert!(!card.contains("<outside-workspace>"), "{card}");
    assert!(!card.contains(&temp.path().display().to_string()), "{card}");
    assert!(!card.contains("SECRET INSTRUCTION BODY"), "{card}");
}

#[test]
fn replay_session_into_transcript_restores_only_retry_exhaustion() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 1,
            delay_ms: 500,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: connection reset".to_owned(),
        },
        AgentEvent::RetryStarted {
            turn: 1,
            retry: 1,
            max_retries: 1,
        },
        AgentEvent::RetryResumed { turn: 1, retry: 1 },
        AgentEvent::RetryExhausted {
            turn: 1,
            retries_used: 1,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: connection reset".to_owned(),
        },
        AgentEvent::Error {
            turn: 1,
            message: "transport error: connection reset".to_owned(),
            code: Some("provider.transport_error".to_owned()),
            retry_after: None,
        },
        AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::Error,
        },
        AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Error,
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);

    let retry_entries = transcript
        .transcript()
        .entries()
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::RetryStatus { .. }))
        .collect::<Vec<_>>();
    assert_eq!(retry_entries.len(), 1);
    assert!(matches!(
        retry_entries[0],
        TranscriptEntry::RetryStatus { data }
            if data.phase == neo_tui::transcript::entry::RetryPhase::Exhausted
    ));
    assert_eq!(
        retry_entries[0].finalization(),
        neo_tui::primitive::Finalization::Finalized
    );
    assert!(
        transcript
            .transcript()
            .entries()
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::Status { .. }))
    );
    let rendered = transcript
        .render_frame(100, 20)
        .expect("render retry replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        rendered.matches("Reconnect failed after 1 retry").count(),
        1,
        "{rendered}"
    );
    for unexpected in ["Reconnecting", "runtime error"] {
        assert!(!rendered.contains(unexpected), "{rendered}");
    }
}

#[test]
fn replay_session_into_transcript_consumes_assistant_coverage_per_occurrence() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("same-assistant")],
                [],
                StopReason::EndTurn,
            ),
        },
        AgentEvent::TextDelta {
            turn: 2,
            text: "same-assistant".to_owned(),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("same-assistant")],
                [],
                StopReason::EndTurn,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(100, 20)
        .expect("render repeated aggregate replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered.matches("same-assistant").count(), 2, "{rendered}");
}

#[test]
fn replay_session_into_transcript_keeps_uncovered_image_without_repeating_text() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageStarted {
            phase: neo_ai::MessagePhase::Unknown,
            turn: 1,
            id: "assistant-image".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "image-caption".to_owned(),
        },
        AgentEvent::MessageFinished {
            phase: neo_ai::MessagePhase::Unknown,
            turn: 1,
            id: "assistant-image".to_owned(),
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [
                    Content::text("image-caption"),
                    Content::Image {
                        mime_type: "image/png".into(),
                        data: neo_agent_core::ImageRef::Base64("aGVsbG8=".into()),
                    },
                ],
                [],
                StopReason::EndTurn,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let entries = transcript.transcript().entries();
    let assistant_text_count = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                TranscriptEntry::AssistantMessage { content } if content == "image-caption"
            )
        })
        .count();
    let image_count = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::Image { .. }))
        .count();

    assert_eq!(assistant_text_count, 1, "{entries:?}");
    assert_eq!(image_count, 1, "{entries:?}");
}

#[test]
fn replay_session_into_transcript_does_not_carry_tool_lifecycle_into_next_assistant() {
    let tool_call = neo_agent_core::AgentToolCall {
        id: "ordered-tool".into(),
        name: "Read".into(),
        raw_arguments: r#"{"path":"ordered.txt"}"#.into(),
    };
    let mut transcript = TranscriptPane::new(110, 24);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageStarted {
            phase: neo_ai::MessagePhase::Unknown,
            turn: 1,
            id: "assistant-before".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "before-tool".to_owned(),
        },
        AgentEvent::ToolCallStarted {
            turn: 1,
            id: "ordered-tool".to_owned(),
            name: "Read".to_owned(),
        },
        AgentEvent::ToolCallFinished {
            turn: 1,
            tool_call: tool_call.clone(),
        },
        AgentEvent::MessageFinished {
            phase: neo_ai::MessagePhase::Unknown,
            turn: 1,
            id: "assistant-before".to_owned(),
            stop_reason: StopReason::ToolUse,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("before-tool")],
                [tool_call.clone()],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "ordered-tool".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({"path": "ordered.txt"}),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "ordered-tool".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("ordered-result"),
            workflow_origin: None,
            output_ref: None,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "ordered-tool",
                "Read",
                [Content::text("ordered-result")],
                false,
            ),
        },
        AgentEvent::MessageStarted {
            phase: neo_ai::MessagePhase::Unknown,
            turn: 2,
            id: "assistant-after".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 2,
            text: "after-tool".to_owned(),
        },
        AgentEvent::MessageFinished {
            phase: neo_ai::MessagePhase::Unknown,
            turn: 2,
            id: "assistant-after".to_owned(),
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("after-tool")],
                [],
                StopReason::EndTurn,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(110, 24)
        .expect("render ordered replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered.matches("before-tool").count(), 1, "{rendered}");
    assert_eq!(rendered.matches("after-tool").count(), 1, "{rendered}");
    assert_eq!(rendered.matches("ordered-result").count(), 1, "{rendered}");
}

#[test]
fn replay_finalizes_dangling_shell_queue_without_restart() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::ToolCallStarted {
            turn: 1,
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
        },
        AgentEvent::ToolCallFinished {
            turn: 1,
            tool_call: neo_agent_core::AgentToolCall {
                id: "call-1".into(),
                name: "Bash".into(),
                raw_arguments: r#"{"command":"cargo test"}"#.into(),
            },
        },
        AgentEvent::ToolExecutionQueued {
            turn: 1,
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({"command": "cargo test"}),
            workflow_origin: None,
        },
    ]);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Interrupted when terminal exited"),
        "{rendered}"
    );
    assert!(!rendered.contains("Queued Bash"), "{rendered}");
}

#[test]
fn replay_restores_resolved_workflow_source_from_durable_request() {
    let request = replay_workflow_request();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::ApprovalRequested {
            request: request.clone(),
        },
        AgentEvent::ApprovalResolved {
            turn: 1,
            request_id: request.id.clone(),
            resolution: ApprovalResolution::Selected {
                action: ApprovalAction::LaunchWorkflow,
                label: "Launch".to_owned(),
                feedback: None,
            },
        },
    ]);
    let mut transcript = TranscriptPane::new(100, 80);
    replay_session_into_transcript(&mut transcript, &loaded);

    let card = transcript
        .transcript()
        .approval("workflow-replay")
        .expect("replayed workflow approval");
    assert_eq!(card.request, request);
    let rendered = transcript
        .render_frame(100, 80)
        .expect("render replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("approval: Launch"), "frame: {rendered}");
    assert!(rendered.contains("neo.phase('work')"), "frame: {rendered}");
    assert!(rendered.contains("return {}"), "frame: {rendered}");
}

#[test]
fn rebuild_transcript_from_session_keeps_configured_context_window_without_snapshot() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "new",
        "deepseek/deepseek-v4-pro",
        test_workspace_root(),
        |_| async { Ok(Vec::new()) },
    );
    controller
        .tui
        .chrome_mut()
        .set_context_window(Some(ContextWindow::new(1_000_000)));

    let loaded =
        LoadedSessionTranscript::new("alpha", Vec::new(), [AgentMessage::user_text("hello")]);

    controller.rebuild_transcript_from_session(&loaded);

    assert_eq!(
        controller.chrome().context_window(),
        Some(ContextWindow::new(1_000_000))
    );
}

#[tokio::test]
async fn load_session_transcript_keeps_context_usage_event_authoritative() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    writer
        .append(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("remember this"),
        })
        .await
        .expect("append user message");
    writer.flush().await.expect("flush session");

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");

    assert_eq!(
        loaded.messages,
        vec![AgentMessage::user_text("remember this")]
    );
}

#[tokio::test]
async fn load_session_transcript_replays_token_usage_for_footer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    writer
        .append(&AgentEvent::TokenUsage {
            turn: 1,
            usage: neo_agent_core::AgentTokenUsage {
                input_tokens: 33_900,
                output_tokens: 2_800,
                input_cache_read_tokens: 169_200,
                input_cache_write_tokens: 0,
            },
        })
        .await
        .expect("append token usage");
    writer.flush().await.expect("flush session");

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");

    assert_eq!(loaded.main_agent_token_usage.input_tokens, 33_900);
    assert_eq!(loaded.main_agent_token_usage.output_tokens, 2_800);
    assert_eq!(
        loaded.main_agent_token_usage.input_cache_read_tokens,
        169_200
    );
    assert_eq!(loaded.main_agent_token_usage.input_cache_write_tokens, 0);
}

#[tokio::test]
async fn load_session_transcript_preserves_delegate_events_for_replay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    let snapshot = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("audit paths");
    writer
        .append(&AgentEvent::DelegateStarted {
            turn: 1,
            agent: snapshot,
            workflow_origin: None,
        })
        .await
        .expect("append delegate");
    writer.flush().await.expect("flush session");

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");

    assert!(
        loaded
            .events
            .iter()
            .any(|event| matches!(event, AgentEvent::DelegateStarted { .. })),
        "delegate events should be preserved for transcript replay"
    );
}

#[tokio::test]
async fn load_session_replay_preserves_interleaved_visible_entry_order() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    write_interleaved_replay_session(&config).await;

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");
    let mut transcript = TranscriptPane::new(140, 200);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(140, 200)
        .expect("render replayed transcript")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    let markers = [
        "resume-user",
        "resume-thinking",
        "resume-output",
        "restored delegate card",
        "first-order.txt",
        "failed delegate marker",
        "later-order-command",
        "resume-summary",
    ];
    let positions = markers
        .iter()
        .map(|marker| {
            rendered
                .find(marker)
                .unwrap_or_else(|| panic!("missing {marker:?}: {rendered}"))
        })
        .collect::<Vec<_>>();

    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "resume replay order mismatch: {rendered}"
    );
}

#[tokio::test]
async fn load_session_transcript_rejects_oversized_main_wire_before_replay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    fs::create_dir_all(session_path.parent().expect("main wire parent")).expect("create parent");
    let file = fs::File::create(&session_path).expect("create oversized session");
    file.set_len(crate::modes::sessions::MAX_RESUME_SESSION_BYTES + 1)
        .expect("make sparse oversized session");

    let error = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect_err("oversized session should be rejected before replay");
    let message = error.to_string();

    assert!(message.contains("too large to resume safely"), "{message}");
    assert!(!message.contains("neo sessions slim"), "{message}");
}

#[test]
fn rebuild_transcript_from_session_restores_footer_token_usage() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller
        .tui
        .chrome_mut()
        .set_context_window(Some(ContextWindow::new(512_000)));

    let mut usage = neo_tui::shell::MainAgentTokenUsage::default();
    usage.add(neo_agent_core::AgentTokenUsage {
        input_tokens: 33_900,
        output_tokens: 2_800,
        input_cache_read_tokens: 169_200,
        input_cache_write_tokens: 0,
    });
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new())
        .with_events([AgentEvent::ContextWindowUpdated {
            turn: 1,
            used_tokens: 152_000,
            projected_tokens: Some(152_000),
            max_tokens: Some(262_000),
            trigger_tokens: Some(209_600),
            remaining_tokens: Some(110_000),
            source: Some(neo_agent_core::ContextWindowSource::Configured),
        }])
        .with_main_agent_token_usage(usage);

    controller.rebuild_transcript_from_session(&loaded);

    let footer = controller
        .render_snapshot()
        .lines()
        .find(|line| line.contains("ctx "))
        .expect("footer contains context")
        .to_owned();

    assert!(footer.contains("ctx 152k/262k"));
    assert!(footer.contains("↑33.9k"));
    assert!(footer.contains("↓2.8k"));
    assert!(footer.contains("cache 169.2k read"));
    assert!(footer.contains("hit 100.0%"));
}
