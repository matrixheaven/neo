//! Incremental document layout and the logical scroll anchor.
//!
//! The fullscreen transcript is one ordered document. Each entry contributes
//! a rendered block (possibly empty) at a virtual row range; [`DocumentLayout`]
//! owns the per-entry heights, the virtual start rows, and the single logical
//! [`TranscriptAnchor`] that keeps a locked view stable while entries above
//! grow or shrink. The physical terminal only ever receives a bounded visible
//! slice resolved against this document.

use std::collections::{BTreeSet, HashMap};

use super::store::TranscriptEntryId;

/// A logical scroll anchor: entry identity plus the position inside that
/// entry. Unlike an absolute row offset, the anchor survives height changes
/// in entries above it because the entry's virtual start row is recomputed
/// from the per-entry layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptAnchor {
    pub entry_id: TranscriptEntryId,
    pub row_in_entry: usize,
    pub cell_offset: usize,
}

/// Layout record for one entry: its identity, the revision it was laid out
/// at, its virtual start row, and its rendered height in rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryLayout {
    pub entry_id: TranscriptEntryId,
    pub revision: u64,
    pub start_row: usize,
    pub height: usize,
}

/// The document view state: one logical anchor, tail-follow vs. locked, and
/// one Boolean new-activity indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentViewport {
    pub anchor: Option<TranscriptAnchor>,
    pub following_tail: bool,
    pub new_activity: bool,
}

/// Legacy absolute-row viewport geometry, retained for the Ctrl+O review
/// surface until the fullscreen lifecycle replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptViewport {
    scroll_top_rows: usize,
    content_rows: usize,
    viewport_rows: usize,
    follow_tail: bool,
}

impl TranscriptViewport {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            scroll_top_rows: 0,
            content_rows: 0,
            viewport_rows: 0,
            follow_tail: true,
        }
    }

    #[must_use]
    pub fn scrollback(&self) -> usize {
        self.max_scroll_top().saturating_sub(self.scroll_top_rows)
    }

    #[must_use]
    pub const fn is_following_tail(&self) -> bool {
        self.follow_tail
    }

    pub fn follow_bottom(&mut self) {
        self.follow_tail = true;
        self.scroll_top_rows = self.max_scroll_top();
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.follow_tail = false;
        self.scroll_top_rows = self.scroll_top_rows.saturating_sub(rows);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.scroll_top_rows = self
            .scroll_top_rows
            .saturating_add(rows)
            .min(self.max_scroll_top());
        if self.scroll_top_rows == self.max_scroll_top() {
            self.follow_tail = true;
        }
    }

    pub fn sync(&mut self, content_rows: usize, viewport_rows: usize) {
        self.content_rows = content_rows;
        self.viewport_rows = viewport_rows;
        let max = self.max_scroll_top();
        if self.follow_tail {
            self.scroll_top_rows = max;
        } else {
            self.scroll_top_rows = self.scroll_top_rows.min(max);
        }
    }

    #[must_use]
    pub fn visible_row_range(&self, row_count: usize, height: usize) -> std::ops::Range<usize> {
        if height == 0 || row_count == 0 {
            return 0..0;
        }
        let window = height.min(row_count);
        let max_start = row_count.saturating_sub(window);
        let start = self.scroll_top_rows.min(max_start);
        start..start + window
    }

    fn max_scroll_top(&self) -> usize {
        self.content_rows.saturating_sub(self.viewport_rows)
    }
}

impl Default for TranscriptViewport {
    fn default() -> Self {
        Self::new()
    }
}

/// Incremental document layout: per-entry heights, virtual start rows, and
/// the logical scroll anchor.
///
/// The pane feeds the trimmed block height of every entry whose rendered
/// output changed (revision change, live animation, or width/theme rebuild);
/// the layout invalidates only that entry, recomputes its height, and shifts
/// later virtual starts by the height delta.
#[derive(Debug, Clone)]
pub struct DocumentLayout {
    layouts: Vec<EntryLayout>,
    /// Trimmed block rows per entry (parallel to `layouts`), as fed by the
    /// pane. A block of 0 rows means the entry renders nothing.
    block_rows: Vec<usize>,
    /// Entries whose height is stale and must be re-fed before resolution.
    invalid: BTreeSet<usize>,
    /// Set by [`Self::rebuild`]/[`Self::set_width`]; the next
    /// [`Self::sync_entries`] invalidates every entry so the pane re-renders
    /// and re-feeds heights (theme/expansion/policy or width changes do not
    /// bump entry revisions).
    force_rebuild: bool,
    /// Total virtual document height in rows.
    total_rows: usize,
    /// Content width of the last layout pass.
    width: usize,
    /// Physical viewport height of the last resolution.
    viewport_height: usize,
    /// The document view state.
    view: DocumentViewport,
}

