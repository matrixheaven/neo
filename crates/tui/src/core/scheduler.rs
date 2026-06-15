#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderKind {
    Incremental,
    ForceFull,
}

#[derive(Debug, Default)]
pub struct RenderScheduler {
    dirty: bool,
    force_full: bool,
    pending: Option<RenderKind>,
}

impl RenderScheduler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&mut self, kind: RenderKind) {
        self.dirty = true;
        match (self.pending, kind) {
            (Some(RenderKind::ForceFull), _) => {}
            (_, RenderKind::ForceFull) => {
                self.force_full = true;
                self.pending = Some(RenderKind::ForceFull);
            }
            (None, RenderKind::Incremental) => {
                self.pending = Some(RenderKind::Incremental);
            }
            (Some(RenderKind::Incremental), RenderKind::Incremental) => {}
        }
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    #[must_use]
    pub fn requires_full_redraw(&self) -> bool {
        self.force_full
    }

    pub fn take_next(&mut self) -> Option<RenderKind> {
        self.dirty = false;
        self.force_full = false;
        self.pending.take()
    }
}
