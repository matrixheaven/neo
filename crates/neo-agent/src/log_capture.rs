//! Log capture: redirect tracing output away from terminal stderr into a
//! bounded ring buffer, and surface WARN/ERROR events to the TUI transcript.
//!
//! In TUI mode the global `tracing` subscriber writes formatted log lines into
//! a [`LogCapture`] instead of `std::io::stderr`. The ring buffer keeps the
//! last *N* lines for diagnostics (`/mcp` panel, error context). WARN and
//! ERROR level events are additionally forwarded through an `mpsc` channel so
//! the interactive controller can display them as `TranscriptEntry::Status`
//! lines — keeping them inside the scrollable transcript rather than leaking
//! onto the composer.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing_subscriber::prelude::*;

/// A captured log event forwarded to the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedEvent {
    /// Severity level as a lowercase string (`"warn"`, `"error"`).
    pub level: String,
    /// Human-readable message (the tracing event's formatted message field).
    pub message: String,
}

/// Thread-safe ring buffer for captured log lines.
///
/// Keeps the most recent `capacity` lines. The buffer is shared between the
/// tracing `MakeWriter` (producer) and anyone reading diagnostics (consumer).
#[derive(Clone)]
pub struct LogCapture {
    inner: Arc<Mutex<LogCaptureInner>>,
}

struct LogCaptureInner {
    lines: VecDeque<String>,
    capacity: usize,
    event_tx: mpsc::UnboundedSender<CapturedEvent>,
}

impl LogCapture {
    /// Create a new ring buffer holding at most `capacity` lines, forwarding
    /// WARN/ERROR events to `event_tx`.
    #[must_use]
    pub fn new(capacity: usize, event_tx: mpsc::UnboundedSender<CapturedEvent>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LogCaptureInner {
                lines: VecDeque::with_capacity(capacity),
                capacity,
                event_tx,
            })),
        }
    }

    /// Append a complete log line to the ring buffer.
    pub fn push_line(&self, line: impl Into<String>) {
        let mut inner = self.inner.lock().expect("LogCapture mutex poisoned");
        if inner.lines.len() >= inner.capacity {
            inner.lines.pop_front();
        }
        inner.lines.push_back(line.into());
    }

    /// Forward a structured event (WARN/ERROR) to the TUI channel.
    pub fn push_event(&self, event: CapturedEvent) {
        let inner = self.inner.lock().expect("LogCapture mutex poisoned");
        let _ = inner.event_tx.send(event);
    }

    /// Return a snapshot of the most recent lines (oldest first).
    #[cfg(test)]
    pub fn recent_lines(&self) -> Vec<String> {
        let inner = self.inner.lock().expect("LogCapture mutex poisoned");
        inner.lines.iter().cloned().collect()
    }
}

/// A `MakeWriter` that routes formatted tracing output into a [`LogCapture`]
/// ring buffer, so nothing reaches the terminal's stderr.
///
/// Each call to `make_writer_for` returns a fresh `CaptureSink` that buffers
/// bytes in a `Vec<u8>`, then pushes the completed line on write.
#[derive(Clone)]
pub struct CapturingWriter {
    capture: LogCapture,
}

impl CapturingWriter {
    #[must_use]
    pub fn new(capture: LogCapture) -> Self {
        Self { capture }
    }
}

/// Sink returned by `CapturingWriter` — collects bytes and flushes complete
/// lines into the ring buffer.
pub struct CaptureSink {
    capture: LogCapture,
    buf: Vec<u8>,
}

impl io::Write for CaptureSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&self.buf).into_owned();
        for line in text.lines() {
            if !line.trim().is_empty() {
                self.capture.push_line(line);
            }
        }
        self.buf.clear();
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
    type Writer = CaptureSink;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureSink {
            capture: self.capture.clone(),
            buf: Vec::new(),
        }
    }
}

/// A `tracing_subscriber::Layer` that intercepts WARN and ERROR events and
/// forwards them to a [`LogCapture`] for display in the TUI transcript.
pub struct CapturingLayer {
    capture: LogCapture,
}

/// Initialize tracing for TUI mode: all output goes into an in-memory ring
/// buffer (never stderr), and WARN/ERROR events are forwarded to the returned
/// receiver so the interactive controller can surface them in the transcript.
///
/// Returns the `LogCapture` (for diagnostics access) and the event receiver.
/// If a global subscriber is already installed (e.g. a previous call in tests),
/// this is a no-op and returns `(None, None)`.
pub fn setup_tui_tracing(
    ring_capacity: usize,
) -> (
    Option<LogCapture>,
    Option<mpsc::UnboundedReceiver<CapturedEvent>>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    let capture = LogCapture::new(ring_capacity, tx);
    let writer = CapturingWriter::new(capture.clone());
    let layer = CapturingLayer::new(capture.clone());
    let default_filter = "neo=info,neo_agent_core=info,rmcp=off,warn";
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    let result = tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(filter)
        .with_writer(writer)
        .finish()
        .with(layer)
        .try_init();
    if result.is_ok() {
        (Some(capture), Some(rx))
    } else {
        (None, None)
    }
}

impl CapturingLayer {
    #[must_use]
    pub fn new(capture: LogCapture) -> Self {
        Self { capture }
    }
}

