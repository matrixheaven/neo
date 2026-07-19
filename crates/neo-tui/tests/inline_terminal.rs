use std::io::{self, Write};

use neo_tui::screen_output::{CursorPos, InlineTerminal, TerminalFrame};
use neo_tui::transcript::TranscriptPane;

const KITTY_IMAGE: &str = "\x1b_Ga=T,f=100,i=41,r=1;payload\x1b\\";
const DELETE_KITTY_IMAGE: &str = "\x1b_Ga=d,d=I,i=41,q=2\x1b\\";

#[test]
fn failed_transaction_retries_history_and_live_without_advancing_state() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status("committed history");
    let update = pane.render_terminal_update(80, 12);
    let frame = TerminalFrame::new(update.history, vec!["live surface".to_owned()], None);
    let mut terminal = InlineTerminal::for_test(80, 12);

    let mut failing = FailAfterBytes::new(4);
    assert!(terminal.render_to(&mut failing, &frame).is_err());

    let mut retry = Vec::new();
    terminal
        .render_to(&mut retry, &frame)
        .expect("retry complete transaction");
    let retry = String::from_utf8(retry).expect("ANSI output is UTF-8");
    assert!(retry.contains("committed history"));
    assert!(retry.contains("live surface"));
}

#[test]
fn initial_history_starts_at_observed_cursor_and_restores_live_cursor() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status("initial history");
    let update = pane.render_terminal_update(80, 12);
    let frame = TerminalFrame::new(
        update.history,
        vec!["live composer".to_owned()],
        Some(CursorPos { row: 0, col: 4 }),
    );
    let mut terminal = InlineTerminal::for_test_with_cursor(80, 12, 0, 0);
    let mut output = Vec::new();

    terminal
        .render_to(&mut output, &frame)
        .expect("render initial history and live surface");

    let mut screen = vt100::Parser::new(12, 80, 0);
    screen.process(&output);
    let rows = screen.screen().rows(0, 80).collect::<Vec<_>>();
    assert!(rows[0].contains("initial history"), "{rows:#?}");
    assert!(rows[1].contains("live composer"), "{rows:#?}");
    assert_eq!(screen.screen().cursor_position(), (1, 4));
}

#[test]
fn committed_kitty_image_outlives_live_ownership() {
    let mut terminal = InlineTerminal::for_test(80, 12);
    terminal
        .render_to(
            &mut Vec::new(),
            &TerminalFrame::new(Vec::new(), vec![KITTY_IMAGE.to_owned()], None),
        )
        .expect("initial live image");

    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status(KITTY_IMAGE);
    let update = pane.render_terminal_update(80, 12);
    let mut commit = Vec::new();
    terminal
        .render_to(
            &mut commit,
            &TerminalFrame::new(update.history, Vec::new(), None),
        )
        .expect("commit image history");
    let commit = String::from_utf8(commit).expect("ANSI output is UTF-8");
    let committed_image = commit
        .rfind(KITTY_IMAGE)
        .expect("history transaction contains the committed image");
    assert!(
        commit
            .rfind(DELETE_KITTY_IMAGE)
            .is_none_or(|deletion| deletion < committed_image),
        "live cleanup deleted the image after it entered committed history"
    );

    terminal.resize_for_test(100, 20);
    let mut later_cleanup = Vec::new();
    terminal
        .render_to(
            &mut later_cleanup,
            &TerminalFrame::new(Vec::new(), Vec::new(), None),
        )
        .expect("resize committed image terminal");
    terminal
        .leave(&mut later_cleanup)
        .expect("clear remaining live surface");
    assert!(
        !String::from_utf8(later_cleanup)
            .expect("ANSI output is UTF-8")
            .contains(DELETE_KITTY_IMAGE),
        "later live cleanup deleted a committed image"
    );
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
