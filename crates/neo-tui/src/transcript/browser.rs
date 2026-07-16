use super::TranscriptViewport;

/// State owned by the transcript review surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptBrowserState {
    expanded: bool,
    pub(crate) viewport: TranscriptViewport,
}

impl TranscriptBrowserState {
    #[must_use]
    pub const fn new(expanded: bool) -> Self {
        Self {
            expanded,
            viewport: TranscriptViewport::new(),
        }
    }

    #[must_use]
    pub const fn expanded(&self) -> bool {
        self.expanded
    }

    pub fn toggle(&mut self) {
        self.expanded = !self.expanded;
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.viewport.scroll_up(rows);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.viewport.scroll_down(rows);
    }

    pub fn follow_bottom(&mut self) {
        self.viewport.follow_bottom();
    }
}
