use std::fmt::Write as _;

use crate::markdown::render_markdown;
use crate::primitive::theme::TuiTheme;
use crate::primitive::wrap_width;
use crate::primitive::{RESET, Style, paint, visible_width};
use crate::widgets::box_draw::{ROUNDED, bottom_border, content_line};

/// Stable identifier for a `/btw` sidecar session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BtwSidecarId(pub String);

/// Lifecycle phase of a sidecar turn or the sidecar as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtwPhase {
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

/// A single question/answer exchange inside a `/btw` sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwTurn {
    pub prompt: String,
    pub answer: String,
    pub thinking: String,
    pub error: Option<String>,
    pub phase: BtwPhase,
}

impl BtwTurn {
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            answer: String::new(),
            thinking: String::new(),
            error: None,
            phase: BtwPhase::Idle,
        }
    }

    #[must_use]
    pub fn with_phase(mut self, phase: BtwPhase) -> Self {
        self.phase = phase;
        self
    }

    #[must_use]
    pub fn with_answer(mut self, answer: impl Into<String>) -> Self {
        self.answer = answer.into();
        self
    }

    #[must_use]
    pub fn with_thinking(mut self, thinking: impl Into<String>) -> Self {
        self.thinking = thinking.into();
        self
    }

    #[must_use]
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }
}

/// Runtime state for an active `/btw` sidecar session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwSidecar {
    pub id: BtwSidecarId,
    pub parent_session_id: Option<String>,
    pub turns: Vec<BtwTurn>,
    pub phase: BtwPhase,
}

impl BtwSidecar {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: BtwSidecarId(id.into()),
            parent_session_id: None,
            turns: Vec::new(),
            phase: BtwPhase::Idle,
        }
    }

    #[must_use]
    pub fn with_parent_session_id(mut self, parent: impl Into<String>) -> Self {
        self.parent_session_id = Some(parent.into());
        self
    }

    #[must_use]
    pub fn with_turn(mut self, turn: BtwTurn) -> Self {
        self.turns.push(turn);
        self
    }
}

const MIN_PANEL_LINES: usize = 3;
const THINKING_PREVIEW_LINES: usize = 2;

fn max_body_lines(terminal_rows: u16) -> usize {
    let rows = usize::from(terminal_rows);
    let max_panel_lines = MIN_PANEL_LINES.max(rows / 3);
    max_panel_lines.saturating_sub(1).max(1)
}

/// TUI state backing the `/btw` panel. Kept inside [`crate::shell::NeoChromeState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwPanelState {
    pub sidecar: BtwSidecar,
    pub scroll_offset: usize,
    /// Maximum scroll offset given the current content and panel size.
    pub max_scroll_offset: usize,
    /// Smallest body height the panel has ever taken; prevents the panel from
    /// shrinking as content changes and causing layout jumps.
    pub min_body_lines: usize,
    /// Whether new content should keep the view pinned to the bottom.
    pub follow_tail: bool,
    /// Optional panel-wide notice shown below the turn list (e.g. busy or
    /// tool-denied messages).
    pub status_message: Option<String>,
}

impl BtwPanelState {
    #[must_use]
    pub fn new(sidecar: BtwSidecar) -> Self {
        Self {
            sidecar,
            scroll_offset: 0,
            max_scroll_offset: 0,
            min_body_lines: 0,
            follow_tail: true,
            status_message: None,
        }
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(rows);
        self.follow_tail = false;
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.scroll_offset = (self.scroll_offset + rows).min(self.max_scroll_offset);
        self.follow_tail = self.scroll_offset == self.max_scroll_offset;
    }
}

pub struct BtwPanel<'a> {
    state: &'a mut BtwPanelState,
    theme: TuiTheme,
}

