use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResponse,
    PermissionOperation,
};
use neo_tui::input::{InputEvent, KeyId, KeybindingAction};
use neo_tui::primitive::theme::ChromeMode;
use neo_tui::shell::{
    CommandPaletteState, CommandSpec, NeoChromeState, Overlay, OverlayKind, PromptEdit,
    ThemeCatalogEntrySnapshot,
};
use neo_tui::tasks_browser::{
    TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot, TaskBrowserState, TaskBrowserStatus,
};
use neo_tui::transcript::{TranscriptPane, render_chrome_lines};
use std::path::PathBuf;

fn render_app(width: u16, app: &NeoChromeState) -> Vec<String> {
    render_chrome_lines(app, usize::from(width), 30)
        .lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect()
}
fn strip_lines(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect()
}
fn task_browser_item(id: &str, status: TaskBrowserStatus) -> TaskBrowserItem {
    TaskBrowserItem {
        id: id.to_owned(),
        kind: TaskBrowserKind::Bash,
        status,
        title: "cargo test".to_owned(),
        description: "cargo test".to_owned(),
        elapsed: "00:05".to_owned(),
        detail_lines: vec![format!("id:          {id}")],
        preview_lines: vec!["running tests".to_owned()],
        can_stop: status.is_active(),
        human_handle: None,
        list_cursor: None,
        workflow: None,
    }
}
fn background_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
        },
        options: vec![
            ApprovalOption {
                label: "Approve once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::Reject,
            },
        ],

        workflow_origin: None,
    }
}
fn plan_revision_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: PermissionOperation::PlanTransition,
        presentation: ApprovalPresentation::Plan {
            title: "Plan Review".to_owned(),
            path: None,
            markdown: "Ready?".to_owned(),
            summary: Some("Ready?".to_owned()),
        },
        options: vec![
            ApprovalOption {
                label: "Approve".to_owned(),
                description: None,
                action: ApprovalAction::ApprovePlan { selection: None },
            },
            ApprovalOption {
                label: "Suggestion: Keep 85% window".to_owned(),
                description: Some("Keep compaction at 85%.".to_owned()),
                action: ApprovalAction::RevisePlan {
                    preset_feedback: Some("Keep compaction at 85%.".to_owned()),
                },
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::RejectPlan,
            },
            ApprovalOption {
                label: "Reject with feedback".to_owned(),
                description: None,
                action: ApprovalAction::RevisePlan {
                    preset_feedback: None,
                },
            },
        ],

        workflow_origin: None,
    }
}
fn complete_plan_revision(app: &mut NeoChromeState) -> ApprovalResponse {
    // First confirm enters editing with preset.
    assert!(
        app.handle_pending_approval_input(InputEvent::Submit)
            .is_none(),
        "first Enter on revision enters feedback editing"
    );
    let (_, _, feedback, collecting) = app.approval_selection().expect("pending");
    assert!(collecting);
    assert_eq!(feedback, "Keep compaction at 85%.");
    // Edit the preset, then submit.
    app.handle_pending_approval_input(InputEvent::Insert(' '));
    app.handle_pending_approval_input(InputEvent::Insert('x'));
    app.handle_pending_approval_input(InputEvent::Submit)
        .expect("Enter after feedback submits")
}

