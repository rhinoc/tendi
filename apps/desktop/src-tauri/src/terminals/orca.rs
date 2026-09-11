use std::process::Command;

use super::{TerminalLaunchError, TerminalProvider, app_available, orca_cli_path, shell_command};

pub(crate) struct OrcaProvider;

fn classify_create_failure(status: &str, detail: &str) -> TerminalLaunchError {
    if detail == "selector_not_found" {
        return TerminalLaunchError::worktree_not_found(detail);
    }
    if detail.is_empty() {
        return TerminalLaunchError::launch_failed(format!(
            "Orca terminal create exited with {status}"
        ));
    }
    TerminalLaunchError::launch_failed(format!("Orca terminal create failed: {detail}"))
}

impl TerminalProvider for OrcaProvider {
    fn id(&self) -> &str {
        "orca"
    }
    fn available(&self) -> bool {
        app_available(&["/Applications/Orca.app"]) && orca_cli_path().is_some()
    }
    fn application_name(&self) -> String {
        "Orca".to_string()
    }
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), TerminalLaunchError> {
        let cli = orca_cli_path().ok_or_else(|| {
            TerminalLaunchError::terminal_unavailable(
                "Orca CLI not found; install Orca.app or add its bundled CLI to PATH",
            )
        })?;
        let mut process = Command::new(&cli);
        process
            .args(["terminal", "create", "--command"])
            .arg(shell_command(command))
            .arg("--focus");
        if let Some(cwd) = &command.cwd {
            process
                .arg("--worktree")
                .arg(format!("path:{}", cwd.display()));
        }
        let output = process.output().map_err(|err| {
            TerminalLaunchError::launch_failed(format!("failed to create terminal in Orca: {err}"))
        })?;
        if output.status.success() {
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(classify_create_failure(&output.status.to_string(), &detail))
    }
}

#[cfg(test)]
mod tests {
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
}
