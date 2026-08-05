//! Document-coordinate text selection for the fullscreen transcript.
//!
//! Selection endpoints are document coordinates — `(entry id, rendered row
//! within the entry, display cell)` — rather than terminal screen rows, so a
//! drag may cross entry and card boundaries without re-mapping through the
//! visible slice. The pane resolves screen positions into [`DocumentPoint`]
//! values through [`super::document::DocumentLayout`]; this module owns the
//! endpoint state, the click-versus-drag movement threshold, double-click
//! word selection, the drag auto-scroll intent, and the materialized plain
//! text captured at mouse release.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton};

use super::store::TranscriptEntryId;
use crate::primitive::text_layout::display_width;
use unicode_segmentation::UnicodeSegmentation;

/// The number of rows one wheel notch scrolls. Historical SGR wheel events
/// mapped to three transcript rows; the typed event keeps that cadence.
pub const WHEEL_SCROLL_ROWS: usize = 3;

/// Movement (in display cells or rows) that separates a click from a
/// selection drag, so single clicks keep their control semantics.
pub const MOVEMENT_THRESHOLD: u16 = 2;

/// Maximum time between two presses that still form a double-click.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// Maximum endpoint distance (rows or cells) between the two presses of a
/// double-click.
const DOUBLE_CLICK_DISTANCE: u16 = 2;

/// One endpoint of a transcript selection, in document coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentPoint {
    pub entry_id: TranscriptEntryId,
    pub row_in_entry: usize,
    pub display_cell: usize,
}

/// The semantic kind of a mouse event delivered to the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Drag,
    Release,
    ScrollUp,
    ScrollDown,
}

/// A typed mouse event with zero-based coordinates and decoded modifiers.
///
/// One-based SGR wire coordinates are converted to zero-based exactly once,
/// in the raw input parser, so every consumer shares the same coordinate
/// space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub button: MouseButton,
    pub column: u16,
    pub row: u16,
    pub modifiers: KeyModifiers,
}

impl MouseEvent {
    #[must_use]
    pub const fn is_wheel(&self) -> bool {
        matches!(self.kind, MouseKind::ScrollUp | MouseKind::ScrollDown)
    }

    /// Whether the event is transient pointer motion (drag or wheel) that a
    /// stale queue may safely coalesce or drop.
    #[must_use]
    pub const fn is_motion(&self) -> bool {
        matches!(
            self.kind,
            MouseKind::Drag | MouseKind::ScrollUp | MouseKind::ScrollDown
        )
    }

    #[must_use]
    pub const fn is_wheel_up(&self) -> bool {
        matches!(self.kind, MouseKind::ScrollUp)
    }

    #[must_use]
    pub const fn is_shift_modified(&self) -> bool {
        self.modifiers.contains(KeyModifiers::SHIFT)
    }

    /// Whether the event selects transcript text (left-button press, drag,
    /// or release). Wheel and other buttons never select.
    #[must_use]
    pub const fn is_selection_event(&self) -> bool {
        matches!(
            self.kind,
            MouseKind::Press | MouseKind::Drag | MouseKind::Release
        )
    }
}

/// Direction of drag auto-scroll while the pointer crosses the viewport edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoScroll {
    Up,
    Down,
}

/// Outcome of feeding one drag-motion event to a [`DocumentSelection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragUpdate {
    /// Whether the movement threshold was crossed by this event.
    pub started: bool,
    /// Whether the selection is currently a drag (threshold crossed and the
    /// press anchor is still held).
    pub dragging: bool,
}

/// Document-coordinate selection state: endpoints, drag lifecycle, word
/// selection, auto-scroll intent, and materialized plain text.
///
/// The pane owns the document layout and rendered row text; this state is a
/// pure endpoint machine plus text utilities. No rendering, no store access.
#[derive(Debug, Clone)]
pub struct DocumentSelection {
    anchor: Option<DocumentPoint>,
    active: Option<DocumentPoint>,
    /// Threshold crossed: the gesture is a drag, not a click.
    dragging: bool,
    /// A double-click established a word selection on the current press;
    /// its release must keep the selection instead of clearing it.
    word_selected: bool,
    /// Body position of the press, for the click/drag movement threshold.
    press_row: u16,
    press_col: u16,
    /// Previous press, for double-click detection.
    last_press_at: Option<Instant>,
    last_press_point: Option<DocumentPoint>,
    /// Active drag auto-scroll request, driven by the frame cadence.
    auto_scroll: Option<AutoScroll>,
    /// Plain text materialized at release; frozen against later updates.
    materialized: Option<String>,
}

