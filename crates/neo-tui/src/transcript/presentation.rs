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
    pub live_overflow: bool,
    pub has_live_frontier: bool,
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
                        separator_before,
                    ));
                    commit_blocked |= finalization == Finalization::Live;
                }
                None => {}
            }
            index += 1;
        }
        update.history = pending_history;
        let (live, has_visible_animation) = compose_live_blocks(live_blocks);
        update.live_overflow = live.len() > live_budget;
        update.has_live_frontier = commit_blocked;
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
    (lines, has_visible_animation)
}

#[cfg(test)]
mod tests {
    use neo_agent_core::multi_agent::MultiAgentRuntime;

    use super::TranscriptPresentation;
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
        assert!(!completed_update.has_live_frontier);
    }

    #[test]
    fn live_overflow_preserves_complete_rows() {
        let runtime = MultiAgentRuntime::new();
        let running = runtime.start_foreground_delegate_for_test("overflow living card");
        let mut pane = TranscriptPane::new(80, 12);
        pane.set_live_chrome_height(0);
        pane.transcript_mut().upsert_delegate(1, running);
        for index in 0..12 {
            pane.push_status(format!("deferred status {index}"));
        }

        let update = pane.render_terminal_update(80, 4);
        let live = update.live.join("\n");

        assert!(update.live_overflow);
        assert!(update.has_live_frontier);
        assert!(live.contains("overflow living card"), "live:\n{live}");
        for index in 0..12 {
            assert!(
                live.contains(&format!("deferred status {index}")),
                "missing deferred status {index} in complete live source:\n{live}"
            );
        }
        let omission_marker = format!("{} {}", "earlier rows", "omitted");
        assert!(!live.contains(&omission_marker), "live:\n{live}");
        let living = live.find("overflow living card").expect("living card");
        let first_deferred = live.find("deferred status 0").expect("first deferred");
        let last_deferred = live.find("deferred status 11").expect("last deferred");
        assert!(living < first_deferred);
        assert!(first_deferred < last_deferred);
        assert!(update.live.len() > 4);
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
        assert!(suppressed.has_live_frontier);

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
        assert!(!released.has_live_frontier);
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
        assert!(running.has_live_frontier);

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
        assert!(!finished.has_live_frontier);
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
}
