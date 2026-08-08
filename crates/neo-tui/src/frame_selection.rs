//! Final-frame mouse text selection over chrome and overlay surfaces.
//!
//! The transcript body keeps its document-coordinate selection; every other
//! row of the final rendered frame — todo panel, footer, rich dialogs, and
//! full-screen overlays (task browser, theme manager) — is selectable through
//! this module. Endpoints are screen coordinates — `(row, display cell)` over
//! the final gutter-processed frame lines — and copy materializes from the
//! frame's [`FrameTextMap`], so only what the terminal actually displayed can
//! ever be copied: masked inputs copy their mask, never the stored secret.
//!
//! The surface identity comes straight from
//! [`NeoChromeState::focused_overlay_id`](crate::shell::NeoChromeState::focused_overlay_id):
//! `None` is the main frame, `Some(id)` the focused overlay that owns the
//! frame. There is no separate overlay registry.
//!
//! Long-press activation is driven by the existing frame cadence, mirroring
//! [`DocumentSelection`](crate::transcript::DocumentSelection): a pending
//! press requests animation ([`FrameSelection::requests_animation`]) and
//! [`FrameSelection::tick`] activates it once [`LONG_PRESS_DELAY`] elapses,
//! with no timers or input threads of its own.

use std::time::Instant;

use crate::primitive::{Color, strip_ansi, visible_width};
use crate::shell::OverlayId;
use crate::transcript::{
    ChromeRowKind, LONG_PRESS_DELAY, MOVEMENT_THRESHOLD, paint_selection_range, slice_text_by_cells,
};

/// Surface identity of a rendered frame: `None` is the main frame (no focused
/// overlay), `Some(id)` the focused overlay that owns the frame.
pub type SurfaceId = Option<OverlayId>;

/// Classification of one final-frame row, mirroring the per-widget mouse
/// routing in the frame map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRowKind {
    /// Transcript body row (main frame only).
    Transcript,
    /// Prompt input content row.
    Prompt,
    /// Any other chrome row or a full-screen overlay row.
    Frame,
}

/// One row of the final frame: plain visible text plus its classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRow {
    kind: FrameRowKind,
    text: String,
}

/// The final rendered frame as plain visible rows, recorded after the gutter
/// and cursor extraction and before selection painting. Rows keep the gutter
/// cells, so endpoint cells are full-line display cells (the raw screen
/// column). ANSI and terminal protocol bytes never enter the map.
#[derive(Debug, Clone, Default)]
pub struct FrameTextMap {
    width: usize,
    height: usize,
    surface: SurfaceId,
    rows: Vec<FrameRow>,
}

impl FrameTextMap {
    /// Replace the map with the final rendered frame. `body_rows` and
    /// `row_kinds` classify each row: transcript body rows become
    /// [`FrameRowKind::Transcript`], prompt content rows
    /// [`FrameRowKind::Prompt`], and everything else (including every row of
    /// a full-screen overlay) [`FrameRowKind::Frame`].
    pub fn record(
        &mut self,
        width: usize,
        height: usize,
        surface: SurfaceId,
        lines: &[String],
        body_rows: usize,
        row_kinds: &[ChromeRowKind],
    ) {
        self.width = width;
        self.height = height;
        self.surface = surface;
        self.rows = lines
            .iter()
            .enumerate()
            .map(|(row, line)| {
                let kind = if row < body_rows {
                    FrameRowKind::Transcript
                } else {
                    match row_kinds.get(row - body_rows) {
                        Some(ChromeRowKind::Prompt) => FrameRowKind::Prompt,
                        _ => FrameRowKind::Frame,
                    }
                };
                FrameRow {
                    kind,
                    text: strip_ansi(line),
                }
            })
            .collect();
    }

