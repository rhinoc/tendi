use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value;

use super::{TerminalLaunchError, TerminalProvider, app_available, shell_command};

const SUPERSET_APP_PATHS: &[&str] = &["/Applications/Superset.app"];

pub(crate) struct SupersetProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SupersetWorkspace {
    organization_id: String,
    workspace_id: String,
}

fn superset_cli_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(env::split_paths(&path).map(|dir| dir.join("superset")));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".superset/bin/superset"));
        candidates.push(home.join("Applications/Superset.app/Contents/Resources/bin/superset"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn workspace_id_for_path(value: &Value, cwd: &Path) -> Option<String> {
    let workspaces = value
        .as_array()
        .or_else(|| value.get("workspaces").and_then(Value::as_array))?;
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    workspaces.iter().find_map(|workspace| {
        let path = workspace
            .get("worktreePath")
            .or_else(|| workspace.get("worktree_path"))
            .and_then(Value::as_str)?;
        let path = PathBuf::from(path)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(path));
        if path != cwd {
            return None;
        }
        workspace
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .map(str::to_string)
    })
}

fn organization_ids(value: &Value) -> Result<Vec<String>, String> {
    let organizations = value
        .as_array()
        .or_else(|| value.get("organizations").and_then(Value::as_array))
        .ok_or_else(|| {
            "Superset organization lookup returned an unexpected JSON shape".to_string()
        })?;
    let ids = organizations
        .iter()
        .filter_map(|organization| organization.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err("Superset organization lookup returned no organizations".to_string());
    }
    Ok(ids)
}

fn command_for_organization(cli: &Path, organization_id: &str) -> Command {
    let mut command = Command::new(cli);
    command.env("SUPERSET_ORGANIZATION_ID", organization_id);
    command
}

fn workspace_for_path(cli: &Path, cwd: &Path) -> Result<SupersetWorkspace, TerminalLaunchError> {
    let organizations = Command::new(cli)
        .args(["organization", "list", "--json"])
        .output()
        .map_err(|error| {
            TerminalLaunchError::launch_failed(format!(
                "failed to list Superset organizations: {error}"
            ))
        })?;
    if !organizations.status.success() {
        return Err(command_error("organization lookup", &organizations));
    }
    let value: Value = serde_json::from_slice(&organizations.stdout).map_err(|error| {
        TerminalLaunchError::launch_failed(format!(
            "Superset organization lookup returned invalid JSON: {error}"
        ))
    })?;
    let organization_ids = organization_ids(&value).map_err(TerminalLaunchError::launch_failed)?;

    for organization_id in organization_ids {
        let workspaces = command_for_organization(cli, &organization_id)
            .args(["workspaces", "list", "--json"])
            .output()
            .map_err(|error| {
                TerminalLaunchError::launch_failed(format!(
                    "failed to list Superset workspaces for organization {organization_id}: {error}"
                ))
            })?;
        if !workspaces.status.success() {
            return Err(command_error(
                &format!("workspace lookup for organization {organization_id}"),
                &workspaces,
            ));
        }
        let value: Value = serde_json::from_slice(&workspaces.stdout).map_err(|error| {
            TerminalLaunchError::launch_failed(format!(
                "Superset workspace lookup for organization {organization_id} returned invalid JSON: {error}"
            ))
        })?;
        if let Some(workspace_id) = workspace_id_for_path(&value, cwd) {
            return Ok(SupersetWorkspace {
                organization_id,
                workspace_id,
            });
        }
    }

    Err(TerminalLaunchError::worktree_not_found(format!(
        "no Superset workspace matches {} in any organization",
        cwd.display()
    )))
}

fn command_error(action: &str, output: &std::process::Output) -> TerminalLaunchError {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if detail.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        detail
    };
    let detail = if detail.is_empty() {
        format!("{action} exited with {}", output.status)
    } else {
        detail
    };
    TerminalLaunchError::launch_failed(format!("Superset {action} failed: {detail}"))
}

impl TerminalProvider for SupersetProvider {
    fn id(&self) -> &str {
        "superset"
    }

    fn available(&self) -> bool {
        app_available(SUPERSET_APP_PATHS) && superset_cli_path().is_some()
    }

    fn application_name(&self) -> String {
        "Superset".to_string()
    }

    fn launch(&self, command: &tendi_core::SessionCommand) -> Result<(), TerminalLaunchError> {
        let cli = superset_cli_path().ok_or_else(|| {
            TerminalLaunchError::terminal_unavailable(
                "Superset CLI not found; install Superset.app or add the Superset CLI to PATH",
            )
        })?;
        let cwd = command.cwd.as_ref().ok_or_else(|| {
            TerminalLaunchError::worktree_not_found(
                "Superset requires a local workspace path to create a terminal",
            )
        })?;
        let workspace = workspace_for_path(&cli, cwd)?;
        let output = command_for_organization(&cli, &workspace.organization_id)
            .args([
                "terminals",
                "create",
                "--workspace",
                &workspace.workspace_id,
                "--cwd",
                &cwd.display().to_string(),
                "--command",
            ])
            .arg(shell_command(command))
            .output()
            .map_err(|error| {
                TerminalLaunchError::launch_failed(format!(
                    "failed to create a Superset terminal: {error}"
                ))
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_error("terminal creation", &output))
        }
    }
}

#[cfg(test)]
#[path = "superset_tests.rs"]
mod tests;
