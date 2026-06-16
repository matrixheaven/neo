use crate::app::TuiTheme;
use crate::core::{Finalization, Line};

use super::messages::TranscriptEntry;

#[derive(Debug, Default)]
pub struct TranscriptController {
    live: Vec<TranscriptEntry>,
}

impl TranscriptController {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: TranscriptEntry) {
        self.live.push(entry);
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        if let Some(TranscriptEntry::Assistant {
            content,
            finalized: false,
            ..
        }) = self.live.last_mut()
        {
            content.push_str(text);
        } else {
            self.live.push(TranscriptEntry::Assistant {
                thinking: String::new(),
                content: text.to_owned(),
                finalized: false,
            });
        }
    }

    pub fn finalize_active_assistant(&mut self) {
        if let Some(TranscriptEntry::Assistant { finalized, .. }) =
            self.live.iter_mut().rev().find(|entry| {
                matches!(
                    entry,
                    TranscriptEntry::Assistant {
                        finalized: false,
                        ..
                    }
                )
            })
        {
            *finalized = true;
        }
    }

    #[must_use]
    pub fn tail_is_live_assistant(&self) -> bool {
        matches!(
            self.live.last(),
            Some(TranscriptEntry::Assistant {
                finalized: false,
                ..
            })
        )
    }

    #[must_use]
    pub fn live_entries(&self) -> &[TranscriptEntry] {
        &self.live
    }

    pub fn drain_finalized_rows(&mut self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let finalized_count = self
            .live
            .iter()
            .take_while(|entry| entry.finalization() == Finalization::Finalized)
            .count();
        let drained: Vec<TranscriptEntry> = self.live.drain(..finalized_count).collect();
        drained
            .into_iter()
            .flat_map(|entry| entry.render(width, theme))
            .collect()
    }

    #[must_use]
    pub fn render_live_rows(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        self.live
            .iter()
            .flat_map(|entry| entry.render(width, theme))
            .collect()
    }
}
