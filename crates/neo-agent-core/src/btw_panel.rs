use std::fmt::Write as _;

use crate::ansi::{RESET, Style, paint, visible_width};
use crate::chrome::TuiTheme;
use crate::components::wrap_width;
use crate::markdown::render_markdown;
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

/// TUI state backing the `/btw` panel. Kept inside [`crate::chrome::NeoChromeState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwPanelState {
    pub sidecar: BtwSidecar,
    pub scroll_offset: usize,
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
            status_message: None,
        }
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(rows);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(rows);
    }
}

pub struct BtwPanel<'a> {
    state: &'a BtwPanelState,
    theme: TuiTheme,
}

impl<'a> BtwPanel<'a> {
    #[must_use]
    pub fn new(state: &'a BtwPanelState) -> Self {
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

    /// Render the sidecar panel at a fixed height of `max_height` rows.
    ///
    /// The panel is always `max_height` lines tall while active so that the
    /// chrome layout stays stable as sidecar content grows; content scrolls
    /// once it exceeds the visible inner area.
    #[must_use]
    pub fn render(&self, width: usize, max_height: usize) -> Vec<String> {
        if width < 2 || max_height < 2 {
            return Vec::new();
        }

        let border_style = Style::default().fg(self.theme.surface_border);
        let inner_width = width.saturating_sub(2);
        let content_lines = self.build_content_lines(inner_width);
        let visible_content_height = max_height.saturating_sub(2);
        let overflows = content_lines.len() > visible_content_height;

        let title = self.title(overflows);
        let top = top_border_with_title(width, &title, border_style);
        let bottom = bottom_border(width, border_style);

        let mut lines = Vec::with_capacity(max_height);
        lines.push(top);

        let max_offset = content_lines.len().saturating_sub(visible_content_height);
        let offset = self.state.scroll_offset.min(max_offset);
        for line in content_lines
            .iter()
            .skip(offset)
            .take(visible_content_height)
        {
            lines.push(content_line(line, width, border_style));
        }
        while lines.len().saturating_add(1) < max_height {
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
                lines.extend(wrap_ansi(
                    &paint(status, Style::default().fg(self.theme.status_warn)),
                    inner_width,
                ));
            } else {
                lines.extend(wrap_ansi(
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
            lines.extend(wrap_ansi(
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
        lines.extend(wrap_ansi(&format!("{q_label}{prompt}"), inner_width));

        // Optional thinking preview.
        if !turn.thinking.is_empty() {
            let thinking = paint(&turn.thinking, Style::default().fg(self.theme.text_muted));
            lines.extend(wrap_ansi(&thinking, inner_width));
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
                    lines.extend(wrap_ansi(&error, inner_width));
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
        paint(&repeat_char(ROUNDED.horizontal, fill), border_style),
        paint(&ROUNDED.top_right.to_string(), border_style),
    )
}

fn wrap_ansi(text: &str, max_width: usize) -> Vec<String> {
    wrap_width(text, max_width)
}

fn repeat_char(ch: char, n: usize) -> String {
    std::iter::repeat_n(ch, n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::strip_ansi;

    fn plain(lines: &[String]) -> Vec<String> {
        lines.iter().map(|l| strip_ansi(l).to_owned()).collect()
    }

    fn assert_width(lines: &[String], expected: usize) {
        for line in lines {
            assert_eq!(
                visible_width(line),
                expected,
                "line width mismatch: {line:?}"
            );
        }
    }

    #[test]
    fn btw_panel_renders_empty_state_with_esc_hint() {
        let state = BtwPanelState::new(BtwSidecar::new("btw-1"));
        let lines = BtwPanel::new(&state).render(40, 5);

        assert_eq!(lines.len(), 5);
        assert_width(&lines, 40);
        let plain = plain(&lines);
        assert!(plain[0].contains('╭'));
        assert!(plain[0].contains("BTW"));
        assert!(plain[0].contains("Esc close"));
        assert!(!plain[0].contains("scroll"));
        assert!(
            plain
                .iter()
                .any(|l| l.contains("Ready for a side question..."))
        );
        assert!(plain[plain.len() - 1].contains('╰'));
    }

    #[test]
    fn btw_panel_renders_running_turn_with_thinking() {
        let sidecar = BtwSidecar::new("btw-1").with_turn(
            BtwTurn::new("Explain lifetimes")
                .with_thinking("Let me think...")
                .with_phase(BtwPhase::Running),
        );
        let state = BtwPanelState::new(sidecar);
        let lines = BtwPanel::new(&state).render(40, 7);

        let plain = plain(&lines);
        assert!(plain.iter().any(|l| l.contains("Q: Explain lifetimes")));
        assert!(plain.iter().any(|l| l.contains("Let me think...")));
        assert!(plain.iter().any(|l| l.contains("Waiting for answer...")));
    }

    #[test]
    fn btw_panel_renders_answered_turn() {
        let sidecar = BtwSidecar::new("btw-1").with_turn(
            BtwTurn::new("What is 2+2?")
                .with_answer("4")
                .with_phase(BtwPhase::Done),
        );
        let state = BtwPanelState::new(sidecar);
        let lines = BtwPanel::new(&state).render(40, 6);

        let plain = plain(&lines);
        assert!(plain.iter().any(|l| l.contains("Q: What is 2+2?")));
        assert!(plain.iter().any(|l| l.contains("4")));
        assert!(!plain.iter().any(|l| l.contains("Waiting for answer...")));
    }

    #[test]
    fn btw_panel_renders_busy_status_message() {
        let sidecar = BtwSidecar::new("btw-1").with_turn(
            BtwTurn::new("explain the trust flow")
                .with_thinking("Thinking through startup config and project context loading...")
                .with_phase(BtwPhase::Running),
        );
        let mut state = BtwPanelState::new(sidecar);
        state.status_message =
            Some("Wait for /btw to finish before sending another question.".to_owned());
        let lines = BtwPanel::new(&state).render(80, 7);

        let plain = plain(&lines);
        assert!(
            plain
                .iter()
                .any(|l| l.contains("Q: explain the trust flow"))
        );
        assert!(plain.iter().any(|l| {
            l.contains("Thinking through startup config and project context loading...")
        }));
        assert!(plain.iter().any(|l| l.contains("Wait for /btw to finish")));

        // The busy notice must appear after the turn, separated by a blank content line.
        let q_idx = plain
            .iter()
            .position(|l| l.contains("Q: explain the trust flow"))
            .expect("question line");
        let status_idx = plain
            .iter()
            .position(|l| l.contains("Wait for /btw to finish"))
            .expect("status line");
        assert!(
            status_idx > q_idx + 1,
            "status should be separated from the question by at least one line"
        );
        let separator_inner = plain[status_idx - 1]
            .trim_start_matches('│')
            .trim_end_matches('│')
            .trim();
        assert!(separator_inner.is_empty(), "blank separator missing");
    }

    #[test]
    fn btw_panel_renders_tool_denied_error() {
        let sidecar = BtwSidecar::new("btw-1").with_turn(
            BtwTurn::new("Run a tool")
                .with_error("Tool calls are disabled for side questions. Answer with text only.")
                .with_phase(BtwPhase::Failed),
        );
        let state = BtwPanelState::new(sidecar);
        let lines = BtwPanel::new(&state).render(50, 5);

        let plain = plain(&lines);
        assert!(
            plain
                .iter()
                .any(|l| l.contains("Tool calls are disabled for side questions"))
        );
    }

    #[test]
    fn btw_panel_truncates_long_lines_without_overlapping_border() {
        let sidecar = BtwSidecar::new("btw-1")
            .with_turn(BtwTurn::new("a".repeat(200)).with_phase(BtwPhase::Running));
        let state = BtwPanelState::new(sidecar);
        let lines = BtwPanel::new(&state).render(20, 8);

        assert_width(&lines, 20);
        let plain = plain(&lines);
        assert!(plain.iter().any(|l| l.starts_with('│')));
        assert!(plain.iter().any(|l| l.ends_with('│')));
    }

    #[test]
    fn btw_panel_caps_height_to_one_third_terminal_rows() {
        let sidecar = BtwSidecar::new("btw-1").with_turn(
            BtwTurn::new("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10")
                .with_phase(BtwPhase::Running),
        );
        let state = BtwPanelState::new(sidecar);
        let max_height = 6; // simulates max(3, 18 / 3)
        let lines = BtwPanel::new(&state).render(40, max_height);

        assert_eq!(lines.len(), max_height);
        let plain = plain(&lines);
        assert!(plain[0].contains("↑↓ scroll"));
    }

    #[test]
    fn btw_panel_scrolls_content_with_offset() {
        let sidecar = BtwSidecar::new("btw-1").with_turn(
            BtwTurn::new("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8")
                .with_phase(BtwPhase::Running),
        );
        let mut state = BtwPanelState::new(sidecar);
        state.scroll_offset = 2;
        let lines = BtwPanel::new(&state).render(40, 6);

        let plain = plain(&lines);
        assert!(!plain.iter().any(|l| l.contains("line1")));
        assert!(plain.iter().any(|l| l.contains("line3")));
    }

    #[test]
    fn btw_panel_renders_narrow_width() {
        let sidecar = BtwSidecar::new("btw-1").with_turn(BtwTurn::new("Hi"));
        let state = BtwPanelState::new(sidecar);
        let lines = BtwPanel::new(&state).render(8, 5);

        assert_width(&lines, 8);
        let plain = plain(&lines);
        assert!(plain[0].starts_with('╭'));
        assert!(plain[0].ends_with('╮'));
        assert!(plain[plain.len() - 1].starts_with('╰'));
        assert!(plain[plain.len() - 1].ends_with('╯'));
        // Content rows are clipped inside the border, never spilling past the right edge.
        for line in plain.iter().take(plain.len() - 1).skip(1) {
            assert_eq!(line.chars().filter(|c| *c == '│').count(), 2);
        }
    }

    #[test]
    fn btw_panel_renders_answer_markdown_snapshot() {
        let sidecar = BtwSidecar::new("btw-1").with_turn(
            BtwTurn::new("What to do?")
                .with_answer("- first\n- second")
                .with_phase(BtwPhase::Done),
        );
        let state = BtwPanelState::new(sidecar);
        let width = 30;
        let lines = BtwPanel::new(&state).render(width, 6);

        assert_width(&lines, width);
        let plain = plain(&lines).join("\n");
        let dashes = |n: usize| "─".repeat(n);
        let spaces = |n: usize| " ".repeat(n);
        let expected = format!(
            "╭ BTW ─ Esc close {top_dashes}╮\n\
             │Q: What to do?{q_pad}│\n\
             │• first{first_pad}│\n\
             │• second{second_pad}│\n\
             │{empty_pad}│\n\
             ╰{bottom_dashes}╯",
            top_dashes = dashes(11),
            q_pad = spaces(14),
            first_pad = spaces(21),
            second_pad = spaces(20),
            empty_pad = spaces(28),
            bottom_dashes = dashes(28),
        );
        assert_eq!(plain, expected);
    }
}
