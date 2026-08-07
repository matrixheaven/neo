use neo_tui::input::{KeyId, KeybindingAction, KeybindingsManager};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{visible_width, wrap_width};
use neo_tui::shell::{SelectItem, SelectListState};
use neo_tui::transcript::{DocumentLayout, TranscriptEntry, TranscriptStore};

fn strip_ansi_escapes(text: &str) -> String {
    let mut visible = String::new();
    let mut index = 0;
    while index < text.len() {
        if text.as_bytes().get(index).copied() == Some(0x1b)
            && let Some(end) = text[index..].find('m')
        {
            index += end + 1;
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };
        visible.push(character);
        index += character.len_utf8();
    }
    visible
}

#[test]
fn ansi_width_cases_are_display_width_safe() {
    struct Case {
        name: &'static str,
        input: &'static str,
        width: usize,
        expected_width: usize,
    }

    let cases = [
        Case {
            name: "plain ascii",
            input: "hello",
            width: 10,
            expected_width: 5,
        },
        Case {
            name: "ansi sgr ignored",
            input: "\x1b[31mred\x1b[0m",
            width: 10,
            expected_width: 3,
        },
        Case {
            name: "osc ignored",
            input: "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
            width: 10,
            expected_width: 4,
        },
        Case {
            name: "wide cjk",
            input: "你好",
            width: 10,
            expected_width: 4,
        },
    ];

    for case in &cases {
        assert_eq!(
            visible_width(case.input),
            case.expected_width,
            "{}",
            case.name
        );
        for line in wrap_width(case.input, case.width) {
            assert!(
                visible_width(&line) <= case.width,
                "{} overflowed: {line:?}",
                case.name
            );
        }
    }
}

#[test]
fn keybinding_manager_matches_defaults_overrides_and_conflicts() {
    let mut manager = KeybindingsManager::default();

    assert!(manager.matches(
        &KeyId::new("ctrl+b").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));
    assert!(manager.matches(
        &KeyId::new("left").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+n").expect("valid key"),
        KeybindingAction::SessionFork
    ));
    assert!(manager.matches(
        &KeyId::new("shift+up").expect("valid key"),
        KeybindingAction::TranscriptSelectionExtendUp
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+c").expect("valid key"),
        KeybindingAction::TranscriptCopySelection
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+o").expect("valid key"),
        KeybindingAction::ToolOutputToggle
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+t").expect("valid key"),
        KeybindingAction::TodoPanelToggle
    ));
    assert_eq!(
        KeybindingAction::from_id("tui.todo.toggle"),
        Some(KeybindingAction::TodoPanelToggle)
    );
    assert!(!manager.matches(
        &KeyId::new("ctrl+o").expect("valid key"),
        KeybindingAction::ModelPickerOpen
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+c").expect("valid key"),
        KeybindingAction::AppClear
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+d").expect("valid key"),
        KeybindingAction::AppExit
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+z").expect("valid key"),
        KeybindingAction::AppSuspend
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+_").expect("valid key"),
        KeybindingAction::EditorUndo
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+p").expect("valid key"),
        KeybindingAction::PromptCompletionToggle
    ));
    assert!(!manager.matches(
        &KeyId::new("ctrl+c").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));

    manager.set_user_bindings([(
        KeybindingAction::EditorCursorLeft,
        vec![KeyId::new("alt+h").expect("valid key")],
    )]);

    assert!(manager.matches(
        &KeyId::new("alt+h").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));
    assert!(!manager.matches(
        &KeyId::new("left").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));

    manager.set_user_bindings([
        (
            KeybindingAction::EditorCursorLeft,
            vec![KeyId::new("alt+h").expect("valid key")],
        ),
        (
            KeybindingAction::EditorCursorRight,
            vec![KeyId::new("alt+h").expect("valid key")],
        ),
    ]);

    let conflicts = manager.conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].key, KeyId::new("alt+h").expect("valid key"));
}

