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
#[path = "writer_tests.rs"]
mod tests;