impl Default for DocumentSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentSelection {
    #[must_use]
    pub fn new() -> Self {
        Self {
            anchor: None,
            active: None,
            dragging: false,
            word_selected: false,
            press_row: 0,
            press_col: 0,
            last_press_at: None,
            last_press_point: None,
            auto_scroll: None,
            materialized: None,
        }
    }

    #[must_use]
    pub const fn anchor(&self) -> Option<DocumentPoint> {
        self.anchor
    }

    #[must_use]
    pub const fn active(&self) -> Option<DocumentPoint> {
        self.active
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.anchor.is_some() && self.active.is_some()
    }

    #[must_use]
    pub const fn has_anchor(&self) -> bool {
        self.anchor.is_some()
    }

    #[must_use]
    pub const fn is_dragging(&self) -> bool {
        self.dragging
    }

    #[must_use]
    pub const fn auto_scroll(&self) -> Option<AutoScroll> {
        self.auto_scroll
    }

    #[must_use]
    pub fn materialized(&self) -> Option<&str> {
        self.materialized.as_deref()
    }

    /// Whether the frame loop must keep rendering so drag auto-scroll can
    /// advance the document at the existing animation cadence.
    #[must_use]
    pub const fn requests_animation(&self) -> bool {
        self.dragging && self.auto_scroll.is_some()
    }

    /// Start (or restart) a selection at `point`. Returns `true` when this
    /// press forms a double-click with the previous press, in which case the
    /// caller should replace the endpoints with the word under the point.
    pub fn press(&mut self, point: DocumentPoint, row: u16, col: u16, now: Instant) -> bool {
        let double_click = self
            .last_press_at
            .is_some_and(|at| now.saturating_duration_since(at) <= DOUBLE_CLICK_WINDOW)
            && self.last_press_point.is_some_and(|previous| {
                previous.entry_id == point.entry_id
                    && previous.row_in_entry.abs_diff(point.row_in_entry)
                        <= usize::from(DOUBLE_CLICK_DISTANCE)
                    && previous.display_cell.abs_diff(point.display_cell)
                        <= usize::from(DOUBLE_CLICK_DISTANCE)
            });
        self.anchor = Some(point);
        self.active = Some(point);
        self.dragging = false;
        self.word_selected = false;
        self.auto_scroll = None;
        self.materialized = None;
        self.press_row = row;
        self.press_col = col;
        self.last_press_at = Some(now);
        self.last_press_point = Some(point);
        double_click
    }

    /// Feed one drag-motion event. `point` is the resolved document point,
    /// or `None` when the pointer is outside the body. The active endpoint
    /// moves only after the movement threshold distinguishes a drag.
    pub fn drag(&mut self, point: Option<DocumentPoint>, row: u16, col: u16) -> DragUpdate {
        let was_dragging = self.dragging;
        if self.anchor.is_some()
            && !self.dragging
            && (row.abs_diff(self.press_row) > MOVEMENT_THRESHOLD
                || col.abs_diff(self.press_col) > MOVEMENT_THRESHOLD)
        {
            self.dragging = true;
        }
        if self.dragging
            && let Some(point) = point
        {
            self.active = Some(point);
        }
        DragUpdate {
            started: !was_dragging && self.dragging,
            dragging: self.dragging,
        }
    }

    /// End the gesture. Returns `true` when a real selection exists (a drag
    /// or a double-click word selection) and should be materialized; a plain
    /// click clears the selection so single clicks stay inert.
    pub fn release(&mut self) -> bool {
        let keep = self.dragging || self.word_selected;
        self.dragging = false;
        self.word_selected = false;
        self.auto_scroll = None;
        if !keep {
            self.anchor = None;
            self.active = None;
            self.materialized = None;
        }
        keep
    }

