use crate::operation_coordinator::OperationCoordinator;
use serde_json::Value;
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tendi_core::{
    OperationId,
    coordination::{ResourceRequest, ResourceReservation},
};

const PENDING_LIMIT: usize = 128;
const ADMISSION_DEADLINE: Duration = Duration::from_secs(30);
const CONCURRENCY: [usize; 4] = [2, 4, 2, 2];
// Higher-priority requests may pass older conflicting lower-priority
// requests, but only a bounded number of times. This is scheduler policy,
// not a timeout: after the debt is paid, FIFO order is restored.
const MAX_PRIORITY_OVERTAKES: usize = 8;
thread_local! { static IN_SCHEDULER_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

fn trace_skill_operation(operation: &OperationId) -> bool {
    operation.as_str().starts_with("rpc-skills_update")
}

fn trace_maintenance_operation(operation: &OperationId) -> bool {
    operation.as_str().starts_with("skills-reconcile")
        || operation.as_str().starts_with("projection-refresh-skills")
}

fn trace_runtime_stage(operation: &OperationId, workload: Workload) -> bool {
    operation.as_str().starts_with("rpc-")
        && matches!(workload, Workload::ExternalIo | Workload::Compute)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Workload {
    Interactive,
    ExternalIo,
    Compute,
    Prepare,
}
impl Workload {
    fn index(self) -> usize {
        match self {
            Self::Interactive => 0,
            Self::ExternalIo => 1,
            Self::Compute => 2,
            Self::Prepare => 3,
        }
    }

    // Lower values have higher admission priority. Callers only classify a
    // stage; they do not need to know which command is maintenance work.
    fn priority(self) -> usize {
        self.index()
    }
}

/// Only admission is retried. Completed preparation and filesystem mutations
/// are never replayed to acquire resources; each continuation is consumed once.
pub(crate) enum Step<T> {
    Complete(T),
    Acquire {
        workload: Workload,
        resources: Vec<ResourceRequest>,
        run: Box<dyn FnOnce() -> anyhow::Result<Step<T>> + Send>,
    },
}
impl<T> Step<T> {
    pub fn acquire(
        workload: Workload,
        resources: Vec<ResourceRequest>,
        run: impl FnOnce() -> anyhow::Result<Self> + Send + 'static,
    ) -> Self {
        Self::Acquire {
            workload,
            resources,
            run: Box::new(run),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ScheduleError {
    QueueFull,
    Closed,
    Cancelled,
    Deadline,
    Failed(String),
}
impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull => write!(f, "request admission queue is full"),
            Self::Closed => write!(f, "request scheduler is closed"),
            Self::Cancelled => write!(f, "request cancelled before its next stage started"),
            Self::Deadline => write!(f, "request resource admission deadline exceeded"),
            Self::Failed(error) => write!(f, "{error}"),
        }
    }
}

type Task = Box<dyn FnOnce() -> anyhow::Result<Option<Stage>> + Send>;
struct Stage {
    workload: Workload,
    resources: Vec<ResourceRequest>,
    run: Task,
}
struct Request {
    operation: OperationId,
    submitted_at: Instant,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    stage: Stage,
    priority_overtakes: usize,
    reject: Box<dyn FnOnce(ScheduleError) + Send>,
    _permit: PendingPermit,
}
impl Request {
    fn reject(self, error: ScheduleError) {
        let Self {
            operation,
            submitted_at,
            stage,
            reject,
            ..
        } = self;
        if trace_skill_operation(&operation) || trace_maintenance_operation(&operation) {
            tendi_core::logging::global().warn(
                "runtime request admission rejected",
                serde_json::json!({
                    "operationId": operation.as_str(),
                    "queueWaitMs": submitted_at.elapsed().as_secs_f64() * 1000.0,
                    "error": error.to_string(),
                }),
            );
        }
        // Terminal observers must see task-owned guards already released.
        drop(stage);
        reject(error);
    }
}
struct PendingPermit(Arc<AtomicUsize>);
impl Drop for PendingPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
enum Message {
    Submit(Request),
    Finished(Workload, Option<Request>),
    Wake,
}

struct Inner {
    sender: mpsc::Sender<Message>,
    pending: [Arc<AtomicUsize>; 4],
    closed: Arc<AtomicBool>,
    pump: Mutex<Option<thread::JoinHandle<()>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl Inner {
    fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        let _ = self.sender.send(Message::Wake);
        if IN_SCHEDULER_WORKER.with(|inside| inside.get()) {
            return;
        }
        if let Some(pump) = self.pump.lock().expect("admission pump lock").take() {
            if pump.thread().id() != thread::current().id() {
                let _ = pump.join();
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct RequestScheduler {
    inner: Arc<Inner>,
}
impl fmt::Debug for RequestScheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestScheduler").finish_non_exhaustive()
    }
}

fn erase<T: Send + 'static>(
    step: Step<T>,
    sender: mpsc::Sender<Result<T, ScheduleError>>,
) -> Option<Stage> {
    match step {
        Step::Complete(value) => {
            let _ = sender.send(Ok(value));
            None
        }
        Step::Acquire {
            workload,
            resources,
            run,
        } => Some(Stage {
            workload,
            resources,
            run: Box::new(move || run().map(|step| erase(step, sender))),
        }),
    }
}

impl Default for RequestScheduler {
    fn default() -> Self {
        Self::with_concurrency(CONCURRENCY)
    }
}

impl RequestScheduler {
    fn with_concurrency(concurrency: [usize; 4]) -> Self {
        assert!(concurrency.iter().all(|count| *count > 0));
        let (sender, receiver) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let pump_sender = sender.clone();
        let pump_closed = Arc::clone(&closed);
        let pump = thread::Builder::new()
            .name("tendi-resource-admission".into())
            .spawn(move || admission_loop(receiver, pump_sender, pump_closed, concurrency))
            .expect("resource admission pump must start");
        Self {
            inner: Arc::new(Inner {
                sender,
                closed,
                pending: std::array::from_fn(|_| Arc::new(AtomicUsize::new(0))),
                pump: Mutex::new(Some(pump)),
            }),
        }
    }
}
impl RequestScheduler {
    pub fn submit<T: Send + 'static>(
        &self,
        operation: OperationId,
        step: Step<T>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<mpsc::Receiver<Result<T, ScheduleError>>, ScheduleError> {
        self.submit_until(
            operation,
            step,
            cancelled,
            Instant::now() + ADMISSION_DEADLINE,
        )
    }
    fn submit_until<T: Send + 'static>(
        &self,
        operation: OperationId,
        step: Step<T>,
        cancelled: Arc<AtomicBool>,
        deadline: Instant,
    ) -> Result<mpsc::Receiver<Result<T, ScheduleError>>, ScheduleError> {
        let class = match &step {
            Step::Acquire { workload, .. } => *workload,
            Step::Complete(_) => Workload::Interactive,
        };
        self.submit_class_until(class, operation, step, cancelled, deadline)
    }

    fn submit_class_until<T: Send + 'static>(
        &self,
        class: Workload,
        operation: OperationId,
        step: Step<T>,
        cancelled: Arc<AtomicBool>,
        deadline: Instant,
    ) -> Result<mpsc::Receiver<Result<T, ScheduleError>>, ScheduleError> {
        self.submit_with_rejection_until(class, operation, step, cancelled, deadline, |_| {})
    }

    pub fn submit_with_rejection<T: Send + 'static>(
        &self,
        operation: OperationId,
        step: Step<T>,
        cancelled: Arc<AtomicBool>,
        rejected: impl FnOnce(ScheduleError) + Send + 'static,
    ) -> Result<mpsc::Receiver<Result<T, ScheduleError>>, ScheduleError> {
        let class = match &step {
            Step::Acquire { workload, .. } => *workload,
            Step::Complete(_) => Workload::Interactive,
        };
        self.submit_with_rejection_until(
            class,
            operation,
            step,
            cancelled,
            Instant::now() + ADMISSION_DEADLINE,
            rejected,
        )
    }

    fn submit_with_rejection_until<T: Send + 'static>(
        &self,
        class: Workload,
        operation: OperationId,
        step: Step<T>,
        cancelled: Arc<AtomicBool>,
        deadline: Instant,
        rejected: impl FnOnce(ScheduleError) + Send + 'static,
    ) -> Result<mpsc::Receiver<Result<T, ScheduleError>>, ScheduleError> {
        if self.inner.closed.load(Ordering::Acquire) {
            drop(step);
            rejected(ScheduleError::Closed);
            return Err(ScheduleError::Closed);
        }
        let pending = &self.inner.pending[class.index()];
        if pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < PENDING_LIMIT).then_some(count + 1)
            })
            .is_err()
        {
            drop(step);
            rejected(ScheduleError::QueueFull);
            return Err(ScheduleError::QueueFull);
        }
        let permit = PendingPermit(Arc::clone(pending));
        let (sender, receiver) = mpsc::channel();
        if let Some(stage) = erase(step, sender.clone()) {
            let request = Request {
                operation,
                submitted_at: Instant::now(),
                deadline,
                cancelled,
                stage,
                priority_overtakes: 0,
                reject: Box::new(move |error| {
                    let _ = sender.send(Err(error.clone()));
                    rejected(error);
                }),
                _permit: permit,
            };
            if let Err(error) = self.inner.sender.send(Message::Submit(request)) {
                if let Message::Submit(request) = error.0 {
                    request.reject(ScheduleError::Closed);
                }
                return Err(ScheduleError::Closed);
            }
        }
        Ok(receiver)
    }
    pub fn execute_class<T: Send + 'static>(
        &self,
        class: Workload,
        operation: OperationId,
        step: Step<T>,
    ) -> Result<T, ScheduleError> {
        if IN_SCHEDULER_WORKER.with(|inside| inside.get()) {
            return Err(ScheduleError::Failed(
                "nested synchronous request scheduling is not allowed; return a continuation"
                    .into(),
            ));
        }
        self.submit_class_until(
            class,
            operation,
            step,
            Arc::new(AtomicBool::new(false)),
            Instant::now() + ADMISSION_DEADLINE,
        )?
        .recv()
        .map_err(|_| ScheduleError::Closed)?
    }
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

