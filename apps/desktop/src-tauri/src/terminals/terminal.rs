#[cfg(not(target_os = "macos"))]
use super::open_command_file;
use super::{
    TerminalLaunchError, TerminalProvider, app_available, applescript_quote, command_status,
    terminal_script,
};

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
    #[cfg(target_os = "macos")]
    fn focus_process(&self, pid: i32) -> Result<bool, TerminalLaunchError> {
        let Some(tty) = super::process_tty(pid)? else { return Ok(false) };
        super::focus_with_applescript("Terminal", r#"
on run argv
    set targetTTY to item 1 of argv
    tell application "Terminal"
        repeat with win in windows
            repeat with term in tabs of win
                if tty of term is targetTTY then
                    set selected of term to true
                    set frontmost of win to true
                    activate
                    return "found"
                end if
            end repeat
        end repeat
    end tell
    return "not found"
end run
"#, &tty)
    }
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), TerminalLaunchError> {
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
                .map_err(|err| {
                    TerminalLaunchError::launch_failed(format!("failed to open Terminal: {err}"))
                })?;
            return command_status(status, "open Terminal")
                .map_err(TerminalLaunchError::launch_failed);
        }
        #[cfg(not(target_os = "macos"))]
        {
            open_command_file("Terminal", &script).map_err(TerminalLaunchError::launch_failed)
        }
    }
}
