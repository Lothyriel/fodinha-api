use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::task::JoinHandle;

#[derive(Clone, Default)]
pub(crate) struct TaskTracker {
    inner: Arc<TaskTrackerInner>,
}

#[derive(Default)]
struct TaskTrackerInner {
    closed: AtomicBool,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl TaskTracker {
    pub(crate) fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);

        if self.inner.closed.load(Ordering::SeqCst) {
            handle.abort();
            return;
        }

        let mut tasks = self.inner.tasks.lock().expect("task tracker lock poisoned");
        tasks.retain(|task| !task.is_finished());

        if self.inner.closed.load(Ordering::SeqCst) {
            handle.abort();
            return;
        }

        tasks.push(handle);
    }

    pub(crate) async fn shutdown(&self, task_timeout: Duration) {
        self.inner.closed.store(true, Ordering::SeqCst);

        for mut handle in self.drain() {
            if tokio::time::timeout(task_timeout, &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    pub(crate) fn abort_all(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);

        for handle in self.drain() {
            handle.abort();
        }
    }

    fn drain(&self) -> Vec<JoinHandle<()>> {
        std::mem::take(&mut *self.inner.tasks.lock().expect("task tracker lock poisoned"))
    }
}