#[test]
fn api_key_dialog_paste_then_submit_closes_overlay_with_result() {
    use neo_tui::dialogs::{ApiKeyInputOptions, ApiKeyInputResult};
    use neo_tui::input::{InputEvent, KeybindingAction};

    let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
    app.open_api_key_input(ApiKeyInputOptions {
        title: "API Key".into(),
        provider_name: "minimax-cn-coding-plan".into(),
    });
    assert!(app.focused_overlay_is_rich_dialog());

    // Paste a long key (the scenario that used to crash / be ignored).
    let result = app.handle_focused_dialog_input(InputEvent::Paste(
        "sk-minimax-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
    ));
    assert_eq!(result, neo_tui::primitive::InputResult::Handled);

    // Render at a narrow width to ensure the masked field does not overflow.
    let _ = app.focused_overlay_lines(60);

    // The keybinding layer delivers Enter as `Action(SelectConfirm)` for
    // focused overlays (see `OVERLAY_ACTION_PRIORITY`). The dialog translate
    // layer must normalize it back to Submit.
    let result =
        app.handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectConfirm));
    assert_eq!(
        result,
        neo_tui::primitive::InputResult::Submitted,
        "SelectConfirm (Enter) must submit the API key dialog"
    );

    // The dialog must expose the submitted result while still focused.
    match app.api_key_input_result() {
        Some(ApiKeyInputResult::Submitted(v)) => {
            assert_eq!(v, "sk-minimax-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        }
        other => panic!("expected Submitted result, got {other:?}"),
    }

    // Likewise Esc arrives as `Action(SelectCancel)` and must cancel.
    app.open_api_key_input(ApiKeyInputOptions {
        title: "API Key".into(),
        provider_name: "p".into(),
    });
    let result =
        app.handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectCancel));
    assert_eq!(result, neo_tui::primitive::InputResult::Cancelled);
    assert!(matches!(
        app.api_key_input_result(),
        Some(ApiKeyInputResult::Cancelled)
    ));
}

#[test]
fn approval_digits_while_editing_feedback_do_not_reselect_options() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    // Two adjacent revise options so arrow can retarget the editor without
    // landing on a non-revise option in between.
    app.push_approval(ApprovalRequest {
        turn: 1,
        id: "exit-plan-digits".to_owned(),
        operation: PermissionOperation::PlanTransition,
        presentation: ApprovalPresentation::Plan {
            title: "Plan Review".to_owned(),
            path: None,
            markdown: "Ready?".to_owned(),
            summary: Some("Ready?".to_owned()),
        },
        options: vec![
            ApprovalOption {
                label: "Approve".to_owned(),
                description: None,
                action: ApprovalAction::ApprovePlan { selection: None },
            },
            ApprovalOption {
                label: "Suggestion A".to_owned(),
                description: None,
                action: ApprovalAction::RevisePlan {
                    preset_feedback: Some("Keep 85%.".to_owned()),
                },
            },
            ApprovalOption {
                label: "Suggestion B".to_owned(),
                description: None,
                action: ApprovalAction::RevisePlan {
                    preset_feedback: Some("Keep 70%.".to_owned()),
                },
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::RejectPlan,
            },
        ],

        workflow_origin: None,
    });

    // Select suggestion A (index 1) and enter feedback editing.
    assert!(app.choose_approval_number(2).is_none());
    let (_, selected, feedback, collecting) = app.approval_selection().expect("pending");
    assert_eq!(selected, 1);
    assert!(collecting);
    assert_eq!(feedback, "Keep 85%.");

    // Digit that is a valid option index must append to feedback, not re-select
    // or submit.
    assert!(
        app.handle_pending_approval_input(InputEvent::Insert('3'))
            .is_none(),
        "digit while editing must not resolve approval"
    );
    let (_, selected, feedback, collecting) = app.approval_selection().expect("still pending");
    assert_eq!(
        selected, 1,
        "selection must not change on digit while editing"
    );
    assert!(collecting);
    assert_eq!(feedback, "Keep 85%.3");

    // Arrow while collecting onto another revise re-seeds that option's preset.
    app.handle_pending_approval_input(InputEvent::Action(KeybindingAction::SelectDown));
    let (_, selected, feedback, collecting) = app.approval_selection().expect("pending");
    assert_eq!(selected, 2);
    assert!(
        collecting,
        "landing on another revise keeps the editor open"
    );
    assert_eq!(
        feedback, "Keep 70%.",
        "must re-seed from the newly selected revise preset, not carry prior text"
    );

    // Arrow onto a non-revise option exits the editor.
    app.handle_pending_approval_input(InputEvent::Action(KeybindingAction::SelectDown));
    let (_, selected, feedback, collecting) = app.approval_selection().expect("pending");
    assert_eq!(selected, 3);
    assert!(
        !collecting,
        "non-revise selection must exit feedback editing"
    );
    assert!(feedback.is_empty());
}

