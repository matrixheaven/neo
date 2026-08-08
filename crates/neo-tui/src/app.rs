use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::frame_selection::{FrameSelection, FrameTextMap};
use crate::input::MouseEvent;
use crate::primitive::{Style, pad_to_width, paint, truncate_to_width};
use crate::screen_output::{CursorPos, TerminalFrame};
use crate::shell::{NeoChromeState, OverlayKind};
use crate::transcript::chrome_render::extract_cursor;
use crate::transcript::{
    CHROME_GUTTER, ChromeRender, ChromeRowKind, MOVEMENT_THRESHOLD, TranscriptPane, apply_gutter,
    frame_content_width, prompt_body_width, render_chrome_lines_mut, render_footer_only_lines,
};
pub struct NeoTui {
    chrome: NeoChromeState,
    transcript: TranscriptPane,
    /// Row layout of the most recently rendered frame, used to route mouse
    /// events to the transcript body or the chrome widget that owns a row.
    last_layout: Option<FrameLayout>,
    /// Terminal size of the most recently rendered frame, used by the Task
    /// Browser to hit-test pointer events against its rendered regions.
    last_frame_size: Option<(usize, usize)>,
    /// Plain text a right-click asked to copy (by region), drained by the
    /// controller after the mouse event is routed.
    pending_copy: Option<String>,
    clipboard_notice_until: Option<Instant>,
    /// Owner of the in-progress pointer gesture (left press → drag →
    /// release), fixed at the press and cleared at the release.
    active_gesture: Option<GestureOwner>,
    /// Final-frame text map of the most recently rendered frame, used to
    /// materialize frame-selection copies from the visible rows.
    frame_map: FrameTextMap,
    /// Screen-coordinate mouse selection over frame rows (todo panel,
    /// footer, rich dialogs, full-screen overlays).
    frame_selection: FrameSelection,
}

/// Per-row classification of the last rendered frame, for mouse routing.
#[derive(Debug, Clone)]
struct FrameLayout {
    width: usize,
    body_rows: usize,
    row_kinds: Vec<ChromeRowKind>,
}

impl FrameLayout {
    /// Region-local row of `chrome_row` inside the first run of `kind`
    /// (e.g. the Todo panel or the prompt content), or `None` when the row
    /// is outside that region.
    fn region_row(&self, kind: ChromeRowKind, chrome_row: usize) -> Option<usize> {
        let start = self
            .row_kinds
            .iter()
            .position(|candidate| *candidate == kind)?;
        (chrome_row >= start).then_some(chrome_row - start)
    }
}

/// Content column inside the prompt box for a chrome-line column: the box
/// border (1 cell) and the `> ` prefix (3 cells) precede the text.
fn prompt_content_col(chrome_col: usize) -> usize {
    chrome_col.saturating_sub(1 + 3)
}

/// Which region owns the current pointer gesture. The owner is fixed at the
/// left-button press and never re-evaluated, so a drag crossing out of its
/// region still routes back to it; the release ends and clears the gesture
/// wherever the pointer lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GestureOwner {
    Transcript,
    Prompt(PromptGesture),
    Frame,
}

/// Press anchor of a prompt mouse gesture; the selection endpoints are only
/// created once movement past the threshold confirms a drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PromptGesture {
    /// Screen row of the press, for the click/drag movement threshold even
    /// after the pointer leaves the prompt rows.
    press_row: usize,
    /// Screen column (minus the gutter) of the press.
    press_col: usize,
    /// Char index of the press inside the prompt.
    anchor_char: usize,
    /// Threshold crossed: the gesture is a drag, not a click.
    dragging: bool,
}

const ANIMATION_INTERVAL: Duration = Duration::from_millis(100);
const CLIPBOARD_NOTICE_DURATION: Duration = Duration::from_millis(1_500);

