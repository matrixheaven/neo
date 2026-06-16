//! Markdown rendering for the live transcript.
//!
//! Parses assistant content with [`pulldown_cmark`] (CommonMark + GFM) and
//! emits styled [`Line`]s. Code blocks are syntax-highlighted with
//! [`syntect`]. Styling mirrors the kimi-code markdown theme.

use std::sync::OnceLock;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::ansi::{Color, Style, visible_width};
use crate::app::TuiTheme;
use crate::core::{Line, Span};

/// Render markdown `text` into styled lines, wrapped to `width`.
#[must_use]
pub fn render_markdown(text: &str, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let width = width.max(1);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(text, opts);
    let mut renderer = MdRenderer::new(width, theme);
    renderer.run(parser);
    renderer.finish()
}

/// Global lazy-loaded syntect syntax set.
fn syntax_set() -> &'static syntect::parsing::SyntaxSet {
    static SYNTAX_SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_nonewlines)
}

/// The default highlight theme set.
fn theme_set() -> &'static syntect::highlighting::ThemeSet {
    static THEME_SET: OnceLock<syntect::highlighting::ThemeSet> = OnceLock::new();
    THEME_SET.get_or_init(syntect::highlighting::ThemeSet::load_defaults)
}

// ---------------------------------------------------------------------------
// Inline style stack
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    fg: Option<Color>,
}

impl InlineStyle {
    fn to_style(self, theme: &TuiTheme) -> Style {
        let mut style = Style::default().fg(self.fg.unwrap_or(theme.assistant));
        style.bold = self.bold;
        style.italic = self.italic;
        style.crossed_out = self.strike;
        style.underline = self.underline;
        style
    }
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

struct MdRenderer<'a> {
    width: usize,
    theme: &'a TuiTheme,
    out: Vec<Line>,
    inline: Vec<Span>,
    inline_style: InlineStyle,
    /// List nesting: each entry is (indent_spaces, marker) e.g. ("  ", "• ").
    list_stack: Vec<(usize, String)>,
    /// Ordered-list start counters, parallel to list_stack for ordered lists.
    ordered_counters: Vec<u64>,
    quote_depth: usize,
    // code block buffer
    code_lang: Option<String>,
    code_buffer: String,
    buffering_code: bool,
    // table state
    table_head: Vec<String>,
    table_rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    in_table_head: bool,
    in_table: bool,
}

impl<'a> MdRenderer<'a> {
    fn new(width: usize, theme: &'a TuiTheme) -> Self {
        Self {
            width,
            theme,
            out: Vec::new(),
            inline: Vec::new(),
            inline_style: InlineStyle::default(),
            list_stack: Vec::new(),
            ordered_counters: Vec::new(),
            quote_depth: 0,
            code_lang: None,
            code_buffer: String::new(),
            buffering_code: false,
            table_head: Vec::new(),
            table_rows: Vec::new(),
            current_row: Vec::new(),
            in_table_head: false,
            in_table: false,
        }
    }

