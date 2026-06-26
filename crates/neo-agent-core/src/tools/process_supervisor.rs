use std::{collections::HashMap, sync::Arc};

use futures::future::BoxFuture;
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
        for (handle, process) in processes {
            (process.cleanup)(handle).await;
        }
    }
}
