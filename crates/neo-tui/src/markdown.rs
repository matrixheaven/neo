//! Markdown rendering for the live transcript.
//!
//! Parses assistant content with [`pulldown_cmark`] (`CommonMark` + GFM) and
//! emits styled [`Line`]s. Code blocks are syntax-highlighted with
//! [`syntect`]. Styling mirrors the Neo markdown theme.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{
    Color, Line, Span, Style, clip_plain_to_width, clip_visible_to_width, pad_to_width,
    truncate_to_width, visible_width, wrap_width,
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Inner horizontal padding between the side border and code content.
const CODE_SIDE_PADDING: usize = 2;
/// Minimum width for a code block box. Below this we fall back to plain text.
const CODE_MIN_BOX_WIDTH: usize = 12;

/// Render markdown `text` into styled lines, wrapped to `width`.
///
/// `first_prefix` is prepended to the very first emitted line;
/// `cont_prefix` is prepended to every continuation line. Both are visible
/// width (e.g. `"● "` and `"  "` so wrapped body lines align under the
/// bullet, not under the bullet glyph).
#[must_use]
pub fn render_markdown(
    text: &str,
    width: usize,
    theme: &TuiTheme,
    first_prefix: &str,
    cont_prefix: &str,
) -> Vec<Line> {
    let width = width.max(1);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(text, opts);
    let mut renderer = MdRenderer::new(width, theme, first_prefix, cont_prefix);
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

/// Global cache for syntax-highlighted code blocks, keyed by (hash, lang).
/// Avoids re-running syntect regex tokenization on identical code across
/// renders. Bounded to 256 entries to cap memory.
static HIGHLIGHT_CACHE: OnceLock<Mutex<HashMap<(u64, String), Vec<String>>>> = OnceLock::new();
const HIGHLIGHT_CACHE_CAP: usize = 256;

fn highlight_cache() -> &'static Mutex<HashMap<(u64, String), Vec<String>>> {
    HIGHLIGHT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// FxHash-style hash for quick cache key generation.
fn quick_hash(s: &str) -> u64 {
    // Use std DefaultHasher — not cryptographic, just a cache key.
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Inline style stack
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    fg: Option<Color>,
}

impl InlineStyle {
    fn to_style(self, theme: &TuiTheme) -> Style {
        let mut style = Style::default().fg(self.fg.unwrap_or(theme.text_primary));
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
    /// List nesting: each entry is (`indent_spaces`, marker) e.g. ("  ", "• ").
    list_stack: Vec<(usize, String)>,
    /// Ordered-list start counters, parallel to `list_stack` for ordered lists.
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
    /// Prefix prepended to the very first emitted line (e.g. "● ").
    first_prefix: String,
    /// Prefix prepended to every continuation line (e.g. "  ").
    cont_prefix: String,
}

impl<'a> MdRenderer<'a> {
    fn new(width: usize, theme: &'a TuiTheme, first_prefix: &str, cont_prefix: &str) -> Self {
        // Reserve space for the continuation prefix so wrapped lines never
        // overflow once we indent them in `finish()`.
        let reserved = visible_width(cont_prefix).max(visible_width(first_prefix));
        let width = width.saturating_sub(reserved).max(1);
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
            first_prefix: first_prefix.to_owned(),
            cont_prefix: cont_prefix.to_owned(),
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
                style.fg = Some(self.theme.brand);
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
                style.fg = Some(self.theme.brand);
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
                self.inline_style.fg = Some(self.theme.brand);
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
        self.emit_wrapped_spans(&spans, &prefix);
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

    fn emit_wrapped_spans(&mut self, spans: &[Span], prefix: &str) {
        let body_width = self.width.saturating_sub(visible_width(prefix)).max(1);
        let wrapped = wrap_spans(spans, body_width);
        let indent = " ".repeat(visible_width(prefix));
        for (i, line_spans) in wrapped.into_iter().enumerate() {
            let mut line = Line::from_spans(line_spans);
            line = if i == 0 {
                line.prepend_prefix(prefix)
            } else {
                line.prepend_prefix(&indent)
            };
            self.out.push(line);
        }
    }

    fn emit_rule(&mut self) {
        let len = self.width.min(80);
        let rule = "─".repeat(len);
        self.out.push(Line::styled(
            rule,
            Style::default().fg(self.theme.text_muted),
        ));
        self.out.push(Line::raw(""));
    }

    fn finish_code_block(&mut self) {
        let lang = self.code_lang.take().unwrap_or_default();
        let code = std::mem::take(&mut self.code_buffer);
        self.buffering_code = false;

        if self.width < CODE_MIN_BOX_WIDTH {
            self.emit_plain_code_block(&lang, &code);
            return;
        }

        let border_style = Style::default().fg(self.theme.text_muted);
        let brand_style = Style::default().fg(self.theme.brand);

        // Content-adaptive box width.
        let raw_lines: Vec<&str> = code.trim_end_matches('\n').lines().collect();
        let max_content_width = raw_lines
            .iter()
            .map(|line| {
                let display = if lang.eq_ignore_ascii_case("diff") {
                    line.strip_prefix('+')
                        .or_else(|| line.strip_prefix('-'))
                        .unwrap_or(line)
                } else {
                    line
                };
                visible_width(display)
            })
            .max()
            .unwrap_or(0);
        let header_label_width = if lang.is_empty() {
            1
        } else {
            visible_width(&format!("─ {lang} "))
        };
        let desired_inner_width = max_content_width + 2 * CODE_SIDE_PADDING;
        let horz_len = desired_inner_width
            .max(header_label_width)
            .max(CODE_MIN_BOX_WIDTH - 2)
            .min(self.width - 2);
        let content_width = horz_len.saturating_sub(2 * CODE_SIDE_PADDING).max(1);

        // Top border with language label. Border chars and fill use border_style;
        // only the language text itself uses brand_style so the frame stays uniform.
        let mut top_spans = vec![Span::styled("╭", border_style)];
        if lang.is_empty() {
            top_spans.push(Span::styled("─".repeat(horz_len), border_style));
        } else {
            let label_prefix = "─ ";
            let label_suffix = " ";
            let lang_text = lang.as_str();
            let fill_width = horz_len.saturating_sub(
                visible_width(label_prefix)
                    + visible_width(lang_text)
                    + visible_width(label_suffix),
            );
            top_spans.push(Span::styled(label_prefix.to_owned(), border_style));
            top_spans.push(Span::styled(lang_text.to_owned(), brand_style));
            top_spans.push(Span::styled(label_suffix.to_owned(), border_style));
            top_spans.push(Span::styled("─".repeat(fill_width), border_style));
        }
        top_spans.push(Span::styled("╮", border_style));
        self.out.push(Line::from_spans(top_spans));

        // Content lines.
        if raw_lines.is_empty() {
            self.emit_code_content_line("", content_width, border_style);
        } else if lang.eq_ignore_ascii_case("diff") {
            for line in raw_lines {
                self.emit_diff_box_line(line, content_width, border_style);
            }
        } else {
            let highlighted = highlight_code(&code, &lang, self.theme);
            for line in highlighted {
                let fitted = fit_ansi_line_to_width(&line, content_width);
                self.emit_code_content_line(&fitted, content_width, border_style);
            }
        }

        // Bottom border.
        let bottom_inner = "─".repeat(horz_len);
        self.out.push(Line::from_spans(vec![
            Span::styled("╰", border_style),
            Span::styled(bottom_inner, border_style),
            Span::styled("╯", border_style),
        ]));

        // Trailing blank line.
        self.out.push(Line::raw(""));
    }

    fn emit_code_content_line(&mut self, text: &str, content_width: usize, border_style: Style) {
        let fitted = pad_to_width(text, content_width);
        self.out.push(Line::from_spans(vec![
            Span::styled("│", border_style),
            Span::raw(" ".repeat(CODE_SIDE_PADDING)),
            Span::raw(fitted),
            Span::raw(crate::primitive::RESET.to_string()),
            Span::raw(" ".repeat(CODE_SIDE_PADDING)),
            Span::styled("│", border_style),
        ]));
    }

    fn emit_diff_box_line(&mut self, line: &str, content_width: usize, border_style: Style) {
        let (color, text) = if let Some(t) = line.strip_prefix('+') {
            (self.theme.diff_added, t)
        } else if let Some(t) = line.strip_prefix('-') {
            (self.theme.diff_removed, t)
        } else if line.starts_with("@@") {
            (self.theme.diff_hunk, line)
        } else {
            (self.theme.diff_context, line)
        };
        let fitted = pad_to_width(&truncate_to_width(text, content_width), content_width);
        self.out.push(Line::from_spans(vec![
            Span::styled("│", border_style),
            Span::raw(" ".repeat(CODE_SIDE_PADDING)),
            Span::styled(fitted, Style::default().fg(color)),
            Span::raw(crate::primitive::RESET.to_string()),
            Span::raw(" ".repeat(CODE_SIDE_PADDING)),
            Span::styled("│", border_style),
        ]));
    }

    fn emit_plain_code_block(&mut self, lang: &str, code: &str) {
        let border = if lang.is_empty() {
            "```".to_owned()
        } else {
            format!("```{lang}")
        };
        self.out.push(Line::styled(
            format!("  {border}"),
            Style::default().fg(self.theme.text_muted),
        ));
        for raw_line in code.trim_end_matches('\n').lines() {
            self.out.push(Line::raw(format!("  {raw_line}")));
        }
        self.out.push(Line::styled(
            "  ```".to_owned(),
            Style::default().fg(self.theme.text_muted),
        ));
        self.out.push(Line::raw(""));
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
        // Apply the outer prefix: the very first emitted line gets
        // `first_prefix` (e.g. "● "), every subsequent line gets
        // `cont_prefix` (e.g. "  ") so wrapped body aligns under the bullet.
        let first = self.first_prefix.clone();
        let cont = self.cont_prefix.clone();
        let len = self.out.len();
        for i in 0..len {
            let prefix = if i == 0 {
                first.as_str()
            } else {
                cont.as_str()
            };
            let line = self.out[i].clone();
            self.out[i] = line.prepend_prefix(prefix);
        }
        self.out
    }
}

fn spans_to_plain(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| crate::primitive::strip_ansi(&s.to_ansi()))
        .collect()
}

/// Wrap a sequence of styled spans to a maximum visible width, preserving the
/// style of each original span. Spans that fit entirely on the current line are
/// kept intact; spans that would overflow are split and continued on the next
/// line.
fn wrap_spans(spans: &[Span], max_width: usize) -> Vec<Vec<Span>> {
    if max_width == 0 {
        return vec![Vec::new()];
    }

    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut current_line: Vec<Span> = Vec::new();
    let mut current_width: usize = 0;

    for span in spans {
        let style = span.style();
        let text = span.text();
        let span_width = span.visible_width();

        if span_width == 0 {
            continue;
        }

        // If the span fits on the current line, keep it whole.
        if current_width + span_width <= max_width {
            current_line.push(span.clone());
            current_width += span_width;
            continue;
        }

        // The span does not fit. Flush the current line first.
        if !current_line.is_empty() {
            lines.push(std::mem::take(&mut current_line));
            current_width = 0;
        }

        // Now the line is empty. If the span still overflows the width, hard-wrap it.
        if span_width > max_width {
            let mut remaining = text;
            while !remaining.is_empty() {
                let chunk = clip_plain_to_width(remaining, max_width);
                if chunk.is_empty() {
                    break;
                }
                let consumed = chunk.len();
                current_line.push(Span::styled(chunk, style));
                lines.push(std::mem::take(&mut current_line));
                remaining = &remaining[consumed..];
            }
        } else {
            current_line.push(span.clone());
            current_width = span_width;
        }
    }

    if !current_line.is_empty() || lines.is_empty() {
        lines.push(current_line);
    }

    lines
}

fn highlight_code(code: &str, lang: &str, theme: &TuiTheme) -> Vec<String> {
    // Check the global highlight cache first.
    let key = (quick_hash(code), lang.to_owned());
    if let Ok(cache) = highlight_cache().lock() {
        if let Some(cached) = cache.get(&key) {
            return cached.clone();
        }
    }

    let result = highlight_code_uncached(code, lang, theme);

    // Store in cache (evict if over capacity — simple strategy, not LRU).
    if let Ok(mut cache) = highlight_cache().lock() {
        if cache.len() >= HIGHLIGHT_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, result.clone());
    }
    result
}

fn highlight_code_uncached(code: &str, lang: &str, theme: &TuiTheme) -> Vec<String> {
    let fallback = || {
        code.trim_end_matches('\n')
            .lines()
            .map(|l| crate::primitive::paint(l, Style::default().fg(theme.text_primary)))
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
        let line = line.trim_end_matches(['\r', '\n']);
        match h.highlight_line(line, ss) {
            Ok(ranges) => out.push(syntect_to_ansi(&ranges, theme)),
            Err(_) => out.push(crate::primitive::paint(
                line,
                Style::default().fg(theme.text_primary),
            )),
        }
    }
    out
}

fn syntect_to_ansi(ranges: &[(syntect::highlighting::Style, &str)], theme: &TuiTheme) -> String {
    ranges
        .iter()
        .map(|(st, text)| {
            let style = Style {
                fg: Some(syntect_color(st.foreground).unwrap_or(theme.text_primary)),
                bold: st
                    .font_style
                    .contains(syntect::highlighting::FontStyle::BOLD),
                italic: st
                    .font_style
                    .contains(syntect::highlighting::FontStyle::ITALIC),
                underline: st
                    .font_style
                    .contains(syntect::highlighting::FontStyle::UNDERLINE),
                ..Style::default()
            };
            crate::primitive::paint(text, style)
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
        // Column widths are bounded by the terminal width, so precision loss
        // and truncation here only affect display proportions.
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        {
            let scale = available as f64 / total as f64;
            for w in &mut col_widths {
                *w = ((*w as f64) * scale).round() as usize;
            }
        }
        for w in &mut col_widths {
            if *w == 0 {
                *w = 1;
            }
        }
    }

    let border_style = Style::default().fg(theme.text_muted);
    let header_style = Style::default().fg(theme.text_primary).bold();
    let body_style = Style::default().fg(theme.text_primary);

    let border_line = |joiners: &[char; 3]| -> String {
        let mut s = String::from(joiners[0]);
        for (i, w) in col_widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push(if i + 1 == ncols {
                joiners[2]
            } else {
                joiners[1]
            });
        }
        s
    };

    out.push(Line::styled(border_line(&['┌', '┬', '┐']), border_style));
    out.extend(make_table_row(
        head,
        &col_widths,
        ncols,
        header_style,
        border_style,
    ));
    let separator = Line::styled(border_line(&['├', '┼', '┤']), border_style);
    out.push(separator.clone());
    for (index, row) in rows.iter().enumerate() {
        out.extend(make_table_row(
            row,
            &col_widths,
            ncols,
            body_style,
            border_style,
        ));
        if index + 1 < rows.len() {
            out.push(separator.clone());
        }
    }
    out.push(Line::styled(border_line(&['└', '┴', '┘']), border_style));
}

fn make_table_row(
    cells: &[String],
    widths: &[usize],
    ncols: usize,
    cell_style: Style,
    border_style: Style,
) -> Vec<Line> {
    let wrapped_cells = wrap_table_cells(cells, widths, ncols);
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
    let mut lines = Vec::with_capacity(row_height);

    for line_index in 0..row_height {
        let mut spans = vec![Span::styled("│", border_style)];
        for (i, w) in widths.iter().enumerate().take(ncols) {
            let displayed = wrapped_cells
                .get(i)
                .and_then(|cell| cell.get(line_index))
                .map_or("", String::as_str);
            let vw = visible_width(displayed);
            let pad = w.saturating_sub(vw);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(displayed.to_owned(), cell_style));
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::raw(" "));
            spans.push(Span::styled("│", border_style));
        }
        lines.push(Line::from_spans(spans));
    }

    lines
}

