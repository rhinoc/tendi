use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use crate::{
    fsutil::{atomic_write, sha256_text},
    skills::AgentKind,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigFile {
    pub agent: AgentKind,
    pub label: String,
    pub path: PathBuf,
    pub format: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigContent {
    pub path: PathBuf,
    pub content: String,
    pub sha256: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// Successful saves omit echoed content; callers already hold the bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigWriteResult {
    pub path: PathBuf,
    pub sha256: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug)]
pub struct ConfigChangedError {
    pub current: AgentConfigContent,
}

impl std::fmt::Display for ConfigChangedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "config changed on disk; review it before saving")
    }
}

impl std::error::Error for ConfigChangedError {}

pub fn list_agent_configs() -> Result<Vec<AgentConfigFile>> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    Ok(configs_for_environment(&home))
}

pub fn read_agent_config(path: &Path) -> Result<AgentConfigContent> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let config = resolve_config_for_path(&home, path)?;
    read_config(&config)
}

pub fn save_agent_config(
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigWriteResult> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    let config = resolve_config_for_path(&home, path)?;
    save_config(&config, expected_sha256, content)
}

pub fn delete_agent_configs(paths: &[PathBuf]) -> Result<()> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    delete_configs_for_home(&home, paths)
}

pub fn create_config_profile(
    agent: AgentKind,
    name: &str,
    content: &str,
) -> Result<AgentConfigFile> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    create_profile_for_roots(agent, &home, name, content)
}

pub fn config_profile_exists(agent: AgentKind, name: &str) -> Result<bool> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    Ok(config_profile_path_for_roots(agent, &home, name)?.is_file())
}

pub fn config_profile_path(agent: AgentKind, name: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("home directory is unavailable")?;
    config_profile_path_for_roots(agent, &home, name)
}

pub fn validate_profile_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid config profile name; use letters, numbers, hyphens, or underscores");
    }
    Ok(())
}

fn configs_for_environment(home: &Path) -> Vec<AgentConfigFile> {
    configs_for_roots(home)
}

fn configs_for_roots(home: &Path) -> Vec<AgentConfigFile> {
    let mut configs = crate::providers::agent_providers()
        .into_iter()
        .flat_map(|provider| provider.config_files(home, &provider.config_home(home)))
        .collect::<Vec<_>>();
    for config in &mut configs {
        config.updated_at = file_updated_at(&config.path);
    }
    configs.sort_by_key(|config| crate::providers::agent_provider(config.agent).config_order());
    configs
}

#[cfg(test)]
fn configs_for_roots_with_codex_home(home: &Path, codex_home: &Path) -> Vec<AgentConfigFile> {
    let mut configs = crate::providers::agent_providers()
        .into_iter()
        .flat_map(|provider| {
            provider.config_files(home, &provider.config_home_for_test(home, codex_home))
        })
        .collect::<Vec<_>>();
    for config in &mut configs {
        config.updated_at = file_updated_at(&config.path);
    }
    configs.sort_by_key(|config| crate::providers::agent_provider(config.agent).config_order());
    configs
}

pub(crate) fn profile_paths_for_root(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?.strip_suffix(suffix)?;
            validate_profile_name(name).ok().map(|_| name.to_string())
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|profile| root.join(format!("{profile}{suffix}")))
        .collect()
}

fn config_profile_path_for_roots(agent: AgentKind, home: &Path, name: &str) -> Result<PathBuf> {
    validate_profile_name(name)?;
    let provider = crate::providers::agent_provider(agent);
    let agent_home = provider.config_home(home);
    provider
        .config_profile_path(home, &agent_home, name)
        .ok_or_else(|| anyhow::anyhow!("config profiles are not supported for this agent"))
}

fn resolve_config_for_path(home: &Path, path: &Path) -> Result<AgentConfigFile> {
    crate::providers::agent_providers()
        .into_iter()
        .find_map(|provider| {
            let agent_home = provider.config_home(home);
            provider.config_file_for_path(home, &agent_home, path)
        })
        .ok_or_else(|| anyhow::anyhow!("unsupported agent config path: {}", path.display()))
}

