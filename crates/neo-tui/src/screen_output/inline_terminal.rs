use std::io::Write;
use std::time::Instant;

use crate::terminal_capabilities::TerminalCapabilities;
use crate::transcript::FinalizedBlock;

use super::LiveRenderer;
use super::terminal_modes::{
    TerminalModeGuard, write_enter_review_output, write_leave_review_output,
};
use super::types::CursorPos;

const SYNCHRONIZED_OUTPUT_START: &str = "\x1b[?2026h";
const SYNCHRONIZED_OUTPUT_END: &[u8] = b"\x1b[?2026l";

#[derive(Debug, Clone)]
pub struct TerminalFrame {
    pub history: Vec<FinalizedBlock>,
    pub live: Vec<String>,
    pub cursor: Option<CursorPos>,
    pub review_surface: bool,
    pub next_animation_deadline: Option<Instant>,
}

impl TerminalFrame {
    #[must_use]
    pub const fn new(
        history: Vec<FinalizedBlock>,
        live: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> Self {
        Self {
            history,
            live,
            cursor,
            review_surface: false,
            next_animation_deadline: None,
        }
    }

    #[must_use]
    pub const fn with_surface(
        history: Vec<FinalizedBlock>,
        live: Vec<String>,
        cursor: Option<CursorPos>,
        review_surface: bool,
        next_animation_deadline: Option<Instant>,
    ) -> Self {
        Self {
            history: if review_surface { Vec::new() } else { history },
            live,
            cursor,
            review_surface,
            next_animation_deadline,
        }
    }

    #[must_use]
    pub const fn with_animation_deadline(
        history: Vec<FinalizedBlock>,
        live: Vec<String>,
        cursor: Option<CursorPos>,
        next_animation_deadline: Option<Instant>,
    ) -> Self {
        Self {
            history,
            live,
            cursor,
            review_surface: false,
            next_animation_deadline,
        }
    }
}

#[derive(Debug)]
pub struct InlineTerminal {
    synchronized_output: bool,
    live: LiveRenderer,
    saved_normal_live: Option<LiveRenderer>,
    modes: Option<TerminalModeGuard>,
    review_surface: bool,
}

impl InlineTerminal {
    #[must_use]
    pub fn new(width: u16, height: u16, capabilities: TerminalCapabilities) -> Self {
        Self {
            synchronized_output: capabilities.ansi.synchronized_output,
            live: LiveRenderer::new(width, height),
            saved_normal_live: None,
            modes: None,
            review_surface: false,
        }
    }

    pub fn enter(
        width: u16,
        height: u16,
        capabilities: TerminalCapabilities,
    ) -> std::io::Result<Self> {
        let modes = TerminalModeGuard::enter(capabilities)?;
        let mut terminal = Self::new(width, height, capabilities);
        terminal.modes = Some(modes);
        Ok(terminal)
    }

    #[must_use]
    pub fn for_test(width: u16, height: u16) -> Self {
        Self::new(width, height, TerminalCapabilities::default())
    }

    pub fn render_to(
        &mut self,
        output: &mut dyn Write,
        frame: &TerminalFrame,
    ) -> std::io::Result<()> {
        let entering_review = frame.review_surface && !self.review_surface;
        let leaving_review = !frame.review_surface && self.review_surface;
        let mut transaction = Vec::new();
        let saved_normal_live = entering_review.then(|| self.live.clone());

        if entering_review {
            append_review_transition(&mut transaction, &mut self.modes, true)?;
        } else if leaving_review {
            append_review_transition(&mut transaction, &mut self.modes, false)?;
        }

        let history_lines = if frame.review_surface {
            Vec::new()
        } else {
            flatten_history(&frame.history)
        };
        let mut next_live = if leaving_review {
            self.saved_normal_live
                .clone()
                .unwrap_or_else(|| self.live.clone())
        } else {
            self.live.clone()
        };
        if entering_review {
            next_live.reset();
        } else if leaving_review {
            transaction.extend_from_slice(next_live.clear_for_history_redraw().as_bytes());
        }
        if !history_lines.is_empty() {
            if !leaving_review {
                transaction.extend_from_slice(next_live.clear_for_history_redraw().as_bytes());
            }
            let mut history = String::new();
            append_history_lines(&mut history, &history_lines);
            transaction.extend_from_slice(history.as_bytes());
        }

        let mut live_bytes = Vec::new();
        if let Err(error) = next_live.render_to(&mut live_bytes, frame.live.clone(), frame.cursor) {
            if entering_review {
                if let Some(modes) = &mut self.modes {
                    modes.set_review_active(false);
                }
            } else if leaving_review {
                if let Some(modes) = &mut self.modes {
                    modes.set_review_active(true);
                }
            }
            return Err(error);
        }
        transaction.extend_from_slice(&live_bytes);
        if transaction.is_empty() {
            return Ok(());
        }

        let transaction = if self.synchronized_output {
            format!(
                "{SYNCHRONIZED_OUTPUT_START}{}{}",
                String::from_utf8_lossy(&transaction),
                String::from_utf8_lossy(SYNCHRONIZED_OUTPUT_END)
            )
            .into_bytes()
        } else {
            transaction
        };
        if let Err(error) = output.write_all(&transaction).and_then(|()| output.flush()) {
            if self.synchronized_output {
                let _ = output.write_all(SYNCHRONIZED_OUTPUT_END);
                let _ = output.flush();
            }
            if entering_review || leaving_review {
                recover_review_transition(&mut self.modes, output, entering_review);
            }
            return Err(error);
        }

        self.live = next_live;
        if entering_review {
            self.review_surface = true;
            self.saved_normal_live = saved_normal_live;
            if let Some(modes) = &mut self.modes {
                modes.set_review_active(true);
            }
        } else if leaving_review {
            self.review_surface = false;
            self.saved_normal_live = None;
            if let Some(modes) = &mut self.modes {
                modes.set_review_active(false);
            }
        }
        Ok(())
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.live.resize(width, height);
        if let Some(saved) = &mut self.saved_normal_live {
            saved.resize(width, height);
        }
    }

