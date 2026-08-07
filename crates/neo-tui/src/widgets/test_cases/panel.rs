use super::*;
use crate::primitive::strip_ansi;

fn plain(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_ansi(l).clone()).collect()
}

fn assert_width(lines: &[String], expected: usize) {
    for line in lines {
        assert_eq!(
            visible_width(line),
            expected,
            "line width mismatch: {line:?}"
        );
    }
}

#[test]
fn btw_panel_renders_empty_state_with_esc_hint() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    let lines = BtwPanel::new(&mut state).render(40, 10);

    // 10 rows -> max body = max(3, 3) - 1 = 2, but empty content is only
    // one line, so the panel collapses to the smallest possible height.
    assert_eq!(lines.len(), 3);
    assert_width(&lines, 40);
    let plain = plain(&lines);
    assert!(plain[0].contains('╭'));
    assert!(plain[0].contains("BTW"));
    assert!(plain[0].contains("Esc close"));
    assert!(!plain[0].contains("scroll"));
    assert!(
        plain
            .iter()
            .any(|l| l.contains("Ready for a side question..."))
    );
    assert!(plain[plain.len() - 1].contains('╰'));
}

#[test]
fn btw_panel_renders_running_turn_with_thinking() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("Explain lifetimes")
            .with_thinking("Let me think...")
            .with_phase(BtwPhase::Running),
    );
    let mut state = BtwPanelState::new(sidecar);
    let lines = BtwPanel::new(&mut state).render(40, 12);

    // 12 rows -> max body = max(3, 4) - 1 = 3; content is exactly 3 lines.
    assert_eq!(lines.len(), 5);
    let plain = plain(&lines);
    assert!(plain.iter().any(|l| l.contains("Q: Explain lifetimes")));
    assert!(plain.iter().any(|l| l.contains("Let me think...")));
    assert!(plain.iter().any(|l| l.contains("Waiting for answer...")));
}

#[test]
fn btw_panel_renders_answered_turn() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("What is 2+2?")
            .with_answer("4")
            .with_phase(BtwPhase::Done),
    );
    let mut state = BtwPanelState::new(sidecar);
    let lines = BtwPanel::new(&mut state).render(40, 30);

    let plain = plain(&lines);
    assert!(plain.iter().any(|l| l.contains("Q: What is 2+2?")));
    assert!(plain.iter().any(|l| l.contains('4')));
    assert!(!plain.iter().any(|l| l.contains("Waiting for answer...")));
}

#[test]
fn btw_panel_renders_busy_status_message() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("explain the trust flow")
            .with_thinking("Thinking through startup config and project context loading...")
            .with_phase(BtwPhase::Running),
    );
    let mut state = BtwPanelState::new(sidecar);
    state.status_message =
        Some("Wait for /btw to finish before sending another question.".to_owned());
    let lines = BtwPanel::new(&mut state).render(80, 20);

    let plain = plain(&lines);
    assert!(
        plain
            .iter()
            .any(|l| l.contains("Q: explain the trust flow"))
    );
    assert!(
        plain.iter().any(|l| {
            l.contains("Thinking through startup config and project context loading...")
        })
    );
    assert!(plain.iter().any(|l| l.contains("Wait for /btw to finish")));

    // The busy notice must appear after the turn, separated by a blank content line.
    let q_idx = plain
        .iter()
        .position(|l| l.contains("Q: explain the trust flow"))
        .expect("question line");
    let status_idx = plain
        .iter()
        .position(|l| l.contains("Wait for /btw to finish"))
        .expect("status line");
    assert!(
        status_idx > q_idx + 1,
        "status should be separated from the question by at least one line"
    );
    let separator_inner = plain[status_idx - 1]
        .trim_start_matches('│')
        .trim_end_matches('│')
        .trim();
    assert!(separator_inner.is_empty(), "blank separator missing");
}

#[test]
fn btw_panel_renders_tool_denied_error() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("Run a tool")
            .with_error("Tool calls are disabled for side questions. Answer with text only.")
            .with_phase(BtwPhase::Failed),
    );
    let mut state = BtwPanelState::new(sidecar);
    let lines = BtwPanel::new(&mut state).render(50, 10);

    assert_eq!(lines.len(), 4);
    let plain = plain(&lines);
    assert!(
        plain
            .iter()
            .any(|l| l.contains("Tool calls are disabled for side questions"))
    );
}

#[test]
fn btw_panel_truncates_long_lines_without_overlapping_border() {
    let sidecar = BtwSidecar::new("btw-1")
        .with_turn(BtwTurn::new("a".repeat(200)).with_phase(BtwPhase::Running));
    let mut state = BtwPanelState::new(sidecar);
    let lines = BtwPanel::new(&mut state).render(20, 20);

    assert_width(&lines, 20);
    let plain = plain(&lines);
    assert!(plain.iter().any(|l| l.starts_with('│')));
    assert!(plain.iter().any(|l| l.ends_with('│')));
}

#[test]
fn btw_panel_caps_height_to_one_third_terminal_rows() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10")
            .with_phase(BtwPhase::Running),
    );
    let mut state = BtwPanelState::new(sidecar);
    let lines = BtwPanel::new(&mut state).render(40, 18);

    // 18 rows -> max body = max(3, 6) - 1 = 5, plus top/bottom borders = 7.
    assert_eq!(lines.len(), 7);
    let plain = plain(&lines);
    assert!(plain[0].contains("↑↓ scroll"));
}

