use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineImageRenderCache {
    rendered: BTreeMap<String, String>,
}

impl InlineImageRenderCache {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rendered.is_empty()
    }

    pub fn reset_for_full_redraw(&mut self) {
        self.rendered.clear();
    }

    pub fn take_pending(
        &mut self,
        renders: impl IntoIterator<Item = crate::transcript::InlineImageRender>,
    ) -> Vec<crate::transcript::InlineImageRender> {
        let mut pending = Vec::new();
        for render in renders {
            if self.rendered.get(&render.id) == Some(&render.escape_sequence) {
                continue;
            }
            self.rendered
                .insert(render.id.clone(), render.escape_sequence.clone());
            pending.push(render);
        }
        pending
    }
}
