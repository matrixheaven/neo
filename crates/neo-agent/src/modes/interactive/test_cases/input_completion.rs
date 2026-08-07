//! Slash/prompt completion behavior (split from `input.rs`).

use std::{collections::BTreeMap, fs};

use neo_agent_core::{
    AgentEvent, PermissionMode,
    skills::{
        LoadedSkill, SkillHostMetadata, SkillInterface, SkillManifest, SkillSource, SkillStore,
    },
};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::OverlayKind,
};

use super::super::snapshot::render_overlay_snapshot;
use super::super::*;
use super::*;

#[tokio::test]
async fn event_loop_tabs_through_real_filesystem_prompt_completions() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("src")).expect("create src");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main");
    fs::write(temp.path().join("src/matrix.rs"), "pub fn matrix() {}\n").expect("write matrix");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.completion_root = temp.path().to_path_buf();

    controller.type_text("open src/ma");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab opens completion picker");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("completion confirms");

    assert_eq!(controller.chrome().prompt().text, "open src/main.rs");
    assert_eq!(controller.chrome().prompt().cursor, 16);
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_opens_slash_completion_after_typing_slash() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("slash insert opens completion");

    assert_eq!(controller.chrome().prompt().text, "/");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));
    assert!(
        controller.chrome().selected_prompt_completion().is_some(),
        "slash completion should select the first local command"
    );
}

#[tokio::test]
async fn event_loop_opens_slash_completion_after_whitespace() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("foo ");
    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("inline slash insert opens completion");

    assert_eq!(controller.chrome().prompt().text, "foo /");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));
}

#[tokio::test]
async fn slash_completion_includes_btw_command() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    for ch in "/btw".chars() {
        controller
            .handle_input_event(InputEvent::Insert(ch))
            .await
            .expect("typing slash command updates completion");
    }

    let rendered = controller.chrome().focused_overlay_lines(80).join("\n");
    assert!(
        rendered.contains("/btw"),
        "slash completion should include /btw; got:\n{rendered}"
    );
}

#[tokio::test]
async fn slash_completion_refreshes_skills_from_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let extra_skills = temp.path().join("extra-skills");
    fs::create_dir_all(&extra_skills).expect("create extra skills");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.extra_skill_dirs = vec![extra_skills.to_string_lossy().into_owned()];

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);

    let skill_dir = extra_skills.join("fresh-skill");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fresh-skill\ndescription: Fresh from disk\n---\n\nUse me.",
    )
    .expect("write skill");

    for ch in "/skill:f".chars() {
        controller
            .handle_input_event(InputEvent::Insert(ch))
            .await
            .expect("typing skill prefix updates completion");
    }

    let rendered = render_overlay_snapshot(controller.chrome(), 100).join("\n");
    assert!(
        rendered.contains("/skill:fresh-skill"),
        "slash completion should include freshly reloaded skill; got:\n{rendered}"
    );
}

#[tokio::test]
async fn event_loop_backspace_deletes_slash_while_completion_is_open() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("slash insert opens completion");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
        .await
        .expect("backspace edits prompt");

    assert_eq!(controller.chrome().prompt().text, "");
    assert_eq!(controller.chrome().prompt().cursor, 0);
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_escape_closes_slash_completion_without_exiting() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("slash insert opens completion");
    let should_exit = controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("escape closes completion");

    assert!(!should_exit);
    assert_eq!(controller.chrome().prompt().text, "/");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_tabs_through_local_slash_prompt_template_completions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(&prompts_dir).expect("create prompts");
    fs::write(
        prompts_dir.join("review.md"),
        "---\ndescription: Review the current change\n---\nReview this change.\n",
    )
    .expect("write review prompt");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.completion_root = temp.path().to_path_buf();

    controller.type_text("/rev");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab opens slash prompt picker");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab completes slash prompt");

    assert_eq!(controller.chrome().prompt().text, "/review");
    assert_eq!(controller.chrome().prompt().cursor, 7);
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn tab_confirms_selected_prompt_completion() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("/");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab opens completion picker");

    assert!(controller.chrome().focused_overlay().is_some());
    assert!(controller.chrome().selected_prompt_completion().is_some());

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab confirms selected completion");

    assert!(controller.chrome().focused_overlay().is_none());
    assert!(!controller.chrome().prompt().text.is_empty());
}

