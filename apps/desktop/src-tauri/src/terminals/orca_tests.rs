use super::super::TerminalLaunchErrorCode;
use super::classify_create_failure;

#[test]
fn classifies_missing_worktree_selector() {
    let error = classify_create_failure("exit status: 1", "selector_not_found");
    assert_eq!(error.code, TerminalLaunchErrorCode::WorktreeNotFound);
    assert_eq!(error.detail, "selector_not_found");
}

#[test]
fn keeps_other_orca_failures_as_retryable_launch_errors() {
    let error = classify_create_failure("exit status: 1", "connection refused");
    assert_eq!(error.code, TerminalLaunchErrorCode::LaunchFailed);
    assert_eq!(
        error.detail,
        "Orca terminal create failed: connection refused"
    );
}
