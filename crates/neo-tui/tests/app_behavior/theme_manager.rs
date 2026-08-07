//! Focused integration coverage for the theme manager overlay and the shared
//! pure preview renderer: responsive layouts, visible-width safety, the
//! filter/focus/action contract, stable selection, and composer blocking.

use neo_tui::input::{InputEvent, KeybindingAction};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{strip_ansi, visible_width};
use neo_tui::shell::{
    NeoChromeState, OverlayKind, ThemeCatalogEntrySnapshot, ThemeManagerAction, ThemeManagerFocus,
    ThemeManagerPending,
};
use neo_tui::theme_preview::ThemePreviewRenderer;
use neo_tui::transcript::TranscriptPane;

fn entry(id: &str, name: &str) -> ThemeCatalogEntrySnapshot {
    ThemeCatalogEntrySnapshot {
        id: id.to_owned(),
        display_name: name.to_owned(),
        theme: Some(TuiTheme::default()),
        error: None,
        active: false,
        startup_default: false,
    }
}
fn invalid_entry(id: &str, name: &str) -> ThemeCatalogEntrySnapshot {
    ThemeCatalogEntrySnapshot {
        id: id.to_owned(),
        display_name: name.to_owned(),
        theme: None,
        error: Some("expected token \"brand\"".to_owned()),
        active: false,
        startup_default: false,
    }
}
fn open_manager() -> NeoChromeState {
    let mut chrome = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/ws");
    chrome.open_theme_manager(sample_entries());
    chrome
}
fn overlay_plain(chrome: &NeoChromeState, width: usize, height: usize) -> Vec<String> {
    chrome
        .render_focused_full_screen_overlay(width, height)
        .expect("theme manager is a full-screen overlay")
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect()
}
fn sample_entries() -> Vec<ThemeCatalogEntrySnapshot> {
    vec![
        entry("solarized-dark.json", "Solarized Dark"),
        entry("aurora-night.json", "Aurora Night"),
        entry("monokai.json", "Monokai Pro"),
        invalid_entry("broken.json", "Broken Theme"),
    ]
}

#[test]
fn theme_manager_overlay_blocks_prompt_and_is_rich_dialog() {
    let chrome = open_manager();
    assert!(chrome.focused_overlay_blocks_prompt());
    assert!(chrome.focused_overlay_is_rich_dialog());
    assert!(matches!(
        chrome.focused_overlay().map(|overlay| &overlay.kind),
        Some(OverlayKind::ThemeManager(_))
    ));
    assert_eq!(chrome.focused_overlay_height(), 0);

    let plain = overlay_plain(&chrome, 120, 24).join("\n");
    assert!(plain.contains("THEME MANAGER"), "{plain}");
    assert!(plain.contains("Solarized Dark"), "{plain}");
    assert!(plain.contains("Broken Theme"), "{plain}");
    assert!(plain.contains("expected token"), "{plain}");
    assert!(plain.contains("Enter apply"), "{plain}");
}

#[test]
fn theme_manager_overlay_hides_transcript_and_composer() {
    let chrome = open_manager();
    let mut transcript = TranscriptPane::new(120, 24);
    transcript.push_status("hidden transcript line");
    let mut tui = neo_tui::NeoTui::new(chrome, transcript);
    let frame = tui.render_terminal_frame(120, 24);
    let rendered = frame
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("THEME MANAGER"), "{rendered}");
    assert!(!rendered.contains("hidden transcript line"), "{rendered}");
    assert!(frame.cursor.is_none());
}

