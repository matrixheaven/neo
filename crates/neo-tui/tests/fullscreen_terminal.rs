use std::io::{self, Write};

use neo_tui::screen_output::{CursorPos, FullscreenTerminal, TerminalFrame};
use neo_tui::transcript::TranscriptPane;

const KITTY_IMAGE: &str = "\x1b_Ga=T,f=100,i=41,r=1;payload\x1b\\";
const DELETE_KITTY_IMAGE: &str = "\x1b_Ga=d,d=I,i=41,q=2\x1b\\";

#[test]
fn failed_transaction_retries_the_frame_without_advancing_state() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status("committed status");
    let lines = pane.render_visible_slice(80, 12);
    let frame = TerminalFrame::new(lines, None);
    let mut terminal = FullscreenTerminal::for_test(80, 12);

    let mut failing = FailAfterBytes::new(4);
    assert!(terminal.render_to(&mut failing, &frame).is_err());

    let mut retry = Vec::new();
    terminal
        .render_to(&mut retry, &frame)
        .expect("retry complete transaction");
    let retry = String::from_utf8(retry).expect("ANSI output is UTF-8");
    assert!(retry.contains("committed status"));
}

#[test]
fn first_frame_starts_at_fullscreen_origin_and_restores_live_cursor() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status("initial status");
    let lines = pane.render_visible_slice(80, 12);
    let frame = TerminalFrame::new(lines, Some(CursorPos { row: 1, col: 4 }));
    let mut terminal = FullscreenTerminal::for_test(80, 12);
    let mut output = Vec::new();

    terminal
        .render_to(&mut output, &frame)
        .expect("render initial frame");

    let mut screen = vt100::Parser::new(12, 80, 0);
    screen.process(&output);
    let rows = screen.screen().rows(0, 80).collect::<Vec<_>>();
    assert!(rows[0].contains("initial status"), "{rows:#?}");
    assert_eq!(screen.screen().cursor_position(), (1, 4));
}

#[test]
fn unchanged_fullscreen_frame_emits_no_bytes_and_keeps_image_ownership() {
    let mut terminal = FullscreenTerminal::for_test(80, 12);
    terminal
        .render_to(
            &mut Vec::new(),
            &TerminalFrame::new(vec![KITTY_IMAGE.to_owned()], None),
        )
        .expect("initial live image");

    let mut unchanged = Vec::new();
    terminal
        .render_to(
            &mut unchanged,
            &TerminalFrame::new(vec![KITTY_IMAGE.to_owned()], None),
        )
        .expect("unchanged frame");
    assert!(
        unchanged.is_empty(),
        "repeated frame content must emit no bytes: {unchanged:?}"
    );
    assert!(
        !String::from_utf8_lossy(&unchanged).contains(DELETE_KITTY_IMAGE),
        "unchanged frame must not delete its own image"
    );

    // Replacing the image with text deletes exactly the tracked image id.
    let mut replaced = Vec::new();
    terminal
        .render_to(
            &mut replaced,
            &TerminalFrame::new(vec!["text replacement".to_owned()], None),
        )
        .expect("replace live image");
    let replaced = String::from_utf8(replaced).expect("ANSI output is UTF-8");
    assert!(replaced.contains(DELETE_KITTY_IMAGE));
}

struct FailAfterBytes {
    remaining: usize,
}

impl FailAfterBytes {
    const fn new(remaining: usize) -> Self {
        Self { remaining }
    }
}

impl Write for FailAfterBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected failure",
            ));
        }
        let written = bytes.len().min(self.remaining);
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.remaining == 0 {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected failure",
            ))
        } else {
            Ok(())
        }
    }
}
