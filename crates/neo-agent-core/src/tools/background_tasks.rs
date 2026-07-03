use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{io::AsyncWriteExt, sync::Mutex, task::JoinHandle};
use uuid::Uuid;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};
use crate::QuestionEventData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskKind {
    Bash,
    Question,
    Delegate,
    DelegateSwarm,
}

impl BackgroundTaskKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Question => "question",
            Self::Delegate => "delegate",
            Self::DelegateSwarm => "delegate-swarm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    WaitingForUser,
    Completed,
    Failed,
    Cancelled,
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
            Self::Cancelled => "cancelled",
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
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

pub struct ManagedBackgroundCommand {
    pub stdout: Arc<Mutex<Vec<u8>>>,
    pub stderr: Arc<Mutex<Vec<u8>>>,
    pub stdout_truncated: Arc<Mutex<bool>>,
    pub stderr_truncated: Arc<Mutex<bool>>,
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
    pub delegate: Option<crate::multi_agent::AgentSnapshot>,
    pub swarm: Option<crate::multi_agent::SwarmSnapshot>,
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
    DelegateRunning {
        snapshot: crate::multi_agent::AgentSnapshot,
    },
    DelegateFinished {
        status: BackgroundTaskStatus,
        snapshot: crate::multi_agent::AgentSnapshot,
    },
    DelegateSwarmRunning {
        snapshot: crate::multi_agent::SwarmSnapshot,
    },
    DelegateSwarmFinished {
        status: BackgroundTaskStatus,
        snapshot: crate::multi_agent::SwarmSnapshot,
    },
}

struct BackgroundTaskRecord {
    description: String,
    started_at: Instant,
    state: BackgroundTaskState,
    detached: bool,
    deadline: Option<Instant>,
    detach_timeout: Option<Duration>,
}

#[derive(Clone, Default)]
pub struct BackgroundTaskManager {
    inner: Arc<Mutex<HashMap<String, BackgroundTaskRecord>>>,
    persistence_dir: Option<Arc<PathBuf>>,
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

    #[must_use]
    pub fn with_persistence_dir(mut self, path: PathBuf) -> Self {
        self.persistence_dir = Some(Arc::new(path));
        self
    }

    #[must_use]
    pub(crate) fn next_bash_task_id(&self) -> String {
        format!("bash-{}", Uuid::new_v4())
    }

    pub async fn start_bash(
        &self,
        description: String,
        command: ManagedBackgroundCommand,
        max_output_bytes: usize,
    ) -> Result<ToolResult, ToolError> {
        let task_id = self.next_bash_task_id();
        self.start_bash_with_task_id(task_id, description, command, max_output_bytes)
            .await
    }

