//! Extracted: TUI snapshot/frame rendering helpers for tests and rendering.

use neo_tui::primitive::theme::TuiTheme;
use neo_tui::shell::{NeoChromeState, OverlayKind};
use neo_tui::transcript::TranscriptPane;

#[cfg(test)]
pub(super) fn compose_tui_frame(
    app: &NeoChromeState,
    transcript: &mut TranscriptPane,
    cols: u16,
    rows: u16,
) -> Option<Vec<String>> {
    if cols == 0 || rows == 0 {
        return None;
    }
    transcript.mark_dirty();
    let mut tui = neo_tui::NeoTui::new(app.clone(), transcript.clone());
    let (lines, _) = tui.render_frame(usize::from(cols), usize::from(rows));
    *transcript = tui.transcript().clone();
    Some(lines)
}

pub(super) fn render_transcript_snapshot(
    app: &NeoChromeState,
    transcript: &mut TranscriptPane,
    width: usize,
    height: usize,
) -> String {
    transcript.resize(width, height);
    transcript.mark_dirty();
    let _ = transcript.render_frame(width, height);

    let mut lines = transcript
        .frame_ansi_lines()
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line).trim_end().to_owned())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    lines.extend(render_overlay_snapshot(app, width));
    format!("{}\n", lines.join("\n").trim_end())
}

pub(super) fn render_overlay_snapshot(app: &NeoChromeState, width: usize) -> Vec<String> {
    let content_width = neo_tui::transcript::frame_content_width(width);
    let mut lines = render_overlay_content_snapshot(app, content_width);
    lines.extend(render_chrome_snapshot_lines(app, width));
    lines
}

fn render_overlay_content_snapshot(app: &NeoChromeState, content_width: usize) -> Vec<String> {
    match app.focused_overlay().map(|overlay| &overlay.kind) {
        Some(OverlayKind::SessionPicker(picker)) => {
            let theme = app.theme();
            picker.render_lines(content_width, &theme)
        }
        Some(OverlayKind::ModelPicker(picker)) => {
            let theme = app.theme();
            render_picker_snapshot("Models", picker, content_width, &theme)
        }
        Some(OverlayKind::CommandPalette(_)) => vec!["Commands".to_owned()],
        Some(OverlayKind::PromptCompletion(_)) => vec![],
        Some(OverlayKind::Message(message)) => vec![message.clone()],
        Some(OverlayKind::QuestionDialog(_)) | None => Vec::new(),
        // Rich dialogs — use their own render_lines.
        Some(_) => app.focused_overlay_lines(content_width),
    }
}

fn render_chrome_snapshot_lines(
    app: &NeoChromeState,
    width: usize,
) -> impl Iterator<Item = String> {
    neo_tui::transcript::render_chrome_lines(app, width, 24)
        .lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line).trim_end().to_owned())
}

pub(super) fn render_picker_snapshot(
    title: &str,
    picker: &neo_tui::shell::PickerState,
    width: usize,
    theme: &TuiTheme,
) -> Vec<String> {
    let mut lines = vec![title.to_owned()];
    lines.extend(picker.render_lines(width, theme));
    lines
}
