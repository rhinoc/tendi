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
                worker_coordinator.execute(OperationId::new("closed-child").unwrap(), || Ok(())),
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
