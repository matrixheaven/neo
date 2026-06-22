use crate::chrome::{NeoChromeState, OverlayKind};
use crate::terminal::CursorPos;
use crate::transcript::{
    CHROME_GUTTER, ChromeRender, TranscriptPane, apply_gutter, frame_content_width,
    render_chrome_lines_mut, render_footer_only_lines,
};

pub struct NeoTui {
    chrome: NeoChromeState,
    transcript: TranscriptPane,
}

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
    ) -> Self {
        let mut transcript = TranscriptPane::new(width, height);
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

    pub fn render_frame(
        &mut self,
        width: usize,
        height: usize,
    ) -> (Vec<String>, Option<CursorPos>) {
        let chrome_render = render_chrome(&mut self.chrome, width, height);
        let chrome_height = chrome_render.lines.len();
        if self.transcript.live_chrome_height() != chrome_height {
            self.transcript.set_live_chrome_height(chrome_height);
        }
        self.transcript.set_theme(self.chrome.theme());
        self.transcript.resize(width, height);
        let mut lines = self
            .transcript
            .render_frame(width, height)
            .unwrap_or_else(|| self.transcript.frame_ansi_lines());
        let cursor = append_chrome(&mut lines, chrome_render);
        (lines, cursor)
    }

    pub fn render(&mut self, width: usize, height: usize) -> Vec<String> {
        self.render_frame(width, height).0
    }
}

fn render_chrome(app: &mut NeoChromeState, width: usize, height: usize) -> ChromeRender {
    if app.focused_overlay_blocks_prompt()
        && app.focused_overlay().is_some_and(|overlay| {
            !matches!(
                overlay.kind,
                OverlayKind::QuestionDialog(_) | OverlayKind::Approval(_)
            )
        })
    {
        let content_width = frame_content_width(width);
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
