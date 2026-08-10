use std::time::{Duration, SystemTime, UNIX_EPOCH};

use neo_agent_core::tools::{
    BackgroundTaskKind, BackgroundTaskSnapshot, BackgroundTaskStatus, CommandOutput,
};
use neo_tui::tasks_browser::{
    TaskBrowserItem, TaskBrowserKind, TaskBrowserPendingUserRequest, TaskBrowserSnapshot,
    TaskBrowserStatus, TaskBrowserWorkflowChild, TaskBrowserWorkflowChildPage,
    TaskBrowserWorkflowRowState, TaskBrowserWorkflowStep,
};

#[must_use]
#[allow(dead_code)] // retained for unit tests / non-paged callers
pub fn snapshots_to_browser_snapshot(snapshots: &[BackgroundTaskSnapshot]) -> TaskBrowserSnapshot {
    TaskBrowserSnapshot::new(snapshots.iter().map(snapshot_to_item).collect())
}

/// Build a browser snapshot from a paged list response (design §38).
#[must_use]
pub fn list_page_to_browser_snapshot(
    page: &neo_agent_core::tools::BackgroundTaskListPage,
) -> TaskBrowserSnapshot {
    TaskBrowserSnapshot {
        items: page.items.iter().map(snapshot_to_item).collect(),
        next_cursor: page.next_cursor.clone(),
        has_more: page.has_more,
        query_hash: Some(page.query_hash.clone()),
        total_matched: Some(page.total_matched),
    }
}

#[must_use]
pub fn preferred_workflow_task_id(
    page: &neo_agent_core::tools::BackgroundTaskListPage,
) -> Option<String> {
    let latest = |requires_input: bool| {
        page.items
            .iter()
            .filter(|item| item.kind == BackgroundTaskKind::Workflow)
            .filter(|item| {
                item.workflow.as_ref().is_some_and(|workflow| {
                    if requires_input {
                        workflow.pending_user.is_some()
                    } else {
                        item.status.is_active()
                    }
                })
            })
            .max_by_key(|item| {
                item.workflow
                    .as_ref()
                    .and_then(|workflow| workflow.updated_at_ms)
                    .unwrap_or_default()
            })
            .map(|item| item.task_id.clone())
    };
    latest(true).or_else(|| latest(false))
}

pub async fn update_workflow_save_availability(
    page: &mut neo_agent_core::tools::BackgroundTaskListPage,
    background_tasks: &neo_agent_core::tools::BackgroundTaskManager,
    definitions: &neo_agent_core::workflow::WorkflowDefinitionRegistry,
) {
    let candidates = page
        .items
        .iter()
        .filter_map(|item| {
            item.workflow
                .as_ref()
                .filter(|workflow| workflow.inline_unsaved)
                .map(|_| item.task_id.clone())
        })
        .collect::<Vec<_>>();
    for task_id in candidates {
        let Some(handle) = background_tasks.workflow_handle(&task_id).await else {
            continue;
        };
        let Ok(output) = handle.output().await else {
            continue;
        };
        if workflow_definition_is_saved(&output.metadata, definitions)
            && let Some(workflow) = page
                .items
                .iter_mut()
                .find(|item| item.task_id == task_id)
                .and_then(|item| item.workflow.as_mut())
        {
            workflow.inline_unsaved = false;
        }
    }
}

fn workflow_definition_is_saved(
    metadata: &neo_agent_core::workflow::WorkflowRunMetadata,
    definitions: &neo_agent_core::workflow::WorkflowDefinitionRegistry,
) -> bool {
    let Some(output_schema) = metadata.output_schema.as_ref() else {
        return false;
    };
    let display_name = metadata.display_name.as_deref().unwrap_or(&metadata.name);
    definitions
        .list(neo_agent_core::workflow::WorkflowListScope::Effective)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|summary| definitions.resolve(summary.name.as_str()).ok())
        .any(|definition| {
            definition.source_sha256 == metadata.script_sha256
                && definition.display_name == display_name
                && definition.description == metadata.description
                && definition.phases == metadata.phases
                && input_schema_matches(
                    definition.input_schema.as_ref(),
                    metadata.input_schema.as_ref(),
                )
                && &definition.output_schema == output_schema
        })
}