    fn run(&mut self, parser: Parser<'_>) {
        for event in parser {
            // If we are inside a code block, buffer raw text.
            if self.code_lang.is_some() || self.buffering_code {
                if let Event::Text(t) = &event {
                    self.code_buffer.push_str(t);
                    continue;
                }
                if matches!(event, Event::End(TagEnd::CodeBlock)) {
                    self.finish_code_block();
                    continue;
                }
                // Other events inside a fenced block (rare): keep buffering.
                if let Event::Text(t) = &event {
                    self.code_buffer.push_str(t);
                }
                continue;
            }
            self.handle_event(event);
        }
    }

    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(end) => self.end_tag(end),
            Event::Text(text) => {
                if self.in_table {
                    // accumulate into current cell as plain text
                    self.push_text(&text);
                } else {
                    self.push_text(&text);
                }
            }
            Event::Code(code) => {
                let mut style = self.inline_style;
                style.fg = Some(self.theme.accent);
                self.inline
                    .push(Span::styled(code.into_string(), style.to_style(self.theme)));
            }
            Event::SoftBreak | Event::HardBreak => self.flush_inline(),
            Event::Rule => {
                self.flush_inline();
                self.emit_rule();
            }
            Event::TaskListMarker(checked) => {
                let marker = if checked { "[x] " } else { "[ ] " };
                let mut style = self.inline_style;
                style.fg = Some(self.theme.accent);
                self.inline
                    .push(Span::styled(marker.to_owned(), style.to_style(self.theme)));
            }
            Event::InlineHtml(html) | Event::Html(html) => {
                self.push_text(&html);
            }
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                let (bold, underline) = match level {
                    pulldown_cmark::HeadingLevel::H1 => (true, true),
                    _ => (true, false),
                };
                self.inline_style.bold = bold;
                self.inline_style.underline = underline;
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                self.code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => Some(lang.into_string()),
                    pulldown_cmark::CodeBlockKind::Indented => None,
                };
                self.code_buffer.clear();
                self.buffering_code = true;
            }
            Tag::List(start) => {
                if let Some(n) = start {
                    self.ordered_counters.push(n);
                } else {
                    self.ordered_counters.push(0);
                }
            }
            Tag::Item => {
                // determine marker from the top of list_stack/ordered_counters
                self.push_list_marker_for_current_level();
            }
            Tag::Emphasis => self.inline_style.italic = true,
            Tag::Strong => self.inline_style.bold = true,
            Tag::Strikethrough => self.inline_style.strike = true,
            Tag::BlockQuote(_) => {
                self.quote_depth += 1;
            }
            Tag::Link { .. } => {
                self.inline_style.fg = Some(self.theme.accent);
                self.inline_style.underline = true;
            }
            Tag::Table(_) => {
                self.flush_inline();
                self.in_table = true;
                self.table_head.clear();
                self.table_rows.clear();
            }
            Tag::TableHead => {
                self.in_table_head = true;
            }
            Tag::TableRow => {
                self.current_row.clear();
            }
            Tag::TableCell => {}
            _ => {}
        }
    }

    fn end_tag(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph => self.flush_inline(),
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.inline_style = InlineStyle::default();
            }
            TagEnd::CodeBlock => {
                // handled in run() buffering path
                self.finish_code_block();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                self.ordered_counters.pop();
            }
            TagEnd::Item => {
                self.flush_inline();
                // pop the marker pushed at Item start
                self.list_stack.pop();
            }
            TagEnd::Emphasis => self.inline_style.italic = false,
            TagEnd::Strong => self.inline_style.bold = false,
            TagEnd::Strikethrough => self.inline_style.strike = false,
            TagEnd::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::Link => {
                self.inline_style.fg = None;
                self.inline_style.underline = false;
            }
            TagEnd::Table => {
                self.flush_table();
                self.in_table = false;
            }
            TagEnd::TableHead => {
                self.in_table_head = false;
            }
            TagEnd::TableRow => {
                let row = std::mem::take(&mut self.current_row);
                self.table_rows.push(row);
            }
            TagEnd::TableCell => {
                // cell content accumulated in self.inline
                let spans = std::mem::take(&mut self.inline);
                let text = spans_to_plain(&spans);
                if self.in_table_head {
                    self.table_head.push(text);
                } else {
                    self.current_row.push(text);
                }
            }
            _ => {}
        }
    }

    fn push_list_marker_for_current_level(&mut self) {
        let depth = self.list_stack.len();
        let indent = depth * 2;
        let counter = self.ordered_counters.last().copied().unwrap_or(0);
        let marker = if counter > 0 {
            // ordered list
            let n = counter;
            *self.ordered_counters.last_mut().unwrap() = n + 1;
            format!("{n}. ")
        } else {
            "• ".to_owned()
        };
        self.list_stack.push((indent, marker));
    }

    fn push_text(&mut self, text: &str) {
        let mut first = true;
        for part in text.split('\n') {
            if !first {
                self.flush_inline();
            }
            first = false;
            if part.is_empty() {
                continue;
            }
            self.inline.push(Span::styled(
                part.to_owned(),
                self.inline_style.to_style(self.theme),
            ));
        }
    }

    fn flush_inline(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.inline);
        let prefix = self.current_prefix();
        self.emit_wrapped_spans(spans, &prefix);
    }

    fn current_prefix(&self) -> String {
        let mut prefix = String::new();
        for _ in 0..self.quote_depth {
            prefix.push_str("│ ");
        }
        if let Some((indent, marker)) = self.list_stack.last() {
            prefix.push_str(&" ".repeat(*indent));
            prefix.push_str(marker);
        }
        prefix
    }

    fn emit_wrapped_spans(&mut self, spans: Vec<Span>, prefix: &str) {
        let body_width = self.width.saturating_sub(visible_width(prefix)).max(1);
        let single = spans_to_ansi(&spans);
        let wrapped = crate::wrap_width(&single, body_width);
        let indent = " ".repeat(visible_width(prefix));
        for (i, line) in wrapped.into_iter().enumerate() {
            let text = crate::ansi::strip_ansi(&line);
            if i == 0 {
                self.out.push(Line::raw(format!("{prefix}{text}")));
            } else {
                self.out.push(Line::raw(format!("{indent}{text}")));
            }
        }
    }

    fn emit_rule(&mut self) {
        let len = self.width.min(80);
        let rule = "─".repeat(len);
        self.out
            .push(Line::styled(rule, Style::default().fg(self.theme.muted)));
        self.out.push(Line::raw(""));
    }

    fn finish_code_block(&mut self) {
        let lang = self.code_lang.take().unwrap_or_default();
        let code = std::mem::take(&mut self.code_buffer);
        self.buffering_code = false;
        let lines: Vec<&str> = code.trim_end_matches('\n').lines().collect();
        let border = if lang.is_empty() {
            "```".to_owned()
        } else {
            format!("```{lang}")
        };
        self.out.push(Line::styled(
            format!("  {border}"),
            Style::default().fg(self.theme.muted),
        ));
        if lang.eq_ignore_ascii_case("diff") {
            for line in &lines {
                self.emit_diff_line(line);
            }
        } else {
            let highlighted = highlight_code(&code, &lang, self.theme);
            for line in highlighted {
                self.out.push(Line::raw(format!("  {line}")));
            }
        }
        self.out.push(Line::styled(
            "  ```".to_owned(),
            Style::default().fg(self.theme.muted),
        ));
        self.out.push(Line::raw(""));
    }

    fn emit_diff_line(&mut self, line: &str) {
        let (color, text) = if let Some(t) = line.strip_prefix('+') {
            (self.theme.diff_added, t)
        } else if let Some(t) = line.strip_prefix('-') {
            (self.theme.diff_removed, t)
        } else if line.starts_with("@@") {
            (self.theme.diff_hunk, line)
        } else {
            (self.theme.diff_context, line)
        };
        self.out.push(Line::styled(
            format!("  {text}"),
            Style::default().fg(color),
        ));
    }

    fn flush_table(&mut self) {
        let head = std::mem::take(&mut self.table_head);
        let rows = std::mem::take(&mut self.table_rows);
        render_table(&head, &rows, self.width, self.theme, &mut self.out);
        self.out.push(Line::raw(""));
    }

    fn finish(mut self) -> Vec<Line> {
        self.flush_inline();
        if self.out.last().is_some_and(|l| l.text().is_empty()) {
            self.out.pop();
        }
        self.out
    }
}

