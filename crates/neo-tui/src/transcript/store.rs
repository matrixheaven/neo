use crate::primitive::theme::TuiTheme;
use crate::primitive::{Component, Finalization, Line};
use crate::shell::ToolStatusKind;
use crate::terminal_image::{ImageRenderPolicy, TerminalImageCapabilities};
use crate::transcript::{
    DelegateCardComponent, DelegateGroupComponent, ShellRunComponent, SwarmCardComponent,
    ToolCallComponent, ToolCallState, WorkflowCardComponent,
};

use super::entry::{
    ApprovalPromptData, RetryPhase, RetryStatusData, ThinkingPhase, TranscriptEntry,
};
use neo_agent_core::multi_agent::{
    AgentLifecycleState, AgentProgressSnapshot, AgentSnapshot, SwarmAggregate, SwarmChildProgress,
    SwarmChildSnapshot, SwarmSnapshot, apply_agent_progress, apply_swarm_child_progress,
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

/// Stable identity for an entry in a [`TranscriptStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TranscriptEntryId(u64);

#[derive(Debug, Clone, Default)]
pub struct TranscriptStore {
    entries: Vec<TranscriptEntry>,
    entry_ids: Vec<TranscriptEntryId>,
    entry_revisions: Vec<u64>,
    next_entry_id: u64,
    suppressed_tool_run_ids: Vec<String>,
    active_assistant: Option<usize>,
    active_thinking: Option<usize>,
    live_model_attempt: Option<(u32, usize)>,
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
        } else {
            self.mark_visible_boundary();
        }
        self.append_entry(entry);
    }

    pub(crate) fn begin_live_model_attempt(&mut self, turn: u32) {
        if self
            .live_model_attempt
            .is_none_or(|(active_turn, _)| active_turn != turn)
        {
            self.live_model_attempt = Some((turn, self.entries.len()));
        }
    }

    pub(crate) fn finish_live_model_attempt(&mut self, turn: u32) {
        if self
            .live_model_attempt
            .is_some_and(|(active_turn, _)| active_turn == turn)
        {
            self.live_model_attempt = None;
        }
    }

    #[must_use]
    pub(crate) fn live_model_attempt_start(&self) -> Option<usize> {
        self.live_model_attempt.map(|(_, start)| start)
    }

    pub(crate) fn take_empty_live_attempt_anchor(&mut self) -> Option<usize> {
        let Some((_, index)) = self.live_model_attempt else {
            return None;
        };
        if !matches!(
            self.entries.get(index),
            Some(TranscriptEntry::AssistantMessage { content }) if content.is_empty()
        ) {
            return None;
        }

        if self.active_assistant == Some(index) {
            self.active_assistant = None;
        }
        Some(index)
    }

    pub fn start_assistant(&mut self) {
        if self.active_assistant.is_some() {
            return;
        }
        self.mark_visible_boundary();
        let index = self.append_entry(TranscriptEntry::assistant_message(""));
        self.active_assistant = Some(index);
    }

    pub fn append_assistant_delta(&mut self, text: &str) {
        self.start_assistant();
        let Some(index) = self.active_assistant else {
            return;
        };
        if let Some(TranscriptEntry::AssistantMessage { content }) = self.entries.get_mut(index) {
            content.push_str(text);
        }
        self.touch_entry(index);
    }

    pub fn finish_assistant(&mut self) {
        if let Some(index) = self.active_assistant.take() {
            self.touch_entry(index);
        }
    }

    pub fn upsert_retry_status(&mut self, mut data: RetryStatusData) -> bool {
        let turn = data.turn;
        if let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::RetryStatus { data: current } if current.turn == data.turn),
        ) {
            let changed = self.mutate_entry(index, |entry| {
                let TranscriptEntry::RetryStatus { data: current } = entry else {
                    return false;
                };
                if current.phase == RetryPhase::Exhausted {
                    return false;
                }
                if data.phase == RetryPhase::Connecting {
                    data.delay_ms = current.delay_ms;
                    data.started_at_ms = current.started_at_ms;
                    if data.error_code.is_empty() {
                        data.error_code.clone_from(&current.error_code);
                    }
                    if data.message.is_empty() {
                        data.message.clone_from(&current.message);
                    }
                }
                if *current == data {
                    return false;
                }
                *current = data;
                true
            });
            self.live_model_attempt = Some((turn, index));
            return changed;
        }

        let slot = self.active_assistant.or(self.active_thinking);
        let Some(index) = slot else {
            let index = self.append_entry(TranscriptEntry::retry_status(data));
            self.live_model_attempt = Some((turn, index));
            return true;
        };
        let changed = self.mutate_entry(index, |entry| {
            *entry = TranscriptEntry::retry_status(data);
            true
        });
        if self.active_assistant == Some(index) {
            self.active_assistant = None;
        }
        if self.active_thinking == Some(index) {
            self.active_thinking = None;
        }
        self.live_model_attempt = Some((turn, index));
        changed
    }

    pub fn clear_retry_status(&mut self, turn: u32) -> bool {
        let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::RetryStatus { data } if data.turn == turn),
        ) else {
            return false;
        };
        let changed = self.mutate_entry(index, |entry| {
            *entry = TranscriptEntry::assistant_message("");
            true
        });
        self.active_thinking = None;
        self.active_assistant = Some(index);
        self.live_model_attempt = Some((turn, index));
        changed
    }

    #[must_use]
    pub fn has_exhausted_retry_status(&self, turn: u32) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry,
                TranscriptEntry::RetryStatus { data }
                    if data.turn == turn && data.phase == RetryPhase::Exhausted
            )
        })
    }

    pub fn interrupt_retry_status(&mut self, turn: u32) -> bool {
        let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::RetryStatus { data } if data.turn == turn),
        ) else {
            return false;
        };
        self.mutate_entry(index, TranscriptEntry::interrupt)
    }

    pub fn reset_live_model_attempt(&mut self, turn: u32) -> bool {
        if self.entries.iter().any(
            |entry| matches!(entry, TranscriptEntry::RetryStatus { data } if data.turn == turn),
        ) {
            return false;
        }

        let mut provisional = if let Some((_, start)) = self
            .live_model_attempt
            .filter(|(active_turn, _)| *active_turn == turn)
        {
            self.entries
                .iter()
                .enumerate()
                .skip(start)
                .filter_map(|(index, entry)| {
                    matches!(
                        entry,
                        TranscriptEntry::AssistantMessage { .. }
                            | TranscriptEntry::ThinkingBlock { .. }
                            | TranscriptEntry::ToolRun { .. }
                    )
                    .then_some(index)
                })
                .collect::<Vec<_>>()
        } else {
            [self.active_assistant, self.active_thinking]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        };
        provisional.sort_unstable();
        provisional.dedup();
        let Some(anchor) = provisional.first().copied() else {
            return false;
        };

        let mut changed = false;
        for index in provisional.into_iter().skip(1).rev() {
            changed |= self.remove(index).is_some();
        }
        changed |= self.mutate_entry(anchor, |entry| {
            if matches!(entry, TranscriptEntry::AssistantMessage { content } if content.is_empty())
            {
                return false;
            }
            *entry = TranscriptEntry::assistant_message("");
            true
        });
        self.active_thinking = None;
        self.active_assistant = Some(anchor);
        self.live_model_attempt = Some((turn, anchor));
        changed
    }

    pub fn start_thinking(&mut self) {
        if self.active_thinking.is_some() {
            return;
        }
        if let Some(index) = self.take_empty_live_attempt_anchor() {
            self.mutate_entry(index, |entry| {
                *entry = TranscriptEntry::thinking_streaming(String::new());
                true
            });
            self.active_thinking = Some(index);
            return;
        }
        let index = self.append_entry(TranscriptEntry::thinking_streaming(String::new()));
        self.active_thinking = Some(index);
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
            content.push_str(text);
        }
        self.touch_entry(index);
    }

    pub fn finish_thinking(&mut self) {
        if let Some(index) = self.active_thinking.take() {
            if let Some(TranscriptEntry::ThinkingBlock { phase, .. }) = self.entries.get_mut(index)
            {
                *phase = ThinkingPhase::Complete;
            }
            self.touch_entry(index);
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

    #[must_use]
    pub fn tool(&self, id: &str) -> Option<&ToolCallComponent> {
        self.entries.iter().find_map(|entry| match entry {
            TranscriptEntry::ToolRun { component } if component.id() == id => Some(component),
            _ => None,
        })
    }

    pub fn mutate_tool(
        &mut self,
        id: &str,
        mutate: impl FnOnce(&mut ToolCallComponent) -> bool,
    ) -> bool {
        let index = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == id),
        );
        let Some(index) = index else {
            return false;
        };
        self.mutate_entry(index, |entry| match entry {
            TranscriptEntry::ToolRun { component } => mutate(component),
            _ => false,
        })
    }

    pub fn mutate_shell_run(
        &mut self,
        id: &str,
        mutate: impl FnOnce(&mut ShellRunComponent) -> bool,
    ) -> bool {
        let index = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::ShellRun { component } if component.id() == id),
        );
        let Some(index) = index else {
            return false;
        };
        self.mutate_entry(index, |entry| match entry {
            TranscriptEntry::ShellRun { component } => mutate(component),
            _ => false,
        })
    }

    pub fn mutate_approval(
        &mut self,
        id: &str,
        mutate: impl FnOnce(&mut ApprovalPromptData) -> bool,
    ) -> bool {
        let index = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::ApprovalPrompt(data) if data.id == id),
        );
        let Some(index) = index else {
            return false;
        };
        self.mutate_entry(index, |entry| match entry {
            TranscriptEntry::ApprovalPrompt(data) => mutate(data),
            _ => false,
        })
    }

    #[must_use]
    pub fn shell_run(&self, id: &str) -> Option<&ShellRunComponent> {
        self.entries.iter().find_map(|entry| match entry {
            TranscriptEntry::ShellRun { component } if component.id() == id => Some(component),
            _ => None,
        })
    }

    #[must_use]
    pub fn approval(&self, id: &str) -> Option<&ApprovalPromptData> {
        self.entries.iter().find_map(|entry| match entry {
            TranscriptEntry::ApprovalPrompt(data) if data.id == id => Some(data),
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
            self.insert_entry(index, TranscriptEntry::ApprovalPrompt(data));
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
        if let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::DelegateGroup { component } if component.contains(&id)),
        ) {
            let preserve_terminal = matches!(
                &self.entries[index],
                TranscriptEntry::DelegateGroup { component }
                    if component.snapshot(&id).is_some_and(|current| {
                        current.state.is_terminal() && !snapshot.state.is_terminal()
                    })
            );
            if preserve_terminal {
                return;
            }
            self.mutate_entry(index, |entry| {
                let TranscriptEntry::DelegateGroup { component } = entry else {
                    return false;
                };
                component.upsert(snapshot)
            });
            return;
        }
        if let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::Delegate { component } if component.id() == id),
        ) {
            let merged = match &self.entries[index] {
                TranscriptEntry::Delegate { component } => {
                    merge_delegate_snapshot(component.snapshot(), snapshot)
                }
                _ => return,
            };
            if matches!(
                &self.entries[index],
                TranscriptEntry::Delegate { component } if component.snapshot() == &merged
            ) {
                return;
            }
            self.mutate_entry(index, |entry| {
                let TranscriptEntry::Delegate { component } = entry else {
                    return false;
                };
                component.update(merged)
            });
            return;
        }
        if is_root_delegate(&snapshot)
            && let Some(index) = self.entries.iter().position(|entry| {
                matches!(entry, TranscriptEntry::DelegateGroup { component }
                    if component.turn() == turn
                        && component.finalization() == Finalization::Live)
            })
        {
            let preserve_terminal = matches!(
                &self.entries[index],
                TranscriptEntry::DelegateGroup { component }
                    if component
                        .snapshot(snapshot.id.as_str())
                        .is_some_and(|current| {
                            current.state.is_terminal() && !snapshot.state.is_terminal()
                        })
            );
            if preserve_terminal {
                return;
            }
            self.mutate_entry(index, |entry| {
                let TranscriptEntry::DelegateGroup { component } = entry else {
                    return false;
                };
                component.upsert(snapshot)
            });
            return;
        }
        if is_root_delegate(&snapshot)
            && let Some(index) = self.entries.iter().position(|entry| {
                matches!(
                    entry,
                    TranscriptEntry::Delegate { component }
                        if component.turn() == Some(turn)
                            && is_root_delegate(component.snapshot())
                            && component.finalization() == Finalization::Live
                )
            })
        {
            let existing = match &self.entries[index] {
                TranscriptEntry::Delegate { component } => component.snapshot().clone(),
                _ => return,
            };
            self.mutate_entry(index, |entry| {
                *entry = TranscriptEntry::DelegateGroup {
                    component: DelegateGroupComponent::new(turn, vec![existing, snapshot]),
                };
                true
            });
            return;
        }
        self.push(TranscriptEntry::Delegate {
            component: DelegateCardComponent::with_turn(turn, snapshot),
        });
    }

    pub fn upsert_delegate_progress(&mut self, turn: u32, progress: &AgentProgressSnapshot) {
        let id = progress.agent_id.as_str().to_owned();
        if let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::DelegateGroup { component } if component.contains(&id)),
        ) {
            let Some(mut snapshot) = (match &self.entries[index] {
                TranscriptEntry::DelegateGroup { component } => component.snapshot(&id).cloned(),
                _ => None,
            }) else {
                return;
            };
            if snapshot.state.is_terminal() && !progress.state.is_terminal() {
                return;
            }
            let previous = snapshot.clone();
            if !apply_agent_progress(&mut snapshot, progress) || snapshot == previous {
                return;
            }
            self.mutate_entry(index, |entry| {
                let TranscriptEntry::DelegateGroup { component } = entry else {
                    return false;
                };
                component.upsert(snapshot)
            });
            return;
        }
        if let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::Delegate { component } if component.id() == id),
        ) {
            let mut snapshot = match &self.entries[index] {
                TranscriptEntry::Delegate { component } => component.snapshot().clone(),
                _ => return,
            };
            if snapshot.state.is_terminal() && !progress.state.is_terminal() {
                return;
            }
            let previous = snapshot.clone();
            if !apply_agent_progress(&mut snapshot, progress) || snapshot == previous {
                return;
            }
            self.mutate_entry(index, |entry| {
                let TranscriptEntry::Delegate { component } = entry else {
                    return false;
                };
                component.update(snapshot)
            });
            return;
        }
        let _ = turn;
    }

    /// Upsert a swarm card by swarm ID. If a card for this swarm already
    /// exists, update it in place; otherwise append a new entry.
    pub fn upsert_delegate_swarm(&mut self, snapshot: SwarmSnapshot) {
        let id = snapshot.swarm_id.clone();
        if let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::DelegateSwarm { component } if component.swarm_id() == id),
        ) {
            let merged = match &self.entries[index] {
                TranscriptEntry::DelegateSwarm { component } => {
                    merge_swarm_snapshot(component.snapshot(), snapshot)
                }
                _ => return,
            };
            if matches!(
                &self.entries[index],
                TranscriptEntry::DelegateSwarm { component } if component.snapshot() == &merged
            ) {
                return;
            }
            self.mutate_entry(index, |entry| {
                let TranscriptEntry::DelegateSwarm { component } = entry else {
                    return false;
                };
                component.update(merged)
            });
            return;
        }
        self.push(TranscriptEntry::DelegateSwarm {
            component: SwarmCardComponent::new(snapshot),
        });
    }

    pub fn upsert_delegate_swarm_progress(
        &mut self,
        swarm_id: &str,
        state: AgentLifecycleState,
        aggregate: SwarmAggregate,
        child_progress: &SwarmChildProgress,
    ) {
        if let Some(index) = self.entries.iter().position(
            |entry| matches!(entry, TranscriptEntry::DelegateSwarm { component } if component.swarm_id() == swarm_id),
        ) {
            let mut snapshot = match &self.entries[index] {
                TranscriptEntry::DelegateSwarm { component }
                    if !swarm_snapshot_is_terminal(component.snapshot()) =>
                {
                    component.snapshot().clone()
                }
                _ => return,
            };
            let previous = snapshot.clone();
            apply_swarm_child_progress(&mut snapshot, child_progress, aggregate, state);
            if snapshot == previous {
                return;
            }
            self.mutate_entry(index, |entry| {
                let TranscriptEntry::DelegateSwarm { component } = entry else {
                    return false;
                };
                component.update(snapshot)
            });
        }
    }

    /// Upsert a workflow card by workflow ID.
    pub fn upsert_workflow(&mut self, snapshot: WorkflowSnapshot) {
        let id = snapshot.id.0.clone();
        let existing_index = self
            .entries
            .iter()
            .position(|entry| matches!(entry, TranscriptEntry::Workflow { component } if component.id() == id));
        if let Some(index) = existing_index {
            if self.entries[index].finalization() == Finalization::Finalized
                && snapshot.state == neo_agent_core::workflow::WorkflowState::Running
            {
                return;
            }
            self.mutate_entry(index, |entry| {
                let TranscriptEntry::Workflow { component } = entry else {
                    return false;
                };
                component.update(snapshot)
            });
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

    #[must_use]
    pub fn entry_ids(&self) -> &[TranscriptEntryId] {
        &self.entry_ids
    }

    #[must_use]
    pub fn entry_revisions(&self) -> &[u64] {
        &self.entry_revisions
    }

    #[must_use]
    pub fn entry_finalization(&self, index: usize) -> Option<Finalization> {
        let entry = self.entries.get(index)?;
        if self.active_assistant == Some(index) {
            Some(Finalization::Live)
        } else {
            Some(entry.finalization())
        }
    }

    pub fn finalize_interrupted_live_entries(&mut self) -> bool {
        let mut changed = self.live_model_attempt.take().is_some()
            || self.active_assistant.is_some()
            || self.active_thinking.is_some();
        self.finish_assistant();
        self.finish_thinking();

        for index in 0..self.entries.len() {
            changed |= self.mutate_entry(index, TranscriptEntry::interrupt);
        }
        changed
    }

    pub fn mutate_entry(
        &mut self,
        index: usize,
        mutate: impl FnOnce(&mut TranscriptEntry) -> bool,
    ) -> bool {
        let changed = match self.entries.get_mut(index) {
            Some(entry) => mutate(entry),
            None => return false,
        };
        if changed {
            self.touch_entry(index);
        }
        changed
    }

    pub(crate) fn invalidate_render_cache(&mut self) {
        self.invalidate_all_cache();
    }

    pub fn tick_live_entries(&mut self, now_ms: u64) -> bool {
        // Fast path: if no live-capable entries exist, skip the full scan.
        // This avoids an O(n) iteration over all entries every 50ms tick
        // when there are no delegates, MCP connections, or streaming blocks.
        if !self.has_live_entries() {
            return false;
        }
        let mut changed = false;
        for index in 0..self.entries.len() {
            let entry_changed = self.entries[index].on_render_tick(now_ms);
            if entry_changed {
                self.touch_entry(index);
                changed = true;
            }
        }
        changed
    }

    #[must_use]
    pub fn has_live_entries(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry,
                TranscriptEntry::Delegate { .. }
                    | TranscriptEntry::DelegateGroup { .. }
                    | TranscriptEntry::DelegateSwarm { .. }
                    | TranscriptEntry::McpStartupStatus { .. }
            ) || matches!(
                entry,
                TranscriptEntry::RetryStatus { data } if data.phase != RetryPhase::Exhausted
            )
        })
    }

    /// Remove the entry at `index`, shifting later entries down. Returns the
    /// removed entry. Used to pop a queued follow-up when it is promoted to a
    /// steer.
    pub fn remove(&mut self, index: usize) -> Option<TranscriptEntry> {
        if index >= self.entries.len() {
            return None;
        }
        let entry = self.entries.remove(index);
        self.entry_ids.remove(index);
        self.entry_revisions.remove(index);
        if index < self.render_cache.len() {
            self.render_cache.remove(index);
        }
        self.active_assistant = adjusted_index_after_remove(self.active_assistant, index);
        self.active_thinking = adjusted_index_after_remove(self.active_thinking, index);
        if let Some((_, start)) = &mut self.live_model_attempt
            && *start > index
        {
            *start -= 1;
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

    fn mark_visible_boundary(&mut self) {
        self.finish_thinking();
    }

    // ── Render cache management ───────────────────────────────────────────

    fn append_entry(&mut self, entry: TranscriptEntry) -> usize {
        let index = self.entries.len();
        let id = self.allocate_entry_id();
        self.entries.push(entry);
        self.entry_ids.push(id);
        self.entry_revisions.push(0);
        self.render_cache.push(None);
        self.mark_dirty_from(index);
        index
    }

    fn insert_entry(&mut self, index: usize, entry: TranscriptEntry) {
        let id = self.allocate_entry_id();
        self.entries.insert(index, entry);
        self.entry_ids.insert(index, id);
        self.entry_revisions.insert(index, 0);
        self.render_cache.insert(index, None);
        if let Some(active) = &mut self.active_assistant
            && *active >= index
        {
            *active += 1;
        }
        if let Some(active) = &mut self.active_thinking
            && *active >= index
        {
            *active += 1;
        }
        if let Some((_, start)) = &mut self.live_model_attempt
            && *start >= index
        {
            *start += 1;
        }
        self.mark_dirty_from(index);
    }

    fn allocate_entry_id(&mut self) -> TranscriptEntryId {
        let id = TranscriptEntryId(self.next_entry_id);
        self.next_entry_id = self
            .next_entry_id
            .checked_add(1)
            .expect("transcript entry ID space exhausted");
        id
    }

    fn touch_entry(&mut self, index: usize) {
        if let Some(revision) = self.entry_revisions.get_mut(index) {
            *revision = revision
                .checked_add(1)
                .expect("transcript entry revision space exhausted");
        }
        self.invalidate_cache(index);
    }

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

        let lines = self.render_entry_lines(
            index,
            width,
            theme,
            activity_frame,
            ImageRenderPolicy::default(),
            TerminalImageCapabilities::default(),
        );

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
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
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

        let lines = self.render_entry_lines(
            index,
            width,
            theme,
            activity_frame,
            image_render_policy,
            image_capabilities,
        );
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
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
    ) -> Vec<Line> {
        match self.entries.get(index) {
            Some(entry) => entry.render_with_image_context(
                width,
                theme,
                activity_frame,
                image_render_policy,
                image_capabilities,
            ),
            None => Vec::new(),
        }
    }
}

fn adjusted_index_after_remove(active: Option<usize>, removed: usize) -> Option<usize> {
    active.and_then(|index| {
        (index != removed).then_some(if index > removed { index - 1 } else { index })
    })
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
    if current.state.is_terminal() && !incoming.state.is_terminal() {
        return current.clone();
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
    if swarm_snapshot_is_terminal(current) && !swarm_snapshot_is_terminal(&incoming) {
        return current.clone();
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

fn swarm_snapshot_is_terminal(snapshot: &SwarmSnapshot) -> bool {
    snapshot
        .children
        .iter()
        .all(|child| child.agent.state.is_terminal())
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

        let first = store.render_entry_ansi_cached(
            0,
            80,
            &theme,
            0,
            ImageRenderPolicy::default(),
            TerminalImageCapabilities::default(),
        );

        assert!(first.iter().any(|line| line.contains("cached answer")));
        let cached = store.render_cache[0].as_ref().expect("cached render");
        assert_eq!(cached.ansi_lines, first);
        assert_eq!(
            store.render_entry_ansi_cached(
                0,
                80,
                &theme,
                99,
                ImageRenderPolicy::default(),
                TerminalImageCapabilities::default(),
            ),
            first
        );
    }
}
