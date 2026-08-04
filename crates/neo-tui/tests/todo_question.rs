use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use neo_tui::dialogs::{
    QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption, QuestionStateMachine,
};
use neo_tui::input::{InputEvent, KeybindingAction};
use neo_tui::primitive::InputResult;
use neo_tui::screen_output::CURSOR_MARKER;
use neo_tui::shell::NeoChromeState;
use neo_tui::transcript::TranscriptPane;
use neo_tui::widgets::{TodoDisplayItem, TodoDisplayStatus, select_visible_todos};

// ---------------------------------------------------------------------------
// QuestionDialog state machine tests
// ---------------------------------------------------------------------------

fn make_single_question() -> Vec<QuestionDisplayData> {
    vec![QuestionDisplayData {
        question: "Which option?".into(),
        header: Some("Choice".into()),
        body: None,
        options: vec![
            QuestionDisplayOption {
                label: "Yes".into(),
                description: None,
            },
            QuestionDisplayOption {
                label: "No".into(),
                description: None,
            },
        ],
        multi_select: false,
    }]
}

fn make_two_questions() -> Vec<QuestionDisplayData> {
    vec![
        QuestionDisplayData {
            question: "Q1?".into(),
            header: Some("H1".into()),
            body: None,
            options: vec![QuestionDisplayOption {
                label: "A".into(),
                description: None,
            }],
            multi_select: false,
        },
        QuestionDisplayData {
            question: "Q2?".into(),
            header: Some("H2".into()),
            body: None,
            options: vec![
                QuestionDisplayOption {
                    label: "X".into(),
                    description: None,
                },
                QuestionDisplayOption {
                    label: "Y".into(),
                    description: None,
                },
            ],
            multi_select: true,
        },
    ]
}

#[test]
fn app_pushes_question_overlay() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-123", make_single_question());

    assert!(app.question_dialog_is_focused());
    assert!(app.question_dialog_state().is_some());
}

#[test]
fn question_prompt_renders_in_live_tui_frame() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-123", make_single_question());
    let mut transcript = TranscriptPane::new(80, 24);
    // The transcript card is the single visible owner of the question; the
    // chrome overlay keeps the runtime selection state.
    transcript.upsert_question_prompt("q-123", make_single_question());
    let mut tui = neo_tui::NeoTui::new(app, transcript);

    let (lines, _) = tui.render_frame(80, 24);
    let frame = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(frame.contains("question"));
    assert!(frame.contains("Choice"));
    assert!(frame.contains("Which option?"));
    assert!(frame.contains("[1] Yes"));
    assert!(frame.contains("[2] No"));
    assert!(frame.contains("[3] Other"));
}

#[test]
fn question_prompt_lines_fit_terminal_width() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay(
        "q-123",
        vec![QuestionDisplayData {
            question: "This is a deliberately long question that needs wrapping".into(),
            header: Some("Extremely long header text that must not overflow".into()),
            body: None,
            options: vec![
                QuestionDisplayOption {
                    label: "A long option label that also needs wrapping".into(),
                    description: Some(
                        "A description with enough words to wrap in a narrow terminal".into(),
                    ),
                },
                QuestionDisplayOption {
                    label: "Second option".into(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
    );
    let mut transcript = TranscriptPane::new(40, 24);
    transcript.upsert_question_prompt(
        "q-123",
        vec![QuestionDisplayData {
            question: "This is a deliberately long question that needs wrapping".into(),
            header: Some("Extremely long header text that must not overflow".into()),
            body: None,
            options: vec![
                QuestionDisplayOption {
                    label: "A long option label that also needs wrapping".into(),
                    description: Some(
                        "A description with enough words to wrap in a narrow terminal".into(),
                    ),
                },
                QuestionDisplayOption {
                    label: "Second option".into(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
    );
    let mut tui = neo_tui::NeoTui::new(app, transcript);

    let (lines, _) = tui.render_frame(40, 24);

    for line in lines {
        let plain = neo_tui::primitive::strip_ansi(&line);
        assert!(
            neo_tui::primitive::visible_width(&plain) <= 40,
            "line exceeded width: {plain:?}"
        );
    }
}

#[test]
fn question_submit_page_number_two_cancels() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_single_question());

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.question_dialog_state().unwrap().on_submit_tab());

    let action = app
        .handle_question_dialog_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE))
        .unwrap();

    assert_eq!(action, QuestionDialogAction::Cancel);
    assert!(!app.question_dialog_is_focused());
}

#[test]
fn app_confirm_question_returns_answers() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_single_question());

    // Answer the question
    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let result = app.confirm_question();
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.id, "q-1");
    assert_eq!(result.answers, vec!["Yes"]);
    assert!(!app.question_dialog_is_focused());
}

#[test]
fn question_dialog_cancel_paths_close_overlay() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-cancel", make_single_question());
    assert_eq!(app.cancel_question(), Some("q-cancel".to_owned()));
    assert!(!app.question_dialog_is_focused());

    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-close", make_single_question());
    assert!(app.close_question_overlay("q-close").is_some());
    assert!(!app.question_dialog_is_focused());

    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-esc", make_single_question());
    let action = app
        .handle_question_dialog_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("question handles esc");
    assert_eq!(action, QuestionDialogAction::Cancel);
    assert!(!app.question_dialog_is_focused());
}

