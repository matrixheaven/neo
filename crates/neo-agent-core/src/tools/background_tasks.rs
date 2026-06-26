use std::{
    collections::HashMap,
    fmt::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::{sync::Mutex, task::JoinHandle};
use uuid::Uuid;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};
use crate::QuestionEventData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskKind {
    Bash,
    Question,
}

impl BackgroundTaskKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Question => "question",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskStatus {
    Running,
    WaitingForUser,
    Completed,
    Failed,
    Stopped,
    TimedOut,
}

impl BackgroundTaskStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::WaitingForUser => "waiting_for_user",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::TimedOut => "timed_out",
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::WaitingForUser)
    }
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub struct ManagedBackgroundCommand {
    pub stdout: Arc<Mutex<Vec<u8>>>,
    pub stderr: Arc<Mutex<Vec<u8>>>,
    pub stdout_task: JoinHandle<()>,
    pub stderr_task: JoinHandle<()>,
    pub try_wait: Arc<dyn Fn() -> BoxFuture<'static, std::io::Result<Option<i32>>> + Send + Sync>,
    pub cleanup: Arc<dyn Fn() -> BoxFuture<'static, Option<i32>> + Send + Sync>,
    pub drain: Arc<dyn Fn(JoinHandle<()>) -> BoxFuture<'static, ()> + Send + Sync>,
}

#[derive(Clone)]
pub struct BackgroundTaskSnapshot {
    pub task_id: String,
    pub kind: BackgroundTaskKind,
    pub status: BackgroundTaskStatus,
    pub description: String,
    pub elapsed: Duration,
    pub output: Option<CommandOutput>,
    pub answers: Option<Vec<String>>,
}

enum BackgroundTaskState {
    BashRunning(ManagedBackgroundCommand),
    BashFinished {
        status: BackgroundTaskStatus,
        output: CommandOutput,
    },
    QuestionWaiting,
    QuestionFinished {
        status: BackgroundTaskStatus,
        answers: Option<Vec<String>>,
    },
}

struct BackgroundTaskRecord {
    description: String,
    started_at: Instant,
    state: BackgroundTaskState,
}

#[derive(Clone, Default)]
pub struct BackgroundTaskManager {
    inner: Arc<Mutex<HashMap<String, BackgroundTaskRecord>>>,
}

impl std::fmt::Debug for BackgroundTaskManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackgroundTaskManager")
            .finish_non_exhaustive()
    }
}