impl Default for DocumentLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentLayout {
    #[must_use]
    pub fn new() -> Self {
        Self {
            layouts: Vec::new(),
            block_rows: Vec::new(),
            invalid: BTreeSet::new(),
            force_rebuild: false,
            total_rows: 0,
            width: 0,
            viewport_height: 0,
            view: DocumentViewport {
                anchor: None,
                following_tail: true,
                new_activity: false,
            },
        }
    }

    #[must_use]
    pub const fn view(&self) -> DocumentViewport {
        self.view
    }

    #[must_use]
    pub const fn is_following_tail(&self) -> bool {
        self.view.following_tail
    }

    #[must_use]
    pub const fn total_rows(&self) -> usize {
        self.total_rows
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn layouts(&self) -> &[EntryLayout] {
        &self.layouts
    }

    #[must_use]
    pub fn entry_layout(&self, index: usize) -> Option<&EntryLayout> {
        self.layouts.get(index)
    }

    /// Reconcile the layout with the store's entry identities and revisions.
    ///
    /// New, removed, and revision-changed entries are invalidated; their
    /// heights stay stale until [`Self::set_entry_height`] feeds fresh values.
    /// If the anchored entry disappeared (e.g. a retry removed provisional
    /// content), the anchor falls back to the nearest preceding surviving
    /// entry while remaining locked. While locked, any change sets the single
    /// Boolean new-activity indicator without moving the anchor.
    pub fn sync_entries(&mut self, ids: &[TranscriptEntryId], revisions: &[u64]) {
        // Fast path: no pending rebuild and every entry matches by identity
        // and revision, so nothing can have changed.
        if !self.force_rebuild
            && self.layouts.len() == ids.len()
            && self
                .layouts
                .iter()
                .zip(ids.iter().zip(revisions.iter()))
                .all(|(layout, (&id, &revision))| {
                    layout.entry_id == id && layout.revision == revision
                })
        {
            return;
        }
        let old_layouts = std::mem::take(&mut self.layouts);
        let old_block_rows = std::mem::take(&mut self.block_rows);
        let mut positions = HashMap::with_capacity(old_layouts.len());
        for (index, layout) in old_layouts.iter().enumerate() {
            positions.insert(layout.entry_id, index);
        }
        let anchor = self.view.anchor;
        // Position of the anchored entry in the previous layout, used to find
        // the nearest preceding survivor when the anchor is removed.
        let anchor_position = anchor.and_then(|a| positions.get(&a.entry_id).copied());
        let anchor_removed = anchor.is_some_and(|a| !ids.contains(&a.entry_id));

        let mut changed = false;
        self.invalid.clear();
        for (index, (&id, &revision)) in ids.iter().zip(revisions.iter()).enumerate() {
            if let Some(&old) = positions.get(&id)
                && old_layouts[old].revision == revision
            {
                self.layouts.push(old_layouts[old]);
                self.block_rows.push(old_block_rows[old]);
            } else {
                changed = true;
                self.invalid.insert(index);
                self.layouts.push(EntryLayout {
                    entry_id: id,
                    revision,
                    start_row: 0,
                    height: 0,
                });
                self.block_rows.push(0);
            }
        }
        let any_removed = self.layouts.len() != old_layouts.len();

        if self.force_rebuild {
            self.force_rebuild = false;
            self.invalid = (0..self.layouts.len()).collect();
        }

        if anchor_removed {
            self.fallback_anchor(&old_layouts, anchor_position);
            changed = true;
        } else if let Some(anchor) = &mut self.view.anchor {
            // Clamp the anchor to the entry's current height; start rows are
            // re-resolved when heights are fed.
            if let Some(layout) = self.layouts.iter().find(|l| l.entry_id == anchor.entry_id)
                && layout.height > 0
            {
                anchor.row_in_entry = anchor.row_in_entry.min(layout.height - 1);
            }
        }

        if !self.view.following_tail && (changed || any_removed) {
            self.view.new_activity = true;
        }
    }

    /// Entries whose height is stale and must be re-rendered and re-fed.
    #[must_use]
    pub fn invalid_entries(&self) -> Vec<usize> {
        self.invalid.iter().copied().collect()
    }

    /// Apply a freshly computed trimmed block height for one entry.
    ///
    /// Recomputes that entry's height (block plus one separator row when a
    /// preceding non-empty block exists) and the virtual start rows of every
    /// later entry, then re-resolves the anchor against the new geometry.
    pub fn set_entry_height(&mut self, index: usize, block_rows: usize) {
        if index >= self.layouts.len() {
            return;
        }
        let previous = self.block_rows[index];
        if previous == block_rows && !self.invalid.contains(&index) {
            return;
        }
        self.invalid.remove(&index);
        self.block_rows[index] = block_rows;

        let has_prior = self.layouts[..index].iter().any(|l| l.height > 0);
        let mut row = if index == 0 {
            0
        } else {
            self.layouts[index - 1].start_row + self.layouts[index - 1].height
        };
        let mut prior = has_prior;
        for i in index..self.layouts.len() {
            let block = self.block_rows[i];
            let entry_height = if block > 0 {
                block + usize::from(prior)
            } else {
                0
            };
            self.layouts[i].start_row = row;
            self.layouts[i].height = entry_height;
            row += entry_height;
            if block > 0 {
                prior = true;
            }
        }
        self.total_rows = row;

        // Resolve the anchor against the new geometry.
        let anchored_id = self.view.anchor.map(|a| a.entry_id);
        if let Some(id) = anchored_id
            && id == self.layouts[index].entry_id
        {
            if self.layouts[index].height == 0 {
                // The entry lost all of its rows: fall back to the nearest
                // preceding surviving entry, still locked.
                self.fallback_to_preceding(index);
            } else if let Some(anchor) = &mut self.view.anchor {
                anchor.row_in_entry = anchor.row_in_entry.min(self.layouts[index].height - 1);
            }
        }
    }

    /// Move the view up by `rows` virtual rows, locking the current top
    /// logical point as the anchor.
    pub fn scroll_up(&mut self, rows: usize) {
        if rows == 0 {
            return;
        }
        let viewport = self.viewport_height.max(1);
        let current_top = if self.view.following_tail {
            self.total_rows.saturating_sub(viewport)
        } else {
            self.view.anchor.map_or_else(
                || self.total_rows.saturating_sub(viewport),
                |a| self.anchor_row(a),
            )
        };
        self.set_anchor_at_row(current_top.saturating_sub(rows));
    }

    /// Move the view down by `rows` virtual rows. Reaching the document
    /// bottom resumes tail following.
    pub fn scroll_down(&mut self, rows: usize) {
        if self.view.following_tail || self.total_rows == 0 {
            return;
        }
        let viewport = self.viewport_height.max(1);
        let max_top = self.total_rows.saturating_sub(viewport);
        let current_top = self.view.anchor.map_or(0, |a| self.anchor_row(a));
        let target = current_top
            .saturating_add(rows)
            .min(self.total_rows.saturating_sub(1));
        if target >= max_top {
            self.follow_bottom();
        } else {
            self.set_anchor_at_row(target);
        }
    }

    /// Resolve the view directly to the new document bottom.
    pub fn follow_bottom(&mut self) {
        self.view.following_tail = true;
        self.view.anchor = None;
        self.view.new_activity = false;
    }

    /// Read and clear the one Boolean new-activity indicator.
    #[must_use]
    pub fn consume_new_activity(&mut self) -> bool {
        let had = self.view.new_activity;
        self.view.new_activity = false;
        had
    }

    /// Resolve the visible virtual row range for a physical viewport of
    /// `height` rows, remembering the height for scroll arithmetic.
    pub fn visible_row_range(&mut self, height: usize) -> std::ops::Range<usize> {
        self.viewport_height = height;
        if height == 0 || self.total_rows == 0 {
            return 0..0;
        }
        let start = if self.view.following_tail {
            self.total_rows.saturating_sub(height)
        } else {
            let anchor_row = self.view.anchor.map_or_else(
                || self.total_rows.saturating_sub(height),
                |a| self.anchor_row(a),
            );
            anchor_row.min(self.total_rows.saturating_sub(1))
        };
        let end = (start + height).min(self.total_rows);
        start..end
    }

    /// The entry whose rendered block contains virtual `row`, if any.
    #[must_use]
    pub fn entry_at_row(&self, row: usize) -> Option<usize> {
        self.layouts
            .iter()
            .position(|l| l.start_row <= row && row < l.start_row + l.height)
    }

    /// Rebuild the layout for a new content width. All entries invalidate on
    /// the next sync so the pane re-renders and re-feeds heights against the
    /// new wrapping; the anchor survives and is re-resolved when the heights
    /// land.
    pub fn set_width(&mut self, width: usize) {
        if self.width == width {
            return;
        }
        self.width = width;
        self.force_rebuild = true;
    }

    /// Invalidate every entry on the next sync (theme, expansion, or
    /// image-policy change can change any rendered height). The anchor is
    /// preserved.
    pub fn rebuild(&mut self) {
        self.force_rebuild = true;
    }

    fn set_anchor_at_row(&mut self, row: usize) {
        let row = row.min(self.total_rows.saturating_sub(1));
        let Some(index) = self.entry_at_row(row) else {
            self.view.following_tail = true;
            self.view.anchor = None;
            return;
        };
        let layout = self.layouts[index];
        self.view.anchor = Some(TranscriptAnchor {
            entry_id: layout.entry_id,
            row_in_entry: row - layout.start_row,
            cell_offset: 0,
        });
        self.view.following_tail = false;
    }

    fn anchor_row(&self, anchor: TranscriptAnchor) -> usize {
        let Some(layout) = self.layouts.iter().find(|l| l.entry_id == anchor.entry_id) else {
            return self.total_rows;
        };
        layout
            .start_row
            .saturating_add(anchor.row_in_entry)
            .min(layout.start_row + layout.height.saturating_sub(1))
    }

    /// Fall back to the nearest preceding surviving entry while remaining
    /// locked, anchoring at that entry's last row. `anchor_position` is the
    /// anchored entry's index in the previous layout (before this sync).
    fn fallback_anchor(&mut self, old_layouts: &[EntryLayout], anchor_position: Option<usize>) {
        let Some(position) = anchor_position else {
            self.view.anchor = None;
            return;
        };
        for offset in (0..position).rev() {
            let predecessor_id = old_layouts[offset].entry_id;
            if let Some(layout) = self.layouts.iter().find(|l| l.entry_id == predecessor_id)
                && layout.height > 0
            {
                self.view.anchor = Some(TranscriptAnchor {
                    entry_id: layout.entry_id,
                    row_in_entry: layout.height - 1,
                    cell_offset: self.view.anchor.map_or(0, |a| a.cell_offset),
                });
                return;
            }
        }
        if let Some(first) = self.layouts.first() {
            self.view.anchor = Some(TranscriptAnchor {
                entry_id: first.entry_id,
                row_in_entry: 0,
                cell_offset: self.view.anchor.map_or(0, |a| a.cell_offset),
            });
        } else {
            self.view.anchor = None;
        }
    }

    /// Fall back to the nearest preceding surviving entry from `position`,
    /// anchored at that entry's last row. Used when the anchored entry's
    /// rendered block collapses to zero rows.
    fn fallback_to_preceding(&mut self, position: usize) {
        for offset in (0..position).rev() {
            if let Some(layout) = self.layouts.get(offset)
                && layout.height > 0
            {
                self.view.anchor = Some(TranscriptAnchor {
                    entry_id: layout.entry_id,
                    row_in_entry: layout.height - 1,
                    cell_offset: self.view.anchor.map_or(0, |a| a.cell_offset),
                });
                return;
            }
        }
        self.view.anchor = None;
    }
}
