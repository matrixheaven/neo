//! Init command and preflight behavior (split from `workflow.rs`).

use std::path::{Path, PathBuf};

use neo_agent_core::{AgentEvent, Content, MessageOrigin, PermissionMode};
use neo_tui::{
    input::{InputEvent, KeybindingAction},
    shell::OverlayKind,
};

use super::super::*;
use super::*;

#[test]
fn init_command_prompt_includes_instruction_and_structure_guardrails() {
    let prompt = init_command::build_init_workflow_prompt(init_command::InitPromptRequest {
        workspace_root: Path::new("/workspace/neo"),
        current_date: "2026-07-07",
        source_commit: Some("abc1234"),
        instruction: Some("排除掉 generated 目录"),
        auto_mode_best_effort: false,
    });

    assert!(prompt.contains("Target file: /workspace/neo/AGENTS.md"));
    assert!(prompt.contains("Current date: 2026-07-07"));
    assert!(prompt.contains("Source commit: abc1234"));
    assert!(prompt.contains("User instruction after /init: 排除掉 generated 目录"));
    assert!(prompt.contains("Required top-level sections, in order:"));
    assert!(prompt.contains("1. Reference"));
    assert!(prompt.contains("14. Metadata"));
    assert!(prompt.contains("Before writing AGENTS.md, prepare a concise outline plan"));
    assert!(prompt.contains("Golden style example"));
    assert!(
        prompt
            .to_lowercase()
            .contains("ask the user where reference projects or reference documents live")
    );
    assert!(prompt.contains("Do not treat this guide as a Neo product specification"));
    assert!(
        !prompt.contains("CLAUDE.md"),
        "legacy guide leaked: {prompt}"
    );
    assert!(
        !prompt.contains("GEMINI.md"),
        "legacy guide leaked: {prompt}"
    );
}

#[test]
fn init_command_prompt_marks_auto_mode_best_effort() {
    let prompt = init_command::build_init_workflow_prompt(init_command::InitPromptRequest {
        workspace_root: Path::new("/workspace/neo"),
        current_date: "2026-07-07",
        source_commit: None,
        instruction: None,
        auto_mode_best_effort: true,
    });

    assert!(prompt.contains("Source commit: unavailable"));
    assert!(prompt.contains("Auto permission mode remained active"));
    assert!(prompt.contains("proceed with best-effort assumptions"));
}

#[test]
fn agents_guide_validator_accepts_required_structure() {
    let guide = init_command::example_agents_guide_for_tests("abc1234");
    assert_eq!(init_command::validate_agents_guide(&guide), Vec::new());
}

#[test]
fn agents_guide_validator_reports_missing_duplicate_and_reordered_headings() {
    let guide = r"# Project Agent Guide

## Reference
text

## Workflow
text

## Project Identity
text

## Workflow
text

## Metadata
Created: 2026-07-07
Source commit: abc1234
Best valid until: 2026-10-07
";

    let issues = init_command::validate_agents_guide(guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::MissingHeading));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::DuplicateHeading));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::HeadingOrder));
}

#[test]
fn agents_guide_validator_reports_missing_metadata_and_placeholders() {
    let guide = init_command::REQUIRED_AGENTS_HEADINGS
        .iter()
        .map(|heading| format!("## {heading}\nplaceholder content\n"))
        .collect::<Vec<_>>()
        .join("\n");

    let issues = init_command::validate_agents_guide(&guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::MissingMetadataField));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::PlaceholderText));
}

#[test]
fn agents_guide_validator_reports_hardcoded_reference_default_variants() {
    let mut guide = init_command::example_agents_guide_for_tests("abc1234");
    guide.push_str("\nReferences are in `.references`.\n");

    let issues = init_command::validate_agents_guide(&guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::HardcodedReferenceDefault));
}

#[test]
fn agents_guide_validator_allows_negative_reference_default_framing() {
    let mut guide = init_command::example_agents_guide_for_tests("abc1234");
    guide.push_str("\nDo not assume .references/ is the reference location.\n");

    assert_eq!(init_command::validate_agents_guide(&guide), Vec::new());
}

#[test]
fn agents_guide_validator_reports_product_spec_framing_variants() {
    let mut guide = init_command::example_agents_guide_for_tests("abc1234");
    guide.push_str("\nThis guide is the Neo product specification.\n");

    let issues = init_command::validate_agents_guide(&guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::ProductSpecFraming));
}

#[test]
fn agents_guide_validator_allows_negative_product_spec_framing() {
    let mut guide = init_command::example_agents_guide_for_tests("abc1234");
    guide.push_str("\nDo not treat this guide as a Neo product specification.\n");

    assert_eq!(init_command::validate_agents_guide(&guide), Vec::new());
}

