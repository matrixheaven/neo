use std::collections::VecDeque;

use crate::primitive::ansi_escape::AnsiParser;

/// A bounded, streaming live-output tail.
///
/// Chunks are sanitized incrementally and reassembled into lines. A trailing
/// partial line is held back until a newline arrives or the owner is
/// finalized, so arbitrary chunk boundaries do not create spurious line breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveOutput {
    lines: VecDeque<String>,
    incomplete: String,
    parser: AnsiParser,
    char_count: usize,
    dropped_lines: usize,
    max_lines: usize,
    max_chars: usize,
}

impl LiveOutput {
    #[must_use]
    pub(crate) fn new(max_lines: usize, max_chars: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            incomplete: String::new(),
            parser: AnsiParser::new(),
            char_count: 0,
            dropped_lines: 0,
            max_lines,
            max_chars,
        }
    }

    /// Append a chunk of raw tool/shell output. Returns `true` if visible state changed.
    pub(crate) fn append(&mut self, chunk: &str) -> bool {
        let before_len = self.incomplete.len();
        self.parser.consume(chunk, &mut self.incomplete);
        if self.incomplete.len() == before_len && self.incomplete.is_empty() {
            // No visible text was produced and there was nothing pending.
            return false;
        }

        // Extract complete lines, leaving any trailing partial line in `incomplete`.
        let mut extracted = 0;
        while let Some(pos) = self.incomplete[extracted..].find('\n') {
            let absolute = extracted + pos;
            let line = self.incomplete[extracted..absolute].to_owned();
            extracted = absolute + 1;
            self.push_line(line);
        }
        if extracted > 0 {
            self.incomplete.replace_range(..extracted, "");
        }
        true
    }

    /// Borrow the complete lines and any trailing partial line for rendering.
    #[must_use]
    pub(crate) fn tail(&self) -> Vec<String> {
        let mut result: Vec<String> = self.lines.iter().cloned().collect();
        if !self.incomplete.is_empty() {
            result.push(self.incomplete.clone());
        }
        result
    }

    /// Flush the trailing partial line and return the full tail, then reset.
    pub(crate) fn finalize(&mut self) -> (Vec<String>, usize) {
        let mut result: Vec<String> = self.lines.drain(..).collect();
        if !self.incomplete.is_empty() {
            result.push(std::mem::take(&mut self.incomplete));
        }
        let dropped = self.dropped_lines;
        self.reset();
        (result, dropped)
    }

    pub(crate) fn reset(&mut self) {
        self.lines.clear();
        self.incomplete.clear();
        self.parser.reset();
        self.char_count = 0;
        self.dropped_lines = 0;
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.incomplete.is_empty()
    }

    #[must_use]
    pub(crate) fn dropped_lines(&self) -> usize {
        self.dropped_lines
    }

    fn push_line(&mut self, line: String) {
        self.char_count += line.chars().count();
        self.lines.push_back(line);
        self.trim();
    }

    fn trim(&mut self) {
        while self.lines.len() > self.max_lines || self.char_count > self.max_chars {
            let Some(line) = self.lines.pop_front() else {
                // Never drop the partial tail; stop when only it remains.
                break;
            };
            self.char_count = self.char_count.saturating_sub(line.chars().count());
            self.dropped_lines += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_output_reassembles_split_lines() {
        let mut live = LiveOutput::new(6, 50_000);
        live.append("line one\nline ");
        assert_eq!(live.tail(), ["line one", "line "]);
        live.append("two\nline three");
        assert_eq!(live.tail(), ["line one", "line two", "line three"]);
    }

    #[test]
    fn live_output_finalizes_partial_tail_once() {
        let mut live = LiveOutput::new(6, 50_000);
        live.append("a\nb");
        let (lines, dropped) = live.finalize();
        assert_eq!(lines, ["a", "b"]);
        assert_eq!(dropped, 0);
        assert!(live.is_empty());
    }

    #[test]
    fn live_output_evicts_old_lines_but_keeps_partial_tail() {
        let mut live = LiveOutput::new(2, 1_000);
        live.append("one\ntwo\nthree\npart");
        assert_eq!(live.tail(), ["two", "three", "part"]);
        assert_eq!(live.dropped_lines(), 1);
    }

    #[test]
    fn live_output_reassembles_split_ansi() {
        let mut live = LiveOutput::new(6, 50_000);
        live.append("\x1b[3");
        live.append("1mred\x1b[0m\n\x1b]0;ti");
        assert_eq!(live.tail(), ["red"]);
        live.append("tle\x07green");
        assert_eq!(live.tail(), ["red", "green"]);
    }
}