#[test]
fn approval_selection_returns_the_visible_option_action() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    let request = background_request();
    app.push_approval(request.clone());
    app.handle_pending_approval_input(InputEvent::Key(KeyId::new("down").unwrap()));

    let mut transcript = TranscriptPane::new(80, 20);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested { request });
    let mut tui = neo_tui::NeoTui::new(app, transcript);

    let (lines, _cursor) = tui.render_frame(80, 20);
    let frame = strip_lines(lines).join("\n");
    assert!(frame.contains("2. Reject"), "frame: {frame}");
    assert_eq!(
        tui.chrome()
            .approval_selection()
            .map(|(_, selected, ..)| selected),
        Some(1)
    );

    let response = tui
        .chrome_mut()
        .handle_pending_approval_input(InputEvent::Key(KeyId::new("enter").unwrap()))
        .expect("Enter resolves visible Reject");
    assert!(matches!(
        response,
        ApprovalResponse::Selected {
            action: ApprovalAction::Reject,
            feedback: None,
            ..
        }
    ));
}

#[test]
fn blocking_question_dialog_hides_composer_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut().apply_edit(PromptEdit::Insert("draft"));
    app.push_question_overlay(
        "question-1",
        vec![neo_tui::dialogs::QuestionDisplayData {
            question: "Pick one".to_owned(),
            header: Some("Question".to_owned()),
            body: None,
            options: vec![neo_tui::dialogs::QuestionDisplayOption {
                label: "Yes".to_owned(),
                description: None,
            }],
            multi_select: false,
        }],
    );

    let mut transcript = TranscriptPane::new(80, 20);
    // The transcript card is the single visible owner of the question; the
    // chrome overlay keeps the runtime selection state.
    transcript.upsert_question_prompt(
        "question-1",
        vec![neo_tui::dialogs::QuestionDisplayData {
            question: "Pick one".to_owned(),
            header: Some("Question".to_owned()),
            body: None,
            options: vec![neo_tui::dialogs::QuestionDisplayOption {
                label: "Yes".to_owned(),
                description: None,
            }],
            multi_select: false,
        }],
    );
    let mut tui = neo_tui::NeoTui::new(app, transcript);
    let (lines, cursor) = tui.render_frame(80, 20);
    let frame = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        frame.contains("question"),
        "question dialog must be visible: {frame}"
    );
    assert!(
        !frame.contains("> draft"),
        "composer should be hidden: {frame}"
    );
    assert!(
        cursor.is_none(),
        "blocking dialog should not expose prompt cursor"
    );
}

#[test]
fn escape_cancels_with_escape_reason() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.push_approval(background_request());
    let response = app
        .handle_pending_approval_input(InputEvent::Cancel)
        .expect("escape cancels");
    assert!(matches!(
        response,
        ApprovalResponse::Cancelled {
            reason: neo_agent_core::ApprovalCancelReason::Escape,
            ..
        }
    ));
    assert!(!app.approval_is_pending());
}

#[test]
fn event_approval_requested_does_not_open_live_modal() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.push_overlay(Overlay::new(
        "commands",
        OverlayKind::CommandPalette(CommandPaletteState::new([CommandSpec::new(
            "cmd",
            "Command",
            None::<String>,
        )])),
    ));
    assert!(app.focused_overlay().is_some());

    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: background_request(),
    });

    // Observable event alone must not open the live chrome modal.
    assert!(!app.approval_is_pending());
    assert!(app.focused_overlay().is_some());
    assert_ne!(app.mode(), ChromeMode::Approval);
}