impl BackgroundTaskManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start_bash(
        &self,
        description: String,
        command: ManagedBackgroundCommand,
        max_output_bytes: usize,
    ) -> Result<ToolResult, ToolError> {
        let task_id = format!("bash-{}", Uuid::new_v4());
        let description_trimmed = description.trim().to_owned();
        self.inner.lock().await.insert(
            task_id.clone(),
            BackgroundTaskRecord {
                description,
                started_at: Instant::now(),
                state: BackgroundTaskState::BashRunning(command),
            },
        );
        Ok(ToolResult::ok(format!(
            "task_id: {task_id}\n\
             kind: bash\n\
             status: running\n\
             description: {description_trimmed}\n\
             automatic_notification: true\n\
             next_step: You will be automatically notified when it completes.\n\
             next_step: Use TaskOutput with this task_id for a non-blocking status/output snapshot.\n\
             next_step: Use TaskStop only if the task must be cancelled."
        ))
        .with_details(json!({
            "task_id": task_id,
            "kind": "bash",
            "status": "running",
            "description": description_trimmed,
            "automatic_notification": true,
            "next_steps": [
                "You will be automatically notified when it completes.",
                "Use TaskOutput with this task_id for a non-blocking status/output snapshot.",
                "Use TaskStop only if the task must be cancelled."
            ],
            "stdout": "",
            "stderr": "",
            "stdout_truncated": false,
            "stderr_truncated": false,
            "truncated": false,
            "max_output_bytes": max_output_bytes,
        })))
    }

    pub async fn start_question(&self, task_id: String, description: String) -> ToolResult {
        self.inner.lock().await.insert(
            task_id.clone(),
            BackgroundTaskRecord {
                description: description.clone(),
                started_at: Instant::now(),
                state: BackgroundTaskState::QuestionWaiting,
            },
        );
        ToolResult::ok(format!(
            "task_id: {task_id}\nkind: question\nstatus: waiting_for_user\ndescription: {description}\nautomatic_notification: true\nnext_step: Continue your current work; the answer will arrive automatically when the user responds.\nnext_step: Use TaskOutput with this task_id to check the current state.\nnext_step: Use TaskStop with this task_id to cancel the pending question."
        ))
        .with_details(json!({
            "task_id": task_id,
            "kind": "question",
            "status": "waiting_for_user",
            "description": description,
            "automatic_notification": true,
            "next_steps": [
                "Continue your current work; the answer will arrive automatically when the user responds.",
                "Use TaskOutput with this task_id to check the current state.",
                "Use TaskStop with this task_id to cancel the pending question."
            ],
        }))
    }

    pub async fn complete_question(&self, task_id: &str, answers: Vec<String>) {
        let mut tasks = self.inner.lock().await;
        if let Some(record) = tasks.get_mut(task_id)
            && matches!(record.state, BackgroundTaskState::QuestionWaiting)
        {
            record.state = BackgroundTaskState::QuestionFinished {
                status: BackgroundTaskStatus::Completed,
                answers: Some(answers),
            };
        }
    }

    pub async fn list(&self, active_only: bool, limit: usize) -> Vec<BackgroundTaskSnapshot> {
        let task_ids = self.inner.lock().await.keys().cloned().collect::<Vec<_>>();
        let mut snapshots = Vec::new();
        for task_id in task_ids {
            if let Ok(snapshot) = self.snapshot(&task_id).await
                && (!active_only || snapshot.status.is_active())
            {
                snapshots.push(snapshot);
            }
        }
        snapshots.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        snapshots.truncate(limit);
        snapshots
    }

    pub async fn output(
        &self,
        task_id: &str,
        block: bool,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<ToolResult, ToolError> {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = self.snapshot(task_id).await?;
            if !block || !snapshot.status.is_active() || Instant::now() >= deadline {
                return Ok(snapshot_result(&snapshot, max_output_bytes));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[allow(clippy::too_many_lines)]
    pub async fn stop(
        &self,
        task_id: &str,
        reason: &str,
        max_output_bytes: usize,
    ) -> Result<ToolResult, ToolError> {
        enum StopAction {
            Already(BackgroundTaskSnapshot),
            StopQuestion {
                started_at: Instant,
                description: String,
            },
            StopBash {
                started_at: Instant,
                description: String,
                command: ManagedBackgroundCommand,
            },
        }

        let action = {
            let mut tasks = self.inner.lock().await;
            let Some(record) = tasks.get(task_id) else {
                return Err(ToolError::InvalidInput {
                    tool: "TaskStop".to_owned(),
                    message: format!("unknown background task `{task_id}`"),
                });
            };
            match &record.state {
                BackgroundTaskState::BashFinished { status, output } => {
                    StopAction::Already(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Bash,
                        status: *status,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: Some(output.clone()),
                        answers: None,
                    })
                }
                BackgroundTaskState::QuestionFinished { status, answers } => {
                    StopAction::Already(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Question,
                        status: *status,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: answers.clone(),
                    })
                }
                BackgroundTaskState::QuestionWaiting => {
                    let record = tasks.get_mut(task_id).expect("record still exists");
                    record.state = BackgroundTaskState::QuestionFinished {
                        status: BackgroundTaskStatus::Stopped,
                        answers: None,
                    };
                    StopAction::StopQuestion {
                        started_at: record.started_at,
                        description: record.description.clone(),
                    }
                }
                BackgroundTaskState::BashRunning(_) => {
                    let record = tasks.remove(task_id).expect("record still exists");
                    let BackgroundTaskState::BashRunning(command) = record.state else {
                        unreachable!();
                    };
                    StopAction::StopBash {
                        started_at: record.started_at,
                        description: record.description,
                        command,
                    }
                }
            }
        };

        match action {
            StopAction::Already(snapshot) => Ok(snapshot_result(&snapshot, max_output_bytes)),
            StopAction::StopQuestion {
                started_at,
                description,
            } => {
                let snapshot = BackgroundTaskSnapshot {
                    task_id: task_id.to_owned(),
                    kind: BackgroundTaskKind::Question,
                    status: BackgroundTaskStatus::Stopped,
                    description,
                    elapsed: started_at.elapsed(),
                    output: None,
                    answers: None,
                };
                let mut result = snapshot_result(&snapshot, max_output_bytes);
                result.details = Some(json!({
                    "task_id": task_id,
                    "kind": "question",
                    "status": "stopped",
                    "description": snapshot.description,
                    "elapsed_ms": snapshot.elapsed.as_millis(),
                    "reason": reason,
                }));
                Ok(result)
            }
            StopAction::StopBash {
                started_at,
                description,
                command,
            } => {
                let exit_code = (command.cleanup)().await;
                (command.drain)(command.stdout_task).await;
                (command.drain)(command.stderr_task).await;
                let output = output_from_buffers(exit_code, command.stdout, command.stderr).await;
                let snapshot = BackgroundTaskSnapshot {
                    task_id: task_id.to_owned(),
                    kind: BackgroundTaskKind::Bash,
                    status: BackgroundTaskStatus::Stopped,
                    description: description.clone(),
                    elapsed: started_at.elapsed(),
                    output: Some(output.clone()),
                    answers: None,
                };
                self.inner.lock().await.insert(
                    task_id.to_owned(),
                    BackgroundTaskRecord {
                        description,
                        started_at,
                        state: BackgroundTaskState::BashFinished {
                            status: BackgroundTaskStatus::Stopped,
                            output,
                        },
                    },
                );
                Ok(snapshot_result(&snapshot, max_output_bytes))
            }
        }
    }

    async fn snapshot(&self, task_id: &str) -> Result<BackgroundTaskSnapshot, ToolError> {
        self.snapshot_inner(task_id)
            .await
            .ok_or_else(|| ToolError::InvalidInput {
                tool: "TaskOutput".to_owned(),
                message: format!("unknown background task `{task_id}`"),
            })
    }

    #[allow(clippy::too_many_lines)]
    async fn snapshot_inner(&self, task_id: &str) -> Option<BackgroundTaskSnapshot> {
        enum SnapshotAction {
            Ready(BackgroundTaskSnapshot),
            Running {
                started_at: Instant,
                description: String,
                stdout: Arc<Mutex<Vec<u8>>>,
                stderr: Arc<Mutex<Vec<u8>>>,
            },
            Finish {
                started_at: Instant,
                description: String,
                command: ManagedBackgroundCommand,
                exit_code: Option<i32>,
            },
        }

        let action = {
            let mut tasks = self.inner.lock().await;
            let record = tasks.get_mut(task_id)?;
            match &mut record.state {
                BackgroundTaskState::BashFinished { status, output } => {
                    SnapshotAction::Ready(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Bash,
                        status: *status,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: Some(output.clone()),
                        answers: None,
                    })
                }
                BackgroundTaskState::QuestionWaiting => {
                    SnapshotAction::Ready(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Question,
                        status: BackgroundTaskStatus::WaitingForUser,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: None,
                    })
                }
                BackgroundTaskState::QuestionFinished { status, answers } => {
                    SnapshotAction::Ready(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Question,
                        status: *status,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: answers.clone(),
                    })
                }
                BackgroundTaskState::BashRunning(command) => match (command.try_wait)().await {
                    Ok(Some(status)) => {
                        let record = tasks.remove(task_id).expect("record still exists");
                        let BackgroundTaskState::BashRunning(command) = record.state else {
                            unreachable!();
                        };
                        SnapshotAction::Finish {
                            started_at: record.started_at,
                            description: record.description,
                            command,
                            exit_code: Some(status),
                        }
                    }
                    Ok(None) | Err(_) => SnapshotAction::Running {
                        started_at: record.started_at,
                        description: record.description.clone(),
                        stdout: command.stdout.clone(),
                        stderr: command.stderr.clone(),
                    },
                },
            }
        };

        match action {
            SnapshotAction::Ready(snapshot) => Some(snapshot),
            SnapshotAction::Running {
                started_at,
                description,
                stdout,
                stderr,
            } => {
                let stdout = stdout.lock().await;
                let stderr = stderr.lock().await;
                Some(BackgroundTaskSnapshot {
                    task_id: task_id.to_owned(),
                    kind: BackgroundTaskKind::Bash,
                    status: BackgroundTaskStatus::Running,
                    description,
                    elapsed: started_at.elapsed(),
                    output: Some(output_from_locked_buffers(None, &stdout, &stderr)),
                    answers: None,
                })
            }
            SnapshotAction::Finish {
                started_at,
                description,
                command,
                exit_code,
            } => {
                (command.drain)(command.stdout_task).await;
                (command.drain)(command.stderr_task).await;
                let output = output_from_buffers(exit_code, command.stdout, command.stderr).await;
                let status = if output.exit_code == Some(0) {
                    BackgroundTaskStatus::Completed
                } else {
                    BackgroundTaskStatus::Failed
                };
                let snapshot = BackgroundTaskSnapshot {
                    task_id: task_id.to_owned(),
                    kind: BackgroundTaskKind::Bash,
                    status,
                    description: description.clone(),
                    elapsed: started_at.elapsed(),
                    output: Some(output.clone()),
                    answers: None,
                };
                self.inner.lock().await.insert(
                    task_id.to_owned(),
                    BackgroundTaskRecord {
                        description,
                        started_at,
                        state: BackgroundTaskState::BashFinished { status, output },
                    },
                );
                Some(snapshot)
            }
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskListInput {
    /// Whether to list only non-terminal background tasks.
    #[schemars(
        description = "Whether to list only non-terminal background tasks. Defaults to true."
    )]
    active_only: Option<bool>,
    /// Maximum number of tasks to return (1-100).
    #[schemars(
        description = "Maximum number of tasks to return. Accepts a value between 1 and 100 and defaults to 20 when omitted."
    )]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskOutputInput {
    /// The background task ID to inspect.
    #[schemars(description = "The background task ID to inspect.")]
    task_id: String,
    /// Whether to wait for the task to finish before returning.
    #[schemars(
        description = "Whether to wait for the task to finish before returning. Defaults to false."
    )]
    block: Option<bool>,
    /// Maximum number of seconds to wait when block=true.
    #[schemars(description = "Maximum number of seconds to wait when block=true. Defaults to 30.")]
    timeout: Option<u64>,
    /// Maximum bytes of output to include in the preview.
    #[schemars(
        description = "Maximum bytes of output to include in the preview. Defaults to the runtime limit when omitted."
    )]
    max_output_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TaskStopInput {
    /// The background task ID to stop.
    #[schemars(description = "The background task ID to stop.")]
    task_id: String,
    /// Short reason recorded when the task is stopped.
    #[schemars(
        description = "Short reason recorded when the task is stopped. Defaults to 'Stopped by TaskStop'."
    )]
    reason: Option<String>,
    /// Maximum bytes of output to include in the result.
    #[schemars(
        description = "Maximum bytes of output to include in the result. Defaults to the runtime limit when omitted."
    )]
    max_output_bytes: Option<usize>,
}