#[test]
fn theme_manager_breakpoints_map_width_to_layout() {
    // Each named case renders the same manager fixture at a breakpoint and
    // asserts the layout contract for that width/height.
    struct Case {
        name: &'static str,
        width: usize,
        height: usize,
        assert: fn(&mut NeoChromeState, &[String]),
    }

    fn assert_wide(_: &mut NeoChromeState, plain: &[String]) {
        let rendered = plain.join("\n");
        // Both panes visible in the same frame.
        assert!(rendered.contains(" themes "), "{rendered}");
        assert!(rendered.contains(" preview "), "{rendered}");
        // The preview pane renders the shared sample surface with the
        // selected theme (default = Solarized Dark, the first entry).
        assert!(rendered.contains("Welcome back"), "{rendered}");
        assert!(rendered.contains("Approve write access"), "{rendered}");
        assert!(rendered.contains("12.4k/200k"), "{rendered}");
    }

    fn assert_medium(_: &mut NeoChromeState, plain: &[String]) {
        let rendered = plain.join("\n");
        assert!(rendered.contains(" themes "), "{rendered}");
        assert!(rendered.contains(" preview "), "{rendered}");
        // List rows appear above the preview pane in the stacked layout.
        let list_at = plain.iter().position(|line| line.contains(" themes "));
        let preview_at = plain.iter().position(|line| line.contains(" preview "));
        assert!(list_at < preview_at, "{rendered}");
    }

    fn assert_narrow_list(_: &mut NeoChromeState, plain: &[String]) {
        let rendered = plain.join("\n");
        assert!(rendered.contains("focus List"), "{rendered}");
        assert!(rendered.contains("Solarized Dark"), "{rendered}");
        assert!(!rendered.contains("Approve write access"), "{rendered}");
    }

    fn assert_narrow_preview_after_tab(chrome: &mut NeoChromeState, _: &[String]) {
        // Tab to Preview: the preview panel replaces the list.
        chrome.handle_focused_dialog_input(InputEvent::Insert('\t'));
        let plain = overlay_plain(chrome, 60, 20);
        let rendered = plain.join("\n");
        assert!(rendered.contains("focus Preview"), "{rendered}");
        assert!(rendered.contains("Approve write access"), "{rendered}");
        assert!(!rendered.contains("Solarized Dark"), "{rendered}");
    }

    fn assert_short(_: &mut NeoChromeState, _: &[String]) {
        let mut chrome = open_manager();
        for width in [40, 80, 120] {
            let plain = overlay_plain(&mut chrome, width, 5);
            assert_eq!(plain.len(), 5, "width={width}");
            let rendered = plain.join("\n");
            assert!(rendered.contains("THEME MANAGER"), "{rendered}");
            assert!(rendered.contains("focus List"), "{rendered}");
            assert!(rendered.contains("Enter apply"), "{rendered}");
        }
    }

    let cases = [
        Case {
            name: "wide",
            width: 120,
            height: 24,
            assert: assert_wide,
        },
        Case {
            name: "medium",
            width: 80,
            height: 20,
            assert: assert_medium,
        },
        Case {
            name: "narrow_list",
            width: 60,
            height: 20,
            assert: assert_narrow_list,
        },
        Case {
            name: "narrow_preview_after_tab",
            width: 60,
            height: 20,
            assert: assert_narrow_preview_after_tab,
        },
        Case {
            name: "short",
            width: 120,
            height: 5,
            assert: assert_short,
        },
    ];

    for case in &cases {
        let mut chrome = open_manager();
        let plain = overlay_plain(&mut chrome, case.width, case.height);
        (case.assert)(&mut chrome, &plain);
        let rendered = plain.join("\n");
        assert!(
            rendered.contains("THEME MANAGER"),
            "case {} must stay on the manager screen: {rendered}",
            case.name
        );
    }
}

#[test]
fn all_layouts_never_exceed_width_with_cjk_and_long_values() {
    let entries = vec![
        ThemeCatalogEntrySnapshot {
            id: "超级长的主题名称-abcdefghijklmnopqrstuvwxyz-0123456789.json".to_owned(),
            display_name: "深色主题 · 长名称 · 日本語 · emoji ⚙".to_owned(),
            theme: Some(TuiTheme::default()),
            error: None,
            active: true,
            startup_default: true,
        },
        invalid_entry(
            "invalid-带着错误的名字.json",
            "坏主题 · 无法加载 · very long display name that keeps going",
        ),
    ];
    let mut chrome = NeoChromeState::new("neo", "session-a", "model", "/tmp/ws");
    chrome.open_theme_manager(entries);
    for width in [32, 60, 80, 100, 120] {
        for height in [5, 10, 20, 30] {
            let lines = overlay_plain(&chrome, width, height);
            assert_eq!(lines.len(), height, "width={width} height={height}");
            assert!(
                lines.iter().all(|line| visible_width(line) <= width),
                "width={width} height={height}:\n{}",
                lines.join("\n")
            );
        }
    }
}

#[test]
fn selection_changes_preview_only_and_never_applies() {
    let mut chrome = open_manager();
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.selected_id()),
        Some("aurora-night.json")
    );
    // The preview value equals the selected entry's theme.
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.preview()),
        Some(TuiTheme::default())
    );

    chrome.handle_focused_dialog_input(InputEvent::Insert('j'));
    let state = chrome.theme_manager_state().expect("manager open");
    assert_eq!(state.selected_id(), Some("broken.json"));
    assert!(
        !chrome.take_theme_manager_action().is_some(),
        "navigation must not emit actions"
    );
}

#[test]
fn enter_applies_selected_with_single_action_and_closes() {
    let mut chrome = open_manager();
    assert_eq!(
        chrome.handle_focused_dialog_input(InputEvent::Submit),
        neo_tui::primitive::InputResult::Submitted
    );
    // The chrome closes the overlay on `Submitted`, but the drained action
    // remains pollable exactly once.
    assert!(chrome.focused_overlay().is_none(), "overlay closed");
    assert_eq!(
        chrome.take_theme_manager_action(),
        Some(ThemeManagerAction::ApplySession(
            "aurora-night.json".to_owned()
        ))
    );
    // A second poll must not re-apply: exactly one action per keystroke.
    assert!(chrome.take_theme_manager_action().is_none());
}