/// Resolved definitions fill a missing `input_schema` with a default object
/// schema at resolve time, while inline run metadata keeps `None`. Compare
/// both sides normalized so a saved workflow with no explicit input schema
/// still matches its inline origin.
fn input_schema_matches(
    definition: Option<&serde_json::Value>,
    metadata: Option<&serde_json::Value>,
) -> bool {
    fn normalize(schema: Option<&serde_json::Value>) -> serde_json::Value {
        schema.cloned().unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
            })
        })
    }
    normalize(definition) == normalize(metadata)
}

fn workflow_browser_meta(
    meta: &neo_agent_core::tools::WorkflowTaskProjection,
) -> neo_tui::tasks_browser::TaskBrowserWorkflowMeta {
    neo_tui::tasks_browser::TaskBrowserWorkflowMeta {
        run_id: meta.run_id.clone(),
        display_name: meta.display_name.clone(),
        purpose: meta.purpose.clone(),
        elapsed_ms: meta.elapsed_ms,
        current_step_key: meta.current_step_key.clone(),
        steps: meta
            .steps
            .iter()
            .map(|step| TaskBrowserWorkflowStep {
                key: step.key.clone(),
                title: step.title.clone(),
                state: match step.state {
                    neo_agent_core::workflow::StepRowState::Pending => {
                        TaskBrowserWorkflowRowState::Pending
                    }
                    neo_agent_core::workflow::StepRowState::Active => {
                        TaskBrowserWorkflowRowState::Working
                    }
                    neo_agent_core::workflow::StepRowState::Completed => {
                        TaskBrowserWorkflowRowState::Completed
                    }
                    neo_agent_core::workflow::StepRowState::Failed => {
                        TaskBrowserWorkflowRowState::Failed
                    }
                    neo_agent_core::workflow::StepRowState::Paused => {
                        TaskBrowserWorkflowRowState::Paused
                    }
                },
                done_count: step.done_count,
                working_count: step.working_count,
                queued_count: step.queued_count,
                failed_count: step.failed_count,
            })
            .collect(),
        child_page: meta
            .child_page
            .as_ref()
            .map_or_else(TaskBrowserWorkflowChildPage::default, workflow_child_page),
        pending_user: meta
            .pending_user
            .as_ref()
            .map(|request| TaskBrowserPendingUserRequest {
                request_id: request.request_id.clone(),
                prompt: request.prompt.clone(),
                answer_schema: request
                    .answer_schema
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({})),
                default: request.default.clone(),
                title: request.title.clone(),
                answer_policy: request.answer_policy.clone(),
            }),
        inline_unsaved: meta.inline_unsaved,
    }
}

fn workflow_child_page(
    page: &neo_agent_core::workflow::WorkflowChildPage,
) -> TaskBrowserWorkflowChildPage {
    TaskBrowserWorkflowChildPage {
        items: page.items.iter().map(workflow_child_row).collect(),
        next_cursor: page.next_cursor.clone(),
        has_more: page.has_more,
        query_hash: page.query_hash.clone(),
    }
}

fn workflow_child_row(
    child: &neo_agent_core::workflow::WorkflowChildRow,
) -> TaskBrowserWorkflowChild {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let end_ms = child.terminal_at_ms.unwrap_or(now_ms);
    let elapsed = child.started_at_ms.map_or(Duration::ZERO, |started| {
        Duration::from_millis(end_ms.saturating_sub(started))
    });
    TaskBrowserWorkflowChild {
        key: child.key.clone(),
        title: child
            .title
            .clone()
            .unwrap_or_else(|| match child.child_kind {
                neo_agent_core::workflow::WorkflowChildKind::Delegate => "Delegate".to_owned(),
                neo_agent_core::workflow::WorkflowChildKind::SwarmItem => "Work item".to_owned(),
            }),
        role: child.role.clone(),
        state: match child.state {
            neo_agent_core::workflow::WorkflowChildState::Queued => {
                TaskBrowserWorkflowRowState::Pending
            }
            neo_agent_core::workflow::WorkflowChildState::Running => {
                TaskBrowserWorkflowRowState::Working
            }
            neo_agent_core::workflow::WorkflowChildState::Completed => {
                TaskBrowserWorkflowRowState::Completed
            }
            neo_agent_core::workflow::WorkflowChildState::Failed
            | neo_agent_core::workflow::WorkflowChildState::Cancelled
            | neo_agent_core::workflow::WorkflowChildState::Interrupted => {
                TaskBrowserWorkflowRowState::Failed
            }
            neo_agent_core::workflow::WorkflowChildState::Recovering => {
                TaskBrowserWorkflowRowState::Recovering
            }
        },
        elapsed: format_elapsed(elapsed),
        actual_usage: child.actual_usage.clone(),
        latest_activity: child.latest_activity.clone(),
        terminal_summary: child
            .error_summary
            .clone()
            .or_else(|| child.terminal_summary.clone()),
        generated_files: child.generated_files.clone(),
    }
}

