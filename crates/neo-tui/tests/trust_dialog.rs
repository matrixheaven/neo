use neo_tui::dialogs::{
    TrustDialogChoice, TrustDialogData, TrustDialogInput, TrustDialogInputKind, TrustDialogResult,
    TrustDialogState,
};
use neo_tui::input::{InputEvent, KeybindingAction};
use neo_tui::shell::{NeoChromeState, Overlay, OverlayKind};
use std::path::PathBuf;

fn sample_data() -> TrustDialogData {
    TrustDialogData {
        current_dir: PathBuf::from("/Users/me/src/acme/tools/cli"),
        detected: vec![
            TrustDialogInput {
                path: PathBuf::from("/Users/me/src/acme/tools/cli/AGENTS.md"),
                kind: TrustDialogInputKind::ContextFile,
            },
            TrustDialogInput {
                path: PathBuf::from("/Users/me/src/acme/tools/cli/.neo"),
                kind: TrustDialogInputKind::NeoDir,
            },
            TrustDialogInput {
                path: PathBuf::from("/Users/me/src/acme/tools/cli/.agents/skills"),
                kind: TrustDialogInputKind::AgentsSkillsDir,
            },
        ],
        parent_candidates: vec![
            PathBuf::from("/Users/me/src/acme/tools"),
            PathBuf::from("/Users/me/src/acme"),
        ],
    }
}

fn render_lines(state: &TrustDialogState, width: usize) -> Vec<String> {
    state
        .render_lines(width)
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect()
}

#[test]
fn trust_dialog_defaults_to_continue_untrusted() {
    let state = TrustDialogState::new(sample_data(), neo_tui::primitive::theme::TuiTheme::default());
    assert_eq!(
        state.selected_choice(),
        TrustDialogChoice::ContinueUntrusted
    );
}

#[test]
fn trust_dialog_renders_detected_inputs_without_file_contents() {
    let state = TrustDialogState::new(sample_data(), neo_tui::primitive::theme::TuiTheme::default());
    let lines = render_lines(&state, 80);
    let text = lines.join("\n");

    assert!(text.contains("Workspace Trust"), "{text}");
    assert!(text.contains("Directory"), "{text}");
    assert!(text.contains("/Users/me/src/acme/tools/cli"), "{text}");
    assert!(text.contains("Detected"), "{text}");
    assert!(text.contains("AGENTS.md"), "{text}");
    assert!(text.contains(".neo/"), "{text}");
    assert!(text.contains(".agents/skills/"), "{text}");
    assert!(
        !text.contains("injected prompt text"),
        "must not render file contents: {text}"
    );
    assert!(text.contains("Continue untrusted"), "{text}");
}

#[test]
fn trust_dialog_renders_parent_selection_step() {
    let mut state = TrustDialogState::new(sample_data(), neo_tui::primitive::theme::TuiTheme::default());

    // Move up once from ContinueUntrusted to TrustParent.
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectUp));
    assert_eq!(state.selected_choice(), TrustDialogChoice::TrustParent);

    // With multiple candidates, Enter enters the parent selection step.
    let result = state.handle_input(&InputEvent::Submit);
    assert_eq!(
        result,
        neo_tui::primitive::InputResult::Handled,
        "multiple parent candidates should open selection step"
    );

    let lines = render_lines(&state, 80);
    let text = lines.join("\n");
    assert!(text.contains("Trust Parent Directory"), "{text}");
    assert!(text.contains("/Users/me/src/acme/tools"), "{text}");
    assert!(text.contains("/Users/me/src/acme"), "{text}");
    assert!(text.contains("Esc back"), "{text}");
}

#[test]
fn trust_dialog_hides_prompt_when_focused() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut()
        .apply_edit(neo_tui::shell::PromptEdit::Insert("draft prompt"));
    app.push_overlay(Overlay::new(
        "trust",
        OverlayKind::TrustDialog(TrustDialogState::new(
            sample_data(),
            neo_tui::primitive::theme::TuiTheme::default(),
        )),
    ));

    let mut tui = neo_tui::NeoTui::new(app, neo_tui::transcript::TranscriptPane::new(80, 24));
    let (lines, cursor) = tui.render_frame(80, 24);
    let frame = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(frame.contains("Workspace Trust"), "{frame}");
    assert!(
        !frame.contains("> draft prompt"),
        "composer should be hidden while trust dialog is focused: {frame}"
    );
    assert!(
        cursor.is_none(),
        "blocking trust dialog should not expose prompt cursor"
    );
}

#[test]
fn trust_dialog_routes_input_through_chrome() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    let id = app.open_trust_dialog(sample_data());

    // Digit 3 selects Continue untrusted; Enter confirms.
    let result = app.handle_focused_dialog_input(InputEvent::Insert('3'));
    assert_eq!(result, neo_tui::primitive::InputResult::Handled);
    let result = app.handle_focused_dialog_input(InputEvent::Submit);
    assert_eq!(result, neo_tui::primitive::InputResult::Submitted);

    match app.take_trust_dialog_result() {
        Some(TrustDialogResult::Untrusted { target }) => {
            assert_eq!(target, PathBuf::from("/Users/me/src/acme/tools/cli"));
        }
        other => panic!("expected Untrusted result, got {other:?}"),
    }
    let _ = app.close_overlay(id);
}

#[test]
fn trust_dialog_cancel_chooses_untrusted() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.open_trust_dialog(sample_data());

    let result = app.handle_focused_dialog_input(InputEvent::Cancel);
    assert_eq!(result, neo_tui::primitive::InputResult::Cancelled);

    match app.take_trust_dialog_result() {
        Some(TrustDialogResult::Untrusted { target }) => {
            assert_eq!(target, PathBuf::from("/Users/me/src/acme/tools/cli"));
        }
        other => panic!("expected Untrusted result, got {other:?}"),
    }
}

#[test]
fn trust_dialog_single_parent_trusts_directly() {
    let data = TrustDialogData {
        current_dir: PathBuf::from("/Users/me/src/acme/tools/cli"),
        detected: vec![TrustDialogInput {
            path: PathBuf::from("/Users/me/src/acme/tools/cli/AGENTS.md"),
            kind: TrustDialogInputKind::ContextFile,
        }],
        parent_candidates: vec![PathBuf::from("/Users/me/src/acme")],
    };
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.open_trust_dialog(data);

    app.handle_focused_dialog_input(InputEvent::Insert('2'));
    let result = app.handle_focused_dialog_input(InputEvent::Submit);
    assert_eq!(result, neo_tui::primitive::InputResult::Submitted);

    match app.take_trust_dialog_result() {
        Some(TrustDialogResult::Trust { target }) => {
            assert_eq!(target, PathBuf::from("/Users/me/src/acme"));
        }
        other => panic!("expected Trust result, got {other:?}"),
    }
}
