//! Bounded admission with FIFO order within each workload class.
//! Deadline cancellation happens before execution. Admitted requests always
//! return their actual commit/rollback outcome, never detach a late write.
use anyhow::Result;
use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex},
    thread::ThreadId,
    time::{Duration, Instant},
};

const MAX_WAITING_WRITERS: usize = 256;
const INTERACTIVE_BURST: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WritePriority {
    Interactive,
    /// Only independently rebuildable work with commit-time validation may use
    /// this class. Related business mutations must retain their command order.
    Background,
}

struct WaitingWriter {
    ticket: u64,
    priority: WritePriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionError {
    Reentrant,
    Full,
    Deadline,
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Reentrant => "database write transactions must not be nested",
            Self::Full => "database writer queue is full",
            Self::Deadline => {
                "database write cancelled before execution: writer queue deadline exceeded"
            }
        })
    }
}
impl std::error::Error for AdmissionError {}

#[derive(Default)]
struct QueueState {
    owner: Option<ThreadId>,
    next_ticket: u64,
    waiting: VecDeque<WaitingWriter>,
    interactive_streak: usize,
}

impl QueueState {
    fn next_index(&self) -> Option<usize> {
        let interactive = self
            .waiting
            .iter()
            .position(|writer| writer.priority == WritePriority::Interactive);
        let background = self
            .waiting
            .iter()
            .position(|writer| writer.priority == WritePriority::Background);
        if self.interactive_streak >= INTERACTIVE_BURST {
            background.or(interactive)
        } else {
            interactive.or(background)
        }
    }
}

#[derive(Default)]
pub(super) struct WriterQueue {
    state: Mutex<QueueState>,
    changed: Condvar,
}
pub(super) struct WriterTurn<'a>(&'a WriterQueue);

impl WriterQueue {
    #[cfg(test)]
    pub(super) fn acquire(&self, timeout: Duration) -> Result<WriterTurn<'_>> {
        self.acquire_for(timeout, WritePriority::Interactive)
    }

    pub(super) fn acquire_for(
        &self,
        timeout: Duration,
        priority: WritePriority,
    ) -> Result<WriterTurn<'_>> {
        let deadline = Instant::now() + timeout;
        let thread = std::thread::current().id();
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("database writer queue poisoned"))?;
        if state.owner == Some(thread) {
            return Err(AdmissionError::Reentrant.into());
        }
        if state.waiting.len() >= MAX_WAITING_WRITERS {
            return Err(AdmissionError::Full.into());
        }
        let ticket = state.next_ticket;
        state.next_ticket = state.next_ticket.wrapping_add(1);
        state.waiting.push_back(WaitingWriter { ticket, priority });
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.waiting.retain(|waiting| waiting.ticket != ticket);
                self.changed.notify_all();
                return Err(AdmissionError::Deadline.into());
            }
            let next = state.next_index();
            if state.owner.is_none()
                && next.is_some_and(|index| state.waiting[index].ticket == ticket)
            {
                state.waiting.remove(next.expect("selected writer exists"));
                state.interactive_streak = if priority == WritePriority::Interactive
                    && state
                        .waiting
                        .iter()
                        .any(|writer| writer.priority == WritePriority::Background)
                {
                    (state.interactive_streak + 1).min(INTERACTIVE_BURST)
                } else {
                    0
                };
                state.owner = Some(thread);
                return Ok(WriterTurn(self));
            }
            (state, _) = self
                .changed
                .wait_timeout(state, remaining)
                .map_err(|_| anyhow::anyhow!("database writer queue poisoned"))?;
        }
    }
}

impl Drop for WriterTurn<'_> {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.owner = None;
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
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
}