#[must_use]
pub fn snapshot_to_item(snapshot: &BackgroundTaskSnapshot) -> TaskBrowserItem {
    let kind = match snapshot.kind {
        BackgroundTaskKind::Bash => TaskBrowserKind::Bash,
        BackgroundTaskKind::Question => TaskBrowserKind::Question,
        BackgroundTaskKind::Delegate => TaskBrowserKind::Delegate,
        BackgroundTaskKind::DelegateSwarm => TaskBrowserKind::DelegateSwarm,
        BackgroundTaskKind::Workflow => TaskBrowserKind::Workflow,
    };
    let status = map_status(snapshot.status);
    let title = snapshot.delegate.as_ref().map_or_else(
        || snapshot.description.clone(),
        |agent| agent.task_title.clone(),
    );
    let human_handle = snapshot
        .workflow
        .as_ref()
        .and_then(|w| w.human_handle.clone());
    let title = if let Some(handle) = human_handle.as_ref() {
        handle.clone()
    } else {
        title
    };
    TaskBrowserItem {
        id: snapshot.task_id.clone(),
        kind,
        status,
        title,
        description: snapshot.description.clone(),
        elapsed: format_elapsed(snapshot.elapsed),
        detail_lines: detail_lines(snapshot, status),
        preview_lines: preview_lines(snapshot),
        can_stop: snapshot.status.is_active(),
        human_handle,
        list_cursor: None,
        workflow: snapshot.workflow.as_ref().map(workflow_browser_meta),
    }
}