fn wrap_table_cells(cells: &[String], widths: &[usize], ncols: usize) -> Vec<Vec<String>> {
    let mut wrapped = Vec::with_capacity(ncols);
    for (i, w) in widths.iter().enumerate().take(ncols) {
        let content = cells.get(i).map_or("", String::as_str);
        let mut lines = wrap_table_cell(content, (*w).max(1));
        if lines.is_empty() {
            lines.push(String::new());
        }
        wrapped.push(lines);
    }
    wrapped
}

fn wrap_table_cell(content: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for logical_line in content.lines() {
        wrap_table_cell_logical_line(logical_line, width, &mut lines);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_table_cell_logical_line(line: &str, width: usize, out: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in line.split_whitespace() {
        let word_width = visible_width(word);
        if current_width == 0 {
            if word_width <= width {
                current.push_str(word);
                current_width = word_width;
            } else {
                out.extend(wrap_width(word, width));
            }
        } else if current_width + 1 + word_width <= width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            out.push(std::mem::take(&mut current));
            current_width = 0;
            if word_width <= width {
                current.push_str(word);
                current_width = word_width;
            } else {
                out.extend(wrap_width(word, width));
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    } else if line.is_empty() {
        out.push(String::new());
    }
}

/// Pad or hard-truncate an ANSI-styled line to exactly `width` visible columns.
fn fit_ansi_line_to_width(line: &str, width: usize) -> String {
    let vis = visible_width(line);
    if vis > width {
        clip_visible_to_width(line, width)
    } else {
        let mut result = line.to_owned();
        result.push_str(&" ".repeat(width - vis));
        result
    }
}

// ---------------------------------------------------------------------------
// Syntax highlighting helpers for tool cards
// ---------------------------------------------------------------------------

/// Map a file path to a syntect language name.
#[must_use]
pub fn lang_from_path(path: &str) -> Option<&'static str> {
    let ext = Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)?
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "sh" | "bash" | "zsh" => "bash",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "css" => "css",
        "html" | "htm" => "html",
        "sql" => "sql",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        _ => return None,
    })
}

