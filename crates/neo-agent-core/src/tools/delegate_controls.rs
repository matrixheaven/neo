use std::fmt::Write as _;
use std::time::Duration;

use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};
use crate::multi_agent::{AgentLifecycleState, SwarmSnapshot};

#[derive(Debug, Clone, Copy)]
enum DelegateTerminalAction {
    Message,
    Interrupt,
}

impl DelegateTerminalAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Interrupt => "interrupt",
        }
    }

    const fn terminal_clause(self) -> &'static str {
        match self {
            Self::Message => "terminal agents cannot receive live messages",
            Self::Interrupt => "terminal agents cannot be interrupted",
        }
    }
}

fn delegate_target_not_found(id: &str) -> ToolResult {
    ToolResult::error(format!("unknown delegate target `{id}`")).with_details(json!({
        "kind": "delegate_target",
        "id": id,
        "outcome": "not_found",
    }))
}

fn terminal_delegate_error(
    agent_id: &str,
    state: AgentLifecycleState,
    action: DelegateTerminalAction,
) -> ToolResult {
    ToolResult::error(format!(
        "agent already {}; {}. To continue this agent, call Delegate with resume=\"{}\".",
        state.as_str(),
        action.terminal_clause(),
        agent_id
    ))
    .with_details(json!({
        "agent_id": agent_id,
        "status": state.as_str(),
        "terminal": true,
        "action": action.as_str(),
        "resume_hint": format!("Delegate with resume=\"{agent_id}\""),
    }))
}

// ---------------------------------------------------------------------------
// ListDelegates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DelegateListKind {
    Agent,
    Swarm,
    #[default]
    All,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DelegateListOrder {
    #[default]
    Newest,
    Oldest,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DelegateStateScope {
    #[default]
    Current,
    AnyRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum DelegateListInclude {
    Meta,
    Task,
    Summary,
    Activity,
}

fn default_delegate_list_include() -> Vec<DelegateListInclude> {
    vec![DelegateListInclude::Meta]
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListDelegatesInput {
    #[serde(default)]
    #[schemars(
        description = "Whether to include completed/cancelled delegates. Defaults to false (active only)."
    )]
    include_completed: bool,
    #[serde(default)]
    #[schemars(description = "Filter by delegate kind: agent, swarm, or all. Defaults to all.")]
    kind: DelegateListKind,
    #[serde(default)]
    #[schemars(
        description = "Filter by lifecycle state (e.g. running, completed, cancelled). Omit for any state."
    )]
    state: Option<AgentLifecycleState>,
    #[serde(default)]
    #[schemars(
        description = "When state is set, current matches only the current lifecycle state. any_run also matches terminal states recorded before resume."
    )]
    state_scope: DelegateStateScope,
    #[serde(default = "default_delegate_list_limit")]
    #[schemars(description = "Maximum number of rows to return. Defaults to 20.")]
    limit: usize,
    #[serde(default)]
    #[schemars(
        description = "Pagination cursor from a previous response's next_cursor. Omit for the first page."
    )]
    cursor: Option<String>,
    #[serde(default)]
    #[schemars(description = "Row ordering: newest (default) or oldest.")]
    order: DelegateListOrder,
    #[serde(default = "default_delegate_list_include")]
    #[schemars(
        description = "Fields to include in each row. Defaults to [\"meta\"]. Add task, summary, or activity only when needed."
    )]
    include: Vec<DelegateListInclude>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DelegateListCursor {
    offset: usize,
    query: DelegateListCursorQuery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DelegateListCursorQuery {
    include_completed: bool,
    kind: String,
    state: Option<String>,
    state_scope: String,
    order: String,
    include: Vec<String>,
}

impl DelegateListCursorQuery {
    fn from_input(input: &ListDelegatesInput, include_completed: bool) -> Self {
        Self {
            include_completed,
            kind: match input.kind {
                DelegateListKind::Agent => "agent",
                DelegateListKind::Swarm => "swarm",
                DelegateListKind::All => "all",
            }
            .to_owned(),
            state: input.state.map(|state| state.as_str().to_owned()),
            state_scope: state_scope_label(input.state_scope).to_owned(),
            order: match input.order {
                DelegateListOrder::Newest => "newest",
                DelegateListOrder::Oldest => "oldest",
            }
            .to_owned(),
            include: input.include.iter().map(include_label).collect(),
        }
    }
}

// Pass-by-ref kept for caller convenience (`iter().map(include_label)`); making
// the enum `Copy` is more invasive than this one-liner deserves.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn include_label(include: &DelegateListInclude) -> String {
    match include {
        DelegateListInclude::Meta => "meta",
        DelegateListInclude::Task => "task",
        DelegateListInclude::Summary => "summary",
        DelegateListInclude::Activity => "activity",
    }
    .to_owned()
}