pub struct TaskListTool;

impl Tool for TaskListTool {
    fn name(&self) -> &'static str {
        "TaskList"
    }

    fn description(&self) -> &'static str {
        "List background tasks and their current status.\n\n\
         Use this tool to discover which background tasks exist and where each one stands. It is the entry point for inspecting background work: it returns a task ID, status, kind, description, and elapsed time for every task it reports.\n\n\
         Guidelines:\n\
         - After a context compaction, or whenever you are unsure which background tasks are running or what their task IDs are, call this tool to re-enumerate them instead of guessing a task ID.\n\
         - Prefer the default `active_only=true`, which lists only non-terminal tasks. Pass `active_only=false` only when you specifically need to see tasks that have already finished.\n\
         - `limit` caps how many tasks are returned. It accepts a value between 1 and 100 and defaults to 20 when omitted.\n\
         - This tool only lists tasks; it does not return their output. Use it first to locate the task ID you need, then call `TaskOutput` with that ID to read the task's output and details.\n\
         - This tool is read-only and does not change any state, so it is always safe to call, including in plan mode.\n\n\
         Return format:\n\
         Returns a list of background tasks. Each entry includes:\n\
         - task_id: Unique identifier for the task (use this with TaskOutput/TaskStop).\n\
         - status: \"running\", \"completed\", \"failed\", or \"stopped\".\n\
         - kind: The type of background task (e.g. \"bash\", \"question\").\n\
         - description: Short human-readable description provided at creation time.\n\
         - elapsed: Time since the task was started (e.g. \"2m 30s\")."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TaskListInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: TaskListInput = parse_input(self.name(), input)?;
            let active_only = input.active_only.unwrap_or(true);
            let limit = input.limit.unwrap_or(20).clamp(1, 100);
            let tasks = ctx.background_tasks.list(active_only, limit).await;
            Ok(task_list_result(&tasks, active_only))
        })
    }
}

