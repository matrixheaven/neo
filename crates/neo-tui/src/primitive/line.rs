use crate::primitive::ansi_escape::{paint, strip_ansi};
use crate::primitive::style::Style;
use crate::primitive::text_layout::{clip_plain_to_width, visible_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    text: String,
    style: Style,
}

impl Span {
    #[must_use]
    pub fn raw(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }

    #[must_use]
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    #[must_use]
    pub fn to_ansi(&self) -> String {
        paint(&self.text, self.style)
    }

    /// The visible (ANSI-stripped) text of this span.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The style applied to this span.
    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }

    #[must_use]
    pub fn visible_width(&self) -> usize {
        visible_width(&self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    spans: Vec<Span>,
}

impl Line {
    #[must_use]
    pub fn raw(text: impl Into<String>) -> Self {
        Self {
            spans: vec![Span::raw(text)],
        }
    }

    #[must_use]
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            spans: vec![Span::styled(text, style)],
        }
    }

    #[must_use]
    pub fn from_spans(spans: Vec<Span>) -> Self {
        Self { spans }
    }

    /// Borrow the underlying spans.
    #[must_use]
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Prepend an unstyled prefix to this line, preserving existing styling.
    #[must_use]
    pub fn prepend_prefix(mut self, prefix: &str) -> Self {
        if !prefix.is_empty() {
            self.spans.insert(0, Span::raw(prefix));
        }
        self
    }

    #[must_use]
    pub fn to_ansi(&self) -> String {
        self.spans.iter().map(Span::to_ansi).collect()
    }

    /// The visible (ANSI-stripped) text of all spans concatenated.
    #[must_use]
    pub fn text(&self) -> String {
        self.spans
            .iter()
            .map(|span| strip_ansi(span.text()))
            .collect()
    }

    #[must_use]
    pub fn visible_width(&self) -> usize {
        self.spans.iter().map(Span::visible_width).sum()
    }

    #[must_use]
    pub fn truncate_to_width(&self, width: usize) -> Self {
        if width == 0 {
            return Self { spans: Vec::new() };
        }

        // Nothing to truncate: preserve original spans and styling.
        let total = self.visible_width();
        if total <= width {
            return self.clone();
        }

        // Reserve one display column for the ellipsis.
        let target = width.saturating_sub(1);
        let mut used = 0usize;
        let mut out = Vec::new();
        for span in &self.spans {
            let span_width = span.visible_width();
            if used + span_width <= target {
                out.push(span.clone());
                used += span_width;
            } else {
                let remaining = target.saturating_sub(used);
                if remaining > 0 {
                    let prefix = clip_plain_to_width(span.text(), remaining);
                    if !prefix.is_empty() {
                        out.push(Span::styled(prefix, span.style));
                    }
                }
                break;
            }
        }
        out.push(Span::raw("…".to_owned()));
        Self { spans: out }
    }
}
