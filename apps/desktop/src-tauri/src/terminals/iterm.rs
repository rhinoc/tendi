#[cfg(not(target_os = "macos"))]
use super::open_command_file;
use super::{TerminalProvider, app_available, applescript_quote, command_status, terminal_script};

pub(crate) struct Iterm;

impl TerminalProvider for Iterm {
    fn id(&self) -> &str {
        "iterm"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["iterm2"]
    }
    fn available(&self) -> bool {
        app_available(&["/Applications/iTerm.app", "/Applications/iTerm2.app"])
    }
    fn application_name(&self) -> String {
        "iTerm".to_string()
    }
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), String> {
        let script = terminal_script(command);
        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("osascript")
                .args([
                    "-e",
                    "tell application \"iTerm\"",
                    "-e",
                    "activate",
                    "-e",
                    "set newWindow to (create window with default profile)",
                    "-e",
                    "tell current session of newWindow",
                    &format!("write text \"{}\"", applescript_quote(&script)),
                    "-e",
                    "end tell",
                    "-e",
                    "end tell",
                ])
                .status()
                .map_err(|err| format!("failed to open iTerm: {err}"))?;
            return command_status(status, "open iTerm");
        }
        #[cfg(not(target_os = "macos"))]
        {
            open_command_file("iTerm", &script)
        }
    }
}
