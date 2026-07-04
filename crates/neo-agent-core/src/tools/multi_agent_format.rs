use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::multi_agent::{
    AgentLifecycleState, AgentRunMode, AgentSnapshot, AgentTerminalReason, DelegateContext,
    SwarmSnapshot,
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum SummaryScope {
    CurrentRun,
    SwarmItems,
    None,
}

impl SummaryScope {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CurrentRun => "current_run",
            Self::SwarmItems => "swarm_items",
            Self::None => "none",
        }
    }
}

pub(crate) const fn context_mode_label(context: DelegateContext) -> &'static str {
    match context {
        DelegateContext::Inherit => "inherit",
        DelegateContext::Summary => "summary",
        DelegateContext::None => "none",
    }
}

pub(crate) const fn mode_label(mode: AgentRunMode) -> &'static str {
    match mode {
        AgentRunMode::Foreground => "foreground",
        AgentRunMode::Background => "background",
    }
}

pub(crate) fn agent_details(
    kind: &'static str,
    agent: &AgentSnapshot,
    context: Option<DelegateContext>,
    summary_scope: SummaryScope,
    include_task: bool,
    include_summary: bool,
    include_activity: bool,
) -> Value {
    let mut value = json!({
        "kind": kind,
        "id": agent.id.as_str(),
        "agent_id": agent.id.as_str(),
        "status": agent.state.as_str(),
        "mode": mode_label(agent.mode),
        "role": agent.role.as_str(),
        "actual_role": agent.role.as_str(),
        "display_name": agent.display_name.as_str(),
        "title": agent.task_title.as_str(),
        "created_at_ms": agent.created_at_ms,
        "updated_at_ms": agent.updated_at_ms,
        "started_at_ms": agent.started_at_ms,
        "terminal_at_ms": agent.terminal_at_ms,
        "elapsed_ms": u64::try_from(agent.elapsed.as_millis()).unwrap_or(u64::MAX),
        "tool_count": agent.tool_count,
        "token_count": agent.token_count,
        "run_index": agent.run_count,
        "run_count": agent.run_count,
        "live_messages_received": agent.live_messages_received,
        "previous_status": agent.previous_status.map(AgentLifecycleState::as_str),
        "resumed_from": agent.resumed_from.as_ref().map(crate::multi_agent::AgentId::as_str),
        "summary_scope": summary_scope.as_str(),
    });
    if let Some(context) = context {
        value["context_mode"] = json!(context_mode_label(context));
    }
    if include_task {
        value["task"] = json!(agent.task.as_str());
    }
    if include_summary {
        value["summary"] = json!(
            agent
                .outcome
                .as_ref()
                .map(|outcome| outcome.summary.clone())
                .unwrap_or_default()
        );
    }
    if include_activity {
        value["activity_tail"] = json!(&agent.activity);
    }
    if matches!(
        agent.terminal_reason,
        Some(AgentTerminalReason::Lost | AgentTerminalReason::ProcessExited)
    ) {
        value["resume_hint"] = json!(format!(
            "Delegate(resume=\"{}\", task=\"continue\")",
            agent.id.as_str()
        ));
    }
    value
}

pub(crate) fn delegate_result_content(agent: &AgentSnapshot, context: DelegateContext) -> String {
    let mut summary_text = format!(
        "agent_id: {}\nname: {}\nstatus: {}\nrun_index: {}\nsummary_scope: current_run\ncontext_mode: {}",
        agent.id.as_str(),
        agent.display_name.as_str(),
        agent.state.as_str(),
        agent.run_count,
        context_mode_label(context),
    );
    if let Some(previous) = agent.previous_status {
        let _ = writeln!(summary_text, "\nprevious_status: {}", previous.as_str());
    }
    if let Some(outcome) = &agent.outcome {
        let _ = writeln!(summary_text, "\nsummary: {}", outcome.summary);
    }
    summary_text
}

pub(crate) fn swarm_details(swarm: &SwarmSnapshot) -> Value {
    let items = swarm
        .children
        .iter()
        .map(|child| {
            let agent = &child.agent;
            json!({
                "index": child.item_index,
                "item": child.item.as_str(),
                "agent_id": agent.id.as_str(),
                "name": agent.display_name.as_str(),
                "status": agent.state.as_str(),
                "title": agent.task_title.as_str(),
                "elapsed_ms": u64::try_from(agent.elapsed.as_millis()).unwrap_or(u64::MAX),
                "tool_count": agent.tool_count,
                "token_count": agent.token_count,
                "summary": agent.outcome.as_ref().map(|outcome| outcome.summary.clone()),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "kind": "delegate_swarm",
        "id": swarm.swarm_id.as_str(),
        "swarm_id": swarm.swarm_id.as_str(),
        "status": swarm.state.as_str(),
        "mode": mode_label(swarm.mode),
        "role": swarm.role.as_str(),
        "description": swarm.description.as_str(),
        "summary_scope": SummaryScope::SwarmItems.as_str(),
        "aggregate": swarm.aggregate,
        "items": items,
        "resume_hint": "Call DelegateSwarm with resume_agent_ids for unfinished children.",
    })
}
