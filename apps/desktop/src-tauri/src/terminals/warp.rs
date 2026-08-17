use super::{TerminalProvider, app_available, open_command_file, terminal_script};

pub(crate) struct WarpProvider;

impl TerminalProvider for WarpProvider {
    fn id(&self) -> &str {
        "warp"
    }
    fn label(&self) -> String {
        "Warp".to_string()
    }
    fn available(&self) -> bool {
        app_available(&["/Applications/Warp.app"])
    }
    fn application_name(&self) -> String {
        "Warp".to_string()
    }
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), String> {
        open_command_file("Warp", &terminal_script(command))
    }
}
