//! Interactive themes behavior (moved from `tests.rs`).

use std::fs;

use neo_agent_core::{AgentEvent, AgentMessage, skills::SkillStore};
use neo_tui::{
    input::{InputEvent, KeybindingAction},
    shell::OverlayKind,
    transcript::TranscriptEntry,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn custom_theme_skill_activates_only_via_explicit_slash() {
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
                Ok(vec![AgentEvent::MessageAppended {
                    message: AgentMessage::user_text("theme work done".to_owned()),
                }])
            }
        },
    );
    let builtins = neo_agent_core::skills::builtin::builtin_skills().expect("built-in skills load");
    let custom_theme = builtins
        .iter()
        .find(|skill| skill.name == "custom-theme")
        .expect("custom-theme must be registered as a built-in");
    assert!(
        !custom_theme.manifest.auto_invokable(),
        "custom-theme must stay explicit-only"
    );
    controller.skill_store = Some(SkillStore::load(&[], &[], builtins));

    controller.type_text("/skill:custom-theme\nI want a dark theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("explicit skill activation succeeds");

    let entries = transcript_entries(&controller);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
            .count(),
        1,
        "explicit invocation should render exactly one semantic card"
    );
    assert!(matches!(
        entries.last(),
        Some(TranscriptEntry::SkillActivation {
            names,
            source: neo_agent_core::SkillInvocationSource::Manual,
            outcome: neo_agent_core::SkillInvocationOutcome::Activated,
            ..
        }) if names == &["custom-theme".to_owned()]
    ));

    controller
        .wait_for_active_turn()
        .await
        .expect("explicit skill turn completes");
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let skill_context = requests[0].skill_context.as_deref().expect("skill context");
    assert!(
        skill_context.contains("User activated the skill \"custom-theme\""),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("<neo-skill-loaded name=\"custom-theme\" source=\"builtin\""),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("ThemeDraft"),
        "activated custom-theme body must teach the ThemeDraft flow: {skill_context}"
    );
}

#[tokio::test]
async fn theme_slash_bare_opens_manager_overlay_when_idle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("submit /theme");
    assert!(
        matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::ThemeManager(_))
        ),
        "bare /theme must open the theme manager overlay"
    );
    assert_eq!(
        controller.chrome().prompt().text,
        "",
        "bare /theme must clear the submitted prompt"
    );
}

#[tokio::test]
async fn theme_slash_bare_manager_blocked_while_turn_runs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");

    let mut controller = busy_turn_controller(&project_dir);
    controller.type_text("busy turn");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start busy turn");
    tokio::task::yield_now().await;
    assert!(controller.active_turn.is_some());

    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("submit /theme while busy");
    assert!(
        controller.chrome().focused_overlay().is_none(),
        "the manager overlay must not open while a turn runs"
    );
    assert!(
        controller.active_turn.is_some(),
        "the running turn must stay intact"
    );
    assert_eq!(
        controller.chrome().prompt().text,
        "",
        "bare /theme must clear the submitted prompt even while busy"
    );
    assert!(
        transcript_has_status(&controller, "Finish or interrupt"),
        "the busy hint must name the idle requirement"
    );
    controller
        .cancel_active_turn()
        .await
        .expect("cancel busy turn");
}

#[tokio::test]
async fn theme_slash_direct_apply_resolves_id_and_unique_display_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");
    write_test_theme(&project_dir, "gruvbox.json", "Gruvbox", "#00ff00");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme solarized.json");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("apply by id");
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(255, 0, 0),
        "exact id resolution must apply the theme"
    );
    assert!(controller.chrome().focused_overlay().is_none());
    assert_eq!(
        controller.chrome().prompt().text,
        "",
        "applied slash clears the prompt"
    );

    controller.type_text("/theme Gruvbox");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("apply by display name");
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(0, 255, 0),
        "a unique exact display name must apply the theme"
    );
    assert!(transcript_has_status(&controller, "Theme applied"));
}

#[tokio::test]
async fn theme_slash_direct_apply_works_while_turn_runs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");

    let mut controller = busy_turn_controller(&project_dir);
    controller.type_text("busy turn");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start busy turn");
    tokio::task::yield_now().await;
    assert!(controller.active_turn.is_some());

    controller.type_text("/theme solarized.json");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("apply theme while busy");
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(255, 0, 0),
        "direct apply must work during a model turn"
    );
    assert!(
        controller.active_turn.is_some(),
        "the running turn must stay intact"
    );
    assert!(
        controller.chrome().focused_overlay().is_none(),
        "direct apply must not open the manager overlay"
    );
    controller
        .cancel_active_turn()
        .await
        .expect("cancel busy turn");
}

