use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::screen_output::{CursorPos, TerminalFrame};
use crate::shell::{NeoChromeState, OverlayKind};
use crate::transcript::{
    CHROME_GUTTER, ChromeRender, TranscriptPane, TranscriptViewport, apply_gutter,
    frame_content_width, render_chrome_lines_mut, render_footer_only_lines,
};

pub struct NeoTui {
    chrome: NeoChromeState,
    transcript: TranscriptPane,
    /// Latched automatic overflow viewport. `None` means the primary surface;
    /// `Some` keeps the alternate surface until the live commit frontier clears.
    automatic_overflow: Option<TranscriptViewport>,
}

const ANIMATION_INTERVAL: Duration = Duration::from_millis(100);

impl NeoTui {
    #[must_use]
    pub fn new(chrome: NeoChromeState, transcript: TranscriptPane) -> Self {
        Self {
            chrome,
            transcript,
            automatic_overflow: None,
        }
    }

    #[must_use]
    pub fn with_welcome_banner(
        chrome: NeoChromeState,
        width: usize,
        height: usize,
        version: &str,
        neo_home: Option<PathBuf>,
    ) -> Self {
        let mut transcript = TranscriptPane::new(width, height);
        transcript.set_neo_home(neo_home);
        transcript.set_theme(chrome.theme());
        transcript.push_welcome_banner(
            chrome.title(),
            chrome.session_label(),
            chrome.model_label(),
            &chrome.cwd_label(),
            version,
            None,
        );
        Self {
            chrome,
            transcript,
            automatic_overflow: None,
        }
    }

    /// Whether automatic transcript overflow currently owns the alternate surface.
    #[must_use]
    pub const fn automatic_overflow_active(&self) -> bool {
        self.automatic_overflow.is_some()
    }

    pub fn scroll_automatic_overflow_up(&mut self, rows: usize) {
        if let Some(viewport) = self.automatic_overflow.as_mut() {
            viewport.scroll_up(rows);
        }
    }

    pub fn scroll_automatic_overflow_down(&mut self, rows: usize) {
        if let Some(viewport) = self.automatic_overflow.as_mut() {
            viewport.scroll_down(rows);
        }
    }

    #[must_use]
    pub const fn chrome(&self) -> &NeoChromeState {
        &self.chrome
    }

    pub fn chrome_mut(&mut self) -> &mut NeoChromeState {
        &mut self.chrome
    }

    #[must_use]
    pub const fn transcript(&self) -> &TranscriptPane {
        &self.transcript
    }

    pub fn transcript_mut(&mut self) -> &mut TranscriptPane {
        &mut self.transcript
    }

    /// Whether the transcript pane has pending changes requiring a re-render.
    #[must_use]
    pub fn is_transcript_dirty(&self) -> bool {
        self.transcript.is_dirty()
    }

    pub fn render_frame(
        &mut self,
        width: usize,
        height: usize,
    ) -> (Vec<String>, Option<CursorPos>) {
        if self.chrome.transcript_browser_state().is_some() {
            let chrome =
                fit_chrome_to_height(render_chrome(&mut self.chrome, width, height), height);
            let mut lines = self
                .render_transcript_browser_frame(width, height.saturating_sub(chrome.lines.len()))
                .expect("transcript browser state exists");
            let cursor = append_chrome(&mut lines, chrome);
            return (lines, cursor);
        }
        if let Some(mut lines) = render_full_screen_overlay_frame(&self.chrome, width, height) {
            lines.truncate(height);
            apply_gutter(&mut lines);
            return (lines, None);
        }

        let chrome_render =
            fit_chrome_to_height(render_chrome(&mut self.chrome, width, height), height);
        let chrome_height = chrome_render.lines.len();
        if self.transcript.live_chrome_height() != chrome_height {
            self.transcript.set_live_chrome_height(chrome_height);
        }
        self.transcript.set_theme(self.chrome.theme());
        self.transcript
            .set_image_render_policy(self.chrome.image_render_policy());
        self.transcript
            .set_image_capabilities(self.chrome.image_capabilities());
        self.transcript
            .set_workspace_root(self.chrome.workspace_root());
        self.transcript.resize(width, height);
        let mut lines = self
            .transcript
            .render_frame(width, height)
            .unwrap_or_else(|| self.transcript.frame_ansi_lines());
        let cursor = append_chrome(&mut lines, chrome_render);
        (lines, cursor)
    }

    #[must_use]
    pub fn render_terminal_frame(&mut self, width: usize, height: usize) -> TerminalFrame {
        self.render_terminal_frame_at(width, height, Instant::now())
    }

