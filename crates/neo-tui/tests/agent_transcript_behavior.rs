//! Multi-agent transcript behavior: delegate cards, delegate groups,
//! swarms, workflow cards, and background updates.

#[path = "agent_transcript_behavior/background_updates.rs"]
mod background_updates;
#[path = "agent_transcript_behavior/delegate.rs"]
mod delegate;
#[path = "agent_transcript_behavior/delegate_cards.rs"]
mod delegate_cards;
#[path = "agent_transcript_behavior/delegate_group.rs"]
mod delegate_group;
#[path = "agent_transcript_behavior/delegate_options.rs"]
mod delegate_options;
#[path = "agent_transcript_behavior/delegate_swarm.rs"]
mod delegate_swarm;
#[path = "agent_transcript_behavior/swarm_cards.rs"]
mod swarm_cards;
#[path = "agent_transcript_behavior/workflow.rs"]
mod workflow;
#[path = "agent_transcript_behavior/workflow_group.rs"]
mod workflow_group;
#[path = "agent_transcript_behavior/workflow_origin.rs"]
mod workflow_origin;
#[path = "agent_transcript_behavior/workflow_replay.rs"]
mod workflow_replay;
#[path = "agent_transcript_behavior/workflow_tool.rs"]
mod workflow_tool;