fn detail_lines(snapshot: &BackgroundTaskSnapshot, status: TaskBrowserStatus) -> Vec<String> {
    match snapshot.kind {
        BackgroundTaskKind::Bash | BackgroundTaskKind::Question => {
            let description_label = match snapshot.kind {
                BackgroundTaskKind::Bash => "description",
                BackgroundTaskKind::Question => "prompt",
                _ => unreachable!(),
            };
            vec![
                format!("id:          {}", snapshot.task_id),
                format!("kind:        {}", snapshot.kind.as_str()),
                format!("status:      {}", status.label()),
                format!("elapsed:     {}", format_elapsed(snapshot.elapsed)),
                format!("{description_label}: {}", snapshot.description),
            ]
        }
        BackgroundTaskKind::Delegate => {
            let mut lines = vec![
                format!("id:          {}", snapshot.task_id),
                format!("kind:        {}", snapshot.kind.as_str()),
                format!("status:      {}", status.label()),
                format!("elapsed:     {}", format_elapsed(snapshot.elapsed)),
            ];
            if let Some(agent) = &snapshot.delegate {
                lines.push(format!("name:        {}", agent.display_name.as_str()));
                lines.push(format!("mode:        {:?}", agent.mode));
                lines.push(format!("tokens:      {}", agent.token_count));
                lines.push(format!("tools:       {}", agent.tool_count));
                lines.push(format!("task:        {}", agent.task_title));
                if let Some(outcome) = &agent.outcome {
                    lines.push(format!("summary:     {}", outcome.summary));
                }
                if let Some(text) = &agent.latest_text {
                    lines.push(format!("latest:      {text}"));
                }
                for activity in agent.activity.iter().rev().take(4).rev() {
                    lines.push(format!("activity:    {}", format_agent_activity(activity)));
                }
            }
            lines
        }
        BackgroundTaskKind::DelegateSwarm => {
            let mut lines = vec![
                format!("id:          {}", snapshot.task_id),
                format!("kind:        {}", snapshot.kind.as_str()),
                format!("status:      {}", status.label()),
                format!("elapsed:     {}", format_elapsed(snapshot.elapsed)),
            ];
            if let Some(swarm) = &snapshot.swarm {
                lines.push(format!("swarm_id:    {}", swarm.swarm_id));
                lines.push(format!("status:      {}", swarm.state.as_str()));
                lines.push(format!(
                    "aggregate:   total={} queued={} running={} completed={} failed={} cancelled={} timed_out={}",
                    swarm.aggregate.total,
                    swarm.aggregate.queued,
                    swarm.aggregate.running,
                    swarm.aggregate.completed,
                    swarm.aggregate.failed,
                    swarm.aggregate.cancelled,
                    swarm.aggregate.timed_out,
                ));
                let completed = swarm.aggregate.completed;
                lines.push(format!(
                    "progress:    {}/{}",
                    completed,
                    swarm.children.len()
                ));
                lines.push(format!("children:    {}", swarm.children.len()));
                for child in &swarm.children {
                    let result = child
                        .agent
                        .outcome
                        .as_ref()
                        .map_or(child.agent.task_title.as_str(), |outcome| {
                            outcome.summary.as_str()
                        });
                    lines.push(format!(
                        "  {} {} {} {}",
                        child.item_index,
                        child.agent.id.as_str(),
                        child.agent.state.as_str(),
                        result
                    ));
                }
            }
            lines
        }
        BackgroundTaskKind::Workflow => {
            let mut lines = vec![
                format!("id:          {}", snapshot.task_id),
                format!("kind:        {}", snapshot.kind.as_str()),
                format!("status:      {}", status.label()),
                format!("elapsed:     {}", format_elapsed(snapshot.elapsed)),
                format!("description: {}", snapshot.description),
            ];
            if let Some(meta) = &snapshot.workflow {
                if let Some(handle) = &meta.human_handle {
                    lines.push(format!("handle:      {handle}"));
                }
                lines.push(format!("definition:  {}", meta.definition_name));
                if let Some(rev) = &meta.definition_revision {
                    let short = if rev.len() > 12 {
                        &rev[..12]
                    } else {
                        rev.as_str()
                    };
                    lines.push(format!("revision:    {short}"));
                }
                if let Some(scope) = &meta.source_scope {
                    lines.push(format!("origin:      {scope}"));
                }
                if let Some(phase) = &meta.current_phase {
                    lines.push(format!("phase:       {phase}"));
                }
                lines.push(format!(
                    "children:    started={} queued={} terminal={}",
                    meta.started_child_count, meta.queued_child_count, meta.terminal_child_count
                ));
                if let Some(reason) = &meta.admission_wait_reason {
                    lines.push(format!("queue:       {reason}"));
                }
                if let Some(usage) = meta.actual_usage_total {
                    lines.push(format!("usage:       {usage} tokens"));
                }
                lines.push(format!(
                    "result:      {}",
                    if meta.has_final_result {
                        "present"
                    } else {
                        "none"
                    }
                ));
                lines.push(format!("artifacts:   {}", meta.artifact_count));
                if let Some(reason) = &meta.terminal_reason {
                    lines.push(format!("terminal:    {reason}"));
                }
                if let Some(log) = &meta.latest_log_summary {
                    lines.push(format!("log:         {log}"));
                }
                if let Some(report) = &meta.latest_report_summary {
                    lines.push(format!("report:      {report}"));
                }
            }
            lines
        }
    }
}