fn spans_to_ansi(spans: &[Span]) -> String {
    spans.iter().map(Span::to_ansi).collect()
}

fn spans_to_plain(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| crate::ansi::strip_ansi(&s.to_ansi()))
        .collect()
}

fn highlight_code(code: &str, lang: &str, theme: &TuiTheme) -> Vec<String> {
    let fallback = || {
        code.trim_end_matches('\n')
            .lines()
            .map(|l| crate::ansi::paint(l, Style::default().fg(theme.assistant)))
            .collect::<Vec<_>>()
    };
    let ss = syntax_set();
    let ts = theme_set();
    let syntax = if lang.is_empty() {
        None
    } else {
        ss.find_syntax_by_token(lang)
            .or_else(|| ss.find_syntax_by_extension(lang))
    };
    let Some(syntax) = syntax else {
        return fallback();
    };
    let Some(syntax_theme) = ts.themes.get("base16-ocean.dark") else {
        return fallback();
    };
    let mut h = syntect::easy::HighlightLines::new(syntax, syntax_theme);
    let mut out = Vec::new();
    for line in syntect::util::LinesWithEndings::from(code.trim_end_matches('\n')) {
        match h.highlight_line(line, ss) {
            Ok(ranges) => out.push(syntect_to_ansi(&ranges, theme)),
            Err(_) => out.push(crate::ansi::paint(
                line.trim_end_matches('\n'),
                Style::default().fg(theme.assistant),
            )),
        }
    }
    out
}