fn empty_delegate_list_next_steps(
    input: &ListDelegatesInput,
    include_completed: bool,
    total: usize,
    offset: usize,
) -> Vec<String> {
    if total > 0 && offset >= total {
        return vec![
            "This page is empty because the cursor is past the available rows.".to_owned(),
            "Restart pagination by calling ListDelegates again without cursor.".to_owned(),
        ];
    }

    if let Some(state) = input.state {
        let kind = match input.kind {
            DelegateListKind::Agent => "agents",
            DelegateListKind::Swarm => "swarms",
            DelegateListKind::All => "delegates",
        };
        return vec![format!(
            "No {} {kind} found for the current query.",
            state.as_str()
        )];
    }

    if include_completed {
        return vec![
            "No delegates found in active or terminal history for the current query.".to_owned(),
        ];
    }

    vec![
        "No active delegates found.".to_owned(),
        "Pass include_completed=true to list completed, failed, cancelled, or timed_out delegates."
            .to_owned(),
    ]
}

fn state_scope_label(scope: DelegateStateScope) -> &'static str {
    match scope {
        DelegateStateScope::Current => "current",
        DelegateStateScope::AnyRun => "any_run",
    }
}

fn agent_matches_state(
    agent: &crate::multi_agent::AgentSnapshot,
    filter_state: AgentLifecycleState,
    state_scope: DelegateStateScope,
) -> bool {
    if agent.state == filter_state {
        return true;
    }
    matches!(state_scope, DelegateStateScope::AnyRun)
        && agent
            .terminal_status_history
            .iter()
            .copied()
            .any(|state| state == filter_state)
}

fn parse_list_cursor(
    tool: &str,
    cursor: Option<&str>,
    expected_query: &DelegateListCursorQuery,
) -> Result<usize, ToolError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "cursor must be a ListDelegates next_cursor value".to_owned(),
        })?;
    let decoded: DelegateListCursor =
        serde_json::from_slice(&bytes).map_err(|_| ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "cursor must be a ListDelegates next_cursor value".to_owned(),
        })?;
    if decoded.query != *expected_query {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message:
                "cursor was created for a different ListDelegates query; restart pagination without cursor"
                    .to_owned(),
        });
    }
    Ok(decoded.offset)
}

