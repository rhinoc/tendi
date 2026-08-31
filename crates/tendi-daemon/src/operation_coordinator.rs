use std::{
    fmt,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
};
use std::thread;

use tendi_core::OperationId;

type OperationJob = Box<dyn FnOnce() + Send + 'static>;

struct QueuedOperation {
    operation_id: OperationId,
    job: OperationJob,
}

enum CoordinatorMessage {
    Run(QueuedOperation),
    Shutdown,
}

struct CoordinatorInner {
    sender: SyncSender<CoordinatorMessage>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Drop for CoordinatorInner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl CoordinatorInner {
    fn shutdown(&self) {
        let _ = self.sender.send(CoordinatorMessage::Shutdown);
        if let Some(worker) = self
            .worker
            .lock()
            .expect("worker lock is healthy")
            .take()
        {
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
pub struct OperationCoordinator {
    inner: Arc<CoordinatorInner>,
}

impl fmt::Debug for OperationCoordinator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("OperationCoordinator").finish_non_exhaustive()
    }
}

impl OperationCoordinator {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel::<CoordinatorMessage>(32);
        let worker = thread::Builder::new()
            .name("tendi-operation-coordinator".to_string())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        CoordinatorMessage::Run(operation) => {
                            let _operation_id = operation.operation_id;
                            (operation.job)();
                        }
                        CoordinatorMessage::Shutdown => break,
                    }
                }
            })
            .expect("operation coordinator thread must start");
        Self {
            inner: Arc::new(CoordinatorInner {
                sender,
                worker: Mutex::new(Some(worker)),
            }),
        }
    }

    pub fn submit<F>(&self, operation_id: OperationId, job: F) -> Result<(), SubmitError>
    where
        F: FnOnce() + Send + 'static,
    {
        match self.inner.sender.try_send(CoordinatorMessage::Run(QueuedOperation {
            operation_id,
            job: Box::new(job),
        })) {
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
    fn jobs_run_in_submission_order() {
        let coordinator = OperationCoordinator::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let (done_tx, done_rx) = mpsc::channel();
        for value in [1, 2, 3] {
            let order = Arc::clone(&order);
            let done_tx = done_tx.clone();
            coordinator
                .submit(
                    OperationId::new(format!("op-{value}"))
                        .expect("test operation id is valid"),
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
}
