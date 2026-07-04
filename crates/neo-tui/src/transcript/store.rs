use crate::primitive::Line;
use crate::primitive::theme::TuiTheme;
use crate::shell::ToolStatusKind;
use crate::transcript::{
    DelegateCardComponent, DelegateGroupComponent, ShellRunComponent, SwarmCardComponent,
    ToolCallComponent, ToolCallState, WorkflowCardComponent,
};

use super::entry::{ApprovalPromptData, ThinkingPhase, TranscriptEntry};
use neo_agent_core::multi_agent::{
    AgentLifecycleState, AgentSnapshot, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::workflow::WorkflowSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptSelection {
    start: usize,
    end: usize,
}

impl TranscriptSelection {
    #[must_use]
    pub const fn new(index: usize) -> Self {
        Self {
            start: index,
            end: index,
        }
    }

    fn extend_up(&mut self, len: usize, count: usize) {
        let max_index = len.saturating_sub(1);
        self.start = self.start.saturating_sub(count).min(max_index);
        self.end = self.end.min(max_index);
    }

    fn extend_down(&mut self, len: usize, count: usize) {
        let max_index = len.saturating_sub(1);
        self.start = self.start.min(max_index);
        self.end = self.end.saturating_add(count).min(max_index);
    }

    #[must_use]
    fn range(&self, len: usize) -> Option<std::ops::Range<usize>> {
        if len == 0 {
            return None;
        }
        let max_index = len - 1;
        let start = self.start.min(max_index).min(self.end.min(max_index));
        let end = self.start.min(max_index).max(self.end.min(max_index)) + 1;
        Some(start..end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptViewport {
    scroll_top_rows: usize,
    content_rows: usize,
    viewport_rows: usize,
    follow_tail: bool,
    selection: Option<TranscriptSelection>,
}

impl TranscriptViewport {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            scroll_top_rows: 0,
            content_rows: 0,
            viewport_rows: 0,
            follow_tail: true,
            selection: None,
        }
    }

    #[must_use]
    pub const fn selection(&self) -> Option<&TranscriptSelection> {
        self.selection.as_ref()
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

/// Cached render output for a single transcript entry. The cache is valid
/// only while `width` matches the current terminal content width.
#[derive(Debug, Clone)]
struct CachedRender {
    width: usize,
    lines: Vec<Line>,
    ansi_lines: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TranscriptStore {
    entries: Vec<TranscriptEntry>,
    suppressed_tool_run_ids: Vec<String>,
    active_assistant: Option<usize>,
    active_thinking: Option<usize>,
    can_coalesce_thinking: bool,
    separate_next_thinking_delta: bool,
    viewport: TranscriptViewport,
    /// Per-entry render cache, parallel to `entries`. `None` means the entry
    /// needs re-rendering (new, mutated, or width changed).
    render_cache: Vec<Option<CachedRender>>,
    first_dirty_entry: Option<usize>,
}

impl TranscriptStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: TranscriptEntry) {
        if matches!(entry, TranscriptEntry::ThinkingBlock { .. }) {
            self.active_thinking = None;
            self.can_coalesce_thinking = false;
            self.separate_next_thinking_delta = false;
        } else {
            self.mark_visible_boundary();
        }
        let index = self.entries.len();
        self.entries.push(entry);
        self.render_cache.push(None);
        self.mark_dirty_from(index);
    }

    pub fn start_assistant(&mut self) {
        if self.active_assistant.is_some() {
            return;
        }
        self.mark_visible_boundary();
        let index = self.entries.len();
        self.entries.push(TranscriptEntry::assistant_message(""));
        self.active_assistant = Some(self.entries.len() - 1);
        self.render_cache.push(None);
        self.mark_dirty_from(index);
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        self.start_assistant();
        let Some(index) = self.active_assistant else {
            return;
        };
        if let Some(TranscriptEntry::AssistantMessage { content }) = self.entries.get_mut(index) {
            content.push_str(text);
        }
        self.invalidate_cache(index);
    }

    pub fn finish_assistant(&mut self) {
        self.active_assistant = None;
    }

    pub fn start_thinking(&mut self) {
        if self.active_thinking.is_some() {
            return;
        }
        if self.resume_previous_visual_thinking() {
            return;
        }
        let index = self.entries.len();
        self.entries
            .push(TranscriptEntry::thinking_streaming(String::new()));
        self.active_thinking = Some(self.entries.len() - 1);
        self.can_coalesce_thinking = false;
        self.separate_next_thinking_delta = false;
        self.render_cache.push(None);
        self.mark_dirty_from(index);
    }

    pub fn append_thinking_delta(&mut self, text: &str) {
        if text.is_empty() && self.active_thinking.is_none() {
            return;
        }
        self.start_thinking();
        let Some(index) = self.active_thinking else {
            return;
        };
        if let Some(TranscriptEntry::ThinkingBlock { content, .. }) = self.entries.get_mut(index) {
            if self.separate_next_thinking_delta && !text.is_empty() {
                if !content.is_empty() && !content.ends_with('\n') {
                    content.push('\n');
                }
                self.separate_next_thinking_delta = false;
            }
            content.push_str(text);
        }
        self.invalidate_cache(index);
    }

    pub fn finish_thinking(&mut self) {
        if let Some(index) = self.active_thinking.take() {
            if let Some(TranscriptEntry::ThinkingBlock { phase, .. }) = self.entries.get_mut(index)
            {
                *phase = ThinkingPhase::Complete;
                self.can_coalesce_thinking = true;
                self.separate_next_thinking_delta = false;
            }
            self.invalidate_cache(index);
        }
    }

    pub fn push_tool_run(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Option<String>,
    ) {
        let component = ToolCallComponent::new(ToolCallState {
            id: id.into(),
            name: name.into(),
            arguments,
            result: None,
            details: None,
            status: ToolStatusKind::Running,
            exit_code: None,
        });
        self.push(TranscriptEntry::tool_run(component));
    }

    pub fn push_shell_run(&mut self, component: ShellRunComponent) {
        self.push(TranscriptEntry::shell_run(component));
    }

    pub fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCallComponent> {
        let index = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == id),
        )?;
        self.invalidate_cache(index);
        match self.entries.get_mut(index)? {
            TranscriptEntry::ToolRun { component } => Some(component),
            _ => None,
        }
    }

    pub fn suppress_tool_run(&mut self, id: &str) {
        if !self
            .suppressed_tool_run_ids
            .iter()
            .any(|existing| existing == id)
        {
            self.suppressed_tool_run_ids.push(id.to_owned());
            self.mark_dirty_from(0);
        }
    }

    pub fn unsuppress_tool_run(&mut self, id: &str) {
        let before = self.suppressed_tool_run_ids.len();
        self.suppressed_tool_run_ids
            .retain(|existing| existing != id);
        if self.suppressed_tool_run_ids.len() != before {
            self.mark_dirty_from(0);
        }
    }

    #[must_use]
    pub fn is_tool_run_suppressed(&self, id: &str) -> bool {
        self.suppressed_tool_run_ids
            .iter()
            .any(|existing| existing == id)
    }

    pub fn shell_run_mut(&mut self, id: &str) -> Option<&mut ShellRunComponent> {
        let index = self
            .entries
            .iter()
            .position(|entry| matches!(entry, TranscriptEntry::ShellRun { component } if component.id() == id))?;
        self.invalidate_cache(index);
        match self.entries.get_mut(index)? {
            TranscriptEntry::ShellRun { component } => Some(component),
            _ => None,
        }
    }

    pub fn approval_mut(&mut self, id: &str) -> Option<&mut ApprovalPromptData> {
        let index = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::ApprovalPrompt(data) if data.id == id),
        )?;
        self.invalidate_cache(index);
        match self.entries.get_mut(index)? {
            TranscriptEntry::ApprovalPrompt(data) => Some(data),
            _ => None,
        }
    }

    pub fn insert_approval_after_tool_or_push(&mut self, data: ApprovalPromptData) {
        self.mark_visible_boundary();
        let insert_at = self
            .entries
            .iter()
            .rposition(
                |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == data.id),
            )
            .map(|index| index + 1);
        if let Some(index) = insert_at {
            self.entries
                .insert(index, TranscriptEntry::ApprovalPrompt(data));
            self.render_cache.insert(index, None);
            self.mark_dirty_from(index);
        } else {
            self.push(TranscriptEntry::ApprovalPrompt(data));
        }
    }

    #[must_use]
    pub fn has_tool(&self, id: &str) -> bool {
        self.entries.iter().any(
            |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == id),
        )
    }

    #[must_use]
    pub fn has_shell_run(&self, id: &str) -> bool {
        self.entries.iter().any(
            |entry| matches!(entry, TranscriptEntry::ShellRun { component } if component.id() == id),
        )
    }

    /// Upsert a delegate card by agent ID. If a card for this agent already
    /// exists, update it in place; otherwise append a new entry.
    pub fn upsert_delegate(&mut self, turn: u32, snapshot: AgentSnapshot) {
        self.mark_visible_boundary();
        let id = snapshot.id.as_str().to_owned();
        if let Some(group) = self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::DelegateGroup { component } if component.contains(&id) => {
                Some(component)
            }
            _ => None,
        }) {
            group.upsert(snapshot);
            self.invalidate_all_cache();
            return;
        }
        if let Some(entry) = self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::Delegate { component } if component.id() == id => Some(component),
            _ => None,
        }) {
            let merged = merge_delegate_snapshot(entry.snapshot(), snapshot);
            entry.update(merged);
            self.invalidate_all_cache();
            return;
        }
        if let Some(group) = self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::DelegateGroup { component } if component.turn() == turn => {
                Some(component)
            }
            _ => None,
        }) && is_root_delegate(&snapshot)
        {
            group.upsert(snapshot);
            self.invalidate_all_cache();
            return;
        }
        if is_root_delegate(&snapshot)
            && let Some(index) = self.entries.iter().position(|entry| {
                matches!(
                    entry,
                    TranscriptEntry::Delegate { component }
                        if component.turn() == Some(turn)
                            && is_root_delegate(component.snapshot())
                )
            })
            && let TranscriptEntry::Delegate { component } = self.entries.remove(index)
        {
            let existing = component.into_snapshot();
            self.entries.insert(
                index,
                TranscriptEntry::DelegateGroup {
                    component: DelegateGroupComponent::new(turn, vec![existing, snapshot]),
                },
            );
            // Delegate→DelegateGroup replacement: both non-cacheable, slot
            // stays None. Keep render_cache in sync.
            if index < self.render_cache.len() {
                self.render_cache[index] = None;
            }
            self.mark_dirty_from(index);
            return;
        }
        self.push(TranscriptEntry::Delegate {
            component: DelegateCardComponent::with_turn(turn, snapshot),
        });
    }

    /// Upsert a swarm card by swarm ID. If a card for this swarm already
    /// exists, update it in place; otherwise append a new entry.
    pub fn upsert_delegate_swarm(&mut self, snapshot: SwarmSnapshot) {
        self.mark_visible_boundary();
        let id = snapshot.swarm_id.clone();
        if let Some(entry) = self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::DelegateSwarm { component } if component.swarm_id() == id => {
                Some(component)
            }
            _ => None,
        }) {
            let merged = merge_swarm_snapshot(entry.snapshot(), snapshot);
            entry.update(merged);
            self.invalidate_all_cache();
            return;
        }
        self.push(TranscriptEntry::DelegateSwarm {
            component: SwarmCardComponent::new(snapshot),
        });
    }

    /// Upsert a workflow card by workflow ID.
    pub fn upsert_workflow(&mut self, snapshot: WorkflowSnapshot) {
        self.mark_visible_boundary();
        let id = snapshot.id.0.clone();
        let existing_index = self
            .entries
            .iter()
            .position(|entry| matches!(entry, TranscriptEntry::Workflow { component } if component.id() == id));
        if let Some(index) = existing_index {
            if let Some(TranscriptEntry::Workflow { component }) = self.entries.get_mut(index) {
                component.update(snapshot);
            }
            self.invalidate_cache(index);
            return;
        }
        self.push(TranscriptEntry::Workflow {
            component: WorkflowCardComponent::new(snapshot),
        });
    }

    #[must_use]
    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [TranscriptEntry] {
        self.invalidate_all_cache();
        self.can_coalesce_thinking = false;
        self.separate_next_thinking_delta = false;
        &mut self.entries
    }

    pub(crate) fn invalidate_render_cache(&mut self) {
        self.invalidate_all_cache();
    }

    pub fn tick_live_entries(&mut self, now_ms: u64) -> bool {
        // Fast path: if no live-capable entries exist, skip the full scan.
        // This avoids an O(n) iteration over all entries every 50ms tick
        // when there are no delegates, MCP connections, or streaming blocks.
        let has_live = self.entries.iter().any(|e| {
            matches!(
                e,
                TranscriptEntry::Delegate { .. }
                    | TranscriptEntry::DelegateGroup { .. }
                    | TranscriptEntry::DelegateSwarm { .. }
                    | TranscriptEntry::McpStartupStatus { .. }
            )
        });
        if !has_live {
            return false;
        }
        let mut first_changed = None;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.on_render_tick(now_ms) {
                first_changed.get_or_insert(index);
            }
        }
        if let Some(index) = first_changed {
            self.mark_dirty_from(index);
            true
        } else {
            false
        }
    }

    /// Remove the entry at `index`, shifting later entries down. Returns the
    /// removed entry. Used to pop a queued follow-up when it is promoted to a
    /// steer.
    pub fn remove(&mut self, index: usize) -> Option<TranscriptEntry> {
        if index >= self.entries.len() {
            return None;
        }
        self.can_coalesce_thinking = false;
        self.separate_next_thinking_delta = false;
        let entry = self.entries.remove(index);
        if index < self.render_cache.len() {
            self.render_cache.remove(index);
        }
        self.mark_dirty_from(index);
        Some(entry)
    }

    #[must_use]
    pub const fn viewport(&self) -> &TranscriptViewport {
        &self.viewport
    }

    pub fn viewport_mut(&mut self) -> &mut TranscriptViewport {
        &mut self.viewport
    }

    pub fn select_visible_entry(&mut self) {
        if self.viewport.is_following_tail() {
            self.viewport.selection = self
                .entries
                .len()
                .checked_sub(1)
                .map(TranscriptSelection::new);
            return;
        }

        let range = self.viewport.visible_row_range(self.entries.len(), 1);
        let Some(index) = range.end.checked_sub(1) else {
            self.viewport.selection = None;
            return;
        };
        self.viewport.selection =
            (index < self.entries.len()).then(|| TranscriptSelection::new(index));
    }

    pub fn clear_selection(&mut self) {
        self.viewport.selection = None;
    }

    pub fn extend_selection_up(&mut self, count: usize) {
        if self.viewport.selection.is_none() {
            self.select_visible_entry();
        }
        if let Some(selection) = &mut self.viewport.selection {
            selection.extend_up(self.entries.len(), count);
        }
    }

    pub fn extend_selection_down(&mut self, count: usize) {
        if self.viewport.selection.is_none() {
            self.select_visible_entry();
        }
        if let Some(selection) = &mut self.viewport.selection {
            selection.extend_down(self.entries.len(), count);
        }
    }

    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.viewport.selection.is_some()
    }

    #[must_use]
    pub fn copy_selection(&self) -> Option<String> {
        let range = self.viewport.selection?.range(self.entries.len())?;
        let mut copied = String::new();
        for (offset, entry) in self.entries[range].iter().enumerate() {
            if offset > 0 {
                copied.push_str("\n\n");
            }
            let (label, content) = entry.copy_parts();
            copied.push_str(label);
            copied.push('\n');
            copied.push_str(&content);
        }
        Some(copied)
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn render_rows(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        self.entries
            .iter()
            .flat_map(|entry| entry.render(width, theme))
            .collect()
    }

    fn resume_previous_visual_thinking(&mut self) -> bool {
        if !self.can_coalesce_thinking {
            return false;
        }
        let Some(index) = self.entries.len().checked_sub(1) else {
            return false;
        };
        let Some(TranscriptEntry::ThinkingBlock { phase, .. }) = self.entries.get_mut(index) else {
            return false;
        };
        if *phase != ThinkingPhase::Complete {
            return false;
        }

        *phase = ThinkingPhase::Streaming;
        self.active_thinking = Some(index);
        self.can_coalesce_thinking = false;
        self.separate_next_thinking_delta = true;
        // Entry transitions from Complete (cached) to Streaming (not cached).
        self.invalidate_cache(index);
        true
    }

    fn mark_visible_boundary(&mut self) {
        self.finish_thinking();
        self.can_coalesce_thinking = false;
        self.separate_next_thinking_delta = false;
    }

    // ── Render cache management ───────────────────────────────────────────

    /// Invalidate the render cache for a single entry index.
    fn invalidate_cache(&mut self, index: usize) {
        if index < self.render_cache.len() {
            self.render_cache[index] = None;
        }
        self.mark_dirty_from(index);
    }

    /// Invalidate all cached renders (e.g. on terminal resize).
    fn invalidate_all_cache(&mut self) {
        for slot in &mut self.render_cache {
            *slot = None;
        }
        if !self.entries.is_empty() {
            self.mark_dirty_from(0);
        }
    }

    pub(crate) const fn first_dirty_entry(&self) -> Option<usize> {
        self.first_dirty_entry
    }

    pub(crate) fn clear_dirty_entries(&mut self) {
        self.first_dirty_entry = None;
    }

    fn mark_dirty_from(&mut self, index: usize) {
        self.first_dirty_entry = Some(
            self.first_dirty_entry
                .map_or(index, |current| current.min(index)),
        );
    }

    /// Ensure `render_cache` has the same length as `entries`.
    fn sync_cache_len(&mut self) {
        if self.render_cache.len() != self.entries.len() {
            self.render_cache.resize(self.entries.len(), None);
        }
    }

    /// Invalidate all cached renders when the terminal content width changes.
    /// Called once at the start of each render frame.
    pub fn ensure_cache_width(&mut self, width: usize) {
        // Check whether any cached entry has a stale width. If the cache is
        // empty this is a no-op.
        let needs_invalidation = self
            .render_cache
            .iter()
            .flatten()
            .any(|cached| cached.width != width);
        if needs_invalidation {
            self.invalidate_all_cache();
        }
    }

    /// Render a single entry, using the per-entry render cache for static
    /// (non-live) entries. Live entries — those whose output depends on
    /// `activity_frame` or per-tick animation — bypass the cache entirely.
    ///
    /// Returns a clone of the cached lines on a cache hit, or renders fresh
    /// and populates the cache on a miss.
    pub fn render_entry_cached(
        &mut self,
        index: usize,
        width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
    ) -> Vec<Line> {
        self.sync_cache_len();

        let cacheable = self
            .entries
            .get(index)
            .is_some_and(TranscriptEntry::is_render_cacheable);

        if cacheable
            && let Some(Some(cached)) = self.render_cache.get(index)
            && cached.width == width
        {
            return cached.lines.clone();
        }

        let lines = self.render_entry_lines(index, width, theme, activity_frame);

        if cacheable && let Some(slot) = self.render_cache.get_mut(index) {
            let ansi_lines = lines.iter().map(Line::to_ansi).collect();
            *slot = Some(CachedRender {
                width,
                lines: lines.clone(),
                ansi_lines,
            });
        }

        lines
    }

    /// Render a single entry to final ANSI rows, using the same cache as
    /// [`Self::render_entry_cached`] so transcript body composition can avoid
    /// cloning cached `Line` spans and re-running `to_ansi()` on every frame.
    pub fn render_entry_ansi_cached(
        &mut self,
        index: usize,
        width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
    ) -> Vec<String> {
        self.sync_cache_len();

        let cacheable = self
            .entries
            .get(index)
            .is_some_and(TranscriptEntry::is_render_cacheable);

        if cacheable
            && let Some(Some(cached)) = self.render_cache.get(index)
            && cached.width == width
        {
            return cached.ansi_lines.clone();
        }

        let lines = self.render_entry_lines(index, width, theme, activity_frame);
        let ansi_lines = lines.iter().map(Line::to_ansi).collect::<Vec<_>>();

        if cacheable && let Some(slot) = self.render_cache.get_mut(index) {
            *slot = Some(CachedRender {
                width,
                lines,
                ansi_lines: ansi_lines.clone(),
            });
        }

        ansi_lines
    }

    fn render_entry_lines(
        &self,
        index: usize,
        width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
    ) -> Vec<Line> {
        match self.entries.get(index) {
            Some(entry) => entry.render_with_activity_frame(width, theme, activity_frame),
            None => Vec::new(),
        }
    }
}

