use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::input::MouseEvent;
use crate::screen_output::{CursorPos, TerminalFrame};
use crate::shell::{NeoChromeState, OverlayKind};
use crate::transcript::chrome_render::extract_cursor;
use crate::transcript::{
    CHROME_GUTTER, ChromeRender, TranscriptPane, apply_gutter, frame_content_width,
    render_chrome_lines_mut, render_footer_only_lines,
};

pub struct NeoTui {
    chrome: NeoChromeState,
    transcript: TranscriptPane,
}

const ANIMATION_INTERVAL: Duration = Duration::from_millis(100);

impl NeoTui {
    #[must_use]
    pub fn new(chrome: NeoChromeState, transcript: TranscriptPane) -> Self {
        Self { chrome, transcript }
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
        Self { chrome, transcript }
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
        if let Some(mut lines) = render_full_screen_overlay_frame(&self.chrome, width, height) {
            lines.truncate(height);
            apply_gutter(&mut lines);
            return (lines, None);
        }

        let chrome_render =
            fit_chrome_to_height(render_chrome(&mut self.chrome, width, height), height);
        let chrome_height = chrome_render.lines.len();
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
        lines.truncate(height.saturating_sub(chrome_height));
        let cursor = append_chrome(&mut lines, chrome_render, width, height);
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
        if let Some(mut lines) = render_full_screen_overlay_frame(&self.chrome, width, height) {
            lines.truncate(height);
            apply_gutter(&mut lines);
            // The fullscreen surface is already active; blocking overlays
            // (Task Browser, rich dialogs) render inside it without any
            // physical transition.
            return TerminalFrame::new(lines, None);
        }

        self.transcript.set_theme(self.chrome.theme());
        self.transcript
            .set_image_render_policy(self.chrome.image_render_policy());
        self.transcript
            .set_image_capabilities(self.chrome.image_capabilities());
        self.transcript
            .set_workspace_root(self.chrome.workspace_root());
        self.transcript.resize(width, height);

        // Compose the visible document slice and the fitted chrome once. The
        // document slice is bounded to the remaining body height, so the
        // combined frame can never overflow the terminal.
        let chrome_render =
            fit_chrome_to_height(render_chrome(&mut self.chrome, width, height), height);
        let chrome_height = chrome_render.lines.len();
        let mut lines = self
            .transcript
            .render_terminal_slice(width, height.saturating_sub(chrome_height));
        let cursor = append_chrome(&mut lines, chrome_render, width, height);

        let next_animation_deadline = (self.chrome.working_label().is_some()
            || self.transcript.has_visible_animation()
            || self.transcript.has_live_entries()
            || self.transcript.selection_requests_animation())
        .then(|| now.checked_add(ANIMATION_INTERVAL).unwrap_or(now));

        TerminalFrame::with_animation_deadline(lines, cursor, next_animation_deadline)
    }

    pub fn advance_animation_at(&mut self, _now: Instant) {
        self.chrome.advance_activity_frame();
        self.transcript.advance_animation_at_ms(current_time_ms());
    }

    /// Map a screen-space mouse event into transcript-body coordinates and
    /// route it to the document selection. In the fullscreen layout the
    /// transcript body occupies the top of the screen (the chrome is appended
    /// below it), so the body row equals the screen row and the body column
    /// is the screen column minus the gutter. Wheel events and Shift-modified
    /// drags are not consumed here; the runtime routes wheels and the
    /// terminal emulator owns Shift selection.
    pub fn handle_mouse_event(&mut self, event: MouseEvent) {
        if !event.is_selection_event() || event.is_shift_modified() {
            return;
        }
        if self.chrome.focused_overlay_blocks_prompt() {
            // Blocking overlays own the full screen; they keep input priority.
            return;
        }
        let body_col = usize::from(event.column).saturating_sub(CHROME_GUTTER);
        self.transcript
            .handle_mouse_event(event, usize::from(event.row), body_col);
    }

    pub fn render(&mut self, width: usize, height: usize) -> Vec<String> {
        self.render_frame(width, height).0
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

fn append_chrome(
    lines: &mut Vec<String>,
    chrome: ChromeRender,
    width: usize,
    height: usize,
) -> Option<CursorPos> {
    debug_assert!(
        lines.len() + chrome.lines.len() <= height,
        "transcript and fitted chrome must fit the terminal height: body={} chrome={} height={}",
        lines.len(),
        chrome.lines.len(),
        height
    );
    let body_cursor = extract_cursor(lines);
    let body_len = lines.len();
    lines.extend(chrome.lines);
    apply_gutter(lines);
    let cursor = chrome
        .cursor
        .map(|cursor| CursorPos {
            row: body_len + chrome.prompt_start_row + cursor.row,
            col: cursor.col + CHROME_GUTTER,
        })
        .or_else(|| {
            body_cursor.map(|cursor| CursorPos {
                row: cursor.row,
                col: cursor.col + CHROME_GUTTER,
            })
        });
    debug_assert!(
        cursor.is_none_or(|cursor| cursor.row < height && cursor.col < width),
        "terminal cursor must remain inside the rendered frame"
    );
    cursor
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