impl<'a> BtwPanel<'a> {
    #[must_use]
    pub fn new(state: &'a mut BtwPanelState) -> Self {
        Self {
            state,
            theme: TuiTheme::default(),
        }
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Render the sidecar panel.
    ///
    /// The panel body grows with its content, from a single line up to roughly
    /// one third of `terminal_rows`, then scrolls. The panel height never
    /// shrinks below the largest height it has already reached for the current
    /// content (unless the terminal is resized smaller) so that layout stays
    /// stable while new content streams in.
    #[must_use]
    pub fn render(&mut self, width: usize, terminal_rows: u16) -> Vec<String> {
        if width < 2 || terminal_rows < 2 {
            return Vec::new();
        }

        let border_style = Style::default().fg(self.theme.surface_border);
        let inner_width = width.saturating_sub(2);
        let content_lines = self.build_content_lines(inner_width);
        let cap = max_body_lines(terminal_rows);
        let previous_min = self.state.min_body_lines;
        let target_body_lines = cap.min(content_lines.len().max(previous_min));
        self.state.min_body_lines = target_body_lines;

        let overflows = content_lines.len() > target_body_lines;

        let title = self.title(overflows);
        let top = top_border_with_title(width, &title, border_style);
        let bottom = bottom_border(width, border_style);

        let mut lines = Vec::with_capacity(target_body_lines + 2);
        lines.push(top);

        let visible = if overflows {
            self.state.max_scroll_offset = content_lines.len() - target_body_lines;
            if self.state.follow_tail {
                self.state.scroll_offset = self.state.max_scroll_offset;
            } else {
                self.state.scroll_offset =
                    self.state.scroll_offset.min(self.state.max_scroll_offset);
            }
            content_lines
                .iter()
                .skip(self.state.scroll_offset)
                .take(target_body_lines)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            self.state.scroll_offset = 0;
            self.state.max_scroll_offset = 0;
            self.state.follow_tail = true;
            content_lines
        };

        for line in &visible {
            lines.push(content_line(line, width, border_style));
        }
        while lines.len().saturating_add(1) < target_body_lines + 2 {
            lines.push(content_line("", width, border_style));
        }
        lines.push(bottom);
        lines
    }

    fn title(&self, overflows: bool) -> String {
        let brand = Style::default().fg(self.theme.brand);
        let muted = Style::default().fg(self.theme.text_muted);
        let mut title = format!(" {} ─ Esc close", paint("BTW", brand));
        if overflows {
            let _ = write!(title, " {}", paint("· ↑↓ scroll", muted));
        }
        title.push(' ');
        title
    }

    fn build_content_lines(&self, inner_width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        if self.state.sidecar.turns.is_empty() {
            if let Some(status) = &self.state.status_message {
                lines.extend(wrap_width(
                    &paint(status, Style::default().fg(self.theme.status_warn)),
                    inner_width,
                ));
            } else {
                lines.extend(wrap_width(
                    &paint(
                        "Ready for a side question...",
                        Style::default().fg(self.theme.text_muted),
                    ),
                    inner_width,
                ));
            }
            return lines;
        }
        for turn in &self.state.sidecar.turns {
            lines.extend(self.render_turn(turn, inner_width));
        }
        if let Some(status) = &self.state.status_message {
            lines.push(String::new());
            lines.extend(wrap_width(
                &paint(status, Style::default().fg(self.theme.status_warn)),
                inner_width,
            ));
        }
        lines
    }

    fn render_turn(&self, turn: &BtwTurn, inner_width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Question line: "Q: <prompt>".
        let q_label = paint("Q: ", Style::default().fg(self.theme.brand).bold());
        let prompt = paint(&turn.prompt, Style::default().fg(self.theme.text_primary));
        lines.extend(wrap_width(&format!("{q_label}{prompt}"), inner_width));

        // Optional thinking preview. While the answer is still streaming only
        // the last few reasoning lines are shown so the panel stays compact.
        if !turn.thinking.is_empty() {
            let thinking = paint(&turn.thinking, Style::default().fg(self.theme.text_muted));
            let mut thinking_lines = wrap_width(&thinking, inner_width);
            if turn.phase == BtwPhase::Running && thinking_lines.len() > THINKING_PREVIEW_LINES {
                thinking_lines =
                    thinking_lines.split_off(thinking_lines.len() - THINKING_PREVIEW_LINES);
            }
            lines.extend(thinking_lines);
        }

        match turn.phase {
            BtwPhase::Running => {
                lines.push(paint(
                    "Waiting for answer...",
                    Style::default().fg(self.theme.text_muted),
                ));
            }
            BtwPhase::Done if !turn.answer.is_empty() => {
                let md_lines = render_markdown(
                    &turn.answer,
                    inner_width,
                    &self.theme,
                    "", // first_prefix
                    "", // cont_prefix
                );
                lines.extend(md_lines.into_iter().map(|line| line.to_ansi()));
            }
            BtwPhase::Failed => {
                if let Some(error) = &turn.error {
                    let error = paint(error, Style::default().fg(self.theme.status_error));
                    lines.extend(wrap_width(&error, inner_width));
                } else {
                    lines.push(paint(
                        "Failed.",
                        Style::default().fg(self.theme.status_error),
                    ));
                }
            }
            BtwPhase::Cancelled => {
                lines.push(paint(
                    "Cancelled.",
                    Style::default().fg(self.theme.status_cancelled),
                ));
            }
            BtwPhase::Idle | BtwPhase::Done => {}
        }

        lines
    }
}

#[must_use]
fn top_border_with_title(width: usize, title: &str, border_style: Style) -> String {
    if width < 2 {
        return String::new();
    }
    let inner = width - 2;
    let title_width = visible_width(title);
    if title_width >= inner {
        // Title is too wide: fall back to a plain top border.
        return crate::widgets::box_draw::top_border(width, border_style);
    }
    let fill = inner - title_width;
    format!(
        "{}{}{}{}{}",
        paint(&ROUNDED.top_left.to_string(), border_style),
        title,
        RESET,
        paint(
            &std::iter::repeat_n(ROUNDED.horizontal, fill).collect::<String>(),
            border_style,
        ),
        paint(&ROUNDED.top_right.to_string(), border_style),
    )
}

#[cfg(test)]
#[path = "test_cases/panel.rs"]
mod panel;