fn is_root_delegate(snapshot: &AgentSnapshot) -> bool {
    snapshot.path.is_root_child()
}

/// Merge an incoming delegate snapshot with the current one, respecting
/// terminal precedence. A stale `Completed` snapshot arriving after a
/// `Cancelled` snapshot must not regress the card — Cancelled always
/// wins over Completed for the same run, regardless of timestamp.
fn merge_delegate_snapshot(current: &AgentSnapshot, incoming: AgentSnapshot) -> AgentSnapshot {
    // Different agents — just take the incoming.
    if current.id != incoming.id {
        return incoming;
    }
    // Cancelled always beats a late Completed for the same run.
    if current.state == AgentLifecycleState::Cancelled
        && incoming.state == AgentLifecycleState::Completed
    {
        return current.clone();
    }
    // Both terminal: prefer the earlier one (it happened first).
    if current.state.is_terminal()
        && incoming.state.is_terminal()
        && incoming.updated_at_ms < current.updated_at_ms
    {
        return current.clone();
    }
    incoming
}

fn merge_swarm_snapshot(current: &SwarmSnapshot, incoming: SwarmSnapshot) -> SwarmSnapshot {
    if current.swarm_id != incoming.swarm_id {
        return incoming;
    }

    let mut children = incoming
        .children
        .into_iter()
        .map(|incoming_child| {
            let current_child = current.children.iter().find(|child| {
                child.item_index == incoming_child.item_index
                    || child.agent.id == incoming_child.agent.id
            });
            current_child.map_or(incoming_child.clone(), |current_child| {
                merge_swarm_child(current_child, incoming_child)
            })
        })
        .collect::<Vec<_>>();

    for current_child in &current.children {
        if !children.iter().any(|child| {
            child.item_index == current_child.item_index || child.agent.id == current_child.agent.id
        }) {
            children.push(current_child.clone());
        }
    }
    children.sort_by_key(|child| child.item_index);

    SwarmSnapshot {
        swarm_id: current.swarm_id.clone(),
        description: incoming.description,
        role: current.role,
        mode: incoming.mode,
        state: SwarmAggregate::from_states(children.iter().map(|child| child.agent.state)).status(),
        max_concurrency: incoming.max_concurrency.max(current.max_concurrency).max(1),
        aggregate: SwarmAggregate::from_states(children.iter().map(|child| child.agent.state)),
        children,
    }
}

