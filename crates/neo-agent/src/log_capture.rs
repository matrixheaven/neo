//! Forward structured WARN/ERROR tracing events to the TUI transcript.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::sync::mpsc;
use tracing_subscriber::prelude::*;

const TUI_LOG_CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedEvent {
    pub level: String,
    pub message: String,
}

pub(crate) struct CapturingLayer {
    event_tx: mpsc::Sender<CapturedEvent>,
    dropped_events: Arc<AtomicUsize>,
}

impl CapturingLayer {
    #[must_use]
    fn new(event_tx: mpsc::Sender<CapturedEvent>, dropped_events: Arc<AtomicUsize>) -> Self {
        Self {
            event_tx,
            dropped_events,
        }
    }
}

pub(crate) struct CapturedEventReceiver {
    event_rx: mpsc::Receiver<CapturedEvent>,
    dropped_events: Arc<AtomicUsize>,
}

impl CapturedEventReceiver {
    pub(crate) fn try_recv(&mut self) -> Result<CapturedEvent, mpsc::error::TryRecvError> {
        self.event_rx.try_recv()
    }

    pub(crate) fn take_dropped(&self) -> usize {
        self.dropped_events.swap(0, Ordering::Relaxed)
    }
}

pub(crate) fn capture_channel(capacity: usize) -> (CapturingLayer, CapturedEventReceiver) {
    let (event_tx, event_rx) = mpsc::channel(capacity);
    let dropped_events = Arc::new(AtomicUsize::new(0));
    (
        CapturingLayer::new(event_tx, dropped_events.clone()),
        CapturedEventReceiver {
            event_rx,
            dropped_events,
        },
    )
}

pub(crate) fn setup_tui_tracing() -> Option<CapturedEventReceiver> {
    let (layer, event_rx) = capture_channel(TUI_LOG_CHANNEL_CAPACITY);
    let default_filter = "neo=info,neo_agent_core=info,rmcp=off,warn";
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
        .ok()
        .map(|()| event_rx)
}

impl<S> tracing_subscriber::Layer<S> for CapturingLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = event.metadata().level();
        if level > &tracing::Level::WARN {
            return;
        }
        let permit = match self.event_tx.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(())) => {
                self.dropped_events.fetch_add(1, Ordering::Relaxed);
                return;
            }
            Err(mpsc::error::TrySendError::Closed(())) => return,
        };
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        permit.send(CapturedEvent {
            level: level.as_str().to_owned(),
            message: visitor.format_message(),
        });
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: Option<String>,
    fields: Vec<(String, String)>,
}

impl MessageVisitor {
    fn format_message(&self) -> String {
        let fields = self
            .fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        match (&self.message, fields.is_empty()) {
            (Some(message), true) => message.clone(),
            (Some(message), false) => format!("{message} {fields}"),
            (None, false) => fields,
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
    use std::fmt;

    use super::*;

    struct CountedDebug(Arc<AtomicUsize>);

    impl fmt::Debug for CountedDebug {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fetch_add(1, Ordering::Relaxed);
            formatter.write_str("counted")
        }
    }

    fn capture() -> (CapturingLayer, CapturedEventReceiver) {
        capture_channel(8)
    }

    #[test]
    fn capturing_layer_forwards_warn_events() {
        let (layer, mut rx) = capture();
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::warn!(server_id = "linear", "MCP server unavailable");

        let event = rx.try_recv().expect("warning should be captured");
        assert_eq!(event.level, "WARN");
        assert!(event.message.contains("MCP server unavailable"));
        assert!(event.message.contains("server_id=linear"));
    }

    #[test]
    fn capturing_layer_forwards_error_events() {
        let (layer, mut rx) = capture();
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::error!("critical failure");

        let event = rx.try_recv().expect("error should be captured");
        assert_eq!(event.level, "ERROR");
        assert!(event.message.contains("critical failure"));
    }

    #[test]
    fn capturing_layer_skips_info_events() {
        let (layer, mut rx) = capture();
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::info!("informational message");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn core_warning_reaches_tui_capture_without_stderr_write() {
        let (layer, mut rx) = capture();
        let _guard = tracing_subscriber::registry().with(layer).set_default();

        neo_agent_core::emit_repaired_tool_arguments_warning("Bash", "repaired required fields");

        let event = rx.try_recv().expect("warning should be captured");
        assert_eq!(event.level, "WARN");
        assert!(event.message.contains("tool arguments repaired"));
        assert!(event.message.contains("tool_name=Bash"));
        assert!(event.message.contains("warning=repaired required fields"));
    }

    #[test]
    fn capturing_layer_drops_events_when_channel_is_full() {
        let (layer, mut rx) = capture_channel(1);
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        let debug_calls = Arc::new(AtomicUsize::new(0));
        tracing::warn!("first");
        tracing::warn!(value = ?CountedDebug(debug_calls.clone()), "second");

        assert!(rx.try_recv().is_ok());
        assert_eq!(rx.take_dropped(), 1);
        assert_eq!(debug_calls.load(Ordering::Relaxed), 0);
    }
}
