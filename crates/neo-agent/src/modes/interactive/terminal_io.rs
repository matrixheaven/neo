use std::collections::VecDeque;
use std::io::{ErrorKind, IsTerminal, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(not(windows))]
use std::time::Instant;

use anyhow::Result;
use crossterm::terminal::size;
use neo_tui::input::{InputEvent, InputParser, KeybindingsManager};
use neo_tui::screen_output::InlineTerminal;

/// Shared absolute geometry observation between the raw stdin owner and the
/// interactive terminal. Cloneable; no process-global state.
#[derive(Debug, Clone)]
pub(super) struct GeometryObservation {
    inner: Arc<Mutex<GeometryState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GeometryState {
    width: u16,
    height: u16,
    cursor_col: u16,
    cursor_row: u16,
    generation: u64,
}

impl GeometryObservation {
    fn new(width: u16, height: u16, cursor_col: u16, cursor_row: u16) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GeometryState {
                width: width.max(1),
                height: height.max(1),
                cursor_col,
                cursor_row,
                generation: 0,
            })),
        }
    }

    fn snapshot(&self) -> GeometryState {
        *self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn publish(&self, width: u16, height: u16, cursor_col: u16, cursor_row: u16, generation: u64) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.width = width.max(1);
        state.height = height.max(1);
        state.cursor_col = cursor_col;
        state.cursor_row = cursor_row;
        state.generation = generation;
    }

    fn next_generation(&self) -> u64 {
        let state = self.snapshot();
        state.generation.saturating_add(1)
    }
}

#[cfg(not(windows))]
const CURSOR_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(not(windows))]
const CSI_REQUEST_CURSOR: &[u8] = b"\x1b[6n";

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
    geometry: GeometryObservation,
}

impl RawStdinEvents {
    pub(super) fn new(keybindings: KeybindingsManager, geometry: GeometryObservation) -> Self {
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
        let last_size = {
            let snap = geometry.snapshot();
            Some((snap.width, snap.height))
        };
        Self {
            parser: InputParser::with_keybindings(keybindings),
            pending: VecDeque::new(),
            rx,
            disconnected: false,
            last_size,
            geometry,
        }
    }

    fn drain_parser_into_pending(&mut self, bytes: &[u8]) {
        // feed_bytes never yields CPR as InputEvent; it is stored on the parser.
        self.pending.extend(self.parser.feed_bytes(bytes));
    }

