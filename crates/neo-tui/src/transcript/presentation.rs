use std::collections::{BTreeMap, VecDeque};

use crate::primitive::Finalization;
use crate::primitive::theme::TuiTheme;
use crate::terminal_image::{ImageRenderPolicy, TerminalImageCapabilities};

use super::streaming_prefix::stable_prefix_len;
use super::{TranscriptEntry, TranscriptEntryId, TranscriptStore};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TranscriptBlockId {
    Entries(Vec<TranscriptEntryId>),
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
            Self::AssistantSegment { entry, .. } => Some(*entry),
        }
    }

    fn last_owner(&self) -> Option<TranscriptEntryId> {
        match self {
            Self::Entries(ids) => ids.last().copied(),
            Self::AssistantSegment { entry, .. } => Some(*entry),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizedBlockProof {
    EntryRevisions(Vec<u64>),
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

const MAX_DIAGNOSTICS: usize = 32;

#[derive(Debug, Clone)]
struct LiveBlock {
    lines: Vec<String>,
    header_indices: Vec<usize>,
    animated_line_indices: Vec<usize>,
    pinned_line_indices: Vec<usize>,
    separator_before: bool,
    atomic: bool,
}

impl LiveBlock {
    fn without_header(lines: Vec<String>, animated: bool, separator_before: bool) -> Self {
        let atomic = lines
            .iter()
            .any(|line| super::pane::ansi_line_is_image(line));
        Self {
            animated_line_indices: (animated && !lines.is_empty())
                .then_some(0)
                .into_iter()
                .collect(),
            lines,
            header_indices: Vec::new(),
            pinned_line_indices: Vec::new(),
            separator_before,
            atomic,
        }
    }

    fn with_header(
        lines: Vec<String>,
        animated: bool,
        pin_first_header: bool,
        separator_before: bool,
    ) -> Self {
        let atomic = lines
            .iter()
            .any(|line| super::pane::ansi_line_is_image(line));
        let header_indices = (!lines.is_empty()).then_some(0).into_iter().collect();
        let pinned_line_indices = (pin_first_header && !lines.is_empty())
            .then_some(0)
            .into_iter()
            .collect();
        Self {
            animated_line_indices: (animated && !lines.is_empty())
                .then_some(0)
                .into_iter()
                .collect(),
            lines,
            header_indices,
            pinned_line_indices,
            separator_before,
            atomic,
        }
    }

    fn with_detected_headers(
        lines: Vec<String>,
        animated_line_indices: Vec<usize>,
        pinned_line_indices: Vec<usize>,
        separator_before: bool,
    ) -> Self {
        let atomic = lines
            .iter()
            .any(|line| super::pane::ansi_line_is_image(line));
        let mut header_indices = Vec::new();
        let mut after_separator = true;
        for (index, line) in lines.iter().enumerate() {
            if line.is_empty() {
                after_separator = true;
            } else if after_separator {
                header_indices.push(index);
                after_separator = false;
            }
        }
        Self {
            lines,
            header_indices,
            animated_line_indices,
            pinned_line_indices,
            separator_before,
            atomic,
        }
    }
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

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "render carries the complete terminal presentation context and frontier ledger"
    )]
    pub(super) fn render(
        &mut self,
        transcript: &mut TranscriptStore,
        width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
        live_budget: usize,
    ) -> TranscriptTerminalUpdate {
        let mut update = TranscriptTerminalUpdate::default();
        let mut live_blocks = Vec::new();
        let mut pending_history = Vec::new();
        let mut commit_blocked = false;
        let mut rendered_tail_owner = self.acknowledged_tail_owner;
        let live_model_attempt_start = transcript.live_model_attempt_start();
        let mut index = 0;
        while index < transcript.entries().len() {
            commit_blocked |= live_model_attempt_start == Some(index);
            let Some(id) = transcript.entry_ids().get(index).copied() else {
                index += 1;
                continue;
            };
            let Some(revision) = transcript.entry_revisions().get(index).copied() else {
                index += 1;
                continue;
            };
            if let Some(TranscriptEntry::AssistantMessage { content }) =
                transcript.entries().get(index)
            {
                let source_mismatch = self
                    .assistant_sources
                    .get(&id)
                    .is_some_and(|source| content.get(..source.len()) != Some(source.as_str()));
                if source_mismatch {
                    self.record_diagnostic(format!(
                        "committed assistant source changed for entry {id:?}"
                    ));
                    index += 1;
                    continue;
                }
                let finalization = transcript.entry_finalization(index);
                let source_start = self.assistant_offsets.get(&id).copied().unwrap_or(0);
                // Markdown can become temporarily less decidable when a later
                // delta introduces a reference definition or footnote. A
                // previously acknowledged prefix is immutable, so the stable
                // boundary may advance or pause, but it must never rewind.
                let source_end = if finalization == Some(Finalization::Finalized) {
                    content.len()
                } else {
                    stable_prefix_len(content)
                        .max(source_start)
                        .min(content.len())
                };
                if commit_blocked {
                    if source_start < content.len() {
                        let lines = render_assistant_segment(
                            &content[source_start..],
                            width,
                            theme,
                            source_start > 0,
                        );
                        let separator_before = advance_semantic_owner(
                            &mut rendered_tail_owner,
                            Some(id),
                            Some(id),
                            !lines.is_empty(),
                        );
                        live_blocks.push(LiveBlock::without_header(lines, false, separator_before));
                    }
                } else if source_end > source_start {
                    let source = &content[source_start..source_end];
                    let id = TranscriptBlockId::AssistantSegment {
                        entry: id,
                        source_start,
                        source_end,
                    };
                    let lines = render_assistant_segment(source, width, theme, source_start > 0);
                    let separator_before = advance_semantic_owner(
                        &mut rendered_tail_owner,
                        id.first_owner(),
                        id.last_owner(),
                        !lines.is_empty(),
                    );
                    pending_history.push(FinalizedBlock {
                        id,
                        proof: FinalizedBlockProof::AssistantSource(source.to_owned()),
                        lines,
                        separator_before,
                    });
                }
                if !commit_blocked
                    && finalization == Some(Finalization::Live)
                    && source_end < content.len()
                {
                    let lines = render_assistant_segment(
                        &content[source_end..],
                        width,
                        theme,
                        source_end > 0,
                    );
                    let separator_before = advance_semantic_owner(
                        &mut rendered_tail_owner,
                        Some(id),
                        Some(id),
                        !lines.is_empty(),
                    );
                    live_blocks.push(LiveBlock::without_header(lines, false, separator_before));
                }
                commit_blocked |= finalization == Some(Finalization::Live);
                index += 1;
                continue;
            }

            if let Some(expected_revision) = self.committed_entry_revisions.get(&id).copied() {
                if expected_revision != revision {
                    self.record_diagnostic(format!(
                        "committed entry {id:?} changed from revision {expected_revision} to {revision}"
                    ));
                }
                index += 1;
                continue;
            }

            if let Some(TranscriptEntry::ToolRun { component }) = transcript.entries().get(index) {
                if transcript.is_tool_run_suppressed(component.id()) {
                    commit_blocked |=
                        transcript.entry_finalization(index) == Some(Finalization::Live);
                    index += 1;
                    continue;
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
                let rendered_tools = render_tool_entries(transcript, indexes.clone(), width, theme);
                let lines = rendered_tools.lines;
                let id = TranscriptBlockId::Entries(ids);
                let separator_before = advance_semantic_owner(
                    &mut rendered_tail_owner,
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
                if all_finalized && !commit_blocked {
                    pending_history.push(block);
                } else {
                    live_blocks.push(LiveBlock::with_detected_headers(
                        block.lines,
                        rendered_tools.animated_header_indices,
                        rendered_tools.live_header_indices,
                        block.separator_before,
                    ));
                }
                commit_blocked |= !all_finalized;
                index = end;
                continue;
            }

            let block_id = TranscriptBlockId::Entries(vec![id]);

            let mut lines = transcript.render_entry_ansi_cached(
                index,
                width,
                theme,
                activity_frame,
                image_render_policy,
                image_capabilities,
            );
            super::pane::trim_ansi_transcript_block(&mut lines);
            match transcript.entry_finalization(index) {
                Some(Finalization::Finalized) if !commit_blocked => {
                    let separator_before = advance_semantic_owner(
                        &mut rendered_tail_owner,
                        block_id.first_owner(),
                        block_id.last_owner(),
                        !lines.is_empty(),
                    );
                    pending_history.push(FinalizedBlock {
                        id: block_id,
                        proof: FinalizedBlockProof::EntryRevisions(vec![revision]),
                        lines,
                        separator_before,
                    });
                }
                Some(finalization) => {
                    let separator_before = advance_semantic_owner(
                        &mut rendered_tail_owner,
                        block_id.first_owner(),
                        block_id.last_owner(),
                        !lines.is_empty(),
                    );
                    live_blocks.push(LiveBlock::with_header(
                        lines,
                        transcript
                            .entries()
                            .get(index)
                            .is_some_and(TranscriptEntry::has_visible_animation),
                        finalization == Finalization::Live,
                        separator_before,
                    ));
                    commit_blocked |= finalization == Finalization::Live;
                }
                None => {}
            }
            index += 1;
        }
        update.history = pending_history;
        let (live, has_visible_animation) = fit_live_blocks(live_blocks, live_budget, theme);
        update.live = live;
        update.has_visible_animation = has_visible_animation;
        update
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
    live_header_indices: Vec<usize>,
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
        live_header_indices: rendered.live_header_indices,
    }
}

#[derive(Debug)]
struct FittedLine {
    text: String,
    animated: bool,
    pinned: bool,
}

#[derive(Debug)]
struct FittedLiveBlock {
    headers: Vec<FittedLine>,
    body: Vec<FittedLine>,
    tail_len: usize,
    show_omission: bool,
    separator_before: bool,
}

impl FittedLiveBlock {
    fn is_pinned(&self) -> bool {
        self.headers.iter().any(|line| line.pinned)
    }
}

impl From<LiveBlock> for FittedLiveBlock {
    fn from(block: LiveBlock) -> Self {
        let mut is_header = vec![false; block.lines.len()];
        for &index in &block.header_indices {
            if let Some(slot) = is_header.get_mut(index) {
                *slot = true;
            }
        }
        let mut is_internal_separator = vec![false; block.lines.len()];
        for &header_index in block.header_indices.iter().skip(1) {
            let mut index = header_index;
            while index > 0 && block.lines[index - 1].is_empty() {
                index -= 1;
                is_internal_separator[index] = true;
            }
        }
        let mut is_animated = vec![false; block.lines.len()];
        for index in block.animated_line_indices {
            if let Some(slot) = is_animated.get_mut(index) {
                *slot = true;
            }
        }
        let mut is_pinned = vec![false; block.lines.len()];
        for index in block.pinned_line_indices {
            if let Some(slot) = is_pinned.get_mut(index) {
                *slot = true;
            }
        }
        let mut headers = Vec::new();
        let mut body = Vec::new();
        for (index, line) in block.lines.into_iter().enumerate() {
            let line = FittedLine {
                text: line,
                animated: is_animated[index],
                pinned: is_pinned[index],
            };
            if is_header[index] {
                headers.push(line);
            } else if !is_internal_separator[index] {
                body.push(line);
            }
        }
        Self {
            headers,
            body,
            tail_len: 0,
            show_omission: false,
            separator_before: block.separator_before,
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "bounded fitting keeps header, separator, image, and animation accounting together"
)]
fn fit_live_blocks(
    mut blocks: Vec<LiveBlock>,
    budget: usize,
    theme: &TuiTheme,
) -> (Vec<String>, bool) {
    blocks.retain(|block| !block.lines.is_empty());
    if budget == 0 || blocks.is_empty() {
        return (Vec::new(), false);
    }
    let mut full_height = blocks
        .iter()
        .map(|block| block.lines.len() + usize::from(block.separator_before))
        .sum::<usize>();
    if full_height > budget {
        let prefix_separator_after_atomic_removal = blocks
            .iter()
            .position(|block| !block.atomic)
            .filter(|&index| index > 0)
            .and_then(|_| blocks.first().map(|block| block.separator_before));
        blocks.retain(|block| !block.atomic);
        if blocks.is_empty() {
            return (Vec::new(), false);
        }
        if let Some(separator_before) = prefix_separator_after_atomic_removal
            && let Some(first) = blocks.first_mut()
        {
            // The first retained block no longer has a visible predecessor.
            // Preserve only the external history boundary from the removed
            // prefix; an internal separator belonged to the omitted image.
            first.separator_before = separator_before;
        }
        full_height = blocks
            .iter()
            .map(|block| block.lines.len() + usize::from(block.separator_before))
            .sum();
    }
    if full_height <= budget {
        let mut has_visible_animation = false;
        let mut lines = Vec::with_capacity(full_height);
        for block in blocks {
            if block.separator_before {
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
        return (lines, has_visible_animation);
    }

    let mut fitted = blocks
        .into_iter()
        .map(FittedLiveBlock::from)
        .collect::<Vec<_>>();
    let header_count = fitted
        .iter()
        .map(|block| block.headers.len())
        .sum::<usize>();
    let separator_count = fitted
        .iter()
        .map(|block| usize::from(block.separator_before) + block.headers.len().saturating_sub(1))
        .sum::<usize>();
    if header_count.saturating_add(separator_count) > budget {
        let mut selected_headers = fitted
            .iter()
            .map(|block| vec![false; block.headers.len()])
            .collect::<Vec<_>>();
        let mut selected_blocks = vec![false; fitted.len()];
        let mut suppress_separator = vec![false; fitted.len()];
        let mut remaining = budget;
        // Running headers are the live anchors for their cards. Reserve them
        // in canonical order before backfilling with newer deferred headers.
        for block_index in 0..fitted.len() {
            for (header_index, selected) in selected_headers[block_index].iter_mut().enumerate() {
                if !fitted[block_index].headers[header_index].pinned || *selected {
                    continue;
                }
                let cost = if selected_blocks[block_index] {
                    2
                } else {
                    1 + usize::from(fitted[block_index].separator_before)
                };
                if cost <= remaining {
                    *selected = true;
                    selected_blocks[block_index] = true;
                    remaining -= cost;
                }
            }
        }
        for block_index in (0..fitted.len()).rev() {
            for (header_index, selected) in
                selected_headers[block_index].iter_mut().enumerate().rev()
            {
                if fitted[block_index].headers[header_index].pinned || *selected {
                    continue;
                }
                let cost = if selected_blocks[block_index] {
                    2
                } else {
                    1 + usize::from(fitted[block_index].separator_before)
                };
                if cost <= remaining {
                    *selected = true;
                    selected_blocks[block_index] = true;
                    remaining -= cost;
                }
            }
        }
        if !selected_blocks.iter().any(|selected| *selected) && budget > 0 {
            // A one-row live surface cannot also carry an external separator.
            // Keep the earliest live header visible and spend that row on the
            // card itself; normal-sized surfaces retain the separator above.
            let first_header = fitted.iter().enumerate().find_map(|(block_index, block)| {
                block
                    .headers
                    .iter()
                    .position(|header| header.pinned)
                    .or_else(|| (!block.headers.is_empty()).then_some(0))
                    .map(|header_index| (block_index, header_index))
            });
            if let Some((block_index, header_index)) = first_header {
                selected_headers[block_index][header_index] = true;
                selected_blocks[block_index] = true;
                suppress_separator[block_index] = true;
                remaining = 0;
            }
        }
        let mut lines = Vec::with_capacity(budget - remaining);
        let mut animated = false;
        for (block_index, block) in fitted.into_iter().enumerate() {
            if !selected_blocks[block_index] {
                continue;
            }
            if block.separator_before && !suppress_separator[block_index] {
                lines.push(String::new());
            }
            let mut wrote_header = false;
            for (line, selected) in block
                .headers
                .into_iter()
                .zip(selected_headers[block_index].iter().copied())
            {
                if selected {
                    if wrote_header {
                        lines.push(String::new());
                    }
                    animated |= line.animated;
                    lines.push(line.text);
                    wrote_header = true;
                }
            }
        }
        return (lines, animated);
    }

    let mut remaining = budget - header_count - separator_count;
    for pinned in [true, false] {
        for block in fitted
            .iter_mut()
            .rev()
            .filter(|block| block.is_pinned() == pinned && !block.body.is_empty())
        {
            if remaining == 0 {
                break;
            }
            block.show_omission = true;
            remaining -= 1;
        }
        for block in fitted
            .iter_mut()
            .rev()
            .filter(|block| block.is_pinned() == pinned && block.show_omission)
        {
            let retainable = block.body.len().saturating_sub(1);
            block.tail_len = retainable.min(remaining);
            remaining -= block.tail_len;
            if pinned && block.tail_len + 1 == block.body.len() {
                block.show_omission = false;
                block.tail_len = block.body.len();
            }
        }
    }

    let mut lines = Vec::with_capacity(budget);
    let mut has_visible_animation = false;
    for mut block in fitted {
        let mut block_lines = Vec::new();
        for header in block.headers {
            if !block_lines.is_empty() {
                block_lines.push(FittedLine {
                    text: String::new(),
                    animated: false,
                    pinned: false,
                });
            }
            block_lines.push(header);
        }
        if block.show_omission {
            if block.tail_len == 0 && !block.body.is_empty() {
                // The budget is too tight for an omission hint; show the single
                // most recent content line instead.
                block_lines.push(block.body.pop().expect("non-empty body"));
            } else {
                block_lines.push(FittedLine {
                    text: omission_line(block.body.len().saturating_sub(block.tail_len), theme),
                    animated: false,
                    pinned: false,
                });
            }
        }
        block_lines.extend(
            block
                .body
                .into_iter()
                .rev()
                .take(block.tail_len)
                .collect::<Vec<_>>()
                .into_iter()
                .rev(),
        );
        if block_lines.is_empty() {
            continue;
        }
        if block.separator_before {
            lines.push(String::new());
        }
        for line in block_lines {
            has_visible_animation |= line.animated;
            lines.push(line.text);
        }
    }
    lines.truncate(budget);
    (lines, has_visible_animation)
}

fn omission_line(omitted: usize, theme: &TuiTheme) -> String {
    crate::primitive::Line::styled(
        format!("  ... {omitted} earlier rows omitted"),
        crate::primitive::Style::default().fg(theme.text_muted),
    )
    .to_ansi()
}

#[cfg(test)]
mod tests {
    use neo_agent_core::multi_agent::MultiAgentRuntime;

    use super::{LiveBlock, TranscriptPresentation, fit_live_blocks};
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
    fn living_card_holds_later_blocks_in_canonical_order_until_it_finishes() {
        let runtime = MultiAgentRuntime::new();
        let running = runtime.start_foreground_delegate_for_test("background task");
        let id = running.id.clone();
        let mut pane = TranscriptPane::new(80, 12);
        pane.transcript_mut().upsert_delegate(1, running);
        pane.push_status("later status");

        let running_update = pane.render_terminal_update(80, 12);
        assert!(
            running_update.history.is_empty(),
            "later blocks must wait behind the earliest living card"
        );
        let running_live = running_update.live.join("\n");
        let delegate = running_live.find("background task").expect("delegate card");
        let status = running_live.find("later status").expect("later status");
        assert!(
            delegate < status,
            "live suffix must stay in canonical order"
        );

        pane.transcript_mut()
            .upsert_delegate(1, runtime.complete_delegate_for_test(&id, "done"));

        let completed_update = pane.render_terminal_update(80, 12);
        let blocks = completed_update
            .history
            .iter()
            .map(|block| block.lines.join("\n"))
            .collect::<Vec<_>>();

        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("background task"));
        assert!(blocks[1].contains("later status"));
        assert!(completed_update.live.is_empty());
    }

    #[test]
    fn living_card_header_survives_extreme_live_budget() {
        let runtime = MultiAgentRuntime::new();
        let running = runtime.start_foreground_delegate_for_test("pinned delegate");
        let mut pane = TranscriptPane::new(80, 12);
        pane.set_live_chrome_height(0);
        pane.transcript_mut().upsert_delegate(1, running);
        pane.push_status("old status");
        pane.push_status("recent status");
        pane.push_status("latest status");

        let update = pane.render_terminal_update(80, 3);
        let live = update.live.join("\n");

        let delegate = live
            .find("pinned delegate")
            .expect("living delegate header");
        let latest = live.find("latest status").expect("latest deferred header");
        assert!(!live.contains("old status"));
        assert!(!live.contains("recent status"));
        assert!(delegate < latest);
        assert!(update.live[1].is_empty());
        assert!(update.has_visible_animation);
        assert_eq!(update.live.len(), 3);
    }

    #[test]
    fn living_card_body_outranks_later_deferred_result_body() {
        let theme = TuiTheme::default();
        let living = LiveBlock::with_header(
            vec![
                "delegate".to_owned(),
                "delegate tool one".to_owned(),
                "delegate tool two".to_owned(),
            ],
            true,
            true,
            false,
        );
        let deferred = LiveBlock::with_header(
            vec![
                "wait delegate".to_owned(),
                "status".to_owned(),
                "summary".to_owned(),
                "latest result".to_owned(),
            ],
            false,
            false,
            true,
        );

        let (lines, _) = fit_live_blocks(vec![living, deferred], 6, &theme);

        assert_eq!(
            lines,
            vec![
                "delegate",
                "delegate tool one",
                "delegate tool two",
                "",
                "wait delegate",
                "latest result",
            ]
        );
    }

    #[test]
    fn all_running_card_headers_outrank_deferred_static_headers() {
        let runtime = MultiAgentRuntime::new();
        let first = runtime.start_foreground_delegate_for_test("first live delegate");
        let second = runtime.start_foreground_delegate_for_test("second live delegate");
        let mut pane = TranscriptPane::new(80, 12);
        pane.set_live_chrome_height(0);
        pane.transcript_mut().upsert_delegate(1, first);
        pane.transcript_mut().upsert_delegate(2, second);
        pane.push_status("deferred status");

        let update = pane.render_terminal_update(80, 4);
        let live = update.live.join("\n");

        assert!(live.contains("first live delegate"), "live: {live}");
        assert!(live.contains("second live delegate"), "live: {live}");
        assert!(!live.contains("deferred status"), "live: {live}");
    }

    #[test]
    fn truncated_static_header_does_not_inherit_hidden_tool_animation() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.set_live_chrome_height(0);
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "static-read".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "static-header.txt" }),
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "static-read".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("done"),
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "first-live-tool".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({ "command": "first-live-command" }),
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
            turn: 1,
            id: "animated-write".to_owned(),
            name: "Write".to_owned(),
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
            turn: 1,
            id: "animated-write".to_owned(),
            json_fragment: serde_json::json!({
                "path": "hidden-animated-header.txt",
                "content": "content"
            })
            .to_string(),
        });

        let update = pane.render_terminal_update(80, 1);
        let live = update.live.join("\n");

        assert!(!live.contains("static-header.txt"));
        assert!(live.contains("first-live-command"));
        assert!(!live.contains("hidden-animated-header.txt"));
        assert!(!update.has_visible_animation);
    }

    #[test]
    fn truncated_live_block_preserves_ack_boundary_separator() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.set_live_chrome_height(0);
        pane.push_status("committed owner");
        let committed = pane.render_terminal_update(80, 12);
        pane.acknowledge_history(&committed.history);
        pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
            turn: 1,
            id: "bounded-thinking".to_owned(),
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
            turn: 1,
            text: "bounded thinking body".to_owned(),
        });

        let update = pane.render_terminal_update(80, 2);

        assert_eq!(update.live.len(), 2);
        assert!(update.live[0].is_empty());
        assert!(update.live[1].contains("thinking..."));
    }

    #[test]
    fn pinned_live_header_survives_when_only_one_row_fits() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.set_live_chrome_height(0);
        pane.push_status("committed owner");
        let committed = pane.render_terminal_update(80, 12);
        pane.acknowledge_history(&committed.history);

        let runtime = MultiAgentRuntime::new();
        pane.transcript_mut().upsert_delegate(
            1,
            runtime.start_foreground_delegate_for_test("one-row live"),
        );

        let update = pane.render_terminal_update(80, 1);
        assert_eq!(update.live.len(), 1);
        assert!(
            update.live[0].contains("one-row live"),
            "{:#?}",
            update.live
        );
    }

    #[test]
    fn truncated_tool_headers_preserve_internal_entry_separator() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.set_live_chrome_height(0);
        for (id, command) in [
            ("first-tool", "first-command"),
            ("second-tool", "second-command"),
        ] {
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: id.to_owned(),
                name: "Bash".to_owned(),
                arguments: serde_json::json!({ "command": command }),
            });
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
                turn: 1,
                id: id.to_owned(),
                name: "Bash".to_owned(),
                partial_result: neo_agent_core::ToolResult::ok(format!("{command}-output")),
            });
        }

        let update = pane.render_terminal_update(80, 3);

        assert_eq!(update.live.len(), 3);
        assert!(update.live[0].contains("first-command"));
        assert!(update.live[1].is_empty());
        assert!(update.live[2].contains("second-command"));
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
    fn truncation_never_emits_partial_terminal_image() {
        let blocks = vec![
            LiveBlock::with_header(
                vec![
                    "\x1b_Gf=100;image-payload\x1b\\".to_owned(),
                    String::new(),
                    String::new(),
                ],
                false,
                true,
                false,
            ),
            LiveBlock::with_header(vec!["later block".to_owned()], false, false, true),
        ];

        let (lines, _) = fit_live_blocks(blocks, 3, &TuiTheme::default());

        if let Some(image_row) = lines.iter().position(|line| line.contains("\x1b_G")) {
            assert_eq!(lines.get(image_row + 1), Some(&String::new()));
            assert_eq!(lines.get(image_row + 2), Some(&String::new()));
            assert!(!lines.iter().any(|line| line.contains("later block")));
        }
    }

    #[test]
    fn omitted_atomic_block_does_not_consume_successor_live_budget() {
        let blocks = vec![
            LiveBlock::with_header(
                vec!["\x1b_Gf=100;image-payload\x1b\\".to_owned()],
                false,
                true,
                false,
            ),
            LiveBlock::with_header(vec!["later live header".to_owned()], false, false, true),
        ];

        let (lines, _) = fit_live_blocks(blocks, 1, &TuiTheme::default());

        assert_eq!(lines, vec!["later live header"]);
    }

    #[test]
    fn suppressed_living_tool_blocks_later_history_until_visibility_is_resolved() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.transcript_mut()
            .push_tool_run("delegate-tool", "Delegate", Some("{}".to_owned()));
        pane.transcript_mut().suppress_tool_run("delegate-tool");
        pane.push_status("later status");

        let suppressed = pane.render_terminal_update(80, 12);
        assert!(
            suppressed.history.is_empty(),
            "a transiently suppressed live entry must still block later commits"
        );

        pane.transcript_mut().unsuppress_tool_run("delegate-tool");
        let visible = pane.render_terminal_update(80, 12);
        let live = visible.live.join("\n");
        let tool = live.find("Delegate").expect("restored tool card");
        let status = live.find("later status").expect("later status");
        assert!(
            tool < status,
            "restored visibility must preserve canonical order"
        );
    }

    #[test]
    fn finalized_suppressed_tool_releases_later_history() {
        let mut pane = TranscriptPane::new(80, 12);
        pane.transcript_mut()
            .push_tool_run("delegate-tool", "Delegate", Some("{}".to_owned()));
        pane.transcript_mut().suppress_tool_run("delegate-tool");
        pane.push_status("later status");
        assert!(pane.render_terminal_update(80, 12).history.is_empty());

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
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "read-1".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("one"),
        });
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "read-2".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "two.rs" }),
        });

        let running = pane.render_terminal_update(80, 12);
        assert!(running.history.is_empty());
        assert!(!running.live.is_empty());

        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "read-2".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("two"),
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
        let first = presentation.render(
            &mut transcript,
            80,
            &TuiTheme::default(),
            0,
            ImageRenderPolicy::default(),
            TerminalImageCapabilities::default(),
            8,
        );
        presentation.acknowledge(&first.history);

        assert!(transcript.mutate_entry(0, |entry| {
            *entry = TranscriptEntry::status("changed after commit");
            true
        }));
        for _ in 0..2 {
            let update = presentation.render(
                &mut transcript,
                80,
                &TuiTheme::default(),
                0,
                ImageRenderPolicy::default(),
                TerminalImageCapabilities::default(),
                8,
            );
            assert!(update.history.is_empty());
        }

        assert_eq!(presentation.diagnostics.len(), 1);
    }

    #[test]
    fn truncated_live_blocks_keep_each_header_and_omission_row() {
        let blocks = vec![
            LiveBlock::with_header(
                vec![
                    "tool one".to_owned(),
                    "one-a".to_owned(),
                    "one-b".to_owned(),
                    "one-c".to_owned(),
                ],
                true,
                false,
                false,
            ),
            LiveBlock::with_header(
                vec![
                    "tool two".to_owned(),
                    "two-a".to_owned(),
                    "two-b".to_owned(),
                    "two-c".to_owned(),
                ],
                true,
                false,
                false,
            ),
        ];

        let (lines, animated) = fit_live_blocks(blocks, 7, &TuiTheme::default());
        let text = lines.join("\n");

        assert!(animated);
        assert!(text.contains("tool one"));
        assert!(text.contains("tool two"));
        assert_eq!(text.matches("earlier rows omitted").count(), 2);
        assert!(lines.len() <= 7);
    }
}