#[test]
fn prompt_completions_merges_real_prompt_package_and_session_commands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(prompts_dir.join("review-pack")).expect("create prompts");
    fs::write(
        prompts_dir.join("review.md"),
        "---\ndescription: Review local changes\n---\nReview $1.\n",
    )
    .expect("write local prompt");
    fs::write(
        prompts_dir.join("review-pack/refactor.md"),
        "---\ndescription: Refactor from package\n---\nRefactor $1.\n",
    )
    .expect("write packaged prompt");

    let completions = prompt_completions(temp.path(), "/", None, true).expect("slash completions");
    let by_value = completions
        .iter()
        .map(|item| (item.value.as_str(), item))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(
        by_value["/review"].description.as_deref(),
        Some("Review local changes")
    );
    assert_eq!(
        by_value["/refactor"].description.as_deref(),
        Some("Refactor from package")
    );
    assert_eq!(
        by_value["/resume"].description.as_deref(),
        Some("Resume a local session")
    );
    assert_eq!(
        by_value["/sessions"].description.as_deref(),
        Some("Alias for /resume")
    );
    for item in by_value.values() {
        let description = item.description.as_deref().unwrap_or_default();
        assert!(!description.contains("source:"));
        assert!(!description.contains("provider:"));
        assert!(!description.contains("trust:"));
    }
    assert!(!by_value.contains_key("/tree"));
    assert!(!by_value.contains_key("/sync"));
}

#[tokio::test]
async fn slash_completion_no_match_keeps_the_current_catalog() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(&prompts_dir).expect("create prompts");
    fs::write(prompts_dir.join("alpha.md"), "alpha").expect("write alpha");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("open completion");
    controller
        .handle_input_event(InputEvent::Insert('z'))
        .await
        .expect("hide unmatched completion");
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(controller.slash_completion_catalog.is_some());
    fs::write(prompts_dir.join("zzz.md"), "zzz").expect("write zzz");

    controller
        .handle_input_event(InputEvent::Insert('z'))
        .await
        .expect("continue unmatched completion");

    assert!(controller.chrome().focused_overlay().is_none());
    assert!(
        controller
            .slash_completion_catalog
            .as_ref()
            .expect("catalog remains loaded")
            .slash_prompts
            .iter()
            .all(|item| item.value != "/zzz")
    );
}

#[tokio::test]
async fn escape_ends_the_slash_completion_catalog_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(&prompts_dir).expect("create prompts");
    fs::write(prompts_dir.join("first.md"), "first").expect("write first");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async { Ok(Vec::<AgentEvent>::new()) },
    );
    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("open completion");

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel completion");

    assert!(controller.chrome().focused_overlay().is_none());
    assert!(controller.slash_completion_catalog.is_none());
    fs::write(prompts_dir.join("second.md"), "second").expect("write second");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("open next completion session");
    assert!(
        controller
            .slash_completion_catalog
            .as_ref()
            .expect("new catalog loaded")
            .slash_prompts
            .iter()
            .any(|item| item.value == "/second")
    );
}

#[test]
fn slash_completion_descriptions_hide_internal_metadata() {
    let completions =
        prompt_completions(&test_workspace_root(), "/ask", None, true).expect("slash completions");
    let ask = completions
        .iter()
        .find(|item| item.value == "/ask")
        .expect("missing /ask completion");

    assert_eq!(ask.label, "/ask");
    assert_eq!(ask.description.as_deref(), Some("ask permission mode"));
    let description = ask.description.as_deref().unwrap_or_default();
    assert!(!description.contains("provider:"));
    assert!(!description.contains("trust:"));
    assert!(!description.contains("source:"));
}

