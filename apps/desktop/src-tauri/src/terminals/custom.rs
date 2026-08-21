use super::{TerminalProvider, open_command_file, terminal_script};

pub(crate) struct Custom(pub String);

impl TerminalProvider for Custom {
    fn id(&self) -> &str {
        &self.0
    }
    fn available(&self) -> bool {
        true
    }
    fn application_name(&self) -> String {
        self.0.clone()
    }
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), String> {
        open_command_file(&self.0, &terminal_script(command))
    }
}
