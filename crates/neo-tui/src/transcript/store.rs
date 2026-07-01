use crate::primitive::Line;
use crate::primitive::theme::TuiTheme;
use crate::shell::ToolStatusKind;
use crate::transcript::{
    DelegateCardComponent, DelegateGroupComponent, ShellRunComponent, SwarmCardComponent,
    ToolCallComponent, ToolCallState, WorkflowCardComponent,
};

use super::entry::{ApprovalPromptData, ThinkingPhase, TranscriptEntry};
use neo_agent_core::multi_agent::{
    AgentLifecycleState, AgentSnapshot, SwarmChildSnapshot, SwarmSnapshot,
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

#[derive(Debug, Clone, Default)]
pub struct TranscriptStore {
    entries: Vec<TranscriptEntry>,
    suppressed_tool_run_ids: Vec<String>,
    active_assistant: Option<usize>,
    active_thinking: Option<usize>,
    viewport: TranscriptViewport,
}

impl TranscriptStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: TranscriptEntry) {
        self.entries.push(entry);
    }

    pub fn start_assistant(&mut self) {
        if self.active_assistant.is_some() {
            return;
        }
        self.entries.push(TranscriptEntry::assistant_message(""));
        self.active_assistant = Some(self.entries.len() - 1);
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        self.start_assistant();
        let Some(index) = self.active_assistant else {
            return;
        };
        if let Some(TranscriptEntry::AssistantMessage { content }) = self.entries.get_mut(index) {
            content.push_str(text);
        }
    }

    pub fn finish_assistant(&mut self) {
        self.active_assistant = None;
    }

    pub fn start_thinking(&mut self) {
        if self.active_thinking.is_some() {
            return;
        }
        self.entries
            .push(TranscriptEntry::thinking_streaming(String::new()));
        self.active_thinking = Some(self.entries.len() - 1);
    }

    pub fn append_thinking_delta(&mut self, text: &str) {
        self.start_thinking();
        let Some(index) = self.active_thinking else {
            return;
        };
        if let Some(TranscriptEntry::ThinkingBlock { content, .. }) = self.entries.get_mut(index) {
            content.push_str(text);
        }
    }

    pub fn finish_thinking(&mut self) {
        if let Some(index) = self.active_thinking.take()
            && let Some(TranscriptEntry::ThinkingBlock { phase, .. }) = self.entries.get_mut(index)
        {
            *phase = ThinkingPhase::Complete;
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
        self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::ToolRun { component } if component.id() == id => Some(component),
            _ => None,
        })
    }

    pub fn suppress_tool_run(&mut self, id: &str) {
        if !self
            .suppressed_tool_run_ids
            .iter()
            .any(|existing| existing == id)
        {
            self.suppressed_tool_run_ids.push(id.to_owned());
        }
    }

    pub fn unsuppress_tool_run(&mut self, id: &str) {
        self.suppressed_tool_run_ids
            .retain(|existing| existing != id);
    }

    #[must_use]
    pub fn is_tool_run_suppressed(&self, id: &str) -> bool {
        self.suppressed_tool_run_ids
            .iter()
            .any(|existing| existing == id)
    }

    pub fn shell_run_mut(&mut self, id: &str) -> Option<&mut ShellRunComponent> {
        self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::ShellRun { component } if component.id() == id => Some(component),
            _ => None,
        })
    }

    pub fn approval_mut(&mut self, id: &str) -> Option<&mut ApprovalPromptData> {
        self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::ApprovalPrompt(data) if data.id == id => Some(data),
            _ => None,
        })
    }

    pub fn insert_approval_after_tool_or_push(&mut self, data: ApprovalPromptData) {
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
        let id = snapshot.id.as_str().to_owned();
        if let Some(group) = self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::DelegateGroup { component } if component.contains(&id) => {
                Some(component)
            }
            _ => None,
        }) {
            group.upsert(snapshot);
            return;
        }
        if let Some(entry) = self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::Delegate { component } if component.id() == id => Some(component),
            _ => None,
        }) {
            entry.update(snapshot);
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
            return;
        }
        self.push(TranscriptEntry::Delegate {
            component: DelegateCardComponent::with_turn(turn, snapshot),
        });
    }

    /// Upsert a swarm card by swarm ID. If a card for this swarm already
    /// exists, update it in place; otherwise append a new entry.
    pub fn upsert_delegate_swarm(&mut self, snapshot: SwarmSnapshot) {
        let id = snapshot.swarm_id.clone();
        if let Some(entry) = self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::DelegateSwarm { component } if component.swarm_id() == id => {
                Some(component)
            }
            _ => None,
        }) {
            let merged = merge_swarm_snapshot(entry.snapshot(), snapshot);
            entry.update(merged);
            return;
        }
        self.push(TranscriptEntry::DelegateSwarm {
            component: SwarmCardComponent::new(snapshot),
        });
    }

    /// Upsert a workflow card by workflow ID.
    pub fn upsert_workflow(&mut self, snapshot: WorkflowSnapshot) {
        let id = snapshot.id.0.clone();
        if let Some(entry) = self.entries.iter_mut().find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } if component.id() == id => Some(component),
            _ => None,
        }) {
            entry.update(snapshot);
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
        &mut self.entries
    }

    pub fn tick_live_entries(&mut self, now_ms: u64) -> bool {
        self.entries
            .iter_mut()
            .any(|entry| entry.on_render_tick(now_ms))
    }

    /// Remove the entry at `index`, shifting later entries down. Returns the
    /// removed entry. Used to pop a queued follow-up when it is promoted to a
    /// steer.
    pub fn remove(&mut self, index: usize) -> Option<TranscriptEntry> {
        if index >= self.entries.len() {
            return None;
        }
        let entry = self.entries.remove(index);
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
}

fn is_root_delegate(snapshot: &AgentSnapshot) -> bool {
    snapshot.path.is_root_child()
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
        state: incoming.state,
        max_concurrency: incoming.max_concurrency.max(current.max_concurrency).max(1),
        aggregate: incoming.aggregate,
        children,
    }
}

fn merge_swarm_child(
    current: &SwarmChildSnapshot,
    incoming: SwarmChildSnapshot,
) -> SwarmChildSnapshot {
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
        | AgentLifecycleState::TimedOut => 2,
    }
}
