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
    if method == "skills_updates" && params.get("check").and_then(Value::as_bool) == Some(true) {
        // This RPC only starts the already-scheduled background check. The
        // control-plane response must not wait for an interactive worker.
        return None;
    }
    workload(method)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};
    use tendi_core::coordination::ResourceLease;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "tendi-admission-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn resource(&self, path: &str) -> ResourceRequest {
            ResourceRequest::Paths {
                namespace: self.0.join("authority"),
                paths: vec![self.0.join(path)],
            }
        }
        fn hold(&self, path: &str) -> ResourceLease {
            ResourceLease::acquire_paths(&self.0.join("authority"), &[self.0.join(path)]).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn operation() -> OperationId {
        OperationId::new("admission-regression").unwrap()
    }

    #[test]
    fn remote_skill_update_check_bypasses_local_compute_queue() {
        assert_eq!(
            workload_for_request("skills_updates", &serde_json::json!({ "check": true })),
            None
        );
        assert_eq!(
            workload_for_request("skills_updates", &serde_json::json!({})),
            Some(Workload::Compute)
        );
    }
    fn cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn blocked_resources_do_not_occupy_execution_workers_and_side_effects_run_once() {
        let fixture = Fixture::new();
        let scheduler = RequestScheduler::with_concurrency([1; 4]);
        let held = fixture.hold("busy");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut responses = Vec::new();
        for _ in 0..16 {
            let calls = Arc::clone(&calls);
            responses.push(
                scheduler
                    .submit(
                        operation(),
                        Step::acquire(
                            Workload::Compute,
                            vec![fixture.resource("busy")],
                            move || {
                                assert_eq!(calls.fetch_add(1, Ordering::AcqRel), 0);
                                calls.fetch_sub(1, Ordering::AcqRel);
                                Ok(Step::Complete(()))
                            },
                        ),
                        cancel(),
                    )
                    .unwrap(),
            );
        }
        let independent = scheduler
            .submit(
                operation(),
                Step::acquire(
                    Workload::Compute,
                    vec![fixture.resource("independent")],
                    || Ok(Step::Complete(17)),
                ),
                cancel(),
            )
            .unwrap();
        assert_eq!(
            independent
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            17
        );
        assert!(
            responses
                .iter()
                .all(|response| response.try_recv().is_err())
        );
        drop(held);
        for response in responses {
            response
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap();
        }
        scheduler.shutdown();
    }

    #[test]
    fn io_pending_quota_cannot_reject_interactive_requests() {
        let fixture = Fixture::new();
        let scheduler = RequestScheduler::default();
        let _held = fixture.hold("busy");
        let token = cancel();
        let mut responses = Vec::new();
        for _ in 0..PENDING_LIMIT {
            responses.push(
                scheduler
                    .submit(
                        operation(),
                        Step::acquire(Workload::ExternalIo, vec![fixture.resource("busy")], || {
                            Ok(Step::Complete(()))
                        }),
                        Arc::clone(&token),
                    )
                    .unwrap(),
            );
        }
        assert!(matches!(
            scheduler.submit(
                operation(),
                Step::acquire(Workload::ExternalIo, vec![], || Ok(Step::Complete(()))),
                cancel()
            ),
            Err(ScheduleError::QueueFull)
        ));
        assert_eq!(
            scheduler
                .execute_class(
                    Workload::Interactive,
                    operation(),
                    Step::acquire(Workload::Prepare, vec![], || Ok(Step::acquire(
                        Workload::Interactive,
                        vec![],
                        || Ok(Step::Complete(42))
                    )))
                )
                .unwrap(),
            42
        );
        token.store(true, Ordering::Release);
        for response in responses {
            assert!(matches!(
                response.recv_timeout(Duration::from_secs(3)).unwrap(),
                Err(ScheduleError::Cancelled)
            ));
        }
        scheduler.shutdown();
    }

    #[test]
    fn older_parent_intent_prevents_child_starvation_and_allows_unrelated_work() {
        let fixture = Fixture::new();
        let scheduler = RequestScheduler::default();
        let held = fixture.hold("tree/a");
        let (order, received) = mpsc::channel();
        let first = order.clone();
        let parent = scheduler
            .submit(
                operation(),
                Step::acquire(
                    Workload::Compute,
                    vec![fixture.resource("tree")],
                    move || {
                        first.send("parent").unwrap();
                        Ok(Step::Complete(()))
                    },
                ),
                cancel(),
            )
            .unwrap();
        let child = scheduler
            .submit(
                operation(),
                Step::acquire(
                    Workload::Compute,
                    vec![fixture.resource("tree/b")],
                    move || {
                        order.send("child").unwrap();
                        Ok(Step::Complete(()))
                    },
                ),
                cancel(),
            )
            .unwrap();
        let unrelated = scheduler
            .submit(
                operation(),
                Step::acquire(Workload::Compute, vec![fixture.resource("other")], || {
                    Ok(Step::Complete(()))
                }),
                cancel(),
            )
            .unwrap();
        unrelated
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(received.try_recv().is_err());
        drop(held);
        parent
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        child.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!(received.recv().unwrap(), "parent");
        assert_eq!(received.recv().unwrap(), "child");
        scheduler.shutdown();
    }

    #[test]
    fn higher_priority_work_can_pass_blocked_maintenance_without_sharing_its_resource() {
        let fixture = Fixture::new();
        let scheduler = RequestScheduler::with_concurrency([1; 4]);
        let (compute_started_tx, compute_started_rx) = mpsc::channel();
        let (compute_release_tx, compute_release_rx) = mpsc::channel();
        let blocker = scheduler
            .submit(
                operation(),
                Step::acquire(Workload::Compute, vec![], move || {
                    compute_started_tx.send(()).unwrap();
                    compute_release_rx.recv().unwrap();
                    Ok(Step::Complete(()))
                }),
                cancel(),
            )
            .unwrap();
        compute_started_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();

        let (maintenance_started, maintenance_seen) = mpsc::channel();
        let maintenance = scheduler
            .submit(
                OperationId::new("skills-reconcile").unwrap(),
                Step::acquire(
                    Workload::Compute,
                    vec![fixture.resource("shared-projection")],
                    move || {
                        maintenance_started.send(()).unwrap();
                        Ok(Step::Complete(()))
                    },
                ),
                cancel(),
            )
            .unwrap();
        let (update_started_tx, update_started_rx) = mpsc::channel();
        let (update_release_tx, update_release_rx) = mpsc::channel();
        let update = scheduler
            .submit(
                OperationId::new("rpc-skills_update_many").unwrap(),
                Step::acquire(
                    Workload::ExternalIo,
                    vec![fixture.resource("shared-projection")],
                    move || {
                        update_started_tx.send(()).unwrap();
                        update_release_rx.recv().unwrap();
                        Ok(Step::Complete(()))
                    },
                ),
                cancel(),
            )
            .unwrap();

        update_started_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        compute_release_tx.send(()).unwrap();
        blocker
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(
            maintenance_seen
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );

        update_release_tx.send(()).unwrap();
        update
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        maintenance
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        scheduler.shutdown();
    }

    #[test]
    fn bounded_priority_overtakes_restore_fifo_progress_for_maintenance() {
        let fixture = Fixture::new();
        let scheduler = RequestScheduler::with_concurrency([1; 4]);
        let (compute_started_tx, compute_started_rx) = mpsc::channel();
        let (compute_release_tx, compute_release_rx) = mpsc::channel();
        let blocker = scheduler
            .submit(
                operation(),
                Step::acquire(Workload::Compute, vec![], move || {
                    compute_started_tx.send(()).unwrap();
                    compute_release_rx.recv().unwrap();
                    Ok(Step::Complete(()))
                }),
                cancel(),
            )
            .unwrap();
        compute_started_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();

        let (order, observed) = mpsc::channel();
        let maintenance_order = order.clone();
        let maintenance = scheduler
            .submit(
                OperationId::new("skills-reconcile").unwrap(),
                Step::acquire(
                    Workload::Compute,
                    vec![fixture.resource("shared-projection")],
                    move || {
                        maintenance_order.send("maintenance").unwrap();
                        Ok(Step::Complete(()))
                    },
                ),
                cancel(),
            )
            .unwrap();
        let mut updates = Vec::new();
        for _ in 0..=MAX_PRIORITY_OVERTAKES {
            let order = order.clone();
            updates.push(
                scheduler
                    .submit(
                        OperationId::new("rpc-skills_update_many").unwrap(),
                        Step::acquire(
                            Workload::ExternalIo,
                            vec![fixture.resource("shared-projection")],
                            move || {
                                order.send("update").unwrap();
                                Ok(Step::Complete(()))
                            },
                        ),
                        cancel(),
                    )
                    .unwrap(),
            );
        }

        for update in updates.iter().take(MAX_PRIORITY_OVERTAKES) {
            update
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
            assert_eq!(
                observed.recv_timeout(Duration::from_secs(2)).unwrap(),
                "update"
            );
        }
        assert!(updates[MAX_PRIORITY_OVERTAKES].try_recv().is_err());

        compute_release_tx.send(()).unwrap();
        blocker
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(2)).unwrap(),
            "maintenance"
        );
        maintenance
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        updates[MAX_PRIORITY_OVERTAKES]
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(2)).unwrap(),
            "update"
        );
        scheduler.shutdown();
    }

    #[test]
    fn execution_time_does_not_consume_next_stage_waiting_budget() {
        let scheduler = RequestScheduler::default();
        let first_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&first_calls);
        let response = scheduler
            .submit_until(
                operation(),
                Step::acquire(Workload::ExternalIo, vec![], move || {
                    calls.fetch_add(1, Ordering::AcqRel);
                    thread::sleep(Duration::from_millis(100));
                    Ok(Step::acquire(Workload::Interactive, vec![], || {
                        Ok(Step::Complete(42))
                    }))
                }),
                cancel(),
                Instant::now() + Duration::from_millis(70),
            )
            .unwrap();
        assert_eq!(
            response
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            42
        );
        assert_eq!(first_calls.load(Ordering::Acquire), 1);
        scheduler.shutdown();
    }

    #[test]
    fn cancellation_deadline_and_shutdown_release_queued_continuations() {
        let fixture = Fixture::new();
        let scheduler = RequestScheduler::default();
        let held = fixture.hold("busy");
        let token = cancel();
        let cancelled = scheduler
            .submit(
                operation(),
                Step::acquire(Workload::Compute, vec![fixture.resource("busy")], || {
                    Ok(Step::Complete(()))
                }),
                Arc::clone(&token),
            )
            .unwrap();
        token.store(true, Ordering::Release);
        assert!(matches!(
            cancelled.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(ScheduleError::Cancelled)
        ));
        let expired = scheduler
            .submit_until(
                operation(),
                Step::acquire(Workload::Compute, vec![fixture.resource("busy")], || {
                    Ok(Step::Complete(()))
                }),
                cancel(),
                Instant::now() + Duration::from_millis(20),
            )
            .unwrap();
        assert!(matches!(
            expired.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(ScheduleError::Deadline)
        ));
        let closed = scheduler
            .submit(
                operation(),
                Step::acquire(Workload::Compute, vec![fixture.resource("busy")], || {
                    Ok(Step::Complete(()))
                }),
                cancel(),
            )
            .unwrap();
        scheduler.shutdown();
        assert!(matches!(
            closed.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(ScheduleError::Closed)
        ));
        assert!(
            scheduler
                .inner
                .pending
                .iter()
                .all(|count| count.load(Ordering::Acquire) == 0)
        );
        drop(held);
        assert!(
            ResourceReservation::try_acquire(&[fixture.resource("busy")])
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn worker_shutdown_preserves_external_join_and_does_not_deadlock_pump() {
        let scheduler = RequestScheduler::default();
        let worker = scheduler.clone();
        let result = scheduler
            .submit(
                operation(),
                Step::acquire(Workload::Interactive, vec![], move || {
                    worker.shutdown();
                    Ok(Step::Complete(()))
                }),
                cancel(),
            )
            .unwrap();
        result
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        scheduler.shutdown();
        assert!(scheduler.inner.pump.lock().unwrap().is_none());
    }

    #[test]
    fn rejection_observer_runs_once_after_guard_release_for_cancel_deadline_and_shutdown() {
        for reason in ["cancel", "deadline", "shutdown"] {
            let fixture = Fixture::new();
            let _held = fixture.hold("busy");
            let scheduler = RequestScheduler::default();
            let job = Arc::new(crate::cancellable_job::CancellableJob::default());
            let guard = job.start().unwrap();
            let token = guard.token();
            if reason == "cancel" {
                token.store(true, Ordering::Release);
            }
            let (notified, events) = mpsc::channel();
            let observer_job = Arc::clone(&job);
            let result = scheduler
                .submit_with_rejection_until(
                    Workload::Compute,
                    operation(),
                    Step::acquire(
                        Workload::Compute,
                        vec![fixture.resource("busy")],
                        move || {
                            drop(guard);
                            Ok(Step::Complete(()))
                        },
                    ),
                    token,
                    Instant::now()
                        + if reason == "deadline" {
                            Duration::from_millis(20)
                        } else {
                            Duration::from_secs(2)
                        },
                    move |error| {
                        assert!(
                            observer_job.start().is_some(),
                            "task guard must be released before notification"
                        );
                        notified.send(error).unwrap();
                    },
                )
                .unwrap();
            if reason == "shutdown" {
                scheduler.shutdown();
            }
            assert!(
                result
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .is_err()
            );
            let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(match (reason, event) {
                ("cancel", ScheduleError::Cancelled)
                | ("deadline", ScheduleError::Deadline)
                | ("shutdown", ScheduleError::Closed) => true,
                _ => false,
            });
            scheduler.shutdown();
            assert!(matches!(
                events.try_recv(),
                Err(mpsc::TryRecvError::Disconnected)
            ));
        }
    }

    #[test]
    fn rejection_observer_covers_queue_full_and_closed_without_duplicate_notifications() {
        let fixture = Fixture::new();
        let _held = fixture.hold("busy");
        let scheduler = RequestScheduler::default();
        for _ in 0..PENDING_LIMIT {
            scheduler
                .submit(
                    operation(),
                    Step::acquire(Workload::Compute, vec![fixture.resource("busy")], || {
                        Ok(Step::Complete(()))
                    }),
                    cancel(),
                )
                .unwrap();
        }
        let (notified, events) = mpsc::channel();
        let result = scheduler.submit_with_rejection(
            operation(),
            Step::acquire(Workload::Compute, vec![], || Ok(Step::Complete(()))),
            cancel(),
            move |error| notified.send(error).unwrap(),
        );
        assert!(matches!(result, Err(ScheduleError::QueueFull)));
        assert!(matches!(events.recv().unwrap(), ScheduleError::QueueFull));
        assert!(matches!(
            events.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        scheduler.shutdown();
        let (notified, events) = mpsc::channel();
        let result = scheduler.submit_with_rejection(
            operation(),
            Step::acquire(Workload::Compute, vec![], || Ok(Step::Complete(()))),
            cancel(),
            move |error| notified.send(error).unwrap(),
        );
        assert!(matches!(result, Err(ScheduleError::Closed)));
        assert!(matches!(events.recv().unwrap(), ScheduleError::Closed));
        assert!(matches!(
            events.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn rejection_observer_is_not_called_for_success_but_reports_preparation_failure_once() {
        let scheduler = RequestScheduler::default();
        let (notified, events) = mpsc::channel();
        let result = scheduler
            .submit_with_rejection(
                operation(),
                Step::acquire(Workload::Prepare, vec![], || {
                    Ok(Step::acquire(Workload::ExternalIo, vec![], || {
                        Ok(Step::Complete(42))
                    }))
                }),
                cancel(),
                move |error| notified.send(error).unwrap(),
            )
            .unwrap();
        assert_eq!(
            result
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            42
        );
        let (failed, failures) = mpsc::channel();
        let result = scheduler
            .submit_with_rejection(
                operation(),
                Step::acquire(Workload::Prepare, vec![], || -> anyhow::Result<Step<()>> {
                    anyhow::bail!("preparation failed")
                }),
                cancel(),
                move |error| failed.send(error).unwrap(),
            )
            .unwrap();
        assert!(
            result
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        assert!(matches!(
            failures.recv_timeout(Duration::from_secs(2)).unwrap(),
            ScheduleError::Failed(_)
        ));
        scheduler.shutdown();
        assert!(matches!(
            events.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert!(matches!(
            failures.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