    /// Replace both endpoints with a double-click word selection.
    pub fn set_word_selection(&mut self, start: DocumentPoint, end: DocumentPoint) {
        self.anchor = Some(start);
        self.active = Some(end);
        self.word_selected = true;
        self.dragging = false;
        self.auto_scroll = None;
    }

    /// Start a keyboard-driven selection at `point` (no mouse gesture).
    pub fn start_keyboard_selection(&mut self, point: DocumentPoint) {
        self.anchor = Some(point);
        self.active = Some(point);
        self.dragging = false;
        self.word_selected = false;
        self.auto_scroll = None;
        self.materialized = None;
    }

    /// Move the active endpoint (keyboard extension). The anchor stays fixed.
    pub fn extend_to(&mut self, point: DocumentPoint) {
        self.active = Some(point);
    }

    pub fn set_auto_scroll(&mut self, direction: Option<AutoScroll>) {
        self.auto_scroll = direction;
    }

    /// Freeze the plain text materialized at mouse release. Later document
    /// updates never change it.
    pub fn set_materialized(&mut self, text: String) {
        self.materialized = Some(text);
    }

    /// Forget the materialized text so the next copy re-materializes.
    pub fn invalidate_materialized(&mut self) {
        self.materialized = None;
    }

    /// Clear every selection detail (endpoints, drag, word, auto-scroll,
    /// materialized text). Double-click history is retained so a fresh press
    /// can still form a double-click.
    pub fn clear(&mut self) {
        self.anchor = None;
        self.active = None;
        self.dragging = false;
        self.word_selected = false;
        self.auto_scroll = None;
        self.materialized = None;
    }
}

/// Slice plain text by display cells: keep every grapheme whose cell span
/// intersects `[start_cell, end_cell)`. Wide characters are kept whole.
#[must_use]
pub fn slice_text_by_cells(text: &str, start_cell: usize, end_cell: usize) -> String {
    if start_cell >= end_cell {
        return String::new();
    }
    let mut sliced = String::new();
    let mut width = 0usize;
    for grapheme in text.graphemes(true) {
        let grapheme_width = display_width(grapheme);
        let span_end = width + grapheme_width;
        if span_end <= start_cell {
            width = span_end;
            continue;
        }
        if width >= end_cell {
            break;
        }
        sliced.push_str(grapheme);
        width = span_end;
    }
    sliced
}

/// The grapheme index whose cell span contains `cell` (clamped to the end
/// of the text). Returns the grapheme count when the text is exhausted.
#[must_use]
pub fn cell_to_grapheme_index(text: &str, cell: usize) -> usize {
    let mut width = 0usize;
    for (index, grapheme) in text.graphemes(true).enumerate() {
        let grapheme_width = display_width(grapheme);
        if cell < width + grapheme_width {
            return index;
        }
        width += grapheme_width;
    }
    text.graphemes(true).count()
}

/// The display cell where the grapheme at `grapheme_index` starts.
#[must_use]
pub fn grapheme_index_to_cell(text: &str, grapheme_index: usize) -> usize {
    text.graphemes(true)
        .take(grapheme_index)
        .map(display_width)
        .sum()
}

