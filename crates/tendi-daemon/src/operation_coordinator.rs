use std::thread;
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
};

use tendi_core::OperationId;

type OperationJob = Box<dyn FnOnce() + Send + 'static>;

struct QueuedOperation {
    operation_id: OperationId,
    job: OperationJob,
}

struct CoordinatorInner {
    sender: Mutex<Option<SyncSender<QueuedOperation>>>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
    worker_ids: Vec<thread::ThreadId>,
}

impl Drop for CoordinatorInner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl CoordinatorInner {
    fn shutdown(&self) {
        // Closing the sender never waits for queue capacity. Already accepted
        // jobs drain before the receiver exits.
        self.sender.lock().expect("sender lock is healthy").take();
        // A worker must never join a sibling that is synchronously waiting on
        // the current operation. External shutdown joins the complete pool.
        if self.worker_ids.contains(&thread::current().id()) {
            return;
        }
        let workers = std::mem::take(&mut *self.workers.lock().expect("worker lock is healthy"));
        for worker in workers {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    QueueFull,
    WorkerStopped,
}

#[derive(Clone)]
/// A business-operation lane, independent from the database writer queue.
pub struct OperationCoordinator {
    inner: Arc<CoordinatorInner>,
}

impl fmt::Debug for OperationCoordinator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationCoordinator")
            .finish_non_exhaustive()
    }
}

impl OperationCoordinator {
    pub fn new() -> Self {
        Self::named("operations")
    }

    pub fn named(lane: &str) -> Self {
        Self::pool(lane, 1, 32)
    }

    pub fn pool(lane: &str, concurrency: usize, capacity: usize) -> Self {
        assert!(concurrency > 0 && capacity > 0);
        let (sender, receiver) = mpsc::sync_channel::<QueuedOperation>(capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = (0..concurrency)
            .map(|index| {
                let receiver = Arc::clone(&receiver);
                thread::Builder::new()
                    .name(format!("tendi-business-{lane}-{index}"))
                    .spawn(move || {
                        loop {
                            let operation =
                                receiver.lock().expect("receiver lock is healthy").recv();
                            let Ok(operation) = operation else { break };
                            let operation_id = operation.operation_id;
                            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation.job))
                                .is_err()
                            {
                                tendi_core::logging::global().error(
                                    "business operation panicked",
                                    serde_json::json!({"operationId": operation_id}),
                                );
                            }
                        }
                    })
                    .expect("operation coordinator thread must start")
            })
            .collect::<Vec<_>>();
        let worker_ids = workers.iter().map(|worker| worker.thread().id()).collect();
        Self {
            inner: Arc::new(CoordinatorInner {
                sender: Mutex::new(Some(sender)),
                worker_ids,
                workers: Mutex::new(workers),
            }),
        }
    }

    pub fn submit<F>(&self, operation_id: OperationId, job: F) -> Result<(), SubmitError>
    where
        F: FnOnce() + Send + 'static,
    {
        let sender = self.inner.sender.lock().expect("sender lock is healthy");
        let sender = sender.as_ref().ok_or(SubmitError::WorkerStopped)?;
        match sender.try_send(QueuedOperation {
            operation_id,
            job: Box::new(job),
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(SubmitError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(SubmitError::WorkerStopped),
        }
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    pub fn execute<F, T>(
        &self,
        operation_id: OperationId,
        job: F,
    ) -> Result<Result<T, anyhow::Error>, SubmitError>
    where
        F: FnOnce() -> Result<T, anyhow::Error> + Send + 'static,
        T: Send + 'static,
    {
        // Synchronous substeps belong to the currently executing operation,
        // not new FIFO entries. Waiting on this lane would wait on this worker.
        if self.inner.worker_ids.contains(&thread::current().id()) {
            if self
                .inner
                .sender
                .lock()
                .expect("sender lock is healthy")
                .is_none()
            {
                return Err(SubmitError::WorkerStopped);
            }
            return Ok(job());
        }
        let (sender, receiver): (SyncSender<Result<T, anyhow::Error>>, Receiver<_>) =
            mpsc::sync_channel(0);
        self.submit(operation_id, move || {
            let _ = sender.send(job());
        })?;
        receiver.recv().map_err(|_| SubmitError::WorkerStopped)
    }
}

impl Default for OperationCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "operation_coordinator_tests.rs"]
mod tests;
