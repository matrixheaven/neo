mod client;
mod guardian;
mod output;
mod process_tree;
mod protocol;
mod scheduler;
mod status;
mod terminal_guard;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) use client::{
    GuardedCommandResult, GuardianClient, TerminalClientSession, TerminalClientState,
    TerminalSnapshot,
};
pub use guardian::run_process_guard;
pub(crate) use output::TaggedOutput;
pub use scheduler::{
    ShellAdmissionCallback, ShellAdmissionClass, ShellAdmissionEvent, ShellAdmissionRequest,
};
pub(crate) use scheduler::{ShellCommandPermit, ShellScheduler};
pub(crate) use status::{GuardStatus, GuardStatusKind};

/// Removes prior Neo runtime instances only after every running task has a
/// valid create-once final status.
pub fn scavenge_completed_runtime_instances(runtime_dir: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(runtime_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && entry.file_name().to_string_lossy().starts_with("neo-")
            && runtime_instance_is_terminal(&entry.path())?
        {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn runtime_instance_is_terminal(path: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if !runtime_instance_is_terminal(&path)? {
                return Ok(false);
            }
            continue;
        }
        let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
            return Ok(false);
        };
        if let Some(task_id) = name.strip_suffix(".running.json") {
            if !path
                .with_file_name(format!("{task_id}.status.json"))
                .is_file()
            {
                return Ok(false);
            }
        } else if let Some(task_id) = name.strip_suffix(".status.json") {
            let bytes = std::fs::read(&path)?;
            let Ok(status) = serde_json::from_slice::<GuardStatus>(&bytes) else {
                return Ok(false);
            };
            if status.schema_version != 1 || status.task_id != task_id {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLimitCause {
    ProcessCount,
    TreeMemory,
    SamplerUnavailable,
}

impl ResourceLimitCause {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProcessCount => "process_count",
            Self::TreeMemory => "tree_memory",
            Self::SamplerUnavailable => "sampler_unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimitDetail {
    pub cause: ResourceLimitCause,
    pub configured: Option<u64>,
    pub observed: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardLimits {
    pub timeout_ms: Option<u64>,
    pub max_command_parallelism: usize,
    pub max_command_descendant_processes: usize,
    pub max_command_memory_percent: u8,
    pub max_output_bytes: usize,
    pub max_background_log_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellLimits {
    pub max_active_commands: usize,
    pub max_command_parallelism: usize,
    pub max_command_descendant_processes: usize,
    pub max_command_memory_percent: u8,
    pub max_output_bytes: usize,
    pub max_background_log_bytes: u64,
}

impl Default for ShellLimits {
    fn default() -> Self {
        Self {
            max_active_commands: 4,
            max_command_parallelism: 4,
            max_command_descendant_processes: 32,
            max_command_memory_percent: 25,
            max_output_bytes: 65_536,
            max_background_log_bytes: 10_485_760,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{key} {message}")]
pub struct ShellLimitsError {
    key: &'static str,
    message: &'static str,
}

impl ShellLimitsError {
    #[must_use]
    pub const fn key(self) -> &'static str {
        self.key
    }
}

impl ShellLimits {
    pub fn validate(self) -> Result<(), ShellLimitsError> {
        for (key, value) in [(
            "runtime.shell.max_background_log_bytes",
            self.max_background_log_bytes,
        )] {
            if value == 0 {
                return Err(ShellLimitsError {
                    key,
                    message: "must be greater than zero",
                });
            }
        }

        for (key, value) in [
            (
                "runtime.shell.max_active_commands",
                self.max_active_commands,
            ),
            (
                "runtime.shell.max_command_parallelism",
                self.max_command_parallelism,
            ),
            (
                "runtime.shell.max_command_descendant_processes",
                self.max_command_descendant_processes,
            ),
            ("runtime.shell.max_output_bytes", self.max_output_bytes),
        ] {
            if value == 0 {
                return Err(ShellLimitsError {
                    key,
                    message: "must be greater than zero",
                });
            }
        }

        if self.max_command_memory_percent == 0 || self.max_command_memory_percent > 100 {
            return Err(ShellLimitsError {
                key: "runtime.shell.max_command_memory_percent",
                message: "must be between 1 and 100",
            });
        }
        if u64::try_from(self.max_output_bytes).unwrap_or(u64::MAX) > u64::from(u32::MAX) {
            return Err(ShellLimitsError {
                key: "runtime.shell.max_output_bytes",
                message: "must fit the protocol's 32-bit output length",
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn clamp_output_bytes(self, requested: Option<usize>) -> usize {
        requested
            .unwrap_or(self.max_output_bytes)
            .min(self.max_output_bytes)
    }
}

#[must_use]
pub fn format_resource_limit(detail: Option<&ResourceLimitDetail>) -> String {
    match detail {
        Some(ResourceLimitDetail {
            cause: ResourceLimitCause::ProcessCount,
            configured: Some(limit),
            observed: Some(observed),
        }) => format!(
            "Resource limit exceeded: command descendants {observed} > {limit}. \
             Reduce per-command parallelism or raise \
             runtime.shell.max_command_descendant_processes."
        ),
        Some(ResourceLimitDetail {
            cause: ResourceLimitCause::TreeMemory,
            configured: Some(limit),
            observed: Some(observed),
        }) => format!(
            "Resource limit exceeded: command tree memory {observed}% > {limit}%. \
             Reduce the workload or raise runtime.shell.max_command_memory_percent."
        ),
        Some(ResourceLimitDetail {
            cause: ResourceLimitCause::SamplerUnavailable,
            ..
        }) => "Resource monitoring unavailable; the command was stopped because Neo \
               could not enforce shell limits. Check platform process monitoring and retry."
            .to_owned(),
        _ => "Resource limit exceeded.".to_owned(),
    }
}

#[derive(Debug, Clone)]
pub struct ShellRuntime {
    limits: ShellLimits,
    pub(crate) scheduler: Arc<ShellScheduler>,
    guardian_executable: Arc<PathBuf>,
    runtime_root: Arc<PathBuf>,
    terminal_sessions: Arc<tokio::sync::Mutex<HashMap<String, TerminalClientSession>>>,
}

impl Default for ShellRuntime {
    fn default() -> Self {
        Self::new(
            ShellLimits::default(),
            resolve_default_guardian_executable(),
            std::env::temp_dir().join(format!("neo-runtime-{}", uuid::Uuid::new_v4())),
        )
    }
}

fn resolve_default_guardian_executable() -> PathBuf {
    if let Ok(current_exe) = std::env::current_exe()
        && is_named_neo(&current_exe)
    {
        return current_exe;
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let manifest_dir = PathBuf::from(manifest_dir);
        if let Some(workspace_root) = find_workspace_root(&manifest_dir) {
            if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
                let target = PathBuf::from(target_dir);
                let debug = target.join("neo");
                if debug.is_file() {
                    return debug;
                }
                let release = target.join("neo");
                if release.is_file() {
                    return release;
                }
            } else {
                let debug = workspace_root.join("target").join("debug").join("neo");
                if debug.is_file() {
                    return debug;
                }
                let release = workspace_root.join("target").join("release").join("neo");
                if release.is_file() {
                    return release;
                }
            }
        }
    }

    PathBuf::from("neo")
}

fn is_named_neo(path: &Path) -> bool {
    path.file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("neo"))
}

fn find_workspace_root(manifest_dir: &Path) -> Option<PathBuf> {
    manifest_dir.ancestors().find_map(|ancestor| {
        let cargo_toml = ancestor.join("Cargo.toml");
        if !cargo_toml.is_file() {
            return None;
        }
        std::fs::read_to_string(cargo_toml)
            .ok()
            .filter(|content| content.contains("[workspace]"))
            .map(|_| ancestor.to_path_buf())
    })
}

impl ShellRuntime {
    #[must_use]
    pub fn new(limits: ShellLimits, guardian_executable: PathBuf, runtime_root: PathBuf) -> Self {
        Self {
            limits,
            scheduler: ShellScheduler::new(limits.max_active_commands),
            guardian_executable: Arc::new(guardian_executable),
            runtime_root: Arc::new(runtime_root),
            terminal_sessions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(limits: ShellLimits) -> Self {
        Self::new(limits, PathBuf::from("neo"), PathBuf::from("runtime"))
    }

    pub(crate) async fn acquire(
        &self,
        request: ShellAdmissionRequest,
        callback: Option<ShellAdmissionCallback>,
    ) -> ShellCommandPermit {
        self.scheduler.acquire(request, callback).await
    }

    #[must_use]
    pub const fn limits(&self) -> ShellLimits {
        self.limits
    }

    #[must_use]
    pub fn guardian_executable(&self) -> &Path {
        &self.guardian_executable
    }

    #[must_use]
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    #[must_use]
    pub fn guard_limits(&self, timeout: Option<Duration>, max_output_bytes: usize) -> GuardLimits {
        GuardLimits {
            timeout_ms: timeout
                .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
            max_command_parallelism: self.limits.max_command_parallelism,
            max_command_descendant_processes: self.limits.max_command_descendant_processes,
            max_command_memory_percent: self.limits.max_command_memory_percent,
            max_output_bytes: max_output_bytes.min(self.limits.max_output_bytes),
            max_background_log_bytes: self.limits.max_background_log_bytes,
        }
    }

    pub(crate) async fn insert_terminal(&self, handle: String, session: TerminalClientSession) {
        self.terminal_sessions.lock().await.insert(handle, session);
    }

    pub(crate) async fn terminal(&self, handle: &str) -> Option<TerminalClientSession> {
        self.terminal_sessions.lock().await.get(handle).cloned()
    }

    pub(crate) async fn remove_terminal(&self, handle: &str) -> Option<TerminalClientSession> {
        self.terminal_sessions.lock().await.remove(handle)
    }
}

fn command_deadline(timeout_ms: Option<u64>) -> Option<Pin<Box<tokio::time::Sleep>>> {
    timeout_ms.map(|millis| Box::pin(tokio::time::sleep(Duration::from_millis(millis))))
}

async fn wait_for_deadline(deadline: &mut Option<Pin<Box<tokio::time::Sleep>>>) {
    match deadline {
        Some(deadline) => deadline.as_mut().await,
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_output_may_exceed_one_protocol_frame() {
        let limits = ShellLimits {
            max_output_bytes: protocol::MAX_FRAME_BODY + 1,
            ..ShellLimits::default()
        };

        assert!(limits.validate().is_ok());
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn max_output_must_fit_protocol_u32_length() {
        let limits = ShellLimits {
            max_output_bytes: usize::try_from(u32::MAX).unwrap() + 1,
            ..ShellLimits::default()
        };

        let error = limits
            .validate()
            .expect_err("oversized output limit was accepted");

        assert_eq!(error.key(), "runtime.shell.max_output_bytes");
    }

    #[test]
    fn runtime_scavenger_keeps_live_instances_and_removes_completed_ones() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let done_tasks = runtime.join("neo-done/agents/main/tasks");
        let live_tasks = runtime.join("neo-live/agents/main/tasks");
        std::fs::create_dir_all(&done_tasks).unwrap();
        std::fs::create_dir_all(&live_tasks).unwrap();
        std::fs::write(
            done_tasks.join("task.status.json"),
            serde_json::to_vec(&GuardStatus {
                schema_version: 1,
                task_id: "task".to_owned(),
                started_at_ms: 1,
                finished_at_ms: 2,
                exit: status::GuardExit {
                    status: GuardStatusKind::Completed,
                    exit_code: Some(0),
                    signal: None,
                    resource_limit: None,
                    omitted_output_bytes: 0,
                    omitted_log_bytes: 0,
                },
                cleanup_errors: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(live_tasks.join("task.running.json"), b"{}").unwrap();

        scavenge_completed_runtime_instances(&runtime).unwrap();

        assert!(!runtime.join("neo-done").exists());
        assert!(runtime.join("neo-live").exists());
    }

    #[test]
    fn limits_are_direct_per_command_budgets() {
        let limits = ShellLimits {
            max_active_commands: 8,
            max_command_descendant_processes: 32,
            max_command_memory_percent: 25,
            ..ShellLimits::default()
        };
        let runtime = ShellRuntime::for_tests(limits);
        let guard = runtime.guard_limits(Some(Duration::from_secs(60)), limits.max_output_bytes);
        assert_eq!(guard.max_command_descendant_processes, 32);
        assert_eq!(guard.max_command_memory_percent, 25);
        assert_eq!(guard.timeout_ms, Some(60_000));
    }

    }