#[tokio::test]
async fn theme_slash_direct_apply_errors_are_local_and_side_effect_free() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Duplicate Name", "#ff0000");
    write_test_theme(&project_dir, "b.json", "Duplicate Name", "#00ff00");

    let mut controller = theme_controller_with_project(&project_dir);
    let before = controller.chrome().theme().brand;

    controller.type_text("/theme missing.json");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("unknown reference is a local error");
    assert_eq!(
        controller.chrome().theme().brand,
        before,
        "unknown id must not apply"
    );
    assert!(transcript_has_status(&controller, "Theme error"));

    controller.type_text("/theme Duplicate Name");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("ambiguous display name is a local error");
    assert_eq!(
        controller.chrome().theme().brand,
        before,
        "an ambiguous display name must never apply"
    );
    assert!(transcript_has_status(&controller, "ambiguous"));
    assert_eq!(
        controller.chrome().prompt().text,
        "",
        "the consumed slash clears the prompt"
    );
}

#[tokio::test]
async fn theme_slash_reload_clears_override_and_applies_config_theme() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");
    write_test_theme(&project_dir, "gruvbox.json", "Gruvbox", "#00ff00");
    fs::write(
        project_dir.join(".neo/config.toml"),
        "[tui]\ntheme = \"gruvbox.json\"\n",
    )
    .expect("write config theme");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme solarized.json");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("apply session override");
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(255, 0, 0)
    );

    controller.type_text("/theme reload");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("reload startup theme");
    assert!(
        controller.session_theme_override.is_none(),
        "reload must clear the session override"
    );
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(0, 255, 0),
        "reload must apply the resolved config theme"
    );
    assert!(transcript_has_status(&controller, "Theme reloaded"));
}

#[tokio::test]
async fn theme_slash_reload_reports_config_load_failure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");
    fs::write(project_dir.join(".neo/config.toml"), "\"unterminated").expect("write broken config");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme solarized.json");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("apply session override");
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(255, 0, 0)
    );

    controller.type_text("/theme reload");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("reload with a broken config");
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(255, 0, 0),
        "a failed reload must keep the current chrome theme instead of applying stale state"
    );
    assert!(
        transcript_has_status(&controller, "Theme reload failed"),
        "the reload failure must be reported accurately instead of claiming success"
    );
}

#[tokio::test]
async fn theme_slashish_and_embedded_prose_stay_normal_prompts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = theme_controller_with_project(&project_dir);

    assert!(
        !controller.handle_slash_command("/themeish").await,
        "/themeish must stay a normal prompt"
    );
    assert!(
        !controller
            .handle_slash_command("apply /theme solarized for me")
            .await,
        "embedded prose must stay a normal prompt"
    );
    assert!(
        !controller.handle_slash_command("/THEME").await,
        "uppercase /THEME must stay a normal prompt"
    );
    assert!(
        !controller.handle_slash_command("/theme:solarized").await,
        "a non-whitespace suffix must stay a normal prompt"
    );
}

#[tokio::test]
async fn theme_manager_apply_session_sets_chrome_and_override() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");
    write_test_theme(&project_dir, "b.json", "Beta", "#00ff00");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select the second theme");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("apply the selected theme");
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(0, 255, 0),
        "apply-session must set the chrome to the selected theme"
    );
    assert_eq!(
        controller
            .session_theme_override
            .as_ref()
            .map(|id| id.as_str()),
        Some("b.json"),
        "apply-session must record the override id"
    );
    assert!(transcript_has_status(&controller, "Theme applied"));
}

#[tokio::test]
async fn theme_manager_set_startup_default_persists_only_the_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");
    write_test_theme(&project_dir, "b.json", "Beta", "#00ff00");
    fs::write(project_dir.join(".neo/config.toml"), "model_scope = []\n").expect("config");

    let mut controller = theme_controller_with_project(&project_dir);
    let before = controller.chrome().theme().brand;
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('d'))
        .await
        .expect("set startup default on the selected theme");
    assert_eq!(
        controller.chrome().theme().brand,
        before,
        "set-startup-default must leave the current chrome unchanged"
    );
    assert!(
        controller.session_theme_override.is_none(),
        "set-startup-default must not create a session override"
    );
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));
    let config = fs::read_to_string(project_dir.join(".neo/config.toml")).expect("read config");
    assert!(
        config.contains("theme = \"a.json\""),
        "set-startup-default must persist only the logical id, got:\n{config}"
    );
    assert!(
        config.contains("model_scope = []"),
        "set-startup-default must leave unrelated config sections intact, got:\n{config}"
    );
    assert!(transcript_has_status(&controller, "Startup theme set"));
}

