use std::{path::Path, process::Command, thread, time::Duration};

use serde::Deserialize;

use super::{TerminalLaunchError, TerminalProvider, open_terminal_application};

const CLI_PATH: &str =
    "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex";

pub(crate) struct RexBetaProvider;

#[derive(Deserialize)]
struct ClientList {
    clients: Vec<Client>,
}

#[derive(Deserialize)]
struct Client {
    client_id: String,
    info: Option<ClientInfo>,
    principal: Principal,
    #[serde(default)]
    connection_state: String,
}

#[derive(Deserialize)]
struct ClientInfo {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct Principal {
    transport: String,
}

fn cli_command() -> Result<Command, TerminalLaunchError> {
    let socket = dirs::data_dir()
        .ok_or_else(|| TerminalLaunchError::terminal_unavailable("Rex state directory not found"))?
        .join("rex/server.sock");
    let mut command = Command::new(CLI_PATH);
    command
        .arg("--server")
        .arg(socket)
        .env_remove("REX_SESSION")
        .env_remove("REX_BLOCK")
        .env_remove("REX_CLIENT");
    Ok(command)
}

fn cli_output(command: &mut Command) -> Result<Vec<u8>, TerminalLaunchError> {
    let output = command.output().map_err(|error| {
        TerminalLaunchError::launch_failed(format!("failed to run Rex Beta CLI: {error}"))
    })?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(TerminalLaunchError::launch_failed(format!(
        "Rex Beta CLI exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn local_client(bytes: &[u8]) -> Result<Option<String>, TerminalLaunchError> {
    let list: ClientList = serde_json::from_slice(bytes).map_err(|error| {
        TerminalLaunchError::launch_failed(format!("invalid Rex Beta client list: {error}"))
    })?;
    let mut clients = list.clients.into_iter().filter(|client| {
        client.principal.transport == "unix"
            && client.connection_state != "lost"
            && client
                .info
                .as_ref()
                .is_some_and(|info| info.kind == "app" && info.name == "Rex Beta")
    });
    let client = clients.next();
    if clients.next().is_some() {
        return Err(TerminalLaunchError::launch_failed(
            "multiple local Rex Beta app clients are connected",
        ));
    }
    Ok(client.map(|client| client.client_id))
}

fn wait_for_client() -> Result<String, TerminalLaunchError> {
    for _ in 0..50 {
        let bytes = cli_output(cli_command()?.args(["client", "ls", "--json"]))?;
        if let Some(client) = local_client(&bytes)? {
            return Ok(client);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(TerminalLaunchError::launch_failed(
        "Rex Beta app did not connect to its local server",
    ))
}

fn new_session_command(
    session: &tendi_core::SessionCommand,
) -> Result<Command, TerminalLaunchError> {
    let mut command = cli_command()?;
    command.args(["new", "--json"]);
    if let Some(cwd) = &session.cwd {
        command.arg("--cwd").arg(cwd);
    }
    command.arg("--");
    if !session.env.is_empty() {
        command.arg("/usr/bin/env");
        for (key, value) in &session.env {
            command.arg(format!("{key}={value}"));
        }
    }
    command.arg(&session.executable).args(&session.args);
    Ok(command)
}

impl TerminalProvider for RexBetaProvider {
    fn id(&self) -> &str {
        "rex-beta"
    }

    fn available(&self) -> bool {
        Path::new(CLI_PATH).is_file()
    }

    fn application_name(&self) -> String {
        "Rex Beta".to_string()
    }

    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), TerminalLaunchError> {
        if !self.available() {
            return Err(TerminalLaunchError::terminal_unavailable(
                "Rex Beta bundled CLI not found; install Rex Beta.app",
            ));
        }
        open_terminal_application("Rex Beta").map_err(TerminalLaunchError::launch_failed)?;
        let client = wait_for_client()?;
        let bytes = cli_output(&mut new_session_command(command)?)?;
        #[derive(Deserialize)]
        struct CreatedSession {
            session_id: String,
        }
        let session: CreatedSession = serde_json::from_slice(&bytes).map_err(|error| {
            TerminalLaunchError::launch_failed(format!(
                "invalid Rex Beta session response: {error}"
            ))
        })?;
        let selection = cli_output(cli_command()?.args([
            "-C",
            &client,
            "do",
            "session.select",
            &format!("session_id={}", session.session_id),
        ]));
        if let Err(error) = selection {
            if let Err(cleanup) = cli_output(cli_command()?.args(["kill", &session.session_id])) {
                return Err(TerminalLaunchError::launch_failed(format!(
                    "{}; failed to remove Rex Beta session {}: {}",
                    error.detail, session.session_id, cleanup.detail
                )));
            }
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "rex_beta_tests.rs"]
mod tests;