/// Highlight a block of code into per-line spans, using the language inferred
/// from `path`. Falls back to plain text if the language is unknown.
#[must_use]
pub fn highlight_code_lines(content: &str, path: &str, theme: &TuiTheme) -> Vec<Vec<Span>> {
    let ss = syntax_set();
    let ts = theme_set();
    let syntax = lang_from_path(path).and_then(|lang| {
        ss.find_syntax_by_token(lang)
            .or_else(|| ss.find_syntax_by_extension(lang))
    });
    let Some(syntax_theme) = ts.themes.get("base16-ocean.dark") else {
        return plain_code_lines(content, theme);
    };
    let Some(syntax) = syntax else {
        return plain_code_lines(content, theme);
    };

    let mut h = syntect::easy::HighlightLines::new(syntax, syntax_theme);
    content
        .trim_end_matches('\n')
        .lines()
        .map(|line| match h.highlight_line(line, ss) {
            Ok(ranges) => ranges
                .into_iter()
                .map(|(st, text)| Span::styled(text.to_owned(), syntect_style_to_style(&st, theme)))
                .collect(),
            Err(_) => vec![Span::styled(
                line.to_owned(),
                Style::default().fg(theme.text_primary),
            )],
        })
        .collect()
}

fn plain_code_lines(content: &str, theme: &TuiTheme) -> Vec<Vec<Span>> {
    content
        .trim_end_matches('\n')
        .lines()
        .map(|line| {
            vec![Span::styled(
                line.to_owned(),
                Style::default().fg(theme.text_primary),
            )]
        })
        .collect()
}

