use crate::app::{NeoTuiApp, OverlayKind};
use crate::pi_tui::CursorPos;
use crate::transcript::{
    CHROME_GUTTER, ChromeRender, TranscriptPane, apply_gutter, frame_content_width,
    render_chrome_lines, render_footer_only_lines,
};

pub struct NeoTui {
    app: NeoTuiApp,
    transcript: TranscriptPane,
}

impl NeoTui {
    #[must_use]
    pub fn new(app: NeoTuiApp, transcript: TranscriptPane) -> Self {
        Self { app, transcript }
    }

    #[must_use]
    pub fn with_welcome_banner(app: NeoTuiApp, width: usize, height: usize, version: &str) -> Self {
        let mut transcript = TranscriptPane::new(width, height);
        transcript.set_theme(app.theme());
        transcript.push_welcome_banner(
            app.title(),
            app.session_label(),
            app.model_label(),
            &app.cwd_label(),
            version,
            None,
        );
        Self { app, transcript }
    }

    #[must_use]
    pub const fn app(&self) -> &NeoTuiApp {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut NeoTuiApp {
        &mut self.app
    }

    #[must_use]
    pub const fn transcript(&self) -> &TranscriptPane {
        &self.transcript
    }

    pub fn transcript_mut(&mut self) -> &mut TranscriptPane {
        &mut self.transcript
    }

    pub fn split_mut(&mut self) -> (&mut NeoTuiApp, &mut TranscriptPane) {
        (&mut self.app, &mut self.transcript)
    }

    pub fn replace_transcript(&mut self, transcript: TranscriptPane) {
        self.transcript = transcript;
    }

    pub fn render_frame(
        &mut self,
        width: usize,
        height: usize,
    ) -> (Vec<String>, Option<CursorPos>) {
        let app = &self.app;
        self.transcript.set_theme(app.theme());
        self.transcript.resize(width, height);
        let mut lines = self
            .transcript
            .render_frame(width, height)
            .unwrap_or_else(|| self.transcript.frame_ansi_lines());
        let chrome = render_chrome(app, width);
        let cursor = append_chrome(&mut lines, chrome);
        (lines, cursor)
    }

    pub fn render(&mut self, width: usize, height: usize) -> Vec<String> {
        self.render_frame(width, height).0
    }
}

fn render_chrome(app: &NeoTuiApp, width: usize) -> ChromeRender {
    if app.focused_overlay().is_some_and(|overlay| {
        matches!(
            overlay.kind,
            OverlayKind::SessionPicker(_)
                | OverlayKind::ModelSelector(_)
                | OverlayKind::TabbedModelSelector(_)
                | OverlayKind::ProviderManager(_)
                | OverlayKind::ChoicePicker(_)
                | OverlayKind::ApiKeyInput(_)
                | OverlayKind::CustomRegistryImport(_)
        )
    }) {
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
        render_chrome_lines(app, width)
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