#[test]
fn overlay_message_renders_plain_line() {
    let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
    app.push_overlay(Overlay::new(
        "message",
        OverlayKind::Message("hello".to_owned()),
    ));

    assert_eq!(app.focused_overlay_lines(80), vec!["hello".to_owned()]);
}

#[test]
fn pending_approval_has_one_width_bounded_presentation() {
    let mut request = background_request();
    request.presentation = ApprovalPresentation::Command {
        title: "Run this command?".to_owned(),
        command: "rtk git status ".repeat(20),
        cwd: Some(PathBuf::from("/Users/chenyuanhao/Workspace/neo")),
    };
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.push_approval(request.clone());
    let mut transcript = TranscriptPane::new(80, 20);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested { request });
    let mut tui = neo_tui::NeoTui::new(app, transcript);

    let (lines, cursor) = tui.render_frame(80, 20);
    let plain = strip_lines(lines.clone()).join("\n");

    assert_eq!(
        plain.matches("Run this command?").count(),
        1,
        "approval must have one visible presentation owner: {plain}"
    );
    assert!(
        lines
            .iter()
            .all(|line| neo_tui::primitive::visible_width(line) <= 80),
        "approval frame must fit terminal width: {lines:#?}"
    );
    assert!(cursor.is_none(), "approval must keep composer blocked");
}

#[test]
fn pending_approval_hides_composer_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut().apply_edit(PromptEdit::Insert("draft"));
    app.push_approval(background_request());

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let frame = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !frame.contains("> draft"),
        "composer should be hidden: {frame}"
    );
    assert!(
        frame.contains("[ask]"),
        "footer should remain visible: {frame}"
    );
    assert!(
        cursor.is_none(),
        "blocking approval should not expose prompt cursor"
    );
}

#[test]
fn plan_revision_arrow_and_number_share_one_editor_path() {
    // Arrow path.
    let mut arrow_app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    arrow_app.push_approval(plan_revision_request());
    arrow_app.handle_pending_approval_input(InputEvent::Action(KeybindingAction::SelectDown));
    assert!(matches!(
        arrow_app.approval_selected_action(),
        Some(ApprovalAction::RevisePlan {
            preset_feedback: Some(text)
        }) if text == "Keep compaction at 85%."
    ));
    assert!(
        !render_app(100, &arrow_app)
            .iter()
            .any(|line| line.contains("feedback:")),
        "navigation alone must not enter feedback editing"
    );
    let arrow_response = complete_plan_revision(&mut arrow_app);

    // Number path in a fresh app.
    let mut number_app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    number_app.push_approval(plan_revision_request());
    assert!(
        number_app.choose_approval_number(2).is_none(),
        "number selects revision and enters editing without submitting"
    );
    let (_, selected, feedback, collecting) = number_app.approval_selection().expect("pending");
    assert_eq!(selected, 1);
    assert!(collecting);
    assert_eq!(feedback, "Keep compaction at 85%.");
    // Allow the same edit as the arrow path, then submit.
    number_app.handle_pending_approval_input(InputEvent::Insert(' '));
    number_app.handle_pending_approval_input(InputEvent::Insert('x'));
    let number_response = number_app
        .handle_pending_approval_input(InputEvent::Submit)
        .expect("number path submits after edit");

    assert_eq!(arrow_response, number_response);
    assert!(matches!(
        arrow_response,
        ApprovalResponse::Selected {
            action: ApprovalAction::RevisePlan {
                preset_feedback: Some(ref preset)
            },
            feedback: Some(ref text),
            ..
        } if preset == "Keep compaction at 85%." && text == "Keep compaction at 85%. x"
    ));
}

#[test]
fn push_approval_stores_request_and_blocks_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.push_approval(background_request());
    assert!(app.approval_is_pending());
    assert_eq!(app.mode(), ChromeMode::Approval);
    assert!(app.focused_overlay_blocks_prompt());
    assert_eq!(
        app.pending_approval()
            .map(|modal| modal.request.options.len()),
        Some(2)
    );
}