fn syntect_style_to_style(st: &syntect::highlighting::Style, theme: &TuiTheme) -> Style {
    Style {
        fg: syntect_color(st.foreground).or(Some(theme.text_primary)),
        bold: st
            .font_style
            .contains(syntect::highlighting::FontStyle::BOLD),
        italic: st
            .font_style
            .contains(syntect::highlighting::FontStyle::ITALIC),
        underline: st
            .font_style
            .contains(syntect::highlighting::FontStyle::UNDERLINE),
        ..Style::default()
    }
}

#[cfg(test)]
mod highlight_tests {
    use super::*;

    #[test]
    fn lang_from_path_maps_common_extensions() {
        let cases = [
            ("main.rs", Some("rust")),
            ("lib.ts", Some("typescript")),
            ("app.tsx", Some("typescript")),
            ("index.js", Some("javascript")),
            ("page.jsx", Some("javascript")),
            ("script.py", Some("python")),
            ("main.go", Some("go")),
            ("Foo.java", Some("java")),
            ("deploy.sh", Some("bash")),
            ("config.json", Some("json")),
            ("values.yaml", Some("yaml")),
            ("values.yml", Some("yaml")),
            ("Cargo.toml", Some("toml")),
            ("README.md", Some("markdown")),
            ("style.css", Some("css")),
            ("index.html", Some("html")),
            ("query.sql", Some("sql")),
            ("foo.c", Some("c")),
            ("foo.h", Some("c")),
            ("foo.cpp", Some("cpp")),
            ("foo.hpp", Some("cpp")),
            ("no_extension", None),
            ("file.unknown", None),
        ];
        for (path, expected) in cases {
            assert_eq!(
                lang_from_path(path),
                expected,
                "extension mismatch for {path}"
            );
        }
    }