#[tokio::test]
async fn theme_manager_delete_removes_file_and_keeps_manager_open() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");
    write_test_theme(&project_dir, "b.json", "Beta", "#00ff00");
    write_test_theme(&project_dir, "c.json", "Gamma", "#0000ff");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select beta");
    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("begin delete");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm delete");
    assert!(
        !project_dir.join(".neo/themes/b.json").exists(),
        "delete must remove the theme file"
    );
    assert!(project_dir.join(".neo/themes/a.json").exists());
    assert!(project_dir.join(".neo/themes/c.json").exists());
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));
    assert_eq!(
        theme_manager_selected_id(&controller).as_deref(),
        Some("c.json"),
        "after a delete the selection must move to the nearest remaining stable item, not jump to the first entry"
    );
    assert!(transcript_has_status(&controller, "Theme deleted"));
}

#[tokio::test]
async fn theme_manager_import_conflict_asks_before_overwriting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");
    let source = temp.path().join("a.json");
    fs::write(
        &source,
        r##"{"name": "External", "colors": {"brand": "#0000ff"}}"##,
    )
    .expect("write external theme");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('i'))
        .await
        .expect("begin import");
    assert!(
        matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::TextInput(_))
        ),
        "the manager must hand off the import path dialog"
    );
    controller
        .handle_input_event(InputEvent::Paste(source.display().to_string()))
        .await
        .expect("paste import path");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit import path");
    assert!(
        matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::ChoicePicker(_))
        ),
        "a conflicting destination must ask overwrite vs save-as-new"
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("choose overwrite");
    let content =
        fs::read_to_string(project_dir.join(".neo/themes/a.json")).expect("read overwritten theme");
    assert!(
        content.contains("External"),
        "overwrite must replace the theme file, got:\n{content}"
    );
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));
    assert!(transcript_has_status(&controller, "Theme imported"));
}

#[tokio::test]
async fn theme_completion_offers_ids_and_display_names_without_mutation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme");
    controller.sync_inline_prompt_completion();
    let catalog = controller
        .slash_completion_catalog
        .as_ref()
        .expect("slash completion catalog");
    let values = catalog
        .theme_items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();
    assert!(
        values.contains(&"/theme solarized.json"),
        "completion must offer the theme id, got: {values:?}"
    );
    assert!(
        values.contains(&"/theme Solarized"),
        "completion must offer the display name, got: {values:?}"
    );
    let content =
        fs::read_to_string(project_dir.join(".neo/themes/solarized.json")).expect("theme file");
    assert!(
        content.contains("Solarized"),
        "completion must never mutate the repository"
    );
}

#[tokio::test]
async fn theme_manager_palette_command_opens_manager_overlay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");

    let mut controller = theme_controller_with_project(&project_dir);
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..48 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "theme.manager" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to the theme manager command");
    }
    let selected = controller
        .chrome()
        .selected_command()
        .expect("theme manager command");
    assert_eq!(selected.id, "theme.manager");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("theme manager command runs");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));
}

#[tokio::test]
async fn theme_manager_copy_duplicates_under_fresh_id_and_selects_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('c'))
        .await
        .expect("begin copy");
    assert!(
        matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::TextInput(_))
        ),
        "copy must hand off the display-name dialog"
    );
    controller
        .handle_input_event(InputEvent::Paste("Beta".to_owned()))
        .await
        .expect("paste display name");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit display name");

    let copy_path = project_dir.join(".neo/themes/Beta.json");
    let copy_content =
        fs::read_to_string(&copy_path).expect("the duplicate must be written to a fresh id");
    assert!(
        copy_content.contains("Beta"),
        "the duplicate must carry the new display name, got:\n{copy_content}"
    );
    let original =
        fs::read_to_string(project_dir.join(".neo/themes/a.json")).expect("original theme");
    assert!(
        original.contains("Alpha"),
        "the source theme must stay untouched, got:\n{original}"
    );
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));
    assert_eq!(
        theme_manager_selected_id(&controller).as_deref(),
        Some("Beta.json"),
        "the fresh duplicate must be selected after the rescan"
    );
    assert!(transcript_has_status(&controller, "Theme duplicated"));
}

