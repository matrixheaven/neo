//! Extracted: log-event draining helpers for [`InteractiveController`].

use neo_tui::transcript::TranscriptEntry;

use crate::modes::interactive::InteractiveController;

impl InteractiveController {
    pub(super) fn set_log_event_receiver(
        &mut self,
        rx: tokio::sync::mpsc::UnboundedReceiver<crate::log_capture::CapturedEvent>,
    ) {
        self.log_event_rx = Some(rx);
    }

    /// Drain any pending captured log events and surface them as transcript
    /// status lines. Called from the terminal loop on every tick.
    pub(super) fn drain_log_events(&mut self) {
        let Some(rx) = self.log_event_rx.as_mut() else {
            return;
        };
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        for event in events {
            self.transcript_mut()
                .push_transcript(TranscriptEntry::Status {
                    text: event.message,
                    severity: Some(match event.level.as_str() {
                        "ERROR" => neo_tui::transcript::StatusSeverity::Error,
                        "WARN" | "WARNING" => neo_tui::transcript::StatusSeverity::Warning,
                        _ => neo_tui::transcript::StatusSeverity::Info,
                    }),
                });
        }
    }
}
