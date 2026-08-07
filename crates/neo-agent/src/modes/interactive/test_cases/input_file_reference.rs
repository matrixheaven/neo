//! File-reference completion behavior (split from `input.rs`).

use std::fs;

use neo_agent_core::{AgentEvent, AgentMessage, Content, StopReason};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    transcript::TranscriptEntry,
};

use super::super::*;
use super::*;

#[test]
fn at_file_reference_completion_fuzzy_ranks_basename_matches() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("crates/neo-agent/src/modes/interactive");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("prompt_completion.rs"), "").expect("write prompt completion");
    fs::write(src.join("completion_prompt.rs"), "").expect("write weaker match");

    let catalog = CompletionCatalog::default();
    let candidates =
        completion_source_candidates(temp.path(), "@prom", &catalog).expect("file references");

    assert_eq!(
        candidates[0].value,
        "@crates/neo-agent/src/modes/interactive/prompt_completion.rs"
    );
    assert_eq!(candidates[0].label, "prompt_completion.rs");
    assert_eq!(
        candidates[0].description.as_deref(),
        Some("crates/neo-agent/src/modes/interactive/")
    );
    assert_eq!(candidates[0].source, CompletionSource::FileReference);
}

#[test]
fn at_file_reference_completion_preserves_match_ranking_over_value_sort() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("aaa")).expect("mkdir aaa");
    fs::create_dir_all(temp.path().join("zzz")).expect("mkdir zzz");
    fs::write(temp.path().join("aaa/not_prompt.rs"), "").expect("write weaker match");
    fs::write(temp.path().join("zzz/prompt_completion.rs"), "").expect("write stronger match");

    let catalog = CompletionCatalog::default();
    let candidates =
        completion_source_candidates(temp.path(), "@prom", &catalog).expect("file references");

    assert_eq!(candidates[0].value, "@zzz/prompt_completion.rs");
    assert_eq!(candidates[0].label, "prompt_completion.rs");
    assert_eq!(candidates[0].source, CompletionSource::FileReference);
}

#[test]
fn at_file_reference_completion_caps_large_walks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let inspected_cap = 7;
    for index in 0..(inspected_cap + 5) {
        fs::write(temp.path().join(format!("prompt_{index:04}.rs")), "").expect("write match");
    }

    let candidates = super::prompt_completion::file_reference_completion_candidates_with_limits(
        temp.path(),
        "@prompt",
        inspected_cap,
        super::prompt_completion::MAX_FILE_REFERENCE_COMPLETIONS,
    );

    assert_eq!(candidates.len(), inspected_cap);
    assert!(candidates.iter().all(|candidate| {
        candidate.value.starts_with("@prompt_")
            && candidate.source == CompletionSource::FileReference
    }));
}

#[test]
fn at_file_reference_completion_hides_dotfiles_until_dot_query() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join(".env"), "secret").expect("write env");
    fs::write(temp.path().join("Cargo.toml"), "").expect("write cargo");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir src");
    fs::write(temp.path().join("src/.env"), "nested secret").expect("write nested env");

    let catalog = CompletionCatalog::default();
    let hidden = completion_source_candidates(temp.path(), "@e", &catalog).expect("hidden query");
    assert!(hidden.iter().all(|candidate| candidate.label != ".env"));

    let visible = completion_source_candidates(temp.path(), "@.e", &catalog).expect("dot query");
    assert!(visible.iter().any(|candidate| candidate.label == ".env"));

    let nested_visible =
        completion_source_candidates(temp.path(), "@src/.e", &catalog).expect("nested dot query");
    assert!(
        nested_visible
            .iter()
            .any(|candidate| candidate.value == "@src/.env")
    );
}

#[test]
fn at_file_reference_completion_no_longer_returns_provider_models() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("docs")).expect("mkdir docs");
    fs::write(temp.path().join("docs/anthology.md"), "notes\n").expect("write file");
    let catalog = CompletionCatalog::default();

    let candidates =
        completion_source_candidates(temp.path(), "@anth", &catalog).expect("file references");

    assert!(!candidates.is_empty());
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.value != "@anthropic/claude-sonnet")
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.source == CompletionSource::FileReference)
    );
}

#[tokio::test]
async fn event_loop_tab_coalesces_latest_file_completion_and_inserts_marker() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write file");

    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![
                PickerItem::new(
                    "anthropic/claude-sonnet",
                    "anthropic/claude-sonnet",
                    Some("Messages"),
                ),
                PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
            ],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller
        .handle_input_event(InputEvent::Insert('@'))
        .await
        .expect("start file completion");
    controller
        .handle_input_event(InputEvent::Paste("main".to_owned()))
        .await
        .expect("queue latest file completion");
    let (queued, complete_on_finish) = controller
        .queued_file_completion
        .as_ref()
        .expect("latest file completion is queued");
    assert_eq!(queued.text, "@main");
    assert!(!*complete_on_finish);
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab inserts file reference");
    let (queued, complete_on_finish) = controller
        .queued_file_completion
        .as_ref()
        .expect("tab upgrades the latest queued completion");
    assert_eq!(queued.text, "@main");
    assert!(*complete_on_finish);
    wait_for_file_completion(&mut controller).await;

    assert_eq!(controller.chrome().prompt().text, "[file #1 main.rs]");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_rejects_parent_dir_file_reference_completion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("mkdir workspace");
    fs::write(temp.path().join("outside.txt"), "outside\n").expect("write outside");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &workspace,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("@bad");
    controller.tui.chrome_mut().open_prompt_completion_picker(
        PromptCompletionPrefix {
            start: 0,
            end: 4,
            text: "@bad".to_owned(),
        },
        [PickerItem::new(
            "@../outside.txt",
            "outside.txt",
            None::<String>,
        )],
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab rejects parent-dir file reference");

    assert_eq!(controller.chrome().prompt().text, "@bad");
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "File reference is outside the workspace"
    ));
}

