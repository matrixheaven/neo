use neo_tui::screen_output::{CursorPos, InlineTerminal, LiveRenderer, TerminalFrame};
use neo_tui::transcript::TranscriptPane;

#[test]
fn live_growth_at_terminal_bottom_preserves_unchanged_prefix() {
    let mut screen = vt100::Parser::new(4, 80, 64);
    screen.process(b"shell-0\r\nshell-1\r\nshell-2\r\n");
    let mut renderer = LiveRenderer::new(80, 4);

    let mut initial = Vec::new();
    renderer
        .render_to(
            &mut initial,
            0u16,
            vec!["live-a".to_owned(), "live-b".to_owned()],
            None,
        )
        .expect("initial live frame at terminal bottom");
    screen.process(&initial);

    let mut grown = Vec::new();
    renderer
        .render_to(
            &mut grown,
            0u16,
            vec![
                "live-a".to_owned(),
                "live-b".to_owned(),
                "live-c".to_owned(),
            ],
            None,
        )
        .expect("grown live frame at terminal bottom");
    screen.process(&grown);

    let contents = screen.screen().contents();
    let a = contents.find("live-a").expect("live-a remains visible");
    let b = contents.find("live-b").expect("live-b remains visible");
    let c = contents.find("live-c").expect("live-c is appended");
    assert!(a < b && b < c, "live rows must remain ordered:\n{contents}");
}

#[test]
fn width_resize_redraws_at_absolute_observed_origin() {
    let mut terminal = InlineTerminal::for_test(12, 4);
    terminal
        .render_to(
            &mut Vec::new(),
            &TerminalFrame::new(
                Vec::new(),
                vec!["old-row-one".to_owned(), "old-row-two".to_owned()],
                None,
            ),
        )
        .expect("initial live frame");

    terminal.resize_for_test(6, 4);
    let mut pane = TranscriptPane::new(6, 4);
    pane.push_status("done");
    let update = pane.render_terminal_update(6, 4);
    let mut output = Vec::new();
    terminal
        .render_to(
            &mut output,
            &TerminalFrame::new(
                update.history,
                vec!["new-a".to_owned(), "new-b".to_owned()],
                None,
            ),
        )
        .expect("live frame after ambiguous width resize");
    let output = String::from_utf8(output).expect("ANSI output is UTF-8");

    assert!(
        output.contains("\x1b[2;1H\x1b[2Knew-a"),
        "output: {output:?}"
    );
    assert!(
        output.contains("\x1b[3;1H\x1b[2Knew-b"),
        "output: {output:?}"
    );
    assert!(
        !output.contains("\x1b[J"),
        "resize must not use a relative erase: {output:?}"
    );
    assert!(
        !output.contains("\x1b[1A"),
        "ambiguous geometry must not address the stale live anchor: {output:?}"
    );
}

#[test]
fn height_resize_redraws_live_with_absolute_cup() {
    let live = vec!["same-a".to_owned(), "same-b".to_owned()];
    let mut terminal = InlineTerminal::for_test(12, 4);
    terminal
        .render_to(
            &mut Vec::new(),
            &TerminalFrame::new(Vec::new(), live.clone(), None),
        )
        .expect("initial live frame");

    terminal.resize_for_test(12, 3);
    let mut output = Vec::new();
    terminal
        .render_to(&mut output, &TerminalFrame::new(Vec::new(), live, None))
        .expect("unchanged live frame after height resize");
    let output = String::from_utf8(output).expect("ANSI output is UTF-8");

    assert!(
        output.contains("\x1b[1;1H\x1b[2Ksame-a"),
        "output: {output:?}"
    );
    assert!(
        output.contains("\x1b[2;1H\x1b[2Ksame-b"),
        "output: {output:?}"
    );
    assert!(!output.contains("\x1b[J"), "output: {output:?}");
    assert!(!output.contains("\x1b[1A"), "output: {output:?}");
}

#[test]
fn unchanged_live_frame_emits_no_bytes() {
    let mut renderer = LiveRenderer::new(80, 24);
    renderer
        .render_to(&mut Vec::new(), 0u16, vec!["live".to_owned()], None)
        .expect("first live render");

    let mut second = Vec::new();
    renderer
        .render_to(&mut second, 0u16, vec!["live".to_owned()], None)
        .expect("unchanged live render");

    assert!(second.is_empty());
}

#[test]
fn logical_cursor_state_controls_hardware_cursor_visibility() {
    let mut renderer = LiveRenderer::new(80, 24);
    let mut hidden = Vec::new();
    renderer
        .render_to(&mut hidden, 0u16, vec!["live".to_owned()], None)
        .expect("render without a logical cursor");
    assert!(
        String::from_utf8(hidden)
            .expect("ANSI output is UTF-8")
            .contains("\x1b[?25l")
    );

    let mut shown = Vec::new();
    renderer
        .render_to(
            &mut shown,
            0u16,
            vec!["live".to_owned()],
            Some(CursorPos { row: 0, col: 0 }),
        )
        .expect("render with a logical cursor");
    assert!(
        String::from_utf8(shown)
            .expect("ANSI output is UTF-8")
            .contains("\x1b[?25h")
    );
}

#[test]
fn changed_live_row_clears_only_that_row() {
    let mut renderer = LiveRenderer::new(80, 24);
    renderer
        .render_to(
            &mut Vec::new(),
            0u16,
            vec!["old".to_owned(), "unchanged".to_owned()],
            None,
        )
        .expect("first live render");

    let mut output = Vec::new();
    renderer
        .render_to(
            &mut output,
            0u16,
            vec!["new".to_owned(), "unchanged".to_owned()],
            None,
        )
        .expect("changed live render");
    let output = String::from_utf8(output).expect("ANSI output is UTF-8");

    assert!(output.contains("\x1b[2Knew"));
    assert!(!output.contains("unchanged"));
    assert!(!output.contains("\x1b[2J"));
    assert!(!output.contains("\x1b[3J"));
}

#[test]
fn replacing_live_kitty_image_deletes_only_its_image_id() {
    let mut renderer = LiveRenderer::new(80, 24);
    renderer
        .render_to(
            &mut Vec::new(),
            0u16,
            vec!["\x1b_Ga=T,f=100,i=41,r=1;payload\x1b\\".to_owned()],
            None,
        )
        .expect("first image render");

    let mut output = Vec::new();
    renderer
        .render_to(&mut output, 0u16, vec!["text replacement".to_owned()], None)
        .expect("replace live image");
    let output = String::from_utf8(output).expect("ANSI output is UTF-8");

    assert!(output.contains("\x1b_Ga=d,d=I,i=41,q=2\x1b\\"));
    assert!(!output.contains("\x1b[3J"));
}

#[test]
fn invalid_live_dimensions_do_not_advance_renderer_state() {
    let mut renderer = LiveRenderer::new(5, 2);
    assert!(
        renderer
            .render_to(&mut Vec::new(), 0u16, vec!["too-wide".to_owned()], None)
            .is_err()
    );
    assert!(
        renderer
            .render_to(
                &mut Vec::new(),
                0u16,
                vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
                None,
            )
            .is_err()
    );

    let mut valid = Vec::new();
    renderer
        .render_to(&mut valid, 0u16, vec!["valid".to_owned()], None)
        .expect("valid frame after rejected frames");
    assert!(String::from_utf8(valid).unwrap().contains("valid"));
}