#[test]
fn btw_panel_scrolls_content_with_offset() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8")
            .with_phase(BtwPhase::Running),
    );
    let mut state = BtwPanelState::new(sidecar);
    state.scroll_offset = 2;
    state.follow_tail = false;
    let lines = BtwPanel::new(&mut state).render(40, 18);

    let plain = plain(&lines);
    assert!(!plain.iter().any(|l| l.contains("line1")));
    assert!(plain.iter().any(|l| l.contains("line3")));
}

#[test]
fn btw_panel_renders_narrow_width() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(BtwTurn::new("Hi"));
    let mut state = BtwPanelState::new(sidecar);
    let lines = BtwPanel::new(&mut state).render(8, 10);

    assert_width(&lines, 8);
    let plain = plain(&lines);
    assert!(plain[0].starts_with('╭'));
    assert!(plain[0].ends_with('╮'));
    assert!(plain[plain.len() - 1].starts_with('╰'));
    assert!(plain[plain.len() - 1].ends_with('╯'));
    // Content rows are clipped inside the border, never spilling past the right edge.
    for line in plain.iter().take(plain.len() - 1).skip(1) {
        assert_eq!(line.chars().filter(|c| *c == '│').count(), 2);
    }
}

#[test]
fn btw_panel_renders_answer_markdown_snapshot() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("What to do?")
            .with_answer("- first\n- second")
            .with_phase(BtwPhase::Done),
    );
    let mut state = BtwPanelState::new(sidecar);
    let width = 30;
    let lines = BtwPanel::new(&mut state).render(width, 30);

    assert_width(&lines, width);
    let plain = plain(&lines).join("\n");
    let dashes = |n: usize| "─".repeat(n);
    let spaces = |n: usize| " ".repeat(n);
    let expected = format!(
        "╭ BTW ─ Esc close {top_dashes}╮\n\
         │Q: What to do?{q_pad}│\n\
         │• first{first_pad}│\n\
         │• second{second_pad}│\n\
         ╰{bottom_dashes}╯",
        top_dashes = dashes(11),
        q_pad = spaces(14),
        first_pad = spaces(21),
        second_pad = spaces(20),
        bottom_dashes = dashes(28),
    );
    assert_eq!(plain, expected);
}

#[test]
fn btw_panel_grows_dynamically_with_content() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    let empty = BtwPanel::new(&mut state).render(40, 30);
    assert_eq!(empty.len(), 3);

    state.sidecar.turns.push(
        BtwTurn::new("multi")
            .with_answer("one\ntwo\nthree")
            .with_phase(BtwPhase::Done),
    );
    let grown = BtwPanel::new(&mut state).render(40, 30);
    // Q line + three answer lines = 4 body lines + 2 borders.
    assert_eq!(grown.len(), 6);
}

#[test]
fn btw_panel_trims_thinking_preview_while_running() {
    // Use bracketed markers so the per-line assertions cannot collide with
    // ordinary status copy (the running-phase footer is "Waiting for
    // answer...", which contains the letter 'a' and would falsely match a
    // bare `contains('a')` check).
    let thinking = "[a]\n[b]\n[c]\n[d]\n[e]";
    let sidecar_running = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("think")
            .with_thinking(thinking)
            .with_phase(BtwPhase::Running),
    );
    let mut state = BtwPanelState::new(sidecar_running);
    let lines = BtwPanel::new(&mut state).render(40, 30);
    let plain_running = plain(&lines);
    assert!(!plain_running.iter().any(|l| l.contains("[a]")));
    assert!(!plain_running.iter().any(|l| l.contains("[b]")));
    assert!(!plain_running.iter().any(|l| l.contains("[c]")));
    assert!(plain_running.iter().any(|l| l.contains("[d]")));
    assert!(plain_running.iter().any(|l| l.contains("[e]")));

    let sidecar_done = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("think")
            .with_thinking(thinking)
            .with_phase(BtwPhase::Done),
    );
    let mut state = BtwPanelState::new(sidecar_done);
    let lines = BtwPanel::new(&mut state).render(40, 30);
    let plain_done = plain(&lines);
    assert!(plain_done.iter().any(|l| l.contains("[a]")));
    assert!(plain_done.iter().any(|l| l.contains("[e]")));
}

#[test]
fn btw_panel_follow_tail_and_scroll_controls() {
    let sidecar = BtwSidecar::new("btw-1").with_turn(
        BtwTurn::new("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8")
            .with_phase(BtwPhase::Running),
    );
    let mut state = BtwPanelState::new(sidecar);
    assert!(state.follow_tail);

    let _ = BtwPanel::new(&mut state).render(40, 18);
    assert!(state.follow_tail);
    assert_eq!(state.scroll_offset, state.max_scroll_offset);
    assert!(state.max_scroll_offset > 0);

    state.scroll_up(1);
    assert!(!state.follow_tail);
    assert_eq!(state.scroll_offset, state.max_scroll_offset - 1);

    let _ = BtwPanel::new(&mut state).render(40, 18);
    assert!(!state.follow_tail);
    assert_eq!(state.scroll_offset, state.max_scroll_offset - 1);

    state.scroll_down(100);
    assert!(state.follow_tail);
    assert_eq!(state.scroll_offset, state.max_scroll_offset);
}
