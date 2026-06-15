use unicode_width::UnicodeWidthChar;

use crate::ansi::{Style, paint, strip_ansi, visible_width};

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
    pub fn from_spans(spans: Vec<Span>) -> Self {
        Self { spans }
    }

    #[must_use]
    pub fn to_ansi(&self) -> String {
        self.spans.iter().map(Span::to_ansi).collect()
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

        let mut collected = String::new();
        let mut used = 0usize;
        for span in &self.spans {
            let plain = strip_ansi(&span.to_ansi());
            for ch in plain.chars() {
                let cw = ch.width().unwrap_or(0);
                if used + cw > width.saturating_sub(1) {
                    collected.push('…');
                    return Self {
                        spans: vec![Span::raw(collected)],
                    };
                }
                collected.push(ch);
                used += cw;
            }
        }
        Self {
            spans: vec![Span::raw(collected)],
        }
    }
}
