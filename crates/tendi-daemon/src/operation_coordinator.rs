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
mod tests {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use super::*;

    #[test]
    fn execute_can_compose_on_the_same_business_lane() {
        let coordinator = OperationCoordinator::new();
        let nested = coordinator.clone();
        let result = coordinator
            .execute(OperationId::new("outer").unwrap(), move || {
                nested
                    .execute(OperationId::new("inner").unwrap(), || Ok(42))
                    .unwrap()
            })
            .unwrap()
            .unwrap();
        assert_eq!(result, 42);
        coordinator.shutdown();
        assert_eq!(
            coordinator.submit(OperationId::new("closed").unwrap(), || {}),
            Err(SubmitError::WorkerStopped)
        );
    }

    #[test]
    fn worker_can_shutdown_a_full_queue_without_waiting_on_itself() {
        let coordinator = OperationCoordinator::new();
        let worker_coordinator = coordinator.clone();
        let (start_tx, start_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        coordinator
            .submit(OperationId::new("shutdown").unwrap(), move || {
                start_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                worker_coordinator.shutdown();
                assert!(matches!(
                    worker_coordinator
                        .execute(OperationId::new("closed-child").unwrap(), || Ok(())),
                    Err(SubmitError::WorkerStopped)
                ));
                done_tx.send(()).unwrap();
            })
            .unwrap();
        start_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        for id in 0..32 {
            coordinator
                .submit(OperationId::new(format!("queued-{id}")).unwrap(), || {})
                .unwrap();
        }
        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker shutdown cannot block behind its own queued jobs");
    }

    #[test]
    fn jobs_run_in_submission_order() {
        let coordinator = OperationCoordinator::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let (done_tx, done_rx) = mpsc::channel();
        for value in [1, 2, 3] {
            let order = Arc::clone(&order);
            let done_tx = done_tx.clone();
            coordinator
                .submit(
                    OperationId::new(format!("op-{value}")).expect("test operation id is valid"),
                    move || {
                        order.lock().expect("order lock is healthy").push(value);
                        done_tx.send(()).expect("test receiver is alive");
                    },
                )
                .expect("test operation fits in the queue");
        }
        drop(done_tx);
        for _ in 0..3 {
            done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("operation should finish");
        }
        assert_eq!(*order.lock().expect("order lock is healthy"), vec![1, 2, 3]);
    }

    #[test]
    fn worker_shutdown_preserves_handles_for_external_drain() {
        let coordinator = OperationCoordinator::pool("shutdown-test", 2, 4);
        let nested = coordinator.clone();
        let (done_tx, done_rx) = mpsc::channel();
        coordinator
            .submit(OperationId::new("close").unwrap(), move || {
                nested.shutdown();
                assert_eq!(nested.inner.workers.lock().unwrap().len(), 2);
                done_tx.send(()).unwrap();
            })
            .unwrap();
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        coordinator.shutdown();
        assert!(coordinator.inner.workers.lock().unwrap().is_empty());
    }
}
