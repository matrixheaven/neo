use neo_tui::shell::{NeoChromeState, PickerItem, PromptEdit};
use neo_tui::transcript::{TranscriptPane, render_chrome_lines};

#[test]
fn app_shell_prompt_grows_to_eight_lines() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    for _ in 0..9 {
        app.prompt_mut().apply_edit(PromptEdit::Insert("\n"));
    }

    let width = 80;
    let render = render_chrome_lines(&app, width, 30);
    let prompt_box_lines = &render.lines[render.prompt_start_row..render.lines.len() - 1];

    // 8 content rows + top/bottom border = 10 rows.
    assert_eq!(
        prompt_box_lines.len(),
        10,
        "prompt should cap at 8 visible content lines: {prompt_box_lines:?}"
    );
}

#[test]
fn app_shell_prompt_renders_tabs_without_terminal_tab_controls() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    app.prompt_mut()
        .apply_edit(PromptEdit::Insert("\t\t\t\t\t"));

    let width = 80;
    let render = render_chrome_lines(&app, width, 30);
    let content_width = neo_tui::transcript::frame_content_width(width);
    let prompt_box_lines = &render.lines[render.prompt_start_row..render.lines.len() - 1];

    assert!(
        prompt_box_lines.iter().all(|line| !line.contains('\t')),
        "prompt render must not emit raw tab controls: {prompt_box_lines:?}"
    );
    assert!(
        prompt_box_lines
            .iter()
            .all(|line| neo_tui::primitive::visible_width(line) <= content_width),
        "prompt lines must stay inside composer width: {prompt_box_lines:?}"
    );
}

#[test]
fn app_shell_prompt_shows_scroll_indicators_when_clipped() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    for _ in 0..9 {
        app.prompt_mut().apply_edit(PromptEdit::Insert("\n"));
    }
    // Cursor is at the end; viewport should scroll to keep it visible.
    app.prompt_mut()
        .apply_edit_with_width(PromptEdit::MoveEnd, 72);

    let width = 80;
    let render = render_chrome_lines(&app, width, 30);
    let prompt_box_lines = &render.lines[render.prompt_start_row..render.lines.len() - 1];
    let top_border = neo_tui::primitive::strip_ansi(&prompt_box_lines[0]);
    assert!(
        top_border.contains('↑') && top_border.contains("more"),
        "top border should show scroll-up indicator when content is scrolled: {top_border:?}"
    );

    // Move cursor back to the top; viewport should scroll back and show bottom indicator.
    for _ in 0..9 {
        app.prompt_mut()
            .apply_edit_with_width(PromptEdit::MoveUp(72), 72);
    }
    let render = render_chrome_lines(&app, width, 30);
    let prompt_box_lines = &render.lines[render.prompt_start_row..render.lines.len() - 1];
    let bottom_border =
        neo_tui::primitive::strip_ansi(prompt_box_lines.last().expect("prompt has bottom border"));
    assert!(
        bottom_border.contains('↓') && bottom_border.contains("more"),
        "bottom border should show scroll-down indicator when content is clipped: {bottom_border:?}"
    );
}

#[test]
fn app_shell_uses_brand_border_for_non_empty_prompt() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    app.prompt_mut().apply_edit(PromptEdit::Insert("aaaa"));

    let render = render_chrome_lines(&app, 92, 30);
    let top_border = render
        .lines
        .first()
        .expect("composer top border should render");

    assert!(
        top_border.contains("\x1b[38;2;198;120;221m"),
        "non-empty prompt should use Neo brand border: {top_border:?}"
    );
    assert!(
        !top_border.contains("\x1b[38;2;139;148;158m"),
        "non-empty prompt should not stay muted: {top_border:?}"
    );
}

#[test]
fn prompt_completion_keeps_composer_prompt_visible() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut()
        .apply_edit(PromptEdit::Insert("open src/ma"));
    app.open_prompt_completion_picker(
        app.prompt()
            .completion_prefix()
            .expect("completion prefix should exist"),
        [PickerItem::new(
            "src/main.rs",
            "src/main.rs",
            None::<String>,
        )],
    );

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let frame = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(frame.contains("> open src/ma"));
    assert!(
        cursor.is_some(),
        "prompt completion depends on composer cursor"
    );
}