    fn observe_cursor_for_size(&mut self, width: u16, height: u16) -> Result<(u16, u16)> {
        #[cfg(windows)]
        {
            let _ = (width, height);
            let (col, row) = crossterm::cursor::position().map_err(|error| {
                anyhow::anyhow!("failed to read console cursor position: {error}")
            })?;
            return Ok((col, row));
        }
        #[cfg(not(windows))]
        {
            // Request CPR through stdout; the matching reply arrives on raw stdin.
            self.parser.discard_cursor_positions();
            {
                let mut stdout = std::io::stdout().lock();
                stdout.write_all(CSI_REQUEST_CURSOR)?;
                stdout.flush()?;
            }
            let deadline = Instant::now() + CURSOR_PROBE_TIMEOUT;
            loop {
                if let Some((col, row)) = self.parser.take_cursor_position() {
                    if col >= width || row >= height {
                        anyhow::bail!(
                            "cursor position report ({col},{row}) outside screen {width}x{height}"
                        );
                    }
                    return Ok((col, row));
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    anyhow::bail!("timed out waiting for cursor position report");
                }
                match self.rx.recv_timeout(remaining) {
                    Ok(bytes) => {
                        self.drain_parser_into_pending(&bytes);
                        while let Ok(more) = self.rx.try_recv() {
                            self.drain_parser_into_pending(&more);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        anyhow::bail!("timed out waiting for cursor position report");
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        self.disconnected = true;
                        anyhow::bail!("stdin reader closed while waiting for cursor position");
                    }
                }
            }
        }
    }

    fn poll_resize(&mut self) -> Result<Option<InputEvent>> {
        let current = match size() {
            Ok(size) if size.0 > 0 && size.1 > 0 => size,
            _ => return Ok(None),
        };
        if self.last_size == Some(current) {
            return Ok(None);
        }
        let generation = self.geometry.next_generation();
        let (cursor_col, cursor_row) = self.observe_cursor_for_size(current.0, current.1)?;
        if size().ok().filter(|size| size.0 > 0 && size.1 > 0) != Some(current) {
            // The CPR belongs to a screen that changed while the probe was in
            // flight. Keep last_size unchanged so the next poll probes again.
            return Ok(None);
        }
        self.geometry
            .publish(current.0, current.1, cursor_col, cursor_row, generation);
        self.last_size = Some(current);
        Ok(Some(InputEvent::Resize {
            columns: current.0,
            rows: current.1,
        }))
    }
}

impl Default for RawStdinEvents {
    fn default() -> Self {
        let (cols, rows) = size().unwrap_or((80, 24));
        let geometry = GeometryObservation::new(cols.max(1), rows.max(1), 0, 0);
        Self::new(KeybindingsManager::default(), geometry)
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
                    self.drain_parser_into_pending(&bytes);
                    // Drain any more immediately available bytes
                    while let Ok(more_bytes) = self.rx.try_recv() {
                        self.drain_parser_into_pending(&more_bytes);
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

        if let Some(event) = self.poll_resize()? {
            self.pending.push_back(event);
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
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Create the platform-appropriate input backend paired with geometry ownership.
pub(super) fn input_events(
    keybindings: KeybindingsManager,
    geometry: GeometryObservation,
) -> impl TerminalEvents {
    RawStdinEvents::new(keybindings, geometry)
}

pub(super) struct NeoTerminal {
    tui: InlineTerminal,
    title: Option<String>,
    geometry: GeometryObservation,
}

impl NeoTerminal {
    pub(super) fn enter() -> Result<(Self, GeometryObservation)> {
        let capabilities = super::detect_terminal_capabilities(
            neo_tui::terminal_image::ImageProtocolPreference::Auto,
            std::io::stdout().is_terminal(),
        );
        // Seed the initial observation before the background stdin reader starts.
        let (cols, rows, cursor_col, cursor_row) = observe_terminal_geometry()?;
        let geometry = GeometryObservation::new(cols, rows, cursor_col, cursor_row);
        let tui = InlineTerminal::enter(cols, rows, capabilities, cursor_col, cursor_row)?;
        Ok((
            Self {
                tui,
                title: None,
                geometry: geometry.clone(),
            },
            geometry,
        ))
    }

    pub(super) fn draw_tui(
        &mut self,
        tui: &mut neo_tui::NeoTui,
        animation_due: bool,
    ) -> Result<Option<std::time::Instant>> {
        self.sync_title(tui.chrome().terminal_title())?;
        let snap = self.geometry.snapshot();
        let (cols, rows) = if snap.width > 0 && snap.height > 0 {
            (snap.width, snap.height)
        } else {
            let (cols, rows) = size()?;
            if cols == 0 || rows == 0 {
                return Ok(None);
            }
            (cols, rows)
        };
        let now = std::time::Instant::now();
        if animation_due {
            tui.advance_animation_at(now);
        }
        self.tui.resize(
            cols,
            rows,
            snap.cursor_col.min(cols.saturating_sub(1)),
            snap.cursor_row.min(rows.saturating_sub(1)),
            snap.generation,
        )?;
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
        let (cols, rows, cursor_col, cursor_row) = observe_terminal_geometry()?;
        let generation = self.geometry.next_generation();
        self.geometry
            .publish(cols, rows, cursor_col, cursor_row, generation);
        self.tui
            .resume(cols, rows, cursor_col, cursor_row, generation)?;
        Ok(())
    }

    pub(super) fn leave(&mut self) -> Result<()> {
        let mut output = std::io::stdout().lock();
        self.tui.leave(&mut output)?;
        Ok(())
    }
}

fn observe_terminal_geometry() -> std::io::Result<(u16, u16, u16, u16)> {
    for _ in 0..2 {
        let (cols, rows) = size()?;
        if cols == 0 || rows == 0 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "terminal reported zero-sized geometry",
            ));
        }
        let (cursor_col, cursor_row) = crossterm::cursor::position()?;
        if size()? == (cols, rows) && cursor_col < cols && cursor_row < rows {
            return Ok((cols, rows, cursor_col, cursor_row));
        }
    }
    Err(std::io::Error::new(
        ErrorKind::InvalidData,
        "terminal geometry changed while observing cursor position",
    ))
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
    fn terminal_resize_waits_for_matching_cursor_generation() {
        let geometry = GeometryObservation::new(80, 24, 0, 0);
        // Simulate a resize observation: generation advances only with cursor.
        let generation = geometry.next_generation();
        assert_eq!(generation, 1);
        geometry.publish(100, 40, 3, 7, generation);
        let snap = geometry.snapshot();
        assert_eq!(snap.width, 100);
        assert_eq!(snap.height, 40);
        assert_eq!(snap.cursor_col, 3);
        assert_eq!(snap.cursor_row, 7);
        assert_eq!(snap.generation, 1);

        // A later observation must carry a higher generation; InlineTerminal
        // rejects stale ones.
        let mut terminal = InlineTerminal::for_test_with_cursor(80, 24, 0, 0);
        assert!(terminal.resize(100, 40, 3, 7, 1).is_ok());
        assert!(
            terminal.resize(120, 50, 0, 0, 1).is_err(),
            "stale generation must fail closed"
        );
        assert!(terminal.resize(120, 50, 0, 0, 2).is_ok());
    }
}