fn encode_list_cursor(
    tool: &str,
    offset: usize,
    query: &DelegateListCursorQuery,
) -> Result<String, ToolError> {
    let cursor = DelegateListCursor {
        offset,
        query: query.clone(),
    };
    let bytes = serde_json::to_vec(&cursor).map_err(|err| ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: format!("failed to encode ListDelegates cursor: {err}"),
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

pub struct ListDelegatesTool;

impl Tool for ListDelegatesTool {
    fn name(&self) -> &'static str {
        "ListDelegates"
    }

    fn description(&self) -> &'static str {
        "List delegate agents and/or swarms with their current status. \
         Defaults to newest-first, active-only, all kinds, and meta-only rows. \
         Pass include_completed=true to see completed, failed, cancelled, or timed_out history. \
         Use include=[\"task\"], include=[\"summary\"], or include=[\"activity\"] only when that extra context is needed. \
         Pagination cursors are valid only with the same query parameters that produced them."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<ListDelegatesInput>()
    }

    #[allow(clippy::too_many_lines)]
    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: ListDelegatesInput = parse_input(self.name(), input)?;
            if input.limit == 0 {
                return Err(ToolError::InvalidInput {
                    tool: self.name().to_owned(),
                    message: "limit must be >= 1".to_owned(),
                });
            }
            let include_completed = input.include_completed
                || input
                    .state
                    .is_some_and(crate::multi_agent::AgentLifecycleState::is_terminal);
            let cursor_query = DelegateListCursorQuery::from_input(&input, include_completed);
            let offset = parse_list_cursor(self.name(), input.cursor.as_deref(), &cursor_query)?;
            let limit = input.limit;
            let include = input.include.iter().map(include_label).collect::<Vec<_>>();
            let include_task = input.include.contains(&DelegateListInclude::Task);
            let include_summary = input.include.contains(&DelegateListInclude::Summary);
            let include_activity = input.include.contains(&DelegateListInclude::Activity);

            let show_agents = matches!(input.kind, DelegateListKind::Agent | DelegateListKind::All);
            let show_swarms = matches!(input.kind, DelegateListKind::Swarm | DelegateListKind::All);

            let mut all_rows: Vec<DelegateListRow> = Vec::new();

            if show_agents {
                let agents = ctx.multi_agent.list_agents(include_completed);
                for agent in &agents {
                    if let Some(filter_state) = input.state
                        && !agent_matches_state(agent, filter_state, input.state_scope)
                    {
                        continue;
                    }
                    let detail = format!(
                        "\n- agent_id: {} ({}) state: {} title: {}",
                        agent.id.as_str(),
                        agent.display_name.as_str(),
                        agent.state.as_str(),
                        agent.task_title,
                    );
                    let mut row = super::multi_agent_format::agent_details(
                        "agent",
                        agent,
                        None,
                        super::multi_agent_format::SummaryScope::None,
                        include_task,
                        include_summary,
                        include_activity,
                    );
                    row["kind"] = json!("agent");
                    row["current_status"] = json!(agent.state.as_str());
                    row["terminal_status_history"] = json!(
                        agent
                            .terminal_status_history
                            .iter()
                            .map(|state| state.as_str())
                            .collect::<Vec<_>>()
                    );
                    all_rows.push(DelegateListRow {
                        created_index: ctx
                            .multi_agent
                            .agent_created_index(agent.id.as_str())
                            .unwrap_or_default(),
                        id: agent.id.as_str().to_owned(),
                        detail,
                        json: row,
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
            let next_cursor = if page_end < total {
                Some(encode_list_cursor(self.name(), page_end, &cursor_query)?)
            } else {
                None
            };
            let page_rows = all_rows
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();

            let empty_next_steps =
                empty_delegate_list_next_steps(&input, include_completed, total, offset);
            let mut content = if page_rows.is_empty() {
                let mut content = "No delegates found.\n".to_owned();
                for step in &empty_next_steps {
                    let _ = writeln!(content, "next_step: {step}");
                }
                content
            } else {
                format!("total: {total}\n")
            };
            if let Some(cursor) = &next_cursor {
                let _ = writeln!(content, "next_cursor: {cursor}");
            }
            let rows: Vec<_> = page_rows.iter().map(|row| row.json.clone()).collect();
            for row in &page_rows {
                content.push_str(&row.detail);
            }

            let mut details = json!({
                "kind": "delegate_list",
                "count": page_rows.len(),
                "total": total,
                "next_cursor": next_cursor,
                "cursor_query": cursor_query,
                "include_completed": include_completed,
                "include": include,
                "order": match input.order {
                    DelegateListOrder::Newest => "newest",
                    DelegateListOrder::Oldest => "oldest",
                },
                "query": {
                    "kind": match input.kind {
                        DelegateListKind::Agent => "agent",
                        DelegateListKind::Swarm => "swarm",
                        DelegateListKind::All => "all",
                    },
                    "state": input.state.map(crate::multi_agent::AgentLifecycleState::as_str),
                    "state_scope": state_scope_label(input.state_scope),
                },
                "delegates": rows,
            });
            if page_rows.is_empty() {
                details["next_steps"] = json!(empty_next_steps);
            }
            Ok(ToolResult::ok(content).with_details(details))
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
         cancelled, timed_out). A wait timeout returns outcome=\"wait_timed_out\" while preserving \
         the target's current status; this differs from a case where the delegate itself reached timed_out. \
         For swarms, terminal results use the same structured shape as DelegateSwarm and TaskOutput."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<WaitDelegateInput>()
    }

    #[allow(clippy::too_many_lines)]
    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: WaitDelegateInput = parse_input(self.name(), input)?;
            let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(30_000));
            let deadline = std::time::Instant::now() + timeout;

            // Pre-check: if the ID doesn't exist anywhere, return immediately.
            let exists = if input.id.starts_with("swarm_") {
                ctx.multi_agent.swarm_snapshot(&input.id).is_some()
                    || ctx.background_tasks.snapshot(&input.id).await.is_ok()
            } else if input.id.starts_with("agent_") {
                ctx.multi_agent.agent_snapshot(&input.id).is_some()
                    || ctx.background_tasks.snapshot(&input.id).await.is_ok()
            } else {
                ctx.background_tasks.snapshot(&input.id).await.is_ok()
            };
            if !exists {
                return Ok(ToolResult::ok(format!(
                    "id: {}\nstatus: not_found\nnext_step: No delegate or background task with this ID exists. Check the ID or use ListDelegates to see available delegates.",
                    input.id,
                ))
                .with_details(json!({
                    "kind": "delegate_wait",
                    "task_id": input.id,
                    "outcome": "not_found",
                })));
            }

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
                        let mut details = json!({
                            "kind": "delegate_wait",
                            "id": input.id,
                            "task_id": input.id,
                            "status": "running",
                            "outcome": "wait_timed_out",
                        });
                        if let Some(swarm) = ctx.multi_agent.swarm_snapshot(&input.id) {
                            details["status"] = json!(swarm.state.as_str());
                            details["aggregate"] = json!(swarm.aggregate);
                        }
                        return Ok(ToolResult::ok(format!(
                            "id: {}\nstatus: {}\noutcome: wait_timed_out\nnext_step: The swarm is still running. Increase timeout_ms or use ListDelegates to check status.",
                            input.id,
                            details["status"].as_str().unwrap_or("running"),
                        ))
                        .with_details(details));
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
                    let mut details = super::multi_agent_format::agent_details(
                        "delegate_wait",
                        &snapshot,
                        Some(snapshot.context),
                        super::multi_agent_format::SummaryScope::CurrentRun,
                        true,
                        true,
                        false,
                    );
                    details["kind"] = json!("delegate_wait");
                    details["agent"] = json!(super::multi_agent_format::model_safe_agent_snapshot(
                        &snapshot
                    ));
                    details["outcome"] = json!(state_label);
                    return Ok(ToolResult::ok(format!(
                        "id: {}\nstatus: {}\nsummary: {}",
                        snapshot.id.as_str(),
                        state_label,
                        summary,
                    ))
                    .with_details(details));
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
                        "id: {}\nstatus: running\noutcome: wait_timed_out\nnext_step: The delegate is still running. Increase timeout_ms, call ListDelegates, or wait for automatic completion.",
                        input.id,
                    ))
                    .with_details(json!({
                        "kind": "delegate_wait",
                        "id": input.id,
                        "task_id": input.id,
                        "status": "running",
                        "outcome": "wait_timed_out",
                        "next_steps": [
                            "The delegate is still running.",
                            "Increase timeout_ms, call ListDelegates, or wait for automatic completion."
                        ],
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
                        let () = ctx
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
                            "swarm": super::multi_agent_format::model_safe_swarm_snapshot(&swarm),
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
                    return Ok(terminal_delegate_error(
                        agent.id.as_str(),
                        agent.state,
                        DelegateTerminalAction::Interrupt,
                    ));
                }
                let Some(snapshot) = ctx.multi_agent.cancel_agent(&agent_id) else {
                    return Ok(terminal_delegate_error(
                        agent.id.as_str(),
                        agent.state,
                        DelegateTerminalAction::Interrupt,
                    ));
                };
                let () = ctx
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
                    "agent": super::multi_agent_format::model_safe_agent_snapshot(&snapshot),
                    "outcome": "cancelled",
                })));
            }

            // Fall back to background task stop.
            if ctx.background_tasks.snapshot(&input.id).await.is_ok() {
                return match ctx
                    .background_tasks
                    .stop(&input.id, "Interrupted by InterruptDelegate", 1024)
                    .await
                {
                    Ok(result) => Ok(result),
                    Err(_) => Ok(delegate_target_not_found(&input.id)),
                };
            }

            Ok(delegate_target_not_found(&input.id))
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
        "Send a live follow-up message to a currently running delegate agent or broadcast \
         to running children of a swarm. The id may be an agent or swarm ID. \
         MessageDelegate does not queue offline messages for idle or terminal agents. \
         If the target is completed, failed, cancelled, timed_out, or not running, call Delegate with resume=\"agent_xxx\" instead."
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
                    .broadcast_live_swarm_message(&input.id, &input.message)
                {
                    Ok((delivered, skipped)) => {
                        let details = json!({
                            "target": input.id,
                            "delivered": delivered,
                            "skipped": skipped.iter().map(|(agent_id, state)| {
                                json!({ "agent_id": agent_id, "state": state.as_str() })
                            }).collect::<Vec<_>>(),
                        });
                        if delivered.is_empty() {
                            return Ok(ToolResult::error(format!(
                                "target: {}\nno running children to receive message\nskipped: {}",
                                input.id,
                                skipped
                                    .iter()
                                    .map(|(id, state)| format!("{id} ({})", state.as_str()))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            ))
                            .with_details(details));
                        }
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
                        .with_details(details));
                    }
                    Err(message) => {
                        return Ok(ToolResult::error(message));
                    }
                }
            }

            if let Some(agent) = ctx.multi_agent.agent_snapshot(&input.id)
                && agent.state.is_terminal()
            {
                return Ok(terminal_delegate_error(
                    agent.id.as_str(),
                    agent.state,
                    DelegateTerminalAction::Message,
                ));
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
    for child in &swarm.children {
        let _ = writeln!(
            content,
            "\n- index: {} agent_id: {} status: {}",
            child.item_index,
            child.agent.id.as_str(),
            child.agent.state.as_str(),
        );
    }
    ToolResult::ok(content).with_details(super::multi_agent_format::swarm_details(swarm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_context() -> ToolContext {
        let dir = tempfile::tempdir().expect("temp dir");
        ToolContext::new(dir.path()).expect("tool context")
    }

    #[tokio::test]
    async fn list_delegates_empty_steps_follow_state_filter() {
        let ctx = test_context();
        let tool = ListDelegatesTool;

        let result = tool
            .execute(
                &ctx,
                json!({
                    "include_completed": true,
                    "state": "cancelled"
                }),
            )
            .await
            .expect("list result");

        assert!(!result.is_error);
        assert!(result.content.contains("No delegates found."));
        assert!(result.content.contains("No cancelled delegates found"));
        assert!(!result.content.contains("Pass include_completed=true"));
        assert_eq!(
            result.details.as_ref().unwrap()["query"]["state"],
            "cancelled"
        );
        assert_eq!(result.details.as_ref().unwrap()["include_completed"], true);
    }

    #[tokio::test]
    async fn list_delegates_default_empty_steps_explain_active_default() {
        let ctx = test_context();
        let tool = ListDelegatesTool;

        let result = tool.execute(&ctx, json!({})).await.expect("list result");

        assert!(!result.is_error);
        assert!(result.content.contains("No active delegates found."));
        assert!(result.content.contains("Pass include_completed=true"));
    }

    #[tokio::test]
    async fn list_delegates_rejects_zero_limit() {
        let ctx = test_context();
        let tool = ListDelegatesTool;

        let err = tool
            .execute(&ctx, json!({ "limit": 0 }))
            .await
            .expect_err("zero limit should be invalid input");

        match err {
            ToolError::InvalidInput { tool, message } => {
                assert_eq!(tool, "ListDelegates");
                assert!(message.contains("limit must be >= 1"));
            }
            other => panic!("expected invalid input, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn interrupt_delegate_unknown_id_uses_delegate_error() {
        let ctx = test_context();
        let tool = InterruptDelegateTool;

        let result = tool
            .execute(&ctx, json!({ "id": "agent_missing" }))
            .await
            .expect("tool should return result");

        assert!(result.is_error);
        assert_eq!(result.content, "unknown delegate target `agent_missing`");
        assert!(!result.content.contains("TaskStop"));
        assert!(!result.content.contains("background task"));
        assert_eq!(result.details.as_ref().unwrap()["kind"], "delegate_target");
        assert_eq!(result.details.as_ref().unwrap()["outcome"], "not_found");
    }

    #[tokio::test]
    async fn terminal_delegate_errors_are_action_specific() {
        let ctx = test_context();
        let agent = ctx
            .multi_agent
            .start_foreground_delegate_for_test("calculate 2 + 2");
        let _ = ctx
            .multi_agent
            .complete_delegate_for_test(&agent.id, "The answer is 4.");

        let message_result = MessageDelegateTool
            .execute(
                &ctx,
                json!({ "id": agent.id.as_str(), "message": "another question" }),
            )
            .await
            .expect("message result");
        assert!(message_result.is_error);
        assert!(
            message_result
                .content
                .contains("cannot receive live messages")
        );
        assert!(!message_result.content.contains("be interrupted"));
        assert_eq!(
            message_result.details.as_ref().unwrap()["action"],
            "message"
        );

        let interrupt_result = InterruptDelegateTool
            .execute(&ctx, json!({ "id": agent.id.as_str() }))
            .await
            .expect("interrupt result");
        assert!(interrupt_result.is_error);
        assert!(interrupt_result.content.contains("cannot be interrupted"));
        assert!(!interrupt_result.content.contains("live messages"));
        assert_eq!(
            interrupt_result.details.as_ref().unwrap()["action"],
            "interrupt"
        );
    }

    #[tokio::test]
    async fn list_delegates_any_run_state_finds_resumed_cancelled_agent() {
        let ctx = test_context();
        let agent = ctx
            .multi_agent
            .start_foreground_delegate_for_test("first run");
        let cancelled = ctx
            .multi_agent
            .cancel_agent_by_id(agent.id.as_str())
            .expect("agent cancelled");
        assert_eq!(cancelled.state, AgentLifecycleState::Cancelled);

        ctx.multi_agent
            .start_resume_delegate(
                agent.id.as_str(),
                &crate::multi_agent::DelegateRequest {
                    task: "second run".to_owned(),
                    resume: Some(agent.id.as_str().to_owned()),
                    title: None,
                    role: None,
                    mode: crate::multi_agent::AgentRunMode::Foreground,
                    context: crate::multi_agent::DelegateContext::None,
                },
            )
            .expect("resume starts");
        let _ = ctx
            .multi_agent
            .complete_delegate_for_test(&agent.id, "second run done");

        let result = ListDelegatesTool
            .execute(
                &ctx,
                json!({
                    "include_completed": true,
                    "state": "cancelled",
                    "state_scope": "any_run"
                }),
            )
            .await
            .expect("list result");

        assert!(!result.is_error);
        assert!(result.content.contains(agent.id.as_str()));
        let details = result.details.as_ref().unwrap();
        assert_eq!(details["query"]["state"], "cancelled");
        assert_eq!(details["query"]["state_scope"], "any_run");
        assert_eq!(details["delegates"][0]["current_status"], "completed");
        assert_eq!(
            details["delegates"][0]["terminal_status_history"][0],
            "cancelled"
        );
    }

    #[tokio::test]
    async fn list_delegates_current_state_does_not_match_resumed_cancelled_agent() {
        let ctx = test_context();
        let agent = ctx
            .multi_agent
            .start_foreground_delegate_for_test("first run");
        ctx.multi_agent
            .cancel_agent_by_id(agent.id.as_str())
            .expect("agent cancelled");
        ctx.multi_agent
            .start_resume_delegate(
                agent.id.as_str(),
                &crate::multi_agent::DelegateRequest {
                    task: "second run".to_owned(),
                    resume: Some(agent.id.as_str().to_owned()),
                    title: None,
                    role: None,
                    mode: crate::multi_agent::AgentRunMode::Foreground,
                    context: crate::multi_agent::DelegateContext::None,
                },
            )
            .expect("resume starts");
        let _ = ctx
            .multi_agent
            .complete_delegate_for_test(&agent.id, "second run done");

        let result = ListDelegatesTool
            .execute(
                &ctx,
                json!({
                    "include_completed": true,
                    "state": "cancelled"
                }),
            )
            .await
            .expect("list result");

        assert!(!result.is_error);
        assert!(result.content.contains("No cancelled delegates found"));
    }

    #[tokio::test]
    async fn list_delegates_any_run_history_preserves_repeated_terminal_states() {
        let ctx = test_context();
        let agent = ctx
            .multi_agent
            .start_foreground_delegate_for_test("first run");
        let _ = ctx
            .multi_agent
            .complete_delegate_for_test(&agent.id, "first run done");

        ctx.multi_agent
            .start_resume_delegate(
                agent.id.as_str(),
                &crate::multi_agent::DelegateRequest {
                    task: "second run".to_owned(),
                    resume: Some(agent.id.as_str().to_owned()),
                    title: None,
                    role: None,
                    mode: crate::multi_agent::AgentRunMode::Foreground,
                    context: crate::multi_agent::DelegateContext::None,
                },
            )
            .expect("second run starts");
        let _ = ctx
            .multi_agent
            .complete_delegate_for_test(&agent.id, "second run done");
        ctx.multi_agent
            .start_resume_delegate(
                agent.id.as_str(),
                &crate::multi_agent::DelegateRequest {
                    task: "third run".to_owned(),
                    resume: Some(agent.id.as_str().to_owned()),
                    title: None,
                    role: None,
                    mode: crate::multi_agent::AgentRunMode::Foreground,
                    context: crate::multi_agent::DelegateContext::None,
                },
            )
            .expect("third run starts");

        let result = ListDelegatesTool
            .execute(
                &ctx,
                json!({
                    "state": "completed",
                    "state_scope": "any_run"
                }),
            )
            .await
            .expect("list result");

        assert!(!result.is_error);
        let history = &result.details.as_ref().unwrap()["delegates"][0]["terminal_status_history"];
        assert_eq!(history, &json!(["completed", "completed"]));
    }

    #[tokio::test]
    async fn delegate_control_results_strip_live_queue_metadata() {
        let ctx = test_context();
        let agent = ctx
            .multi_agent
            .start_foreground_delegate_for_test("queued command");
        let started_at = std::time::Instant::now();
        let _ = ctx.multi_agent.apply_child_event(
            &agent.id,
            started_at,
            &crate::AgentEvent::ToolExecutionQueued {
                turn: 1,
                id: "bash-queued".to_owned(),
                name: "Bash".to_owned(),
                arguments: json!({"command": "cargo test"}),
            },
        );
        let _ = ctx.multi_agent.apply_child_event(
            &agent.id,
            started_at,
            &crate::AgentEvent::ToolExecutionQueueUpdated {
                turn: 1,
                id: "bash-queued".to_owned(),
                position: 2,
                waiting_ms: 18_000,
            },
        );
        let interrupted = InterruptDelegateTool
            .execute(&ctx, json!({"id": agent.id.as_str()}))
            .await
            .expect("interrupt queued agent");
        assert_queue_metadata_cleared(
            &interrupted.details.as_ref().expect("interrupt details")["agent"]["activity"][0]["kind"]
                ["phase"],
        );

        let listed = ListDelegatesTool
            .execute(
                &ctx,
                json!({
                    "include_completed": true,
                    "include": ["activity"]
                }),
            )
            .await
            .expect("list delegates");
        assert_queue_metadata_cleared(
            &listed.details.as_ref().expect("list details")["delegates"][0]["activity_tail"][0]["kind"]
                ["phase"],
        );

        let waited = WaitDelegateTool
            .execute(&ctx, json!({"id": agent.id.as_str(), "timeout_ms": 1}))
            .await
            .expect("wait delegate");
        assert_queue_metadata_cleared(
            &waited.details.as_ref().expect("wait details")["agent"]["activity"][0]["kind"]["phase"],
        );
    }

    fn assert_queue_metadata_cleared(phase: &serde_json::Value) {
        assert_eq!(phase["queued"]["position"], serde_json::Value::Null);
        assert_eq!(phase["queued"]["queued_at_ms"], 0);
    }
}
