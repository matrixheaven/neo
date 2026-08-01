use std::collections::{BTreeMap, VecDeque};

use crate::primitive::Finalization;
use crate::primitive::theme::TuiTheme;
use crate::terminal_image::{ImageRenderPolicy, TerminalImageCapabilities};

use super::progressive::{ProgressiveFactPayload, render_progressive_fact};
use super::streaming_prefix::stable_prefix_len;
use super::{TranscriptEntry, TranscriptEntryId, TranscriptStore};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TranscriptBlockId {
    Entries(Vec<TranscriptEntryId>),
    Workflow {
        entry: TranscriptEntryId,
    },
    AssistantSegment {
        entry: TranscriptEntryId,
        source_start: usize,
        source_end: usize,
    },
}

impl TranscriptBlockId {
    fn first_owner(&self) -> Option<TranscriptEntryId> {
        match self {
            Self::Entries(ids) => ids.first().copied(),
            Self::Workflow { entry } => Some(*entry),
            Self::AssistantSegment { entry, .. } => Some(*entry),
        }
    }

    fn last_owner(&self) -> Option<TranscriptEntryId> {
        match self {
            Self::Entries(ids) => ids.last().copied(),
            Self::Workflow { entry } => Some(*entry),
            Self::AssistantSegment { entry, .. } => Some(*entry),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizedBlockProof {
    EntryRevisions(Vec<u64>),
    WorkflowTerminal {
        entry: TranscriptEntryId,
        revision: u64,
    },
    AssistantSource(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedBlock {
    pub id: TranscriptBlockId,
    pub proof: FinalizedBlockProof,
    pub lines: Vec<String>,
    pub separator_before: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptTerminalUpdate {
    pub history: Vec<FinalizedBlock>,
    pub live: Vec<String>,
    pub has_visible_animation: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TranscriptRenderOptions<'a> {
    width: usize,
    theme: &'a TuiTheme,
    activity_frame: usize,
    image_render_policy: ImageRenderPolicy,
    image_capabilities: TerminalImageCapabilities,
    live_budget: usize,
}

impl<'a> TranscriptRenderOptions<'a> {
    pub(super) const fn new(
        width: usize,
        theme: &'a TuiTheme,
        activity_frame: usize,
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
        live_budget: usize,
    ) -> Self {
        Self {
            width,
            theme,
            activity_frame,
            image_render_policy,
            image_capabilities,
            live_budget,
        }
    }
}

const MAX_DIAGNOSTICS: usize = 32;

#[derive(Debug, Clone)]
struct LiveBlock {
    lines: Vec<String>,
    animated_line_indices: Vec<usize>,
    separator_before: bool,
}

impl LiveBlock {
    fn without_header(lines: Vec<String>, animated: bool, separator_before: bool) -> Self {
        Self {
            animated_line_indices: (animated && !lines.is_empty())
                .then_some(0)
                .into_iter()
                .collect(),
            lines,
            separator_before,
        }
    }

    fn with_header(lines: Vec<String>, animated: bool, separator_before: bool) -> Self {
        Self {
            animated_line_indices: (animated && !lines.is_empty())
                .then_some(0)
                .into_iter()
                .collect(),
            lines,
            separator_before,
        }
    }

    fn with_detected_headers(
        lines: Vec<String>,
        animated_line_indices: Vec<usize>,
        separator_before: bool,
    ) -> Self {
        Self {
            lines,
            animated_line_indices,
            separator_before,
        }
    }
}

struct PresentationFrame {
    live_blocks: Vec<LiveBlock>,
    pending_history: Vec<FinalizedBlock>,
    rendered_tail_owner: Option<TranscriptEntryId>,
}

impl PresentationFrame {
    fn new(rendered_tail_owner: Option<TranscriptEntryId>) -> Self {
        Self {
            live_blocks: Vec::new(),
            pending_history: Vec::new(),
            rendered_tail_owner,
        }
    }

    fn finish(self, live_budget: usize) -> TranscriptTerminalUpdate {
        let blocks = bound_live_blocks(self.live_blocks, live_budget);
        let (live, has_visible_animation) = compose_live_blocks(blocks);
        TranscriptTerminalUpdate {
            history: self.pending_history,
            live,
            has_visible_animation,
        }
    }
}

/// Bound the mutable live area to `live_budget` rows. Stable history is never
/// omitted: the most recent whole blocks win and omitted mutable rows are
/// summarized by one count line. A single block taller than the budget keeps
/// its first (header) row plus the newest rows that fit.
fn bound_live_blocks(blocks: Vec<LiveBlock>, live_budget: usize) -> Vec<LiveBlock> {
    if live_budget == 0 {
        return Vec::new();
    }
    let blocks = blocks
        .into_iter()
        .filter(|block| !block.lines.is_empty())
        .collect::<Vec<_>>();
    let total = live_blocks_cost(&blocks);
    if total <= live_budget {
        return blocks;
    }

    let mut kept_start = blocks.len();
    for candidate_start in (0..blocks.len()).rev() {
        let candidate = &blocks[candidate_start..];
        let summary_cost = usize::from(candidate_start > 0)
            + usize::from(
                candidate_start > 0
                    && candidate
                        .first()
                        .is_some_and(|block| block.separator_before),
            );
        if live_blocks_cost(candidate).saturating_add(summary_cost) > live_budget {
            break;
        }
        kept_start = candidate_start;
    }

    if kept_start == blocks.len() {
        let mut newest = blocks.into_iter().next_back().expect("non-empty blocks");
        if newest.lines.len() > live_budget {
            let header = newest.lines.remove(0);
            let keep = live_budget.saturating_sub(1);
            newest.lines = newest
                .lines
                .split_off(newest.lines.len().saturating_sub(keep));
            newest.lines.insert(0, header);
        }
        newest.separator_before = false;
        return vec![newest];
    }

    let omitted = total.saturating_sub(live_blocks_cost(&blocks[kept_start..]));
    let mut kept = blocks.into_iter().skip(kept_start).collect::<Vec<_>>();
    kept.insert(
        0,
        LiveBlock {
            lines: vec![format!("… {omitted} more rows")],
            animated_line_indices: Vec::new(),
            separator_before: false,
        },
    );
    kept
}

fn live_blocks_cost(blocks: &[LiveBlock]) -> usize {
    let mut has_preceding_visible = false;
    blocks
        .iter()
        .map(|block| {
            if block.lines.is_empty() {
                return 0;
            }
            let cost =
                block.lines.len() + usize::from(has_preceding_visible && block.separator_before);
            has_preceding_visible = true;
            cost
        })
        .sum()
}

#[derive(Debug, Clone, Default)]
pub(super) struct TranscriptPresentation {
    committed_entry_revisions: BTreeMap<TranscriptEntryId, u64>,
    assistant_offsets: BTreeMap<TranscriptEntryId, usize>,
    assistant_sources: BTreeMap<TranscriptEntryId, String>,
    acknowledged_tail_owner: Option<TranscriptEntryId>,
    diagnostics: VecDeque<String>,
}

impl TranscriptPresentation {
    pub(super) fn is_committed(&self, id: TranscriptEntryId) -> bool {
        self.committed_entry_revisions.contains_key(&id)
    }

    pub(super) fn render(
        &mut self,
        transcript: &mut TranscriptStore,
        options: TranscriptRenderOptions<'_>,
    ) -> TranscriptTerminalUpdate {
        let mut frame = PresentationFrame::new(self.acknowledged_tail_owner);
        let attempt_start = transcript.live_model_attempt_start();
        let blocking_index = blocking_dialog_index(transcript);
        let mut terminal_history_barrier = false;
        let mut index = 0;
        while index < transcript.entries().len() {
            if blocking_index == Some(index) {
                // The earliest unresolved approval owns the live focus; every
                // later row stays deferred until it resolves.
                let Some(id) = transcript.entry_ids().get(index).copied() else {
                    index += 1;
                    continue;
                };
                let Some(revision) = transcript.entry_revisions().get(index).copied() else {
                    index += 1;
                    continue;
                };
                render_entry(transcript, index, id, revision, true, options, &mut frame);
                break;
            }
            // Everything from the live model attempt start is rollback-able
            // attempt content: it renders bounded live, never as history.
            let in_attempt = attempt_start.is_some_and(|start| index >= start);
            let blocked = in_attempt || terminal_history_barrier;
            let Some(id) = transcript.entry_ids().get(index).copied() else {
                index += 1;
                continue;
            };
            let Some(revision) = transcript.entry_revisions().get(index).copied() else {
                index += 1;
                continue;
            };
            if let Some(expected_revision) = self.committed_entry_revisions.get(&id).copied() {
                if expected_revision != revision {
                    self.record_diagnostic(format!(
                        "committed entry {id:?} changed from revision {expected_revision} to {revision}"
                    ));
                }
                index += 1;
                continue;
            }

            if let Some(TranscriptEntry::AssistantMessage { content }) =
                transcript.entries().get(index)
            {
                let finalization = transcript.entry_finalization(index);
                self.render_assistant_entry(
                    id,
                    content,
                    finalization,
                    blocked,
                    options,
                    &mut frame,
                );
                index += 1;
                continue;
            }

            if matches!(
                transcript.entries().get(index),
                Some(TranscriptEntry::Workflow { .. })
            ) {
                terminal_history_barrier |= render_workflow_entry(
                    transcript, index, id, revision, blocked, options, &mut frame,
                );
                index += 1;
                continue;
            }

            if let Some(next_index) =
                self.render_tool_run(transcript, index, blocked, options, &mut frame)
            {
                index = next_index;
                continue;
            }

            render_entry(
                transcript, index, id, revision, blocked, options, &mut frame,
            );
            index += 1;
        }
        frame.finish(options.live_budget)
    }

    fn render_assistant_entry(
        &mut self,
        id: TranscriptEntryId,
        content: &str,
        finalization: Option<Finalization>,
        blocked: bool,
        options: TranscriptRenderOptions<'_>,
        frame: &mut PresentationFrame,
    ) {
        let source_mismatch = self
            .assistant_sources
            .get(&id)
            .is_some_and(|source| content.get(..source.len()) != Some(source.as_str()));
        if source_mismatch {
            self.record_diagnostic(format!(
                "committed assistant source changed for entry {id:?}"
            ));
            return;
        }

        let source_start = self.assistant_offsets.get(&id).copied().unwrap_or(0);
        // Markdown can become temporarily less decidable when a later delta
        // introduces a reference definition or footnote. An acknowledged
        // prefix is immutable, so this boundary must never rewind.
        let source_end = if finalization == Some(Finalization::Finalized) {
            content.len()
        } else {
            stable_prefix_len(content)
                .max(source_start)
                .min(content.len())
        };
        if blocked {
            if source_start < content.len() {
                let lines = render_assistant_segment(
                    &content[source_start..],
                    options.width,
                    options.theme,
                    source_start > 0,
                );
                let separator_before = advance_semantic_owner(
                    &mut frame.rendered_tail_owner,
                    Some(id),
                    Some(id),
                    !lines.is_empty(),
                );
                frame
                    .live_blocks
                    .push(LiveBlock::without_header(lines, false, separator_before));
            }
        } else if source_end > source_start {
            let source = &content[source_start..source_end];
            let block_id = TranscriptBlockId::AssistantSegment {
                entry: id,
                source_start,
                source_end,
            };
            let lines =
                render_assistant_segment(source, options.width, options.theme, source_start > 0);
            let separator_before = advance_semantic_owner(
                &mut frame.rendered_tail_owner,
                block_id.first_owner(),
                block_id.last_owner(),
                !lines.is_empty(),
            );
            frame.pending_history.push(FinalizedBlock {
                id: block_id,
                proof: FinalizedBlockProof::AssistantSource(source.to_owned()),
                lines,
                separator_before,
            });
        }
        if !blocked && finalization == Some(Finalization::Live) && source_end < content.len() {
            let lines = render_assistant_segment(
                &content[source_end..],
                options.width,
                options.theme,
                source_end > 0,
            );
            let separator_before = advance_semantic_owner(
                &mut frame.rendered_tail_owner,
                Some(id),
                Some(id),
                !lines.is_empty(),
            );
            frame
                .live_blocks
                .push(LiveBlock::without_header(lines, false, separator_before));
        }
    }

    fn render_tool_run(
        &self,
        transcript: &TranscriptStore,
        index: usize,
        blocked: bool,
        options: TranscriptRenderOptions<'_>,
        frame: &mut PresentationFrame,
    ) -> Option<usize> {
        let Some(TranscriptEntry::ToolRun { component }) = transcript.entries().get(index) else {
            return None;
        };
        if component.workflow_origin().is_some() {
            return Some(index + 1);
        }
        if transcript.is_tool_run_suppressed(component.id()) {
            return Some(index + 1);
        }

        let end = tool_run_end(self, transcript, index);
        let indexes = index..end;
        let ids = indexes
            .clone()
            .filter_map(|tool_index| transcript.entry_ids().get(tool_index).copied())
            .collect::<Vec<_>>();
        let revisions = indexes
            .clone()
            .filter_map(|tool_index| transcript.entry_revisions().get(tool_index).copied())
            .collect::<Vec<_>>();
        let all_finalized = indexes.clone().all(|tool_index| {
            transcript.entry_finalization(tool_index) == Some(Finalization::Finalized)
        });
        let rendered_tools = render_tool_entries(transcript, indexes, options.width, options.theme);
        let lines = rendered_tools.lines;
        let id = TranscriptBlockId::Entries(ids);
        let separator_before = advance_semantic_owner(
            &mut frame.rendered_tail_owner,
            id.first_owner(),
            id.last_owner(),
            !lines.is_empty(),
        );
        let block = FinalizedBlock {
            id,
            proof: FinalizedBlockProof::EntryRevisions(revisions),
            lines,
            separator_before,
        };
        if all_finalized && !blocked {
            frame.pending_history.push(block);
        } else {
            frame.live_blocks.push(LiveBlock::with_detected_headers(
                block.lines,
                rendered_tools.animated_header_indices,
                block.separator_before,
            ));
        }
        Some(end)
    }

    pub(super) fn acknowledge(&mut self, blocks: &[FinalizedBlock]) {
        for block in blocks {
            match (&block.id, &block.proof) {
                (
                    TranscriptBlockId::Entries(ids),
                    FinalizedBlockProof::EntryRevisions(revisions),
                ) if ids.len() == revisions.len() => {
                    self.committed_entry_revisions
                        .extend(ids.iter().copied().zip(revisions.iter().copied()));
                }
                (
                    TranscriptBlockId::Workflow { entry: block_entry },
                    FinalizedBlockProof::WorkflowTerminal { entry, revision },
                ) if block_entry == entry => {
                    self.committed_entry_revisions.insert(*entry, *revision);
                }
                (
                    TranscriptBlockId::AssistantSegment {
                        entry,
                        source_start,
                        source_end,
                    },
                    FinalizedBlockProof::AssistantSource(source),
                ) => {
                    let mut source_mismatch = false;
                    {
                        let committed_source = self.assistant_sources.entry(*entry).or_default();
                        if *source_start == committed_source.len() {
                            committed_source.push_str(source);
                        } else if committed_source.get(*source_start..*source_end)
                            != Some(source.as_str())
                        {
                            source_mismatch = true;
                        }
                    }
                    if source_mismatch {
                        self.record_diagnostic(format!(
                            "non-contiguous assistant acknowledgement for entry {entry:?}"
                        ));
                    }
                    self.assistant_offsets
                        .entry(*entry)
                        .and_modify(|offset| *offset = (*offset).max(*source_end))
                        .or_insert(*source_end);
                }
                _ => self.record_diagnostic(format!(
                    "presentation proof does not match block identity: {:?}",
                    block.id
                )),
            }
            if !block.lines.is_empty()
                && let Some(owner) = block.id.last_owner()
            {
                self.acknowledged_tail_owner = Some(owner);
            }
        }
    }

    fn record_diagnostic(&mut self, diagnostic: String) {
        if self
            .diagnostics
            .iter()
            .any(|current| current == &diagnostic)
        {
            return;
        }
        if self.diagnostics.len() == MAX_DIAGNOSTICS {
            self.diagnostics.pop_front();
        }
        self.diagnostics.push_back(diagnostic);
    }
}

/// Index of the earliest unresolved blocking dialog (approval or question),
/// if any. That entry is the interactive focus: later history and later facts
/// stay deferred until it resolves so canonical transcript order is preserved.
fn blocking_dialog_index(transcript: &TranscriptStore) -> Option<usize> {
    transcript.entries().iter().position(|entry| {
        matches!(entry, TranscriptEntry::ApprovalPrompt(data) if data.is_pending())
            || matches!(entry, TranscriptEntry::QuestionPrompt(data) if data.is_pending())
    })
}

fn advance_semantic_owner(
    tail_owner: &mut Option<TranscriptEntryId>,
    first_owner: Option<TranscriptEntryId>,
    last_owner: Option<TranscriptEntryId>,
    has_visible_rows: bool,
) -> bool {
    if !has_visible_rows {
        return false;
    }
    let separator_before = matches!(
        (*tail_owner, first_owner),
        (Some(tail), Some(first)) if tail != first
    );
    if let Some(last_owner) = last_owner {
        *tail_owner = Some(last_owner);
    }
    separator_before
}

fn render_assistant_segment(
    source: &str,
    width: usize,
    theme: &TuiTheme,
    continuation: bool,
) -> Vec<String> {
    let first_prefix = if continuation { "  " } else { "\u{25cf} " };
    let mut lines = crate::markdown::render_markdown(source, width, theme, first_prefix, "  ")
        .into_iter()
        .map(|line| line.to_ansi())
        .collect();
    super::pane::trim_ansi_transcript_block(&mut lines);
    lines
}

fn render_entry(
    transcript: &mut TranscriptStore,
    index: usize,
    id: TranscriptEntryId,
    revision: u64,
    blocked: bool,
    options: TranscriptRenderOptions<'_>,
    frame: &mut PresentationFrame,
) {
    let block_id = TranscriptBlockId::Entries(vec![id]);
    let mut lines = transcript.render_entry_ansi_cached(
        index,
        options.width,
        options.theme,
        options.activity_frame,
        options.image_render_policy,
        options.image_capabilities,
    );
    super::pane::trim_ansi_transcript_block(&mut lines);
    match transcript.entry_finalization(index) {
        Some(Finalization::Finalized) if !blocked => {
            if let Some(terminal_lines) =
                render_delegate_family_terminal(transcript, index, id, options)
            {
                lines = terminal_lines;
            }
            if lines.is_empty() {
                return;
            }
            let separator_before = advance_semantic_owner(
                &mut frame.rendered_tail_owner,
                block_id.first_owner(),
                block_id.last_owner(),
                !lines.is_empty(),
            );
            frame.pending_history.push(FinalizedBlock {
                id: block_id,
                proof: FinalizedBlockProof::EntryRevisions(vec![revision]),
                lines,
                separator_before,
            });
        }
        Some(_) => {
            let separator_before = advance_semantic_owner(
                &mut frame.rendered_tail_owner,
                block_id.first_owner(),
                block_id.last_owner(),
                !lines.is_empty(),
            );
            frame.live_blocks.push(LiveBlock::with_header(
                lines,
                transcript
                    .entries()
                    .get(index)
                    .is_some_and(TranscriptEntry::has_visible_animation),
                separator_before,
            ));
        }
        None => {}
    }
}

fn render_delegate_family_terminal(
    transcript: &TranscriptStore,
    index: usize,
    id: TranscriptEntryId,
    options: TranscriptRenderOptions<'_>,
) -> Option<Vec<String>> {
    let entry = transcript.entries().get(index)?;
    if let TranscriptEntry::DelegateSwarm { component } = entry {
        let mut lines = component
            .terminal_summary(options.width, options.theme)
            .into_iter()
            .map(|line| line.to_ansi())
            .collect::<Vec<_>>();
        super::pane::trim_ansi_transcript_block(&mut lines);
        return Some(lines);
    }
    let summary = match entry {
        TranscriptEntry::Delegate { component } => {
            component.terminal_summary(options.width, options.theme)
        }
        TranscriptEntry::DelegateGroup { component } => {
            component.terminal_summary(options.width, options.theme)
        }
        _ => return None,
    };
    let facts = transcript
        .progressive_facts()
        .iter()
        .filter(|fact| fact.id.entry() == id)
        .collect::<Vec<_>>();
    if facts.is_empty() {
        return None;
    }

    let mut lines = summary
        .first()
        .into_iter()
        .map(|line| line.to_ansi())
        .collect::<Vec<_>>();
    let mut terminal_facts = facts
        .iter()
        .copied()
        .filter(|fact| {
            matches!(
                &fact.payload,
                ProgressiveFactPayload::ChildAgent(_) | ProgressiveFactPayload::SwarmItem(_)
            )
        })
        .collect::<Vec<_>>();
    terminal_facts.sort_by_key(|fact| fact_child_rank(entry, fact));
    let mut tool_facts = facts
        .iter()
        .copied()
        .filter(|fact| matches!(&fact.payload, ProgressiveFactPayload::ChildTool(_)))
        .collect::<Vec<_>>();
    tool_facts.sort_by_key(|fact| {
        let activity_index = match &fact.payload {
            ProgressiveFactPayload::ChildTool(tool) => tool.activity_index,
            _ => usize::MAX,
        };
        (fact_child_rank(entry, fact), activity_index)
    });

    for fact in &terminal_facts {
        let rendered = render_progressive_fact(fact, options.width, options.theme);
        lines.extend(rendered.first().cloned());
        for tool in &tool_facts {
            if facts_share_child_run(fact, tool) {
                lines.extend(render_progressive_fact(tool, options.width, options.theme));
            }
        }
        lines.extend(rendered.into_iter().skip(1));
    }
    for tool in &tool_facts {
        if !terminal_facts
            .iter()
            .any(|fact| facts_share_child_run(fact, tool))
        {
            lines.extend(render_progressive_fact(tool, options.width, options.theme));
        }
    }
    lines.extend(summary.iter().skip(1).map(|line| line.to_ansi()));
    super::pane::trim_ansi_transcript_block(&mut lines);
    Some(lines)
}

fn facts_share_child_run(terminal: &super::ProgressiveFact, tool: &super::ProgressiveFact) -> bool {
    let ProgressiveFactPayload::ChildTool(tool) = &tool.payload else {
        return false;
    };
    match &terminal.payload {
        ProgressiveFactPayload::ChildAgent(agent) => {
            agent.agent_id == tool.agent_id && agent.run_count == tool.run_count
        }
        ProgressiveFactPayload::SwarmItem(item) => {
            item.agent_id == tool.agent_id && item.run_count == tool.run_count
        }
        ProgressiveFactPayload::ChildTool(_) => false,
    }
}

fn fact_child_rank(entry: &TranscriptEntry, fact: &super::ProgressiveFact) -> usize {
    let agent_id = match &fact.payload {
        ProgressiveFactPayload::ChildTool(fact) => Some(fact.agent_id.as_str()),
        ProgressiveFactPayload::ChildAgent(fact) => Some(fact.agent_id.as_str()),
        ProgressiveFactPayload::SwarmItem(_) => None,
    };
    match entry {
        TranscriptEntry::Delegate { .. } => 0,
        TranscriptEntry::DelegateGroup { component } => agent_id
            .and_then(|agent_id| {
                component
                    .snapshots()
                    .iter()
                    .position(|snapshot| snapshot.id.as_str() == agent_id)
            })
            .unwrap_or(usize::MAX),
        TranscriptEntry::DelegateSwarm { component } => match &fact.payload {
            ProgressiveFactPayload::SwarmItem(fact) => fact.item_index,
            ProgressiveFactPayload::ChildTool(fact) => component
                .snapshot()
                .children
                .iter()
                .find(|child| child.agent.id.as_str() == fact.agent_id)
                .map_or(usize::MAX, |child| child.item_index),
            ProgressiveFactPayload::ChildAgent(_) => usize::MAX,
        },
        _ => usize::MAX,
    }
}

fn render_workflow_entry(
    transcript: &TranscriptStore,
    index: usize,
    id: TranscriptEntryId,
    revision: u64,
    blocked: bool,
    options: TranscriptRenderOptions<'_>,
    frame: &mut PresentationFrame,
) -> bool {
    let Some(TranscriptEntry::Workflow { component }) = transcript.entries().get(index) else {
        return false;
    };
    let finalized = transcript.entry_finalization(index) == Some(Finalization::Finalized);
    let separator_before = matches!(frame.rendered_tail_owner, Some(owner) if owner != id);
    let available_rows = options
        .live_budget
        .saturating_sub(usize::from(separator_before));
    let group = super::workflow_group::render_workflow_group(
        component,
        options.width,
        available_rows,
        options.theme,
    );
    let has_visible_animation = group.has_visible_animation;
    let mut lines = group
        .into_lines()
        .into_iter()
        .map(|line| line.to_ansi())
        .collect::<Vec<_>>();
    super::pane::trim_ansi_transcript_block(&mut lines);
    if lines.is_empty() {
        return finalized && !blocked;
    }
    let separator_before = advance_semantic_owner(
        &mut frame.rendered_tail_owner,
        Some(id),
        Some(id),
        !lines.is_empty(),
    );
    if finalized && !blocked {
        frame.pending_history.push(FinalizedBlock {
            id: TranscriptBlockId::Workflow { entry: id },
            proof: FinalizedBlockProof::WorkflowTerminal {
                entry: id,
                revision,
            },
            lines,
            separator_before,
        });
    } else {
        frame.live_blocks.push(LiveBlock::with_header(
            lines,
            has_visible_animation,
            separator_before,
        ));
    }
    false
}

fn tool_run_end(
    presentation: &TranscriptPresentation,
    transcript: &TranscriptStore,
    start: usize,
) -> usize {
    let mut end = start;
    while end < transcript.entries().len() {
        let Some(TranscriptEntry::ToolRun { component }) = transcript.entries().get(end) else {
            break;
        };
        if transcript.is_tool_run_suppressed(component.id()) {
            break;
        }
        let Some(id) = transcript.entry_ids().get(end) else {
            break;
        };
        if presentation.committed_entry_revisions.contains_key(id) {
            break;
        }
        end += 1;
    }
    end.max(start + 1)
}

struct RenderedToolEntries {
    lines: Vec<String>,
    animated_header_indices: Vec<usize>,
}

fn render_tool_entries(
    transcript: &TranscriptStore,
    indexes: std::ops::Range<usize>,
    width: usize,
    theme: &TuiTheme,
) -> RenderedToolEntries {
    let mut tools = indexes
        .filter_map(|index| match transcript.entries().get(index) {
            Some(TranscriptEntry::ToolRun { component }) => Some(component.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rendered = super::chrome_render::render_ordered_tools(&mut tools, width, theme);
    let mut lines = rendered
        .lines
        .into_iter()
        .map(|line| line.to_ansi())
        .collect();
    super::pane::trim_ansi_transcript_block(&mut lines);
    RenderedToolEntries {
        lines,
        animated_header_indices: rendered.animated_header_indices,
    }
}

fn compose_live_blocks(blocks: Vec<LiveBlock>) -> (Vec<String>, bool) {
    let mut has_visible_animation = false;
    let mut lines = Vec::new();
    for block in blocks {
        if block.lines.is_empty() {
            continue;
        }
        if !lines.is_empty() && block.separator_before {
            lines.push(String::new());
        }
        let mut is_animated = vec![false; block.lines.len()];
        for index in block.animated_line_indices {
            if let Some(slot) = is_animated.get_mut(index) {
                *slot = true;
            }
        }
        for (line, animated) in block.lines.into_iter().zip(is_animated) {
            has_visible_animation |= animated;
            lines.push(line);
        }
    }
    (lines, has_visible_animation)
}

#[cfg(test)]
mod tests {
    use neo_agent_core::multi_agent::MultiAgentRuntime;

    use super::{TranscriptPresentation, TranscriptRenderOptions};
    use crate::primitive::theme::TuiTheme;
    use crate::terminal_image::{ImageRenderPolicy, TerminalImageCapabilities};
    use crate::transcript::{TranscriptBlockId, TranscriptEntry, TranscriptPane, TranscriptStore};

    #[test]
    fn finalized_entries_wait_for_ack_and_live_entries_stay_live() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.push_status("ready");
        pane.start_assistant_message();
        pane.append_assistant_delta("partial");

        let first = pane.render_terminal_update(80, 12);
        let history_text = first
            .history
            .iter()
            .flat_map(|block| block.lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history_text.contains("ready"));
        assert!(first.live.join("\n").contains("partial"));

        let retry = pane.render_terminal_update(80, 12);
        assert_eq!(retry.history, first.history, "unacked history must retry");

        pane.acknowledge_history(&first.history);
        assert!(pane.render_terminal_update(80, 12).history.is_empty());
    }

    #[test]
    fn ordinary_living_card_does_not_block_later_stable_history() {
        let runtime = MultiAgentRuntime::new();
        let running = runtime.start_foreground_delegate_for_test("background task");
        let id = running.id.clone();
        let mut pane = TranscriptPane::new(80, 12);
        pane.transcript_mut().upsert_delegate(1, running);
        pane.push_status("later status");

        // Ordinary mutable entries are bounded live, not commit barriers: the
        // unrelated stable fact commits while the delegate card stays live.
        let running_update = pane.render_terminal_update(80, 12);
        let history = running_update
            .history
            .iter()
            .flat_map(|block| block.lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history.contains("later status"), "history:\n{history}");
        assert!(!history.contains("background task"));
        let running_live = running_update.live.join("\n");
        assert!(
            running_live.contains("background task"),
            "live:\n{running_live}"
        );

        // Acknowledge the stable fact, then complete the delegate.
        pane.acknowledge_history(&running_update.history);
        pane.transcript_mut()
            .upsert_delegate(1, runtime.complete_delegate_for_test(&id, "done"));

        // Completion commits the canonical delegate card once, without a
        // duplicate of the already-committed later status.
        let completed_update = pane.render_terminal_update(80, 12);
        let blocks = completed_update
            .history
            .iter()
            .map(|block| block.lines.join("\n"))
            .collect::<Vec<_>>();

        assert_eq!(blocks.len(), 1, "no duplicate replay: {blocks:?}");
        assert!(blocks[0].contains("background task"));
        assert!(completed_update.live.is_empty());
    }

    #[test]
    fn assistant_stable_prefix_never_rewinds_when_markdown_becomes_reference_based() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.start_assistant_message();
        pane.append_assistant_delta("first paragraph\n\nsecond paragraph");

        let first = pane.render_terminal_update(80, 12);
        assert!(!first.history.is_empty(), "stable paragraph should commit");
        pane.acknowledge_history(&first.history);

        pane.append_assistant_delta("\n\n[target]: /later");
        let update = pane.render_terminal_update(80, 12);
        let live = update.live.join("\n");

        assert!(
            !live.contains("first paragraph"),
            "stable prefix replayed: {live}"
        );
        assert!(
            live.contains("second paragraph"),
            "live tail missing: {live}"
        );
    }

    #[test]
    fn suppressed_living_tool_stays_bounded_while_later_history_commits() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.transcript_mut()
            .push_tool_run("delegate-tool", "Delegate", Some("{}".to_owned()));
        pane.transcript_mut().suppress_tool_run("delegate-tool");
        pane.push_status("later status");

        let suppressed = pane.render_terminal_update(80, 12);
        let history = suppressed
            .history
            .iter()
            .flat_map(|block| block.lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(history.contains("later status"), "history:\n{history}");
        assert!(!history.contains("Delegate"));

        pane.transcript_mut().unsuppress_tool_run("delegate-tool");
        let visible = pane.render_terminal_update(80, 12);
        let live = visible.live.join("\n");
        assert!(live.contains("Delegate"), "restored tool card: {live}");
        let visible_history = visible
            .history
            .iter()
            .flat_map(|block| block.lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(visible_history.contains("later status"));
        assert!(!visible_history.contains("Delegate"));
    }

    #[test]
    fn finalized_suppressed_tool_stays_out_of_history() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.transcript_mut()
            .push_tool_run("delegate-tool", "Delegate", Some("{}".to_owned()));
        pane.transcript_mut().suppress_tool_run("delegate-tool");
        pane.push_status("later status");

        assert!(pane.transcript_mut().mutate_tool("delegate-tool", |tool| {
            tool.set_terminal_status(
                crate::shell::ToolStatusKind::Succeeded,
                Some("absorbed".to_owned()),
            )
        }));
        let released = pane.render_terminal_update(80, 12);
        let history = released
            .history
            .iter()
            .flat_map(|block| block.lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");

        assert!(history.contains("later status"));
        assert!(!history.contains("Delegate"));
        assert!(released.live.is_empty());
    }

    #[test]
    fn adjacent_tools_commit_as_one_block_after_every_tool_finishes() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "read-1".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "one.rs" }),

            workflow_origin: None,
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "read-1".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("one"),

            workflow_origin: None,
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "read-2".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "two.rs" }),

            workflow_origin: None,
        });

        let running = pane.render_terminal_update(80, 12);
        assert!(running.history.is_empty());
        assert!(!running.live.is_empty());

        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "read-2".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("two"),

            workflow_origin: None,
        });
        let finished = pane.render_terminal_update(80, 12);

        assert_eq!(finished.history.len(), 1);
        assert!(matches!(
            &finished.history[0].id,
            TranscriptBlockId::Entries(ids) if ids.len() == 2
        ));
    }

    #[test]
    fn committed_revision_mismatch_is_diagnosed_once_without_replay() {
        let mut transcript = TranscriptStore::new();
        transcript.push(TranscriptEntry::status("ready"));
        let mut presentation = TranscriptPresentation::default();
        let theme = TuiTheme::default();
        let options = TranscriptRenderOptions::new(
            80,
            &theme,
            0,
            ImageRenderPolicy::default(),
            TerminalImageCapabilities::default(),
            8,
        );
        let first = presentation.render(&mut transcript, options);
        presentation.acknowledge(&first.history);

        assert!(transcript.mutate_entry(0, |entry| {
            *entry = TranscriptEntry::status("changed after commit");
            true
        }));
        for _ in 0..2 {
            let update = presentation.render(&mut transcript, options);
            assert!(update.history.is_empty());
        }

        assert_eq!(presentation.diagnostics.len(), 1);
    }
}
