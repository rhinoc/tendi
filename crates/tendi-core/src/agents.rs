use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    providers::{AgentProvider, ProviderContext},
    skills::AgentKind,
};

const VERSION_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentRecord {
    pub kind: AgentKind,
    pub name: String,
    pub installed: bool,
    pub config_dir: Option<PathBuf>,
    pub executable: Option<String>,
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
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
        .filter_map(|provider| detect_agent(provider, &ctx))
        .collect();

    Ok(AgentScan {
        agents,
        warnings: Vec::new(),
    })
}

fn detect_agent(provider: &dyn AgentProvider, ctx: &ProviderContext) -> Option<AgentRecord> {
    let name = provider.display_name()?;
    let config_dir = provider.config_dir(ctx).filter(|path| path.is_dir());
    let executable = provider.executable_path(ctx);
    let app = provider
        .app_bundle_path()
        .filter(|path| Path::new(path).is_dir());
    let app_version = app.and_then(app_version);
    let version = executable
        .as_deref()
        .and_then(command_version)
        .or_else(|| app_version.clone());

    Some(AgentRecord {
        kind: provider.kind(),
        name: name.to_string(),
        installed: (provider.config_indicates_installation() && config_dir.is_some())
            || executable.is_some()
            || app.is_some(),
        config_dir,
        executable: executable.map(|path| path.to_string_lossy().into_owned()),
        version,
        app_version,
    })
}

pub(crate) fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn command_version(path: &Path) -> Option<String> {
    let output = run_command_with_timeout(path.as_os_str(), &["--version"])?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.lines().next().map(str::trim).map(str::to_string)
}

fn app_version(app_path: &str) -> Option<String> {
    let output = run_command_with_timeout(
        std::ffi::OsStr::new("/usr/libexec/PlistBuddy"),
        &[
            "-c",
            "Print :CFBundleShortVersionString",
            &format!("{app_path}/Contents/Info.plist"),
        ],
    )?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty() && value != "(null)").then_some(value)
}

fn run_command_with_timeout(program: &std::ffi::OsStr, args: &[&str]) -> Option<Output> {
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
