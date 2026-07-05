use std::collections::VecDeque;
use std::io::{ErrorKind, Read};
use std::time::Duration;

use anyhow::Result;
use crossterm::terminal::size;
use neo_tui::input::{InputEvent, InputParser, KeybindingsManager};
use neo_tui::screen_output::TuiRenderer;

pub(crate) trait TerminalEvents {
    fn next_input_event(&mut self) -> Result<InputEvent>;

    fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
        self.next_input_event().map(Some)
    }
}

pub(super) struct RawStdinEvents {
    parser: InputParser,
    pending: VecDeque<InputEvent>,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    disconnected: bool,
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

        Ok(self.pending.pop_front())
    }
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
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

pub(super) struct NeoTerminal {
    tui: TuiRenderer,
}

impl NeoTerminal {
    pub(super) fn enter() -> Result<Self> {
        let tui = TuiRenderer::enter()?;
        Ok(Self { tui })
    }

    pub(super) fn draw_tui(&mut self, tui: &mut neo_tui::NeoTui) -> Result<()> {
        let (cols, rows) = size()?;
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        let (lines, cursor) = tui.render_frame(usize::from(cols), usize::from(rows));
        // Single-buffer differential render: hand the whole frame to
        // TuiRenderer::render, which diffs against the previous frame and
        // rewrites only changed lines in place.
        self.tui.render(lines, cursor)?;
        Ok(())
    }

    pub(super) fn reenter(&mut self) -> Result<()> {
        // Force a full redraw on the next render so the resumed session paints
        // cleanly after the terminal state was disturbed by SIGTSTP.
        self.tui.suspend_resume()?;
        Ok(())
    }
}

/// Compose the full frame (body + chrome) as ANSI strings, without writing to
/// the terminal. Used by tests that need to inspect what would be drawn.
#[cfg(test)]
impl Drop for NeoTerminal {
    fn drop(&mut self) {
        self.tui.leave();
    }
}

impl NeoTerminal {
    pub(super) fn suspend(&mut self) -> Result<()> {
        self.tui.suspend_prepare();
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
}
