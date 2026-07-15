use std::collections::VecDeque;
use std::io::{ErrorKind, IsTerminal, Read, Write};
use std::time::Duration;

use anyhow::Result;
use crossterm::terminal::size;
use neo_tui::input::{InputEvent, InputParser, KeybindingsManager};
use neo_tui::screen_output::InlineTerminal;

pub(crate) trait TerminalEvents {
    fn next_input_event(&mut self) -> Result<InputEvent>;

    fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
        self.next_input_event().map(Some)
    }
}

impl<T: TerminalEvents + ?Sized> TerminalEvents for &mut T {
    fn next_input_event(&mut self) -> Result<InputEvent> {
        (**self).next_input_event()
    }

    fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        (**self).poll_input_event(timeout)
    }
}

pub(super) struct RawStdinEvents {
    parser: InputParser,
    pending: VecDeque<InputEvent>,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    disconnected: bool,
    last_size: Option<(u16, u16)>,
}

impl RawStdinEvents {
    pub(super) fn new(keybindings: KeybindingsManager) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        // Spawn a background thread that blocks on raw stdin reads and forwards
        // byte chunks through the channel. The thread exits on EOF or read error
        // (e.g. terminal closed). The JoinHandle is intentionally dropped — the
        // thread is daemon-like and will be reaped at process exit. When the
        // `RawStdinEvents` is dropped, `rx` is dropped; the next `tx.send()` in
        // the thread fails and the thread exits.
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            read_stdin_chunks(&mut stdin, |chunk| tx.send(chunk.to_vec()).is_ok());
        });
        Self {
            parser: InputParser::with_keybindings(keybindings),
            pending: VecDeque::new(),
            rx,
            disconnected: false,
            last_size: size().ok(),
        }
    }
}

impl Default for RawStdinEvents {
    fn default() -> Self {
        Self::new(KeybindingsManager::default())
    }
}

impl TerminalEvents for RawStdinEvents {
    fn next_input_event(&mut self) -> Result<InputEvent> {
        loop {
            if let Some(input) = self.poll_input_event(Duration::from_millis(250))? {
                return Ok(input);
            }
            if self.disconnected {
                anyhow::bail!("stdin reader closed");
            }
        }
    }

    fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        if let Some(input) = self.pending.pop_front() {
            return Ok(Some(input));
        }

        if self.disconnected {
            return Ok(None);
        }

        let mut got_data = false;
        if !timeout.is_zero() {
            match self.rx.recv_timeout(timeout) {
                Ok(bytes) => {
                    self.pending.extend(self.parser.feed_bytes(&bytes));
                    // Drain any more immediately available bytes
                    while let Ok(more_bytes) = self.rx.try_recv() {
                        self.pending.extend(self.parser.feed_bytes(&more_bytes));
                    }
                    got_data = true;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    self.disconnected = true;
                }
            }
        }

        // Only flush incomplete sequences when no data arrived within the timeout
        // window. Flushing immediately after receiving data could break a partial
        // escape sequence that hasn't fully arrived yet.
        if !got_data {
            self.pending.extend(self.parser.flush_timeout());
        }

        if let Some(event) = self.poll_resize() {
            self.pending.push_back(event);
        }

        Ok(self.pending.pop_front())
    }
}

impl RawStdinEvents {
    fn poll_resize(&mut self) -> Option<InputEvent> {
        resize_event_for_size(&mut self.last_size, size().ok()?)
    }
}

fn resize_event_for_size(
    last_size: &mut Option<(u16, u16)>,
    current_size: (u16, u16),
) -> Option<InputEvent> {
    if *last_size == Some(current_size) {
        return None;
    }
    *last_size = Some(current_size);
    Some(InputEvent::Resize {
        columns: current_size.0,
        rows: current_size.1,
    })
}

