use std::{collections::HashMap, sync::Arc};

use futures::future::{BoxFuture, join_all};
use tokio::sync::Mutex;

type CleanupFn = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;

struct SupervisedProcess {
    cleanup: CleanupFn,
}

#[derive(Clone, Default)]
pub struct ProcessSupervisor {
    processes: Arc<Mutex<HashMap<String, SupervisedProcess>>>,
}

impl std::fmt::Debug for ProcessSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessSupervisor")
            .finish_non_exhaustive()
    }
}

impl ProcessSupervisor {
    pub async fn register<F>(&self, handle: String, cleanup: F)
    where
        F: Fn(String) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        self.processes.lock().await.insert(
            handle,
            SupervisedProcess {
                cleanup: Arc::new(cleanup),
            },
        );
    }

    pub async fn unregister(&self, handle: &str) {
        self.processes.lock().await.remove(handle);
    }

    pub async fn remove_and_cleanup(&self, handle: &str) {
        let process = self.processes.lock().await.remove(handle);
        if let Some(process) = process {
            (process.cleanup)(handle.to_owned()).await;
        }
    }

    pub async fn active_count(&self) -> usize {
        self.processes.lock().await.len()
    }

    pub async fn cleanup_all(&self) {
        let processes = self
            .processes
            .lock()
            .await
            .drain()
            .collect::<Vec<(String, SupervisedProcess)>>();
        join_all(
            processes
                .into_iter()
                .map(|(handle, process)| (process.cleanup)(handle)),
        )
        .await;
    }

    #[cfg(test)]
    pub(crate) fn register_immediately<F>(&self, handle: String, cleanup: F)
    where
        F: Fn(String) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        self.processes
            .try_lock()
            .expect("supervisor lock available")
            .insert(
                handle,
                SupervisedProcess {
                    cleanup: Arc::new(cleanup),
                },
            );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::FutureExt;

    use super::*;

    #[tokio::test]
    async fn cleanup_all_starts_every_cleanup_before_waiting() {
        let supervisor = ProcessSupervisor::default();
        let started = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());

        for handle in ["one", "two"] {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            supervisor
                .register(handle.to_owned(), move |_| {
                    let started = Arc::clone(&started);
                    let release = Arc::clone(&release);
                    async move {
                        started.fetch_add(1, Ordering::SeqCst);
                        release.notified().await;
                    }
                    .boxed()
                })
                .await;
        }

        let cleanup = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.cleanup_all().await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while started.load(Ordering::SeqCst) != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all cleanups should start concurrently");
        release.notify_waiters();
        cleanup.await.expect("cleanup task should finish");
        assert_eq!(supervisor.active_count().await, 0);
    }
}
