use std::process::Command;

use super::{TerminalProvider, app_available, orca_cli_path, shell_command};

pub(crate) struct OrcaProvider;

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
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), String> {
        let cli = orca_cli_path().ok_or_else(|| {
            "Orca CLI not found; install Orca.app or add its bundled CLI to PATH".to_string()
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
        let output = process
            .output()
            .map_err(|err| format!("failed to create terminal in Orca: {err}"))?;
        if output.status.success() {
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.is_empty() {
            Err(format!(
                "Orca terminal create exited with {}",
                output.status
            ))
        } else {
            Err(format!("Orca terminal create failed: {detail}"))
        }
    }
}