fn admission_loop(
    receiver: mpsc::Receiver<Message>,
    sender: mpsc::Sender<Message>,
    closed: Arc<AtomicBool>,
    concurrency: [usize; 4],
) {
    let pools = [
        OperationCoordinator::pool("interactive", concurrency[0], concurrency[0]),
        OperationCoordinator::pool("external-io", concurrency[1], concurrency[1]),
        OperationCoordinator::pool("compute", concurrency[2], concurrency[2]),
        OperationCoordinator::pool("prepare", concurrency[3], concurrency[3]),
    ];
    let mut active = [0usize; 4];
    let mut pending = Vec::<Request>::new();
    loop {
        match receiver.recv_timeout(Duration::from_millis(5)) {
            Ok(Message::Submit(request)) => pending.push(request),
            Ok(Message::Finished(workload, next)) => {
                active[workload.index()] -= 1;
                if let Some(next) = next {
                    pending.push(next);
                }
            }
            Ok(Message::Wake) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let mut index = 0;
        while index < pending.len() {
            let reject = if closed.load(Ordering::Acquire) {
                Some(ScheduleError::Closed)
            } else if pending[index].cancelled.load(Ordering::Acquire) {
                Some(ScheduleError::Cancelled)
            } else if Instant::now() >= pending[index].deadline {
                Some(ScheduleError::Deadline)
            } else {
                None
            };
            if let Some(error) = reject {
                let request = pending.remove(index);
                request.reject(error);
                continue;
            }
            let workload = pending[index].stage.workload;
            if active[workload.index()] >= concurrency[workload.index()] {
                index += 1;
                continue;
            }
            // Preserve FIFO for requests at the same or higher priority. A
            // lower-priority request can be passed while it has not
            // accumulated its bounded fairness debt. Resource admission is
            // still attempted below for every selected request, so this
            // policy never weakens the shared-resource mutex.
            let lower_priority_conflicts = (|| -> anyhow::Result<Option<Vec<usize>>> {
                let candidate_priority = workload.priority();
                let mut lower_priority_conflicts = Vec::new();
                for (older_index, older) in pending[..index].iter().enumerate() {
                    let conflicts = 'resources: {
                        for a in &older.stage.resources {
                            for b in &pending[index].stage.resources {
                                if a.conflicts_with(b)? {
                                    break 'resources true;
                                }
                            }
                        }
                        false
                    };
                    if conflicts {
                        if older.stage.workload.priority() <= candidate_priority
                            || older.priority_overtakes >= MAX_PRIORITY_OVERTAKES
                        {
                            return Ok(None);
                        }
                        lower_priority_conflicts.push(older_index);
                    }
                }
                Ok(Some(lower_priority_conflicts))
            })();
            let lower_priority_conflicts = match lower_priority_conflicts {
                Ok(Some(conflicts)) => conflicts,
                Ok(None) => {
                    index += 1;
                    continue;
                }
                Err(error) => {
                    let request = pending.remove(index);
                    request.reject(ScheduleError::Failed(error.to_string()));
                    continue;
                }
            };
            let reservation =
                match ResourceReservation::try_acquire(&pending[index].stage.resources) {
                    Ok(Some(reservation)) => reservation,
                    Ok(None) => {
                        index += 1;
                        continue;
                    }
                    Err(error) => {
                        let request = pending.remove(index);
                        request.reject(ScheduleError::Failed(error.to_string()));
                        continue;
                    }
                };
            for older_index in lower_priority_conflicts {
                pending[older_index].priority_overtakes += 1;
            }
            let request = pending.remove(index);
            active[workload.index()] += 1;
            let finish = sender.clone();
            let operation = request.operation.clone();
            let held_request = Arc::new(Mutex::new(Some(request)));
            let for_job = Arc::clone(&held_request);
            let submitted = pools[workload.index()].submit(operation, move || {
                let Request {
                    operation,
                    submitted_at,
                    deadline,
                    cancelled,
                    stage,
                    priority_overtakes: _priority_overtakes,
                    reject,
                    _permit,
                } = for_job
                    .lock()
                    .expect("stage transfer lock")
                    .take()
                    .expect("stage consumed once");
                let workload = stage.workload;
                let queue_wait_ms = submitted_at.elapsed().as_secs_f64() * 1000.0;
                let operation_id = operation.as_str().to_string();
                let trace_stage = trace_skill_operation(&operation)
                    || trace_maintenance_operation(&operation)
                    || trace_runtime_stage(&operation, workload);
                if trace_stage {
                    tendi_core::logging::global().info(
                        "runtime request admitted",
                        serde_json::json!({
                            "operationId": operation_id.clone(),
                            "workload": format!("{workload:?}"),
                            "queueWaitMs": queue_wait_ms,
                        }),
                    );
                }
                IN_SCHEDULER_WORKER.with(|inside| inside.set(true));
                if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                    drop(stage);
                    reject(if cancelled.load(Ordering::Acquire) {
                        ScheduleError::Cancelled
                    } else {
                        ScheduleError::Deadline
                    });
                    drop(reservation);
                    let _ = finish.send(Message::Finished(workload, None));
                    IN_SCHEDULER_WORKER.with(|inside| inside.set(false));
                    return;
                }
                let resources = reservation.enter();
                let execution_started = Instant::now();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(stage.run));
                drop(resources);
                let execution_ms = execution_started.elapsed().as_secs_f64() * 1000.0;
                let next = match result {
                    Ok(Ok(Some(stage))) => Some(Request {
                        operation,
                        submitted_at,
                        deadline: deadline + execution_started.elapsed(),
                        cancelled,
                        stage,
                        priority_overtakes: 0,
                        reject,
                        _permit,
                    }),
                    Ok(Ok(None)) => None,
                    Ok(Err(error)) => {
                        reject(ScheduleError::Failed(format!("{error:#}")));
                        None
                    }
                    Err(_) => {
                        reject(ScheduleError::Failed("request stage panicked".into()));
                        None
                    }
                };
                if trace_stage {
                    tendi_core::logging::global().info(
                        "runtime request stage completed",
                        serde_json::json!({
                            "operationId": operation_id,
                            "workload": format!("{workload:?}"),
                            "queueWaitMs": queue_wait_ms,
                            "executionMs": execution_ms,
                            "continued": next.is_some(),
                        }),
                    );
                }
                let _ = finish.send(Message::Finished(workload, next));
                IN_SCHEDULER_WORKER.with(|inside| inside.set(false));
            });
            if let Err(error) = submitted {
                active[workload.index()] -= 1;
                if let Some(request) = held_request.lock().expect("stage transfer lock").take() {
                    request.reject(ScheduleError::Failed(format!(
                        "execution stage admission failed: {error:?}"
                    )));
                }
            }
        }
        if closed.load(Ordering::Acquire)
            && pending.is_empty()
            && active.iter().all(|count| *count == 0)
        {
            break;
        }
    }
    for pool in pools {
        pool.shutdown();
    }
}