    #[test]
    fn highlight_code_lines_returns_lines_for_known_lang() {
        let theme = TuiTheme::default();
        let lines = highlight_code_lines("fn main() {}", "main.rs", &theme);
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].is_empty());
    }

    #[test]
    fn highlight_code_lines_falls_back_for_unknown_lang() {
        let theme = TuiTheme::default();
        let lines = highlight_code_lines("hello world", "file.unknown", &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text(), "hello world");
    }
}

#[cfg(test)]
mod code_block_tests {
    use super::*;

    #[test]
    fn code_block_has_rounded_borders() {
        let text = "```bash\necho hi\n```";
        let lines = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ");
        let top = lines[0].to_ansi();
        assert!(top.contains('╭'), "top must contain ╭");
        assert!(top.contains('╮'), "top must contain ╮");
        let bottom = lines.last().unwrap().to_ansi();
        assert!(bottom.contains('╰'), "bottom must contain ╰");
        assert!(bottom.contains('╯'), "bottom must contain ╯");
        let all_plain: String = lines
            .iter()
            .map(|l| crate::primitive::strip_ansi(&l.to_ansi()))
            .collect();
        assert!(all_plain.contains('│'), "output must contain side borders");
    }

    #[test]
    fn code_block_width_is_consistent_and_within_bounds() {
        let text = "```rust\nfn main() {\n    println!();\n}\n```";
        for width in [20, 40, 60, 80] {
            let lines = render_markdown(text, width, &TuiTheme::default(), "● ", "  ");
            let first_width = lines[0].visible_width();
            for line in &lines {
                assert_eq!(
                    line.visible_width(),
                    first_width,
                    "all lines must have the same width"
                );
                assert!(
                    line.visible_width() <= width,
                    "line width {} should be <= {width}",
                    line.visible_width()
                );
            }
        }
    }

