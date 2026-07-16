use std::{
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::ResourceLimitDetail;
use crate::session::atomic_file::{AtomicWriteStatus, write_file_atomic_create_new};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GuardStatusKind {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    ResourceLimited,
    ParentExited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GuardExit {
    pub(crate) status: GuardStatusKind,
    pub(crate) exit_code: Option<i32>,
    pub(crate) signal: Option<i32>,
    pub(crate) resource_limit: Option<ResourceLimitDetail>,
    pub(crate) omitted_output_bytes: u64,
    pub(crate) omitted_log_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GuardStatus {
    pub(crate) schema_version: u32,
    pub(crate) task_id: String,
    pub(crate) started_at_ms: u64,
    pub(crate) finished_at_ms: u64,
    pub(crate) exit: GuardExit,
    pub(crate) cleanup_errors: Vec<String>,
}

#[derive(Debug, Error)]
pub(crate) enum StatusWriteError {
    #[error("final status already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("serialize final status: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("write final status: {0}")]
    Io(#[from] io::Error),
}

pub(crate) fn write_final_status(
    path: &Path,
    status: &GuardStatus,
) -> Result<AtomicWriteStatus, StatusWriteError> {
    let content = serde_json::to_vec(status)?;
    match write_file_atomic_create_new(path, &content) {
        Ok(status) => Ok(status),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(StatusWriteError::AlreadyExists(path.to_path_buf()))
        }
        Err(error) => Err(StatusWriteError::Io(error)),
    }
}

pub(crate) struct FinalStatusGuard {
    path: PathBuf,
    task_id: String,
    started_at_ms: u64,
    armed: bool,
}

impl FinalStatusGuard {
    pub(crate) fn after_running_write(
        path: PathBuf,
        task_id: impl Into<String>,
        started_at_ms: u64,
        result: io::Result<AtomicWriteStatus>,
    ) -> io::Result<Self> {
        match result {
            Ok(AtomicWriteStatus::Durable) => Ok(Self::new(path, task_id, started_at_ms)),
            Ok(AtomicWriteStatus::CommittedUnsynced(error)) => {
                drop(Self::new(path, task_id, started_at_ms));
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn new(path: PathBuf, task_id: impl Into<String>, started_at_ms: u64) -> Self {
        Self {
            path,
            task_id: task_id.into(),
            started_at_ms,
            armed: true,
        }
    }

    pub(crate) fn write(
        &mut self,
        status: &GuardStatus,
    ) -> Result<AtomicWriteStatus, StatusWriteError> {
        let result = write_final_status(&self.path, status);
        self.finish_write(result)
    }

    fn finish_write(
        &mut self,
        result: Result<AtomicWriteStatus, StatusWriteError>,
    ) -> Result<AtomicWriteStatus, StatusWriteError> {
        if result.is_ok() {
            self.armed = false;
        }
        result
    }
}

impl Drop for FinalStatusGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = write_final_status(
            &self.path,
            &GuardStatus {
                schema_version: 1,
                task_id: self.task_id.clone(),
                started_at_ms: self.started_at_ms,
                finished_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
                exit: GuardExit {
                    status: GuardStatusKind::Failed,
                    exit_code: None,
                    signal: None,
                    resource_limit: None,
                    omitted_output_bytes: 0,
                    omitted_log_bytes: 0,
                },
                cleanup_errors: vec!["guardian exited before writing final status".to_owned()],
            },
        );
    }
}

#[cfg(test)]
impl GuardStatus {
    fn parent_exited_for_test() -> Self {
        Self::for_test(GuardStatusKind::ParentExited)
    }

    fn completed_for_test() -> Self {
        Self::for_test(GuardStatusKind::Completed)
    }

    fn for_test(status: GuardStatusKind) -> Self {
        Self {
            schema_version: 1,
            task_id: "task-1".to_owned(),
            started_at_ms: 1,
            finished_at_ms: 2,
            exit: GuardExit {
                status,
                exit_code: None,
                signal: None,
                resource_limit: None,
                omitted_output_bytes: 0,
                omitted_log_bytes: 0,
            },
            cleanup_errors: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_final_status_guard_writes_failed_status_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("task.status.json");

        drop(FinalStatusGuard::new(path.clone(), "task-1", 1));

        let status: GuardStatus = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(status.exit.status, GuardStatusKind::Failed);
    }

    #[test]
    fn committed_unsynced_running_status_writes_failed_final_status() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("task.status.json");

        let Err(error) = FinalStatusGuard::after_running_write(
            path.clone(),
            "task-1",
            1,
            Ok(
                crate::session::atomic_file::AtomicWriteStatus::CommittedUnsynced(
                    io::Error::other("sync failed"),
                ),
            ),
        ) else {
            panic!("committed-unsynced running status must return the sync error")
        };

        assert_eq!(error.kind(), io::ErrorKind::Other);
        let status: GuardStatus = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(status.exit.status, GuardStatusKind::Failed);
    }

    #[test]
    fn uncommitted_running_status_does_not_write_final_status() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("task.status.json");

        let Err(error) = FinalStatusGuard::after_running_write(
            path.clone(),
            "task-1",
            1,
            Err(io::Error::other("write failed")),
        ) else {
            panic!("uncommitted running status must return the write error")
        };

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(!path.exists());
    }

    #[test]
    fn committed_unsynced_final_status_disarms_guard() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("task.status.json");
        let mut guard = FinalStatusGuard::new(path.clone(), "task-1", 1);

        let result = guard.finish_write(Ok(AtomicWriteStatus::CommittedUnsynced(
            io::Error::other("sync failed"),
        )));

        assert!(matches!(
            result,
            Ok(AtomicWriteStatus::CommittedUnsynced(_))
        ));
        assert!(!guard.armed);
        drop(guard);
        assert!(!path.exists());
    }

    #[test]
    fn writing_final_status_disarms_guard() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("task.status.json");
        let mut guard = FinalStatusGuard::new(path, "task-1", 1);

        guard.write(&GuardStatus::completed_for_test()).unwrap();

        assert!(!guard.armed);
    }

    #[test]
    fn final_status_is_atomic_and_create_once() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("task.status.json");
        write_final_status(&path, &GuardStatus::parent_exited_for_test()).unwrap();
        assert!(matches!(
            write_final_status(&path, &GuardStatus::completed_for_test()),
            Err(StatusWriteError::AlreadyExists(_))
        ));
    }
}
