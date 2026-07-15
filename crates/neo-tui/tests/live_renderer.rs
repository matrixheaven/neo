use neo_tui::screen_output::{InlineTerminal, LiveRenderer, TerminalFrame};
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
            vec!["live-a".to_owned(), "live-b".to_owned()],
            None,
        )
        .expect("initial live frame at terminal bottom");
    screen.process(&initial);

    let mut grown = Vec::new();
    renderer
        .render_to(
            &mut grown,
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
fn ambiguous_resize_starts_fresh_anchor_without_erasing_unknown_rows() {
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

    terminal.resize(6, 4);
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
        output.contains("\r\n\r\x1b[2Knew-a"),
        "ambiguous geometry must establish a fresh line: {output:?}"
    );
    assert!(
        !output.contains("\x1b[J"),
        "ambiguous geometry must not erase unknown rows: {output:?}"
    );
    assert!(
        !output.contains("\x1b[1A"),
        "ambiguous geometry must not address the stale live anchor: {output:?}"
    );
}

#[test]
fn height_resize_never_reuses_unverifiable_anchor() {
    let live = vec!["same-a".to_owned(), "same-b".to_owned()];
    let mut terminal = InlineTerminal::for_test(12, 4);
    terminal
        .render_to(
            &mut Vec::new(),
            &TerminalFrame::new(Vec::new(), live.clone(), None),
        )
        .expect("initial live frame");

    terminal.resize(12, 3);
    let mut output = Vec::new();
    terminal
        .render_to(&mut output, &TerminalFrame::new(Vec::new(), live, None))
        .expect("unchanged live frame after height resize");
    let output = String::from_utf8(output).expect("ANSI output is UTF-8");

    assert!(
        output.contains("\r\n\r\x1b[2Ksame-a"),
        "height resize must establish a fresh line: {output:?}"
    );
    assert!(!output.contains("\x1b[J"), "output: {output:?}");
    assert!(!output.contains("\x1b[1A"), "output: {output:?}");
}

#[test]
fn unchanged_live_frame_emits_no_bytes() {
    let mut renderer = LiveRenderer::new(80, 24);
    renderer
        .render_to(&mut Vec::new(), vec!["live".to_owned()], None)
        .expect("first live render");

    let mut second = Vec::new();
    renderer
        .render_to(&mut second, vec!["live".to_owned()], None)
        .expect("unchanged live render");

    assert!(second.is_empty());
}

#[test]
fn changed_live_row_clears_only_that_row() {
    let mut renderer = LiveRenderer::new(80, 24);
    renderer
        .render_to(
            &mut Vec::new(),
            vec!["old".to_owned(), "unchanged".to_owned()],
            None,
        )
        .expect("first live render");

    let mut output = Vec::new();
    renderer
        .render_to(
            &mut output,
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
            vec!["\x1b_Ga=T,f=100,i=41,r=1;payload\x1b\\".to_owned()],
            None,
        )
        .expect("first image render");

    let mut output = Vec::new();
    renderer
        .render_to(&mut output, vec!["text replacement".to_owned()], None)
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
            .render_to(&mut Vec::new(), vec!["too-wide".to_owned()], None)
            .is_err()
    );
    assert!(
        renderer
            .render_to(
                &mut Vec::new(),
                vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
                None,
            )
            .is_err()
    );

    let mut valid = Vec::new();
    renderer
        .render_to(&mut valid, vec!["valid".to_owned()], None)
        .expect("valid frame after rejected frames");
    assert!(String::from_utf8(valid).unwrap().contains("valid"));
}
