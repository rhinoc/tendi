#[cfg(not(target_os = "macos"))]
use super::open_command_file;
use super::{TerminalProvider, app_available, applescript_quote, command_status, terminal_script};

pub(crate) struct Terminal;

impl TerminalProvider for Terminal {
    fn id(&self) -> &str {
        "terminal"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["auto"]
    }
    fn available(&self) -> bool {
        app_available(&[
            "/System/Applications/Utilities/Terminal.app",
            "/Applications/Utilities/Terminal.app",
        ])
    }
    fn application_name(&self) -> String {
        "Terminal".to_string()
    }
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), String> {
        let script = terminal_script(command);
        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("osascript")
                .args([
                    "-e",
                    "tell application \"Terminal\"",
                    "-e",
                    "activate",
                    "-e",
                    &format!("do script \"{}\"", applescript_quote(&script)),
                    "-e",
                    "end tell",
                ])
                .status()
                .map_err(|err| format!("failed to open Terminal: {err}"))?;
            return command_status(status, "open Terminal");
        }
        #[cfg(not(target_os = "macos"))]
        {
            open_command_file("Terminal", &script)
        }
    }
}
