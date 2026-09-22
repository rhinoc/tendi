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

#[test]
fn session_skill_index_runs_as_background_compute() {
    assert_eq!(
        workload_for_request(
            "session_skill_index_run",
            &serde_json::json!({ "force": false }),
        ),
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
