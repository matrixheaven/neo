use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::Duration;

use base64::Engine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};
use crate::multi_agent::AgentLifecycleState;

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
        description = "Opaque pagination cursor returned as next_cursor by a previous ListDelegates response. Omit this field for the first page; when continuing, pass that value unchanged. Do not pass an empty string, \"0\", or a self-constructed cursor."
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
    let Some(cursor) = cursor.map(str::trim).filter(|cursor| !cursor.is_empty()) else {
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

const MAX_DELEGATE_LIST_FIELD_CHARS: usize = 4096;
const DELEGATE_LIST_TRUNCATION_MARKER: &str = "...";
const DELEGATE_LIST_CONTENT_TRUNCATION_SUFFIX: &str =
    "\n[delegate list output truncated; request a smaller limit or omit optional fields]";

fn append_delegate_list_field(detail: &mut String, label: &str, value: &str) {
    let mut value = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if value.chars().count() > MAX_DELEGATE_LIST_FIELD_CHARS {
        let keep = MAX_DELEGATE_LIST_FIELD_CHARS - DELEGATE_LIST_TRUNCATION_MARKER.len();
        value = value.chars().take(keep).collect();
        value.push_str(DELEGATE_LIST_TRUNCATION_MARKER);
    }
    let _ = writeln!(detail, "  {label}: {value}");
}

fn truncate_delegate_list_content(content: &str, max_bytes: usize) -> String {
    let mut end = max_bytes.min(content.len());
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    content[..end].to_owned()
}

fn cap_delegate_list_content(content: String, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content;
    }
    if max_bytes <= DELEGATE_LIST_CONTENT_TRUNCATION_SUFFIX.len() {
        return truncate_delegate_list_content(&content, max_bytes);
    }
    let mut capped = truncate_delegate_list_content(
        &content,
        max_bytes - DELEGATE_LIST_CONTENT_TRUNCATION_SUFFIX.len(),
    );
    capped.push_str(DELEGATE_LIST_CONTENT_TRUNCATION_SUFFIX);
    capped
}

fn collect_delegate_list_rows(
    ctx: &ToolContext,
    input: &ListDelegatesInput,
    include_completed: bool,
    include_task: bool,
    include_summary: bool,
    include_activity: bool,
) -> Vec<DelegateListRow> {
    let show_agents = matches!(input.kind, DelegateListKind::Agent | DelegateListKind::All);
    let show_swarms = matches!(input.kind, DelegateListKind::Swarm | DelegateListKind::All);
    let mut rows = Vec::new();
    if show_agents {
        let agents = ctx.multi_agent.list_agents(include_completed);
        for agent in &agents {
            if let Some(filter_state) = input.state
                && !agent_matches_state(agent, filter_state, input.state_scope)
            {
                continue;
            }
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
            let mut detail = format!(
                "\n- agent_id: {} ({}) state: {} title: {}",
                agent.id.as_str(),
                agent.display_name.as_str(),
                agent.state.as_str(),
                agent.task_title,
            );
            if include_task && let Some(task) = row.get("task").and_then(Value::as_str) {
                append_delegate_list_field(&mut detail, "task", task);
            }
            if include_summary
                && let Some(summary) = row.get("summary").and_then(Value::as_str)
                && !summary.is_empty()
            {
                append_delegate_list_field(&mut detail, "summary", summary);
            }
            if include_activity && let Some(activity) = row.get("activity_tail") {
                let activity = serde_json::to_string(activity).unwrap_or_else(|_| "[]".to_owned());
                append_delegate_list_field(&mut detail, "activity_tail", &activity);
            }
            rows.push(DelegateListRow {
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
            rows.push(DelegateListRow {
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
    rows
}

pub struct ListDelegatesTool;

impl Tool for ListDelegatesTool {
    fn name(&self) -> &'static str {
        "ListDelegates"
    }

    fn description(&self) -> &'static str {
        "Take a point-in-time snapshot of delegate agents and/or swarms. This tool does not wait. \
         Never poll it with Sleep; use WaitDelegate when a known agent or swarm must reach a terminal state. \
         Defaults to newest-first, active-only, all kinds, and meta-only rows. \
         Pass include_completed=true to see completed, failed, cancelled, or timed_out history. \
         Use include=[\"task\"], include=[\"summary\"], or include=[\"activity\"] only when that extra context is needed. \
         Pagination cursors are valid only with the same query parameters that produced them. \
         This tool is for discovery and status only; use Delegate, WaitDelegate, or TaskOutput(view=\"result\") to read final responses."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<ListDelegatesInput>()
    }

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

            let mut all_rows = collect_delegate_list_rows(
                ctx,
                &input,
                include_completed,
                include_task,
                include_summary,
                include_activity,
            );

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
            let content = cap_delegate_list_content(content, ctx.max_output_bytes);

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
    #[schemars(
        description = "Non-empty unique agent and/or swarm IDs to wait for. Use an array with one item for a single target."
    )]
    ids: Vec<String>,
    #[schemars(
        description = "One global maximum wait in milliseconds for all targets. Defaults to 30000 (30s). Returns outcome=wait_timed_out with partial results if any target has not finished."
    )]
    timeout_ms: Option<u64>,
}