pub struct TaskOutputTool;

impl Tool for TaskOutputTool {
    fn name(&self) -> &'static str {
        "TaskOutput"
    }

    fn description(&self) -> &'static str {
        "Retrieve output from a running or completed background task.\n\n\
         Use this after `Bash` with background mode or `AskUserQuestion` with `background=true` when you need to inspect progress or explicitly wait for completion.\n\n\
         Guidelines:\n\
         - Prefer relying on automatic completion notifications. Use this tool only when you need task output before the automatic notification arrives.\n\
         - By default this tool is non-blocking and returns a current status/output snapshot.\n\
         - Use `block=true` only when you intentionally want to wait for completion or timeout.\n\
         - This tool returns structured task metadata and an output preview.\n\
         - For a terminal task, check `status` and `exit_code` to understand why it ended.\n\
         - This tool works with the generic background task system and should remain the primary read path for future task types.\n\n\
         Return fields:\n\
         - status: One of \"running\" (the task is still executing), \"completed\" (the task \
         finished successfully), \"failed\" (the task exited with a non-zero exit code), or \
         \"stopped\" (the task was cancelled via TaskStop).\n\
         - exit_code: The process exit code for terminal tasks. 0 means success; non-zero means \
         failure. Only present when status is \"completed\", \"failed\", or \"stopped\".\n\
         - output: A preview of the task's stdout/stderr, capped at max_output_bytes."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TaskOutputInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: TaskOutputInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            ctx.background_tasks
                .output(
                    &input.task_id,
                    input.block.unwrap_or(false),
                    Duration::from_secs(input.timeout.unwrap_or(30)),
                    max_output_bytes,
                )
                .await
        })
    }
}