    #[test]
    fn json_code_block_keeps_right_border_aligned_after_highlighting() {
        let text = r#"```json
{
  "kind": "swarm",
  "swarm_id": "swarm_xxx",
  "status": "completed",
  "aggregate": { "total": 2, "completed": 2, ... },
  "items": [
    {"index": 0, "agent_id": "agent_xxx", "status": "completed"},
    {"index": 1, "agent_id": "agent_yyy", "status": "completed"}
  ]
}
```"#;
        let lines = render_markdown(text, 100, &TuiTheme::default(), "", "");
        let plain_lines = lines
            .iter()
            .map(|line| crate::primitive::strip_ansi(&line.to_ansi()))
            .collect::<Vec<_>>();
        let expected_width = crate::primitive::visible_width(&plain_lines[0]);

        for (index, raw_line) in lines.iter().map(Line::to_ansi).enumerate() {
            assert!(
                !raw_line.contains(['\n', '\r']),
                "rendered code block row {index} must not contain embedded line breaks: {raw_line:?}"
            );
        }

        for line in &plain_lines {
            assert_eq!(
                crate::primitive::visible_width(line),
                expected_width,
                "code block line should stay inside the same border columns: {line:?}"
            );
        }
    }

    #[test]
    fn code_block_adapts_to_short_content() {
        let text = "```bash\necho hi\n```";
        let width = 40;
        let lines = render_markdown(text, width, &TuiTheme::default(), "● ", "  ");
        // Short content should not expand to the full 40 columns.
        assert!(
            lines[0].visible_width() < width,
            "box should be narrower than full width for short content: {:?}",
            lines[0].to_ansi()
        );
    }

    #[test]
    fn code_block_language_in_header() {
        let text = "```bash\necho hi\n```";
        let lines = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ");
        let top = lines[0].to_ansi();
        assert!(top.contains("bash"), "header must contain language: {top}");
        let all = lines
            .iter()
            .map(|l| crate::primitive::strip_ansi(&l.to_ansi()))
            .collect::<String>();
        assert!(!all.contains("```bash"), "must not use old fence style");
    }