#[tokio::test]
async fn theme_manager_delete_blocks_active_and_startup_default_themes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");
    write_test_theme(&project_dir, "b.json", "Beta", "#00ff00");

    // The currently active theme cannot be deleted.
    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme a.json");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("apply the active theme");
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("attempt delete on the active theme");
    let visible = theme_manager_overlay_text(&controller);
    assert!(
        visible.contains("active theme cannot be deleted"),
        "the manager must block deleting the active theme, got:\n{visible}"
    );
    assert!(
        project_dir.join(".neo/themes/a.json").exists(),
        "the active theme file must survive the blocked delete"
    );

    // The startup default cannot be deleted either.
    let mut controller = theme_controller_with_project(&project_dir);
    controller.local_config.as_mut().expect("config").tui.theme = Some("b.json".to_owned());
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select the startup default");
    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("attempt delete on the startup default");
    let visible = theme_manager_overlay_text(&controller);
    assert!(
        visible.contains("startup default cannot be deleted"),
        "the manager must block deleting the startup default, got:\n{visible}"
    );
    assert!(
        project_dir.join(".neo/themes/b.json").exists(),
        "the startup default file must survive the blocked delete"
    );
}

#[tokio::test]
async fn theme_manager_set_startup_default_write_failure_retains_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");
    // An unparseable config makes the set-startup-default mutation fail. The
    // manager snapshot reads in-memory config state, so it still opens.
    let config_path = project_dir.join(".neo/config.toml");
    fs::write(&config_path, "\"unterminated").expect("write broken config");

    let mut controller = theme_controller_with_project(&project_dir);
    let before = controller.chrome().theme().brand;
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('d'))
        .await
        .expect("set startup default on a failing config write");

    assert_eq!(
        controller.chrome().theme().brand,
        before,
        "a failed config write must leave the current runtime theme unchanged"
    );
    assert!(
        controller.session_theme_override.is_none(),
        "a failed config write must not create a session override"
    );
    assert_eq!(
        fs::read_to_string(&config_path).expect("config"),
        "\"unterminated",
        "a failed config write must leave the previous config bytes unchanged"
    );
    assert!(
        controller
            .local_config
            .as_ref()
            .expect("config")
            .tui
            .theme
            .is_none(),
        "the in-memory config must not be updated on write failure"
    );
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));
    assert!(
        transcript_has_status(&controller, "Failed to set startup theme"),
        "the write failure must surface a retryable error without crashing"
    );
}

#[tokio::test]
async fn theme_manager_refresh_keeps_stable_selection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");
    write_test_theme(&project_dir, "b.json", "Beta", "#00ff00");
    write_test_theme(&project_dir, "c.json", "Gamma", "#0000ff");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select beta");
    controller
        .handle_input_event(InputEvent::Insert('r'))
        .await
        .expect("refresh unchanged catalog");
    assert_eq!(
        theme_manager_selected_id(&controller).as_deref(),
        Some("b.json"),
        "refresh must keep the previously selected id when it still exists"
    );

    // The selected theme disappears from disk; the next refresh must pick the
    // stable neighbor at the previous position instead of jumping to first.
    fs::remove_file(project_dir.join(".neo/themes/b.json")).expect("remove beta");
    controller
        .handle_input_event(InputEvent::Insert('r'))
        .await
        .expect("refresh after external removal");
    assert_eq!(
        theme_manager_selected_id(&controller).as_deref(),
        Some("c.json"),
        "a vanished selection must fall back to the nearest remaining stable item"
    );
}

#[tokio::test]
async fn theme_manager_import_save_as_new_keeps_existing_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "a.json", "Alpha", "#ff0000");
    let source = temp.path().join("a.json");
    fs::write(
        &source,
        r##"{"name": "External", "colors": {"brand": "#0000ff"}}"##,
    )
    .expect("write external theme");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('i'))
        .await
        .expect("begin import");
    controller
        .handle_input_event(InputEvent::Paste(source.display().to_string()))
        .await
        .expect("paste import path");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit import path");
    assert!(
        matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::ChoicePicker(_))
        ),
        "a conflicting destination must ask before any write"
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("choose save as new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm save as new");

    let original =
        fs::read_to_string(project_dir.join(".neo/themes/a.json")).expect("original theme");
    assert!(
        original.contains("Alpha"),
        "save-as-new must not overwrite the existing theme, got:\n{original}"
    );
    let saved =
        fs::read_to_string(project_dir.join(".neo/themes/a-1.json")).expect("fresh import file");
    assert!(
        saved.contains("External"),
        "the imported theme must land under a fresh id, got:\n{saved}"
    );
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));
    assert_eq!(
        theme_manager_selected_id(&controller).as_deref(),
        Some("a-1.json"),
        "the fresh import must be selected after the rescan"
    );
    assert!(transcript_has_status(&controller, "Theme imported"));
}
