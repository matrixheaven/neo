//! Transcript skill-card behavior (split from `transcript.rs`).

use neo_agent_core::{AgentEvent, AgentMessage, Content};
use neo_tui::{
    input::{InputEvent, KeybindingAction},
    transcript::TranscriptEntry,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn manual_skill_context_uses_shared_path_aware_envelope() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let stripped = "\
foo
bar
test test test
bonjour
hello
test test test test
hola
amigo";
    let stripped_for_event = stripped.to_owned();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            let stripped_for_event = stripped_for_event.clone();
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(vec![AgentEvent::MessageAppended {
                    message: AgentMessage::user_text(stripped_for_event),
                }])
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());
    let prompt = "\
foo
bar
/skill:skill_one test test test
bonjour
hello
/skill:skill_two test test test test
hola
amigo";

    controller.type_text(prompt);
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("skill activation succeeds");

    assert!(controller.chrome().prompt().text.is_empty());
    let entries = transcript_entries(&controller);
    let skill_cards = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
        .count();
    assert_eq!(skill_cards, 1);
    assert!(matches!(
        entries.last(),
        Some(TranscriptEntry::SkillActivation {
            names,
            source: neo_agent_core::SkillInvocationSource::Manual,
            body,
            ..
        }) if names == &vec!["skill_one".to_owned(), "skill_two".to_owned()] && body == stripped
    ));

    controller
        .wait_for_active_turn()
        .await
        .expect("stripped prompt turn completes");
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text(stripped)]);
    let skill_context = requests[0].skill_context.as_deref().expect("skill context");
    assert!(
        skill_context.contains("User activated the skills \"skill_one\", \"skill_two\""),
        "{skill_context}"
    );
    assert!(
        skill_context.contains(&format!(
            "<neo-skill-loaded name=\"skill_one\" source=\"builtin\" root=\"{}\">",
            test_workspace_root().join("builtin/skill_one").display()
        )),
        "{skill_context}"
    );
    assert!(
        skill_context.contains(
            "<dependencies>\n  <mcp value=\"reviewServer\">Review MCP server</mcp>\n</dependencies>"
        ),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("<neo-user-request>\nfoo\nbar\ntest test test"),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("<instructions>\nONE: test test test\n</instructions>"),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("TWO: test test test test"),
        "{skill_context}"
    );
    assert!(
        skill_context.find("ONE:").expect("first skill")
            < skill_context.find("TWO:").expect("second skill"),
        "{skill_context}"
    );
    assert!(
        !transcript_entries(&controller)
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == stripped)),
        "skill activation body should not be rendered again as a user message"
    );
}

#[tokio::test]
async fn automatic_skill_invocation_renders_one_semantic_card() {
    use futures::StreamExt as _;
    use neo_agent_core::harness::FakeHarness;

    let harness = FakeHarness::from_turns([
        vec![
            neo_ai::AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            neo_ai::AiStreamEvent::ToolCallStart {
                id: "skill-1".to_owned(),
                name: "Skill".to_owned(),
            },
            neo_ai::AiStreamEvent::ToolCallEnd {
                id: "skill-1".to_owned(),
                raw_arguments: serde_json::json!({"skill": "refactor"}).to_string(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            neo_ai::AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            neo_ai::AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let model = harness.model();
    let client = harness.client();
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let model = model.clone();
        let client = Arc::clone(&client);
        Box::pin(async move {
            let runtime = neo_agent_core::AgentRuntime::with_tools_and_skills(
                neo_agent_core::AgentConfig::for_model(model),
                client,
                neo_agent_core::ToolRegistry::new(),
                skill_store_with_refactor_skill(),
            );
            let mut context = neo_agent_core::AgentContext::new();
            let mut events =
                runtime.run_turn(&mut context, AgentMessage::user_content(request.prompt));
            while let Some(event) = events.next().await {
                channels.send_event(event?);
            }
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "fake/model",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );

    controller.type_text("use refactor skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("automatic skill turn completes");

    let entries = transcript_entries(&controller);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
            .count(),
        1,
        "automatic invocation should render exactly one semantic card"
    );
    assert!(entries.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::SkillActivation {
            names,
            source: neo_agent_core::SkillInvocationSource::Auto,
            outcome: neo_agent_core::SkillInvocationOutcome::Activated,
            ..
        } if names == &["refactor".to_owned()]
    )));
    assert!(
        entries
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::ToolRun { .. })),
        "the hidden Skill tool must not create a duplicate generic card"
    );
}

#[tokio::test]
async fn inline_skill_directive_with_paste_marker_renders_one_card() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let paste_text = "line one\nline two\nline three";
    let expected_display = format!("{paste_text}review this");
    let expanded_for_event = expected_display.clone();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            let expanded_for_event = expanded_for_event.clone();
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(vec![AgentEvent::MessageAppended {
                    message: AgentMessage::user_text(expanded_for_event),
                }])
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());
    controller.paste_store.insert(1, paste_text.to_owned());

    controller.type_text("/skill:skill_one [paste #1 +3 lines]review this");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("skill activation succeeds");

    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let entries = transcript_entries(&controller);
    let skill_card_count = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
        .count();
    assert_eq!(
        skill_card_count, 1,
        "expected exactly one skill activation card"
    );
    let skill_card = entries
        .iter()
        .find(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
        .expect("one skill activation card");
    assert!(matches!(
        skill_card,
        TranscriptEntry::SkillActivation { names, body, .. }
            if names == &vec!["skill_one".to_owned()] && body == &expected_display
    ));

    assert!(
        !entries.iter().any(
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == &expected_display)
        ),
        "expanded skill activation body should not be rendered again as a user message"
    );

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text(expected_display)]);
}

#[tokio::test]
async fn inline_skill_directive_without_whitespace_prefix_submits_as_plain_prompt() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());

    controller.type_text("abc/skill:skill_one test");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("plain prompt submits");

    controller
        .wait_for_active_turn()
        .await
        .expect("plain prompt turn completes");
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("abc/skill:skill_one test")]
    );
    assert_eq!(requests[0].skill_context, None);
}

#[tokio::test]
async fn inline_skill_directive_unknown_skill_reports_status_without_submitting() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());

    controller.type_text("foo /skill:missing test");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("unknown skill handled");

    assert!(requests.lock().expect("requests lock").is_empty());
    assert!(transcript_has_status(
        &controller,
        "skill `missing` not found"
    ));
    assert_eq!(controller.chrome().prompt().text, "foo /skill:missing test");
}