fn syntect_to_ansi(ranges: &[(syntect::highlighting::Style, &str)], theme: &TuiTheme) -> String {
    ranges
        .iter()
        .map(|(st, text)| {
            let mut style = Style::default();
            style.fg = Some(syntect_color(st.foreground).unwrap_or(theme.assistant));
            if st
                .font_style
                .contains(syntect::highlighting::FontStyle::BOLD)
            {
                style.bold = true;
            }
            if st
                .font_style
                .contains(syntect::highlighting::FontStyle::ITALIC)
            {
                style.italic = true;
            }
            if st
                .font_style
                .contains(syntect::highlighting::FontStyle::UNDERLINE)
            {
                style.underline = true;
            }
            crate::ansi::paint(text, style)
        })
        .collect()
}

fn syntect_color(c: syntect::highlighting::Color) -> Option<Color> {
    let syntect::highlighting::Color { r, g, b, a } = c;
    if a == 0 {
        None
    } else {
        Some(Color::Rgb(r, g, b))
    }
}

fn render_table(
    head: &[String],
    rows: &[Vec<String>],
    width: usize,
    theme: &TuiTheme,
    out: &mut Vec<Line>,
) {
    let ncols = head.len().max(rows.first().map_or(0, Vec::len));
    if ncols == 0 {
        return;
    }
    let min_width = ncols * 3 + 1;
    if width < min_width {
        for cell in head {
            out.push(Line::raw(cell.to_owned()));
        }
        return;
    }

    let cap = 30usize;
    let mut col_widths = vec![0usize; ncols];
    for (i, cell) in head.iter().enumerate() {
        col_widths[i] = col_widths[i].max(visible_width(cell).min(cap));
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < ncols {
                col_widths[i] = col_widths[i].max(visible_width(cell).min(cap));
            }
        }
    }
    let overhead = 3 * ncols + 1;
    let available = width.saturating_sub(overhead);
    let total: usize = col_widths.iter().sum();
    if total > available && available > 0 {
        let scale = available as f64 / total as f64;
        for w in &mut col_widths {
            *w = ((*w as f64) * scale).round() as usize;
        }
        for w in &mut col_widths {
            if *w == 0 {
                *w = 1;
            }
        }
    }

    let border_style = Style::default().fg(theme.muted);
    let header_style = Style::default().fg(theme.assistant).bold();
    let body_style = Style::default().fg(theme.assistant);

    let border_line = |joiners: &[char; 2]| -> String {
        let mut s = String::from(joiners[0]);
        for (i, w) in col_widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push(if i + 1 == ncols {
                joiners[1]
            } else {
                match joiners[0] {
                    '┌' => '┬',
                    '├' => '┼',
                    '└' => '┴',
                    _ => '┬',
                }
            });
        }
        s
    };

    out.push(Line::styled(border_line(&['┌', '┐']), border_style));
    out.push(make_table_row(
        head,
        &col_widths,
        ncols,
        header_style,
        border_style,
    ));
    out.push(Line::styled(border_line(&['├', '┤']), border_style));
    for row in rows {
        out.push(make_table_row(
            row,
            &col_widths,
            ncols,
            body_style,
            border_style,
        ));
    }
    out.push(Line::styled(border_line(&['└', '┘']), border_style));
}

fn make_table_row(
    cells: &[String],
    widths: &[usize],
    ncols: usize,
    cell_style: Style,
    border_style: Style,
) -> Line {
    let mut spans = vec![Span::styled("│", border_style)];
    for i in 0..ncols {
        let content = cells.get(i).map(String::as_str).unwrap_or("");
        let w = widths[i];
        let vw = visible_width(content);
        let pad = w.saturating_sub(vw);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(content.to_owned(), cell_style));
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled("│", border_style));
    }
    Line::from_spans(spans)
}
