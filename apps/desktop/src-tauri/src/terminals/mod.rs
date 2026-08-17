use std::{env, fs, path::PathBuf, process::Command};

use serde::Serialize;

mod custom;
mod ghostty;
mod iterm;
mod orca;
mod terminal;
mod warp;

pub(crate) trait TerminalProvider {
    fn id(&self) -> &str;
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }
    fn matches(&self, value: &str) -> bool {
        self.id() == value || self.aliases().contains(&value)
    }
    fn label(&self) -> String;
    fn available(&self) -> bool;
    fn application_name(&self) -> String;
    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), String>;
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalApp {
    pub id: String,
    pub label: String,
    pub available: bool,
}

pub(crate) fn terminal_apps() -> Vec<TerminalApp> {
    let mut apps = vec![TerminalApp {
        id: "auto".to_string(),
        label: "Auto".to_string(),
        available: true,
    }];
    apps.extend(
        providers()
            .into_iter()
            .map(|provider| TerminalApp {
                id: provider.id().to_string(),
                label: provider.label(),
                available: provider.available(),
            })
            .collect::<Vec<_>>(),
    );
    apps
}

pub(crate) fn resolve_terminal(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized == "auto" {
        return providers()
            .into_iter()
            .find(|provider| provider.id() == "terminal" && provider.available())
            .map(|provider| provider.id().to_string())
            .unwrap_or_else(|| "auto".to_string());
    }
    provider_for(value.trim()).id().to_string()
}

pub(crate) fn terminal_application_name(terminal: &str) -> String {
    provider_for(terminal).application_name()
}

pub(crate) fn open_terminal_application(app_name: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("open")
            .args(["-a", app_name])
            .status()
            .map_err(|err| format!("failed to open {app_name}: {err}"))?;
        return command_status(status, &format!("open {app_name}"));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app_name;
        Err("testing terminal applications is only supported on macOS".to_string())
    }
}

pub(crate) fn launch_command_in_terminal(
    command: &tendi_core::SessionCommand,
    terminal: &str,
) -> Result<String, String> {
    provider_for(terminal).launch(command)?;
    Ok(shell_command(command))
}

fn providers() -> Vec<Box<dyn TerminalProvider>> {
    vec![
        Box::new(terminal::Terminal),
        Box::new(iterm::Iterm),
        Box::new(ghostty::GhosttyProvider),
        Box::new(warp::WarpProvider),
        Box::new(orca::OrcaProvider),
    ]
}

fn provider_for(value: &str) -> Box<dyn TerminalProvider> {
    let normalized = value.trim().to_ascii_lowercase();
    providers()
        .into_iter()
        .find(|provider| provider.matches(&normalized))
        .unwrap_or_else(|| Box::new(custom::Custom(value.trim().to_string())))
}

pub(super) fn app_available(paths: &[&str]) -> bool {
    paths.iter().any(|path| PathBuf::from(path).exists())
}

pub(super) fn terminal_script(command: &tendi_core::SessionCommand) -> String {
    match &command.cwd {
        Some(cwd) => format!(
            "cd {} && exec {}",
            shell_quote(&cwd.display().to_string()),
            shell_command(command)
        ),
        None => format!("exec {}", shell_command(command)),
    }
}

pub(super) fn shell_command(command: &tendi_core::SessionCommand) -> String {
    let command_line = std::iter::once(command.executable.as_str())
        .chain(command.args.iter().map(String::as_str))
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    if command.env.is_empty() {
        command_line
    } else {
        let env = command
            .env
            .iter()
            .map(|(key, value)| format!("{}={}", shell_quote(key), shell_quote(value)))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{env} {command_line}")
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub(super) fn applescript_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn open_command_file(app_name: &str, script: &str) -> Result<(), String> {
    let path = env::temp_dir().join(format!("tendi-session-{}.command", std::process::id()));
    fs::write(&path, format!("#!/bin/zsh\n{script}\n"))
        .map_err(|err| format!("failed to write command file: {err}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path)
            .map_err(|err| format!("failed to stat command file: {err}"))?
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions)
            .map_err(|err| format!("failed to make command file executable: {err}"))?;
    }

    #[cfg(target_os = "macos")]
    let status = Command::new("open")
        .args(["-a", app_name])
        .arg(&path)
        .status()
        .map_err(|err| format!("failed to open {app_name}: {err}"))?;

    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open")
        .arg(&path)
        .status()
        .map_err(|err| format!("failed to open command file: {err}"))?;

    #[cfg(windows)]
    let status = Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(&path)
        .status()
        .map_err(|err| format!("failed to open command file: {err}"))?;

    command_status(status, &format!("open {app_name}"))
}

pub(super) fn command_status(status: std::process::ExitStatus, action: &str) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("{action} exited with {status}"))
    }
}

pub(super) fn orca_cli_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&path).map(|dir| dir.join("orca")));
    }
    candidates.push(PathBuf::from(
        "/Applications/Orca.app/Contents/Resources/bin/orca",
    ));
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Applications/Orca.app/Contents/Resources/bin/orca"));
    }
    candidates.into_iter().find(|path| path.is_file())
}