#[test]
fn question_dialog_key_behaviors() {
    struct Case {
        keys: &'static [KeyCode],
        assert_state: fn(&NeoChromeState),
    }

    fn assert_enter_selects_first_and_advances_to_submit(app: &NeoChromeState) {
        let state = app.question_dialog_state().expect("focused");
        assert!(state.on_submit_tab(), "expected submit tab");
        assert!(state.questions[0].selected[0], "first option selected");
    }

    fn assert_number_selects_matching_option(app: &NeoChromeState) {
        let state = app.question_dialog_state().expect("focused");
        assert!(state.questions[0].selected[1], "second option selected");
        assert!(!state.questions[0].selected[0], "first option unselected");
    }

    fn assert_down_moves_one_row(app: &NeoChromeState) {
        let state = app.question_dialog_state().expect("focused");
        assert_eq!(state.cursor, 1);
    }

    fn assert_right_reaches_submit_and_left_returns(app: &NeoChromeState) {
        let state = app.question_dialog_state().expect("focused");
        assert_eq!(state.active_tab, 0);
    }

    let cases = [
        Case {
            keys: &[KeyCode::Enter],
            assert_state: assert_enter_selects_first_and_advances_to_submit,
        },
        Case {
            keys: &[KeyCode::Char('2')],
            assert_state: assert_number_selects_matching_option,
        },
        Case {
            keys: &[KeyCode::Down],
            assert_state: assert_down_moves_one_row,
        },
        Case {
            keys: &[KeyCode::Enter, KeyCode::Right, KeyCode::Left],
            assert_state: assert_right_reaches_submit_and_left_returns,
        },
    ];

    for case in &cases {
        let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
        app.push_question_overlay("q-1", make_single_question());
        for key in case.keys {
            let _ = app.handle_question_dialog_key(KeyEvent::new(*key, KeyModifiers::NONE));
        }
        (case.assert_state)(&app);
    }
}

#[test]
fn question_dialog_tab_navigation_through_multiple_questions() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_two_questions());

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.question_dialog_state().unwrap().active_tab, 1);

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(app.question_dialog_state().unwrap().on_submit_tab());

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert!(!app.question_dialog_state().unwrap().on_submit_tab());
}

#[test]
fn focused_dialog_input_drives_question_dialog_other_text() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_single_question());

    assert!(app.focused_overlay_is_rich_dialog());
    assert_eq!(
        app.handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectDown)),
        InputResult::Handled
    );
    assert_eq!(
        app.handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectDown)),
        InputResult::Handled
    );
    assert_eq!(
        app.handle_focused_dialog_input(InputEvent::Submit),
        InputResult::Handled
    );
    assert_eq!(
        app.handle_focused_dialog_input(InputEvent::Paste("custom answer".into())),
        InputResult::Handled
    );

    let state = app.question_dialog_state().unwrap();
    assert!(state.questions[0].other_selected);
    assert_eq!(state.questions[0].other_text, "custom answer");
}

#[test]
fn other_text_supports_cursor_editing() {
    let mut state = QuestionStateMachine::new("q-1", make_single_question());
    state.cursor = 2;
    state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    for character in "abcd".chars() {
        state.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    assert_eq!(state.questions[0].other_text, "abcd");

    state.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE));
    assert_eq!(state.questions[0].other_text, "abXcd");

    state.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(state.questions[0].other_text, "abcd");

    state.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert_eq!(state.questions[0].other_text, "bcd");

    state.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    state.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    assert_eq!(state.questions[0].other_text, "bc");
    assert!(
        state
            .render_lines(80)
            .iter()
            .any(|line| line.contains(CURSOR_MARKER))
    );
    for line in state.render_lines(16) {
        assert!(
            neo_tui::primitive::visible_width(&line) <= 16,
            "narrow question row overflowed: {line:?}"
        );
    }
}

#[test]
fn question_other_editing_exposes_terminal_cursor() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_single_question());
    let mut transcript = TranscriptPane::new(80, 24);
    transcript.upsert_question_prompt("q-1", make_single_question());
    let mut tui = neo_tui::NeoTui::new(app, transcript);

    for _ in 0..2 {
        tui.chrome_mut()
            .handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectDown));
    }
    tui.chrome_mut()
        .handle_focused_dialog_input(InputEvent::Submit);
    let machine = tui
        .chrome()
        .question_dialog_state()
        .cloned()
        .expect("question stays focused");
    tui.transcript_mut().sync_question_prompt(&machine);
    let (_, cursor) = tui.render_frame(80, 24);
    assert!(
        cursor.is_some(),
        "empty Other input exposes terminal cursor"
    );

    tui.chrome_mut()
        .handle_focused_dialog_input(InputEvent::Paste("custom answer".into()));
    let machine = tui
        .chrome()
        .question_dialog_state()
        .cloned()
        .expect("question stays focused");
    tui.transcript_mut().sync_question_prompt(&machine);

    let (lines, cursor) = tui.render_frame(80, 24);
    let cursor = cursor.expect("question input exposes terminal cursor");
    let row = lines.get(cursor.row).expect("cursor row is rendered");
    assert!(neo_tui::primitive::strip_ansi(row).contains("Other: custom answer"));
    assert!(lines.iter().all(|line| !line.contains(CURSOR_MARKER)));
}