#[test]
fn set_startup_default_keeps_manager_open_without_apply() {
    let mut chrome = open_manager();
    chrome.handle_focused_dialog_input(InputEvent::Insert('D'));
    assert_eq!(
        chrome.take_theme_manager_action(),
        Some(ThemeManagerAction::SetStartupDefault(
            "aurora-night.json".to_owned()
        ))
    );
    assert!(chrome.take_theme_manager_action().is_none());
    assert!(chrome.focused_overlay().is_some(), "manager stays open");
}

#[test]
fn invalid_entry_cannot_be_applied_or_defaulted() {
    let mut chrome = open_manager();
    // "Broken Theme" sorts second, right after "Aurora Night".
    chrome.handle_focused_dialog_input(InputEvent::Insert('j'));
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.selected_id()),
        Some("broken.json")
    );
    assert_eq!(
        chrome.handle_focused_dialog_input(InputEvent::Submit),
        neo_tui::primitive::InputResult::Handled
    );
    assert!(
        chrome.take_theme_manager_action().is_none(),
        "invalid entries cannot be applied"
    );
    let status = chrome
        .theme_manager_state()
        .and_then(|state| state.status().map(|status| status.text.clone()));
    assert!(status.is_some(), "error status shown");
    let plain = overlay_plain(&chrome, 120, 24).join("\n");
    assert!(plain.contains("invalid"), "{plain}");
}

#[test]
fn delete_confirmation_and_guards() {
    let mut chrome = open_manager();
    // Select broken.json (second entry); delete must be allowed (invalid
    // entries can be removed), confirmed via Enter, and emit Delete.
    chrome.handle_focused_dialog_input(InputEvent::Insert('j'));
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.selected_id()),
        Some("broken.json")
    );
    chrome.handle_focused_dialog_input(InputEvent::Insert('X'));
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.pending()),
        Some(ThemeManagerPending::Delete)
    );
    chrome.handle_focused_dialog_input(InputEvent::Submit);
    assert_eq!(
        chrome.take_theme_manager_action(),
        Some(ThemeManagerAction::Delete("broken.json".to_owned()))
    );

    // Esc cancels the confirmation without emitting.
    chrome.handle_focused_dialog_input(InputEvent::Insert('X'));
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.pending()),
        Some(ThemeManagerPending::Delete)
    );
    chrome.handle_focused_dialog_input(InputEvent::Cancel);
    assert!(chrome.take_theme_manager_action().is_none());

    // The active theme cannot be deleted.
    let mut guarded = NeoChromeState::new("neo", "s", "m", "/tmp");
    let mut active = entry("active.json", "Active Theme");
    active.active = true;
    guarded.open_theme_manager(vec![active]);
    guarded.handle_focused_dialog_input(InputEvent::Insert('X'));
    assert_eq!(
        guarded
            .theme_manager_state()
            .and_then(|state| state.pending()),
        None
    );
    assert!(
        guarded
            .theme_manager_state()
            .and_then(|state| state.status())
            .is_some_and(|status| status.is_error)
    );
}

#[test]
fn filter_esc_contract_via_chrome() {
    let mut chrome = open_manager();
    chrome.handle_focused_dialog_input(InputEvent::Insert('/'));
    assert_eq!(
        chrome.theme_manager_state().map(|state| state.focus()),
        Some(ThemeManagerFocus::Filter)
    );
    for character in ['a', 'u', 'r'] {
        chrome.handle_focused_dialog_input(InputEvent::Insert(character));
    }
    assert_eq!(
        chrome
            .theme_manager_state()
            .map(|state| state.filtered_count()),
        Some(1)
    );
    // Esc clears the filter first (manager stays open).
    chrome.handle_focused_dialog_input(InputEvent::Cancel);
    assert_eq!(
        chrome.theme_manager_state().map(|state| state.filter()),
        Some("")
    );
    assert!(chrome.focused_overlay().is_some());
    // Second Esc closes: the chrome owns the close, no Close action queued.
    assert_eq!(
        chrome.handle_focused_dialog_input(InputEvent::Cancel),
        neo_tui::primitive::InputResult::Cancelled
    );
    assert!(chrome.focused_overlay().is_none(), "overlay closed");
    assert!(chrome.take_theme_manager_action().is_none());
}