#[test]
fn select_list_filters_wraps_and_reports_visible_window() {
    let mut list = SelectListState::new(
        [
            SelectItem::new("open", "Open", Some("Open a file")),
            SelectItem::new("close", "Close", Some("Close the active file")),
            SelectItem::new("copy", "Copy", Some("Copy selection")),
            SelectItem::new("commit", "Commit", Some("Commit staged changes")),
        ],
        2,
    );

    list.set_filter("c");
    assert_eq!(list.filtered_len(), 3);
    assert_eq!(list.selected_item().expect("selected").value, "close");

    list.move_down();
    assert_eq!(list.selected_item().expect("selected").value, "copy");
    assert_eq!(list.visible_range(), 0..2);

    list.move_down();
    assert_eq!(list.selected_item().expect("selected").value, "commit");
    assert_eq!(list.visible_range(), 1..3);

    list.move_down();
    assert_eq!(list.selected_item().expect("selected").value, "close");

    let theme = TuiTheme::default();
    let lines = list.render_lines(18, &theme);
    assert_eq!(lines.len(), 3);
    assert!(strip_ansi_escapes(&lines[0]).contains("> Close"));
    assert!(lines[2].contains("(1/3)"));
    assert!(lines.iter().all(|line| visible_width(line) <= 18));
}

#[test]
fn select_list_pages_by_visible_window_and_clamps() {
    let mut list = SelectListState::new(
        (0..10).map(|index| {
            SelectItem::new(
                format!("item-{index}"),
                format!("Item {index}"),
                None::<String>,
            )
        }),
        4,
    );

    list.page_down();
    assert_eq!(list.selected_item().expect("selected").value, "item-4");
    assert_eq!(list.visible_range(), 2..6);

    list.page_down();
    assert_eq!(list.selected_item().expect("selected").value, "item-8");
    assert_eq!(list.visible_range(), 6..10);

    list.page_down();
    assert_eq!(list.selected_item().expect("selected").value, "item-9");
    assert_eq!(list.visible_range(), 6..10);

    list.page_up();
    assert_eq!(list.selected_item().expect("selected").value, "item-5");
    assert_eq!(list.visible_range(), 3..7);

    list.page_up();
    assert_eq!(list.selected_item().expect("selected").value, "item-1");
    assert_eq!(list.visible_range(), 0..4);

    list.page_up();
    assert_eq!(list.selected_item().expect("selected").value, "item-0");
    assert_eq!(list.visible_range(), 0..4);
}

#[test]
fn transcript_store_explicit_follow_bottom_restores_tail_after_push() {
    let mut store = TranscriptStore::new();
    for index in 0..20 {
        store.push(TranscriptEntry::status(format!("line {index}")));
    }
    let mut document = DocumentLayout::new();
    document.sync_entries(store.entry_ids(), store.entry_revisions());
    for index in 0..20 {
        document.set_entry_height(index, 1);
    }
    document.visible_row_range(5);
    document.scroll_up(6);
    assert!(!document.is_following_tail());

    document.follow_bottom();
    store.push(TranscriptEntry::status("new line"));
    document.sync_entries(store.entry_ids(), store.entry_revisions());
    document.set_entry_height(20, 1);

    assert_eq!(document.visible_row_range(5), 36..41);
    assert_eq!(document.total_rows(), 41);
    assert!(document.is_following_tail());
}

#[test]
fn transcript_store_push_preserves_manual_scroll_state() {
    // The document owns the scroll anchor: a locked view must not be yanked
    // when the store grows below the anchor.
    let mut store = TranscriptStore::new();
    for index in 0..20 {
        store.push(TranscriptEntry::status(format!("line {index}")));
    }
    let mut document = DocumentLayout::new();
    document.sync_entries(store.entry_ids(), store.entry_revisions());
    for index in 0..20 {
        document.set_entry_height(index, 1);
    }
    document.visible_row_range(5);
    document.scroll_up(6);
    assert_eq!(document.visible_row_range(5), 28..33);
    assert!(!document.is_following_tail());

    store.push(TranscriptEntry::status("new line"));
    document.sync_entries(store.entry_ids(), store.entry_revisions());
    document.set_entry_height(20, 1);

    assert_eq!(
        document.visible_row_range(5),
        28..33,
        "new content must not yank a manually scrolled viewport"
    );
    assert!(!document.is_following_tail());
}