pub struct TaskStopTool;

impl Tool for TaskStopTool {
    fn name(&self) -> &'static str {
        "TaskStop"
    }

    fn description(&self) -> &'static str {
        "Stop a running background task.\n\n\
         Only use this when a task must genuinely be cancelled — for a task that is finishing normally, wait for its completion notification or inspect it with `TaskOutput` instead of stopping it.\n\n\
         Guidelines:\n\
         - This is a general-purpose stop capability for any background task. It is not a bash-specific kill.\n\
         - Stopping a task is destructive: it may leave partial side effects behind. Use it with care.\n\
         - If the task has already finished, this tool simply returns its current status.\n\
         - Provide a short `reason` so the task history records why it was stopped.\n\n\
         Return format:\n\
         Returns the task's final status after the stop attempt. If the task was still running, it \
         is stopped and the output collected so far is included. If the task had already finished, \
         the current status and output are returned without any action taken."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TaskStopInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_shell_allowed()?;
            let input: TaskStopInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            ctx.background_tasks
                .stop(
                    &input.task_id,
                    input.reason.as_deref().unwrap_or("Stopped by TaskStop"),
                    max_output_bytes,
                )
                .await
        })
    }
}

pub fn task_list_result(tasks: &[BackgroundTaskSnapshot], active_only: bool) -> ToolResult {
    let header = if active_only {
        format!("active_background_tasks: {}", tasks.len())
    } else {
        format!("background_tasks: {}", tasks.len())
    };
    let mut content = header;
    if tasks.is_empty() {
        content.push_str("\nNo background tasks found.");
    } else {
        for task in tasks {
            content.push_str("\n\n");
            content.push_str(&format_snapshot_header(task));
        }
    }
    let count_key = if active_only {
        "active_background_tasks"
    } else {
        "background_tasks"
    };
    ToolResult::ok(content).with_details(json!({
        count_key: tasks.len(),
        "tasks": tasks.iter().map(snapshot_details).collect::<Vec<_>>(),
    }))
}