    pub(crate) async fn start_bash_with_task_id(
        &self,
        task_id: String,
        description: String,
        command: ManagedBackgroundCommand,
        max_output_bytes: usize,
    ) -> Result<ToolResult, ToolError> {
        let description_trimmed = description.trim().to_owned();
        self.inner.lock().await.insert(
            task_id.clone(),
            BackgroundTaskRecord {
                description,
                started_at: Instant::now(),
                state: BackgroundTaskState::BashRunning(command),
                detached: true,
                deadline: None,
                detach_timeout: None,
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
            "outcome": "backgrounded",
            "max_output_bytes": max_output_bytes,
        })))
    }

    pub(crate) async fn append_persistent_output(
        &self,
        task_id: &str,
        chunk: &str,
    ) -> Result<(), ToolError> {
        let Some(root) = &self.persistence_dir else {
            return Ok(());
        };
        let output = root.join(task_id).join("output.log");
        if let Some(parent) = output.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(output)
            .await?;
        file.write_all(chunk.as_bytes()).await?;
        Ok(())
    }

    #[cfg(test)]
    pub async fn persist_task_output_for_test(
        &self,
        task_id: &str,
        chunk: &str,
    ) -> Result<(), ToolError> {
        self.append_persistent_output(task_id, chunk).await
    }

    pub async fn start_question(&self, task_id: String, description: String) -> ToolResult {
        self.inner.lock().await.insert(
            task_id.clone(),
            BackgroundTaskRecord {
                description: description.clone(),
                started_at: Instant::now(),
                state: BackgroundTaskState::QuestionWaiting,
                detached: true,
                deadline: None,
                detach_timeout: None,
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

    /// Register a delegate agent as a background task. Returns the task ID
    /// (the agent ID).
    pub async fn start_delegate(&self, snapshot: crate::multi_agent::AgentSnapshot) -> String {
        let task_id = snapshot.id.as_str().to_owned();
        let description = snapshot.task.clone();
        let state = if snapshot.state.is_terminal() {
            let status = match snapshot.state {
                crate::multi_agent::AgentLifecycleState::Completed => {
                    BackgroundTaskStatus::Completed
                }
                crate::multi_agent::AgentLifecycleState::Failed => BackgroundTaskStatus::Failed,
                crate::multi_agent::AgentLifecycleState::Cancelled => {
                    BackgroundTaskStatus::Cancelled
                }
                crate::multi_agent::AgentLifecycleState::TimedOut => BackgroundTaskStatus::TimedOut,
                _ => unreachable!(),
            };
            BackgroundTaskState::DelegateFinished { status, snapshot }
        } else {
            BackgroundTaskState::DelegateRunning { snapshot }
        };
        self.inner.lock().await.insert(
            task_id.clone(),
            BackgroundTaskRecord {
                description,
                started_at: Instant::now(),
                state,
                detached: true,
                deadline: None,
                detach_timeout: None,
            },
        );
        task_id
    }

    /// Mark a running delegate as completed.
    pub async fn complete_delegate(
        &self,
        task_id: &str,
        snapshot: crate::multi_agent::AgentSnapshot,
    ) {
        let mut tasks = self.inner.lock().await;
        if let Some(record) = tasks.get_mut(task_id)
            && matches!(record.state, BackgroundTaskState::DelegateRunning { .. })
        {
            record.state = BackgroundTaskState::DelegateFinished {
                status: BackgroundTaskStatus::Completed,
                snapshot,
            };
        }
    }

    /// Mark a running delegate as cancelled.
    pub async fn cancel_delegate(
        &self,
        task_id: &str,
        snapshot: crate::multi_agent::AgentSnapshot,
    ) {
        let mut tasks = self.inner.lock().await;
        if let Some(record) = tasks.get_mut(task_id)
            && matches!(record.state, BackgroundTaskState::DelegateRunning { .. })
        {
            record.state = BackgroundTaskState::DelegateFinished {
                status: BackgroundTaskStatus::Cancelled,
                snapshot,
            };
        }
    }

    /// Register a delegate swarm as a background task. Returns the task ID
    /// (the swarm ID).
    pub async fn start_delegate_swarm(
        &self,
        snapshot: crate::multi_agent::SwarmSnapshot,
    ) -> String {
        let task_id = snapshot.swarm_id.clone();
        self.inner.lock().await.insert(
            task_id.clone(),
            BackgroundTaskRecord {
                description: snapshot.description.clone(),
                started_at: Instant::now(),
                state: BackgroundTaskState::DelegateSwarmRunning { snapshot },
                detached: true,
                deadline: None,
                detach_timeout: None,
            },
        );
        task_id
    }

    /// Update a running delegate swarm's snapshot.
    pub async fn update_delegate_swarm(
        &self,
        task_id: &str,
        snapshot: crate::multi_agent::SwarmSnapshot,
    ) {
        let mut tasks = self.inner.lock().await;
        if let Some(record) = tasks.get_mut(task_id)
            && matches!(
                record.state,
                BackgroundTaskState::DelegateSwarmRunning { .. }
            )
        {
            record.state = BackgroundTaskState::DelegateSwarmRunning { snapshot };
        }
    }

    /// Mark a running delegate swarm as completed.
    pub async fn complete_delegate_swarm(
        &self,
        task_id: &str,
        snapshot: crate::multi_agent::SwarmSnapshot,
    ) {
        let mut tasks = self.inner.lock().await;
        if let Some(record) = tasks.get_mut(task_id)
            && matches!(
                record.state,
                BackgroundTaskState::DelegateSwarmRunning { .. }
            )
        {
            record.state = BackgroundTaskState::DelegateSwarmFinished {
                status: BackgroundTaskStatus::Completed,
                snapshot,
            };
        }
    }

    /// Mark a running delegate swarm as cancelled.
    pub async fn cancel_delegate_swarm(
        &self,
        task_id: &str,
        snapshot: crate::multi_agent::SwarmSnapshot,
    ) {
        let mut tasks = self.inner.lock().await;
        if let Some(record) = tasks.get_mut(task_id)
            && matches!(
                record.state,
                BackgroundTaskState::DelegateSwarmRunning { .. }
            )
        {
            record.state = BackgroundTaskState::DelegateSwarmFinished {
                status: BackgroundTaskStatus::Cancelled,
                snapshot,
            };
        }
    }

    /// Finish a background delegate using the snapshot's actual lifecycle
    /// state to derive the status. This prevents a cancelled child run from
    /// being recorded as `Completed` when cancellation happened through the
    /// runtime before the background record was finalized.
    pub async fn finish_delegate(
        &self,
        task_id: &str,
        snapshot: crate::multi_agent::AgentSnapshot,
    ) {
        let status = status_from_agent_state(snapshot.state);
        let mut tasks = self.inner.lock().await;
        if let Some(record) = tasks.get_mut(task_id)
            && matches!(record.state, BackgroundTaskState::DelegateRunning { .. })
        {
            record.state = BackgroundTaskState::DelegateFinished { status, snapshot };
        }
    }

    /// Finish a background delegate swarm using the snapshot's actual
    /// lifecycle state to derive the status.
    pub async fn finish_delegate_swarm(
        &self,
        task_id: &str,
        snapshot: crate::multi_agent::SwarmSnapshot,
    ) {
        let status = status_from_agent_state(snapshot.state);
        let mut tasks = self.inner.lock().await;
        if let Some(record) = tasks.get_mut(task_id)
            && matches!(
                record.state,
                BackgroundTaskState::DelegateSwarmRunning { .. }
            )
        {
            record.state = BackgroundTaskState::DelegateSwarmFinished { status, snapshot };
        }
    }

    pub async fn start_bash_foreground(
        &self,
        description: String,
        command: ManagedBackgroundCommand,
        _max_output_bytes: usize,
        detach_timeout: Duration,
    ) -> Result<String, ToolError> {
        let task_id = self.next_bash_task_id();
        self.start_bash_foreground_with_task_id(task_id, description, command, detach_timeout)
            .await
    }

    pub(crate) async fn start_bash_foreground_with_task_id(
        &self,
        task_id: String,
        description: String,
        command: ManagedBackgroundCommand,
        detach_timeout: Duration,
    ) -> Result<String, ToolError> {
        self.inner.lock().await.insert(
            task_id.clone(),
            BackgroundTaskRecord {
                description,
                started_at: Instant::now(),
                state: BackgroundTaskState::BashRunning(command),
                detached: false,
                deadline: None,
                detach_timeout: Some(detach_timeout),
            },
        );
        Ok(task_id)
    }

    pub async fn detach(&self, task_id: &str) -> Result<BackgroundTaskSnapshot, ToolError> {
        {
            let mut tasks = self.inner.lock().await;
            let Some(record) = tasks.get_mut(task_id) else {
                return Err(ToolError::InvalidInput {
                    tool: "TaskDetach".to_owned(),
                    message: format!("unknown background task `{task_id}`"),
                });
            };
            if !matches!(record.state, BackgroundTaskState::BashRunning(_)) {
                return Err(ToolError::InvalidInput {
                    tool: "TaskDetach".to_owned(),
                    message: format!("background task `{task_id}` is not running"),
                });
            }
            record.detached = true;
            record.deadline = record
                .detach_timeout
                .map(|timeout| Instant::now() + timeout);
        }
        self.snapshot(task_id).await
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
            AlreadyTerminal(Box<BackgroundTaskSnapshot>),
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
                    StopAction::AlreadyTerminal(Box::new(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Bash,
                        status: *status,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: Some(output.clone()),
                        answers: None,
                        delegate: None,
                        swarm: None,
                    }))
                }
                BackgroundTaskState::QuestionFinished { status, answers } => {
                    StopAction::AlreadyTerminal(Box::new(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Question,
                        status: *status,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: answers.clone(),
                        delegate: None,
                        swarm: None,
                    }))
                }
                BackgroundTaskState::DelegateFinished { status, snapshot } => {
                    return Ok(ToolResult::error(format!(
                        "agent already {}; terminal delegate state is immutable. To continue it, call Delegate with resume=\"{}\".",
                        status.as_str(),
                        snapshot.id.as_str()
                    ))
                    .with_details(serde_json::json!({
                        "task_id": task_id,
                        "kind": "delegate",
                        "status": status.as_str(),
                        "agent_id": snapshot.id.as_str(),
                        "terminal": true,
                        "resume_hint": format!("Delegate with resume=\"{}\"", snapshot.id.as_str()),
                    })));
                }
                BackgroundTaskState::DelegateSwarmFinished { status, snapshot } => {
                    return Ok(ToolResult::error(format!(
                        "swarm already {}; terminal delegate state is immutable. To continue unfinished children, call DelegateSwarm with resume_agent_ids.",
                        status.as_str()
                    ))
                    .with_details(serde_json::json!({
                        "task_id": task_id,
                        "kind": "delegate-swarm",
                        "status": status.as_str(),
                        "swarm_id": snapshot.swarm_id.as_str(),
                        "terminal": true,
                        "resume_hint": "DelegateSwarm with resume_agent_ids",
                    })));
                }
                BackgroundTaskState::QuestionWaiting => {
                    let record = tasks.get_mut(task_id).expect("record still exists");
                    record.state = BackgroundTaskState::QuestionFinished {
                        status: BackgroundTaskStatus::Cancelled,
                        answers: None,
                    };
                    StopAction::StopQuestion {
                        started_at: record.started_at,
                        description: record.description.clone(),
                    }
                }
                BackgroundTaskState::DelegateRunning { snapshot: _ } => {
                    let record = tasks.get_mut(task_id).expect("record still exists");
                    let mut snapshot = match &record.state {
                        BackgroundTaskState::DelegateRunning { snapshot } => snapshot.clone(),
                        _ => unreachable!(),
                    };
                    snapshot.state = crate::multi_agent::AgentLifecycleState::Cancelled;
                    record.state = BackgroundTaskState::DelegateFinished {
                        status: BackgroundTaskStatus::Cancelled,
                        snapshot: snapshot.clone(),
                    };
                    let snap = BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Delegate,
                        status: BackgroundTaskStatus::Cancelled,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: None,
                        delegate: Some(snapshot),
                        swarm: None,
                    };
                    return Ok(snapshot_result(&snap, max_output_bytes));
                }
                BackgroundTaskState::DelegateSwarmRunning { snapshot: _ } => {
                    let record = tasks.get_mut(task_id).expect("record still exists");
                    let snapshot = match &record.state {
                        BackgroundTaskState::DelegateSwarmRunning { snapshot } => snapshot.clone(),
                        _ => unreachable!(),
                    };
                    record.state = BackgroundTaskState::DelegateSwarmFinished {
                        status: BackgroundTaskStatus::Cancelled,
                        snapshot: snapshot.clone(),
                    };
                    let snap = BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::DelegateSwarm,
                        status: BackgroundTaskStatus::Cancelled,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: None,
                        delegate: None,
                        swarm: Some(snapshot),
                    };
                    return Ok(snapshot_result(&snap, max_output_bytes));
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
            StopAction::AlreadyTerminal(snapshot) => {
                Ok(snapshot_result(&snapshot, max_output_bytes))
            }
            StopAction::StopQuestion {
                started_at,
                description,
            } => {
                let snapshot = BackgroundTaskSnapshot {
                    task_id: task_id.to_owned(),
                    kind: BackgroundTaskKind::Question,
                    status: BackgroundTaskStatus::Cancelled,
                    description,
                    elapsed: started_at.elapsed(),
                    output: None,
                    answers: None,
                    delegate: None,
                    swarm: None,
                };
                let mut result = snapshot_result(&snapshot, max_output_bytes);
                result.details = Some(json!({
                    "task_id": task_id,
                    "kind": "question",
                    "status": "cancelled",
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
                let output = output_from_command_buffers(
                    exit_code,
                    command.stdout,
                    command.stderr,
                    command.stdout_truncated,
                    command.stderr_truncated,
                )
                .await;
                let snapshot = BackgroundTaskSnapshot {
                    task_id: task_id.to_owned(),
                    kind: BackgroundTaskKind::Bash,
                    status: BackgroundTaskStatus::Cancelled,
                    description: description.clone(),
                    elapsed: started_at.elapsed(),
                    output: Some(output.clone()),
                    answers: None,
                    delegate: None,
                    swarm: None,
                };
                self.inner.lock().await.insert(
                    task_id.to_owned(),
                    BackgroundTaskRecord {
                        description,
                        started_at,
                        state: BackgroundTaskState::BashFinished {
                            status: BackgroundTaskStatus::Cancelled,
                            output,
                        },
                        detached: true,
                        deadline: None,
                        detach_timeout: None,
                    },
                );
                Ok(snapshot_result(&snapshot, max_output_bytes))
            }
        }
    }

    pub async fn snapshot(&self, task_id: &str) -> Result<BackgroundTaskSnapshot, ToolError> {
        self.snapshot_inner(task_id)
            .await
            .ok_or_else(|| ToolError::InvalidInput {
                tool: "TaskOutput".to_owned(),
                message: format!("unknown background task `{task_id}`"),
            })
    }

    pub async fn is_detached(&self, task_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .get(task_id)
            .is_some_and(|record| record.detached)
    }

    pub async fn foreground_bash_task_id(&self) -> Option<String> {
        self.inner
            .lock()
            .await
            .iter()
            .find_map(|(task_id, record)| {
                if !record.detached && matches!(record.state, BackgroundTaskState::BashRunning(_)) {
                    Some(task_id.clone())
                } else {
                    None
                }
            })
    }

    #[allow(clippy::too_many_lines)]
    async fn snapshot_inner(&self, task_id: &str) -> Option<BackgroundTaskSnapshot> {
        #[allow(clippy::large_enum_variant)]
        enum SnapshotAction {
            Ready(BackgroundTaskSnapshot),
            Running {
                started_at: Instant,
                description: String,
                stdout: Arc<Mutex<Vec<u8>>>,
                stderr: Arc<Mutex<Vec<u8>>>,
                stdout_truncated: Arc<Mutex<bool>>,
                stderr_truncated: Arc<Mutex<bool>>,
            },
            Timeout {
                started_at: Instant,
                description: String,
                command: ManagedBackgroundCommand,
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
                        delegate: None,
                        swarm: None,
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
                        delegate: None,
                        swarm: None,
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
                        delegate: None,
                        swarm: None,
                    })
                }
                BackgroundTaskState::DelegateRunning { snapshot } => {
                    SnapshotAction::Ready(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Delegate,
                        status: BackgroundTaskStatus::Running,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: None,
                        delegate: Some(snapshot.clone()),
                        swarm: None,
                    })
                }
                BackgroundTaskState::DelegateFinished { status, snapshot } => {
                    SnapshotAction::Ready(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::Delegate,
                        status: *status,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: None,
                        delegate: Some(snapshot.clone()),
                        swarm: None,
                    })
                }
                BackgroundTaskState::DelegateSwarmRunning { snapshot } => {
                    SnapshotAction::Ready(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::DelegateSwarm,
                        status: BackgroundTaskStatus::Running,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: None,
                        delegate: None,
                        swarm: Some(snapshot.clone()),
                    })
                }
                BackgroundTaskState::DelegateSwarmFinished { status, snapshot } => {
                    SnapshotAction::Ready(BackgroundTaskSnapshot {
                        task_id: task_id.to_owned(),
                        kind: BackgroundTaskKind::DelegateSwarm,
                        status: *status,
                        description: record.description.clone(),
                        elapsed: record.started_at.elapsed(),
                        output: None,
                        answers: None,
                        delegate: None,
                        swarm: Some(snapshot.clone()),
                    })
                }
                BackgroundTaskState::BashRunning(command)
                    if record.detached
                        && record
                            .deadline
                            .is_some_and(|deadline| deadline <= Instant::now()) =>
                {
                    let record = tasks.remove(task_id).expect("record still exists");
                    let BackgroundTaskState::BashRunning(command) = record.state else {
                        unreachable!();
                    };
                    SnapshotAction::Timeout {
                        started_at: record.started_at,
                        description: record.description,
                        command,
                    }
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
                        stdout_truncated: command.stdout_truncated.clone(),
                        stderr_truncated: command.stderr_truncated.clone(),
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
                stdout_truncated,
                stderr_truncated,
            } => {
                let stdout = stdout.lock().await;
                let stderr = stderr.lock().await;
                let stdout_truncated = *stdout_truncated.lock().await;
                let stderr_truncated = *stderr_truncated.lock().await;
                Some(BackgroundTaskSnapshot {
                    task_id: task_id.to_owned(),
                    kind: BackgroundTaskKind::Bash,
                    status: BackgroundTaskStatus::Running,
                    description,
                    elapsed: started_at.elapsed(),
                    output: Some(output_from_locked_buffers(
                        None,
                        &stdout,
                        &stderr,
                        stdout_truncated,
                        stderr_truncated,
                    )),
                    answers: None,
                    delegate: None,
                    swarm: None,
                })
            }
            SnapshotAction::Timeout {
                started_at,
                description,
                command,
            } => {
                let exit_code = (command.cleanup)().await;
                (command.drain)(command.stdout_task).await;
                (command.drain)(command.stderr_task).await;
                let output = output_from_command_buffers(
                    exit_code,
                    command.stdout,
                    command.stderr,
                    command.stdout_truncated,
                    command.stderr_truncated,
                )
                .await;
                let snapshot = BackgroundTaskSnapshot {
                    task_id: task_id.to_owned(),
                    kind: BackgroundTaskKind::Bash,
                    status: BackgroundTaskStatus::TimedOut,
                    description: description.clone(),
                    elapsed: started_at.elapsed(),
                    output: Some(output.clone()),
                    answers: None,
                    delegate: None,
                    swarm: None,
                };
                self.inner.lock().await.insert(
                    task_id.to_owned(),
                    BackgroundTaskRecord {
                        description,
                        started_at,
                        state: BackgroundTaskState::BashFinished {
                            status: BackgroundTaskStatus::TimedOut,
                            output,
                        },
                        detached: true,
                        deadline: None,
                        detach_timeout: None,
                    },
                );
                Some(snapshot)
            }
            SnapshotAction::Finish {
                started_at,
                description,
                command,
                exit_code,
            } => {
                (command.drain)(command.stdout_task).await;
                (command.drain)(command.stderr_task).await;
                let output = output_from_command_buffers(
                    exit_code,
                    command.stdout,
                    command.stderr,
                    command.stdout_truncated,
                    command.stderr_truncated,
                )
                .await;
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
                    delegate: None,
                    swarm: None,
                };
                self.inner.lock().await.insert(
                    task_id.to_owned(),
                    BackgroundTaskRecord {
                        description,
                        started_at,
                        state: BackgroundTaskState::BashFinished { status, output },
                        detached: true,
                        deadline: None,
                        detach_timeout: None,
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
    /// Short reason recorded when the task is cancelled.
    #[schemars(
        description = "Short reason recorded when the task is cancelled. Defaults to 'Cancelled by TaskStop'."
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
         - status: \"running\", \"completed\", \"failed\", \"cancelled\", or \"timed_out\".\n\
         - kind: The type of background work, such as \"bash\", \"question\", \"delegate\", or \"delegate-swarm\".\n\
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
            let mut tasks = ctx.background_tasks.list(active_only, limit).await;
            let existing_ids = tasks
                .iter()
                .map(|task| task.task_id.clone())
                .collect::<HashSet<_>>();
            tasks.extend(runtime_delegate_task_snapshots(
                ctx,
                active_only,
                &existing_ids,
            ));
            tasks.sort_by(|left, right| left.task_id.cmp(&right.task_id));
            tasks.truncate(limit);
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
         - For delegate agent IDs and swarm IDs, this tool returns the canonical multi-agent result shape used by Delegate, DelegateSwarm, and WaitDelegate.\n\
         - For a terminal task, check `status` and `exit_code` to understand why it ended.\n\
         - This tool works with the generic background task system and should remain the primary read path for future task types.\n\n\
         Return fields:\n\
         - status: One of \"running\" (the task is still executing), \"completed\" (the task \
         finished successfully), \"failed\" (the task exited with a non-zero exit code), \
         \"cancelled\" (the task was cancelled via TaskStop), or \"timed_out\" (the task timed out).\n\
         - exit_code: The process exit code for terminal tasks. 0 means success; non-zero means \
         failure. Only present when status is \"completed\", \"failed\", \"cancelled\", or \"timed_out\".\n\
         - output: A preview of the task's stdout/stderr, capped at max_output_bytes."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TaskOutputInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: TaskOutputInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);

            if let Some(agent) = ctx.multi_agent.agent_snapshot(&input.task_id) {
                return Ok(
                    ToolResult::ok(super::multi_agent_format::delegate_result_content(
                        &agent,
                        agent.context,
                    ))
                    .with_details(super::multi_agent_format::agent_details(
                        "delegate",
                        &agent,
                        Some(agent.context),
                        super::multi_agent_format::SummaryScope::CurrentRun,
                        true,
                        true,
                        false,
                    )),
                );
            }

            // Route swarm IDs to rich swarm output from the runtime.
            if input.task_id.starts_with("swarm_")
                && let Some(swarm) = ctx.multi_agent.swarm_snapshot(&input.task_id)
            {
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
                return Ok(ToolResult::ok(content)
                    .with_details(super::multi_agent_format::swarm_details(&swarm)));
            }

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
         - If a terminal `bash` or `question` task has already finished, this tool returns its current status.\n\
         - If a terminal `delegate` or `delegate-swarm` task has already finished, this tool returns an `already <state>` error with a resume hint.\n\
         - Provide a short `reason` so the task history records why it was cancelled.\n\n\
         Return format:\n\
         Returns the task's final status after the stop attempt. If the task was still running, it \
         is cancelled and the output collected so far is included."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TaskStopInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_shell_allowed()?;
            let input: TaskStopInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            // For swarms, cancel non-terminal children via the runtime and update
            // the background task record, then return the result.
            if input.task_id.starts_with("swarm_") {
                match ctx.multi_agent.cancel_swarm(&input.task_id) {
                    Ok(swarm) => {
                        let () = ctx
                            .background_tasks
                            .cancel_delegate_swarm(&input.task_id, swarm.clone())
                            .await;
                        // The background task record is now terminal; return its output.
                        return ctx
                            .background_tasks
                            .output(
                                &input.task_id,
                                false,
                                Duration::from_secs(0),
                                max_output_bytes,
                            )
                            .await;
                    }
                    Err(message) => {
                        if ctx.background_tasks.snapshot(&input.task_id).await.is_err() {
                            return Ok(ToolResult::error(message));
                        }
                        // Swarm is terminal in runtime but has a background task
                        // record; let the task manager return its richer
                        // already-terminal result.
                    }
                }
            }
            // For delegate IDs, cancel the live token via the runtime FIRST,
            // then finalize the background record from the canonical snapshot.
            // This ensures the child model stream genuinely stops before the
            // background record is marked terminal.
            if !input.task_id.starts_with("swarm_") {
                // Check whether this is a delegate background task before
                // attempting runtime cancellation.
                let is_delegate = ctx
                    .background_tasks
                    .snapshot(&input.task_id)
                    .await
                    .is_ok_and(|snap| snap.kind == BackgroundTaskKind::Delegate);
                if is_delegate {
                    if let Some(snapshot) = ctx.multi_agent.cancel_agent_by_id(&input.task_id) {
                        ctx.background_tasks
                            .finish_delegate(&input.task_id, snapshot)
                            .await;
                        return ctx
                            .background_tasks
                            .output(
                                &input.task_id,
                                false,
                                Duration::from_secs(0),
                                max_output_bytes,
                            )
                            .await;
                    }
                    // Agent already terminal; fall through to stop() for the
                    // already-terminal error message.
                }
            }
            let result = ctx
                .background_tasks
                .stop(
                    &input.task_id,
                    input.reason.as_deref().unwrap_or("Cancelled by TaskStop"),
                    max_output_bytes,
                )
                .await?;
            Ok(result)
        })
    }
}