    #[must_use]
    pub fn render_terminal_frame_at(
        &mut self,
        width: usize,
        height: usize,
        now: Instant,
    ) -> TerminalFrame {
        let task_browser_open = self.chrome.task_browser_state().is_some();
        let manual_review_open = self
            .chrome
            .find_overlay_by_kind(|kind| matches!(kind, OverlayKind::TranscriptBrowser(_)))
            .is_some();
        if let Some(mut lines) = render_full_screen_overlay_frame(&self.chrome, width, height) {
            lines.truncate(height);
            apply_gutter(&mut lines);
            // Preserve the primary live anchor while the full-screen overlay is open.
            return TerminalFrame::with_surface(Vec::new(), lines, None, true, None)
                .with_mouse_capture(task_browser_open);
        }

        let chrome_render =
            fit_chrome_to_height(render_chrome(&mut self.chrome, width, height), height);
        let chrome_height = chrome_render.lines.len();
        if self.transcript.live_chrome_height() != chrome_height {
            self.transcript.set_live_chrome_height(chrome_height);
        }
        self.transcript.set_theme(self.chrome.theme());
        self.transcript
            .set_image_render_policy(self.chrome.image_render_policy());
        self.transcript
            .set_image_capabilities(self.chrome.image_capabilities());
        self.transcript
            .set_workspace_root(self.chrome.workspace_root());
        self.transcript.resize(width, height);

        // Always obtain the complete presentation update so overflow/frontier
        // signals stay accurate even while manual review owns the viewport.
        let mut update = self.transcript.render_terminal_update(width, height);
        if update.live_overflow && self.automatic_overflow.is_none() {
            self.automatic_overflow = Some(TranscriptViewport::new());
        }
        if !update.has_live_frontier {
            self.automatic_overflow = None;
        }

        let next_animation_deadline = (self.chrome.working_label().is_some()
            || update.has_visible_animation
            || self.transcript.has_visible_animation()
            || self.transcript.has_live_entries())
        .then(|| now.checked_add(ANIMATION_INTERVAL).unwrap_or(now));

        // Manual Ctrl+O review keeps logical precedence but shares the same
        // physical alternate surface with latched automatic overflow.
        if self.chrome.transcript_browser_state().is_some() {
            let mut lines = self
                .render_transcript_browser_frame(
                    width,
                    height.saturating_sub(chrome_render.lines.len()),
                )
                .expect("transcript browser state exists");
            let cursor = append_chrome(&mut lines, chrome_render);
            return TerminalFrame::with_surface(
                Vec::new(),
                lines,
                cursor,
                true,
                next_animation_deadline,
            );
        }

        if let Some(viewport) = self.automatic_overflow.as_mut() {
            let body_height = height.saturating_sub(chrome_render.lines.len());
            let mut lines = {
                self.transcript
                    .render_viewport_rows(viewport, width, body_height)
            };
            let cursor = append_chrome(&mut lines, chrome_render);
            if lines.len() > height {
                lines.truncate(height);
            }
            return TerminalFrame::with_surface(
                Vec::new(),
                lines,
                cursor,
                true,
                next_animation_deadline,
            );
        }

        for block in &mut update.history {
            apply_gutter(&mut block.lines);
        }
        let cursor = append_chrome(&mut update.live, chrome_render);
        if update.live.len() > height {
            update.live.truncate(height);
        }
        TerminalFrame::with_surface(
            update.history,
            update.live,
            cursor,
            manual_review_open,
            next_animation_deadline,
        )
    }

    pub fn advance_animation_at(&mut self, _now: Instant) {
        self.chrome.advance_activity_frame();
        self.transcript.advance_animation_at_ms(current_time_ms());
    }

    pub fn acknowledge_history(&mut self, frame: &TerminalFrame) {
        if frame.review_surface {
            return;
        }
        self.transcript.acknowledge_history(&frame.history);
    }

    pub fn render(&mut self, width: usize, height: usize) -> Vec<String> {
        self.render_frame(width, height).0
    }

    fn render_transcript_browser_frame(
        &mut self,
        width: usize,
        height: usize,
    ) -> Option<Vec<String>> {
        self.chrome.transcript_browser_state()?;
        self.transcript.set_theme(self.chrome.theme());
        self.transcript
            .set_image_render_policy(self.chrome.image_render_policy());
        self.transcript
            .set_image_capabilities(self.chrome.image_capabilities());
        self.transcript
            .set_workspace_root(self.chrome.workspace_root());
        self.transcript.resize(width, height);
        let state = self.chrome.transcript_browser_state_mut()?;
        Some(self.transcript.render_browser_rows(state, width, height))
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn render_full_screen_overlay_frame(
    app: &NeoChromeState,
    width: usize,
    height: usize,
) -> Option<Vec<String>> {
    if !app.focused_overlay_blocks_prompt() {
        return None;
    }
    let content_width = frame_content_width(width);
    app.render_focused_full_screen_overlay(content_width, height)
}

fn render_chrome(app: &mut NeoChromeState, width: usize, height: usize) -> ChromeRender {
    let content_width = frame_content_width(width);
    if app.focused_overlay_blocks_prompt()
        && app
            .focused_overlay()
            .is_some_and(|overlay| !matches!(overlay.kind, OverlayKind::QuestionDialog(_)))
    {
        let overlay = app
            .render_focused_overlay(content_width)
            .unwrap_or_default();
        let footer = render_footer_only_lines(app, width);
        ChromeRender {
            lines: overlay.into_iter().chain(footer).collect(),
            cursor: None,
            prompt_start_row: 0,
        }
    } else {
        render_chrome_lines_mut(app, width, height)
    }
}

fn append_chrome(lines: &mut Vec<String>, chrome: ChromeRender) -> Option<CursorPos> {
    let body_len = lines.len();
    lines.extend(chrome.lines);
    apply_gutter(lines);
    chrome.cursor.map(|cursor| CursorPos {
        row: body_len + chrome.prompt_start_row + cursor.row,
        col: cursor.col + CHROME_GUTTER,
    })
}

fn fit_chrome_to_height(mut chrome: ChromeRender, height: usize) -> ChromeRender {
    if chrome.lines.len() <= height {
        return chrome;
    }

    let removed = chrome.lines.len() - height;
    chrome.lines.drain(..removed);
    chrome.cursor = chrome.cursor.and_then(|cursor| {
        chrome
            .prompt_start_row
            .checked_add(cursor.row)
            .and_then(|row| row.checked_sub(removed))
            .filter(|row| *row < height)
            .map(|row| CursorPos {
                row,
                col: cursor.col,
            })
    });
    chrome.prompt_start_row = 0;
    chrome
}