pub fn snapshot_result(snapshot: &BackgroundTaskSnapshot, max_output_bytes: usize) -> ToolResult {
    let mut content = format_snapshot_header(snapshot);
    let mut details = snapshot_details(snapshot);
    if let Some(output) = &snapshot.output {
        let (stdout_capped, stdout_truncated) = cap_plain_output(&output.stdout, max_output_bytes);
        let (stderr_capped, stderr_truncated) = cap_plain_output(&output.stderr, max_output_bytes);
        let truncated = stdout_truncated || stderr_truncated;
        if !stdout_capped.is_empty() || !stderr_capped.is_empty() {
            content.push_str("\n\n[output]\n");
            content.push_str(&stdout_capped);
            content.push_str(&stderr_capped);
        }
        if output.exit_code != Some(0) && !matches!(snapshot.status, BackgroundTaskStatus::Running)
        {
            let exit_label = output
                .exit_code
                .map_or_else(|| "signal".to_owned(), |code| code.to_string());
            if !content.ends_with('\n') && !content.is_empty() {
                content.push('\n');
            }
            let _ = write!(content, "Command failed with exit code: {exit_label}.");
        }
        if truncated {
            if !content.ends_with('\n') && !content.is_empty() {
                content.push('\n');
            }
            content.push_str("[output truncated]");
        }
        details["exit_code"] = json!(output.exit_code);
        details["stdout"] = json!(cap_output_details(&output.stdout, max_output_bytes));
        details["stderr"] = json!(cap_output_details(&output.stderr, max_output_bytes));
        details["stdout_truncated"] = json!(stdout_truncated);
        details["stderr_truncated"] = json!(stderr_truncated);
        details["truncated"] = json!(truncated);
    }
    if let Some(answers) = &snapshot.answers {
        details["answers"] = json!(answers);
    }
    let ok = !matches!(
        snapshot.status,
        BackgroundTaskStatus::Failed | BackgroundTaskStatus::TimedOut
    );
    let result = if ok {
        ToolResult::ok(content)
    } else {
        ToolResult::error(content)
    };
    result.with_details(details)
}

fn snapshot_details(snapshot: &BackgroundTaskSnapshot) -> serde_json::Value {
    json!({
        "task_id": snapshot.task_id,
        "kind": snapshot.kind.as_str(),
        "status": snapshot.status.as_str(),
        "description": snapshot.description,
        "elapsed_ms": snapshot.elapsed.as_millis(),
    })
}

fn format_snapshot_header(snapshot: &BackgroundTaskSnapshot) -> String {
    format!(
        "task_id: {}\nkind: {}\nstatus: {}\ndescription: {}\nelapsed: {}",
        snapshot.task_id,
        snapshot.kind.as_str(),
        snapshot.status.as_str(),
        snapshot.description,
        format_elapsed(snapshot.elapsed)
    )
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

pub async fn output_from_buffers(
    exit_code: Option<i32>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
) -> CommandOutput {
    let stdout = stdout.lock().await;
    let stderr = stderr.lock().await;
    output_from_locked_buffers(exit_code, &stdout, &stderr)
}

fn output_from_locked_buffers(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> CommandOutput {
    CommandOutput {
        exit_code,
        stdout: String::from_utf8_lossy(stdout).into_owned(),
        stderr: String::from_utf8_lossy(stderr).into_owned(),
    }
}

#[must_use]
pub fn cap_plain_output(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content.to_owned(), false);
    }
    let mut capped = String::new();
    for character in content.chars() {
        let next_len = capped.len() + character.len_utf8();
        if next_len > max_bytes {
            break;
        }
        capped.push(character);
    }
    (capped, true)
}