#[test]
fn refresh_import_and_copy_emit_typed_actions() {
    let mut chrome = open_manager();
    chrome.handle_focused_dialog_input(InputEvent::Insert('R'));
    assert_eq!(
        chrome.take_theme_manager_action(),
        Some(ThemeManagerAction::Refresh)
    );

    chrome.handle_focused_dialog_input(InputEvent::Insert('I'));
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.pending()),
        Some(ThemeManagerPending::ImportPath)
    );
    let state = chrome.theme_manager_state_mut().expect("manager open");
    let action = state.submit_import_path(Some("/tmp/new-theme.json".to_owned()));
    assert_eq!(
        action,
        Some(ThemeManagerAction::Import {
            path: "/tmp/new-theme.json".to_owned(),
            conflict_policy: neo_tui::shell::ThemeConflictPolicy::Ask,
        })
    );

    chrome.handle_focused_dialog_input(InputEvent::Insert('C'));
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.pending()),
        Some(ThemeManagerPending::CopyName)
    );
    let state = chrome.theme_manager_state_mut().expect("manager open");
    let action = state.submit_copy_name(Some("Aurora Clone".to_owned()));
    assert_eq!(
        action,
        Some(ThemeManagerAction::Duplicate {
            id: "aurora-night.json".to_owned(),
            new_display_name: "Aurora Clone".to_owned(),
        })
    );
}

#[test]
fn escape_closes_overlay_through_chrome() {
    let mut chrome = open_manager();
    assert_eq!(
        chrome.handle_focused_dialog_input(InputEvent::Cancel),
        neo_tui::primitive::InputResult::Cancelled
    );
    // The chrome closes the overlay directly; no Close action is queued.
    assert!(chrome.focused_overlay().is_none());
    assert!(chrome.take_theme_manager_action().is_none());
    assert!(!chrome.focused_overlay_blocks_prompt());
}

#[test]
fn preview_renderer_reuses_shared_sample_surface() {
    let theme = TuiTheme::default();
    let rows = ThemePreviewRenderer::new(theme, 100, 30, "openai/gpt-4.1").render();
    assert_eq!(rows.len(), 30);
    let plain = rows
        .iter()
        .map(|row| strip_ansi(row))
        .collect::<Vec<_>>()
        .join("\n");
    for sample in [
        "Neo",
        "Welcome back",
        "you  Add a two-pane theme manager",
        "working · esc interrupt",
        "@@ -1,3 +1,4 @@",
        "+  let value = 42;",
        "-  let value = 41;",
        "Approve write access",
        "Yes, allow once",
        "ask",
    ] {
        assert!(plain.contains(sample), "missing {sample:?} in:\n{plain}");
    }
    assert!(
        rows.iter().all(|row| visible_width(row) <= 100),
        "preview rows overflow:\n{}",
        rows.join("\n")
    );
}

#[test]
fn manager_preview_uses_selected_entry_theme() {
    // A customized theme for the second entry: selecting it must change the
    // preview sample without touching the chrome.
    let mut themed = entry("aurora-night.json", "Aurora Night");
    themed.theme = Some(
        TuiTheme::default().with_footer_permission_ask(neo_tui::primitive::Color::Rgb(1, 2, 3)),
    );
    let mut chrome = NeoChromeState::new("neo", "s", "m", "/tmp");
    chrome.open_theme_manager(vec![entry("solarized-dark.json", "Solarized Dark"), themed]);
    // "Aurora Night" sorts first, so it is the initial selection and preview.
    let preview = chrome
        .theme_manager_state()
        .and_then(|state| state.preview())
        .expect("preview value");
    assert_eq!(
        preview.footer_permission_ask,
        neo_tui::primitive::Color::Rgb(1, 2, 3)
    );
    // Moving to the default-themed entry changes only the preview.
    chrome.handle_focused_dialog_input(InputEvent::Insert('j'));
    let preview = chrome
        .theme_manager_state()
        .and_then(|state| state.preview())
        .expect("preview value");
    assert_eq!(
        preview.footer_permission_ask,
        TuiTheme::default().footer_permission_ask
    );
    // The chrome theme itself is untouched by selection.
    assert_eq!(
        chrome.theme().footer_permission_ask,
        TuiTheme::default().footer_permission_ask
    );
}

#[test]
fn selection_shortcuts_via_arrow_and_home_end() {
    let mut chrome = open_manager();
    let down = InputEvent::Action(KeybindingAction::SelectDown);
    chrome.handle_focused_dialog_input(down.clone());
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.selected_id()),
        Some("broken.json")
    );
    chrome.handle_focused_dialog_input(InputEvent::MoveEnd);
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.selected_id()),
        Some("solarized-dark.json")
    );
    chrome.handle_focused_dialog_input(InputEvent::MoveHome);
    assert_eq!(
        chrome
            .theme_manager_state()
            .and_then(|state| state.selected_id()),
        Some("aurora-night.json")
    );
}