#[tokio::test]
async fn event_loop_closes_stale_file_reference_picker_without_inserting_marker() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    let main = temp.path().join("src/main.rs");
    fs::write(&main, "fn main() {}\n").expect("write main");
    fs::write(
        temp.path().join("src/main_test.rs"),
        "#[test]\nfn main_test() {}\n",
    )
    .expect("write second match");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("@main");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab opens file reference picker");
    wait_for_file_completion(&mut controller).await;
    assert!(controller.chrome().focused_overlay().is_some());

    fs::remove_file(main).expect("remove selected completion");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab rejects stale file reference");

    assert_eq!(controller.chrome().prompt().text, "@main");
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "File reference no longer exists"
    ));
}

#[tokio::test]
async fn event_loop_submits_file_reference_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write file");

    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs::default(),
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("review @main");
    controller.tui.chrome_mut().open_prompt_completion_picker(
        PromptCompletionPrefix {
            start: 7,
            end: 12,
            text: "@main".to_owned(),
        },
        [PickerItem::new("@src/main.rs", "main.rs", None::<String>)],
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("insert file reference");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("submit turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text(
            "review <file path=\"src/main.rs\">\nfn main() {}\n</file>"
        )]
    );
}

#[tokio::test]
async fn event_loop_file_reference_marker_keeps_chip_in_user_transcript() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("crates/neo-agent/src/modes/interactive");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("prompt_completion.rs"), "").expect("write prompt completion");
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        phase: neo_ai::MessagePhase::Unknown,
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "file reference expanded".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![
                PickerItem::new(
                    "anthropic/claude-sonnet",
                    "anthropic/claude-sonnet",
                    Some("Messages"),
                ),
                PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
            ],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@prom");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab inserts file reference marker");
    wait_for_file_completion(&mut controller).await;
    assert_eq!(
        controller.chrome().prompt().text,
        "[file #1 prompt_completion.rs]"
    );
    controller.type_text(" explain this file");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits with file reference");
    controller
        .wait_for_active_turn()
        .await
        .expect("file reference turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text(
            "<file path=\"crates/neo-agent/src/modes/interactive/prompt_completion.rs\">\n</file> explain this file"
        )]
    );
    assert_eq!(
        requests[0].prompt_display_text.as_deref(),
        Some("@[prompt_completion.rs] explain this file")
    );
    assert_eq!(requests[0].model, None);
    assert!(transcript_entries(&controller).iter().any(|entry| matches!(
        entry,
        TranscriptEntry::UserMessage { content, .. }
            if content == "@[prompt_completion.rs] explain this file"
    )));
    assert!(
        transcript_entries(&controller)
            .iter()
            .all(|entry| !matches!(
                entry,
                TranscriptEntry::UserMessage { content, .. } if content.contains("<file path=")
            ))
    );
}

#[tokio::test]
async fn queued_file_reference_keeps_chip_when_appended() {
    let mut controller = running_turn_controller().await;
    controller.type_text("review [file #1 main.rs]");

    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter while busy enqueues");

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["review @[main.rs]"]
    );
    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_content_with_display(
            [Content::text(
                "review <file path=\"src/main.rs\">snapshot</file>",
            )],
            "review @[main.rs]",
        ),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .len(),
        1,
        "runtime ack must consume the compact optimistic preview"
    );
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_content_with_display(
            [Content::text(
                "review <file path=\"src/main.rs\">snapshot</file>",
            )],
            "review @[main.rs]",
        ),
    });
    assert!(transcript_entries(&controller).iter().any(|entry| matches!(
        entry,
        TranscriptEntry::UserMessage { content, .. } if content == "review @[main.rs]"
    )));

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn steered_file_reference_keeps_chip_when_appended() {
    let mut controller = running_turn_controller().await;
    controller.type_text("inspect [file #1 lib.rs]");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s steers");

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["inspect @[lib.rs]"]
    );
    controller.apply_turn_event(AgentEvent::SteeringQueued {
        message: AgentMessage::user_content_with_display(
            [Content::text(
                "inspect <file path=\"src/lib.rs\">snapshot</file>",
            )],
            "inspect @[lib.rs]",
        ),
    });
    assert_eq!(
        controller.chrome().pending_input().pending_steers().len(),
        1,
        "runtime ack must consume the compact optimistic preview"
    );
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_content_with_display(
            [Content::text(
                "inspect <file path=\"src/lib.rs\">snapshot</file>",
            )],
            "inspect @[lib.rs]",
        ),
    });
    assert!(transcript_entries(&controller).iter().any(|entry| matches!(
        entry,
        TranscriptEntry::UserMessage { content, .. } if content == "inspect @[lib.rs]"
    )));

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn event_loop_tab_extends_common_filesystem_completion_prefix() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("README.md"), "readme\n").expect("write readme");
    fs::write(temp.path().join("RELEASE.md"), "release\n").expect("write release");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.completion_root = temp.path().to_path_buf();

    controller.type_text("open R");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab extends common prefix");

    assert_eq!(controller.chrome().prompt().text, "open RE");
    assert_eq!(controller.chrome().prompt().cursor, 7);
    assert!(controller.chrome().focused_overlay().is_none());
}
