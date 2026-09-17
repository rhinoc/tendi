use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// A cancellation token belongs to one accepted job, including its queued phase.
/// Publishing the token and claiming the job are one operation, so cancellation
/// cannot be overwritten by a racing start.
#[derive(Debug, Default)]
pub(crate) struct CancellableJob {
    state: Arc<Mutex<Option<Arc<AtomicBool>>>>,
}

pub(crate) struct JobGuard {
    state: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    cancelled: Arc<AtomicBool>,
}

impl CancellableJob {
    pub fn start(&self) -> Option<JobGuard> {
        let mut current = self.state.lock().expect("cancellation state is healthy");
        if current.is_some() {
            return None;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *current = Some(Arc::clone(&cancelled));
        Some(JobGuard {
            state: Arc::clone(&self.state),
            cancelled,
        })
    }

    pub fn cancel(&self) -> bool {
        let current = self.state.lock().expect("cancellation state is healthy");
        if let Some(token) = current.as_ref() {
            token.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
}

impl JobGuard {
    pub fn token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }
    pub fn cancelled(&self) -> &AtomicBool {
        &self.cancelled
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        let mut current = self.state.lock().expect("cancellation state is healthy");
        if current
            .as_ref()
            .is_some_and(|token| Arc::ptr_eq(token, &self.cancelled))
        {
            current.take();
        }
    }
}

#[cfg(test)]
#[path = "cancellable_job_tests.rs"]
mod tests;
