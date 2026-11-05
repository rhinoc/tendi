#[cfg(not(target_os = "macos"))]
use super::open_command_file;
use super::{
    TerminalLaunchError, TerminalProvider, app_available, applescript_quote, command_status,
    terminal_script,
};

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
    #[cfg(target_os = "macos")]
    fn focus_process(&self, pid: i32) -> Result<bool, TerminalLaunchError> {
        let Some(tty) = super::process_tty(pid)? else { return Ok(false) };
        super::focus_with_applescript("iTerm", r#"
on run argv
    set targetTTY to item 1 of argv
    tell application "iTerm"
        repeat with win in windows
            repeat with tabItem in tabs of win
                repeat with term in sessions of tabItem
                    if tty of term is targetTTY then
                        select win
                        select tabItem
                        select term
                        activate
                        return "found"
                    end if
                end repeat
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
                .map_err(|err| {
                    TerminalLaunchError::launch_failed(format!("failed to open iTerm: {err}"))
                })?;
            return command_status(status, "open iTerm")
                .map_err(TerminalLaunchError::launch_failed);
        }
        #[cfg(not(target_os = "macos"))]
        {
            open_command_file("iTerm", &script).map_err(TerminalLaunchError::launch_failed)
        }
    }
}
