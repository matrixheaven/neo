use unicode_segmentation::UnicodeSegmentation;

use crate::ansi::{Style, display_width};

use super::{Component, Finalization, Line, Span};

pub struct Text {
    content: String,
    style: Style,
}

impl Text {
    #[must_use]
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            style: Style::default(),
        }
    }

    #[must_use]
    pub fn styled(content: impl Into<String>, style: Style) -> Self {
        Self {
            content: content.into(),
            style,
        }
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<Line> {
        if width == 0 {
            return self
                .content
                .split('\n')
                .map(|line| Line::from_spans(vec![Span::styled(line.to_owned(), self.style)]))
                .collect();
        }

        let mut rows = Vec::new();
        for raw in self.content.split('\n') {
            if raw.is_empty() {
                rows.push(Line::raw(String::new()));
                continue;
            }
            wrap_paragraph(raw, width, self.style, &mut rows);
        }
        rows
    }
}

impl Component for Text {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_lines(width)
    }

    fn finalization(&self) -> Finalization {
        Finalization::Finalized
    }
}

fn wrap_paragraph(raw: &str, width: usize, style: Style, rows: &mut Vec<Line>) {
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in raw.split(' ') {
        let word_width = visible_width(word);
        if word_width > width {
            if !current.is_empty() {
                rows.push(styled_line(std::mem::take(&mut current), style));
                current_width = 0;
            }
            hard_wrap_word(word, width, style, rows, &mut current, &mut current_width);
        } else if current.is_empty() {
            current.push_str(word);
            current_width = word_width;
        } else if current_width + 1 + word_width <= width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            rows.push(styled_line(std::mem::take(&mut current), style));
            current.push_str(word);
            current_width = word_width;
        }
    }
    if !current.is_empty() {
        rows.push(styled_line(current, style));
    }
}

fn hard_wrap_word(
    word: &str,
    width: usize,
    style: Style,
    rows: &mut Vec<Line>,
    current: &mut String,
    current_width: &mut usize,
) {
    let mut line = String::new();
    let mut line_width = 0usize;
    for grapheme in word.graphemes(true) {
        let grapheme_width = display_width(grapheme);
        if line_width + grapheme_width > width && !line.is_empty() {
            rows.push(styled_line(std::mem::take(&mut line), style));
            line_width = 0;
        }
        line.push_str(grapheme);
        line_width += grapheme_width;
    }
    *current = line;
    *current_width = line_width;
}

fn visible_width(text: &str) -> usize {
    display_width(text)
}

fn styled_line(text: String, style: Style) -> Line {
    Line::from_spans(vec![Span::styled(text, style)])
}