impl<S> tracing_subscriber::Layer<S> for CapturingLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = event.metadata().level();
        if level > &tracing::Level::WARN {
            return;
        }
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        self.capture.push_event(CapturedEvent {
            level: level.as_str().to_owned(),
            message: visitor.format_message(),
        });
    }
}

/// Extracts the `message` field and other named fields from a tracing event.
#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    fields: Vec<(String, String)>,
}

impl MessageVisitor {
    fn format_message(&self) -> String {
        match (&self.message, self.fields.is_empty()) {
            (Some(msg), true) => msg.clone(),
            (Some(msg), false) => {
                let extras = self
                    .fields
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{msg} {extras}")
            }
            (None, false) => self
                .fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" "),
            (None, true) => String::new(),
        }
    }
}

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_owned());
        } else {
            self.fields
                .push((field.name().to_owned(), value.to_owned()));
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else {
            self.fields
                .push((field.name().to_owned(), format!("{value:?}")));
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .push((field.name().to_owned(), value.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_capture(capacity: usize) -> (LogCapture, mpsc::UnboundedReceiver<CapturedEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (LogCapture::new(capacity, tx), rx)
    }

    #[test]
    fn ring_buffer_keeps_last_n_lines() {
        let (capture, _rx) = make_capture(3);
        capture.push_line("line 1");
        capture.push_line("line 2");
        capture.push_line("line 3");
        capture.push_line("line 4");
        assert_eq!(capture.recent_lines(), vec!["line 2", "line 3", "line 4"]);
    }

    #[test]
    fn ring_buffer_empty_returns_empty_vec() {
        let (capture, _rx) = make_capture(5);
        assert!(capture.recent_lines().is_empty());
    }

    #[test]
    fn ring_buffer_under_capacity_keeps_all() {
        let (capture, _rx) = make_capture(10);
        capture.push_line("a");
        capture.push_line("b");
        assert_eq!(capture.recent_lines(), vec!["a", "b"]);
    }

    #[test]
    fn push_event_forwarded_to_channel() {
        let (capture, mut rx) = make_capture(5);
        capture.push_event(CapturedEvent {
            level: "warn".to_owned(),
            message: "MCP server unavailable".to_owned(),
        });
        let event = rx.blocking_recv().expect("event should be in channel");
        assert_eq!(event.level, "warn");
        assert_eq!(event.message, "MCP server unavailable");
    }

    #[test]
    fn push_event_dropped_when_receiver_closed() {
        let (capture, rx) = make_capture(5);
        drop(rx);
        // Should not panic.
        capture.push_event(CapturedEvent {
            level: "error".to_owned(),
            message: "lost".to_owned(),
        });
    }

    // ---- CapturingLayer tests ----

    #[test]
    fn capturing_layer_forwards_warn_events() {
        let (capture, mut rx) = make_capture(10);
        let layer = CapturingLayer::new(capture);
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::warn!("MCP server unavailable");
        let event = rx.blocking_recv().expect("should receive event");
        assert_eq!(event.level, "WARN");
        assert!(event.message.contains("MCP server unavailable"));
    }

    #[test]
    fn capturing_layer_forwards_error_events() {
        let (capture, mut rx) = make_capture(10);
        let layer = CapturingLayer::new(capture);
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::error!("critical failure");
        let event = rx.blocking_recv().expect("should receive event");
        assert_eq!(event.level, "ERROR");
        assert!(event.message.contains("critical failure"));
    }

    #[test]
    fn capturing_layer_skips_info_events() {
        let (capture, mut rx) = make_capture(10);
        let layer = CapturingLayer::new(capture);
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::info!("informational message");
        // Channel should be empty — no event forwarded.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn capturing_layer_includes_structured_fields() {
        let (capture, mut rx) = make_capture(10);
        let layer = CapturingLayer::new(capture);
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::warn!(server_id = "linear", "MCP server unavailable");
        let event = rx.blocking_recv().expect("should receive event");
        assert!(
            event.message.contains("MCP server unavailable"),
            "message was: {}",
            event.message
        );
        assert!(
            event.message.contains("server_id=linear"),
            "message was: {}",
            event.message
        );
    }

    // ---- CapturingWriter tests ----

    use tracing_subscriber::fmt::MakeWriter;

    #[test]
    fn capturing_writer_collects_lines_into_ring_buffer() {
        let (capture, _rx) = make_capture(10);
        let writer = CapturingWriter::new(capture.clone());
        let mut sink = writer.make_writer();
        use std::io::Write;
        writeln!(sink, "hello world").unwrap();
        writeln!(sink, "second line").unwrap();
        sink.flush().unwrap();
        let lines = capture.recent_lines();
        assert!(lines.iter().any(|l| l.contains("hello world")));
        assert!(lines.iter().any(|l| l.contains("second line")));
    }

    #[test]
    fn capturing_writer_skips_empty_lines() {
        let (capture, _rx) = make_capture(10);
        let writer = CapturingWriter::new(capture.clone());
        let mut sink = writer.make_writer();
        use std::io::Write;
        writeln!(sink).unwrap();
        writeln!(sink, "   ").unwrap();
        writeln!(sink, "real content").unwrap();
        sink.flush().unwrap();
        let lines = capture.recent_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("real content"));
    }
}
