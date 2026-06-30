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
    TaskBrowserItem {
        id: snapshot.task_id.clone(),
        kind,
        status,
        title: snapshot.description.clone(),
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
                lines.push(format!("task:        {}", agent.task));
                if let Some(text) = &agent.latest_text {
                    lines.push(format!("latest:      {text}"));
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
                lines.push(format!(
                    "progress:    {}/{}",
                    completed,
                    swarm.children.len()
                ));
                lines.push(format!("children:    {}", swarm.children.len()));
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
        BackgroundTaskStatus::Stopped => TaskBrowserStatus::Stopped,
        BackgroundTaskStatus::TimedOut => TaskBrowserStatus::TimedOut,
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes:02}:{seconds:02}")
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
        let stopped = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Stopped));
        let timed_out = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::TimedOut));

        assert_eq!(completed.status, TaskBrowserStatus::Completed);
        assert_eq!(failed.status, TaskBrowserStatus::Failed);
        assert_eq!(stopped.status, TaskBrowserStatus::Stopped);
        assert_eq!(timed_out.status, TaskBrowserStatus::TimedOut);
        assert!(!completed.can_stop);
        assert!(failed.status.is_interrupted());
        assert!(stopped.status.is_interrupted());
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
            AgentSnapshot, SwarmChildSnapshot, SwarmSnapshot,
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
            tool_count: 0,
            token_count: 0,
            elapsed: Duration::from_secs(5),
            latest_text: None,
            activity: Vec::new(),
            outcome: None,
        };
        let swarm = SwarmSnapshot {
            swarm_id: "swarm-1".to_owned(),
            description: "audit schemas".to_owned(),
            mode: AgentRunMode::Background,
            max_concurrency: 1,
            children: vec![SwarmChildSnapshot {
                item_index: 0,
                item: "check grep".to_owned(),
                agent,
            }],
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
}
