use std::collections::VecDeque;
use std::io::{ErrorKind, IsTerminal, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(not(windows))]
use std::time::Instant;

use anyhow::Result;
use crossterm::terminal::size;
use neo_tui::input::{InputEvent, InputParser, KeybindingsManager};
use neo_tui::screen_output::FullscreenTerminal;
use neo_tui::transcript::MouseKind;

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
const MAX_PENDING_SCROLL_EVENTS: usize = 32;
const STDIN_CHUNK_QUEUE_CAPACITY: usize = 32;

pub(crate) trait TerminalEvents {
    fn next_input_event(&mut self) -> Result<InputEvent>;

    fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
        self.next_input_event().map(Some)
    }

    /// Re-observe terminal geometry after a suspend/resume round trip. Must
    /// go through the single stdin reader; a second crossterm reader would
    /// race the background stdin thread for the CPR reply. Default impl
    /// reports no observation (test fakes); callers fall back.
    fn reobserve_terminal_geometry(&mut self) -> Result<Option<(u16, u16, u16, u16)>> {
        Ok(None)
    }
}

impl<T: TerminalEvents + ?Sized> TerminalEvents for &mut T {
    fn next_input_event(&mut self) -> Result<InputEvent> {
        (**self).next_input_event()
    }

    fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        (**self).poll_input_event(timeout)
    }

    fn reobserve_terminal_geometry(&mut self) -> Result<Option<(u16, u16, u16, u16)>> {
        (**self).reobserve_terminal_geometry()
    }
}

pub(super) struct RawStdinEvents {
    parser: InputParser,
    pending: VecDeque<InputEvent>,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    last_size: Option<(u16, u16)>,
    geometry: GeometryObservation,
    stdin_disconnected: bool,
}

/// CPR probe failure, distinguished from fatal I/O errors. The resume probe
/// maps the recoverable variants to `Ok(None)` and falls back to the parked
/// origin cursor; resize/startup probes keep them fatal.
#[derive(Debug)]
enum CursorProbeError {
    Io(anyhow::Error),
    TimedOut,
    OutOfRange {
        col: u16,
        row: u16,
        width: u16,
        height: u16,
    },
}

impl std::fmt::Display for CursorProbeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::TimedOut => write!(formatter, "timed out waiting for cursor position report"),
            Self::OutOfRange {
                col,
                row,
                width,
                height,
            } => write!(
                formatter,
                "cursor position report ({col},{row}) outside screen {width}x{height}"
            ),
        }
    }
}

impl std::error::Error for CursorProbeError {}