impl NeoTui {
    #[must_use]
    pub fn new(chrome: NeoChromeState, transcript: TranscriptPane) -> Self {
        Self {
            chrome,
            transcript,
            last_layout: None,
            last_frame_size: None,
            pending_copy: None,
            clipboard_notice_until: None,
            active_gesture: None,
            frame_map: FrameTextMap::default(),
            frame_selection: FrameSelection::new(),
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
            last_layout: None,
            last_frame_size: None,
            pending_copy: None,
            clipboard_notice_until: None,
            active_gesture: None,
            frame_map: FrameTextMap::default(),
            frame_selection: FrameSelection::new(),
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

    /// Terminal size of the most recently rendered frame. `None` before the
    /// first render; the Task Browser pointer router treats a missing size as
    /// "no frame to hit-test yet" and consumes the event as a no-op.
    #[must_use]
    pub const fn last_frame_size(&self) -> Option<(usize, usize)> {
        self.last_frame_size
    }

    pub fn render_frame(
        &mut self,
        width: usize,
        height: usize,
    ) -> (Vec<String>, Option<CursorPos>) {
        self.last_frame_size = Some((width, height));
        if let Some(mut lines) = render_full_screen_overlay_frame(&self.chrome, width, height) {
            lines.truncate(height);
            apply_gutter(&mut lines);
            let row_kinds = vec![ChromeRowKind::Other; lines.len()];
            self.last_layout = Some(FrameLayout {
                width,
                body_rows: 0,
                row_kinds: row_kinds.clone(),
            });
            self.finalize_frame(&mut lines, width, height, Instant::now(), 0, &row_kinds);
            return (lines, None);
        }

        let mut chrome_render =
            fit_chrome_to_height(render_chrome(&mut self.chrome, width, height), height);
        self.apply_clipboard_notice(&mut chrome_render, width, Instant::now());
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
        let body_rows = lines.len();
        let row_kinds = chrome_render.row_kinds.clone();
        let cursor = append_chrome(&mut lines, chrome_render, width, height);
        self.last_layout = Some(FrameLayout {
            width,
            body_rows,
            row_kinds: row_kinds.clone(),
        });
        self.finalize_frame(
            &mut lines,
            width,
            height,
            Instant::now(),
            body_rows,
            &row_kinds,
        );
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
        self.last_frame_size = Some((width, height));
        if let Some(mut lines) = render_full_screen_overlay_frame(&self.chrome, width, height) {
            lines.truncate(height);
            apply_gutter(&mut lines);
            let row_kinds = vec![ChromeRowKind::Other; lines.len()];
            self.last_layout = Some(FrameLayout {
                width,
                body_rows: 0,
                row_kinds: row_kinds.clone(),
            });
            self.finalize_frame(&mut lines, width, height, now, 0, &row_kinds);
            // The fullscreen surface is already active; blocking overlays
            // (Task Browser, rich dialogs) render inside it without any
            // physical transition. Frames continue only while a pending
            // frame-selection press needs the cadence for long-press
            // activation.
            let animation_deadline = self
                .frame_selection
                .requests_animation()
                .then(|| now.checked_add(ANIMATION_INTERVAL).unwrap_or(now));
            return TerminalFrame::with_animation_deadline(lines, None, animation_deadline);
        }

        self.transcript.set_theme(self.chrome.theme());
        self.transcript
            .set_image_render_policy(self.chrome.image_render_policy());
        self.transcript
            .set_image_capabilities(self.chrome.image_capabilities());
        self.transcript
            .set_workspace_root(self.chrome.workspace_root());
        self.transcript.resize(width, height);
        // Reconcile the document with the store before reading its view: a
        // revision that arrived since the last frame must turn on the
        // locked-view activity notice in the very frame that renders it.
        self.transcript.ensure_layout_current();

        let mut chrome_render =
            fit_chrome_to_height(render_chrome(&mut self.chrome, width, height), height);
        self.apply_clipboard_notice(&mut chrome_render, width, now);
        let chrome_height = chrome_render.lines.len();
        let mut lines = self
            .transcript
            .render_terminal_slice(width, height.saturating_sub(chrome_height));
        let body_rows = lines.len();
        let row_kinds = chrome_render.row_kinds.clone();
        let cursor = append_chrome(&mut lines, chrome_render, width, height);
        self.last_layout = Some(FrameLayout {
            width,
            body_rows,
            row_kinds: row_kinds.clone(),
        });
        self.finalize_frame(&mut lines, width, height, now, body_rows, &row_kinds);

        let animation_deadline = self.animation_deadline_at(now);
        let next_animation_deadline = match (animation_deadline, self.clipboard_notice_until) {
            (Some(animation), Some(clipboard)) => Some(animation.min(clipboard)),
            (animation, clipboard) => animation.or(clipboard),
        };

        TerminalFrame::with_animation_deadline(lines, cursor, next_animation_deadline)
    }

    /// Whether the frame loop must keep rendering at the 100 ms cadence:
    /// activity/live entries, the transcript's own selection needs, or a
    /// pending frame-selection press waiting for long-press activation.
    fn animation_deadline_at(&self, now: Instant) -> Option<Instant> {
        (self.chrome.working_label().is_some()
            || self.transcript.has_visible_animation()
            || self.transcript.has_live_entries()
            || self.transcript.selection_requests_animation()
            || self.frame_selection.requests_animation())
        .then(|| now.checked_add(ANIMATION_INTERVAL).unwrap_or(now))
    }

    /// Shared final-frame pass for both render entry points: record the
    /// final frame into the text map, drive long-press activation, invalidate
    /// a frame selection whose visual state changed, then paint the selection
    /// background over the frame. Runs after cursor extraction and the gutter
    /// on both paths (the clipboard notice is applied before it on the
    /// normal path only).
    fn finalize_frame(
        &mut self,
        lines: &mut [String],
        width: usize,
        height: usize,
        now: Instant,
        body_rows: usize,
        row_kinds: &[ChromeRowKind],
    ) {
        self.frame_map.record(
            width,
            height,
            self.chrome.focused_overlay_id(),
            lines,
            body_rows,
            row_kinds,
        );
        self.frame_selection.tick(now);
        self.frame_selection.validate_against(&self.frame_map);
        self.frame_selection
            .paint_into(lines, self.chrome.theme().selection_bg);
    }

    pub fn advance_animation_at(&mut self, _now: Instant) {
        self.chrome.advance_activity_frame();
        self.transcript.advance_animation_at_ms(current_time_ms());
    }

    pub fn show_clipboard_copied_at(&mut self, now: Instant) {
        self.clipboard_notice_until = now.checked_add(CLIPBOARD_NOTICE_DURATION);
    }

    fn apply_clipboard_notice(
        &mut self,
        chrome_render: &mut ChromeRender,
        width: usize,
        now: Instant,
    ) {
        if self
            .clipboard_notice_until
            .is_some_and(|deadline| deadline <= now)
        {
            self.clipboard_notice_until = None;
        }
        let content_width = frame_content_width(width);
        let notice = if self.clipboard_notice_until.is_some() {
            Some("copied")
        } else if self.has_any_selection() {
            Some(if content_width >= 48 {
                "selected · right-click or ctrl+c to copy"
            } else if content_width >= 24 {
                "selected · ctrl+c to copy"
            } else {
                "ctrl+c to copy"
            })
        } else {
            None
        };
        if let (Some(notice), Some(footer)) = (notice, chrome_render.lines.last_mut()) {
            let label = truncate_to_width(&format!(" {notice} "), content_width);
            *footer = paint(
                &pad_to_width(&label, content_width),
                Style::default()
                    .fg(self.chrome.theme().selected_fg)
                    .bg(self.chrome.theme().selected_bg)
                    .bold(),
            );
        }
    }

    /// Route one screen-space mouse event by gesture owner. The left-button
    /// press picks the owner from the row it lands on (transcript body,
    /// prompt box, or frame surface); every drag and the release of that
    /// gesture keep routing to the same owner, so a drag that crosses out of
    /// its region — the transcript body into the chrome rows, or the prompt
    /// box into the footer — still drives the region that started it.
    /// Column coordinates are the screen column minus the gutter. Wheel
    /// events and Shift-modified drags are not consumed here; the runtime
    /// routes wheels and the terminal emulator owns Shift selection.
    pub fn handle_mouse_event(&mut self, event: MouseEvent) {
        if !event.is_selection_event() || event.is_shift_modified() {
            return;
        }
        let row = usize::from(event.row);
        let col = usize::from(event.column).saturating_sub(CHROME_GUTTER);

        // Before the first frame there is no layout to route against:
        // selection events are ignored instead of guessed at a region.
        let Some(layout) = self.last_layout.clone() else {
            return;
        };

        match event.kind {
            crate::transcript::MouseKind::Press
                if event.button == crossterm::event::MouseButton::Left =>
            {
                self.begin_gesture(event, &layout, row, col);
            }
            crate::transcript::MouseKind::Press
                if event.button == crossterm::event::MouseButton::Right =>
            {
                self.right_click_press(&layout, row);
            }
            crate::transcript::MouseKind::Drag
                if event.button == crossterm::event::MouseButton::Left =>
            {
                // A drag is forwarded only to the gesture's owner, never
                // re-routed by the row the pointer currently crosses.
                if let Some(owner) = self.active_gesture {
                    self.forward_drag(event, &layout, row, col, owner);
                }
            }
            crate::transcript::MouseKind::Release => {
                // The release ends the gesture wherever the pointer lands;
                // releases outside an open gesture are inert.
                if let Some(owner) = self.active_gesture.take() {
                    self.finish_gesture(event, owner);
                }
            }
            _ => {}
        }
    }

    /// Left-button press: pick the gesture owner from the row under the
    /// pointer and clear the other regions' selections before the press.
    fn begin_gesture(&mut self, event: MouseEvent, layout: &FrameLayout, row: usize, col: usize) {
        if row < layout.body_rows {
            self.clear_widget_selections();
            self.transcript.handle_mouse_event(event, row, col);
            self.active_gesture = Some(GestureOwner::Transcript);
            return;
        }
        let chrome_row = row.saturating_sub(layout.body_rows);
        let Some(&kind) = layout.row_kinds.get(chrome_row) else {
            return;
        };
        match kind {
            ChromeRowKind::Prompt => {
                let Some(row_in_prompt) = layout.region_row(ChromeRowKind::Prompt, chrome_row)
                else {
                    return;
                };
                let body_width = prompt_body_width(frame_content_width(layout.width));
                self.transcript.clear_transcript_selection();
                self.frame_selection.clear();
                let char_pos = self.chrome.prompt().char_index_at_content_position(
                    row_in_prompt,
                    prompt_content_col(col),
                    body_width,
                );
                self.chrome
                    .prompt_mut()
                    .move_cursor_to(char_pos, body_width);
                self.active_gesture = Some(GestureOwner::Prompt(PromptGesture {
                    press_row: row,
                    press_col: col,
                    anchor_char: char_pos,
                    dragging: false,
                }));
            }
            ChromeRowKind::Other => {
                self.transcript.clear_transcript_selection();
                self.chrome.prompt_mut().clear_selection();
                // Frame-selection endpoints are full-line display cells (the
                // frame map is recorded after the gutter): the raw screen
                // row and column.
                self.frame_selection
                    .press(row, usize::from(event.column), Instant::now());
                self.active_gesture = Some(GestureOwner::Frame);
            }
        }
    }

    /// Left-button drag: forward to the gesture owner. The transcript keeps
    /// the real screen row — crossing the body bottom into the chrome rows
    /// drives its down auto-scroll — and the prompt clamps its endpoint to
    /// the visible character boundaries once the pointer leaves the prompt
    /// rows.
    fn forward_drag(
        &mut self,
        event: MouseEvent,
        layout: &FrameLayout,
        row: usize,
        col: usize,
        owner: GestureOwner,
    ) {
        match owner {
            GestureOwner::Transcript => {
                self.transcript.handle_mouse_event(event, row, col);
            }
            GestureOwner::Prompt(mut gesture) => {
                let crossing = gesture.dragging
                    || row.abs_diff(gesture.press_row) > usize::from(MOVEMENT_THRESHOLD)
                    || col.abs_diff(gesture.press_col) > usize::from(MOVEMENT_THRESHOLD);
                if !crossing {
                    return;
                }
                gesture.dragging = true;
                if let Some(active) = &mut self.active_gesture {
                    *active = GestureOwner::Prompt(gesture);
                }
                let body_width = prompt_body_width(frame_content_width(layout.width));
                let chrome_row = row.saturating_sub(layout.body_rows);
                let prompt_rows = layout
                    .row_kinds
                    .iter()
                    .filter(|kind| **kind == ChromeRowKind::Prompt)
                    .count();
                let char_pos = match layout
                    .row_kinds
                    .iter()
                    .position(|kind| *kind == ChromeRowKind::Prompt)
                {
                    Some(prompt_start) if chrome_row >= prompt_start => {
                        let row_in_prompt = chrome_row - prompt_start;
                        if row_in_prompt < prompt_rows {
                            // Inside the box: the pointer row and column map
                            // to the text.
                            self.chrome.prompt().char_index_at_content_position(
                                row_in_prompt,
                                prompt_content_col(col),
                                body_width,
                            )
                        } else {
                            // Below the box: clamp the endpoint to the last
                            // visible character (the end of the last rendered
                            // prompt row), never into the text hidden behind
                            // the scroll window.
                            self.chrome.prompt().char_index_at_content_position(
                                prompt_rows - 1,
                                body_width,
                                body_width,
                            )
                        }
                    }
                    Some(_) => {
                        // Above the box: clamp the endpoint to the first
                        // visible character.
                        self.chrome
                            .prompt()
                            .char_index_at_content_position(0, 0, body_width)
                    }
                    None => self.chrome.prompt().char_len(),
                };
                let prompt = self.chrome.prompt_mut();
                prompt.begin_drag_selection(gesture.anchor_char);
                prompt.extend_drag_selection(char_pos);
            }
            GestureOwner::Frame => {
                self.frame_selection.drag(row, usize::from(event.column));
            }
        }
    }

    /// Release: end the gesture in its owner. A prompt click that never
    /// confirmed a drag collapses the selection (a click only places the
    /// caret); confirmed drags keep their selection for copy.
    fn finish_gesture(&mut self, event: MouseEvent, owner: GestureOwner) {
        match owner {
            GestureOwner::Transcript => {
                self.transcript.handle_mouse_event(event, 0, 0);
            }
            GestureOwner::Prompt(gesture) => {
                if !gesture.dragging {
                    self.chrome.prompt_mut().clear_selection();
                }
            }
            GestureOwner::Frame => {
                self.frame_selection.release(&self.frame_map);
            }
        }
    }

    /// Right-button press: copy by the row under the pointer, independent of
    /// any in-progress left gesture (transcript, prompt, or frame copy).
    fn right_click_press(&mut self, layout: &FrameLayout, row: usize) {
        if row < layout.body_rows {
            self.clear_widget_selections();
            self.pending_copy = self.transcript.copy_selected_transcript_text();
            return;
        }
        let chrome_row = row.saturating_sub(layout.body_rows);
        let Some(&kind) = layout.row_kinds.get(chrome_row) else {
            return;
        };
        match kind {
            ChromeRowKind::Prompt => {
                self.pending_copy = self
                    .chrome
                    .prompt()
                    .selection_text()
                    .or_else(|| self.chrome.prompt().copy_text());
                // Mirror the transcript: right-click copy also collapses the
                // selection.
                self.chrome.prompt_mut().clear_selection();
            }
            ChromeRowKind::Other => {
                // Validate before materializing so a selection whose visual
                // state changed since the last frame is never copied with
                // text that differs from its highlight.
                self.frame_selection.validate_against(&self.frame_map);
                self.pending_copy = self.frame_map.materialize(&self.frame_selection);
                // Mirror the prompt: right-click copy also collapses the
                // selection (the transcript keeps its highlight instead).
                self.frame_selection.clear();
            }
        }
    }

    /// Whether any region currently has a selection (transcript, prompt, or
    /// the frame surface) — drives the selection hint line.
    #[must_use]
    pub fn has_any_selection(&self) -> bool {
        self.transcript.has_transcript_selection()
            || self.chrome.prompt().selection_range().is_some()
            || self.frame_selection.is_active()
    }

    /// Clear every region's selection (transcript, prompt, frame).
    pub fn clear_all_selections(&mut self) {
        self.transcript.clear_transcript_selection();
        self.chrome.prompt_mut().clear_selection();
        self.frame_selection.clear();
    }

    /// Plain text a right-click requested to copy, drained by the controller
    /// after the mouse event is routed.
    pub fn take_pending_copy(&mut self) -> Option<String> {
        self.pending_copy.take()
    }

    /// Plain text of the current frame selection, materialized from the
    /// final frame's visible rows, or `None` when no frame selection is
    /// active. The selection is validated against the frame map first, so
    /// the copy always matches the painted highlight; returns a clone and
    /// keeps the selection (a stale selection is cleared instead of copied).
    #[must_use]
    pub fn frame_selection_text(&mut self) -> Option<String> {
        self.frame_selection.validate_against(&self.frame_map);
        self.frame_map.materialize(&self.frame_selection)
    }

    fn clear_widget_selections(&mut self) {
        self.chrome.prompt_mut().clear_selection();
        self.frame_selection.clear();
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
        // The overlay renders into the height left over by the footer: rich
        // dialogs slice themselves to this budget, so `fit_chrome_to_height`
        // below stays a defensive backstop instead of silently dropping the
        // dialog's title from the top.
        let footer = render_footer_only_lines(app, width);
        let mut overlay = app
            .render_focused_overlay(content_width, height.saturating_sub(footer.len()))
            .unwrap_or_default();
        let cursor = extract_cursor(&mut overlay);
        let rows: Vec<String> = overlay.into_iter().chain(footer).collect();
        let row_count = rows.len();
        ChromeRender {
            lines: rows,
            cursor,
            prompt_start_row: 0,
            row_kinds: vec![ChromeRowKind::Other; row_count],
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
    chrome.row_kinds.drain(..removed);
    debug_assert_eq!(chrome.lines.len(), chrome.row_kinds.len());
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
