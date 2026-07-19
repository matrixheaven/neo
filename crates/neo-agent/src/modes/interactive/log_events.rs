//! Extracted: log-event draining helpers for [`InteractiveController`].

use neo_tui::transcript::TranscriptEntry;

use crate::modes::interactive::InteractiveController;

pub(super) const MAX_LOG_EVENTS_PER_TICK: usize = 32;
pub(super) const MAX_CAPTURED_LOG_STATUSES_PER_SESSION: usize = 256;
const LOG_SUPPRESSION_NOTICE: &str = "Some captured log events were suppressed for this session";

impl InteractiveController {
    pub(super) const fn reset_captured_log_budget(&mut self) {
        self.captured_log_status_count = 0;
        self.captured_log_suppression_notified = false;
    }

    pub(super) fn set_log_event_receiver(&mut self, rx: crate::log_capture::CapturedEventReceiver) {
        self.log_event_rx = Some(rx);
    }

    /// Drain any pending captured log events and surface them as transcript
    /// status lines. Called from the terminal loop on every tick.
    pub(super) fn drain_log_events(&mut self) {
        let Some(rx) = self.log_event_rx.as_mut() else {
            return;
        };
        let mut events = Vec::with_capacity(MAX_LOG_EVENTS_PER_TICK);
        while events.len() < MAX_LOG_EVENTS_PER_TICK
            && let Ok(event) = rx.try_recv()
        {
            events.push(event);
        }
        let mut suppressed = rx.take_dropped() > 0;
        for event in events {
            if self.captured_log_status_count >= MAX_CAPTURED_LOG_STATUSES_PER_SESSION {
                suppressed = true;
                continue;
            }
            self.transcript_mut()
                .push_transcript(TranscriptEntry::Status {
                    text: event.message,
                    severity: Some(match event.level.as_str() {
                        "ERROR" => neo_tui::transcript::StatusSeverity::Error,
                        "WARN" | "WARNING" => neo_tui::transcript::StatusSeverity::Warning,
                        _ => neo_tui::transcript::StatusSeverity::Info,
                    }),
                });
            self.captured_log_status_count += 1;
        }
        if suppressed && !self.captured_log_suppression_notified {
            self.transcript_mut()
                .push_transcript(TranscriptEntry::Status {
                    text: LOG_SUPPRESSION_NOTICE.to_owned(),
                    severity: Some(neo_tui::transcript::StatusSeverity::Warning),
                });
            self.captured_log_suppression_notified = true;
        }
    }
}