    /// Plain text of `selection`'s display-cell range, sliced from the map's
    /// visible rows. Visible newlines and blank rows inside the range are
    /// kept; source tabs are never reconstructed. Rows are purified before
    /// slicing: prompt rows and decoration rows — rows with Box Drawing
    /// characters whose [`content_span`] covers no content cell — contribute
    /// no line, while blank rows (empty or space-only, no Box Drawing) keep
    /// their newline, and each remaining row contributes only its
    /// intersection with the selection. Returns `None` when the selection is
    /// empty or covers nothing visible.
    #[must_use]
    pub fn materialize(&self, selection: &FrameSelection) -> Option<String> {
        let (min_row, max_row) = selection.row_range()?;
        let mut slices = Vec::with_capacity(max_row - min_row + 1);
        for row in min_row..=max_row {
            let visible = self.rows.get(row)?;
            if visible.kind == FrameRowKind::Prompt {
                continue;
            }
            let (sel_start, sel_end) = selection.cell_span(row);
            if !has_box_drawing(&visible.text) {
                // Blank or plain rows (no border characters) keep their
                // line: an empty row slices to an empty line that still
                // joins with a newline.
                slices.push(slice_text_by_cells(&visible.text, sel_start, sel_end));
                continue;
            }
            // Decorated rows contribute only their content cells inside the
            // selection; a row whose content span is empty is pure border
            // decoration and is dropped without leaving a blank line.
            let (span_start, span_end) = content_span(&visible.text);
            if span_start >= span_end {
                continue;
            }
            let start = span_start.max(sel_start);
            let end = span_end.min(sel_end);
            if start >= end {
                continue;
            }
            slices.push(slice_text_by_cells(&visible.text, start, end));
        }
        let text = slices.join("\n");
        (!text.is_empty()).then_some(text)
    }

    /// Classification of one final-frame row, or `None` when the row is
    /// outside the recorded frame.
    #[must_use]
    pub(crate) fn row_kind(&self, row: usize) -> Option<FrameRowKind> {
        self.rows.get(row).map(|row| row.kind)
    }

    /// Plain visible text of one final-frame row (gutter cell included), or
    /// `None` when the row is outside the recorded frame.
    #[must_use]
    pub(crate) fn plain_row(&self, row: usize) -> Option<&str> {
        self.rows.get(row).map(|row| row.text.as_str())
    }
}

/// True for Box Drawing block characters (U+2500..=U+257F): frame borders,
/// separators, and corner glyphs. All of them occupy exactly one display
/// cell.
fn is_box_drawing(ch: char) -> bool {
    ('\u{2500}'..='\u{257F}').contains(&ch)
}

/// Whether a row contains any Box Drawing character — the marker that
/// distinguishes a decoration row (border content with an empty
/// [`content_span`], dropped from copies) from a blank row (empty or
/// space-only, which keeps its newline).
fn has_box_drawing(text: &str) -> bool {
    text.chars().any(is_box_drawing)
}

/// 行内容单元格区间（显示单元格）：剥离行首连续 Box Drawing 字符及其后
/// 紧邻空格、行尾连续 Box Drawing 字符及其前紧邻空格。无边框的行（如缩进
/// 行）原样保留（缩进是内容）。返回 `(start_cell, end_cell)`（end 不含，
/// 均为显示单元格，直接供 `slice_text_by_cells`/`paint_selection_range`
/// 使用）。
///
/// The recorded rows keep the leading gutter cell, so the leading border run
/// is detected after the leading spaces; when no run follows, indentation
/// stays content. A pure border row (`────`) collapses to an empty span and
/// is dropped by materialization, while blank rows (empty or space-only,
/// without Box Drawing characters) keep their newline there, and a mid-line
/// column separator (`│ text │ │ other │`) survives inside the content.
///
/// The stripped runs consist only of spaces and box-drawing characters —
/// every one a single-width cell — so the walks accumulate exactly one cell
/// per consumed character and both returned boundaries land on whole
/// character edges: a wide content character next to a border is never
/// split, keeping its cells fully inside or outside the span.
#[must_use]
fn content_span(text: &str) -> (usize, usize) {
    let total_width = visible_width(text);
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    // Leading run: spaces (the gutter), then consecutive box-drawing
    // characters, then the spaces right after the run.
    let mut index = 0;
    let mut width = 0usize;
    let mut saw_border_run = false;
    while index < len && chars[index] == ' ' {
        index += 1;
        width += 1;
    }
    let start = if index < len && is_box_drawing(chars[index]) {
        saw_border_run = true;
        while index < len && is_box_drawing(chars[index]) {
            index += 1;
            width += 1;
        }
        while index < len && chars[index] == ' ' {
            index += 1;
            width += 1;
        }
        width
    } else {
        0
    };

    // The leading walk consumed the whole line through a border run: the
    // row is pure decoration — a border run plus padding, e.g. `" ──── "`
    // or `" ┌────┐ "` — with no content character at all, so collapse it
    // to an empty span instead of treating the padding as content.
    // Space-only rows never take this branch and keep their full span
    // (blank-line semantics).
    if saw_border_run && index == len {
        return (start, start);
    }

    // Trailing run: consecutive box-drawing characters at the line end plus
    // the spaces right before them, counted back from the total width.
    let mut end = total_width;
    index = len;
    while index > 0 && is_box_drawing(chars[index - 1]) {
        index -= 1;
        end -= 1;
    }
    if index < len {
        while index > 0 && chars[index - 1] == ' ' {
            index -= 1;
            end -= 1;
        }
    }

    (start, end)
}

