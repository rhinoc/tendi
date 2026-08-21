use super::{TerminalProvider, app_available, open_command_file, terminal_script};

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
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), String> {
        open_command_file("Ghostty", &terminal_script(command))
    }
}
