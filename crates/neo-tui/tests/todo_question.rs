use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use neo_tui::chrome::NeoChromeState;
use neo_tui::transcript::TranscriptPane;
use neo_tui::widgets::{
    QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption, QuestionStateMachine,
    TodoDisplayItem, TodoDisplayStatus, select_visible_todos,
};

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
fn question_overlay_renders_in_live_tui_frame() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-123", make_single_question());
    let transcript = TranscriptPane::new(80, 24);
    let mut tui = neo_tui::NeoTui::new(app, transcript);

    let (lines, _) = tui.render_frame(80, 24);
    let frame = lines
        .iter()
        .map(|line| neo_tui::ansi::strip_ansi(line))
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
fn question_overlay_lines_fit_terminal_width() {
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
    let transcript = TranscriptPane::new(40, 24);
    let mut tui = neo_tui::NeoTui::new(app, transcript);

    let (lines, _) = tui.render_frame(40, 24);

    for line in lines {
        let plain = neo_tui::ansi::strip_ansi(&line);
        assert!(
            neo_tui::ansi::visible_width(&plain) <= 40,
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
fn app_cancel_question_returns_id() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-456", make_single_question());

    let id = app.cancel_question();
    assert_eq!(id, Some("q-456".to_owned()));
    assert!(!app.question_dialog_is_focused());
}

#[test]
fn app_closes_question_overlay_by_question_id() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("question-1", make_single_question());

    assert!(app.close_question_overlay("question-1").is_some());
    assert!(!app.question_dialog_is_focused());
}

#[test]
fn question_dialog_esc_cancels() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_single_question());

    let action = app
        .handle_question_dialog_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(action, QuestionDialogAction::Cancel);
    assert!(!app.question_dialog_is_focused());
}

#[test]
fn question_dialog_tab_navigation_through_keys() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_two_questions());

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.question_dialog_state().unwrap().active_tab == 1);

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(app.question_dialog_state().unwrap().on_submit_tab());

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert!(!app.question_dialog_state().unwrap().on_submit_tab());
}

#[test]
fn question_dialog_number_key_selection() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_single_question());

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));

    let state = app.question_dialog_state().unwrap();
    assert!(state.questions[0].selected[1]);
    assert!(!state.questions[0].selected[0]);
}

#[test]
fn question_dialog_down_moves_one_option_at_a_time() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-1", make_single_question());

    let _ = app.handle_question_dialog_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

    let state = app.question_dialog_state().unwrap();
    assert_eq!(state.cursor, 1);
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
    assert_eq!(visible.len(), 5);
    assert!(visible.contains(&2));
    assert!(visible.contains(&5));
    assert_eq!(visible, vec![0, 1, 2, 3, 5]);
}