#[test]
fn slash_completions_include_dynamic_skill_commands_without_metadata() {
    let skill_store = skill_store_with_refactor_skill();
    let completions =
        prompt_completions(&test_workspace_root(), "/skill:", Some(&skill_store), true)
            .expect("skill completions resolve");
    let skill = completions
        .iter()
        .find(|item| item.value == "/skill:refactor")
        .expect("missing dynamic skill command");

    assert_eq!(skill.label, "/skill:refactor");
    assert_eq!(
        skill.description.as_deref(),
        Some("Refactor with project conventions")
    );
    let description = skill.description.as_deref().unwrap_or_default();
    assert!(!description.contains("provider:"), "{description}");
    assert!(!description.contains("trust:"), "{description}");
    assert!(!description.contains("source:"), "{description}");
}

#[tokio::test]
async fn completion_keeps_full_skill_command_and_uses_host_description_fallback() {
    let skill_store = SkillStore::load(
        &[],
        &[],
        vec![LoadedSkill {
            name: "schema-review".to_owned(),
            root: test_workspace_root().join("schema-review"),
            manifest: SkillManifest {
                name: "schema-review".to_owned(),
                description: "Manifest fallback".to_owned(),
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::new(),
            },
            body: "Review schemas.".to_owned(),
            source: SkillSource::User,
            host_metadata: SkillHostMetadata {
                interface: Some(SkillInterface {
                    display_name: Some("Schema Review".to_owned()),
                    short_description: None,
                }),
                dependencies: Vec::new(),
            },
        }],
    );
    let completions = prompt_completions(
        &test_workspace_root(),
        "/skill:schema",
        Some(&skill_store),
        true,
    )
    .expect("skill completions resolve");
    let skill = completions
        .iter()
        .find(|item| item.value == "/skill:schema-review")
        .expect("missing host-labelled skill command");

    assert_eq!(skill.value, "/skill:schema-review");
    assert_eq!(skill.label, "/skill:schema-review");
    assert_eq!(
        skill.description.as_deref(),
        Some("Schema Review: Manifest fallback")
    );

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.skill_store = Some(skill_store);
    controller.type_text("/skill:schema");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab completes canonical skill command");

    assert_eq!(controller.chrome().prompt().text, "/skill:schema-review");
}

#[test]
fn slash_completions_include_help_command() {
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("completions resolve");
    let help = completions
        .iter()
        .find(|item| item.value == "/help")
        .expect("missing /help completion");

    assert_eq!(help.label, "/help");
    assert_eq!(help.description.as_deref(), Some("Show help information"));
}

#[test]
fn slash_completions_include_init_command() {
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("slash completions");
    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();

    assert!(values.contains(&"/init"), "missing /init: {values:?}");
}

#[test]
fn completion_catalog_excludes_extension_commands() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("src")).expect("create src");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main");

    let catalog = CompletionCatalog {
        slash_prompts: vec![PickerItem::new(
            "/review",
            "/review",
            Some("Review project changes"),
        )],
        prompt_packages: vec![PickerItem::new(
            "/review-package",
            "/review-package",
            Some("Packaged review prompt"),
        )],
        session_commands: vec![PickerItem::new(
            "/review-session",
            "/review-session",
            Some("Session command"),
        )],
        theme_items: Vec::new(),
    };

    let files =
        completion_source_candidates(temp.path(), "src/ma", &catalog).expect("file completions");
    assert!(files.iter().any(|candidate| {
        candidate.value == "src/main.rs" && candidate.source == CompletionSource::LocalFile
    }));

    let slash =
        completion_source_candidates(temp.path(), "/rev", &catalog).expect("slash completions");
    let slash_sources = slash
        .iter()
        .map(|candidate| candidate.source)
        .collect::<Vec<_>>();
    assert!(slash_sources.contains(&CompletionSource::SlashPrompt));
    assert!(slash_sources.contains(&CompletionSource::PromptPackage));
    assert!(slash_sources.contains(&CompletionSource::SessionCommand));
    assert!(slash.iter().all(|candidate| {
        candidate
            .to_picker_item()
            .description
            .as_deref()
            .is_none_or(|description| !description.contains("extension command"))
    }));

    let file_references =
        completion_source_candidates(temp.path(), "@anth", &catalog).expect("file references");
    assert!(
        file_references
            .iter()
            .all(|candidate| candidate.value != "@anthropic/claude-sonnet")
    );
    assert!(
        file_references
            .iter()
            .all(|candidate| candidate.source == CompletionSource::FileReference)
    );
}