struct WaitTargetSnapshot {
    details: Value,
    found: bool,
    terminal: bool,
}

fn wait_target_snapshot(ctx: &ToolContext, id: &str) -> WaitTargetSnapshot {
    if id.starts_with("swarm_")
        && let Some(swarm) = ctx.multi_agent.swarm_snapshot(id)
    {
        return WaitTargetSnapshot {
            details: super::multi_agent_format::swarm_details(&swarm),
            found: true,
            terminal: swarm.state.is_terminal(),
        };
    }
    if id.starts_with("agent_")
        && let Some(agent) = ctx.multi_agent.agent_snapshot(id)
    {
        return WaitTargetSnapshot {
            details: super::multi_agent_format::agent_details(
                "delegate",
                &agent,
                Some(agent.context),
                super::multi_agent_format::SummaryScope::CurrentRun,
                true,
                true,
                true,
            ),
            found: true,
            terminal: agent.state.is_terminal(),
        };
    }
    WaitTargetSnapshot {
        details: json!({
            "kind": "delegate_target",
            "id": id,
            "status": "not_found",
        }),
        found: false,
        terminal: false,
    }
}

async fn wait_delegate_result(
    ctx: &ToolContext,
    snapshots: Vec<WaitTargetSnapshot>,
    outcome: &'static str,
) -> Result<ToolResult, ToolError> {
    let total = snapshots.len();
    let terminal = snapshots
        .iter()
        .filter(|snapshot| snapshot.terminal)
        .count();
    let not_found = snapshots.iter().filter(|snapshot| !snapshot.found).count();
    let pending = total.saturating_sub(terminal + not_found);
    let mut items = Vec::with_capacity(snapshots.len());
    let mut detail_items = Vec::with_capacity(snapshots.len());
    let mut pending_ids = Vec::with_capacity(pending);
    for snapshot in snapshots {
        let details = snapshot.details;
        let id = details
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if snapshot.found && !snapshot.terminal {
            pending_ids.push(id.clone());
        }
        let status = details
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        detail_items.push(details.clone());
        if outcome == "all_terminal"
            && id.starts_with("swarm_")
            && let Some(swarm) = ctx.multi_agent.swarm_snapshot(&id)
            && swarm.state.as_str() == status
        {
            let pages = ctx
                .multi_agent
                .swarm_result_pages(&swarm, ctx.max_output_bytes)
                .await
                .map_err(|message| ToolError::InvalidInput {
                    tool: "WaitDelegate".to_owned(),
                    message: format!("failed to read swarm result: {message}"),
                })?;
            let content = super::multi_agent_format::swarm_result_content(
                &swarm,
                &pages,
                ctx.max_output_bytes,
            );
            items.push(serde_json::from_str(&content).unwrap_or(details));
            continue;
        }
        if outcome == "all_terminal"
            && id.starts_with("agent_")
            && let Some(agent) = ctx.multi_agent.agent_snapshot(&id)
            && agent.state.as_str() == status
        {
            let page = ctx
                .multi_agent
                .agent_result_page(&id, 0, ctx.max_output_bytes)
                .await
                .map_err(|message| ToolError::InvalidInput {
                    tool: "WaitDelegate".to_owned(),
                    message: format!("failed to read delegate result: {message}"),
                })?;
            let content = super::multi_agent_format::delegate_model_result_content(
                &agent,
                agent.context,
                page,
                ctx.max_output_bytes,
            );
            items.push(serde_json::from_str(&content).unwrap_or(details));
        } else {
            items.push(details);
        }
    }
    let mut next_actions = Vec::new();
    if outcome == "wait_timed_out" {
        next_actions.push(json!({
            "tool": "WaitDelegate",
            "arguments": {
                "ids": pending_ids,
                "timeout_ms": 30_000,
            },
        }));
    }
    let content = serde_json::to_string(&json!({
        "ok": outcome == "all_terminal",
        "kind": "delegate_wait",
        "outcome": outcome,
        "aggregate": {
            "total": total,
            "terminal": terminal,
            "pending": pending,
            "not_found": not_found,
        },
        "items": items,
        "next_actions": next_actions,
    }))
    .expect("delegate wait JSON serializes");
    Ok(ToolResult::ok(content).with_details(json!({
        "kind": "delegate_wait",
        "outcome": outcome,
        "aggregate": {
            "total": total,
            "terminal": terminal,
            "pending": pending,
            "not_found": not_found,
        },
        "items": detail_items,
    })))
}

