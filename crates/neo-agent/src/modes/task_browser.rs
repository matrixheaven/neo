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
    let description_label = match snapshot.kind {
        BackgroundTaskKind::Bash => "description",
        BackgroundTaskKind::Question => "prompt",
    };
    vec![
        format!("id:          {}", snapshot.task_id),
        format!("kind:        {}", snapshot.kind.as_str()),
        format!("status:      {}", status.label()),
        format!("elapsed:     {}", format_elapsed(snapshot.elapsed)),
        format!("{description_label}: {}", snapshot.description),
    ]
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
}
