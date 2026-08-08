use base64::Engine;
use serde_json::{Value, json};

use crate::multi_agent::{
    AgentLifecycleState, AgentResultPage, AgentRunMode, AgentSnapshot, AgentTerminalReason,
    DelegateContext, SwarmSnapshot,
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
        value["activity_tail"] = json!(model_safe_agent_snapshot(agent).activity);
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

pub(crate) fn model_safe_agent_snapshot(agent: &AgentSnapshot) -> AgentSnapshot {
    let mut snapshot = agent.clone();
    snapshot.clear_live_queue_metadata();
    snapshot
}

pub(crate) fn model_safe_swarm_snapshot(swarm: &SwarmSnapshot) -> SwarmSnapshot {
    let mut snapshot = swarm.clone();
    snapshot.clear_live_queue_metadata();
    snapshot
}

pub(crate) fn delegate_model_result_content(
    agent: &AgentSnapshot,
    context: DelegateContext,
    mut result_page: Option<AgentResultPage>,
    max_output_bytes: usize,
) -> String {
    loop {
        let mut next_actions = Vec::new();
        let mut value = json!({
            "ok": agent.state == AgentLifecycleState::Completed,
            "kind": "delegate_result",
            "target": {"kind": "agent", "id": agent.id.as_str()},
            "status": agent.state.as_str(),
            "context_mode": context_mode_label(context),
        });
        if let Some(result) =
            result_value(agent.id.as_str(), result_page.as_ref(), &mut next_actions)
        {
            value["result"] = result;
        }
        if let Some(outcome) = agent.outcome.as_ref().filter(|outcome| outcome.is_error) {
            value["error"] = json!(outcome.summary);
        }
        value["next_actions"] = json!(next_actions);
        let content = serde_json::to_string(&value).expect("delegate result JSON serializes");
        if content.len() <= max_output_bytes
            || !result_page
                .as_mut()
                .is_some_and(|page| shrink_result_page(page, content.len() - max_output_bytes))
        {
            return content;
        }
    }
}

pub(crate) fn swarm_result_content(
    swarm: &SwarmSnapshot,
    result_pages: &[Option<AgentResultPage>],
    max_output_bytes: usize,
) -> String {
    let mut result_pages = result_pages.to_vec();
    loop {
        let mut next_actions = Vec::new();
        let items = swarm
            .children
            .iter()
            .enumerate()
            .map(|(position, child)| {
                let agent = &child.agent;
                let mut item = json!({
                    "index": child.item_index,
                    "agent_id": agent.id.as_str(),
                    "title": agent.task_title,
                    "status": agent.state.as_str(),
                });
                if let Some(result) = result_value(
                    agent.id.as_str(),
                    result_pages.get(position).and_then(Option::as_ref),
                    &mut next_actions,
                ) {
                    item["result"] = result;
                }
                if let Some(outcome) = agent.outcome.as_ref().filter(|outcome| outcome.is_error) {
                    item["error"] = json!(outcome.summary);
                }
                item
            })
            .collect::<Vec<_>>();
        let content = serde_json::to_string(&json!({
            "ok": swarm.state == AgentLifecycleState::Completed,
            "kind": "delegate_swarm_result",
            "target": {"kind": "swarm", "id": swarm.swarm_id.as_str()},
            "status": swarm.state.as_str(),
            "aggregate": swarm.aggregate,
            "items": items,
            "next_actions": next_actions,
        }))
        .expect("delegate swarm result JSON serializes");
        if content.len() <= max_output_bytes
            || !shrink_largest_result_page(&mut result_pages, content.len() - max_output_bytes)
        {
            return content;
        }
    }
}

pub(crate) fn parse_agent_result_cursor(agent_id: &str, cursor: &str) -> Result<usize, String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor.trim())
        .map_err(|_| "cursor must be a TaskOutput result cursor".to_owned())?;
    let decoded = std::str::from_utf8(&bytes)
        .map_err(|_| "cursor must be a TaskOutput result cursor".to_owned())?;
    let Some((cursor_agent_id, offset)) = decoded.rsplit_once(':') else {
        return Err("cursor must be a TaskOutput result cursor".to_owned());
    };
    if cursor_agent_id != agent_id {
        return Err("cursor was created for a different delegate result".to_owned());
    }
    offset
        .parse()
        .map_err(|_| "cursor must be a TaskOutput result cursor".to_owned())
}

fn encode_agent_result_cursor(agent_id: &str, offset: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{agent_id}:{offset}"))
}

fn result_value(
    agent_id: &str,
    result_page: Option<&AgentResultPage>,
    next_actions: &mut Vec<Value>,
) -> Option<Value> {
    let page = result_page?;
    let cursor = page
        .next_offset
        .map(|next_offset| encode_agent_result_cursor(agent_id, next_offset));
    if let Some(cursor) = cursor.as_deref() {
        next_actions.push(json!({
            "tool": "TaskOutput",
            "arguments": {
                "task_id": agent_id,
                "view": "result",
                "cursor": cursor,
            },
        }));
    }
    Some(json!({
        "mode": if cursor.is_some() { "page" } else { "inline" },
        "text": page.text,
        "total_chars": page.total_chars,
        "has_more": cursor.is_some(),
        "cursor": cursor,
    }))
}

fn shrink_largest_result_page(
    result_pages: &mut [Option<AgentResultPage>],
    overflow_bytes: usize,
) -> bool {
    let page = result_pages
        .iter_mut()
        .filter_map(Option::as_mut)
        .max_by_key(|page| page.text.len());
    page.is_some_and(|page| shrink_result_page(page, overflow_bytes))
}

fn shrink_result_page(page: &mut AgentResultPage, overflow_bytes: usize) -> bool {
    if page.text.is_empty() {
        return false;
    }
    let mut end = page.text.len().saturating_sub(overflow_bytes.max(1));
    while end > 0 && !page.text.is_char_boundary(end) {
        end -= 1;
    }
    if end == page.text.len() {
        return false;
    }
    page.text.truncate(end);
    page.next_offset = Some(page.offset + end);
    true
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
