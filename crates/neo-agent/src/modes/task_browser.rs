use std::time::Duration;

use neo_agent_core::tools::{
    BackgroundTaskKind, BackgroundTaskSnapshot, BackgroundTaskStatus, CommandOutput,
};
use neo_tui::tasks_browser::{
    TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot, TaskBrowserStatus,
};

#[must_use]
pub fn snapshots_to_browser_snapshot(snapshots: &[BackgroundTaskSnapshot]) -> TaskBrowserSnapshot {
    TaskBrowserSnapshot::new(snapshots.iter().map(snapshot_to_item).collect())
}

#[must_use]
pub fn snapshot_to_item(snapshot: &BackgroundTaskSnapshot) -> TaskBrowserItem {
    let kind = match snapshot.kind {
        BackgroundTaskKind::Bash => TaskBrowserKind::Bash,
        BackgroundTaskKind::Question => TaskBrowserKind::Question,
        BackgroundTaskKind::Delegate => TaskBrowserKind::Delegate,
        BackgroundTaskKind::DelegateSwarm => TaskBrowserKind::DelegateSwarm,
    };
    let status = map_status(snapshot.status);
    let title = snapshot.delegate.as_ref().map_or_else(
        || snapshot.description.clone(),
        |agent| agent.task_title.clone(),
    );
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
                        .map(|outcome| outcome.summary.as_str())
                        .unwrap_or(child.agent.task_title.as_str());
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
        BackgroundTaskStatus::Completed => TaskBrowserStatus::Completed,
        BackgroundTaskStatus::Failed => TaskBrowserStatus::Failed,
        BackgroundTaskStatus::Cancelled => TaskBrowserStatus::Cancelled,
        BackgroundTaskStatus::TimedOut => TaskBrowserStatus::TimedOut,
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn format_agent_activity(activity: &neo_agent_core::multi_agent::AgentActivityEntry) -> String {
    use neo_agent_core::multi_agent::AgentActivityKind;
    match &activity.kind {
        AgentActivityKind::Tool {
            name,
            summary,
            failed,
            ..
        } => {
            let verb = if *failed { "Failed" } else { "Used" };
            match summary {
                Some(summary) => format!("{verb} {name} ({summary})"),
                None => format!("{verb} {name}"),
            }
        }
        AgentActivityKind::Text { text, .. } => text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash_snapshot(status: BackgroundTaskStatus) -> BackgroundTaskSnapshot {
        BackgroundTaskSnapshot {
            task_id: "bash-1".to_owned(),
            kind: BackgroundTaskKind::Bash,
            status,
            description: "cargo test".to_owned(),
            elapsed: Duration::from_secs(65),
            output: Some(CommandOutput {
                exit_code: Some(0),
                stdout: "ok\nnext".to_owned(),
                stderr: "warn".to_owned(),
                stdout_truncated: true,
                stderr_truncated: false,
            }),
            answers: None,
            delegate: None,
            swarm: None,
        }
    }

    #[test]
    fn task_browser_adapter_maps_bash_snapshot() {
        let item = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Running));

        assert_eq!(item.id, "bash-1");
        assert_eq!(item.kind, TaskBrowserKind::Bash);
        assert_eq!(item.status, TaskBrowserStatus::Running);
        assert_eq!(item.title, "cargo test");
        assert_eq!(item.elapsed, "01:05");
        assert!(item.can_stop);
        assert!(item.detail_lines.iter().any(|line| line.contains("bash-1")));
        assert!(item.preview_lines.iter().any(|line| line == "stdout:"));
        assert!(item.preview_lines.iter().any(|line| line == "ok"));
        assert!(
            item.preview_lines
                .iter()
                .any(|line| line == "[stdout truncated]")
        );
    }

    #[test]
    fn task_browser_adapter_maps_terminal_statuses() {
        let completed = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Completed));
        let failed = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Failed));
        let cancelled = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Cancelled));
        let timed_out = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::TimedOut));

        assert_eq!(completed.status, TaskBrowserStatus::Completed);
        assert_eq!(failed.status, TaskBrowserStatus::Failed);
        assert_eq!(cancelled.status, TaskBrowserStatus::Cancelled);
        assert_eq!(timed_out.status, TaskBrowserStatus::TimedOut);
        assert!(!completed.can_stop);
        assert!(failed.status.is_interrupted());
        assert!(cancelled.status.is_interrupted());
        assert!(timed_out.status.is_interrupted());
    }

    #[test]
    fn task_browser_adapter_maps_question_snapshot() {
        let snapshot = BackgroundTaskSnapshot {
            task_id: "question-1".to_owned(),
            kind: BackgroundTaskKind::Question,
            status: BackgroundTaskStatus::WaitingForUser,
            description: "Pick one".to_owned(),
            elapsed: Duration::from_secs(2),
            output: None,
            answers: Some(vec!["yes".to_owned()]),
            delegate: None,
            swarm: None,
        };

        let item = snapshot_to_item(&snapshot);

        assert_eq!(item.kind, TaskBrowserKind::Question);
        assert_eq!(item.status, TaskBrowserStatus::Waiting);
        assert!(item.can_stop);
        assert_eq!(item.preview_lines, vec!["answer 1: yes".to_owned()]);
    }

    #[test]
    fn task_browser_adapter_shows_waiting_question_prompt() {
        let snapshot = BackgroundTaskSnapshot {
            task_id: "question-1".to_owned(),
            kind: BackgroundTaskKind::Question,
            status: BackgroundTaskStatus::WaitingForUser,
            description: "Pick one".to_owned(),
            elapsed: Duration::from_secs(2),
            output: None,
            answers: None,
            delegate: None,
            swarm: None,
        };

        let item = snapshot_to_item(&snapshot);

        assert!(
            item.detail_lines
                .iter()
                .any(|line| line == "prompt: Pick one")
        );
        assert_eq!(item.preview_lines, vec!["Pick one".to_owned()]);
    }

    #[test]
    fn task_browser_adapter_builds_snapshot_collection() {
        let snapshot = bash_snapshot(BackgroundTaskStatus::Running);
        let browser_snapshot = snapshots_to_browser_snapshot(&[snapshot]);

        assert_eq!(browser_snapshot.items().len(), 1);
        assert_eq!(browser_snapshot.items()[0].id, "bash-1");
    }

    #[test]
    fn task_browser_adapter_maps_delegate_snapshot() {
        use neo_agent_core::multi_agent::{
            AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
            AgentSnapshot,
        };
        let name = AgentDisplayName::new("Gibbs");
        let agent = AgentSnapshot {
            id: AgentId::from_suffix_for_test("del-1"),
            display_name: name.clone(),
            path: AgentPath::root_child(&name),
            role: AgentRole::Coder,
            mode: AgentRunMode::Background,
            state: AgentLifecycleState::Running,
            task: "fix the border".to_owned(),
            task_title: "fix the border".to_owned(),
            tool_count: 2,
            token_count: 1000,
            elapsed: Duration::from_secs(10),
            latest_text: Some("reading file...".to_owned()),
            activity: Vec::new(),
            outcome: None,
        };
        let snapshot = BackgroundTaskSnapshot {
            task_id: agent.id.as_str().to_owned(),
            kind: BackgroundTaskKind::Delegate,
            status: BackgroundTaskStatus::Running,
            description: agent.task.clone(),
            elapsed: Duration::from_secs(10),
            output: None,
            answers: None,
            delegate: Some(agent),
            swarm: None,
        };
        let item = snapshot_to_item(&snapshot);
        assert_eq!(item.kind, TaskBrowserKind::Delegate);
        assert!(item.detail_lines.iter().any(|l| l.contains("name:")));
        assert!(item.preview_lines.iter().any(|l| l.contains("latest")));
    }

    #[test]
    fn task_browser_adapter_maps_swarm_snapshot() {
        use neo_agent_core::multi_agent::{
            AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
            AgentSnapshot, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
        };
        let name = AgentDisplayName::new("Zeno");
        let agent = AgentSnapshot {
            id: AgentId::from_suffix_for_test("sw-0"),
            display_name: name.clone(),
            path: AgentPath::root_child(&name),
            role: AgentRole::Coder,
            mode: AgentRunMode::Background,
            state: AgentLifecycleState::Running,
            task: "item 0".to_owned(),
            task_title: "item 0".to_owned(),
            tool_count: 0,
            token_count: 0,
            elapsed: Duration::from_secs(5),
            latest_text: None,
            activity: Vec::new(),
            outcome: None,
        };
        let children = vec![SwarmChildSnapshot {
            item_index: 0,
            item: "check grep".to_owned(),
            agent,
        }];
        let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
        let swarm = SwarmSnapshot {
            swarm_id: "swarm-1".to_owned(),
            description: "audit schemas".to_owned(),
            role: AgentRole::Coder,
            mode: AgentRunMode::Background,
            state: AgentLifecycleState::Running,
            max_concurrency: 1,
            aggregate,
            children,
        };
        let snapshot = BackgroundTaskSnapshot {
            task_id: swarm.swarm_id.clone(),
            kind: BackgroundTaskKind::DelegateSwarm,
            status: BackgroundTaskStatus::Running,
            description: swarm.description.clone(),
            elapsed: Duration::from_secs(5),
            output: None,
            answers: None,
            delegate: None,
            swarm: Some(swarm),
        };
        let item = snapshot_to_item(&snapshot);
        assert_eq!(item.kind, TaskBrowserKind::DelegateSwarm);
        assert!(item.detail_lines.iter().any(|l| l.contains("children:")));
    }

    fn delegate_swarm_snapshot_with_completed_children() -> BackgroundTaskSnapshot {
        use neo_agent_core::multi_agent::{
            AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
            AgentSnapshot, AgentTerminalOutcome, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
        };
        let name_a = AgentDisplayName::new("Alpha");
        let name_b = AgentDisplayName::new("Beta");
        let child_a = AgentSnapshot {
            id: AgentId::from_suffix_for_test("sw-comp-a"),
            display_name: name_a.clone(),
            path: AgentPath::swarm_child("swarm_comp", &name_a),
            role: AgentRole::Coder,
            mode: AgentRunMode::Background,
            state: AgentLifecycleState::Completed,
            task: "child A prompt".to_owned(),
            task_title: "Child A".to_owned(),
            tool_count: 2,
            token_count: 500,
            elapsed: Duration::from_secs(10),
            latest_text: None,
            activity: Vec::new(),
            outcome: Some(AgentTerminalOutcome {
                summary: "All good".to_owned(),
                is_error: false,
            }),
        };
        let child_b = AgentSnapshot {
            id: AgentId::from_suffix_for_test("sw-comp-b"),
            display_name: name_b.clone(),
            path: AgentPath::swarm_child("swarm_comp", &name_b),
            role: AgentRole::Coder,
            mode: AgentRunMode::Background,
            state: AgentLifecycleState::Completed,
            task: "child B prompt".to_owned(),
            task_title: "Child B".to_owned(),
            tool_count: 1,
            token_count: 300,
            elapsed: Duration::from_secs(8),
            latest_text: None,
            activity: Vec::new(),
            outcome: Some(AgentTerminalOutcome {
                summary: "Done too".to_owned(),
                is_error: false,
            }),
        };
        let children = vec![
            SwarmChildSnapshot {
                item_index: 0,
                item: "item-a".to_owned(),
                agent: child_a,
            },
            SwarmChildSnapshot {
                item_index: 1,
                item: "item-b".to_owned(),
                agent: child_b,
            },
        ];
        let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
        let swarm = SwarmSnapshot {
            swarm_id: "swarm_comp".to_owned(),
            description: "completed swarm".to_owned(),
            role: AgentRole::Coder,
            mode: AgentRunMode::Background,
            state: AgentLifecycleState::Completed,
            max_concurrency: 2,
            aggregate,
            children,
        };
        BackgroundTaskSnapshot {
            task_id: swarm.swarm_id.clone(),
            kind: BackgroundTaskKind::DelegateSwarm,
            status: BackgroundTaskStatus::Completed,
            description: swarm.description.clone(),
            elapsed: Duration::from_secs(20),
            output: None,
            answers: None,
            delegate: None,
            swarm: Some(swarm),
        }
    }

    #[test]
    fn task_browser_uses_cancelled_vocabulary_for_interrupted_tasks() {
        let cancelled = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Cancelled));

        assert_eq!(cancelled.status, TaskBrowserStatus::Cancelled);
        assert_eq!(cancelled.status.label(), "cancelled");
        assert!(cancelled.status.is_interrupted());
    }

    #[test]
    fn task_browser_swarm_details_include_aggregate_and_child_results() {
        let item = snapshot_to_item(&delegate_swarm_snapshot_with_completed_children());
        let details = item.detail_lines.join("\n");

        assert!(details.contains("aggregate:"), "{details}");
        assert!(details.contains("completed"), "{details}");
        assert!(details.contains("agent_"), "{details}");
    }
}