fn merge_swarm_child(
    current: &SwarmChildSnapshot,
    incoming: SwarmChildSnapshot,
) -> SwarmChildSnapshot {
    // Cancelled always beats a late Completed for the same child.
    if current.agent.state == AgentLifecycleState::Cancelled
        && incoming.agent.state == AgentLifecycleState::Completed
    {
        return current.clone();
    }
    if child_progress_rank(incoming.agent.state) < child_progress_rank(current.agent.state) {
        return current.clone();
    }
    if child_progress_rank(incoming.agent.state) == child_progress_rank(current.agent.state)
        && incoming.agent.activity.len() < current.agent.activity.len()
        && incoming.agent.latest_text.is_none()
        && incoming.agent.outcome.is_none()
    {
        return current.clone();
    }
    incoming
}

fn child_progress_rank(state: AgentLifecycleState) -> u8 {
    match state {
        AgentLifecycleState::Queued => 0,
        AgentLifecycleState::Running => 1,
        AgentLifecycleState::Completed
        | AgentLifecycleState::Failed
        | AgentLifecycleState::Cancelled
        | AgentLifecycleState::TimedOut
        | AgentLifecycleState::Interrupted => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_entry_ansi_cached_stores_final_ansi_rows() {
        let mut store = TranscriptStore::new();
        let theme = TuiTheme::default();
        store.push(TranscriptEntry::assistant_message("cached answer"));

        let first = store.render_entry_ansi_cached(0, 80, &theme, 0);

        assert!(first.iter().any(|line| line.contains("cached answer")));
        let cached = store.render_cache[0].as_ref().expect("cached render");
        assert_eq!(cached.ansi_lines, first);
        assert_eq!(store.render_entry_ansi_cached(0, 80, &theme, 99), first);
    }
}
