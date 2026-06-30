use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};
use crate::multi_agent::{AgentLifecycleState, SwarmSnapshot};

fn terminal_delegate_error(agent_id: &str, state: AgentLifecycleState) -> ToolResult {
    ToolResult::error(format!(
        "agent already {}; terminal delegate state is immutable. To continue it, call Delegate with resume=\"{}\".",
        state.as_str(),
        agent_id
    ))
    .with_details(serde_json::json!({
        "agent_id": agent_id,
        "status": state.as_str(),
        "terminal": true,
        "resume_hint": format!("Delegate with resume=\"{agent_id}\""),
    }))
}

// ---------------------------------------------------------------------------
// ListDelegates
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DelegateListKind {
    Agent,
    Swarm,
    #[default]
    All,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DelegateListOrder {
    #[default]
    Newest,
    Oldest,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListDelegatesInput {
    #[serde(default)]
    #[schemars(
        description = "Whether to include completed/cancelled delegates. Defaults to false (active only)."
    )]
    include_completed: bool,
    #[serde(default)]
    kind: DelegateListKind,
    #[serde(default)]
    state: Option<AgentLifecycleState>,
    #[serde(default = "default_delegate_list_limit")]
    #[schemars(description = "Maximum number of rows to return. Defaults to 20.")]
    limit: usize,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    order: DelegateListOrder,
}

struct DelegateListRow {
    created_index: u64,
    id: String,
    detail: String,
    json: serde_json::Value,
}

fn default_delegate_list_limit() -> usize {
    20
}

fn parse_list_cursor(tool: &str, cursor: Option<&str>) -> Result<usize, ToolError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    cursor
        .parse::<usize>()
        .map_err(|_| ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "cursor must be a ListDelegates next_cursor value".to_owned(),
        })
}

pub struct ListDelegatesTool;

impl Tool for ListDelegatesTool {
    fn name(&self) -> &'static str {
        "ListDelegates"
    }

    fn description(&self) -> &'static str {
        "List delegate agents and/or swarms with their current status. \
         Supports filtering by kind (agent, swarm, all), state, and ordering. \
         Defaults to newest-first, active-only, all kinds."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<ListDelegatesInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: ListDelegatesInput = parse_input(self.name(), input)?;
            let include_completed = input.include_completed;
            let offset = parse_list_cursor(self.name(), input.cursor.as_deref())?;
            let limit = input.limit.max(1);

            let show_agents = matches!(input.kind, DelegateListKind::Agent | DelegateListKind::All);
            let show_swarms = matches!(input.kind, DelegateListKind::Swarm | DelegateListKind::All);

            let mut all_rows: Vec<DelegateListRow> = Vec::new();

            if show_agents {
                let agents = ctx.multi_agent.list_agents(include_completed);
                for agent in &agents {
                    if let Some(filter_state) = input.state
                        && agent.state != filter_state
                    {
                        continue;
                    }
                    let mut detail = format!(
                        "\n- agent_id: {} ({}) state: {} task: {}",
                        agent.id.as_str(),
                        agent.display_name.as_str(),
                        agent.state.as_str(),
                        agent.task,
                    );
                    if let Some(outcome) = &agent.outcome {
                        detail.push_str(&format!(" | summary: {}", outcome.summary));
                    }
                    all_rows.push(DelegateListRow {
                        created_index: ctx
                            .multi_agent
                            .agent_created_index(agent.id.as_str())
                            .unwrap_or_default(),
                        id: agent.id.as_str().to_owned(),
                        detail,
                        json: json!({
                            "kind": "agent",
                            "id": agent.id.as_str(),
                            "status": agent.state.as_str(),
                            "display_name": agent.display_name.as_str(),
                            "task": agent.task,
                        }),
                    });
                }
            }

            if show_swarms {
                let swarms = ctx.multi_agent.list_swarms();
                for swarm in &swarms {
                    if !include_completed && swarm.state.is_terminal() {
                        continue;
                    }
                    if let Some(filter_state) = input.state
                        && swarm.state != filter_state
                    {
                        continue;
                    }
                    let detail = format!(
                        "\n- swarm_id: {}\n  kind: swarm\n  status: {}\n  description: {}\n  aggregate: total={} queued={} running={} completed={} failed={} cancelled={} timed_out={}",
                        swarm.swarm_id,
                        swarm.state.as_str(),
                        swarm.description,
                        swarm.aggregate.total,
                        swarm.aggregate.queued,
                        swarm.aggregate.running,
                        swarm.aggregate.completed,
                        swarm.aggregate.failed,
                        swarm.aggregate.cancelled,
                        swarm.aggregate.timed_out,
                    );
                    all_rows.push(DelegateListRow {
                        created_index: ctx
                            .multi_agent
                            .swarm_created_index(&swarm.swarm_id)
                            .unwrap_or_default(),
                        id: swarm.swarm_id.clone(),
                        detail,
                        json: json!({
                            "kind": "swarm",
                            "id": swarm.swarm_id,
                            "status": swarm.state.as_str(),
                            "description": swarm.description,
                            "aggregate": swarm.aggregate,
                        }),
                    });
                }
            }

            match input.order {
                DelegateListOrder::Newest => {
                    all_rows.sort_by(|a, b| {
                        b.created_index
                            .cmp(&a.created_index)
                            .then_with(|| b.id.cmp(&a.id))
                    });
                }
                DelegateListOrder::Oldest => {
                    all_rows.sort_by(|a, b| {
                        a.created_index
                            .cmp(&b.created_index)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                }
            }

            let total = all_rows.len();
            let page_end = offset.saturating_add(limit).min(total);
            let next_cursor = (page_end < total).then(|| page_end.to_string());
            let page_rows = all_rows
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();

            let mut content = if page_rows.is_empty() {
                "No delegates found.\n".to_owned()
            } else {
                format!("delegates: {total}\n")
            };
            let rows: Vec<_> = page_rows.iter().map(|row| row.json.clone()).collect();
            for row in &page_rows {
                content.push_str(&row.detail);
            }

            Ok(ToolResult::ok(content).with_details(json!({
                "kind": "delegate_list",
                "count": page_rows.len(),
                "total": total,
                "next_cursor": next_cursor,
                "include_completed": include_completed,
                "delegates": rows,
            })))
        })
    }
}