impl RawStdinEvents {
    pub(super) fn new(keybindings: KeybindingsManager, geometry: GeometryObservation) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(STDIN_CHUNK_QUEUE_CAPACITY);
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
            last_size,
            geometry,
            stdin_disconnected: false,
        }
    }

    fn drain_parser_into_pending(&mut self, bytes: &[u8]) {
        // feed_bytes never yields CPR as InputEvent; it is stored on the parser.
        for event in self.parser.feed_bytes(bytes) {
            self.enqueue_pending(event);
        }
    }

    fn enqueue_pending(&mut self, event: InputEvent) {
        if is_motion_event(&event) {
            // Coalesce consecutive drag/wheel motion: a newer motion event
            // supersedes the motion events queued directly behind it, so a
            // drag flood never accumulates. Press and release events are
            // never dropped and never replaced.
            while self.pending.back().is_some_and(is_motion_event) {
                self.pending.pop_back();
            }
            let motion_count = self
                .pending
                .iter()
                .filter(|pending| is_motion_event(pending))
                .count();
            if motion_count >= MAX_PENDING_SCROLL_EVENTS
                && let Some(index) = self.pending.iter().position(is_motion_event)
            {
                self.pending.remove(index);
            }
        } else {
            // Motion input is transient: a fresh keyboard event should never
            // wait behind stale drag/wheel events captured by a blocking
            // overlay. The one exception is a mouse release closing a drag
            // gesture: it must not discard the gesture's final drag, because
            // the selection state machine needs that last motion to anchor
            // the drag before the release is delivered. Preserve a trailing
            // drag; flush every other motion event.
            let preserves_trailing_drag = matches!(&event, InputEvent::Mouse(mouse) if mouse.kind == MouseKind::Release)
                && self.pending.back().is_some_and(is_drag_event);
            if preserves_trailing_drag {
                if let Some(last_drag) = self.pending.pop_back() {
                    self.pending.retain(|pending| !is_motion_event(pending));
                    self.pending.push_back(last_drag);
                }
            } else {
                self.pending.retain(|pending| !is_motion_event(pending));
            }
        }
        self.pending.push_back(event);
    }

    fn probe_cursor_position(
        &mut self,
        width: u16,
        height: u16,
    ) -> std::result::Result<Option<(u16, u16)>, CursorProbeError> {
        #[cfg(windows)]
        {
            let _ = (width, height);
            let (col, row) = crossterm::cursor::position().map_err(|error| {
                CursorProbeError::Io(anyhow::anyhow!(
                    "failed to read console cursor position: {error}"
                ))
            })?;
            return Ok(Some((col, row)));
        }
        #[cfg(not(windows))]
        {
            // Request CPR through stdout; the matching reply arrives on raw stdin.
            self.parser.discard_cursor_positions();
            {
                let mut stdout = std::io::stdout().lock();
                stdout
                    .write_all(CSI_REQUEST_CURSOR)
                    .map_err(|error| CursorProbeError::Io(error.into()))?;
                stdout
                    .flush()
                    .map_err(|error| CursorProbeError::Io(error.into()))?;
            }
            let deadline = Instant::now() + CURSOR_PROBE_TIMEOUT;
            loop {
                if let Some((col, row)) = self.parser.take_cursor_position() {
                    if col >= width || row >= height {
                        return Err(CursorProbeError::OutOfRange {
                            col,
                            row,
                            width,
                            height,
                        });
                    }
                    return Ok(Some((col, row)));
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(CursorProbeError::TimedOut);
                }
                match self.rx.recv_timeout(remaining) {
                    Ok(bytes) => {
                        self.drain_parser_into_pending(&bytes);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        return Err(CursorProbeError::TimedOut);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        self.stdin_disconnected = true;
                        return Ok(None);
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
        let Some((cursor_col, cursor_row)) = self.probe_cursor_position(current.0, current.1)?
        else {
            return Ok(None);
        };
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

    /// Re-observe geometry after a suspend/resume round trip, using only the
    /// app's single stdin reader (never a second crossterm reader on stdin).
    /// Returns `Ok(None)` when the probe cannot produce a trustworthy
    /// observation (stdin disconnected, no usable size, CPR timeout, or an
    /// out-of-range reply) so callers can fall back instead of failing the
    /// session. Fatal I/O errors still propagate.
    pub(super) fn reobserve_terminal_geometry(&mut self) -> Result<Option<(u16, u16, u16, u16)>> {
        let (cols, rows) = match size() {
            Ok(size) if size.0 > 0 && size.1 > 0 => size,
            _ => return Ok(None),
        };
        let Some((cursor_col, cursor_row)) = (match self.probe_cursor_position(cols, rows) {
            Ok(observed) => observed,
            Err(CursorProbeError::Io(error)) => return Err(error),
            Err(CursorProbeError::TimedOut | CursorProbeError::OutOfRange { .. }) => {
                return Ok(None);
            }
        }) else {
            return Ok(None);
        };
        Ok(Some((cols, rows, cursor_col, cursor_row)))
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
        }
    }

    fn reobserve_terminal_geometry(&mut self) -> Result<Option<(u16, u16, u16, u16)>> {
        RawStdinEvents::reobserve_terminal_geometry(self)
    }

    fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        if self.stdin_disconnected && self.pending.is_empty() {
            anyhow::bail!("stdin reader closed");
        }
        let mut got_data = false;
        if !self.stdin_disconnected && self.pending.is_empty() && !timeout.is_zero() {
            match self.rx.recv_timeout(timeout) {
                Ok(bytes) => {
                    self.drain_parser_into_pending(&bytes);
                    got_data = true;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    self.stdin_disconnected = true;
                }
            }
        } else if !self.stdin_disconnected {
            match self.rx.try_recv() {
                Ok(bytes) => {
                    self.drain_parser_into_pending(&bytes);
                    got_data = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.stdin_disconnected = true;
                }
            }
        }

        // Only flush incomplete sequences when no data arrived within the timeout
        // window. Flushing immediately after receiving data could break a partial
        // escape sequence that hasn't fully arrived yet.
        if !got_data {
            for event in self.parser.flush_timeout() {
                self.enqueue_pending(event);
            }
        }

        if self.pending.is_empty()
            && let Some(event) = self.poll_resize()?
        {
            self.enqueue_pending(event);
        }

        if self.stdin_disconnected && self.pending.is_empty() {
            anyhow::bail!("stdin reader closed");
        }

        Ok(self.pending.pop_front())
    }
}

/// Transient pointer motion (drag or wheel) that a stale queue may drop;
/// press and release events are never classified as motion.
fn is_motion_event(event: &InputEvent) -> bool {
    matches!(event, InputEvent::Mouse(mouse) if mouse.is_motion())
}

fn is_drag_event(event: &InputEvent) -> bool {
    matches!(event, InputEvent::Mouse(mouse) if mouse.kind == MouseKind::Drag)
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
    tui: FullscreenTerminal,
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
        let tui = FullscreenTerminal::enter(cols, rows, capabilities)?;
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

    pub(super) fn reenter(&mut self, geometry: (u16, u16, u16, u16)) -> Result<()> {
        // Force a full redraw on the next render so the resumed session paints
        // cleanly after the terminal state was disturbed by SIGTSTP.
        let (cols, rows, cursor_col, cursor_row) = geometry;
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
    /// Prepare for suspend and stop the process group. The process resumes on
    /// SIGCONT; the caller then re-observes geometry and calls `reenter`.
    /// The geometry probe must NOT happen here: it reads the CPR reply through
    /// the app's single stdin reader, and that reader is only reachable after
    /// this function returns.
    pub(super) fn suspend_prepare(&mut self) -> Result<()> {
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyModifiers, MouseButton};
    use neo_tui::transcript::MouseKind;
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
    fn poll_input_event_reports_stdin_reader_disconnect() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        drop(tx);
        let geometry = GeometryObservation::new(80, 24, 0, 0);
        let mut events = RawStdinEvents {
            parser: InputParser::with_keybindings(KeybindingsManager::default()),
            pending: VecDeque::new(),
            rx,
            last_size: Some((80, 24)),
            geometry,
            stdin_disconnected: false,
        };

        let error = events
            .poll_input_event(Duration::from_millis(1))
            .expect_err("closed stdin channel must not look like an idle timeout");

        assert_eq!(error.to_string(), "stdin reader closed");
    }

    #[test]
    fn poll_input_event_returns_queued_input_before_disconnect() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        tx.send(b"x".to_vec()).expect("stdin chunk is queued");
        drop(tx);
        let geometry = GeometryObservation::new(80, 24, 0, 0);
        let mut events = RawStdinEvents {
            parser: InputParser::with_keybindings(KeybindingsManager::default()),
            pending: VecDeque::new(),
            rx,
            last_size: Some((80, 24)),
            geometry,
            stdin_disconnected: false,
        };

        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(1))
                .expect("queued input must be delivered"),
            Some(InputEvent::Insert('x'))
        );
        let error = events
            .poll_input_event(Duration::from_millis(1))
            .expect_err("disconnect must be reported after queued input");
        assert_eq!(error.to_string(), "stdin reader closed");
    }

    #[test]
    fn fresh_keyboard_input_overtakes_pending_scroll_backlog() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let geometry = GeometryObservation::new(80, 24, 0, 0);
        let mut events = RawStdinEvents {
            parser: InputParser::with_keybindings(KeybindingsManager::default()),
            pending: VecDeque::new(),
            rx,
            last_size: Some((80, 24)),
            geometry,
            stdin_disconnected: false,
        };
        for _ in 0..(MAX_PENDING_SCROLL_EVENTS * 4) {
            events.enqueue_pending(wheel_down());
        }
        tx.send(b"x".to_vec()).expect("stdin chunk is queued");

        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(0))
                .expect("poll input"),
            Some(InputEvent::Insert('x'))
        );
        assert!(events.pending.is_empty());
    }

    #[test]
    fn drag_motion_coalesces_but_press_and_release_are_never_dropped() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let geometry = GeometryObservation::new(80, 24, 0, 0);
        let mut events = RawStdinEvents {
            parser: InputParser::with_keybindings(KeybindingsManager::default()),
            pending: VecDeque::new(),
            rx,
            last_size: Some((80, 24)),
            geometry,
            stdin_disconnected: false,
        };
        events.enqueue_pending(mouse_event(MouseKind::Press, 5, 3));
        for column in 6..20 {
            events.enqueue_pending(mouse_event(MouseKind::Drag, column, 3));
        }
        // Consecutive drags collapse to the latest one.
        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(0))
                .expect("poll input"),
            Some(mouse_event(MouseKind::Press, 5, 3))
        );
        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(0))
                .expect("poll input"),
            Some(mouse_event(MouseKind::Drag, 19, 3))
        );
        // A release must not discard the gesture's final drag: the last
        // motion is delivered before the release closes the gesture.
        events.enqueue_pending(mouse_event(MouseKind::Drag, 20, 3));
        events.enqueue_pending(mouse_event(MouseKind::Release, 20, 3));
        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(0))
                .expect("poll input"),
            Some(mouse_event(MouseKind::Drag, 20, 3))
        );
        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(0))
                .expect("poll input"),
            Some(mouse_event(MouseKind::Release, 20, 3))
        );
        assert!(events.pending.is_empty());
    }

    #[test]
    fn drag_release_preserves_last_motion_in_same_batch() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let geometry = GeometryObservation::new(80, 24, 0, 0);
        let mut events = RawStdinEvents {
            parser: InputParser::with_keybindings(KeybindingsManager::default()),
            pending: VecDeque::new(),
            rx,
            last_size: Some((80, 24)),
            geometry,
            stdin_disconnected: false,
        };
        // One raw byte batch: press, several drags, release. SGR coordinates
        // are one-based on the wire and become zero-based in the parser.
        let mut batch = b"\x1b[<0;5;3M".to_vec();
        for column in 6..20 {
            batch.extend_from_slice(format!("\x1b[<32;{column};3M").as_bytes());
        }
        batch.extend_from_slice(b"\x1b[<3;20;3m");
        events.drain_parser_into_pending(&batch);

        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(0))
                .expect("poll input"),
            Some(mouse_event(MouseKind::Press, 4, 2))
        );
        // Drags coalesce, but the release must not discard the last one.
        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(0))
                .expect("poll input"),
            Some(mouse_event(MouseKind::Drag, 18, 2))
        );
        assert_eq!(
            events
                .poll_input_event(Duration::from_millis(0))
                .expect("poll input"),
            Some(mouse_event(MouseKind::Release, 19, 2))
        );
        assert!(events.pending.is_empty());
    }

    fn wheel_down() -> InputEvent {
        InputEvent::Mouse(neo_tui::transcript::MouseEvent {
            kind: MouseKind::ScrollDown,
            button: MouseButton::Left,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn mouse_event(kind: MouseKind, column: u16, row: u16) -> InputEvent {
        InputEvent::Mouse(neo_tui::transcript::MouseEvent {
            kind,
            button: MouseButton::Left,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
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

        // A later observation must carry a higher generation; FullscreenTerminal
        // rejects stale ones.
        let mut terminal = FullscreenTerminal::for_test_with_cursor(80, 24, 0, 0);
        assert!(terminal.resize(100, 40, 3, 7, 1).is_ok());
        assert!(
            terminal.resize(120, 50, 0, 0, 1).is_err(),
            "stale generation must fail closed"
        );
        assert!(terminal.resize(120, 50, 0, 0, 2).is_ok());
    }
}