/// Screen-coordinate selection over the final frame: endpoints, the
/// click-versus-drag movement threshold, frame-driven long-press activation,
/// and the selected-row snapshot used to invalidate the selection when the
/// visual state it sits on changes.
#[derive(Debug, Clone)]
pub struct FrameSelection {
    /// Surface the selection was confirmed on (captured at release).
    surface: SurfaceId,
    /// Frame size the selection was confirmed on.
    width: usize,
    height: usize,
    anchor: Option<(usize, usize)>,
    active: Option<(usize, usize)>,
    /// Threshold crossed: the gesture is a drag, not a click.
    dragging: bool,
    /// Whether the mouse gesture between a press and its release is still
    /// open. Any-event terminals keep reporting hover motion (parsed as
    /// drags) after the release; without this flag that motion would keep
    /// extending the selection after the button is up.
    gesture_active: bool,
    /// Press point of a not-yet-activated mouse gesture. A press is tentative
    /// until deliberate movement crosses [`MOVEMENT_THRESHOLD`] or the press
    /// is held past [`LONG_PRESS_DELAY`] (frame-driven via [`Self::tick`]),
    /// so plain clicks never keep a selection.
    pending_point: Option<(usize, usize)>,
    /// Wall clock of the pending press, for long-press activation.
    press_at: Instant,
    /// Body position of the press, for the click/drag movement threshold.
    press_row: usize,
    press_col: usize,
    /// Plain text of the selected rows at release — the snapshot that clears
    /// the selection when the selected visual state changes.
    selected_rows: Vec<String>,
}

impl Default for FrameSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameSelection {
    #[must_use]
    pub fn new() -> Self {
        Self {
            surface: None,
            width: 0,
            height: 0,
            anchor: None,
            active: None,
            dragging: false,
            gesture_active: false,
            pending_point: None,
            press_at: Instant::now(),
            press_row: 0,
            press_col: 0,
            selected_rows: Vec::new(),
        }
    }