#[must_use]
pub fn cap_output_details(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_owned();
    }
    let mut capped = String::new();
    for character in content.chars() {
        let next_len = capped.len() + character.len_utf8();
        if next_len > max_bytes {
            break;
        }
        capped.push(character);
    }
    capped
}

#[must_use]
pub fn format_collected_answers(questions: &[QuestionEventData], answers: &[String]) -> String {
    let mut out = String::from("Collected your answers");
    for (question, answer) in questions.iter().zip(answers) {
        out.push_str("\nQ  ");
        out.push_str(&question.question);
        out.push_str("\n-> ");
        out.push_str(answer);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_command(
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
    ) -> ManagedBackgroundCommand {
        let wait_exit_code = exit_code;
        let cleanup_exit_code = exit_code;
        ManagedBackgroundCommand {
            stdout: Arc::new(Mutex::new(stdout.as_bytes().to_vec())),
            stderr: Arc::new(Mutex::new(stderr.as_bytes().to_vec())),
            stdout_task: tokio::spawn(async {}),
            stderr_task: tokio::spawn(async {}),
            try_wait: Arc::new(move || {
                let exit_code = wait_exit_code;
                Box::pin(async move { Ok(exit_code) })
            }),
            cleanup: Arc::new(move || {
                let exit_code = cleanup_exit_code;
                Box::pin(async move { exit_code })
            }),
            drain: Arc::new(|handle| {
                Box::pin(async move {
                    let _ = handle.await;
                })
            }),
        }
    }

    fn result_task_id(result: &ToolResult) -> String {
        result
            .details
            .as_ref()
            .and_then(|details| details["task_id"].as_str())
            .expect("task id detail")
            .to_owned()
    }

    #[tokio::test]
    async fn manager_lists_active_and_completed_questions() {
        let manager = BackgroundTaskManager::new();
        manager
            .start_question("question-test".to_owned(), "Pick one".to_owned())
            .await;

        let active = manager.list(true, 10).await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].kind, BackgroundTaskKind::Question);
        assert_eq!(active[0].status, BackgroundTaskStatus::WaitingForUser);

        manager
            .complete_question("question-test", vec!["Project config".to_owned()])
            .await;

        assert!(manager.list(true, 10).await.is_empty());
        let all = manager.list(false, 10).await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, BackgroundTaskStatus::Completed);
        assert_eq!(all[0].answers, Some(vec!["Project config".to_owned()]));
    }

    #[tokio::test]
    async fn manager_stops_question_and_ignores_late_answer() {
        let manager = BackgroundTaskManager::new();
        manager
            .start_question("question-stop".to_owned(), "Pick one".to_owned())
            .await;

        let stopped = manager
            .stop("question-stop", "Stopped by test", 1024)
            .await
            .expect("question should stop");
        assert_eq!(stopped.details.as_ref().unwrap()["status"], "stopped");

        manager
            .complete_question("question-stop", vec!["Too late".to_owned()])
            .await;

        let output = manager
            .output("question-stop", false, Duration::from_millis(1), 1024)
            .await
            .expect("stopped question should be readable");
        let details = output.details.expect("details");
        assert_eq!(details["status"], "stopped");
        assert!(details.get("answers").is_none());
    }

    #[tokio::test]
    async fn manager_finishes_bash_and_truncates_output() {
        let manager = BackgroundTaskManager::new();
        let started = manager
            .start_bash(
                "run fake command".to_owned(),
                fake_command(Some(0), "abcdef", "stderr"),
                3,
            )
            .await
            .expect("bash task should start");
        let task_id = result_task_id(&started);

        let output = manager
            .output(&task_id, false, Duration::from_millis(1), 3)
            .await
            .expect("bash task should be readable");
        assert!(output.content.contains("status: completed"));
        assert!(output.content.contains("[output]\nabcstd"));
        assert!(output.content.contains("[output truncated]"));

        let details = output.details.expect("details");
        assert_eq!(details["kind"], "bash");
        assert_eq!(details["status"], "completed");
        assert_eq!(details["exit_code"], 0);
        assert_eq!(details["stdout"], "abc");
        assert_eq!(details["stderr"], "std");
        assert_eq!(details["truncated"], true);
    }

    #[test]
    fn task_list_result_shows_empty_notice() {
        let result = task_list_result(&[], true);
        assert!(result.content.contains("active_background_tasks: 0"));
        assert!(result.content.contains("No background tasks found."));
    }

    #[test]
    fn task_list_result_lists_tasks() {
        let snapshot = BackgroundTaskSnapshot {
            task_id: "bash-abc".to_owned(),
            kind: BackgroundTaskKind::Bash,
            status: BackgroundTaskStatus::Running,
            description: "long command".to_owned(),
            elapsed: Duration::from_secs(5),
            output: None,
            answers: None,
        };
        let result = task_list_result(&[snapshot], true);
        assert!(result.content.contains("active_background_tasks: 1"));
        assert!(result.content.contains("task_id: bash-abc"));
        assert!(result.content.contains("status: running"));
    }

    #[tokio::test]
    async fn task_list_tool_lists_active_tasks() {
        let manager = BackgroundTaskManager::new();
        manager
            .start_question("q-1".to_owned(), "Pick one".to_owned())
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path())
            .unwrap()
            .with_background_tasks(manager);
        let tool = TaskListTool;
        let result = tool.execute(&ctx, json!({})).await.expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("active_background_tasks: 1"));
        assert!(result.content.contains("task_id: q-1"));
    }

    #[tokio::test]
    async fn task_output_tool_reads_task() {
        let manager = BackgroundTaskManager::new();
        let started = manager
            .start_bash(
                "run fake command".to_owned(),
                fake_command(Some(0), "hello", ""),
                1024,
            )
            .await
            .expect("bash task should start");
        let task_id = result_task_id(&started);
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path())
            .unwrap()
            .with_background_tasks(manager);
        let tool = TaskOutputTool;
        let result = tool
            .execute(&ctx, json!({"task_id": task_id}))
            .await
            .expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("task_id:"));
        assert!(result.content.contains("status:"));
    }

    #[tokio::test]
    async fn task_stop_tool_stops_running_bash() {
        let manager = BackgroundTaskManager::new();
        let started = manager
            .start_bash(
                "run fake command".to_owned(),
                fake_command(Some(0), "hello", ""),
                1024,
            )
            .await
            .expect("bash task should start");
        let task_id = result_task_id(&started);
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path())
            .unwrap()
            .with_access(crate::ToolAccess::all())
            .with_background_tasks(manager);
        let tool = TaskStopTool;
        let result = tool
            .execute(&ctx, json!({"task_id": task_id, "reason": "test done"}))
            .await
            .expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("task_id:"));
        assert!(result.content.contains("status: stopped"));
    }

    #[tokio::test]
    async fn task_stop_tool_requires_shell_permission() {
        let manager = BackgroundTaskManager::new();
        let started = manager
            .start_bash(
                "run fake command".to_owned(),
                fake_command(Some(0), "hello", ""),
                1024,
            )
            .await
            .expect("bash task should start");
        let task_id = result_task_id(&started);
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path())
            .unwrap()
            .with_access(crate::ToolAccess {
                file_read: true,
                file_write: false,
                shell: false,
                tool: true,
                user_question: false,
            })
            .with_background_tasks(manager);
        let tool = TaskStopTool;
        let result = tool.execute(&ctx, json!({"task_id": task_id})).await;
        assert!(result.is_err());
    }

    #[test]
    fn tool_descriptions_are_non_empty() {
        assert!(!TaskListTool.description().is_empty());
        assert!(!TaskOutputTool.description().is_empty());
        assert!(!TaskStopTool.description().is_empty());
    }
}