#[cfg(test)]
fn resolve_config_for_roots(
    home: &Path,
    codex_home: &Path,
    path: &Path,
) -> Result<AgentConfigFile> {
    crate::providers::agent_providers()
        .into_iter()
        .find_map(|provider| {
            let agent_home = provider.config_home_for_test(home, codex_home);
            provider.config_file_for_path(home, &agent_home, path)
        })
        .ok_or_else(|| anyhow::anyhow!("unsupported agent config path: {}", path.display()))
}

#[cfg(test)]
fn read_config_from_home(home: &Path, path: &Path) -> Result<AgentConfigContent> {
    let config = resolve_config_for_path(home, path)?;
    read_config(&config)
}

fn read_config(config: &AgentConfigFile) -> Result<AgentConfigContent> {
    let exists = config.path.is_file();
    let content = if exists {
        fs::read_to_string(&config.path)
            .with_context(|| format!("failed to read {}", config.path.display()))?
    } else if config.format == "json" {
        "{}\n".to_string()
    } else {
        String::new()
    };
    let updated_at = file_updated_at(&config.path);
    Ok(AgentConfigContent {
        path: config.path.clone(),
        sha256: sha256_text(if exists { &content } else { "" }),
        content,
        exists,
        updated_at,
    })
}

#[cfg(test)]
fn save_config_from_home(
    home: &Path,
    path: &Path,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigWriteResult> {
    let config = resolve_config_for_path(home, path)?;
    save_config(&config, expected_sha256, content)
}

fn save_config(
    config: &AgentConfigFile,
    expected_sha256: &str,
    content: &str,
) -> Result<AgentConfigWriteResult> {
    validate_config(&config.format, content)?;
    let _resources =
        crate::coordination::acquire_file_resources(std::slice::from_ref(&config.path))?;
    let current = if config.path.is_file() {
        fs::read_to_string(&config.path)
            .with_context(|| format!("failed to read {}", config.path.display()))?
    } else {
        String::new()
    };
    if sha256_text(&current) != expected_sha256 {
        return Err(ConfigChangedError {
            current: read_config(config)?,
        }
        .into());
    }
    atomic_write(&config.path, content)?;
    let updated_at = file_updated_at(&config.path);
    Ok(AgentConfigWriteResult {
        path: config.path.clone(),
        sha256: sha256_text(content),
        exists: true,
        updated_at,
    })
}

fn delete_configs_for_home(home: &Path, paths: &[PathBuf]) -> Result<()> {
    let configs = paths
        .iter()
        .map(|path| resolve_config_for_path(home, path))
        .collect::<Result<Vec<_>>>()?;
    let _resources = crate::coordination::acquire_file_resources(
        &configs
            .iter()
            .map(|config| config.path.clone())
            .collect::<Vec<_>>(),
    )?;
    for config in &configs {
        if !config.path.is_file() {
            bail!("config file not found: {}", config.path.display());
        }
    }
    for config in configs {
        fs::remove_file(&config.path)
            .with_context(|| format!("failed to delete {}", config.path.display()))?;
    }
    Ok(())
}

fn create_profile_for_roots(
    agent: AgentKind,
    home: &Path,
    name: &str,
    content: &str,
) -> Result<AgentConfigFile> {
    let format = crate::providers::agent_provider(agent)
        .config_profile_format()
        .ok_or_else(|| anyhow::anyhow!("config profiles are not supported for this agent"))?;
    let path = config_profile_path_for_roots(agent, home, name)?;
    validate_config(format, content)?;
    let _resources = crate::coordination::acquire_file_resources(std::slice::from_ref(&path))?;
    if fs::symlink_metadata(&path).is_ok() {
        bail!("config profile already exists: {name}");
    }
    atomic_write(&path, content)?;
    let mut config = resolve_config_for_path(home, &path)?;
    config.exists = true;
    config.updated_at = file_updated_at(&config.path);
    Ok(config)
}

fn file_updated_at(path: &Path) -> Option<String> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .map(|modified| {
            DateTime::<Utc>::from(modified).to_rfc3339_opts(SecondsFormat::Millis, true)
        })
}

fn validate_config(format: &str, content: &str) -> Result<()> {
    match format {
        "json" => {
            serde_json::from_str::<serde_json::Value>(content).context("invalid JSON config")?;
        }
        "toml" => {
            toml::from_str::<toml::Value>(content).context("invalid TOML config")?;
        }
        _ => bail!("unsupported config format: {format}"),
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