// ---------------------------------------------------------------------------
// WaitDelegate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct WaitDelegateInput {
    #[schemars(description = "The agent or swarm ID to wait for.")]
    id: String,
    #[schemars(
        description = "Maximum time to wait in milliseconds. Defaults to 30000 (30s). Returns timed_out if the target hasn't finished."
    )]
    timeout_ms: Option<u64>,
}

pub struct WaitDelegateTool;

impl Tool for WaitDelegateTool {
    fn name(&self) -> &'static str {
        "WaitDelegate"
    }

    fn description(&self) -> &'static str {
        "Wait for a delegate agent or swarm to reach a terminal state (completed, failed, \
         cancelled, timed_out). Returns the agent's final status or the swarm's aggregate \
         and per-child results."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<WaitDelegateInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: WaitDelegateInput = parse_input(self.name(), input)?;
            let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(30_000));
            let deadline = std::time::Instant::now() + timeout;

            // Route by ID prefix.
            if input.id.starts_with("swarm_") {
                loop {
                    if let Some(swarm) = ctx.multi_agent.swarm_snapshot(&input.id)
                        && swarm.state.is_terminal()
                    {
                        return Ok(format_swarm_result(&swarm));
                    }
                    // Also check background task state.
                    if let Ok(task_snap) = ctx.background_tasks.snapshot(&input.id).await
                        && !task_snap.status.is_active()
                        && let Some(swarm) = ctx.multi_agent.swarm_snapshot(&input.id)
                    {
                        return Ok(format_swarm_result(&swarm));
                    }
                    if std::time::Instant::now() >= deadline {
                        return Ok(ToolResult::ok(format!(
                            "id: {}\nstatus: timed_out\nnext_step: The swarm is still running. Increase the timeout or use ListDelegates to check status.",
                            input.id,
                        ))
                        .with_details(json!({
                            "kind": "delegate_wait",
                            "task_id": input.id,
                            "outcome": "timed_out",
                        })));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }

            loop {
                // Check runtime state by searching agents list for matching ID.
                let agents = ctx.multi_agent.list_agents(true);
                if let Some(snapshot) = agents.iter().find(|a| a.id.as_str() == input.id).cloned()
                    && snapshot.state.is_terminal()
                {
                    let state_label = snapshot.state.as_str();
                    let summary = snapshot
                        .outcome
                        .as_ref()
                        .map(|o| o.summary.clone())
                        .unwrap_or_default();
                    return Ok(ToolResult::ok(format!(
                        "id: {}\nstatus: {}\nsummary: {}",
                        snapshot.id.as_str(),
                        state_label,
                        summary,
                    ))
                    .with_details(json!({
                        "kind": "delegate_wait",
                        "agent": snapshot,
                        "outcome": state_label,
                    })));
                }

                // Also check background task state.
                if let Ok(task_snap) = ctx.background_tasks.snapshot(&input.id).await
                    && !task_snap.status.is_active()
                {
                    return Ok(ToolResult::ok(format!(
                        "id: {}\nstatus: {}\noutcome: completed",
                        input.id,
                        task_snap.status.as_str(),
                    ))
                    .with_details(json!({
                        "kind": "delegate_wait",
                        "task_id": input.id,
                        "outcome": task_snap.status.as_str(),
                    })));
                }

                if std::time::Instant::now() >= deadline {
                    return Ok(ToolResult::ok(format!(
                        "id: {}\nstatus: timed_out\nnext_step: The delegate is still running. Increase the timeout or use ListDelegates to check status.",
                        input.id,
                    ))
                    .with_details(json!({
                        "kind": "delegate_wait",
                        "task_id": input.id,
                        "outcome": "timed_out",
                    })));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// InterruptDelegate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct InterruptDelegateInput {
    #[schemars(description = "The agent or swarm ID to interrupt.")]
    id: String,
}

pub struct InterruptDelegateTool;

impl Tool for InterruptDelegateTool {
    fn name(&self) -> &'static str {
        "InterruptDelegate"
    }

    fn description(&self) -> &'static str {
        "Interrupt and cancel a running delegate agent or swarm. \
         Non-terminal children of a swarm are cancelled; terminal children are skipped. \
         Terminal targets return an error."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<InterruptDelegateInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: InterruptDelegateInput = parse_input(self.name(), input)?;

            // Route by ID prefix for swarm targets.
            if input.id.starts_with("swarm_") {
                match ctx.multi_agent.cancel_swarm(&input.id) {
                    Ok(swarm) => {
                        let _ = ctx
                            .background_tasks
                            .cancel_delegate_swarm(&input.id, swarm.clone())
                            .await;
                        return Ok(ToolResult::ok(format!(
                            "id: {}\nstatus: cancelled\naggregate: total={} completed={} failed={} cancelled={} timed_out={}",
                            swarm.swarm_id,
                            swarm.aggregate.total,
                            swarm.aggregate.completed,
                            swarm.aggregate.failed,
                            swarm.aggregate.cancelled,
                            swarm.aggregate.timed_out,
                        ))
                        .with_details(json!({
                            "kind": "delegate_interrupt",
                            "swarm": swarm,
                            "outcome": "cancelled",
                        })));
                    }
                    Err(message) => {
                        return Ok(ToolResult::error(message));
                    }
                }
            }

            // Find the agent by ID in the runtime.
            let agents = ctx.multi_agent.list_agents(true);
            if let Some(agent) = agents.iter().find(|a| a.id.as_str() == input.id).cloned() {
                let agent_id = agent.id.clone();
                if agent.state.is_terminal() {
                    return Ok(terminal_delegate_error(agent.id.as_str(), agent.state));
                }
                let Some(snapshot) = ctx.multi_agent.cancel_agent(&agent_id) else {
                    return Ok(terminal_delegate_error(agent.id.as_str(), agent.state));
                };
                let _ = ctx
                    .background_tasks
                    .cancel_delegate(&input.id, snapshot.clone())
                    .await;
                return Ok(ToolResult::ok(format!(
                    "id: {}\nstatus: cancelled\nname: {}",
                    snapshot.id.as_str(),
                    snapshot.display_name.as_str(),
                ))
                .with_details(json!({
                    "kind": "delegate_interrupt",
                    "agent": snapshot,
                    "outcome": "cancelled",
                })));
            }

            // Fall back to background task stop.
            match ctx
                .background_tasks
                .stop(&input.id, "Interrupted by InterruptDelegate", 1024)
                .await
            {
                Ok(result) => Ok(result),
                Err(err) => Ok(ToolResult::error(format!(
                    "id: {}\nerror: {}",
                    input.id, err
                ))),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// MessageDelegate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct MessageDelegateInput {
    #[schemars(description = "The agent or swarm ID to message.")]
    id: String,
    #[schemars(description = "The message text to deliver.")]
    message: String,
}

pub struct MessageDelegateTool;

impl Tool for MessageDelegateTool {
    fn name(&self) -> &'static str {
        "MessageDelegate"
    }

    fn description(&self) -> &'static str {
        "Send a live follow-up message to a currently running delegate or broadcast \
         to running children of a swarm. MessageDelegate does not queue offline messages \
         for idle or terminal agents. If the target is completed, failed, cancelled, \
         timed_out, or not running, call Delegate with resume=\"agent_...\" instead."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<MessageDelegateInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: MessageDelegateInput = parse_input(self.name(), input)?;

            // Route by ID prefix for swarm targets.
            if input.id.starts_with("swarm_") {
                match ctx
                    .multi_agent
                    .broadcast_live_swarm_message(&input.id, input.message.clone())
                {
                    Ok((delivered, skipped)) => {
                        return Ok(ToolResult::ok(format!(
                            "target: {}\ndelivered: {}\nskipped: {}",
                            input.id,
                            delivered.join(", "),
                            skipped
                                .iter()
                                .map(|(id, state)| format!("{id} ({})", state.as_str()))
                                .collect::<Vec<_>>()
                                .join(", "),
                        ))
                        .with_details(json!({
                            "target": input.id,
                            "delivered": delivered,
                            "skipped": skipped.iter().map(|(agent_id, state)| {
                                json!({ "agent_id": agent_id, "state": state.as_str() })
                            }).collect::<Vec<_>>(),
                        })));
                    }
                    Err(message) => {
                        return Ok(ToolResult::error(message));
                    }
                }
            }

            match ctx
                .multi_agent
                .deliver_live_agent_message(&input.id, input.message.clone())
            {
                Ok(()) => Ok(ToolResult::ok(format!(
                    "target: {}\nstatus: delivered\nmessage: {}",
                    input.id, input.message
                ))
                .with_details(json!({
                    "target": input.id,
                    "status": "delivered",
                    "delivered": [input.id],
                    "message": input.message,
                }))),
                Err(message) => Ok(ToolResult::error(message)),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Swarm result formatting helper
// ---------------------------------------------------------------------------

/// Format a swarm snapshot as a rich tool result with aggregate and per-child items.
fn format_swarm_result(swarm: &SwarmSnapshot) -> ToolResult {
    let mut content = format!(
        "kind: swarm\nswarm_id: {}\nstatus: {}\naggregate: total={} queued={} running={} completed={} failed={} cancelled={} timed_out={}\nitems:",
        swarm.swarm_id,
        swarm.state.as_str(),
        swarm.aggregate.total,
        swarm.aggregate.queued,
        swarm.aggregate.running,
        swarm.aggregate.completed,
        swarm.aggregate.failed,
        swarm.aggregate.cancelled,
        swarm.aggregate.timed_out,
    );
    let items: Vec<serde_json::Value> = swarm
        .children
        .iter()
        .map(|child| {
            json!({
                "index": child.item_index,
                "item": child.item,
                "agent_id": child.agent.id.as_str(),
                "status": child.agent.state.as_str(),
                "summary": child.agent.outcome.as_ref().map(|outcome| outcome.summary.clone()),
            })
        })
        .collect();
    for child in &swarm.children {
        content.push_str(&format!(
            "\n- index: {} agent_id: {} status: {}",
            child.item_index,
            child.agent.id.as_str(),
            child.agent.state.as_str(),
        ));
    }
    ToolResult::ok(content).with_details(json!({
        "kind": "swarm",
        "swarm_id": swarm.swarm_id,
        "status": swarm.state.as_str(),
        "aggregate": swarm.aggregate,
        "items": items,
        "resume_hint": "Call DelegateSwarm with resume_agent_ids for unfinished children.",
    }))
}
