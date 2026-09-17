use super::{
    TerminalLaunchError, TerminalProvider, app_available, applescript_quote, command_status,
    terminal_input_script,
};

#[cfg(not(target_os = "macos"))]
use super::open_command_file;

pub(crate) struct GhosttyProvider;

impl TerminalProvider for GhosttyProvider {
    fn id(&self) -> &str {
        "ghostty"
    }
    fn available(&self) -> bool {
        app_available(&["/Applications/Ghostty.app"])
    }
    fn application_name(&self) -> String {
        "Ghostty".to_string()
    }
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), TerminalLaunchError> {
        let script = terminal_input_script(command);
        #[cfg(target_os = "macos")]
        {
            let applescript = ghostty_applescript(&script);
            let status = std::process::Command::new("osascript")
                .args(["-e", applescript.as_str()])
                .status()
                .map_err(|err| {
                    TerminalLaunchError::launch_failed(format!("failed to open Ghostty: {err}"))
                })?;
            return command_status(status, "open Ghostty")
                .map_err(TerminalLaunchError::launch_failed);
        }
        #[cfg(not(target_os = "macos"))]
        {
            open_command_file("Ghostty", &script).map_err(TerminalLaunchError::launch_failed)
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
#[cfg(test)]
#[path = "ghostty_tests.rs"]
mod tests;

#[cfg(target_os = "macos")]
fn ghostty_applescript(script: &str) -> String {
    format!(
        "tell application \"Ghostty\"\nactivate\nset newWindow to new window\nset newTerminal to focused terminal of selected tab of newWindow\ninput text (\"{}\" & return) to newTerminal\nend tell",
        applescript_quote(script)
    )
}