fn status_from_agent_state(state: crate::multi_agent::AgentLifecycleState) -> BackgroundTaskStatus {
    match state {
        crate::multi_agent::AgentLifecycleState::Queued
        | crate::multi_agent::AgentLifecycleState::Running => BackgroundTaskStatus::Running,
        crate::multi_agent::AgentLifecycleState::Completed => BackgroundTaskStatus::Completed,
        crate::multi_agent::AgentLifecycleState::Failed => BackgroundTaskStatus::Failed,
        crate::multi_agent::AgentLifecycleState::Cancelled
        | crate::multi_agent::AgentLifecycleState::Interrupted => BackgroundTaskStatus::Cancelled,
        crate::multi_agent::AgentLifecycleState::TimedOut => BackgroundTaskStatus::TimedOut,
    }
}

fn runtime_delegate_task_snapshots(
    ctx: &ToolContext,
    active_only: bool,
    existing_ids: &HashSet<String>,
) -> Vec<BackgroundTaskSnapshot> {
    let mut snapshots = Vec::new();

    for agent in ctx.multi_agent.list_agents(!active_only) {
        let task_id = agent.id.as_str().to_owned();
        if existing_ids.contains(&task_id) {
            continue;
        }
        let status = status_from_agent_state(agent.state);
        if active_only && !status.is_active() {
            continue;
        }
        snapshots.push(BackgroundTaskSnapshot {
            task_id,
            kind: BackgroundTaskKind::Delegate,
            status,
            description: agent.display_title(),
            elapsed: agent.elapsed,
            output: None,
            answers: None,
            delegate: Some(agent),
            swarm: None,
        });
    }

    for swarm in ctx.multi_agent.list_swarms() {
        if existing_ids.contains(&swarm.swarm_id) {
            continue;
        }
        let status = status_from_agent_state(swarm.state);
        if active_only && !status.is_active() {
            continue;
        }
        snapshots.push(BackgroundTaskSnapshot {
            task_id: swarm.swarm_id.clone(),
            kind: BackgroundTaskKind::DelegateSwarm,
            status,
            description: swarm.description.clone(),
            elapsed: Duration::ZERO,
            output: None,
            answers: None,
            delegate: None,
            swarm: Some(swarm),
        });
    }

    snapshots
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
    if let Some(delegate) = &snapshot.delegate {
        if let Some(outcome) = &delegate.outcome {
            content.push_str(&format!("\nsummary: {}", outcome.summary));
        }
        details["agent_id"] = json!(delegate.id.as_str());
        details["state"] = json!(delegate.state);
    }
    if let Some(output) = &snapshot.output {
        let (stdout_capped, stdout_truncated) = cap_plain_output(&output.stdout, max_output_bytes);
        let (stderr_capped, stderr_truncated) = cap_plain_output(&output.stderr, max_output_bytes);
        let truncated = output.stdout_truncated
            || output.stderr_truncated
            || stdout_truncated
            || stderr_truncated;
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
        details["stdout_truncated"] = json!(output.stdout_truncated || stdout_truncated);
        details["stderr_truncated"] = json!(output.stderr_truncated || stderr_truncated);
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
    output_from_locked_buffers(exit_code, &stdout, &stderr, false, false)
}

pub async fn output_from_command_buffers(
    exit_code: Option<i32>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_truncated: Arc<Mutex<bool>>,
    stderr_truncated: Arc<Mutex<bool>>,
) -> CommandOutput {
    let stdout_truncated = *stdout_truncated.lock().await;
    let stderr_truncated = *stderr_truncated.lock().await;
    let stdout = stdout.lock().await;
    let stderr = stderr.lock().await;
    output_from_locked_buffers(
        exit_code,
        &stdout,
        &stderr,
        stdout_truncated,
        stderr_truncated,
    )
}

fn output_from_locked_buffers(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    stdout_truncated: bool,
    stderr_truncated: bool,
) -> CommandOutput {
    CommandOutput {
        exit_code,
        stdout: String::from_utf8_lossy(stdout).into_owned(),
        stderr: String::from_utf8_lossy(stderr).into_owned(),
        stdout_truncated,
        stderr_truncated,
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
            stdout_truncated: Arc::new(Mutex::new(false)),
            stderr_truncated: Arc::new(Mutex::new(false)),
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
    async fn background_task_manager_persists_output_under_agent_tasks_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tasks_dir = temp.path().join("agents").join("main").join("tasks");
        let manager = BackgroundTaskManager::new().with_persistence_dir(tasks_dir.clone());

        manager
            .persist_task_output_for_test("bash-12345678", "hello\n")
            .await
            .expect("persist output");

        assert_eq!(
            tokio::fs::read_to_string(tasks_dir.join("bash-12345678").join("output.log"))
                .await
                .expect("read output"),
            "hello\n"
        );
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
            .stop("question-stop", "Cancelled by test", 1024)
            .await
            .expect("question should stop");
        assert_eq!(stopped.details.as_ref().unwrap()["status"], "cancelled");

        manager
            .complete_question("question-stop", vec!["Too late".to_owned()])
            .await;

        let output = manager
            .output("question-stop", false, Duration::from_millis(1), 1024)
            .await
            .expect("stopped question should be readable");
        let details = output.details.expect("details");
        assert_eq!(details["status"], "cancelled");
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
            delegate: None,
            swarm: None,
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
    async fn task_list_tool_includes_active_runtime_delegate_without_background_record() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path()).unwrap();
        let agent = ctx
            .multi_agent
            .start_foreground_delegate_for_test("calculate a small sum");

        let tool = TaskListTool;
        let result = tool.execute(&ctx, json!({})).await.expect("execute");

        assert!(!result.is_error);
        assert!(result.content.contains("active_background_tasks: 1"));
        assert!(
            result
                .content
                .contains(&format!("task_id: {}", agent.id.as_str()))
        );
        assert!(result.content.contains("kind: delegate"));
        assert!(result.content.contains("status: running"));
        assert_eq!(
            result.details.as_ref().unwrap()["tasks"][0]["task_id"],
            agent.id.as_str()
        );
        assert_eq!(
            result.details.as_ref().unwrap()["tasks"][0]["kind"],
            "delegate"
        );
    }

    #[tokio::test]
    async fn task_list_tool_includes_active_runtime_swarm_without_background_record() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path()).unwrap();
        let swarm_id = ctx.multi_agent.create_swarm_for_test(vec![(
            "calculate a small sum",
            crate::multi_agent::AgentLifecycleState::Running,
        )]);

        let tool = TaskListTool;
        let result = tool.execute(&ctx, json!({})).await.expect("execute");

        assert!(!result.is_error);
        assert!(result.content.contains("active_background_tasks: 1"));
        assert!(result.content.contains(&format!("task_id: {swarm_id}")));
        assert!(result.content.contains("kind: delegate-swarm"));
        assert!(result.content.contains("status: running"));
        assert_eq!(
            result.details.as_ref().unwrap()["tasks"][0]["task_id"],
            swarm_id
        );
        assert_eq!(
            result.details.as_ref().unwrap()["tasks"][0]["kind"],
            "delegate-swarm"
        );
    }

    #[tokio::test]
    async fn task_list_tool_deduplicates_delegate_background_records() {
        let manager = BackgroundTaskManager::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path())
            .unwrap()
            .with_background_tasks(manager.clone());
        let agent = ctx
            .multi_agent
            .start_foreground_delegate_for_test("calculate another small sum");
        manager.start_delegate(agent.clone()).await;

        let tool = TaskListTool;
        let result = tool.execute(&ctx, json!({})).await.expect("execute");
        let tasks = result.details.as_ref().unwrap()["tasks"]
            .as_array()
            .unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task_id"], agent.id.as_str());
        assert_eq!(tasks[0]["kind"], "delegate");
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
    async fn task_output_tool_reads_runtime_delegate_without_background_record() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path()).unwrap();
        let agent = ctx
            .multi_agent
            .start_foreground_delegate_for_test("calculate a small sum");

        let result = TaskOutputTool
            .execute(&ctx, json!({ "task_id": agent.id.as_str() }))
            .await
            .expect("execute");

        assert!(!result.is_error);
        assert!(result.content.contains("agent_id:"));
        assert!(result.content.contains(agent.id.as_str()));
        assert_eq!(result.details.as_ref().unwrap()["kind"], "delegate");
        assert_eq!(
            result.details.as_ref().unwrap()["agent_id"],
            agent.id.as_str()
        );
    }

    #[tokio::test]
    async fn task_output_tool_preserves_runtime_delegate_context_mode() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path()).unwrap();
        let agent = ctx.multi_agent.start_delegate(
            "calculate a small sum",
            None,
            crate::multi_agent::AgentRole::Coder,
            crate::multi_agent::AgentRunMode::Foreground,
            crate::multi_agent::DelegateContext::Summary,
            crate::multi_agent::AgentPathKind::Root,
        );

        let result = TaskOutputTool
            .execute(&ctx, json!({ "task_id": agent.id.as_str() }))
            .await
            .expect("execute");

        assert!(!result.is_error);
        assert!(result.content.contains("context_mode: summary"));
        assert_eq!(result.details.as_ref().unwrap()["context_mode"], "summary");
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
        assert!(result.content.contains("status: cancelled"));
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

    #[tokio::test]
    async fn foreground_bash_task_can_be_detached() {
        let manager = BackgroundTaskManager::new();
        let task_id = manager
            .start_bash_foreground(
                "long command".to_owned(),
                fake_command(None, "", ""),
                1024,
                Duration::from_secs(600),
            )
            .await
            .expect("foreground task id");

        let snapshot = manager.detach(&task_id).await.expect("detach");
        assert_eq!(snapshot.status, BackgroundTaskStatus::Running);
        assert_eq!(snapshot.task_id, task_id);
    }

    #[tokio::test]
    async fn detach_resets_deadline_to_background_timeout() {
        let manager = BackgroundTaskManager::new();
        let task_id = manager
            .start_bash_foreground(
                "long command".to_owned(),
                fake_command(None, "", ""),
                1024,
                Duration::from_millis(80),
            )
            .await
            .expect("foreground task id");

        tokio::time::sleep(Duration::from_millis(30)).await;
        manager.detach(&task_id).await.expect("detach");
        tokio::time::sleep(Duration::from_millis(40)).await;

        let snapshot = manager.snapshot(&task_id).await.expect("snapshot");
        assert_eq!(snapshot.status, BackgroundTaskStatus::Running);
    }

    #[test]
    fn tool_descriptions_are_non_empty() {
        assert!(!TaskListTool.description().is_empty());
        assert!(!TaskOutputTool.description().is_empty());
        assert!(!TaskStopTool.description().is_empty());
    }
}
