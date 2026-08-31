use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{providers::ProviderContext, skills::AgentKind};

const VERSION_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentRecord {
    pub kind: AgentKind,
    pub name: String,
    pub installed: bool,
    pub config_dir: Option<PathBuf>,
    pub executable: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentScan {
    pub agents: Vec<AgentRecord>,
    pub warnings: Vec<String>,
}

pub fn scan_agents(cwd: &Path) -> Result<AgentScan> {
    let ctx = ProviderContext::new(cwd);
    let agents = crate::providers::agent_providers()
        .into_iter()
        .filter_map(|provider| {
            Some(detect_agent(
                provider.kind(),
                provider.display_name()?,
                provider.config_dir(&ctx),
                provider.executable_names(),
            ))
        })
        .collect();

    Ok(AgentScan {
        agents,
        warnings: Vec::new(),
    })
}

fn detect_agent(
    kind: AgentKind,
    name: &str,
    config_dir: Option<PathBuf>,
    executable_names: &[&str],
) -> AgentRecord {
    let executable = executable_names.iter().find_map(|name| which(name));
    let version = executable
        .as_ref()
        .and_then(|path| command_version(path))
        .or_else(|| {
            let app_path = crate::providers::agent_provider(kind).app_bundle_path()?;
            app_version(app_path)
        });
    let config_exists = config_dir.as_ref().is_some_and(|path| path.is_dir());

    AgentRecord {
        kind,
        name: name.to_string(),
        installed: config_exists || executable.is_some(),
        config_dir: config_dir.filter(|path| path.exists()),
        executable,
        version,
    }
}

fn which(name: &str) -> Option<String> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.display().to_string());
        }
    }
    None
}

fn command_version(path: &str) -> Option<String> {
    let output = run_command_with_timeout(path, &["--version"])?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.lines().next().map(str::trim).map(str::to_string)
}

fn app_version(app_path: &str) -> Option<String> {
    let output = run_command_with_timeout("mdls", &["-name", "kMDItemVersion", "-raw", app_path])?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty() && value != "(null)").then_some(value)
}

fn run_command_with_timeout(program: &str, args: &[&str]) -> Option<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + VERSION_COMMAND_TIMEOUT;

    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