#[test]
fn question_dialog_full_flow_two_questions() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-full", make_two_questions());

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    let action = app
        .handle_question_dialog_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    match action {
        QuestionDialogAction::Submit(result) => {
            assert_eq!(result.id, "q-full");
            assert_eq!(result.answers, vec!["A", "X, Y"]);
        }
        _ => panic!("expected Submit action"),
    }
}

// ---------------------------------------------------------------------------
// QuestionStateMachine direct tests
// ---------------------------------------------------------------------------

#[test]
fn state_machine_multi_select_other_option() {
    let mut state = QuestionStateMachine::new("q-1", make_two_questions());
    state.active_tab = 1;
    state.cursor = 2;

    state.toggle_current();
    assert!(state.questions[1].other_selected);
    assert!(state.other_editing);

    state.insert_char('Z');
    assert_eq!(state.questions[1].other_text, "Z");

    state.toggle_current();
    assert!(!state.questions[1].other_selected);
    assert!(!state.other_editing);
}

#[test]
fn state_machine_scroll_sync() {
    let options: Vec<QuestionDisplayOption> = (0..10)
        .map(|i| QuestionDisplayOption {
            label: format!("opt-{i}"),
            description: None,
        })
        .collect();

    let questions = vec![QuestionDisplayData {
        question: "Pick many".into(),
        header: None,
        body: None,
        options,
        multi_select: true,
    }];

    let mut state = QuestionStateMachine::new("q-scroll", questions);

    for _ in 0..7 {
        state.move_cursor_down();
    }

    assert!(state.scroll > 0);
    assert!(state.cursor >= state.scroll);
}

// ---------------------------------------------------------------------------
// TodoPanel select_visible_todos tests via public API
// ---------------------------------------------------------------------------

#[test]
fn select_visible_prioritises_in_progress_and_latest_done() {
    let todos = vec![
        TodoDisplayItem::new("p1", TodoDisplayStatus::Pending),
        TodoDisplayItem::new("p2", TodoDisplayStatus::Pending),
        TodoDisplayItem::new("ip1", TodoDisplayStatus::InProgress),
        TodoDisplayItem::new("p3", TodoDisplayStatus::Pending),
        TodoDisplayItem::new("d1", TodoDisplayStatus::Done),
        TodoDisplayItem::new("d2", TodoDisplayStatus::Done),
        TodoDisplayItem::new("p4", TodoDisplayStatus::Pending),
    ];

    let visible = select_visible_todos(&todos, 5);
    assert_eq!(visible.indices.len(), 5);
    assert!(visible.indices.contains(&2));
    assert!(visible.indices.contains(&5));
    assert_eq!(visible.indices, vec![0, 1, 2, 3, 5]);
}

// ---------------------------------------------------------------------------
// Earliest blocking entry focus
// ---------------------------------------------------------------------------

/// The earliest unresolved approval/question owns the focus in transcript
/// order; later requests stay present but inactive until it resolves.
#[test]
fn earliest_blocking_entry_keeps_focus_across_later_requests() {
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
        PermissionOperation,
    };
    use neo_tui::transcript::BlockingEntryKind;

    fn shell_approval(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            turn: 1,
            id: id.to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Tool {
                title: format!("Run {id}?"),
                details: vec!["cargo test".to_owned()],
            },
            options: vec![ApprovalOption {
                action: ApprovalAction::PermitOnce,
                label: "Allow once".to_owned(),
                description: None,
            }],
            workflow_origin: None,
        }
    }

    let mut pane = TranscriptPane::new(120, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: shell_approval("approval-1"),
    });
    // A later question must never displace the earlier approval.
    pane.upsert_question_prompt("question-1", make_single_question());
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: shell_approval("approval-2"),
    });

    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Approval("approval-1".to_owned()))
    );
    let update = pane.render_terminal_update(120, 24);
    let live = update
        .live
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(live.contains("Run approval-1?"), "live:\n{live}");
    assert!(!live.contains("Which option?"), "live:\n{live}");

    // Resolving the approval promotes the question to the focus.
    pane.resolve_approval(
        "approval-1",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Question("question-1".to_owned()))
    );
    let update = pane.render_terminal_update(120, 24);
    let live = update
        .live
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(live.contains("Which option?"), "live:\n{live}");
    assert!(!live.contains("Run approval-2?"), "live:\n{live}");

    // Answering the question promotes the later approval.
    pane.resolve_question_prompt("question-1", vec!["Yes".to_owned()]);
    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Approval("approval-2".to_owned()))
    );
}
