//! Typed identities for completed child activity.

use neo_agent_core::multi_agent::{
    AgentSnapshot, AgentToolActivityPhase, AgentToolFileChange, AgentToolOutputPreview,
};
use neo_agent_core::session::ToolOutputRef;

use super::store::TranscriptEntryId;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProgressiveFactId {
    ChildTool {
        entry: TranscriptEntryId,
        agent_id: String,
        run_count: u32,
        tool_id: String,
    },
    ChildAgent {
        entry: TranscriptEntryId,
        agent_id: String,
        run_count: u32,
    },
    SwarmItem {
        entry: TranscriptEntryId,
        swarm_id: String,
        item_index: usize,
        agent_id: String,
        run_count: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressiveFact {
    pub id: ProgressiveFactId,
    pub(crate) capture_sequence: u64,
    pub(crate) payload: ProgressiveFactPayload,
}

impl ProgressiveFact {
    #[must_use]
    pub(crate) const fn capture_sequence(&self) -> u64 {
        self.capture_sequence
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProgressiveFactPayload {
    ChildTool(ChildToolFact),
    ChildAgent(ChildAgentFact),
    SwarmItem(SwarmItemFact),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildToolFact {
    pub agent_id: String,
    pub run_count: u32,
    pub tool_id: String,
    pub activity_index: usize,
    pub name: String,
    pub summary: Option<String>,
    pub phase: AgentToolActivityPhase,
    pub output: Option<AgentToolOutputPreview>,
    pub files: Vec<AgentToolFileChange>,
    /// Typed complete-display-output artifact for this tool execution, when
    /// the child runtime captured one. Presentation metadata only.
    pub output_ref: Option<ToolOutputRef>,
}

impl ChildToolFact {
    #[must_use]
    pub(crate) const fn is_terminal(&self) -> bool {
        matches!(
            self.phase,
            AgentToolActivityPhase::Done | AgentToolActivityPhase::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildAgentFact {
    pub agent_id: String,
    pub run_count: u32,
    pub snapshot: AgentSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SwarmItemFact {
    pub swarm_id: String,
    pub item_index: usize,
    pub agent_id: String,
    pub run_count: u32,
    pub snapshot: AgentSnapshot,
}
