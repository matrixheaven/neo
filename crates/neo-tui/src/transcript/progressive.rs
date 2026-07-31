//! Typed identities and pure projection helpers for progressive transcript
//! facts.
//!
//! A progressive fact is one immutable piece of a mutable transcript entry
//! (a completed child tool row, a terminal agent run, or a terminal swarm item).
//! Identity and finality are derived
//! exclusively from typed event/snapshot state — never from rendered text,
//! regex matching, row position, or vector indexes. Facts are captured by
//! [`TranscriptStore`] at update time, before the source snapshot can trim or
//! replace them, and rendered here with the existing child-activity helpers so
//! progressive history matches the live card style.
//!
//! [`TranscriptStore`]: super::store::TranscriptStore

use crate::primitive::theme::TuiTheme;
use neo_agent_core::multi_agent::{
    AgentSnapshot, AgentToolActivityPhase, AgentToolFileChange, AgentToolOutputPreview,
};

use super::child_activity::{ChildToolRow, render_child_agent_summary, render_child_tool_row};
use super::store::TranscriptEntryId;

/// Typed identity of one immutable progressive fact.
///
/// Identity combines the owning transcript entry with the structured source
/// identity of the fact (agent run, tool call, or swarm item). It never
/// derives from display text or row position.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProgressiveFactId {
    /// A completed child tool activity (`Done`/`Failed`) inside a Delegate,
    /// DelegateGroup, or DelegateSwarm card.
    ChildTool {
        entry: TranscriptEntryId,
        agent_id: String,
        run_count: u32,
        tool_id: String,
    },
    /// A terminal child agent run inside a Delegate / DelegateGroup card.
    ChildAgent {
        entry: TranscriptEntryId,
        agent_id: String,
        run_count: u32,
    },
    /// A terminal swarm child item.
    SwarmItem {
        entry: TranscriptEntryId,
        swarm_id: String,
        item_index: usize,
        agent_id: String,
        run_count: u32,
    },
}

impl ProgressiveFactId {
    /// The transcript entry that owns this fact.
    #[must_use]
    pub(crate) const fn entry(&self) -> TranscriptEntryId {
        match self {
            Self::ChildTool { entry, .. }
            | Self::ChildAgent { entry, .. }
            | Self::SwarmItem { entry, .. } => *entry,
        }
    }
}

/// One captured immutable fact payload retained by the store in canonical
/// arrival order until its terminal write is acknowledged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressiveFact {
    pub id: ProgressiveFactId,
    pub(crate) payload: ProgressiveFactPayload,
}

/// Typed display payload of a captured fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProgressiveFactPayload {
    /// A completed child tool activity row.
    ChildTool(ChildToolFact),
    /// A terminal child agent run inside a Delegate / DelegateGroup card.
    ChildAgent(ChildAgentFact),
    /// A terminal swarm child item.
    SwarmItem(SwarmItemFact),
}

/// Owned snapshot of one completed child tool activity row, frozen at
/// `Done`/`Failed` before the source snapshot can trim or retry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildToolFact {
    pub agent_id: String,
    pub run_count: u32,
    pub tool_id: String,
    pub name: String,
    pub summary: Option<String>,
    pub phase: AgentToolActivityPhase,
    pub output: Option<AgentToolOutputPreview>,
    pub files: Vec<AgentToolFileChange>,
}

impl ChildToolFact {
    /// Finality proof: only `Done`/`Failed` child tool activity is stable.
    #[must_use]
    pub(crate) const fn is_terminal(&self) -> bool {
        matches!(
            self.phase,
            AgentToolActivityPhase::Done | AgentToolActivityPhase::Failed
        )
    }
}

/// One terminal child agent run, frozen at `AgentLifecycleState::is_terminal()`
/// before the source snapshot can trim its activity or be replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildAgentFact {
    pub agent_id: String,
    pub run_count: u32,
    /// Frozen terminal snapshot: final summary, outcome, counts, usage, and
    /// terminal reason are captured together with the typed terminal state.
    pub snapshot: AgentSnapshot,
}

/// One terminal swarm child item, frozen at its typed terminal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SwarmItemFact {
    pub swarm_id: String,
    pub item_index: usize,
    pub agent_id: String,
    pub run_count: u32,
    pub snapshot: AgentSnapshot,
}

/// Render one captured fact into final ANSI rows, reusing the existing
/// child-activity render helpers so progressive history matches the live card
/// style. Recomputing rows at render time keeps history width-correct across
/// terminal resizes until the block is written and acknowledged.
#[must_use]
pub(crate) fn render_progressive_fact(
    fact: &ProgressiveFact,
    width: usize,
    theme: &TuiTheme,
) -> Vec<String> {
    let mut lines = match &fact.payload {
        ProgressiveFactPayload::ChildTool(tool) => {
            let row = ChildToolRow {
                name: &tool.name,
                summary: tool.summary.as_deref(),
                phase: tool.phase,
                output: tool.output.as_ref(),
                files: &tool.files,
            };
            render_child_tool_row(&row, width, "  ", theme, None)
                .into_iter()
                .map(|line| line.to_ansi())
                .collect::<Vec<_>>()
        }
        ProgressiveFactPayload::ChildAgent(fact) => {
            render_child_agent_summary(&fact.snapshot, width, theme)
                .into_iter()
                .map(|line| line.to_ansi())
                .collect()
        }
        ProgressiveFactPayload::SwarmItem(fact) => {
            render_child_agent_summary(&fact.snapshot, width, theme)
                .into_iter()
                .map(|line| line.to_ansi())
                .collect()
        }
    };
    super::pane::trim_ansi_transcript_block(&mut lines);
    lines
}
