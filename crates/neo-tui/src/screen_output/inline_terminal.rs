use std::io::Write;
use std::time::Instant;

use crate::terminal_capabilities::TerminalCapabilities;
use crate::transcript::FinalizedBlock;

use super::LiveRenderer;
use super::terminal_modes::TerminalModeGuard;
use super::types::CursorPos;

const SYNCHRONIZED_OUTPUT_START: &str = "\x1b[?2026h";
const SYNCHRONIZED_OUTPUT_END: &[u8] = b"\x1b[?2026l";

#[derive(Debug, Clone)]
pub struct TerminalFrame {
    pub history: Vec<FinalizedBlock>,
    pub live: Vec<String>,
    pub cursor: Option<CursorPos>,
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
            next_animation_deadline: None,
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
            next_animation_deadline,
        }
    }
}

#[derive(Debug)]
pub struct InlineTerminal {
    synchronized_output: bool,
    live: LiveRenderer,
    modes: Option<TerminalModeGuard>,
}

impl InlineTerminal {
    #[must_use]
    pub fn new(width: u16, height: u16, capabilities: TerminalCapabilities) -> Self {
        Self {
            synchronized_output: capabilities.ansi.synchronized_output,
            live: LiveRenderer::new(width, height),
            modes: None,
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
        let history_lines = flatten_history(&frame.history);
        let mut next_live = self.live.clone();
        let mut transaction = String::new();
        if !history_lines.is_empty() {
            transaction.push_str(&next_live.clear_for_history_redraw());
            append_history_lines(&mut transaction, &history_lines);
        }

        let mut live_bytes = Vec::new();
        next_live.render_to(&mut live_bytes, frame.live.clone(), frame.cursor)?;
        transaction.push_str(&String::from_utf8_lossy(&live_bytes));
        if transaction.is_empty() {
            return Ok(());
        }

        let transaction = if self.synchronized_output {
            format!(
                "{SYNCHRONIZED_OUTPUT_START}{transaction}{}",
                String::from_utf8_lossy(SYNCHRONIZED_OUTPUT_END)
            )
        } else {
            transaction
        };
        if let Err(error) = output
            .write_all(transaction.as_bytes())
            .and_then(|()| output.flush())
        {
            if self.synchronized_output {
                let _ = output.write_all(SYNCHRONIZED_OUTPUT_END);
                let _ = output.flush();
            }
            return Err(error);
        }

        self.live = next_live;
        Ok(())
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.live.resize(width, height);
    }

    pub fn suspend_prepare(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        let result = self.clear_live_to(output, false);
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