    /// Whether a confirmed selection exists (a drag or a long-press).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.anchor.is_some() && self.active.is_some()
    }

    /// Whether a mouse gesture between a press and its release is still open.
    #[must_use]
    pub const fn is_gesture_open(&self) -> bool {
        self.gesture_active
    }

    /// Whether the frame loop must keep rendering: a pending press needs
    /// frames to drive long-press activation.
    #[must_use]
    pub const fn requests_animation(&self) -> bool {
        self.pending_point.is_some()
    }

    /// Normalized `(min_row, max_row)` of the confirmed selection.
    #[must_use]
    pub fn row_range(&self) -> Option<(usize, usize)> {
        let (anchor_row, _) = self.anchor?;
        let (active_row, _) = self.active?;
        Some(if anchor_row < active_row {
            (anchor_row, active_row)
        } else {
            (active_row, anchor_row)
        })
    }

    /// Whether `row` falls inside the confirmed selection's row range.
    #[must_use]
    pub fn contains_row(&self, row: usize) -> bool {
        self.row_range()
            .is_some_and(|(min_row, max_row)| row >= min_row && row <= max_row)
    }

    /// Display-cell span of `row` inside the selection: the min row is cut by
    /// its endpoint cell, the max row by the other endpoint cell, and rows
    /// between stay whole.
    #[must_use]
    pub fn cell_span(&self, row: usize) -> (usize, usize) {
        let Some((anchor_row, anchor_cell)) = self.anchor else {
            return (0, 0);
        };
        let Some((active_row, active_cell)) = self.active else {
            return (0, 0);
        };
        let (min_row, max_row) = if anchor_row < active_row {
            (anchor_row, active_row)
        } else {
            (active_row, anchor_row)
        };
        if row < min_row || row > max_row {
            return (0, 0);
        }
        match anchor_row.cmp(&active_row) {
            std::cmp::Ordering::Equal => {
                let start = anchor_cell.min(active_cell);
                let end = anchor_cell.max(active_cell).saturating_add(1);
                (start, end)
            }
            std::cmp::Ordering::Less => {
                if row == min_row {
                    (anchor_cell, usize::MAX)
                } else if row == max_row {
                    (0, active_cell.saturating_add(1))
                } else {
                    (0, usize::MAX)
                }
            }
            std::cmp::Ordering::Greater => {
                if row == min_row {
                    (active_cell, usize::MAX)
                } else if row == max_row {
                    (0, anchor_cell.saturating_add(1))
                } else {
                    (0, usize::MAX)
                }
            }
        }
    }

    /// Start a tentative press at `(row, cell)`. A press never confirms a
    /// selection by itself: endpoints appear only when movement crosses
    /// [`MOVEMENT_THRESHOLD`] or the long-press delay elapses, so plain clicks
    /// stay inert. Any prior frame selection is replaced by the new gesture.
    pub fn press(&mut self, row: usize, cell: usize, now: Instant) {
        self.anchor = None;
        self.active = None;
        self.pending_point = Some((row, cell));
        self.press_at = now;
        self.dragging = false;
        self.gesture_active = true;
        self.selected_rows = Vec::new();
        self.press_row = row;
        self.press_col = cell;
    }

    /// Frame-driven long-press activation: a press held still past
    /// [`LONG_PRESS_DELAY`] becomes a selection anchored at the press point.
    /// Called once per rendered frame; the frame loop keeps running while a
    /// press is pending ([`Self::requests_animation`]).
    pub fn tick(&mut self, now: Instant) {
        if self.pending_point.is_some()
            && now.saturating_duration_since(self.press_at) >= LONG_PRESS_DELAY
        {
            self.activate(None);
        }
    }

    /// Confirm the pending press as a drag selection. The anchor is the press
    /// point; the active endpoint is `point` when the confirmation came from
    /// movement, or the anchor itself for long-press activation.
    fn activate(&mut self, point: Option<(usize, usize)>) {
        let Some(anchor) = self.pending_point else {
            return;
        };
        self.pending_point = None;
        self.anchor = Some(anchor);
        self.active = Some(point.unwrap_or(anchor));
        self.dragging = true;
    }

    /// Feed one drag-motion event: movement past [`MOVEMENT_THRESHOLD`]
    /// confirms the pending press as a drag; once confirmed, the active
    /// endpoint follows the pointer. Motion outside an open gesture is inert.
    pub fn drag(&mut self, row: usize, cell: usize) {
        if !self.gesture_active {
            return;
        }
        if self.pending_point.is_some() {
            if row.abs_diff(self.press_row) > usize::from(MOVEMENT_THRESHOLD)
                || cell.abs_diff(self.press_col) > usize::from(MOVEMENT_THRESHOLD)
            {
                self.activate(Some((row, cell)));
            }
        } else if self.dragging {
            self.active = Some((row, cell));
        }
    }

    /// End the gesture. A real drag (or a long-press) keeps the selection and
    /// snapshots its rows from `map`; a plain click collapses the selection
    /// so single clicks stay inert. Releases outside an open gesture are
    /// no-ops, so a standing selection survives unrelated pointer events.
    pub fn release(&mut self, map: &FrameTextMap) {
        if !self.gesture_active {
            return;
        }
        let keep = self.dragging;
        self.dragging = false;
        self.gesture_active = false;
        self.pending_point = None;
        if keep {
            self.capture_snapshot(map);
        } else {
            self.anchor = None;
            self.active = None;
            self.selected_rows = Vec::new();
        }
    }

    /// Freeze the map rows under the confirmed selection: the surface, the
    /// frame size, and the plain text of every selected row.
    fn capture_snapshot(&mut self, map: &FrameTextMap) {
        let Some((min_row, max_row)) = self.row_range() else {
            return;
        };
        self.surface = map.surface;
        self.width = map.width;
        self.height = map.height;
        self.selected_rows = (min_row..=max_row)
            .filter_map(|row| map.rows.get(row).map(|visible| visible.text.clone()))
            .collect();
    }

    /// Drop the selection when the frame it sits on changed underneath it: a
    /// different surface, a different terminal size, or changed content on
    /// any selected row. Unselected rows never invalidate it. An open gesture
    /// is exempt — its endpoints follow the pointer instead.
    ///
    /// The snapshot compares the plain visible rows (ANSI stripped) rather
    /// than the raw escape sequences: the visible text is the rendering
    /// identity the selection covers, so a style-only change keeps the
    /// selection while any text or cell-mapping change clears it.
    pub fn validate_against(&mut self, map: &FrameTextMap) {
        if !self.is_active() || self.is_gesture_open() {
            return;
        }
        let Some((min_row, _)) = self.row_range() else {
            return;
        };
        let rows_match = self.selected_rows.iter().enumerate().all(|(offset, text)| {
            map.rows
                .get(min_row + offset)
                .is_some_and(|row| row.text == *text)
        });
        if map.width != self.width
            || map.height != self.height
            || map.surface != self.surface
            || !rows_match
        {
            self.clear();
        }
    }

    /// Paint the selection background over the final frame lines, keeping
    /// every grapheme whole and never painting decoration or prompt rows:
    /// a decorated row highlights only the intersection of its selection
    /// span with its [`content_span`], while blank rows (empty or
    /// space-only, no Box Drawing) have nothing to highlight, so the
    /// highlight and the materialized copy stay identical. The selection
    /// was validated against the frame before painting, so the endpoints
    /// are within the current map.
    pub fn paint_into(&self, lines: &mut [String], map: &FrameTextMap, bg: Color) {
        let Some((min_row, max_row)) = self.row_range() else {
            return;
        };
        for row in min_row..=max_row {
            let Some(line) = lines.get_mut(row) else {
                continue;
            };
            if map.row_kind(row) == Some(FrameRowKind::Prompt) {
                continue;
            }
            let Some(plain) = map.plain_row(row) else {
                continue;
            };
            let (sel_start, sel_end) = self.cell_span(row);
            if !has_box_drawing(plain) {
                // Blank or plain rows: the selection span is painted
                // directly — an empty row has no cells to paint.
                *line = paint_selection_range(line, sel_start, sel_end, bg);
                continue;
            }
            let (span_start, span_end) = content_span(plain);
            if span_start >= span_end {
                continue;
            }
            let start = span_start.max(sel_start);
            let end = span_end.min(sel_end);
            if start >= end {
                continue;
            }
            *line = paint_selection_range(line, start, end, bg);
        }
    }

    /// Forget the selection entirely (endpoints, gesture, snapshot).
    pub fn clear(&mut self) {
        self.surface = None;
        self.width = 0;
        self.height = 0;
        self.anchor = None;
        self.active = None;
        self.dragging = false;
        self.gesture_active = false;
        self.pending_point = None;
        self.selected_rows = Vec::new();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crate::transcript::ChromeRowKind;

    use super::{FrameSelection, FrameTextMap, content_span};

    #[test]
    fn content_span_collapses_border_rows_and_keeps_blank_rows() {
        // Pure decoration rows — a border run plus trailing padding —
        // collapse to an empty span even when the line ends in spaces.
        assert_eq!(content_span(" ──── "), (6, 6));
        assert_eq!(content_span(" ┌────┐ "), (8, 8));
        // A space-only row keeps its full span: blank lines stay blank.
        assert_eq!(content_span("   "), (0, 3));
        // Indented content keeps its indentation; bordered content strips
        // the leading and trailing border plus the adjacent spaces, with
        // wide characters measured in display cells.
        assert_eq!(content_span("   ○ item"), (0, 9));
        assert_eq!(content_span(" │ 中文任务 │"), (3, 11));
    }

    #[test]
    fn materialize_keeps_blank_rows_and_drops_decoration_rows() {
        let mut map = FrameTextMap::default();
        map.record(
            12,
            5,
            None,
            &[
                " alpha".to_owned(),
                String::new(),
                " ──── ".to_owned(),
                " │ text │".to_owned(),
                String::new(),
            ],
            0,
            &[ChromeRowKind::Other; 5],
        );
        let mut selection = FrameSelection::new();
        selection.press(0, 0, Instant::now());
        selection.drag(4, 9);
        selection.release(&map);
        assert_eq!(
            map.materialize(&selection).as_deref(),
            Some(" alpha\n\ntext\n"),
            "blank rows keep their newline; the pure border row is dropped"
        );
    }
}