fn read_stdin_chunks(reader: &mut impl Read, mut on_chunk: impl FnMut(&[u8]) -> bool) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if !on_chunk(&buf[..n]) {
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Create the platform-appropriate input backend.
pub(super) fn input_events(keybindings: KeybindingsManager) -> impl TerminalEvents {
    RawStdinEvents::new(keybindings)
}
pub(super) struct NeoTerminal {
    tui: InlineTerminal,
    title: Option<String>,
}

impl NeoTerminal {
    pub(super) fn enter() -> Result<Self> {
        let capabilities = super::detect_terminal_capabilities(
            neo_tui::terminal_image::ImageProtocolPreference::Auto,
            std::io::stdout().is_terminal(),
        );
        let (cols, rows) = size()?;
        let tui = InlineTerminal::enter(cols.max(1), rows.max(1), capabilities)?;
        Ok(Self { tui, title: None })
    }

    pub(super) fn draw_tui(
        &mut self,
        tui: &mut neo_tui::NeoTui,
        animation_due: bool,
    ) -> Result<Option<std::time::Instant>> {
        self.sync_title(tui.chrome().terminal_title())?;
        let (cols, rows) = size()?;
        if cols == 0 || rows == 0 {
            return Ok(None);
        }
        let now = std::time::Instant::now();
        if animation_due {
            tui.advance_animation_at(now);
        }
        self.tui.resize(cols, rows);
        let frame = tui.render_terminal_frame_at(usize::from(cols), usize::from(rows), now);
        let mut output = std::io::stdout().lock();
        self.tui.render_to(&mut output, &frame)?;
        tui.acknowledge_history(&frame);
        Ok(frame.next_animation_deadline)
    }

    fn sync_title(&mut self, title: &str) -> Result<()> {
        let sanitized = sanitize_terminal_title(title);
        if self.title.as_deref() == Some(sanitized.as_str()) {
            return Ok(());
        }
        std::io::stdout().write_all(terminal_title_sequence(&sanitized).as_bytes())?;
        self.title = Some(sanitized);
        Ok(())
    }

    pub(super) fn reenter(&mut self) -> Result<()> {
        // Force a full redraw on the next render so the resumed session paints
        // cleanly after the terminal state was disturbed by SIGTSTP.
        self.tui.resume()?;
        Ok(())
    }

    pub(super) fn leave(&mut self) -> Result<()> {
        let mut output = std::io::stdout().lock();
        self.tui.leave(&mut output)?;
        Ok(())
    }
}

const MAX_TERMINAL_TITLE_CHARS: usize = 32;

fn terminal_title_sequence(title: &str) -> String {
    format!("\x1b]0;{}\x07", sanitize_terminal_title(title))
}

fn sanitize_terminal_title(title: &str) -> String {
    let mut sanitized = String::new();
    for character in title.trim().chars() {
        if sanitized.chars().count() >= MAX_TERMINAL_TITLE_CHARS {
            break;
        }
        sanitized.push(if character.is_control() {
            ' '
        } else {
            character
        });
    }
    sanitized.trim().to_owned()
}

/// Compose the full frame (body + chrome) as ANSI strings, without writing to
/// the terminal. Used by tests that need to inspect what would be drawn.
#[cfg(test)]
impl Drop for NeoTerminal {
    fn drop(&mut self) {
        let mut output = std::io::stdout().lock();
        let _ = self.tui.leave(&mut output);
    }
}

impl NeoTerminal {
    pub(super) fn suspend(&mut self) -> Result<()> {
        let mut output = std::io::stdout().lock();
        self.tui.suspend_prepare(&mut output)?;
        drop(output);
        #[cfg(unix)]
        {
            rustix::process::kill_current_process_group(rustix::process::Signal::TSTP)?;
        }
        #[cfg(not(unix))]
        {
            eprintln!("Suspend to background is not supported on this platform");
        }
        self.reenter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind, Read, Result as IoResult};

    struct InterruptedThenBytes {
        reads: usize,
    }

    impl Read for InterruptedThenBytes {
        fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
            self.reads += 1;
            if self.reads == 1 {
                return Err(Error::from(ErrorKind::Interrupted));
            }
            buf[..2].copy_from_slice(b"hi");
            Ok(2)
        }
    }

    #[test]
    fn stdin_reader_continues_after_interrupted_read() {
        let mut reader = InterruptedThenBytes { reads: 0 };
        let mut chunks = Vec::new();

        read_stdin_chunks(&mut reader, |chunk| {
            chunks.push(chunk.to_vec());
            false
        });

        assert_eq!(chunks, vec![b"hi".to_vec()]);
    }

    #[test]
    fn terminal_title_sequence_sanitizes_control_bytes_and_truncates() {
        let title = format!("build\x1b]0;bad\x07{}", "x".repeat(80));
        let sequence = terminal_title_sequence(&title);

        assert_eq!(sequence, "\x1b]0;build ]0;bad xxxxxxxxxxxxxxxxxxx\x07");
    }

    #[test]
    fn terminal_size_changes_emit_resize_events() {
        let mut last_size = Some((80, 24));

        assert_eq!(resize_event_for_size(&mut last_size, (80, 24)), None);
        assert_eq!(
            resize_event_for_size(&mut last_size, (100, 40)),
            Some(InputEvent::Resize {
                columns: 100,
                rows: 40,
            })
        );
        assert_eq!(resize_event_for_size(&mut last_size, (100, 40)), None);
    }
}
