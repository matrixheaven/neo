//! Forward structured WARN/ERROR tracing events to the TUI transcript.

use tokio::sync::mpsc;
use tracing_subscriber::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedEvent {
    pub level: String,
    pub message: String,
}

pub struct CapturingLayer {
    event_tx: mpsc::UnboundedSender<CapturedEvent>,
}

impl CapturingLayer {
    #[must_use]
    pub fn new(event_tx: mpsc::UnboundedSender<CapturedEvent>) -> Self {
        Self { event_tx }
    }
}

pub fn setup_tui_tracing() -> Option<mpsc::UnboundedReceiver<CapturedEvent>> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let layer = CapturingLayer::new(event_tx);
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
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let _ = self.event_tx.send(CapturedEvent {
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
    use super::*;

    fn capture() -> (CapturingLayer, mpsc::UnboundedReceiver<CapturedEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (CapturingLayer::new(tx), rx)
    }

    #[test]
    fn capturing_layer_forwards_warn_events() {
        let (layer, mut rx) = capture();
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::warn!(server_id = "linear", "MCP server unavailable");

        let event = rx.blocking_recv().expect("warning should be captured");
        assert_eq!(event.level, "WARN");
        assert!(event.message.contains("MCP server unavailable"));
        assert!(event.message.contains("server_id=linear"));
    }

    #[test]
    fn capturing_layer_forwards_error_events() {
        let (layer, mut rx) = capture();
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        tracing::error!("critical failure");

        let event = rx.blocking_recv().expect("error should be captured");
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

        let event = rx.blocking_recv().expect("warning should be captured");
        assert_eq!(event.level, "WARN");
        assert!(event.message.contains("tool arguments repaired"));
        assert!(event.message.contains("tool_name=Bash"));
        assert!(event.message.contains("warning=repaired required fields"));
    }
}