#[test]
fn init_repair_prompt_lists_validator_issues() {
    let issues = vec![
        init_command::AgentsGuideIssue {
            code: init_command::AgentsGuideIssueCode::MissingHeading,
            message: "Missing required heading: Security".to_owned(),
        },
        init_command::AgentsGuideIssue {
            code: init_command::AgentsGuideIssueCode::MissingMetadataField,
            message: "Missing Metadata field: Best valid until:".to_owned(),
        },
    ];

    let prompt = init_command::build_agents_guide_repair_prompt(&issues);

    assert!(prompt.contains("AGENTS.md structure validation failed"));
    assert!(prompt.contains("Missing required heading: Security"));
    assert!(prompt.contains("Missing Metadata field: Best valid until:"));
    assert!(prompt.contains("Update AGENTS.md in place"));
    assert!(prompt.contains("Do not claim success until the issues are fixed"));
}

#[tokio::test]
async fn slash_init_submits_generated_workflow_prompt() {
    let (mut controller, requests) = controller_with_session_for_new_tests();

    controller.type_text("/init 排除掉 generated 目录");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("init command submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("requests captured");
    assert_eq!(requests.len(), 1);
    let request = requests.first().expect("captured request");
    let prompt = request
        .prompt
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(prompt.contains("<system-reminder>"), "{prompt}");
    assert!(
        prompt.contains("You are running Neo's /init workflow."),
        "{prompt}"
    );
    assert!(prompt.contains("排除掉 generated 目录"), "{prompt}");
    assert!(prompt.contains("Target file:"), "{prompt}");
    assert!(prompt.contains("AGENTS.md"), "{prompt}");
    assert_eq!(request.prompt_origin, MessageOrigin::injection("init"));
}

#[tokio::test]
async fn slash_init_is_not_persisted_to_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompt-history.jsonl");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

    let mut controller = controller_with_history_store(store);

    controller.type_text("/init 排除掉 generated 目录");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("init command handled");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let persisted = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        persisted.is_empty() && !persisted.contains("/init") && !persisted.contains("generated"),
        "generated /init prompt must not be persisted: {persisted}"
    );
    drop(dir);
}

#[tokio::test]
async fn slash_init_in_auto_opens_generic_preflight_without_starting_turn() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/init opens preflight");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("preflight overlay");
    assert!(matches!(overlay.kind, OverlayKind::ChoicePicker(_)));
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
}

#[tokio::test]
async fn multiple_required_preflight_skills_return_status_without_turn() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:self-evo /skill:create-skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash handled");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "Run one interactive skill workflow at a time"
    ));
}

#[tokio::test]
async fn init_preflight_switch_to_ask_starts_workflow() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm recommended option");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(
        prompt.contains("User instruction after /init: 参考 docs"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Interactive clarification is allowed"),
        "{prompt}"
    );
}

#[tokio::test]
async fn init_preflight_continue_keeps_auto_and_starts_best_effort() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select continue");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm continue");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(
        prompt.contains("Auto permission mode remained active"),
        "{prompt}"
    );
}

#[tokio::test]
async fn init_preflight_dialog_cancel_starts_no_workflow() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel preflight");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn slash_init_runs_one_repair_turn_when_agents_guide_validation_fails() {
    let dir = tempfile::tempdir().expect("temp dir");
    let agents_path = dir.path().join("AGENTS.md");
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let requests_clone = std::sync::Arc::clone(&requests);
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let agents_path_clone = agents_path.clone();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        dir.path().to_path_buf(),
        move |request| {
            let requests = std::sync::Arc::clone(&requests_clone);
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            let agents_path = agents_path_clone.clone();
            async move {
                let prompt = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                requests.lock().expect("requests lock").push(request);
                let turn = turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if prompt.contains("AGENTS.md structure validation failed") {
                    std::fs::write(
                        agents_path,
                        init_command::example_agents_guide_for_tests("abc1234"),
                    )
                    .expect("write repaired AGENTS");
                } else {
                    std::fs::write(
                        agents_path,
                        "## Reference\ntext\n\n## Metadata\nCreated: 2026-07-07\n",
                    )
                    .expect("write invalid AGENTS");
                }
                assert!(turn < 2, "only initial and repair turns should run");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/init");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("/init handled");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    let requests = requests.lock().expect("requests lock");
    assert_eq!(
        requests[1].prompt_origin,
        MessageOrigin::injection("init"),
        "repair turn must remain hidden injection"
    );
    assert!(transcript_has_status(
        &controller,
        "AGENTS.md structure validation passed after repair"
    ));
}