pub struct WaitDelegateTool;

impl Tool for WaitDelegateTool {
    fn name(&self) -> &'static str {
        "WaitDelegate"
    }

    fn description(&self) -> &'static str {
        "Canonical blocking wait for one or more known delegate agents or swarms. Pass every target in ids; \
         the call returns when all targets are terminal (completed, failed, cancelled, timed_out) or one global \
         timeout expires. A wait timeout returns outcome=\"wait_timed_out\" with completed results and current \
         unfinished snapshots; this differs from a case where the delegate itself reached timed_out. \
         When all targets are terminal, the result content includes each complete response or an exact TaskOutput(view=\"result\") action for an oversized response."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<WaitDelegateInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: WaitDelegateInput = parse_input(self.name(), input)?;
            if input.ids.is_empty() {
                return Ok(ToolResult::error(
                    "ids must contain at least one agent or swarm ID",
                ));
            }
            let mut seen = HashSet::with_capacity(input.ids.len());
            if let Some(duplicate) = input.ids.iter().find(|id| !seen.insert(id.as_str())) {
                return Ok(ToolResult::error(format!(
                    "ids must not contain duplicate target `{duplicate}`"
                )));
            }
            let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(30_000));
            let deadline = std::time::Instant::now() + timeout;

            loop {
                let snapshots = input
                    .ids
                    .iter()
                    .map(|id| wait_target_snapshot(ctx, id))
                    .collect::<Vec<_>>();
                if snapshots.iter().any(|snapshot| !snapshot.found) {
                    return Ok(wait_delegate_result(ctx, snapshots, "not_found").await?);
                }
                if snapshots.iter().all(|snapshot| snapshot.terminal) {
                    return Ok(wait_delegate_result(ctx, snapshots, "all_terminal").await?);
                }
                if std::time::Instant::now() >= deadline {
                    return Ok(wait_delegate_result(ctx, snapshots, "wait_timed_out").await?);
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

            // Atomic delivery is the sole authority: no snapshot precheck. Map
            // typed NotRunning/Unknown outcomes to action-specific errors after
            // the registry reports non-delivery.
            match ctx
                .multi_agent
                .deliver_live_agent_message(&input.id, input.message.clone())
            {
                Ok(()) => Ok(ToolResult::ok(format!(
                    "target: {}\noutcome: delivered\nmessage: {}",
                    input.id, input.message
                ))
                .with_details(json!({
                    "target": input.id,
                    "outcome": "delivered",
                    "delivered": [input.id],
                    "message": input.message,
                }))),
                Err(message) => {
                    let target = input.id.as_str();
                    if message.contains("unknown delegate target") {
                        return Ok(delegate_target_not_found(target));
                    }
                    if let Some(agent) = ctx.multi_agent.agent_snapshot(target)
                        && agent.state.is_terminal()
                    {
                        return Ok(terminal_delegate_error(
                            agent.id.as_str(),
                            agent.state,
                            DelegateTerminalAction::Message,
                        ));
                    }
                    Ok(ToolResult::error(message))
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
#[cfg(test)]
#[path = "test_cases/delegate_controls.rs"]
mod tests;