    #[test]
    fn code_block_no_fence_backticks() {
        let text = "```bash\necho hi\n```";
        let all = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ")
            .into_iter()
            .map(|l| crate::primitive::strip_ansi(&l.to_ansi()))
            .collect::<String>();
        assert!(
            !all.contains("```"),
            "output must not contain fence backticks"
        );
    }

    #[test]
    fn code_block_empty_content_renders_box() {
        let text = "```bash\n```";
        let lines = render_markdown(text, 30, &TuiTheme::default(), "● ", "  ");
        let top = lines[0].to_ansi();
        let bottom = lines.last().unwrap().to_ansi();
        assert!(top.contains('╭') && top.contains('╮'));
        assert!(bottom.contains('╰') && bottom.contains('╯'));
    }

    #[test]
    fn code_block_honors_min_width() {
        // Width is too small for a real box; just ensure no panic.
        let text = "```bash\necho hi\n```";
        let _lines = render_markdown(text, 4, &TuiTheme::default(), "● ", "  ");
    }

    #[test]
    fn code_block_in_list_renders_within_width() {
        let text = "- item\n\n  ```bash\n  echo hi\n  ```\n";
        let width = 40;
        let lines = render_markdown(text, width, &TuiTheme::default(), "● ", "  ");
        for line in &lines {
            assert!(
                line.visible_width() <= width,
                "line width {} should be <= {width}: {:?}",
                line.visible_width(),
                line.to_ansi()
            );
        }
    }
}
