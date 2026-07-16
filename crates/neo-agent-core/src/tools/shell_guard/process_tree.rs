use std::io;

#[cfg(unix)]
use std::{thread, time::Duration};

use portable_pty::Child;

#[cfg(windows)]
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
};

pub(super) struct TerminalProcessTree {
    child: Box<dyn Child + Send + Sync>,
    #[cfg(unix)]
    group: Option<rustix::process::Pid>,
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

impl Drop for TerminalProcessTree {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group {
            let killed = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
            if killed.is_ok() {
                let _ = self.child.wait();
            } else {
                let _ = self.child.try_wait();
            }
        } else {
            let killed = self.child.kill();
            if killed.is_ok() {
                let _ = self.child.wait();
            } else {
                let _ = self.child.try_wait();
            }
        }

        #[cfg(windows)]
        drop(self.job.take());

        #[cfg(not(any(unix, windows)))]
        {
            let _ = self.child.kill();
        }
    }
}

impl TerminalProcessTree {
    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn new(child: Box<dyn Child + Send + Sync>) -> io::Result<Self> {
        #[cfg(windows)]
        let mut child = child;
        #[cfg(unix)]
        let group = child
            .process_id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw);

        #[cfg(windows)]
        let job = {
            let mut limits = win32job::ExtendedLimitInfo::new();
            limits.limit_kill_on_job_close();
            let job = win32job::Job::create_with_limit_info(&limits)
                .map_err(|error| io::Error::other(format!("create terminal Job Object: {error}")));
            let job = match job {
                Ok(job) => job,
                Err(error) => return Err(clean_up_unowned_child(&mut child, error)),
            };
            let handle = match child.as_raw_handle() {
                Some(handle) => handle,
                None => {
                    return Err(clean_up_unowned_child(
                        &mut child,
                        io::Error::new(
                            io::ErrorKind::Unsupported,
                            "terminal child has no process handle",
                        ),
                    ));
                }
            };
            if let Err(error) = job.assign_process(handle as isize) {
                return Err(clean_up_unowned_child(
                    &mut child,
                    io::Error::other(format!("assign terminal Job Object: {error}")),
                ));
            }
            Some(job)
        };

        Ok(Self {
            child,
            #[cfg(unix)]
            group,
            #[cfg(windows)]
            job,
        })
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<i32>> {
        self.child.try_wait().map(|status| status.map(exit_code))
    }

    pub(super) fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub(super) fn terminate_and_wait(&mut self) -> io::Result<Option<i32>> {
        #[cfg(unix)]
        {
            let Some(group) = self.group else {
                return Err(clean_up_direct_child_with_limitation(
                    &mut self.child,
                    "terminal child has no process group for tree cleanup",
                ));
            };
            terminate_process_group(group, rustix::process::Signal::TERM)?;
            thread::sleep(Duration::from_millis(500));
            terminate_process_group(group, rustix::process::Signal::KILL)?;
            let status = self.child.wait()?;
            self.group = None;
            Ok(Some(exit_code(status)))
        }

        #[cfg(windows)]
        {
            drop(self.job.take());
            self.child.wait().map(|status| Some(exit_code(status)))
        }

        #[cfg(not(any(unix, windows)))]
        {
            Err(clean_up_direct_child_with_limitation(
                &mut self.child,
                "terminal process-tree cleanup is unavailable on this platform",
            ))
        }
    }
}

#[allow(clippy::needless_pass_by_value)]
fn exit_code(status: portable_pty::ExitStatus) -> i32 {
    i32::try_from(status.exit_code()).unwrap_or(i32::MAX)
}

#[cfg(not(windows))]
fn clean_up_direct_child_with_limitation(
    child: &mut Box<dyn Child + Send + Sync>,
    limitation: &'static str,
) -> io::Error {
    let kill = child.kill();
    let wait = if kill.is_ok() {
        child.wait().map(|_| ())
    } else {
        child.try_wait().map(|_| ())
    };
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{limitation}; direct-child cleanup attempted (kill: {kill:?}, wait: {wait:?})"),
    )
}

#[cfg(windows)]
fn clean_up_unowned_child(child: &mut Box<dyn Child + Send + Sync>, error: io::Error) -> io::Error {
    let _ = child.kill();
    let _ = child.wait();
    error
}

#[cfg(unix)]
fn terminate_process_group(
    group: rustix::process::Pid,
    signal: rustix::process::Signal,
) -> io::Result<()> {
    match rustix::process::kill_process_group(group, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        #[cfg(target_os = "macos")]
        Err(rustix::io::Errno::PERM) if signal == rustix::process::Signal::KILL => Ok(()),
        Err(error) => Err(io::Error::other(format!(
            "signal terminal process group with {signal:?}: {error}"
        ))),
    }
}

#[cfg(windows)]
pub(super) struct WindowsLaunchBarrier {
    path: PathBuf,
}

#[cfg(windows)]
impl WindowsLaunchBarrier {
    pub(super) fn new(runtime_dir: &Path) -> Self {
        Self {
            path: runtime_dir.join(format!(".neo-process-ready-{}", uuid::Uuid::new_v4())),
        }
    }

    pub(super) fn wait_command(&self) -> String {
        let command = format!(
            "if exist \"{}\" exit /b 0 else exit /b 1",
            self.path.display()
        );
        format!(
            "until cmd.exe //d //c {}; do sleep 0.01; done;",
            quote_posix_shell(&command)
        )
    }

    pub(super) fn release(&self) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .map(|_| ())
    }
}

#[cfg(windows)]
impl Drop for WindowsLaunchBarrier {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
fn quote_posix_shell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn launch_barrier_disables_msys_switch_path_conversion() {
        let temp = tempfile::tempdir().unwrap();
        let barrier = WindowsLaunchBarrier::new(temp.path());

        assert!(barrier.wait_command().contains("cmd.exe //d //c"));
    }
}