    pub fn suspend_prepare(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        let was_review = self.review_surface;
        if self.review_surface {
            let mut transition = Vec::new();
            append_review_transition(&mut transition, &mut self.modes, false)?;
            if let Some(saved) = self.saved_normal_live.as_ref() {
                let mut saved = saved.clone();
                transition.extend_from_slice(saved.clear_for_history_redraw().as_bytes());
            }
            if let Err(error) = output.write_all(&transition).and_then(|()| output.flush()) {
                recover_review_transition(&mut self.modes, output, false);
                return Err(error);
            }
            self.saved_normal_live = None;
            if let Some(modes) = &mut self.modes {
                modes.set_review_active(false);
            }
            self.review_surface = false;
            self.live.reset();
        }
        let result = if was_review {
            Ok(())
        } else {
            self.clear_live_to(output, false)
        };
        if let Some(modes) = &mut self.modes {
            modes.leave();
        }
        result
    }

    pub fn resume(&mut self) -> std::io::Result<()> {
        if let Some(modes) = &mut self.modes {
            modes.resume()?;
        }
        self.live.reset();
        Ok(())
    }

    pub fn leave(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        if self.review_surface {
            let mut transition = Vec::new();
            append_review_transition(&mut transition, &mut self.modes, false)?;
            if let Some(saved) = self.saved_normal_live.as_ref() {
                let mut saved = saved.clone();
                transition.extend_from_slice(saved.clear_for_history_redraw().as_bytes());
            }
            if let Err(error) = output.write_all(&transition).and_then(|()| output.flush()) {
                recover_review_transition(&mut self.modes, output, false);
                return Err(error);
            }
            self.saved_normal_live = None;
            if let Some(modes) = &mut self.modes {
                modes.set_review_active(false);
            }
            self.review_surface = false;
            self.live.reset();
            if let Some(modes) = &mut self.modes {
                modes.leave();
            } else {
                output.write_all(b"\x1b[?25h")?;
                output.flush()?;
            }
            return Ok(());
        }
        let show_cursor = self.modes.is_none();
        let result = self.clear_live_to(output, show_cursor);
        if let Some(modes) = &mut self.modes {
            modes.leave();
        }
        result
    }

    fn clear_live_to(&mut self, output: &mut dyn Write, show_cursor: bool) -> std::io::Result<()> {
        let mut next_live = self.live.clone();
        let mut transaction = next_live.clear_for_history_redraw();
        if show_cursor {
            transaction.push_str("\x1b[?25h");
        }
        output.write_all(transaction.as_bytes())?;
        output.flush()?;
        self.live = next_live;
        Ok(())
    }
}

fn append_review_transition(
    transaction: &mut Vec<u8>,
    modes: &mut Option<TerminalModeGuard>,
    entering: bool,
) -> std::io::Result<()> {
    if let Some(modes) = modes {
        if entering {
            modes.enter_review(transaction)
        } else {
            modes.leave_review(transaction)
        }
    } else if entering {
        write_enter_review_output(transaction)
    } else {
        write_leave_review_output(transaction)
    }
}

fn recover_review_transition(
    modes: &mut Option<TerminalModeGuard>,
    output: &mut dyn Write,
    entering: bool,
) {
    if let Some(modes) = modes {
        if entering {
            modes.set_review_active(true);
            modes.leave();
        } else {
            modes.set_review_active(true);
        }
    } else if entering {
        let _ = write_leave_review_output(output);
    } else {
        let _ = write_enter_review_output(output);
    }
}

fn flatten_history(blocks: &[FinalizedBlock]) -> Vec<String> {
    let mut lines = Vec::new();
    for block in blocks {
        if block.separator_before && !block.lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(block.lines.iter().cloned());
    }
    lines
}

fn append_history_lines(output: &mut String, lines: &[String]) {
    for line in lines {
        output.push('\r');
        output.push_str(line);
        output.push_str("\r\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_invalidates_live_cache() {
        let mut terminal = InlineTerminal::for_test(80, 12);
        terminal
            .live
            .render_to(&mut Vec::new(), vec!["live".to_owned()], None)
            .expect("initial live frame");

        terminal.resize(50, 8);

        let mut redraw = Vec::new();
        terminal
            .live
            .render_to(&mut redraw, vec!["live".to_owned()], None)
            .expect("live redraw after resize");
        assert!(String::from_utf8(redraw).unwrap().contains("live"));
    }
}
