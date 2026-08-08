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

use crate::primitive::{Color, strip_ansi};
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
    /// kept; source tabs are never reconstructed. Returns `None` when the
    /// selection is empty or covers nothing visible.
    #[must_use]
    pub fn materialize(&self, selection: &FrameSelection) -> Option<String> {
        let (min_row, max_row) = selection.row_range()?;
        let mut slices = Vec::with_capacity(max_row - min_row + 1);
        for row in min_row..=max_row {
            let visible = self.rows.get(row)?;
            let (start, end) = selection.cell_span(row);
            slices.push(slice_text_by_cells(&visible.text, start, end));
        }
        let text = slices.join("\n");
        (!text.is_empty()).then_some(text)
    }
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
    /// every grapheme whole. The selection was validated against the frame
    /// before painting, so the endpoints are within the current map.
    pub fn paint_into(&self, lines: &mut [String], bg: Color) {
        let Some((min_row, max_row)) = self.row_range() else {
            return;
        };
        for row in min_row..=max_row {
            let Some(line) = lines.get_mut(row) else {
                continue;
            };
            let (start, end) = self.cell_span(row);
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
