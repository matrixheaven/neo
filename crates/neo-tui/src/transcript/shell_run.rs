use neo_agent_core::ShellCommandOutcome;
use neo_agent_core::tools::format_shell_failure;

use crate::primitive::theme::TuiTheme;
use crate::primitive::wrap_width;
use crate::primitive::{Finalization, Line, Span, Style};
use crate::utils::shell_output::{sanitize_shell_output, split_sanitized_shell_lines};

const MAX_LIVE_OUTPUT_LINES: usize = 12;
const MAX_LIVE_OUTPUT_CHARS: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRunState {
    Running,
    Finished {
        stdout: String,
        stderr: String,
        exit_code: Option<i32>,
        /// Unix signal number (`None` on Windows or for normal exits).
        signal: Option<i32>,
        outcome: ShellCommandOutcome,
        truncated: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRunComponent {
    id: String,
    command: String,
    state: ShellRunState,
    live_output: Vec<String>,
    dropped_live_output_lines: usize,
    live_output_chars: usize,
}

impl ShellRunComponent {
    #[must_use]
    pub fn running(id: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            command: command.into(),
            state: ShellRunState::Running,
            live_output: Vec::new(),
            dropped_live_output_lines: 0,
            live_output_chars: 0,
        }
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn finished(
        id: impl Into<String>,
        command: impl Into<String>,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
        exit_code: Option<i32>,
        signal: Option<i32>,
        outcome: ShellCommandOutcome,
        truncated: bool,
    ) -> Self {
        Self {
            id: id.into(),
            command: command.into(),
            state: ShellRunState::Finished {
                stdout: stdout.into(),
                stderr: stderr.into(),
                exit_code,
                signal,
                outcome,
                truncated,
            },
            live_output: Vec::new(),
            dropped_live_output_lines: 0,
            live_output_chars: 0,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    #[must_use]
    pub const fn finalization(&self) -> Finalization {
        match self.state {
            ShellRunState::Running => Finalization::Live,
            ShellRunState::Finished { .. } => Finalization::Finalized,
        }
    }

    pub fn append_live_output(&mut self, output: impl AsRef<str>) -> bool {
        let sanitized = sanitize_shell_output(output.as_ref());
        if sanitized.is_empty() {
            return false;
        }
        for line in sanitized.lines() {
            self.live_output_chars += line.chars().count();
            self.live_output.push(line.to_owned());
        }
        self.trim_live_output();
        true
    }

    pub fn finish(
        &mut self,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
        exit_code: Option<i32>,
        signal: Option<i32>,
        outcome: ShellCommandOutcome,
        truncated: bool,
    ) -> bool {
        let stdout = stdout.into();
        let stderr = stderr.into();
        let next_state = ShellRunState::Finished {
            stdout,
            stderr,
            exit_code,
            signal,
            outcome,
            truncated,
        };
        if self.state == next_state
            && self.live_output.is_empty()
            && self.dropped_live_output_lines == 0
            && self.live_output_chars == 0
        {
            return false;
        }
        self.state = next_state;
        self.live_output.clear();
        self.dropped_live_output_lines = 0;
        self.live_output_chars = 0;
        true
    }

    pub fn interrupt(&mut self) -> bool {
        if self.finalization() == Finalization::Finalized {
            return false;
        }
        let mut stdout = self.live_output.join("\n");
        if self.dropped_live_output_lines > 0 {
            stdout = format!(
                "... ({} earlier lines)\n{stdout}",
                self.dropped_live_output_lines
            );
        }
        let truncated = self.dropped_live_output_lines > 0;
        self.finish(
            stdout,
            "Interrupted when terminal exited",
            None,
            None,
            ShellCommandOutcome::Cancelled,
            truncated,
        )
    }

    #[must_use]
    pub fn render(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let mut rows = Vec::new();
        let command_style = Style::default().fg(theme.shell_mode).bold();
        let body_style = Style::default().fg(theme.text_primary);
        let muted_style = Style::default().fg(theme.text_muted);
        let error_style = Style::default().fg(theme.status_error);

        rows.push(Line::from_spans(vec![
            Span::styled("$ ", command_style),
            Span::styled(self.command.clone(), command_style),
        ]));

        match &self.state {
            ShellRunState::Running => {
                if self.dropped_live_output_lines > 0 {
                    rows.push(Line::styled(
                        format!("  ... ({} earlier lines)", self.dropped_live_output_lines),
                        muted_style,
                    ));
                }
                rows.extend(wrap_output_lines(&self.live_output, width, muted_style));
                rows.push(Line::styled("  ctrl+b to background", muted_style));
            }
            ShellRunState::Finished {
                stdout,
                stderr,
                exit_code,
                signal,
                outcome,
                truncated,
            } => {
                rows.extend(render_finished_output(
                    stdout,
                    stderr,
                    *exit_code,
                    *signal,
                    outcome,
                    *truncated,
                    width,
                    body_style,
                    error_style,
                    muted_style,
                ));
            }
        }

        rows
    }

    #[must_use]
    pub fn copy_text(&self) -> String {
        let mut text = format!("$ {}", self.command);
        match &self.state {
            ShellRunState::Running => {
                for line in &self.live_output {
                    text.push('\n');
                    text.push_str(line);
                }
            }
            ShellRunState::Finished {
                stdout,
                stderr,
                exit_code,
                signal,
                outcome,
                truncated,
            } => {
                for line in
                    finished_plain_lines(stdout, stderr, *exit_code, *signal, outcome, *truncated)
                {
                    text.push('\n');
                    text.push_str(&line);
                }
            }
        }
        text
    }

    fn trim_live_output(&mut self) {
        while self.live_output.len() > MAX_LIVE_OUTPUT_LINES
            || self.live_output_chars > MAX_LIVE_OUTPUT_CHARS
        {
            let Some(line) = self.live_output.first() else {
                self.live_output_chars = 0;
                break;
            };
            self.live_output_chars = self.live_output_chars.saturating_sub(line.chars().count());
            self.live_output.remove(0);
            self.dropped_live_output_lines += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_finished_output(
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    signal: Option<i32>,
    outcome: &ShellCommandOutcome,
    truncated: bool,
    width: usize,
    body_style: Style,
    error_style: Style,
    muted_style: Style,
) -> Vec<Line> {
    let style = if matches!(outcome, ShellCommandOutcome::ResourceLimited) {
        muted_style
    } else if exit_code == Some(0) && matches!(outcome, ShellCommandOutcome::Completed) {
        body_style
    } else {
        error_style
    };
    let lines = finished_plain_lines(stdout, stderr, exit_code, signal, outcome, truncated);
    if lines.is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for line in lines {
        let line_style = if line.starts_with("Moved to background")
            || line.starts_with("Cancelled")
            || line.starts_with("Timed out")
            || line.starts_with("Resource limit")
            || line == "[output truncated]"
        {
            muted_style
        } else {
            style
        };
        rows.extend(wrap_prefixed(&line, width, line_style));
    }
    rows
}

pub(crate) fn finished_plain_lines(
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    signal: Option<i32>,
    outcome: &ShellCommandOutcome,
    truncated: bool,
) -> Vec<String> {
    let mut lines = split_sanitized_shell_lines(stdout, stderr);
    match outcome {
        ShellCommandOutcome::Backgrounded { .. } => {
            lines.push("Moved to background. Use /tasks to view.".to_owned());
        }
        ShellCommandOutcome::Cancelled => {
            lines.push("Cancelled.".to_owned());
        }
        ShellCommandOutcome::TimedOut => {
            lines.push("Timed out.".to_owned());
        }
        ShellCommandOutcome::ResourceLimited => {
            lines.push("Resource limit exceeded.".to_owned());
        }
        ShellCommandOutcome::Completed => {
            if exit_code != Some(0) {
                lines.push(format_shell_failure(exit_code, signal));
            }
        }
    }
    if truncated {
        lines.push("[output truncated]".to_owned());
    }
    lines
}

fn wrap_output_lines(lines: &[String], width: usize, style: Style) -> Vec<Line> {
    lines
        .iter()
        .flat_map(|line| wrap_prefixed(line, width, style))
        .collect()
}

fn wrap_prefixed(line: &str, width: usize, style: Style) -> Vec<Line> {
    const PREFIX: &str = "  ";
    let body_width = width.saturating_sub(PREFIX.len()).max(1);
    wrap_width(line, body_width)
        .into_iter()
        .map(|segment| Line::styled(format!("{PREFIX}{segment}"), style))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_limited_shell_outcome_finalizes_with_terminal_message() {
        assert_eq!(
            finished_plain_lines(
                "",
                "",
                None,
                None,
                &ShellCommandOutcome::ResourceLimited,
                false,
            ),
            ["Resource limit exceeded."]
        );
        let component = ShellRunComponent::finished(
            "shell-1",
            "cargo nextest --workspace",
            "",
            "",
            None,
            None,
            ShellCommandOutcome::ResourceLimited,
            false,
        );
        assert_eq!(component.finalization(), Finalization::Finalized);

        let theme = TuiTheme::default();
        let rows = ShellRunComponent::finished(
            "shell-2",
            "cargo nextest --workspace",
            "partial output",
            "",
            None,
            None,
            ShellCommandOutcome::ResourceLimited,
            false,
        )
        .render(80, &theme);
        assert!(rows[1..].iter().all(|line| {
            line.spans()
                .iter()
                .all(|span| span.style().fg == Some(theme.text_muted))
        }));
    }
}
