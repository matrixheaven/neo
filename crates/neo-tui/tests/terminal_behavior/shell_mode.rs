use neo_tui::primitive::strip_ansi;
use neo_tui::shell::{NeoChromeState, PromptEdit};
use neo_tui::transcript::chrome_render::render_chrome_lines;

fn render(app: &NeoChromeState) -> Vec<String> {
    render_chrome_lines(app, 80, 30)
        .lines
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect()
}

#[test]
fn shell_mode_prompt_uses_exclamation_prefix_and_label() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    app.enter_shell_mode();
    app.prompt_mut().apply_edit(PromptEdit::Insert("whoami"));
    let lines = render(&app);
    assert!(lines.iter().any(|line| line.contains("! shell mode")));
    assert!(lines.iter().any(|line| line.contains("! whoami")));
    assert!(!lines.iter().any(|line| line.contains("> whoami")));
}

#[test]
fn footer_shows_shell_badge_only_in_shell_mode() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    assert!(!render(&app).join("\n").contains("[shell]"));
    app.enter_shell_mode();
    assert!(render(&app).join("\n").contains("[shell]"));
}

#[test]
fn queued_shell_command_preview_uses_dollar_prompt_and_non_steer_hint() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    app.pending_input_mut().queue_shell_command("echo hi");
    let rendered = render(&app).join("\n");
    assert!(rendered.contains("$ echo hi"));
    assert!(rendered.contains("will run after current task"));
    assert!(!rendered.contains("ctrl-s to steer"));
}