#[test]
fn task_browser_overlay_blocks_prompt_and_renders_own_footer() {
    let mut app = NeoChromeState::new("neo", "test-session", "model", "/tmp/neo-ws");
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![task_browser_item(
        "bash-1",
        TaskBrowserStatus::Running,
    )]));
    app.push_task_browser_overlay(state);

    assert!(app.focused_overlay_blocks_prompt());
    assert!(app.focused_overlay_is_rich_dialog());

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let plain = lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>();
    let rendered = plain.join("\n");

    assert!(cursor.is_none());
    assert!(rendered.contains("TASKS"));
    assert!(rendered.contains("[ALL]"));
    assert!(rendered.contains("Tasks"));
    assert!(rendered.contains("bash-1"));
    assert!(rendered.contains("cargo test"));
    assert!(rendered.contains("Tab filter"));
    assert!(rendered.contains("Esc close"));
    assert!(!rendered.contains("/tmp/neo-ws"));
    assert_eq!(
        plain
            .iter()
            .filter(|line| line.contains("Esc close"))
            .count(),
        1
    );
}

#[test]
fn task_browser_overlay_replaces_existing_transcript_body() {
    let mut app = NeoChromeState::new("neo", "test-session", "model", "/tmp/neo-ws");
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![task_browser_item(
        "bash-1",
        TaskBrowserStatus::Running,
    )]));
    app.push_task_browser_overlay(state);

    let mut transcript = TranscriptPane::new(80, 20);
    transcript.push_status("old transcript line should be hidden");
    let mut tui = neo_tui::NeoTui::new(app, transcript);
    let frame = tui.render_terminal_frame(80, 20);
    assert!(frame.cursor.is_none());
    let rendered = frame
        .lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("TASKS"));
    assert!(!rendered.contains("old transcript line should be hidden"));

    tui.chrome_mut().close_focused_overlay();
    assert!(tui.render_terminal_frame(80, 20).lines.len() <= 20);
}

#[test]
fn theme_manager_overlay_blocks_composer_and_escape_closes() {
    let mut app = NeoChromeState::new("neo", "test-session", "model", "/tmp/neo-ws");
    app.prompt_mut()
        .apply_edit(PromptEdit::Insert("draft prompt"));
    let entry = ThemeCatalogEntrySnapshot {
        id: "solarized-dark.json".to_owned(),
        display_name: "Solarized Dark".to_owned(),
        theme: Some(neo_tui::primitive::theme::TuiTheme::default()),
        error: None,
        active: false,
        startup_default: false,
    };
    app.open_theme_manager(vec![entry]);

    assert!(app.focused_overlay_blocks_prompt());
    assert!(app.focused_overlay_is_rich_dialog());

    // The full-screen frame hides the transcript and the composer entirely.
    let mut transcript = TranscriptPane::new(80, 20);
    transcript.push_status("composer must be hidden");
    let mut tui = neo_tui::NeoTui::new(app, transcript);
    let frame = tui.render_terminal_frame(80, 20);
    assert!(frame.cursor.is_none());
    let rendered = frame
        .lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("THEME MANAGER"), "{rendered}");
    assert!(!rendered.contains("composer must be hidden"), "{rendered}");
    assert!(!rendered.contains("draft prompt"), "{rendered}");

    // Esc closes through the chrome (Cancelled); no Close action is queued,
    // so a later controller poll cannot act on a stale close.
    assert_eq!(
        tui.chrome_mut()
            .handle_focused_dialog_input(InputEvent::Cancel),
        neo_tui::primitive::InputResult::Cancelled
    );
    assert!(tui.chrome().focused_overlay().is_none());
    assert!(tui.chrome_mut().take_theme_manager_action().is_none());
    assert!(tui.render_terminal_frame(80, 20).lines.len() <= 20);
}