#[test]
fn slash_fuzzy_completions_keep_empty_query_order() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/", &catalog);

    assert_eq!(
        values[..5],
        ["/review", "/review-package", "/resume", "/new", "/clear"],
        "empty slash query keeps source order and curated command order"
    );
}

#[test]
fn slash_fuzzy_completions_rank_prefix_before_fuzzy() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/m", &catalog);

    let model_index = values
        .iter()
        .position(|value| value == "/model")
        .expect("/model present");
    let permissions_index = values
        .iter()
        .position(|value| value == "/permissions")
        .expect("/permissions present as a weaker fuzzy match");

    assert!(
        model_index < permissions_index,
        "prefix match /model should rank before fuzzy-only /permissions"
    );
}

#[test]
fn slash_fuzzy_completions_match_command_abbreviations() {
    let catalog = slash_test_catalog();

    assert_eq!(
        slash_values_for("/mdl", &catalog).first(),
        Some(&"/model".to_owned())
    );
    assert_eq!(
        slash_values_for("/prv", &catalog).first(),
        Some(&"/provider".to_owned())
    );
    assert_eq!(
        slash_values_for("/perm", &catalog).first(),
        Some(&"/permissions".to_owned())
    );
}

#[test]
fn slash_fuzzy_completions_match_skill_name_without_skill_prefix() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/code", &catalog);

    assert_eq!(
        values.first(),
        Some(&"/skill:code-simplifier".to_owned()),
        "skill commands should be searchable by skill name without typing /skill:"
    );
}

#[test]
fn slash_fuzzy_completions_match_prompt_templates() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/rvw", &catalog);

    assert_eq!(values.first(), Some(&"/review".to_owned()));
}

#[test]
fn slash_fuzzy_completions_return_empty_for_miss() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/zzzznotacommand", &catalog);

    assert!(values.is_empty());
}

#[test]
fn slash_completions_include_permission_commands() {
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("completions resolve");
    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();
    assert!(
        values.contains(&"/permissions"),
        "missing /permissions: {values:?}"
    );
    assert!(values.contains(&"/ask"), "missing /ask: {values:?}");
    assert!(values.contains(&"/auto"), "missing /auto: {values:?}");
    assert!(values.contains(&"/yolo"), "missing /yolo: {values:?}");
}

#[test]
fn slash_completions_include_compact_command() {
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("completions resolve");
    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();
    assert!(values.contains(&"/compact"), "missing /compact: {values:?}");
}

#[test]
fn slash_completions_include_add_workspace_command() {
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("completions resolve");
    let add_workspace = completions
        .iter()
        .find(|item| item.value == "/add-workspace")
        .expect("missing /add-workspace completion");

    assert_eq!(add_workspace.label, "/add-workspace");
    assert_eq!(
        add_workspace.description.as_deref(),
        Some("Manage additional workspace directories")
    );
}

#[test]
fn slash_completions_include_new_clear_and_workflow() {
    let items = super::prompt_completion::session_completion_items(None);
    let values: Vec<&str> = items.iter().map(|item| item.value.as_str()).collect();
    assert!(values.contains(&"/new"), "completions include /new");
    assert!(values.contains(&"/clear"), "completions include /clear");
    assert!(
        values.contains(&"/workflow"),
        "completions include /workflow"
    );
}

#[test]
fn slash_completions_include_effective_workflows_in_colon_form() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let items = super::prompt_completion::session_completion_items_with_registry(
        None,
        Some(&config.workflow_definitions),
    );
    let workflows: Vec<_> = items
        .iter()
        .filter(|item| item.value.starts_with("/workflow:"))
        .map(|item| (item.value.as_str(), item.description.as_deref()))
        .collect();
    assert_eq!(
        workflows,
        vec![("/workflow:demo", Some("Demo: named slash fixture"))]
    );
}

#[test]
fn slash_completions_remove_space_form_workflows() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let items = super::prompt_completion::session_completion_items_with_registry(
        None,
        Some(&config.workflow_definitions),
    );
    assert!(
        items
            .iter()
            .all(|item| !item.value.starts_with("/workflow "))
    );
}
