use super::*;
#[test]
fn nested_writer_fails_without_waiting() {
    let queue = WriterQueue::default();
    let _turn = queue.acquire(Duration::from_secs(1)).unwrap();
    assert!(
        queue
            .acquire(Duration::from_secs(1))
            .err()
            .unwrap()
            .to_string()
            .contains("nested")
    );
}
#[test]
fn expired_request_is_removed_and_never_executes() {
    let queue = std::sync::Arc::new(WriterQueue::default());
    let turn = queue.acquire(Duration::from_secs(1)).unwrap();
    let waiting = queue.clone();
    assert!(
        std::thread::spawn(move || waiting.acquire(Duration::from_millis(10)).is_err())
            .join()
            .unwrap()
    );
    assert!(queue.state.lock().unwrap().waiting.is_empty());
    drop(turn);
    assert!(queue.acquire(Duration::from_secs(1)).is_ok());
}

#[test]
fn expired_deadline_is_checked_before_an_available_turn() {
    let queue = WriterQueue::default();
    let error = queue.acquire(Duration::ZERO).err().unwrap();
    assert_eq!(
        error.downcast_ref::<AdmissionError>(),
        Some(&AdmissionError::Deadline)
    );
    assert!(queue.state.lock().unwrap().waiting.is_empty());
}

#[test]
fn waiting_writers_receive_fifo_turns() {
    let queue = std::sync::Arc::new(WriterQueue::default());
    let first = queue.acquire(Duration::from_secs(5)).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let mut handles = Vec::new();
    for index in 0..4 {
        let next = queue.clone();
        let send = send.clone();
        handles.push(std::thread::spawn(move || {
            let _turn = next.acquire(Duration::from_secs(5)).unwrap();
            send.send(index).unwrap();
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while queue.state.lock().unwrap().waiting.len() != index + 1 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    drop(first);
    for expected in 0..4 {
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(5)).unwrap(),
            expected
        );
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn queue_capacity_rejects_without_allocating_another_request() {
    let queue = WriterQueue::default();
    queue
        .state
        .lock()
        .unwrap()
        .waiting
        .extend((0..MAX_WAITING_WRITERS as u64).map(|ticket| WaitingWriter {
            ticket,
            priority: WritePriority::Interactive,
        }));
    assert_eq!(
        queue
            .acquire(Duration::from_secs(1))
            .err()
            .unwrap()
            .downcast_ref::<AdmissionError>(),
        Some(&AdmissionError::Full)
    );
    assert_eq!(
        queue.state.lock().unwrap().waiting.len(),
        MAX_WAITING_WRITERS
    );
}

#[test]
fn interactive_burst_preserves_fifo_and_does_not_starve_background() {
    let queue = std::sync::Arc::new(WriterQueue::default());
    let held = queue.acquire(Duration::from_secs(5)).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let priorities = [
        WritePriority::Background,
        WritePriority::Background,
        WritePriority::Interactive,
        WritePriority::Interactive,
        WritePriority::Interactive,
        WritePriority::Interactive,
        WritePriority::Interactive,
    ];
    let mut workers = Vec::new();
    for (index, priority) in priorities.into_iter().enumerate() {
        let next = queue.clone();
        let send = send.clone();
        workers.push(std::thread::spawn(move || {
            let _turn = next.acquire_for(Duration::from_secs(5), priority).unwrap();
            send.send(index).unwrap();
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while queue.state.lock().unwrap().waiting.len() != index + 1 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    drop(held);
    for expected in [2, 3, 4, 0, 5, 6, 1] {
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(5)).unwrap(),
            expected
        );
    }
    for worker in workers {
        worker.join().unwrap();
    }
}
