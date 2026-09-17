use super::*;
#[test]
fn queued_job_keeps_its_cancellation_and_next_job_gets_a_fresh_token() {
    let job = CancellableJob::default();
    assert!(!job.cancel());
    let first = job.start().unwrap();
    assert!(job.cancel());
    assert!(job.start().is_none());
    assert!(first.cancelled().load(Ordering::Acquire));
    drop(first);
    let next = job.start().unwrap();
    assert!(!next.cancelled().load(Ordering::Acquire));
}
