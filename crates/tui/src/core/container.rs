use super::{Component, Finalization, Line};

pub struct Container {
    children: Vec<Box<dyn Component>>,
}

impl Container {
    #[must_use]
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }

    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }

    #[must_use]
    pub fn children(&self) -> &[Box<dyn Component>] {
        &self.children
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let mut rows = Vec::new();
        for child in &mut self.children {
            rows.extend(child.render(width));
        }
        rows
    }

    fn finalization(&self) -> Finalization {
        if self
            .children
            .iter()
            .all(|child| child.finalization() == Finalization::Finalized)
        {
            Finalization::Finalized
        } else {
            Finalization::Live
        }
    }
}

pub struct GutterContainer {
    left: usize,
    right: usize,
    inner: Container,
}

impl GutterContainer {
    #[must_use]
    pub fn new(left: usize, right: usize) -> Self {
        Self {
            left,
            right,
            inner: Container::new(),
        }
    }

    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.inner.add_child(child);
    }
}

impl Component for GutterContainer {
    fn render(&mut self, width: usize) -> Vec<Line> {
        let inner_width = width.saturating_sub(self.left + self.right);
        let prefix = " ".repeat(self.left);
        self.inner
            .render(inner_width)
            .into_iter()
            .map(|row| Line::raw(format!("{prefix}{}", row.to_ansi())))
            .collect()
    }

    fn finalization(&self) -> Finalization {
        self.inner.finalization()
    }
}