fn preview_lines(snapshot: &BackgroundTaskSnapshot) -> Vec<String> {
    if let Some(output) = &snapshot.output {
        return command_output_preview(output);
    }
    if let Some(answers) = &snapshot.answers {
        if answers.is_empty() {
            return vec!["No answers yet.".to_owned()];
        }
        return answers
            .iter()
            .enumerate()
            .map(|(index, answer)| format!("answer {}: {answer}", index + 1))
            .collect();
    }
    match snapshot.kind {
        BackgroundTaskKind::Bash => vec!["No output yet.".to_owned()],
        BackgroundTaskKind::Question => vec![snapshot.description.clone()],
        BackgroundTaskKind::Delegate => {
            if let Some(agent) = &snapshot.delegate {
                if let Some(text) = &agent.latest_text {
                    vec![format!("latest: {text}")]
                } else if let Some(outcome) = &agent.outcome {
                    vec![format!("result: {}", outcome.summary)]
                } else {
                    vec!["Agent running...".to_owned()]
                }
            } else {
                vec!["No agent data.".to_owned()]
            }
        }
        BackgroundTaskKind::DelegateSwarm => {
            if let Some(swarm) = &snapshot.swarm {
                let all_queued = swarm.children.iter().all(|c| {
                    matches!(
                        c.agent.state,
                        neo_agent_core::multi_agent::AgentLifecycleState::Queued
                    )
                });
                if all_queued {
                    vec!["Orchestrating...".to_owned()]
                } else {
                    let completed = swarm
                        .children
                        .iter()
                        .filter(|c| {
                            matches!(
                                c.agent.state,
                                neo_agent_core::multi_agent::AgentLifecycleState::Completed
                            )
                        })
                        .count();
                    vec![format!(
                        "Working... {}/{} children done",
                        completed,
                        swarm.children.len()
                    )]
                }
            } else {
                vec!["No swarm data.".to_owned()]
            }
        }
        BackgroundTaskKind::Workflow => {
            if let Some(meta) = &snapshot.workflow {
                let mut lines = Vec::new();
                if let Some(handle) = &meta.human_handle {
                    lines.push(format!("handle: {handle}"));
                }
                if let Some(phase) = &meta.current_phase {
                    lines.push(format!("phase: {phase}"));
                }
                lines.push(format!(
                    "children: {}/{}/{}",
                    meta.started_child_count, meta.queued_child_count, meta.terminal_child_count
                ));
                if let Some(reason) = &meta.admission_wait_reason {
                    lines.push(format!("queue: {reason}"));
                }
                if let Some(usage) = meta.actual_usage_total {
                    lines.push(format!("usage: {usage}"));
                }
                lines.push(format!("status: {}", snapshot.status.as_str()));
                lines
            } else {
                vec![format!("status: {}", snapshot.status.as_str())]
            }
        }
    }
}

fn command_output_preview(output: &CommandOutput) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(exit_code) = output.exit_code {
        lines.push(format!("exit_code: {exit_code}"));
    }
    append_stream_lines(
        &mut lines,
        "stdout",
        &output.stdout,
        output.stdout_truncated,
    );
    append_stream_lines(
        &mut lines,
        "stderr",
        &output.stderr,
        output.stderr_truncated,
    );
    if lines.is_empty() {
        lines.push("No output yet.".to_owned());
    }
    lines
}

fn append_stream_lines(lines: &mut Vec<String>, label: &str, stream: &str, truncated: bool) {
    if stream.is_empty() && !truncated {
        return;
    }
    lines.push(format!("{label}:"));
    lines.extend(stream.lines().map(ToOwned::to_owned));
    if truncated {
        lines.push(format!("[{label} truncated]"));
    }
}

fn map_status(status: BackgroundTaskStatus) -> TaskBrowserStatus {
    match status {
        BackgroundTaskStatus::Running => TaskBrowserStatus::Running,
        BackgroundTaskStatus::WaitingForUser => TaskBrowserStatus::Waiting,
        BackgroundTaskStatus::Paused => TaskBrowserStatus::Paused,
        BackgroundTaskStatus::Completed => TaskBrowserStatus::Completed,
        BackgroundTaskStatus::Failed => TaskBrowserStatus::Failed,
        BackgroundTaskStatus::Cancelled => TaskBrowserStatus::Cancelled,
        BackgroundTaskStatus::TimedOut => TaskBrowserStatus::TimedOut,
        BackgroundTaskStatus::ResourceLimited => TaskBrowserStatus::ResourceLimited,
        BackgroundTaskStatus::ParentExited => TaskBrowserStatus::ParentExited,
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn format_agent_activity(activity: &neo_agent_core::multi_agent::AgentActivityEntry) -> String {
    use neo_agent_core::multi_agent::{AgentActivityKind, AgentToolActivityPhase};
    match &activity.kind {
        AgentActivityKind::Tool {
            name,
            summary,
            phase,
            ..
        } => {
            let verb = match phase {
                AgentToolActivityPhase::Queued { .. } => "queued",
                AgentToolActivityPhase::Failed => "Failed",
                AgentToolActivityPhase::Done | AgentToolActivityPhase::Ongoing => "Used",
            };
            match summary {
                Some(summary) => format!("{verb} {name} ({summary})"),
                None => format!("{verb} {name}"),
            }
        }
        AgentActivityKind::Text { text, .. } => text.clone(),
        AgentActivityKind::Instruction { .. } => "instructions updated".to_owned(),
    }
}

#[cfg(test)]
#[path = "test_cases/task_browser.rs"]
mod tests;