pub(crate) fn workload(method: &str) -> Option<Workload> {
    match method {
        "skills_updates_cancel" => None,
        "mcp_probe"
        | "skills_add"
        | "skills_update"
        | "skills_update_many"
        | "skills_marketplace_search"
        | "skills_backup_configure"
        | "skills_backup_now"
        | "skills_backup_restore"
        | "skills_backup_sync"
        | "skills_backup_versions" => Some(Workload::ExternalIo),
        "session_transcript" | "session_transcript_locator" | "session_transcript_search" => {
            Some(Workload::Interactive)
        }
        "scan"
        | "projects_scan"
        | "skills_refresh"
        | "skills_updates"
        | "sessions_search"
        | "skills_distribute"
        | "skills_remove_locations"
        | "skills_delete_many"
        | "skills_wrap"
        | "skills_set" => Some(Workload::Compute),
        _ if tendi_core::generated::runtime_contract::command_requires_serialized_write(
            method,
            &serde_json::json!({}),
        ) =>
        {
            Some(Workload::Interactive)
        }
        _ => None,
    }
}

pub(crate) fn workload_for_request(method: &str, params: &Value) -> Option<Workload> {
    if method == "session_skill_index_run" {
        return Some(Workload::Compute);
    }
    if method == "skills_updates" && params.get("check").and_then(Value::as_bool) == Some(true) {
        // This RPC only starts the already-scheduled background check. The
        // control-plane response must not wait for an interactive worker.
        return None;
    }
    workload(method)
}

#[cfg(test)]
#[path = "request_scheduler_tests.rs"]
mod tests;