/// The inclusive grapheme range of the Unicode word containing the grapheme
/// at `grapheme_index`. A grapheme that is not part of a word (whitespace,
/// punctuation) selects itself alone.
#[must_use]
pub fn word_span_in_text(text: &str, grapheme_index: usize) -> (usize, usize) {
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    if graphemes.is_empty() {
        return (0, 0);
    }
    let index = grapheme_index.min(graphemes.len() - 1);
    let is_word = |grapheme: &&str| {
        grapheme
            .chars()
            .any(|character| character.is_alphanumeric() || character == '_')
    };
    if !is_word(&graphemes[index]) {
        return (index, index + 1);
    }
    let mut start = index;
    while start > 0 && is_word(&graphemes[start - 1]) {
        start -= 1;
    }
    let mut end = index + 1;
    while end < graphemes.len() && is_word(&graphemes[end]) {
        end += 1;
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_text_by_cells_keeps_wide_characters_whole() {
        assert_eq!(slice_text_by_cells("ab", 0, 2), "ab");
        assert_eq!(slice_text_by_cells("ab", 1, 2), "b");
        assert_eq!(slice_text_by_cells("你a", 0, 2), "你");
        // Clicking inside a wide character's second cell keeps the whole char.
        assert_eq!(slice_text_by_cells("你a", 1, 3), "你a");
        assert_eq!(slice_text_by_cells("你a", 2, 3), "a");
        assert_eq!(slice_text_by_cells("你a", 3, 4), "");
    }

    #[test]
    fn cell_to_grapheme_index_maps_wide_characters() {
        assert_eq!(cell_to_grapheme_index("ab", 0), 0);
        assert_eq!(cell_to_grapheme_index("ab", 1), 1);
        assert_eq!(cell_to_grapheme_index("ab", 5), 2);
        assert_eq!(cell_to_grapheme_index("你a", 0), 0);
        assert_eq!(cell_to_grapheme_index("你a", 1), 0);
        assert_eq!(cell_to_grapheme_index("你a", 2), 1);
        assert_eq!(grapheme_index_to_cell("你a", 1), 2);
    }

    #[test]
    fn word_span_selects_one_unicode_word() {
        let text = "hello wörld";
        let hello = cell_to_grapheme_index(text, 1);
        let world = cell_to_grapheme_index(text, 7);
        let (start, end) = word_span_in_text(text, hello);
        assert_eq!(
            &text.graphemes(true).collect::<Vec<_>>()[start..end],
            ["h", "e", "l", "l", "o"]
        );
        let (start, end) = word_span_in_text(text, world);
        assert_eq!(
            &text.graphemes(true).collect::<Vec<_>>()[start..end],
            ["w", "ö", "r", "l", "d"]
        );
        // Non-word graphemes select themselves.
        let space = cell_to_grapheme_index(text, 5);
        let (start, end) = word_span_in_text(text, space);
        assert_eq!(start + 1, end);
        // CJK: each ideograph is a word character, so the run selects the
        // contiguous ideographs.
        let cjk = "你好世界";
        let (start, end) = word_span_in_text(cjk, 1);
        assert_eq!(start, 0);
        assert_eq!(end, 4);
    }

    #[test]
    fn drag_threshold_separates_clicks_from_drags() {
        let mut selection = DocumentSelection::new();
        let point = DocumentPoint {
            entry_id: TranscriptEntryId::new_for_test(1),
            row_in_entry: 0,
            display_cell: 0,
        };
        let now = Instant::now();
        assert!(!selection.press(point, 2, 3, now));
        // Small movement below the threshold stays a click.
        let update = selection.drag(Some(point), 3, 3);
        assert!(!update.started);
        assert!(!update.dragging);
        // Crossing the threshold starts the drag.
        let update = selection.drag(Some(point), 5, 5);
        assert!(update.started);
        assert!(update.dragging);
        assert!(selection.release());
    }

    #[test]
    fn double_click_requires_window_and_distance() {
        let mut selection = DocumentSelection::new();
        let point = DocumentPoint {
            entry_id: TranscriptEntryId::new_for_test(1),
            row_in_entry: 0,
            display_cell: 0,
        };
        let now = Instant::now();
        assert!(!selection.press(point, 2, 3, now));
        assert!(!selection.release());
        assert!(selection.press(point, 2, 3, now + Duration::from_millis(100)));
        assert!(!selection.release());
        // Outside the double-click window a new press is a single click.
        assert!(!selection.press(point, 2, 3, now + Duration::from_millis(600)));
    }

    #[test]
    fn release_keeps_drag_and_word_selection_but_clears_plain_clicks() {
        let mut selection = DocumentSelection::new();
        let point = DocumentPoint {
            entry_id: TranscriptEntryId::new_for_test(1),
            row_in_entry: 0,
            display_cell: 0,
        };
        let now = Instant::now();
        // Plain click: press + release without threshold movement clears.
        selection.press(point, 2, 3, now);
        assert!(!selection.release());
        assert!(!selection.is_active());
        // A drag keeps the endpoints for materialization.
        selection.press(point, 2, 3, now);
        selection.drag(Some(point), 4, 6);
        assert!(selection.release());
        assert!(selection.is_active());
        // A double-click word selection survives its release.
        selection.set_word_selection(point, point);
        assert!(selection.release());
        assert!(selection.is_active());
    }
}
